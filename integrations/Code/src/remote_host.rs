//! Optional remote transport, sharing the desktop's exact execution authority.
//! The transport is a lazy stdlib helper; no provider lives outside this host.
use super::*;
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    cell::RefCell,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
};

pub(super) struct Service {
    output: mpsc::SyncSender<Value>,
    connection: Arc<Mutex<Option<UnixStream>>>,
    child: Arc<Mutex<std::process::Child>>,
    socket: PathBuf,
    host_id: String,
}
impl Drop for Service {
    fn drop(&mut self) {
        if let Some(stream) = self.connection.lock().unwrap().take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}
impl Service {
    fn start(data: PathBuf,cancel:Arc<std::sync::atomic::AtomicBool>) -> anyhow::Result<(Self, async_channel::Receiver<Value>)> {
        use anyhow::{Context, ensure};
        std::fs::create_dir_all(&data)?;
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700))?;
        let host_file = data.join("host-id");
        let host_id = match std::fs::read_to_string(&host_file) {
            Ok(value) if uuid::Uuid::parse_str(value.trim()).is_ok() => value.trim().to_string(),
            _ => {
                let value = uuid::Uuid::new_v4().to_string();
                std::fs::write(&host_file, &value)?;
                value
            }
        };
        // A unique path never unlinks another running host's socket.
        // macOS's per-user temporary directory is already ~50 bytes. Keep
        // this under sockaddr_un's 104-byte limit; UUID alone is unique.
        let socket = std::env::temp_dir().join(format!("kai-h-{}.sock",uuid::Uuid::new_v4().simple()));
        let listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let packaged = std::env::current_exe()?
            .parent()
            .context("No application directory")?
            .join("../Resources/kai_remote.py");
        let script = if packaged.is_file() {
            packaged
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("native_helpers/kai_remote.py")
        };
        let mut command = Command::new("/usr/bin/python3");
        command
            .arg(script)
            .arg("--data")
            .arg(&data)
            .arg("serve")
            .arg("--socket")
            .arg(&socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let config = data.join("listener.json");
        if config.is_file() {
            let value: Value = serde_json::from_slice(&std::fs::read(config)?)?;
            for (key, flag) in [
                ("bind", "--bind"),
                ("port", "--port"),
                ("public_url", "--public-url"),
                ("tls_cert", "--tls-cert"),
                ("tls_key", "--tls-key"),
            ] {
                if let Some(v) = value.get(key) {
                    ensure!(v.is_string() || v.is_u64(), "Invalid listener setting");
                    command.arg(flag).arg(
                        v.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string()),
                    );
                }
            }
        }
        let mut child = command.spawn()?;
        let until = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if cancel.load(std::sync::atomic::Ordering::Acquire)||child.try_wait()?.is_some() || Instant::now() > until {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = std::fs::remove_file(&socket);
                        anyhow::bail!(
                            "Remote transport did not connect; check listener port and TLS configuration"
                        )
                    };
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(error.into()),
            }
        };
        // BSD/macOS accepts can inherit O_NONBLOCK from the listener. The
        // dedicated reader must wait for the next command rather than treating
        // an idle WouldBlock as a disconnected helper.
        stream.set_nonblocking(false)?;
        let (write_tx, write_rx) = mpsc::sync_channel::<Value>(16);
        let (tx, rx) = async_channel::bounded(32);
        let mut writer = stream.try_clone()?;
        let connection = Arc::new(Mutex::new(Some(stream.try_clone()?)));
        std::thread::Builder::new()
            .name("kai-remote-write".into())
            .spawn(move || {
                while let Ok(value) = write_rx.recv() {
                    if serde_json::to_writer(&mut writer, &value).is_err()
                        || writer.write_all(b"\n").is_err()
                    {
                        break;
                    }
                }
            })?;
        std::thread::Builder::new()
            .name("kai-remote-read".into())
            .spawn(move || {
                let mut reader = BufReader::new(stream);
                loop {
                    let mut bytes = Vec::new();
                    match std::io::Read::by_ref(&mut reader)
                        .take(2 * 1024 * 1024 + 1)
                        .read_until(b'\n', &mut bytes)
                    {
                        Ok(0) | Err(_) => break,
                        Ok(_) if bytes.len() > 2 * 1024 * 1024 => break,
                        _ => {
                            if let Ok(value) = serde_json::from_slice(&bytes) {
                                if tx.send_blocking(value).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            })?;
        Ok((
            Self {
                output: write_tx,
                connection,
                child: Arc::new(Mutex::new(child)),
                socket,
                host_id,
            },
            rx,
        ))
    }
    fn send(&self, value: Value) -> bool {
        let snapshot = value.get("snapshot").is_some();
        match self.output.try_send(value) {
            Ok(()) => true,
            Err(_) => {
                if !snapshot {
                    if let Some(stream) = self.connection.lock().unwrap().as_ref() {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                    }
                }
                false
            }
        }
    }
}
pub(super) struct State {
    pub service: Option<Service>,
    pub starting: bool,
    pub note: Option<String>,
    generation: u64,
    start_cancel:Option<Arc<std::sync::atomic::AtomicBool>>,
    pub policies: RefCell<HashMap<String, orchestrator::Policy>>,
    pub workflow_result:RefCell<Option<Value>>,
    replies: RefCell<HashMap<(String, String), String>>,
    signature: std::cell::Cell<[u8; 32]>,
    snapshot_task: Option<Task<()>>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            service: None,
            starting: false,
            note: None,
            generation: 0,
            start_cancel:None,
            policies: Default::default(),
            workflow_result:Default::default(),
            replies: Default::default(),
            signature: std::cell::Cell::new([0; 32]),
            snapshot_task: None,
        }
    }
}
impl State {
    pub fn reply(&self, owner: &str, token: &str, value: Value) -> bool {
        if token=="workflow"&&self.policies.borrow_mut().remove(owner).is_some(){self.workflow_result.replace(Some(value));return true}
        if let Some(id) = self
            .replies
            .borrow_mut()
            .remove(&(owner.into(), token.into()))
        {
            self.policies.borrow_mut().remove(owner);
            if let Some(service) = &self.service {
                service.send(json!({"id":id,"reply":value}));
            }
            return true;
        }
        false
    }
}
impl Orbit {
    pub(super) fn remote_directory(&self) -> PathBuf {
        self.attachment_dir
            .parent()
            .unwrap_or(&self.attachment_dir)
            .join("remote")
    }
    pub(super) fn toggle_remote(&mut self, cx: &mut Context<Self>) {
        self.remote.generation = self.remote.generation.wrapping_add(1);
        let generation = self.remote.generation;
        if self.remote.service.take().is_some() {
            self.remote.policies.borrow_mut().clear();
            self.remote.replies.borrow_mut().clear();
            self.remote.note = Some("Remote access stopped".into());
            cx.notify();
            return;
        }
        if self.remote.starting {
            if let Some(cancel)=self.remote.start_cancel.take(){cancel.store(true,std::sync::atomic::Ordering::Release);}
            self.remote.starting=false;self.remote.note=Some("Remote start cancelled".into());cx.notify();return;
        }
        self.remote.starting = true;
        let cancel=Arc::new(std::sync::atomic::AtomicBool::new(false));self.remote.start_cancel=Some(cancel.clone());
        let data = self.remote_directory();
        let window = self.orchestrator_window;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { Service::start(data,cancel) })
                .await;
            match result {
                Ok((service, receiver)) => {
                    if this
                        .update(cx, |app, cx| {
                            if app.remote.generation != generation {
                                return;
                            }
                            app.remote.starting = false;
                            app.remote.signature.set([0; 32]);
                            app.remote.service = Some(service);
                            app.remote.note =
                                Some("Remote host running · loopback by default".into());
                            app.publish_remote();
                            cx.notify();
                        })
                        .is_err()
                    {
                        return;
                    }
                    while let Ok(mut packet) = receiver.recv().await {
                        packet["generation"]=json!(generation);
                        if window
                            .update(cx, |app, window, cx| {
                                if app.remote.generation == generation {
                                    app.remote_request(packet, window, cx)
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    let _ = this.update(cx, |app, cx| {
                        if app.remote.generation != generation {
                            return;
                        }
                        app.remote.service = None;
                        app.remote.note = Some("Remote transport disconnected".into());
                        cx.notify();
                    });
                }
                Err(error) => {
                    let _ = this.update(cx, |app, cx| {
                        if app.remote.generation != generation {
                            return;
                        }
                        app.remote.starting = false;
                        app.remote.note = Some(error.to_string());
                        cx.notify();
                    });
                }
            }
        })
        .detach();
        cx.notify();
    }
    pub(super) fn remote_controls(&self, cx: &Context<Self>) -> AnyElement {
        div()
            .mt_3()
            .border_t_1()
            .border_color(rgba(0xffffff16))
            .pt_2()
            .child(
                div()
                    .text_size(theme::ui_px(12.))
                    .child("Remote access & ChatGPT"),
            )
            .child(
                div()
                    .id("remote-toggle")
                    .p_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(if self.remote.starting {
                        "Starting host…"
                    } else if self.remote.service.is_some() {
                        "Stop remote host"
                    } else {
                        "Start remote host"
                    })
                    .on_click(cx.listener(|app, _, _, cx| app.toggle_remote(cx))),
            )
            .child(
                div()
                    .text_size(theme::ui_px(10.))
                    .text_color(rgb(MUTED))
                    .child(self.remote.note.clone().unwrap_or_else(|| {
                        "Optional · no background transport while disabled".into()
                    })),
            )
            .child(
                div()
                    .id("remote-pair")
                    .p_2()
                    .cursor_pointer()
                    .child("Create read-only pairing code")
                    .on_click(cx.listener(|app, _, _, cx| app.remote_pair_code("read", cx))),
            )
            .child(
                div()
                    .id("remote-pair-control")
                    .p_2()
                    .cursor_pointer()
                    .child("Create control pairing code · this project")
                    .on_click(cx.listener(|app, _, _, cx| app.remote_pair_code("control", cx))),
            )
            .into_any_element()
    }
    fn remote_pair_code(&mut self, scope: &'static str, cx: &mut Context<Self>) {
        let data = self.remote_directory();
        let project = self.project.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("native_helpers/kai_remote.py");
                    let packaged = std::env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(|p| p.join("../Resources/kai_remote.py")))
                        .filter(|p| p.is_file());
                    let mut cmd = Command::new("/usr/bin/python3");
                    cmd.arg(packaged.unwrap_or(script))
                        .arg("--data")
                        .arg(data)
                        .arg("pairing-code")
                        .arg("--scope")
                        .arg(scope)
                        .arg("--project")
                        .arg(project);
                    cmd.output()
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                match result {
                    Ok(output) if output.status.success() => {
                        let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        cx.write_to_clipboard(ClipboardItem::new_string(code));
                        app.remote.note = Some(
                            "Pairing code copied · expires in five minutes · selected project only"
                                .into(),
                        )
                    }
                    _ => app.remote.note = Some("Could not generate pairing code".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn remote_tasks(&self, grant: &Value, args: &Value) -> Value {
        let allowed = |project: &PathBuf| {
            grant["projects"].as_array().is_none_or(|roots| {
                roots.is_empty() || roots.iter().any(|p| p.as_str() == project.to_str())
            })
        };
        let requested = |id: &str| {
            args["chat_ids"]
                .as_array()
                .is_none_or(|ids| ids.iter().any(|v| v.as_str() == Some(id)))
        };
        let mut tasks = std::collections::BTreeMap::new();
        for team in &self.teams {
            if !allowed(&team.project) {
                continue;
            }
            for m in &team.members {
                if requested(&m.session) {
                    tasks.insert(m.session.clone(),json!({"chat_id":m.session,"project":team.project,"agent":m.agent.command(),"model":m.model,"effort":m.reasoning_effort,"status":m.status.label(),"terminal":!m.status.active()&&m.status!=teams::Status::Queued}));
                }
            }
        }
        for timing in self.turn_timings.iter().rev() {
            if tasks.len() >= 256 {
                break;
            }
            if let Some(chat) = self
                .sessions
                .iter()
                .find(|s| s.id == timing.session && allowed(&s.project_path) && requested(&s.id))
            {
                if timing.finished_ms.is_some() {
                    tasks.entry(chat.id.clone()).or_insert_with(||json!({"chat_id":chat.id,"project":chat.project_path,"status":timing.outcome,"terminal":true,"elapsed_ms":timing.elapsed_ms,"model":timing.model,"effort":timing.effort}));
                }
            }
        }
        for chat in self
            .sessions
            .iter()
            .filter(|s| allowed(&s.project_path) && requested(&s.id))
        {
            if let Some(run) = self.runs.get(&chat.id) {
                tasks.insert(chat.id.clone(),json!({"chat_id":chat.id,"project":chat.project_path,"agent":run.agent.command(),"run_id":run.parse_epoch.to_string(),"status":run.status,"terminal":false,"usage":self.usage.get(&chat.id),"needs_approval":self.native_pending.values().any(|p|p.owner==chat.id&&p.current(&self.runs))}));
            } else if let Some(launch) = self.launches.get(&chat.id) {
                tasks.insert(chat.id.clone(),json!({"chat_id":chat.id,"project":chat.project_path,"run_id":launch.key,"status":launch.caption,"terminal":false}));
            }
        }
        json!({"tasks":tasks.into_values().collect::<Vec<_>>()})
    }
    pub(super) fn queue_remote_snapshot(&mut self, cx: &mut Context<Self>) {
        if self.remote.service.is_none() || self.remote.snapshot_task.is_some() { return; }
        let generation = self.remote.generation;
        self.remote.snapshot_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(100)).await;
            let _ = this.update(cx, |app, _| {
                app.remote.snapshot_task = None;
                if app.remote.generation == generation { app.publish_remote(); }
            });
        }));
    }

    pub(super) fn publish_remote(&self) {
        use sha2::{Digest, Sha256};
        let Some(service) = &self.remote.service else {
            return;
        };
        let tasks = self.remote_tasks(&json!({}), &json!({}))["tasks"].clone();
        let catalogs:Vec<_>=self.orchestration_models.iter().map(|((agent,project),(at,info))|json!({"agent":agent.command(),"project":project,"metadata_stale":at.elapsed()>Duration::from_secs(300),"capabilities_verified":info.defaults.capabilities_checked,"auth":format!("{:?}",info.auth),"default_model":info.defaults.model,"default_effort":info.defaults.effort,"models":info.models.iter().map(|m|json!({"id":m.id,"name":m.label,"efforts":m.reasoning_efforts})).collect::<Vec<_>>()})).collect();
        let children:Vec<_>=self.unified_children.iter().filter_map(|child|self.child_owner(child,&self.unified_children).map(|owner|json!({"id":child.id,"parent_chat_id":owner.id,"project":owner.project_path,"workspace":child.workspace,"name":child.name,"task":child.task,"agent":child.provider.command(),"model":child.model,"effort":child.effort,"status":child.status,"connected":child.connected,"transcript_revision":child.transcript_revision,"transcript_bytes":child.transcript_bytes}))).collect();
        let value = json!({"connected":true,"protocol":"1","host_id":service.host_id,"catalogs":catalogs,"children":children,"projects":self.projects.iter().map(|p|json!({"path":p.path,"name":p.path.file_name().unwrap_or_default().to_string_lossy()})).collect::<Vec<_>>(),"chats":self.sessions.iter().take(100).map(|s|json!({"id":s.id,"title":s.title,"agent":s.agent.command(),"project":s.project_path,"workspace":s.execution_path.as_ref().unwrap_or(&s.project_path),"model":self.usage.get(&s.id).and_then(|u|u.model.as_ref()).or_else(||self.session_models.get(&s.id).and_then(Option::as_ref)),"effort":self.usage.get(&s.id).and_then(|u|u.reasoning_effort.as_ref()),"updated_at":s.updated_at})).collect::<Vec<_>>(),"tasks":tasks,"approvals":self.native_pending.values().filter(|p|p.current(&self.runs)).map(|p|json!({"owner":p.owner,"request_id":p.request_id(),"approval":p.remote_value(),"project":self.sessions.iter().find(|s|s.id==p.owner).map(|s|&s.project_path)})).collect::<Vec<_>>(),"providers":self.agents.iter().map(|a|json!({"agent":a.kind.command(),"installed":a.available})).collect::<Vec<_>>()});
        let Ok(bytes) = serde_json::to_vec(&value) else {
            return;
        };
        if bytes.len() > 1024 * 1024 {
            return;
        }
        let signature: [u8; 32] = Sha256::digest(&bytes).into();
        if self.remote.signature.get() != signature && service.send(json!({"snapshot":value})) {
            self.remote.signature.set(signature);
        }
    }
    fn remote_request(&mut self, packet: Value, window: &mut Window, cx: &mut Context<Self>) {
        if packet["generation"].as_u64().is_some_and(|g|g!=self.remote.generation){return}
        let id = packet["id"].as_str().unwrap_or("").to_owned();
        let args = packet["request"].clone();
        let grant = &packet["grant"];
        if args.as_object().is_some_and(|fields|fields.keys().any(|key|key.starts_with('_'))){
            self.remote_direct_reply(&id,json!({"error":"Reserved command field"}));return;
        }
        if let Some(chat) = args["chat_id"]
            .as_str()
            .filter(|id| !self.sessions.iter().any(|s| s.id == *id))
        {
            if packet["resolved"].is_null() {
                let rx = self
                    .engine
                    .workspace_query(json!({"action":"resolve_chat","chat_id":chat}));
                let mut packet = packet.clone();
                let id = id.clone();
                cx.spawn_in(window, async move |this, cx| {
                    let value = rx
                        .recv()
                        .await
                        .unwrap_or(json!({"error":"History worker closed"}));
                    let _ = this.update_in(cx, |app, window, cx| {
                        if value["session"].is_object() {
                            packet["resolved"] = value["session"].clone();
                            app.remote_request(packet, window, cx)
                        } else {
                            app.remote_direct_reply(&id, value)
                        }
                    });
                })
                .detach();
                return;
            }
        }
        let result = (|| -> Result<Option<Value>, String> {
            let action = args["action"].as_str().ok_or("Missing action")?;
            let client = grant["id"]
                .as_str()
                .filter(|s| s.len() <= 128)
                .ok_or("Invalid client")?;
            let roots: Vec<PathBuf> = grant["projects"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect();
            let session = args["chat_id"]
                .as_str()
                .and_then(|id| self.sessions.iter().find(|s| s.id == id))
                .cloned()
                .or_else(|| {
                    serde_json::from_value::<SessionSummary>(packet["resolved"].clone()).ok()
                });
            let target = args["project"]
                .as_str()
                .map(PathBuf::from)
                .or_else(|| session.as_ref().map(|s| s.project_path.clone()));
            if let Some(target) = &target {
                if !roots.is_empty() && !roots.contains(target) {
                    return Err("This client has no access to the selected project".into());
                }
            }
            // Authorize the resolved chat itself. An explicit project argument
            // cannot substitute an allowed project for an unrelated chat ID.
            if let Some(chat)=&session {
                if !roots.is_empty() && !roots.contains(&chat.project_path){
                    return Err("This client has no access to the chat's project".into());
                }
                if target.as_ref().is_some_and(|project|project!=&chat.project_path){
                    return Err("The chat belongs to a different project".into());
                }
            }
            if args["chat_id"].is_string() && session.is_none() {
                return Err(
                    "Load the chat's project first, then use its exact discovered ID".into(),
                );
            }
            match action {
                "snapshot" => {
                    return Ok(Some(json!({"use":"GET /v1/snapshot and GET /v1/events"})));
                }
                "list_chats" | "read_chat" => {
                    if action == "list_chats" && !roots.is_empty() && target.is_none() {
                        return Err("Select a granted project".into());
                    }
                    let receiver = self.engine.workspace_query(args.clone());
                    let id = id.clone();
                    cx.spawn(async move |this, cx| {
                        let value = receiver
                            .recv()
                            .await
                            .unwrap_or(json!({"error":"History worker closed"}));
                        let _ = this.update(cx, |app, _| app.remote_direct_reply(&id, value));
                    })
                    .detach();
                    return Ok(None);
                }
                "set_model" => {
                    if grant["scope"] != "control" {
                        return Err("Read-only client".into());
                    }
                    let chat = session.as_ref().ok_or("Choose a chat")?;
                    if self.runs.contains_key(&chat.id)
                        || self.launches.contains_key(&chat.id)
                        || self.voice_call.as_ref().is_some_and(|v| v.owner == chat.id)
                    {
                        return Err(
                            "Finish or stop the active turn before changing its model".into()
                        );
                    }
                    let workspace = chat.execution_path.as_ref().unwrap_or(&chat.project_path);
                    let (model, effort) = self.orchestration_selection(
                        &args,
                        chat.agent,
                        workspace,
                        self.session_models.get(&chat.id).cloned().flatten(),
                    )?;
                    self.engine.set_model(chat.clone(), model.clone());
                    self.session_models.insert(chat.id.clone(), model.clone());
                    let key = format!(
                        "{}:{}:{}",
                        chat.agent.command(),
                        chat.id,
                        model.as_deref().unwrap_or("")
                    );
                    if let Some(effort) = &effort {
                        self.reasoning_efforts.insert(key, effort.clone());
                    }
                    if self.active.as_deref() == Some(&chat.id) {
                        self.model = model.clone();
                    }
                    self.queue_workspace_save(cx);
                    cx.notify();
                    return Ok(Some(
                        json!({"chat_id":chat.id,"model":model,"effort":effort}),
                    ));
                }
                "stop" | "steer" => {
                    if grant["scope"] != "control" {
                        return Err("Read-only client".into());
                    }
                    let chat = session.as_ref().ok_or("Choose a chat")?;
                    let run = self.runs.get(&chat.id).ok_or("Chat is not running")?;
                    if args["run_id"].as_str() != Some(run.parse_epoch.to_string().as_str()) {
                        return Err("Run changed; refresh before acting".into());
                    }
                    if action == "stop" {
                        run.handle.cancel()
                    } else {
                        if run.agent!=AgentKind::Codex{return Err("This provider accepts another prompt when its current turn finishes".into())}
                        run.handle
                            .steer(args["prompt"].as_str().ok_or("Supply steering text")?)
                            .map_err(|e| e.to_string())?;
                    }
                    return Ok(Some(json!({"accepted":true})));
                }
                "answer" => {
                    if grant["scope"] != "control" {
                        return Err("Read-only client".into());
                    }
                    let chat = session.as_ref().ok_or("Choose a chat")?;
                    let mut answer = args["answer"].clone();
                    answer["action"] = json!("native_answer");
                    answer["threadId"] = json!(format!("code:{}", chat.id));
                    self.unified_approval(&answer, cx)?;
                    return Ok(Some(json!({"accepted":true})));
                }
                "list_artifacts" | "read_artifact" | "write_artifact" => {
                    return {
                        let mut args = args.clone();
                        args["_remote_id"] = json!(id);
                        self.artifact_command(&args, grant, cx)
                    };
                }
                "run_workflow" => {
                    let workflow = self
                        .workflows
                        .iter()
                        .find(|w| Some(w.name.as_str()) == args["name"].as_str())
                        .ok_or("Unknown workflow")?;
                    if !roots.is_empty() && !roots.contains(&workflow.project) {
                        return Err("Workflow is outside this grant".into());
                    }
                    let mut expanded = json!({"action":"create_chat","request_id":args["request_id"],"project":workflow.project,"agent":workflow.agent.command(),"name":workflow.name,"prompt":workflow.prompt});
                    if let Some(model) = &workflow.model {
                        expanded["model"] = json!(model)
                    }
                    if let Some(effort) = &workflow.effort {
                        expanded["effort"] = json!(effort)
                    }
                    let mut packet = packet.clone();
                    packet["request"] = expanded;
                    self.remote_request(packet, window, cx);
                    return Ok(None);
                }
                "list_workflows" | "save_workflow" => {
                    return self.workflow_command(&args, grant, window, cx).map(Some);
                }
                "read_file" => return self.remote_file(&args, grant, session.as_ref(), &id, cx),
                "restore" => {
                    if grant["scope"] != "control" {
                        return Err("Read-only client".into());
                    }
                    return self.remote_review(&args, session.as_ref(), &id, cx);
                }
                "review" => return self.remote_review(&args, session.as_ref(), &id, cx),
                _ => {}
            }
            if !["list_projects", "list_models", "task_status", "wait_tasks"].contains(&action)
                && grant["scope"] != "control"
            {
                return Err("Read-only client".into());
            }

            if action == "list_projects" {
                return Ok(Some(
                    json!({"projects":self.projects.iter().filter(|p|roots.is_empty()||roots.contains(&p.path)).map(|p|json!({"path":p.path,"name":self.project_label(p)})).collect::<Vec<_>>()}),
                ));
            }
            if action == "task_status" {
                return Ok(Some(self.remote_tasks(grant, &args)));
            }
            if action == "wait_tasks" {
                let grant = grant.clone();
                let args = args.clone();
                let id = id.clone();
                cx.spawn(async move |this, cx| {
                    let until = Instant::now() + Duration::from_secs(25);
                    loop {
                        let value = this
                            .update(cx, |app, _| app.remote_tasks(&grant, &args))
                            .ok();
                        let Some(value) = value else { return };
                        let done = value["tasks"].as_array().is_none_or(|v| {
                            v.is_empty()
                                || v.iter()
                                    .any(|t| t["terminal"] == true || t["needs_approval"] == true)
                        });
                        if done || Instant::now() >= until {
                            let _ = this.update(cx, |app, _| app.remote_direct_reply(&id, value));
                            break;
                        }
                        Timer::after(Duration::from_millis(250)).await;
                    }
                })
                .detach();
                return Ok(None);
            }

            if !roots.is_empty() && target.is_none() {
                return Err("Select a granted project or chat".into());
            }
            let hash: [u8; 32] = sha2::Sha256::digest(format!("{client}:{id}").as_bytes()).into();
            let owner = uuid::Uuid::from_bytes(hash[..16].try_into().unwrap()).to_string();
            if self.remote.policies.borrow().len() >= 32
                && !self.remote.policies.borrow().contains_key(&owner)
            {
                return Err("Client capacity reached".into());
            }
            self.remote.policies.borrow_mut().insert(
                owner.clone(),
                orchestrator::Policy {
                    project: target.unwrap_or_else(|| self.project.clone()),
                    model: None,
                    allow_edits: grant["scope"] == "control" && self.allow_edits,
                    codex_access: runtime::CodexAccess::Workspace,
                    codex_approvals: runtime::CodexApprovals::Ask,
                },
            );
            self.remote
                .replies
                .borrow_mut()
                .insert((owner.clone(), id.clone()), id.clone());
            self.workspace_tool(&owner, &id, "code_workspace", args, window, cx);
            Ok(None)
        })();
        if let Some(value) = match result {
            Ok(value) => value,
            Err(error) => Some(json!({"error":error})),
        } {
            self.remote_direct_reply(&id, value)
        }
        self.publish_remote();
    }
    pub(super) fn remote_direct_reply(&self, id: &str, value: Value) {
        if let Some(service) = &self.remote.service {
            service.send(json!({"id":id,"reply":value}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn remote_http_uses_the_real_helper_ipc_and_native_session_owner(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        let(root,data)=view.update(cx,|app,_|(app.project.clone(),app.workflow_dir.join("remote-integration-test")));
        std::fs::create_dir_all(&data).unwrap();
        let port={let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();listener.local_addr().unwrap().port()};
        std::fs::write(data.join("listener.json"),serde_json::to_vec(&json!({"port":port})).unwrap()).unwrap();
        let(service,requests)=Service::start(data.clone(),Arc::new(std::sync::atomic::AtomicBool::new(false))).unwrap();
        let script=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("native_helpers/kai_remote.py");
        let pair=Command::new("/usr/bin/python3").arg(&script).arg("--data").arg(&data).args(["pair","--scope","control","--project"]).arg(&root).output().unwrap();
        assert!(pair.status.success());let credential:Value=serde_json::from_slice(&pair.stdout).unwrap();
        view.update(cx,|app,_|{app.remote.service=Some(service);app.runs.insert("fixture-session".into(),crate::ui_tests::idle_run("Working".into()));app.publish_remote();});
        let(tx,rx)=mpsc::channel();
        let worker=std::thread::spawn(move||{
            let output=Command::new("/usr/bin/python3").args(["-c",r#"
import json,os,time,urllib.request
base='http://127.0.0.1:'+os.environ['KAI_TEST_PORT']
headers={'Authorization':'Bearer '+os.environ['KAI_TEST_TOKEN'],'Content-Type':'application/json'}
opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
def call(path,body=None):
 request=urllib.request.Request(base+path,data=None if body is None else json.dumps(body).encode(),headers=headers)
 with opener.open(request,timeout=5)as response:return json.load(response)
for _ in range(30):
 snapshot=call('/v1/snapshot')
 if snapshot.get('connected'):break
 time.sleep(.03)
assert snapshot['connected'] and snapshot['access']=='control'
projects=call('/v1/command',{'action':'list_projects'})
assert len(projects['projects'])==1
tasks=call('/v1/command',{'action':'task_status'})['tasks']
task=next(t for t in tasks if t['chat_id']=='fixture-session')
result=call('/v1/command',{'action':'stop','request_id':'isolated-stop','chat_id':task['chat_id'],'run_id':task['run_id']})
assert result.get('accepted') or result.get('stopped'),result
print('REMOTE_NATIVE_ROUND_TRIP_OK')
"#]).env("KAI_TEST_PORT",port.to_string()).env("KAI_TEST_TOKEN",credential["token"].as_str().unwrap()).output().unwrap();
            let _=tx.send(output);
        });
        let until=Instant::now()+Duration::from_secs(15);
        let output=loop{
            if let Ok(output)=rx.try_recv(){break output}
            assert!(Instant::now()<until,"Remote HTTP integration timed out");
            if let Ok(packet)=requests.try_recv(){view.update_in(cx,|app,window,cx|app.remote_request(packet,window,cx));}
            else{std::thread::sleep(Duration::from_millis(5));}
        };
        worker.join().unwrap();
        view.update(cx,|app,_|{drop(app.remote.service.take());});
        assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        view.update(cx,|app,_|assert!(app.runs["fixture-session"].handle.is_cancelled()));
        assert!(String::from_utf8_lossy(&output.stdout).contains("REMOTE_NATIVE_ROUND_TRIP_OK"));
    }
    #[gpui::test]
    fn remote_controls_enforce_scope_exact_run_and_approval_tokens(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let (handle, answers) = RunHandle::interactive_test_handle();
        view.update_in(cx,|app,window,cx|{
   let mut run=crate::ui_tests::idle_run("Working".into());run.handle=handle;
   let epoch=run.parse_epoch;let root=app.sessions[0].project_path.clone();
   app.runs.insert("fixture-session".into(),run);
   let grant=json!({"id":"isolated-client","scope":"read","projects":[root]});
   let request=json!({"action":"stop","chat_id":"fixture-session","run_id":epoch.to_string()});
   app.remote_request(json!({"id":"1","grant":grant,"request":request}),window,cx);assert!(!app.runs["fixture-session"].handle.is_cancelled());
   let mut grant=grant;grant["scope"]=json!("control");grant["projects"]=json!(["/unrelated"]);
   app.remote_request(json!({"id":"2","grant":grant,"request":request}),window,cx);assert!(!app.runs["fixture-session"].handle.is_cancelled());
   let mut spoofed=request.clone();spoofed["project"]=json!("/unrelated");
   app.remote_request(json!({"id":"2-spoofed","grant":grant,"request":spoofed}),window,cx);assert!(!app.runs["fixture-session"].handle.is_cancelled(),"An allowed project argument must not authorize another project's chat");
   grant["projects"]=json!([root]);let mut stale=request.clone();stale["run_id"]=json!("stale");
   app.remote_request(json!({"id":"3","grant":grant,"request":stale}),window,cx);assert!(!app.runs["fixture-session"].handle.is_cancelled());
   let native=runtime::NativeRequest{id:"opaque".into(),method:"session/request_permission".into(),title:"Write isolated file".into(),detail:"Exact operation".into(),kind:runtime::NativeRequestKind::Approval{choices:vec![runtime::NativeOption{id:"once".into(),label:"Allow once".into()}]}};
   app.native_pending.insert("opaque".into(),native_request_ui::LiveRequest::new("fixture-session",epoch,native));
   let token=app.native_pending["opaque"].remote_value()["approvalRequestId"].as_str().unwrap().to_owned();
   let mut answer=json!({"action":"answer","chat_id":"fixture-session","answer":{"approvalRequestId":"stale","choice":"once"}});
   app.remote_request(json!({"id":"4","grant":grant,"request":answer}),window,cx);assert!(answers.try_recv().is_err());
   answer["answer"]["approvalRequestId"]=json!(token);
   app.remote_request(json!({"id":"5","grant":grant,"request":answer}),window,cx);
   assert!(matches!(answers.try_recv().unwrap(),runtime::NativeControl::Respond{id,answer:runtime::NativeAnswer::Choice(choice)}if id=="opaque"&&choice=="once"));
   app.remote_request(json!({"id":"6","grant":grant,"request":answer}),window,cx);assert!(answers.try_recv().is_err());
   app.remote_request(json!({"id":"7","grant":grant,"request":request}),window,cx);assert!(app.runs["fixture-session"].handle.is_cancelled());
  });
    }
    #[gpui::test]
    fn task_snapshot_includes_launches_and_filters_project_and_ids(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, _| {
            let root = app.sessions[0].project_path.clone();
            app.launches.insert(
                "fixture-session".into(),
                chat_timing::Launch {
                    caption: "Preparing workspace…",
                    preview: vec![],
                    cancel: Default::default(),
                    started: Instant::now(),
                    key: "run-key".into(),
                },
            );
            let state = app.remote_tasks(&json!({"projects":[root]}), &json!({}));
            assert_eq!(state["tasks"][0]["run_id"], "run-key");
            assert_eq!(state["tasks"][0]["terminal"], false);
            assert_eq!(
                app.remote_tasks(&json!({"projects":["/other"]}), &json!({}))["tasks"],
                json!([])
            );
            assert_eq!(
                app.remote_tasks(&json!({}), &json!({"chat_ids":["other"]}))["tasks"],
                json!([])
            );
        });
    }
}
