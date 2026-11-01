//! Native personal-agent surfaces; one revision long-poll per configured host.
use super::*;
use crate::ninjas::{self, Profile};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct Host {
    pub id: String,
    pub name: String,
    pub endpoint: Option<String>,
}
#[derive(Default)]
pub(super) struct State {
    pub hosts: Vec<Host>,
    pub snapshots: HashMap<String, Value>,
    pub watching: HashSet<String>,
    pub panel: Option<Panel>,
    pub errors: HashMap<String, String>,
    pub started: bool,
}
pub(super) struct Panel {
    pub host: String,
    pub profile: Profile,
    pub fields: Vec<Entity<TextInput>>,
    _subscriptions: Vec<Subscription>,
    pub models: Vec<Value>,
    pub busy: bool,
    pub task: Option<Value>,
    pub chat: Option<String>,
    pub memory_revision: u64,
    pub schedule: usize,
    pub at: Instant,
    pub token: uuid::Uuid,
}
impl State {
    pub fn restore() -> Self {
        if cfg!(test) {
            return Self {
                hosts: vec![Host {
                    id: "local".into(),
                    name: "This Mac".into(),
                    endpoint: None,
                }],
                ..Default::default()
            };
        }
        let value = std::fs::read(ninjas::root().join("surfaces.json"))
            .ok()
            .filter(|v| v.len() < 2 * 1024 * 1024)
            .and_then(|v| serde_json::from_slice::<Value>(&v).ok())
            .unwrap_or_default();
        let mut hosts: Vec<Host> =
            serde_json::from_value(value["hosts"].clone()).unwrap_or_default();
        if !hosts.iter().any(|h| h.id == "local") {
            hosts.insert(
                0,
                Host {
                    id: "local".into(),
                    name: ninja::local_machine().name,
                    endpoint: None,
                },
            );
        }
        Self {
            hosts,
            snapshots: serde_json::from_value(value["snapshots"].clone()).unwrap_or_default(),
            ..Default::default()
        }
    }
    fn save(&self) {
        if cfg!(test) {
            return;
        }
        // Coalesce revisions off the UI thread; one sleeping writer, no idle timer.
        type Cache = (
            std::sync::Arc<std::sync::Mutex<Option<Value>>>,
            std::sync::mpsc::SyncSender<()>,
        );
        static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
        let (pending, wake) = CACHE.get_or_init(|| {
            let pending = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
            let p = pending.clone();
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            std::thread::spawn(move || {
                use std::os::unix::fs::PermissionsExt;
                while rx.recv().is_ok() {
                    let Some(v) = p.lock().unwrap().take() else {
                        continue;
                    };
                    let root = ninjas::root();
                    let _ = std::fs::create_dir_all(&root);
                    let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
                    if let Ok(bytes) = serde_json::to_vec(&v) {
                        let temp = root.join("surfaces.next");
                        if std::fs::write(&temp, bytes).is_ok() {
                            let _ = std::fs::set_permissions(
                                &temp,
                                std::fs::Permissions::from_mode(0o600),
                            );
                            let _ = std::fs::rename(temp, root.join("surfaces.json"));
                        }
                    }
                }
            });
            (pending, tx)
        });
        *pending.lock().unwrap() = Some(json!({"hosts":self.hosts,"snapshots":self.snapshots}));
        let _ = wake.try_send(());
    }
    pub fn active(&self) -> Vec<Value> {
        let mut out = vec![];
        for h in &self.hosts {
            let Some(v) = self.snapshots.get(&h.id) else {
                continue;
            };
            let reconnecting = self.errors.contains_key(&h.id);
            for t in
                v["tasks"].as_array().into_iter().flatten().filter(|t| {
                    ["running", "approval"].contains(&t["state"].as_str().unwrap_or(""))
                })
            {
                let p = v["profiles"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|p| p["id"] == t["profile"]);
                let name = p.and_then(|p| p["name"].as_str()).unwrap_or("Ninja");
                out.push(json!({"id":format!("ninja:{}:{}",h.id,t["id"].as_str().unwrap_or("")),"runId":t["id"],"scene":if !reconnecting&&t["state"]=="approval"{"permission"}else{"thinking"},"text":if reconnecting{format!("{name} · Reconnecting to {}",h.name)}else{format!("{name} · {}",t["status"].as_str().unwrap_or("Working"))},"name":name,"agent":"ninja"}));
            }
        }
        out
    }
    pub fn pending(&self) -> Option<(Host, String, Value)> {
        self.hosts
            .iter()
            .filter(|h| !self.errors.contains_key(&h.id))
            .find_map(|h| {
                self.snapshots.get(&h.id)?["tasks"]
                    .as_array()?
                    .iter()
                    .find_map(|t| {
                        let r = t["requests"].as_array()?.first()?.clone();
                        Some((h.clone(), t["id"].as_str()?.into(), r))
                    })
            })
    }
    pub fn review(&self) -> Value {
        let Some((h, task, v)) = self.pending() else {
            return Value::Null;
        };
        let Ok(r) = serde_json::from_value::<runtime::NativeRequest>(v) else {
            return Value::Null;
        };
        let (choices, questions, kind) = match r.kind {
            runtime::NativeRequestKind::Approval { choices } => {
                (json!(choices), json!([]), "approval")
            }
            runtime::NativeRequestKind::Questions { questions } => {
                (json!([]), json!(questions), "questions")
            }
            _ => (json!([]), json!([]), "approval"),
        };
        json!({"owner":format!("ninja:{}:{task}",h.id),"token":r.id,"title":r.title,"detail":r.detail,"choices":choices,"questions":questions,"kind":kind})
    }
}
fn call(host: &Host, value: &Value) -> anyhow::Result<Value> {
    if let Some(endpoint) = &host.endpoint {
        let v = ninja::http::json(
            "POST",
            &format!("{}/agents", endpoint.trim_end_matches('/')),
            Some(value),
            Duration::from_secs(if value["action"] == "import_handoff" {
                130
            } else {
                35
            }),
        )?;
        if let Some(e) = v["error"].as_str() {
            anyhow::bail!("{e}")
        }
        Ok(v)
    } else {
        ninjas::call_local(&ninjas::root(), value, true)
    }
}
fn avatar(style: &str, size: f32) -> Div {
    let color = ninjas::styles()
        .iter()
        .find(|(id, _, _)| *id == style)
        .map(|(_, _, c)| *c)
        .unwrap_or(0xef6758);
    div()
        .relative()
        .size(px(size))
        .child(icon("ninja-body", size, color).absolute().top_0().left_0())
        .child(
            icon("ninja-band", size, 0xf1ead2)
                .absolute()
                .top_0()
                .left_0(),
        )
        .child(
            icon("ninja-eyes", size, 0x17202c)
                .absolute()
                .top_0()
                .left_0(),
        )
}

fn chip(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on: bool,
    cx: &Context<Orbit>,
    action: impl Fn(&mut Orbit, &mut Window, &mut Context<Orbit>) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let label = label.into();
    div()
        .id(id)
        .h(px(28.))
        .px_2()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .bg(rgba(if on { 0x9380ff35 } else { 0xffffff09 }))
        .hover(|s| s.bg(rgba(GLASS_HOVER)))
        .text_size(theme::ui_px(11.))
        .text_color(rgb(TEXT))
        .cursor_pointer()
        .child(label)
        .on_click(cx.listener(move |app, _, w, cx| {
            cx.stop_propagation();
            action(app, w, cx)
        }))
}
impl Orbit {
    pub(super) fn ninja_remote_command(
        &mut self,
        value: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        let action = value["action"].as_str().unwrap_or("");
        let owner = value["threadId"]
            .as_str()
            .or_else(|| value["id"].as_str())
            .unwrap_or("");
        let Some(rest) = owner.strip_prefix("ninja:") else {
            return Ok(false);
        };
        let (host_id, task_id) = rest.rsplit_once(':').ok_or("Invalid Ninja owner")?;
        let host = self
            .ninjas
            .hosts
            .iter()
            .find(|h| h.id == host_id)
            .cloned()
            .ok_or("Unknown Ninja host")?;
        let task = self
            .ninjas
            .snapshots
            .get(host_id)
            .and_then(|v| v["tasks"].as_array())
            .and_then(|v| v.iter().find(|t| t["id"] == task_id))
            .cloned()
            .ok_or("Task is no longer available")?;
        if action == "chat" {
            let profile = task["profile"].as_str().map(str::to_owned);
            if !self
                .ninjas
                .panel
                .as_ref()
                .is_some_and(|p| p.host == host_id && profile.as_deref() == Some(&p.profile.id))
            {
                self.open_ninjas(Some(host_id.to_owned()), profile, window, cx);
            }
            if let Some(p) = &mut self.ninjas.panel {
                p.chat = task["chat"].as_str().map(str::to_owned);
                p.task = Some(task.clone());
            }
            return Ok(true);
        }
        if !matches!(
            action,
            "approve" | "reject" | "approval_stop" | "native_answer" | "native_cancel"
        ) {
            return Ok(false);
        }
        let token = value["approvalRequestId"]
            .as_str()
            .ok_or("Missing request token")?;
        let raw = task["requests"]
            .as_array()
            .and_then(|rs| rs.iter().find(|r| r["id"] == token))
            .ok_or("This request is no longer pending")?;
        let request: runtime::NativeRequest =
            serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
        let answer = if matches!(action, "native_cancel" | "approval_stop") {
            runtime::NativeAnswer::Cancel
        } else {
            match request.kind {
                runtime::NativeRequestKind::Approval { choices } => {
                    let legacy = if action == "approve" {
                        [
                            "accept",
                            "allow",
                            "once",
                            "allow_once",
                            "approved",
                            "accepted",
                        ]
                    } else {
                        ["decline", "deny", "reject", "rejected", "denied", "cancel"]
                    };
                    let choice = value["choice"]
                        .as_str()
                        .or_else(|| {
                            matches!(action, "approve" | "reject")
                                .then(|| {
                                    legacy.iter().find_map(|id| {
                                        choices.iter().find(|c| c.id == *id).map(|c| c.id.as_str())
                                    })
                                })
                                .flatten()
                        })
                        .ok_or("Choose a response")?;
                    if !choices.iter().any(|c| c.id == choice) {
                        return Err("Choice is no longer available".into());
                    }
                    runtime::NativeAnswer::Choice(choice.into())
                }
                runtime::NativeRequestKind::Questions { .. } => runtime::NativeAnswer::Answers(
                    serde_json::from_value(value["answers"].clone())
                        .map_err(|_| "Invalid answers")?,
                ),
                _ => return Err("Open this task's native controls".into()),
            }
        };
        let wire = json!({"action":if action=="approval_stop"{"stop"}else{"respond"},"id":task_id,"request":token,"child":raw["childId"],"answer":answer});
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { call(&host, &wire) })
                .await;
            let _ = this.update(cx, |app, cx| {
                if let Err(e) = result {
                    app.notice = Some(e.to_string());
                }
                cx.notify();
            });
        })
        .detach();
        Ok(true)
    }

    fn ninja_handoff(&mut self, cx: &mut Context<Self>) {
        let context = serde_json::to_string(&self.unified_history()).unwrap_or_default();
        let Some(panel) = &mut self.ninjas.panel else {
            return;
        };
        if panel.busy {
            return;
        }
        let Some(host) = self
            .ninjas
            .hosts
            .iter()
            .find(|h| h.id == panel.host && h.endpoint.is_some())
            .cloned()
        else {
            return;
        };
        if self
            .sessions
            .iter()
            .any(|s| s.project_path == self.project && self.runs.contains_key(&s.id))
        {
            self.notice = Some("Finish local edits before handing off this workspace".into());
            cx.notify();
            return;
        }
        let token = panel.token;
        let id = panel.profile.id.clone();
        let project = self.project.clone();
        if context.len() > 48 * 1024 {
            self.notice = Some("Chat context exceeds the handoff limit".into());
            cx.notify();
            return;
        }
        panel.busy = true;
        cx.spawn(async move|this,cx|{
            let remote=host.clone();
            let result=cx.background_executor().spawn(async move { (||->anyhow::Result<Value>{
                let packet=ninjas::handoff(&json!({"action":"export","project":project,"context":context,"out":ninjas::root().join("handoffs")}))?;
                let endpoint=remote.endpoint.as_deref().ok_or_else(||anyhow::anyhow!("Choose remote host"))?;
                let ip=endpoint.trim_start_matches("http://").trim_start_matches("https://").split('/').next().unwrap_or("").split(':').next().unwrap_or("");
                anyhow::ensure!(!ip.is_empty(),"Missing Taildrop target");
                let cli=crate::process_env::dirs(&dirs::home_dir().unwrap_or_default()).into_iter().map(|p|p.join("tailscale")).chain([PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale")]).find(|p|p.is_file()).ok_or_else(||anyhow::anyhow!("Tailscale CLI unavailable"))?;
                let mut transfer=std::process::Command::new(cli);transfer.args(["file","cp"]).arg(packet["archive"].as_str().unwrap()).arg(format!("{ip}:"));ninja::http::run(transfer,None,Duration::from_secs(120)).map_err(|e|anyhow::anyhow!("Taildrop transfer failed: {e}"))?;
                let until=Instant::now()+Duration::from_secs(120);
                loop{let receipt=call(&remote,&json!({"action":"import_handoff","id":id,"handoff":packet["id"],"sha256":packet["sha256"]}))?;
                    if receipt["received"]==true{return Ok(receipt)}
                    anyhow::ensure!(Instant::now()<until,"Studio has not confirmed receipt; keep both devices online and retry");std::thread::sleep(Duration::from_secs(1));}
            })() }).await;
            let _=this.update(cx,|app,cx|{
                if let Some(p)=app.ninjas.panel.as_mut().filter(|p|p.token==token){p.busy=false;match result{
                    Ok(receipt)=>{if let Ok(profile)=serde_json::from_value::<Profile>(receipt["profile"].clone()){p.profile=profile;}p.fields[2].update(cx,|i,cx|i.set_text(receipt["project"].as_str().unwrap_or(""),cx));p.fields[3].update(cx,|i,cx|i.set_text(&format!("Continue this handed-off workspace. Previous chat context (quoted):\n{}",receipt["context"].as_str().unwrap_or("")),cx));app.notice=Some("Studio confirmed the workspace and context. Assign the next task to continue there.".into());},
                    Err(e)=>app.notice=Some(e.to_string())}}
                cx.notify();});
        }).detach();
        cx.notify();
    }

    pub(super) fn start_ninja_watch(&mut self, host: Host, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        if !self.ninjas.watching.insert(host.id.clone()) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let mut revision = 0;
            let mut backoff = 1;
            loop {
                let request =
                    json!({"action":if revision==0{"snapshot"}else{"watch"},"revision":revision});
                let remote = host.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { call(&remote, &request) })
                    .await;
                let delay = if result.is_ok() {
                    250
                } else {
                    backoff = (backoff * 2).min(30);
                    backoff * 1000
                };
                let keep = this
                    .update(cx, |app, cx| {
                        if !app.ninjas.hosts.iter().any(|h| h.id == host.id) {
                            return false;
                        }
                        match result {
                            Ok(value) => {
                                revision = value["revision"].as_u64().unwrap_or(0);
                                if let Some(panel) = app
                                    .ninjas
                                    .panel
                                    .as_mut()
                                    .filter(|p| p.host == host.id && p.profile.revision == 0)
                                {
                                    if panel.fields[2].read(cx).text().is_empty() {
                                        if let Some(home) = value["home"].as_str() {
                                            panel.fields[2]
                                                .update(cx, |input, cx| input.set_text(home, cx));
                                        }
                                    }
                                }
                                let previous = app.ninjas.snapshots.get(&host.id);
                                for task in
                                    value["tasks"].as_array().into_iter().flatten().filter(|t| {
                                        t["state"] == "completed" && t["acknowledged"] != true
                                    })
                                {
                                    let id = task["id"].as_str().unwrap_or("");
                                    let seen = previous.is_some_and(|v| {
                                        v["tasks"]
                                            .as_array()
                                            .into_iter()
                                            .flatten()
                                            .any(|t| t["id"] == id && t["state"] == "completed")
                                    });
                                    if !seen {
                                        app.unified_successes.push_back((
                                            format!("ninja:{}:{id}", host.id),
                                            id.into(),
                                        ));
                                    }
                                }

                                backoff = 1;
                                app.ninjas.errors.remove(&host.id);
                                if app.ninjas.snapshots.get(&host.id) != Some(&value) {
                                    app.ninjas.snapshots.insert(host.id.clone(), value);
                                    app.ninjas.save();
                                }
                            }
                            Err(e) => {
                                app.ninjas.errors.insert(host.id.clone(), e.to_string());
                            }
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
                Timer::after(Duration::from_millis(delay)).await;
            }
        })
        .detach();
    }
    pub(super) fn open_ninjas(
        &mut self,
        host: Option<String>,
        profile: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ninjas.panel.as_ref().is_some_and(|p| {
            (profile.is_none()
                && p.profile.revision == 0
                && host.as_deref().is_none_or(|h| h == p.host))
                || (host.as_deref() == Some(p.host.as_str())
                    && profile.as_deref() == Some(p.profile.id.as_str()))
        }) {
            self.ninjas.panel = None;
            cx.notify();
            return;
        }
        let selected = host
            .or_else(|| {
                self.ninjas
                    .hosts
                    .iter()
                    .find(|h| h.id == self.ninja.draft.machine)
                    .map(|h| h.id.clone())
            })
            .unwrap_or_else(|| "local".into());
        if self.ninja.draft.node
            && !self
                .ninjas
                .hosts
                .iter()
                .any(|h| h.id == self.ninja.draft.machine)
        {
            self.ninjas.hosts.push(Host {
                id: self.ninja.draft.machine.clone(),
                name: self
                    .ninja
                    .machines
                    .iter()
                    .find(|m| m.id == self.ninja.draft.machine)
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| self.ninja.draft.machine.clone()),
                endpoint: Some(self.ninja.draft.endpoint.clone()),
            });
        }
        let snapshot = self.ninjas.snapshots.get(&selected);
        let existing = profile
            .as_ref()
            .and_then(|id| {
                snapshot?
                    .get("profiles")?
                    .as_array()?
                    .iter()
                    .find(|p| p["id"].as_str() == Some(id))
            })
            .and_then(|v| serde_json::from_value::<Profile>(v.clone()).ok());
        let p=existing.unwrap_or_else(||Profile{id:uuid::Uuid::new_v4().to_string(),name:String::new(),persona:"A resourceful engineering partner. Do the work, verify it, and keep useful long-term context.".into(),style:"fire".into(),project:if selected=="local"{self.execution_root().unwrap_or_else(||self.project.clone())}else{snapshot.and_then(|v|v["home"].as_str()).map(PathBuf::from).unwrap_or_default()},agent:self.agent,model:self.request_model(),effort:self.request_effort(),ninja:(self.agent==AgentKind::Ninja).then(||self.ninja_selection()),edits:self.allow_edits,access:self.codex_access,approvals:self.codex_approvals,revision:0});
        let mut fields = vec![];
        let mut subscriptions = vec![];
        for (placeholder, text) in [
            ("Name", p.name.clone()),
            ("Persona and how this Ninja should work", p.persona.clone()),
            (
                "Absolute workspace on the selected host",
                p.project.to_string_lossy().into_owned(),
            ),
            ("Give this Ninja a task…", String::new()),
            ("Persistent personal memory", String::new()),
            ("Search host models", String::new()),
        ] {
            let field = cx.new(|cx| {
                let mut input = TextInput::new(window, cx);
                input.set_placeholder(placeholder, cx);
                input.set_text(&text, cx);
                input
            });
            subscriptions.push(cx.subscribe(&field, |_, _, _, cx| cx.notify()));
            fields.push(field);
        }
        self.ninjas.panel = Some(Panel {
            host: selected.clone(),
            profile: p,
            fields,
            _subscriptions: subscriptions,
            models: vec![],
            busy: false,
            task: None,
            chat: None,
            memory_revision: 0,
            schedule: 0,
            at: Instant::now(),
            token: uuid::Uuid::new_v4(),
        });
        self.modal = None;
        self.agent_picker = None;
        self.overlay_window = Some(window.window_handle().window_id());
        if let Some(host) = self.ninjas.hosts.iter().find(|h| h.id == selected).cloned() {
            self.start_ninja_watch(host, cx);
        }
        if profile.is_some() {
            self.ninja_operation(
                json!({"action":"memory","id":self.ninjas.panel.as_ref().unwrap().profile.id}),
                cx,
            );
        }
        // Device inventory is metadata only and never probes provider logins.
        if cfg!(test) {
            cx.notify();
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { ninja::machines() })
                .await;
            let _ = this.update(cx, |app, cx| {
                if let Ok(machines) = result {
                    for m in machines.into_iter().filter(|m| !m.local && m.online) {
                        if !app.ninjas.hosts.iter().any(|h| h.id == m.id) {
                            app.ninjas.hosts.push(Host {
                                id: m.id,
                                name: m.name,
                                endpoint: Some(format!("http://{}:32187", m.host)),
                            });
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn ninja_operation(&mut self, request: Value, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.ninjas.panel else {
            return;
        };
        if panel.busy {
            return;
        }
        let Some(host) = self
            .ninjas
            .hosts
            .iter()
            .find(|h| h.id == panel.host)
            .cloned()
        else {
            return;
        };
        let token = panel.token;
        let response_host = host.clone();
        let memory_base = panel.fields[4].read(cx).text();
        let submitted_prompt = request["prompt"].as_str().map(str::to_owned);
        let captured_agent = panel.profile.agent;
        let captured_model = panel.profile.ninja.as_ref().map(|n| n.model.clone());
        let action = request["action"].as_str().unwrap_or("").to_owned();
        panel.busy = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { call(&host, &request) })
                .await;
            let _ = this.update(cx, |app, cx| {
                let Some(panel) = app.ninjas.panel.as_mut().filter(|p| p.token == token) else {
                    return;
                };
                panel.busy = false;
                match result {
                    Ok(value) => {
                        app.ninjas.errors.remove(&panel.host);
                        match action.as_str() {
                            "models" => {
                                if panel.profile.agent != captured_agent
                                    || (captured_agent == AgentKind::Ninja
                                        && panel.profile.ninja.as_ref().map(|n| n.model.clone())
                                            != captured_model)
                                {
                                    app.ninja_load_models(cx);
                                    return;
                                }

                                panel.models =
                                    value["models"].as_array().cloned().unwrap_or_default();
                                if panel.profile.model.is_none() {
                                    panel.profile.model =
                                        value["model"].as_str().map(str::to_owned);
                                    panel.profile.effort =
                                        value["effort"].as_str().map(str::to_owned);
                                }
                            }
                            "import_handoff" => {}
                            "save_profile" => {
                                if let Ok(p) = serde_json::from_value::<Profile>(value) {
                                    panel.profile.revision = p.revision;
                                }
                            }
                            "memory" => {
                                panel.memory_revision = value["revision"].as_u64().unwrap_or(0);
                                if panel.fields[4].read(cx).text() == memory_base {
                                    panel.fields[4].update(cx, |input, cx| {
                                        input.set_text(value["content"].as_str().unwrap_or(""), cx)
                                    });
                                }
                            }
                            "submit" => {
                                if let Some(revision) = value["profile"]["revision"].as_u64() {
                                    panel.profile.revision = revision;
                                }
                                panel.chat = value["chat"].as_str().map(str::to_owned);
                                panel.task = Some(value);
                                if submitted_prompt.as_deref()
                                    == Some(panel.fields[3].read(cx).text().as_str())
                                {
                                    panel.fields[3].update(cx, |input, cx| input.set_text("", cx));
                                }
                            }
                            "task" => {
                                let remote = response_host.clone();
                                let id = value["id"].clone();
                                cx.background_executor()
                                    .spawn(async move {
                                        let _ =
                                            call(&remote, &json!({"action":"acknowledge","id":id}));
                                    })
                                    .detach();
                                panel.chat = value["chat"].as_str().map(str::to_owned);
                                panel.task = Some(value);
                            }
                            "delete_profile" => {
                                app.ninjas.panel = None;
                            }
                            _ => {}
                        }
                    }
                    Err(e) => {
                        app.ninjas.errors.insert(panel.host.clone(), e.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn ninja_load_models(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.ninjas.panel else { return };
        self.ninja_operation(json!({"action":"models","agent":p.profile.agent,"project":p.fields[2].read(cx).text(),"model":p.profile.model,"ninja":p.profile.ninja}),cx);
    }
    fn ninja_save_profile(&mut self, cx: &mut Context<Self>) {
        if self.ninjas.panel.as_ref().is_some_and(|p| p.busy) {
            return;
        }
        let Some(panel) = &mut self.ninjas.panel else {
            return;
        };
        panel.profile.name = panel.fields[0].read(cx).text();
        panel.profile.persona = panel.fields[1].read(cx).text();
        panel.profile.project = PathBuf::from(panel.fields[2].read(cx).text());
        let p = panel.profile.clone();
        self.ninja_operation(json!({"action":"save_profile","profile":p}), cx);
    }
    fn ninja_submit_task(&mut self, cx: &mut Context<Self>) {
        if self
            .ninjas
            .panel
            .as_ref()
            .is_some_and(|p| p.busy || p.profile.revision == 0)
        {
            return;
        }
        let Some(p) = &self.ninjas.panel else { return };
        let repeat = (p.schedule == 2).then_some(86400);
        let due = if p.schedule == 1 {
            use chrono::{Datelike, TimeZone};
            let now = chrono::Local::now();
            let day = if now.time() < chrono::NaiveTime::from_hms_opt(22, 0, 0).unwrap() {
                now.date_naive()
            } else {
                now.date_naive() + chrono::Duration::days(1)
            };
            chrono::Local
                .with_ymd_and_hms(day.year(), day.month(), day.day(), 22, 0, 0)
                .earliest()
                .map(|v| v.timestamp())
                .unwrap_or(0)
        } else {
            0
        };
        let prompt = p.fields[3].read(cx).text();
        if prompt.trim().is_empty() {
            return;
        }
        let mut profile = p.profile.clone();
        profile.name = p.fields[0].read(cx).text();
        profile.persona = p.fields[1].read(cx).text();
        profile.project = PathBuf::from(p.fields[2].read(cx).text());
        let request = json!({"action":"submit","id":profile.id,"profile":profile,"prompt":prompt,"chat":p.chat,"due":due,"repeat":repeat});
        self.ninja_operation(request, cx);
    }
    pub(super) fn ninjas_sidebar(&self, cx: &Context<Self>) -> AnyElement {
        let rows = self
            .ninjas
            .hosts
            .iter()
            .flat_map(|h| {
                self.ninjas
                    .snapshots
                    .get(&h.id)
                    .into_iter()
                    .flat_map(move |v| {
                        v["profiles"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(move |p| (h.id.clone(), p.clone()))
                    })
            })
            .collect::<Vec<_>>();
        div()
            .mx_2()
            .mb_2()
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .child(
                        div()
                            .text_size(theme::ui_px(11.))
                            .text_color(rgb(MUTED))
                            .child("Ninjas"),
                    )
                    .child(chip("new-personal-ninja", "+", false, cx, |app, w, cx| {
                        app.open_ninjas(None, None, w, cx)
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(rows.into_iter().map(|(host, p)| {
                        let id = p["id"].as_str().unwrap_or("").to_owned();
                        let name = p["name"].as_str().unwrap_or("Ninja").to_owned();
                        let active = self.ninjas.snapshots.get(&host).is_some_and(|v| {
                            v["tasks"].as_array().into_iter().flatten().any(|t| {
                                t["profile"] == id
                                    && ["running", "approval"]
                                        .contains(&t["state"].as_str().unwrap_or(""))
                            })
                        });
                        let selected = self
                            .ninjas
                            .panel
                            .as_ref()
                            .is_some_and(|v| v.profile.id == id && v.host == host);
                        div()
                            .id(SharedString::from(format!("personal-ninja-{host}-{id}")))
                            .px_2()
                            .py_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .bg(rgba(if selected { 0x9380ff22 } else { 0xffffff05 }))
                            .hover(|v| v.bg(rgba(GLASS_HOVER)))
                            .cursor_pointer()
                            .child(avatar(p["style"].as_str().unwrap_or("fire"), 24.))
                            .child(div().text_size(theme::ui_px(11.)).child(name))
                            .when(active, |v| {
                                v.child(div().size(px(5.)).rounded_full().bg(rgb(0x82dca9)))
                            })
                            .on_click(cx.listener(move |app, _, w, cx| {
                                cx.stop_propagation();
                                app.open_ninjas(Some(host.clone()), Some(id.clone()), w, cx)
                            }))
                    })),
            )
            .into_any_element()
    }
    pub(super) fn ninjas_panel(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let Some(p) = &self.ninjas.panel else {
            return div().into_any_element();
        };
        let progress = if !self.motion || native::reduced_motion() {
            1.
        } else {
            (p.at.elapsed().as_secs_f32() / 0.24).min(1.)
        };
        if progress < 1. {
            request_motion_frame(window, cx);
        }
        let snapshot = self.ninjas.snapshots.get(&p.host);
        let providers = snapshot
            .and_then(|v| v["providers"].as_array())
            .cloned()
            .unwrap_or_default();
        let query = p.fields[5].read(cx).text().to_lowercase();
        let current_model = p.profile.model.clone();
        let efforts = p
            .models
            .iter()
            .find(|m| m["id"].as_str() == current_model.as_deref())
            .and_then(|m| m["efforts"].as_array())
            .cloned()
            .unwrap_or_default();
        let mut setup = div()
            .flex()
            .flex_col()
            .gap_2()
            .min_w(px(0.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(avatar(&p.profile.style, 36.))
                    .child(div().text_size(theme::ui_px(17.)).child(
                        if p.profile.name.is_empty() {
                            "New Ninja".to_owned()
                        } else {
                            p.profile.name.clone()
                        },
                    )),
            )
            .child(p.fields[0].clone())
            .child(div().h(px(70.)).child(p.fields[1].clone()))
            .child(
                div()
                    .text_size(theme::ui_px(10.))
                    .text_color(rgb(MUTED))
                    .child("Execution host · tasks, files, and provider logins live here"),
            );
        setup = setup
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(self.ninjas.hosts.iter().map(|host| {
                        let id = host.id.clone();
                        chip(
                            SharedString::from(format!("ninja-host-{id}")),
                            host.name.clone(),
                            p.host == id,
                            cx,
                            move |app, w, cx| app.open_ninjas(Some(id.clone()), None, w, cx),
                        )
                    })),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .children(ninjas::styles().into_iter().map(|(id, name, _)| {
                        chip(
                            SharedString::from(format!("ninja-style-{id}")),
                            name,
                            p.profile.style == id,
                            cx,
                            move |app, _, cx| {
                                if let Some(p) = &mut app.ninjas.panel {
                                    p.profile.style = id.into();
                                }
                                cx.notify();
                            },
                        )
                    })),
            )
            .child(p.fields[2].clone());
        setup = setup.child(
            div().flex().flex_wrap().gap_1().children(
                providers
                    .iter()
                    .filter(|v| v["installed"] == true)
                    .filter_map(|v| serde_json::from_value::<AgentKind>(v["agent"].clone()).ok())
                    .map(|agent| {
                        chip(
                            SharedString::from(format!("ninja-provider-{}", agent.command())),
                            agent.name(),
                            p.profile.agent == agent,
                            cx,
                            move |app, _, cx| {
                                let route = app.ninja_selection();
                                if let Some(p) = &mut app.ninjas.panel {
                                    p.profile.agent = agent;
                                    p.profile.model = None;
                                    p.profile.effort = None;
                                    p.profile.ninja = (agent == AgentKind::Ninja).then_some(route);
                                    p.models.clear();
                                }
                                app.ninja_load_models(cx);
                            },
                        )
                    }),
            ),
        );
        if p.profile.agent == AgentKind::Ninja {
            setup = setup
                .child(
                    div()
                        .text_size(theme::ui_px(10.))
                        .text_color(rgb(MUTED))
                        .child(
                            p.profile
                                .ninja
                                .as_ref()
                                .map(|n| format!("Inference: {} · {}", n.machine, n.engine.label()))
                                .unwrap_or_default(),
                        ),
                )
                .child(
                    div().flex().gap_1().children(
                        [
                            (ninja::Harness::Codex, "Codex harness"),
                            (ninja::Harness::Claude, "Claude harness"),
                        ]
                        .into_iter()
                        .map(|(h, label)| {
                            chip(
                                SharedString::from(format!("ninja-harness-{label}")),
                                label,
                                p.profile.ninja.as_ref().is_some_and(|n| n.harness == h),
                                cx,
                                move |app, _, cx| {
                                    if let Some(n) = app
                                        .ninjas
                                        .panel
                                        .as_mut()
                                        .and_then(|p| p.profile.ninja.as_mut())
                                    {
                                        n.harness = h;
                                    }
                                    cx.notify();
                                },
                            )
                        }),
                    ),
                );
        }
        setup = setup.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().child(p.fields[5].clone()))
                .child(chip(
                    "ninja-refresh-models",
                    if p.busy { "Reading…" } else { "Models" },
                    false,
                    cx,
                    |app, _, cx| app.ninja_load_models(cx),
                )),
        );
        setup = setup.child(
            div()
                .id("personal-ninja-model-list")
                .max_h(px(160.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1()
                .children(
                    p.models
                        .iter()
                        .filter(|v| {
                            v["name"]
                                .as_str()
                                .unwrap_or("")
                                .to_lowercase()
                                .contains(&query)
                                || v["id"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_lowercase()
                                    .contains(&query)
                        })
                        .take(128)
                        .map(|model| {
                            let id = model["id"].as_str().unwrap_or("").to_owned();
                            chip(
                                SharedString::from(format!("ninja-model-{id}")),
                                model["name"].as_str().unwrap_or(&id).to_owned(),
                                current_model.as_deref() == Some(id.as_str()),
                                cx,
                                move |app, _, cx| {
                                    if let Some(p) = &mut app.ninjas.panel {
                                        p.profile.model = Some(id.clone());
                                        p.profile.effort = None;
                                        if let Some(n) = &mut p.profile.ninja {
                                            n.model = id.clone();
                                            n.thinking = None;
                                        }
                                    }
                                    if app
                                        .ninjas
                                        .panel
                                        .as_ref()
                                        .is_some_and(|p| p.profile.agent == AgentKind::Ninja)
                                    {
                                        app.ninja_load_models(cx);
                                    }
                                    cx.notify();
                                },
                            )
                        }),
                ),
        );
        setup = setup.child(div().flex().flex_wrap().gap_1().children(
            efforts.iter().filter_map(Value::as_str).map(|effort| {
                let value = effort.to_owned();
                chip(
                    SharedString::from(format!("ninja-effort-{value}")),
                    composer_design::title(&value),
                    p.profile.effort.as_deref() == Some(effort),
                    cx,
                    move |app, _, cx| {
                        if let Some(p) = &mut app.ninjas.panel {
                            p.profile.effort = Some(value.clone());
                            if let Some(n) = &mut p.profile.ninja {
                                n.thinking = Some(value.clone());
                            }
                        }
                        cx.notify();
                    },
                )
            }),
        ));
        setup=setup.child(div().flex().gap_1().children([("Read only",false,runtime::CodexAccess::Workspace),("Workspace",true,runtime::CodexAccess::Workspace),("Full access",true,runtime::CodexAccess::Full)].into_iter().map(|(label,edit,access)|chip(SharedString::from(format!("ninja-access-{label}")),label,p.profile.edits==edit&&p.profile.access==access,cx,move|app,_,cx|{if let Some(p)=&mut app.ninjas.panel{p.profile.edits=edit;p.profile.access=access;}cx.notify();})))).child(div().flex().gap_1().children([("Ask me",runtime::CodexApprovals::Ask),("Never ask",runtime::CodexApprovals::Never)].into_iter().map(|(label,policy)|chip(SharedString::from(format!("ninja-approval-{label}")),label,p.profile.approvals==policy,cx,move|app,_,cx|{if let Some(p)=&mut app.ninjas.panel{p.profile.approvals=policy;}cx.notify();})))).child(div().text_size(theme::ui_px(10.)).text_color(rgb(MUTED)).child("Model and permission changes apply to future tasks. Memory follows this Ninja across chats.")).child(chip("ninja-save-profile",if p.busy{"Saving…"}else{"Save Ninja"},true,cx,|app,_,cx|app.ninja_save_profile(cx)));
        if p.profile.revision > 0
            && self
                .ninjas
                .hosts
                .iter()
                .any(|h| h.id == p.host && h.endpoint.is_some())
        {
            setup = setup.child(chip(
                "ninja-handoff",
                "Handoff current workspace via Taildrop",
                false,
                cx,
                |app, _, cx| app.ninja_handoff(cx),
            ));
        }
        if p.profile.revision > 0 {
            setup=setup.child(div().text_size(theme::ui_px(11.)).child("Personal memory")).child(div().h(px(90.)).child(p.fields[4].clone())).child(chip("ninja-save-memory","Save memory",false,cx,|app,_,cx|{if let Some(p)=&app.ninjas.panel{let v=json!({"action":"memory","id":p.profile.id,"revision":p.memory_revision,"content":p.fields[4].read(cx).text()});app.ninja_operation(v,cx);}}));
        }
        let tasks = snapshot
            .and_then(|v| v["tasks"].as_array())
            .into_iter()
            .flatten()
            .filter(|t| t["profile"] == p.profile.id)
            .take(40)
            .cloned()
            .collect::<Vec<_>>();
        let mut work = div()
            .flex()
            .flex_col()
            .gap_2()
            .min_w(px(0.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(theme::ui_px(15.))
                            .child("Chats and task briefs"),
                    )
                    .child(chip(
                        "ninja-new-chat",
                        "New chat",
                        false,
                        cx,
                        |app, _, cx| {
                            if let Some(p) = &mut app.ninjas.panel {
                                if p.busy {
                                    return;
                                }
                                p.chat = None;
                                p.task = None;
                            }
                            cx.notify();
                        },
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .id("ninja-task-history")
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(tasks.into_iter().map(|t| {
                        let id = t["id"].as_str().unwrap_or("").to_owned();
                        let selected = p.task.as_ref().is_some_and(|v| v["id"] == id);
                        div()
                            .id(SharedString::from(format!("ninja-task-{id}")))
                            .p_3()
                            .rounded_lg()
                            .bg(rgba(if selected { 0xffffff15 } else { 0xffffff07 }))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .cursor_pointer()
                            .child(
                                div()
                                    .text_size(theme::ui_px(11.))
                                    .text_color(rgb(MUTED))
                                    .child(format!(
                                        "{} · {}",
                                        t["state"].as_str().unwrap_or(""),
                                        t["model"].as_str().unwrap_or("")
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(theme::ui_px(12.))
                                    .child(t["prompt"].as_str().unwrap_or("").to_owned()),
                            )
                            .child(
                                div()
                                    .text_size(theme::ui_px(11.))
                                    .text_color(rgb(MUTED))
                                    .child(
                                        t["brief"]
                                            .as_str()
                                            .filter(|s| !s.is_empty())
                                            .or_else(|| t["status"].as_str())
                                            .unwrap_or("")
                                            .to_owned(),
                                    ),
                            )
                            .when(
                                ["running", "approval", "queued"]
                                    .contains(&t["state"].as_str().unwrap_or("")),
                                |s| {
                                    let id = id.clone();
                                    s.child(chip(
                                        SharedString::from(format!("ninja-stop-{id}")),
                                        "Stop",
                                        false,
                                        cx,
                                        move |app, _, cx| {
                                            app.ninja_operation(
                                                json!({"action":"stop","id":id}),
                                                cx,
                                            )
                                        },
                                    ))
                                },
                            )
                            .on_click(cx.listener(move |app, _, _, cx| {
                                cx.stop_propagation();
                                app.ninja_operation(json!({"action":"task","id":id}), cx);
                            }))
                            .into_any_element()
                    })),
            );
        let pending = snapshot
            .and_then(|v| v["tasks"].as_array())
            .into_iter()
            .flatten()
            .filter(|t| t["profile"] == p.profile.id)
            .flat_map(|t| {
                t["requests"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(move |r| (t["id"].as_str().unwrap_or("").to_owned(), r.clone()))
            })
            .collect::<Vec<_>>();
        for (task, r) in pending {
            if let Ok(request) = serde_json::from_value::<runtime::NativeRequest>(r) {
                work=work.child(div().p_3().rounded_lg().bg(rgba(0xffb46718)).flex().flex_col().gap_2().child(div().text_size(theme::ui_px(12.)).child(request.title.clone())).child(div().text_size(theme::ui_px(11.)).child(request.detail.clone())).when(matches!(request.kind,runtime::NativeRequestKind::Approval{..}),|s|{let choices=match &request.kind{runtime::NativeRequestKind::Approval{choices}=>choices.clone(),_=>vec![]};s.child(div().flex().gap_1().children(choices.into_iter().map(|c|{let task=task.clone();let token=request.id.clone();chip(SharedString::from(format!("ninja-response-{}-{}",token,c.id)),c.label,false,cx,move|app,_,cx|app.ninja_operation(json!({"action":"respond","id":task,"request":token,"answer":runtime::NativeAnswer::Choice(c.id.clone())}),cx))})))}));
            }
        }
        work = work
            .child(div().h(px(90.)).child(p.fields[3].clone()))
            .child(
                div().flex().gap_1().children(
                    ["Now", "Tonight 22:00", "Every 24h"]
                        .into_iter()
                        .enumerate()
                        .map(|(i, label)| {
                            chip(
                                SharedString::from(format!("ninja-schedule-{i}")),
                                label,
                                p.schedule == i,
                                cx,
                                move |app, _, cx| {
                                    if let Some(p) = &mut app.ninjas.panel {
                                        p.schedule = i;
                                    }
                                    cx.notify();
                                },
                            )
                        }),
                ),
            )
            .child(chip(
                "ninja-submit-task",
                if p.chat.is_some() {
                    "Continue chat"
                } else {
                    "Assign task"
                },
                true,
                cx,
                |app, _, cx| app.ninja_submit_task(cx),
            ));
        let width = (f32::from(window.viewport_size().width) - 48.).clamp(520., 1060.);
        let height = (f32::from(window.viewport_size().height) - 64.).clamp(380., 850.);
        let host_name = self
            .ninjas
            .hosts
            .iter()
            .find(|h| h.id == p.host)
            .map(|h| h.name.as_str())
            .unwrap_or("Host");
        div()
            .id("personal-ninjas-scrim")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000035))
            .on_click(cx.listener(|app, _, _, cx| {
                app.ninjas.panel = None;
                cx.notify();
            }))
            .child(
                div()
                    .id("personal-ninjas-panel")
                    .w(px(width))
                    .h(px(height))
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgba(0xffffff20))
                    .bg(rgba(self.appearance.tint(GLASS_SURFACE)))
                    .opacity(progress)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(icon("tailscale", 18., TEXT))
                                    .child(
                                        div()
                                            .text_size(theme::ui_px(12.))
                                            .child(host_name.to_owned()),
                                    ),
                            )
                            .child(chip(
                                "close-personal-ninjas",
                                "×",
                                false,
                                cx,
                                |app, _, cx| {
                                    app.ninjas.panel = None;
                                    cx.notify();
                                },
                            )),
                    )
                    .when_some(self.ninjas.errors.get(&p.host).cloned(), |s, e| {
                        s.child(
                            div()
                                .text_size(theme::ui_px(11.))
                                .text_color(rgb(0xffb18a))
                                .child(e),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .flex_1()
                            .min_h(px(0.))
                            .child(
                                div()
                                    .id("ninja-persona-fields")
                                    .w(px(width * 0.45))
                                    .h_full()
                                    .overflow_y_scroll()
                                    .child(setup),
                            )
                            .child(work.flex_1().h_full()),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn personal_panel_preserves_current_chat_and_draft(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                let active = app.active.clone();
                app.composer
                    .update(cx, |i, cx| i.set_text("Keep current draft", cx));
                app.open_ninjas(Some("local".into()), None, window, cx);
                assert!(app.ninjas.panel.is_some());
                assert_eq!(app.active, active);
                assert_eq!(app.composer.read(cx).text(), "Keep current draft");
                app.open_ninjas(Some("local".into()), None, window, cx);
                assert!(app.ninjas.panel.is_none());
                app.open_ninjas(Some("local".into()), None, window, cx);
                assert!(app.ninjas.panel.is_some());
            })
        });
        crate::ui_tests::draw_frame(&view, cx);
    }
    #[gpui::test]
    fn remote_reviews_keep_exact_owner_token_and_native_questions(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx,|app,cx|{
            let native=runtime::NativeRequest{id:"question-42".into(),method:"question".into(),title:"Choose deployment".into(),detail:"Exact remote request".into(),kind:runtime::NativeRequestKind::Questions{questions:vec![runtime::NativeQuestion{id:"target".into(),header:"Target".into(),question:"Which environment?".into(),options:vec![],secret:false,multiple:false,required:true}]}};
            app.ninjas.snapshots.insert("local".into(),json!({"profiles":[{"id":"personal","name":"Ice"}],"tasks":[{"id":"task-7","profile":"personal","state":"approval","requests":[native]}]}));
            let snapshot=app.unified_snapshot(cx);assert_eq!(snapshot["approvalThreadId"],"ninja:local:task-7");assert_eq!(snapshot["nativeReview"]["token"],"question-42");assert_eq!(snapshot["nativeReview"]["kind"],"questions");assert_eq!(snapshot["nativeReview"]["questions"][0]["id"],"target");assert_eq!(snapshot["activeTasks"][0]["scene"],"permission");assert_eq!(snapshot["approvalCount"],1);app.ninjas.errors.insert("local".into(),"Disconnected".into());let offline=app.unified_snapshot(cx);assert!(offline["nativeReview"].is_null());assert!(offline["activeTasks"][0]["text"].as_str().unwrap().contains("Reconnecting"));
        });
    }
}
