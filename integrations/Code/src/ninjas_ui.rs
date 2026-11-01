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
    pub drafts: HashMap<String,String>,
    pub started: bool,
    pub call: Option<crate::ninja_call_ui::Call>,
    pub desktop: Option<Entity<crate::remote_desktop::RemoteDesktop>>,
}
pub(super) struct Panel {
    pub host: String,
    pub profile: Profile,
    pub fields: Vec<Entity<TextInput>>,
    _subscriptions: Vec<Subscription>,
    pub models: Vec<Value>,
    pub busy: bool,
    pub error: Option<String>,
    pub task: Option<Value>,
    pub chat: Option<String>,
    pub memory_revision: u64,
    pub schedule: usize,
    pub settings: bool,
    pub messages: Vec<Value>,
    pub history_loading: bool,
    pub models_requested: bool,
    pub models_loading:Option<uuid::Uuid>,
    pub history_owner: Option<String>,
    pub effort_preview: Option<(String, Instant)>,
    pub decode: std::cell::RefCell<crate::text_decode::TextDecode>,
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
            drafts:serde_json::from_value::<HashMap<String,String>>(value["drafts"].clone()).unwrap_or_default().into_iter().filter(|(k,v)|k.len()<512 && v.len()<=input::MAX_DRAFT_BYTES).take(24).collect(),
            ..Default::default()
        }
    }
    pub(super) fn save(&self) {
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
        *pending.lock().unwrap() = Some(json!({"hosts":self.hosts,"snapshots":self.snapshots,"drafts":self.drafts}));
        let _ = wake.try_send(());
    }
    pub fn menu_identities(&self) -> Vec<Value> {
        self.hosts.iter().flat_map(|h|self.snapshots.get(&h.id).into_iter().flat_map(move|v|v["profiles"].as_array().into_iter().flatten().filter_map(move|p|{
            Some(json!({"id":format!("{}:{}",h.id,p["id"].as_str()?),"name":p["name"].as_str()?,"style":p["style"].as_str().unwrap_or("fire"),"host":h.name}))
        }))).collect()
    }
    pub fn current_menu_identity(&self) -> Value {
        let Some(p) = self.panel.as_ref().filter(|p| p.profile.revision > 0) else {
            return Value::Null;
        };
        self.menu_identities()
            .into_iter()
            .find(|v| v["id"] == format!("{}:{}", p.host, p.profile.id))
            .unwrap_or(Value::Null)
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
                out.push(json!({"id":format!("ninja:{}:{}",h.id,t["id"].as_str().unwrap_or("")),"runId":t["id"],"scene":if reconnecting{"thinking"}else{task_scene(t)},"text":if reconnecting{format!("{name} · Reconnecting to {}",h.name)}else{format!("{name} · {}",t["status"].as_str().unwrap_or("Working"))},"name":name,"agent":"ninja","ninjaStyle":p.and_then(|p|p["style"].as_str()).unwrap_or("fire")}));
            }
        }
        if let Some(call) = &self.call {
            let id = call
                .task
                .as_ref()
                .map(|task| format!("ninja:{}:{task}", call.host.id));
            if let Some(item) = out
                .iter_mut()
                .find(|v| Some(v["id"].as_str().unwrap_or("")) == id.as_deref())
            {
                if item["scene"] == "permission" {
                    return out;
                }
                item["scene"] = json!(call.scene());
                item["text"] = json!(format!(
                    "{} · {}",
                    call.name,
                    if call.connected && call.backend {
                        "On call"
                    } else {
                        "Calling…"
                    }
                ));
            } else {
                out.push(json!({"id":id.unwrap_or_else(||format!("ninja-call:{}",call.session)),"runId":call.session,"scene":call.scene(),"name":call.name,"text":format!("{} · Calling…",call.name),"agent":"ninja","ninjaStyle":call.style}));
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
pub(super) fn call(host: &Host, value: &Value) -> anyhow::Result<Value> {
    if let Some(endpoint) = &host.endpoint {
        let v = ninja::http::json(
            "POST",
            &format!("{}/agents", endpoint.trim_end_matches('/')),
            Some(value),
            Duration::from_secs(35),
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
    chip_content(id, label.into(), None, on, cx, action)
}
fn marked_chip(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    mark: impl IntoElement,
    on: bool,
    cx: &Context<Orbit>,
    action: impl Fn(&mut Orbit, &mut Window, &mut Context<Orbit>) + 'static,
) -> Stateful<Div> {
    chip_content(
        id,
        label.into(),
        Some(mark.into_any_element()),
        on,
        cx,
        action,
    )
}
fn host_glyph(host: &Host) -> &'static str {
    let name = host.name.to_ascii_lowercase();
    if name.contains("book") || name.contains("laptop") {
        "machine-laptop"
    } else if ["server", "linux", "ubuntu", " vm"]
        .iter()
        .any(|s| name.contains(s))
    {
        "machine-server"
    } else {
        "machine-desktop"
    }
}
fn field(input: &Entity<TextInput>, glyph: &'static str) -> Div {
    div()
        .min_w(px(0.))
        .px_2()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(rgba(0xffffff14))
        .bg(rgba(GLASS_SURFACE))
        .flex()
        .items_start()
        .gap_2()
        .child(div().flex_none().pt_1().child(icon(glyph, 14., MUTED)))
        .child(div().flex_1().min_w(px(0.)).child(input.clone()))
}
fn chip_content(
    id: impl Into<SharedString>,
    label: SharedString,
    mark: Option<AnyElement>,
    on: bool,
    cx: &Context<Orbit>,
    action: impl Fn(&mut Orbit, &mut Window, &mut Context<Orbit>) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let label_selector = format!("{id}-label");
    let selector = id.to_string();
    let model_row = id.starts_with("ninja-model-");
    div()
        .id(id)
        .debug_selector(move || selector.clone())
        .h(px(28.))
        .flex_none()
        .px_2()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .rounded_md()
        .bg(rgba(if on { 0x9380ff35 } else { 0xffffff09 }))
        .hover(|s| s.bg(rgba(GLASS_HOVER)))
        .text_size(theme::ui_px(11.))
        .text_color(rgb(TEXT))
        .cursor_pointer()
        .min_w(px(0.))
        .max_w_full()
        .when_some(mark, |v, mark| v.child(div().flex_none().child(mark)))
        .when(!label.is_empty(), |row| {
            row.child(
                div()
                    .debug_selector(move || label_selector.clone())
                    .when(model_row, |v| v.flex_1().min_w(px(0.)).truncate())
                    .child(label),
            )
        })
        .on_click(cx.listener(move |app, _, w, cx| {
            cx.stop_propagation();
            action(app, w, cx)
        }))
}
fn host_address(host: &Host) -> Option<&str> {
    let endpoint = host.endpoint.as_deref()?;
    let authority = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))?
        .split('/')
        .next()?;
    let name = authority.split(':').next()?;
    (!name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
    .then_some(name)
}
fn bubble(user: bool, text: String, style: &str) -> Div {
    div()
        .w_full()
        .flex()
        .gap_2()
        .items_start()
        .when(user, |s| s.justify_end())
        .when(!user, |s| s.child(avatar(style, 24.)))
        .child(
            div()
                .max_w(relative(if user { 0.82 } else { 0.9 }))
                .min_w(px(0.))
                .px_3()
                .py_2()
                .rounded_lg()
                .bg(rgba(if user { 0x9380ff32 } else { 0xffffff0b }))
                .text_size(theme::ui_px(13.))
                .child(text),
        )
}
pub(super) fn task_scene(task: &Value) -> &'static str {
    match task["state"].as_str().unwrap_or("") {
        "approval" => "permission",
        "completed" => "success",
        "failed" | "interrupted" => "failure",
        "stopped" => "declined",
        "queued" => "waiting",
        _ => kai_icons::scene_for_status(task["status"].as_str().unwrap_or("Thinking")),
    }
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
            self.ninja_conversation(cx);
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

    fn ninja_screen(&mut self, take_over: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ninjas.panel.as_ref() else {
            return;
        };
        let Some(host) = self
            .ninjas
            .hosts
            .iter()
            .find(|h| h.id == p.host && h.endpoint.is_some())
            .cloned()
        else {
            return;
        };
        let Some(address) = host_address(&host).map(str::to_owned) else {
            return;
        };
        let username=self.ninjas.snapshots.get(&host.id).and_then(|v|v["home"].as_str()).and_then(|s|s.rsplit('/').next()).unwrap_or("").to_owned();
        let id = p.profile.id.clone();
        let desktop=self.ninjas.desktop.clone().unwrap_or_else(||cx.new(|cx|crate::remote_desktop::RemoteDesktop::new(address,host.name.clone(),username,window,cx)));
        self.ninjas.desktop=Some(desktop.clone());
        if !take_over {cx.notify();return;}
        self.ninjas.call.take();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { call(&host, &json!({"action":"take_over","id":id})) })
                .await;
            let _ = this.update(cx, |app, cx| {
                match result {
                    Ok(_) => {
                        desktop.update(cx,|view,cx|view.controlling(true,cx));
                    }
                    Err(e) => app.notice = Some(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn ninja_send_reference(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.ninjas.panel.as_ref().filter(|p| !p.busy) else {
            return;
        };
        let Some(host) = self
            .ninjas
            .hosts
            .iter()
            .find(|h| h.id == p.host && h.endpoint.is_some())
            .cloned()
        else {
            return;
        };
        let token = p.token;
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Send reference files to Ninja host".into()),
        });
        cx.spawn(async move|this,cx|{
            let Ok(Ok(Some(paths)))=paths.await else{return};
            let remote=host.clone();
            let result=cx.background_executor().spawn(async move{(||->anyhow::Result<Vec<String>>{
                anyhow::ensure!(!paths.is_empty()&&paths.len()<=8,"Send up to eight reference files at once");
                let address=host_address(&remote).ok_or_else(||anyhow::anyhow!("Invalid Taildrop host"))?;
                let cli=crate::process_env::dirs(&dirs::home_dir().unwrap_or_default()).into_iter().map(|p|p.join("tailscale")).chain([PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale")]).find(|p|p.is_file()).ok_or_else(||anyhow::anyhow!("Tailscale CLI unavailable"))?;
                let mut names=vec![];
                for path in paths {let metadata=std::fs::symlink_metadata(&path)?;anyhow::ensure!(metadata.is_file()&&metadata.len()<=64*1024*1024,"References must be regular files up to 64 MiB");
                    let mut command=std::process::Command::new(&cli);command.args(["file","cp"]).arg(&path).arg(format!("{address}:"));
                    ninja::http::run(command,None,Duration::from_secs(120))?;
                    names.push(path.file_name().unwrap_or_default().to_string_lossy().into_owned());
                }Ok(names)
            })()}).await;
            let _=this.update(cx,|app,cx|{
                if let Some(p)=app.ninjas.panel.as_mut().filter(|p|p.token==token){match result{Ok(names)=>{
                    let note=format!("\nReference files sent via Taildrop to this host's Downloads (reference only; keep working in your configured workspace): {}",names.join(", "));
                    let draft=p.fields[3].read(cx).text();p.fields[3].update(cx,|input,cx|input.set_text(&format!("{draft}{note}"),cx));
                },Err(e)=>app.notice=Some(e.to_string())}}cx.notify();
            });
        }).detach();
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
                                let refresh_history = app.ninjas.panel.as_ref().is_some_and(|p| {
                                    p.host == host.id
                                        && value["tasks"].as_array().into_iter().flatten().any(
                                            |t| {
                                                t["profile"] == p.profile.id
                                                    && t["chat"].as_str() == p.chat.as_deref()
                                                    && t["finished"].is_number()
                                                    && !previous.is_some_and(|v| {
                                                        v["tasks"]
                                                            .as_array()
                                                            .into_iter()
                                                            .flatten()
                                                            .any(|old| {
                                                                old["id"] == t["id"]
                                                                    && old["finished"]
                                                                        == t["finished"]
                                                            })
                                                    })
                                            },
                                        )
                                });
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
                                if refresh_history {
                                    app.ninja_conversation(cx);
                                }
                                if app.ninjas.panel.as_ref().is_some_and(|p| {
                                    p.host == host.id
                                        && p.profile.revision == 0
                                        && !p.models_requested
                                        && !p.busy
                                        && !p.fields[2].read(cx).text().is_empty()
                                }) {
                                    if let Some(p) = &mut app.ninjas.panel {
                                        let providers = app.ninjas.snapshots[&host.id]["providers"]
                                            .as_array()
                                            .cloned()
                                            .unwrap_or_default();
                                        if !providers.iter().any(|v| {
                                            v["installed"] == true
                                                && v["agent"] == json!(p.profile.agent)
                                        }) {
                                            if let Some(agent) = providers
                                                .iter()
                                                .filter(|v| v["installed"] == true)
                                                .find_map(|v| {
                                                    serde_json::from_value(v["agent"].clone()).ok()
                                                })
                                            {
                                                p.profile.agent = agent;
                                                p.profile.model = None;
                                                p.profile.effort = None;
                                            }
                                        }
                                    }
                                    app.ninja_load_models(cx);
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
        self.remember_ninja_draft(cx);
        self.ninjas.desktop=None;
        if self.ninjas.panel.as_ref().is_some_and(|p| {
            (profile.is_none()
                && p.profile.revision == 0
                && host.as_deref().is_none_or(|h| h == p.host))
                || (host.as_deref() == Some(p.host.as_str())
                    && profile.as_deref() == Some(p.profile.id.as_str()))
        }) {
            self.ninjas.desktop=None;self.ninjas.panel = None;
            cx.notify();
            return;
        }
        let selected = host
            .or_else(|| {
                self.ninja
                    .draft
                    .node
                    .then(|| self.ninja.draft.machine.clone())
            })
            .or_else(|| {
                self.ninjas
                    .hosts
                    .iter()
                    .find(|h| h.endpoint.is_some() && h.id == self.ninja.draft.machine)
                    .map(|h| h.id.clone())
            })
            .or_else(|| {
                self.ninjas
                    .hosts
                    .iter()
                    .find(|h| h.endpoint.is_some())
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
        let mut p=existing.unwrap_or_else(||Profile{id:uuid::Uuid::new_v4().to_string(),name:String::new(),persona:"A resourceful personal partner. Complete tasks, verify results, and remember useful long-term context.".into(),style:"fire".into(),project:if selected=="local"{self.execution_root().unwrap_or_else(||self.project.clone())}else{snapshot.and_then(|v|v["home"].as_str()).map(PathBuf::from).unwrap_or_default()},agent:self.agent,model:self.request_model(),effort:self.request_effort(),ninja:(self.agent==AgentKind::Ninja).then(||self.ninja_selection()),edits:self.allow_edits,access:self.codex_access,approvals:self.codex_approvals,revision:0});
        if p.revision == 0 && selected != "local" {
            p.model = None;
            p.effort = None;
        }
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
            let task_input = fields.len() == 3;
            subscriptions.push(cx.subscribe(&field, move |app, _, event, cx| {
                if task_input && matches!(event, InputEvent::Submit) {
                    app.ninja_submit_task(cx);
                }
                cx.notify();
            }));
            fields.push(field);
        }
        let latest = snapshot
            .and_then(|v| v["tasks"].as_array())
            .and_then(|ts| ts.iter().find(|t| t["profile"] == p.id))
            .cloned();
        let settings = p.revision == 0;
        let effort_preview = p.effort.clone().map(|v| (v, Instant::now()));
        self.ninjas.panel = Some(Panel {
            host: selected.clone(),
            profile: p,
            fields,
            _subscriptions: subscriptions,
            models: vec![],
            busy: false,
            error: None,
            chat: latest
                .as_ref()
                .and_then(|t| t["chat"].as_str())
                .map(str::to_owned),
            task: latest,
            memory_revision: 0,
            schedule: 0,
            settings,
            messages: vec![],
            history_loading: false,
            models_requested: false,
            models_loading:None,
            history_owner: None,
            effort_preview,
            decode:Default::default(),
            at: Instant::now(),
            token: uuid::Uuid::new_v4(),
        });
        if let Some(p)=&self.ninjas.panel {let key=format!("{}:{}:{}",p.host,p.profile.id,p.chat.as_deref().unwrap_or("draft"));if let Some(draft)=self.ninjas.drafts.get(&key){p.fields[3].update(cx,|input,cx|input.set_text(draft,cx));}}
        self.modal = None;
        self.agent_picker = None;
        self.overlay_window = Some(window.window_handle().window_id());
        if let Some(host) = self.ninjas.hosts.iter().find(|h| h.id == selected).cloned() {
            self.start_ninja_watch(host, cx);
        }
        if profile.is_some() {
            self.ninja_conversation(cx);
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
    pub(super) fn ninja_conversation(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &mut self.ninjas.panel else {
            return;
        };
        if (p.history_loading && p.history_owner == p.chat) || p.profile.revision == 0 {
            return;
        }
        let Some(chat) = p.chat.clone() else {
            p.messages.clear();
            return;
        };
        let Some(host) = self.ninjas.hosts.iter().find(|h| h.id == p.host).cloned() else {
            return;
        };
        p.history_loading = true;
        p.history_owner = Some(chat.clone());
        let token = p.token;
        let profile = p.profile.id.clone();
        cx.spawn(async move |this, cx| {
            let owner = chat.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    call(
                        &host,
                        &json!({"action":"conversation","id":profile,"chat":chat}),
                    )
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                if let Some(p) = app
                    .ninjas
                    .panel
                    .as_mut()
                    .filter(|p| p.token == token && p.history_owner.as_deref() == Some(&owner))
                {
                    p.history_loading = false;
                    if p.chat.as_deref() == Some(&owner) {
                        match result {
                            Ok(v) => {
                                p.messages = v["messages"].as_array().cloned().unwrap_or_default()
                            }
                            Err(e) => p.error = Some(e.to_string()),
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
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
        let model_owner=(action=="models").then(uuid::Uuid::new_v4);
        if let Some(owner)=model_owner{panel.models_loading=Some(owner);}else{panel.busy=true;}
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { call(&host, &request) })
                .await;
            let _ = this.update(cx, |app, cx| {
                let Some(panel) = app.ninjas.panel.as_mut().filter(|p| p.token == token) else {
                    return;
                };
                if let Some(owner)=model_owner{if panel.models_loading!=Some(owner){return}panel.models_loading=None;}else{panel.busy=false;}
                match result {
                    Ok(value) => {
                        app.ninjas.errors.remove(&panel.host);
                        panel.error = None;
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
                                panel.effort_preview =
                                    panel.profile.effort.clone().map(|v| (v, Instant::now()));
                            }
                            "save_profile" => {
                                if let Ok(p) = serde_json::from_value::<Profile>(value) {
                                    panel.profile.revision = p.revision;
                                    panel.settings = false;
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
                                app.remember_ninja_draft(cx);app.ninjas.desktop=None;app.ninjas.panel = None;
                            }
                            _ => {}
                        }
                    }
                    Err(e) => {
                        panel.error = Some(e.to_string());
                    }
                }
                if matches!(action.as_str(), "task" | "submit") {
                    app.ninja_conversation(cx);
                }
                if action == "save_profile" && app.ninjas.panel.as_ref().is_some_and(|p|!p.models_requested){app.ninja_load_models(cx);}
                if action == "memory"
                    && app
                        .ninjas
                        .panel
                        .as_ref()
                        .is_some_and(|p| p.models.is_empty())
                {
                    app.ninja_load_models(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn ninja_load_models(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.ninjas.panel else { return };
        if p.busy {
            return;
        }
        let request = json!({"action":"models","agent":p.profile.agent,"project":p.fields[2].read(cx).text(),"model":p.profile.model,"ninja":p.profile.ninja});
        self.ninjas.panel.as_mut().unwrap().models_requested = true;
        self.ninja_operation(request, cx);
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
    pub(super) fn ninja_submit_task(&mut self, cx: &mut Context<Self>) {
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
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon("ninja", 15., TEXT))
                            .child("Ninjas"),
                    )
                    .child(marked_chip(
                        "new-personal-ninja",
                        "+",
                        icon("ninja", 14., TEXT),
                        false,
                        cx,
                        |app, w, cx| app.open_ninjas(None, None, w, cx),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
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
                            .w_full()
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
                            .child(crate::ninja_art::nameplate(name,p["style"].as_str().unwrap_or("fire"),active,self.motion && !native::reduced_motion() && cx.active_window().is_some()))
                            .when(active, |v| {
                                let task = self.ninjas.snapshots[&host]["tasks"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .find(|t| {
                                        t["profile"] == id
                                            && ["running", "approval"]
                                                .contains(&t["state"].as_str().unwrap_or(""))
                                    })
                                    .unwrap();
                                v.child(self.sidebar_status_mark(
                                    if task["state"] == "approval" {
                                        "permission"
                                    } else {
                                        kai_icons::scene_for_status(
                                            task["status"].as_str().unwrap_or("Working"),
                                        )
                                    },
                                    cx.active_window().is_some(),
                                ))
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
        let compact = f32::from(window.viewport_size().width) < 640.;
        let Some(p) = &self.ninjas.panel else {
            return div().into_any_element();
        };
        if let Some(desktop)=&self.ninjas.desktop {
            return div().absolute().inset_0().p_4().bg(rgba(appearance::MENU_SURFACE)).flex().flex_col().gap_2()
                .child(div().flex().gap_2()
                    .child(chip("desktop-observe","Watch",false,cx,|app,_,cx|{if let Some(v)=&app.ninjas.desktop{v.update(cx,|v,cx|v.controlling(false,cx));}}))
                    .child(chip("desktop-takeover","Take over",false,cx,|app,w,cx|app.ninja_screen(true,w,cx)))
                    .child(chip("desktop-close","Close viewer",false,cx,|app,_,cx|{app.ninjas.desktop=None;cx.notify();})))
                .child(desktop.clone()).into_any_element();
        }
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
            .w_full()
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
            .child(field(&p.fields[0], "ninja"))
            .child(field(&p.fields[1], "chat").min_h(px(76.)))
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
                        marked_chip(
                            SharedString::from(format!("ninja-host-{id}")),
                            host.name.clone(),
                            icon(host_glyph(host), 16., TEXT),
                            p.host == id,
                            cx,
                            move |app, w, cx| app.open_ninjas(Some(id.clone()), None, w, cx),
                        )
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(ninjas::styles().into_iter().map(|(id, _name, _)| {
                        marked_chip(
                            SharedString::from(format!("ninja-style-{id}")),
                            "",
                            avatar(id, 26.),
                            p.profile.style == id,
                            cx,
                            move |app, _, cx| {
                                if let Some(p) = &mut app.ninjas.panel {
                                    p.profile.style = id.into();
                                }
                                cx.notify();
                            },
                        )
                        .w(px(42.))
                        .h(px(38.))
                    })),
            )
            .child(
                field(&p.fields[2], "workspace-folder-open")
                    .debug_selector(|| "ninja-workspace-field".into()),
            );
        setup = setup.child(
            div().flex().flex_wrap().gap_1().children(
                providers
                    .iter()
                    .filter(|v| v["installed"] == true)
                    .filter_map(|v| serde_json::from_value::<AgentKind>(v["agent"].clone()).ok())
                    .map(|agent| {
                        marked_chip(
                            SharedString::from(format!("ninja-provider-{}", agent.command())),
                            agent.name(),
                            icon(agent_icon(agent), 16., agent_color(agent)),
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
                        .flex()
                        .items_center()
                        .gap_2()
                        .when_some(p.profile.ninja.as_ref(), |v, n| {
                            v.child(icon(
                                match n.engine {
                                    ninja::Engine::Ollama => "lab-ollama",
                                    ninja::Engine::LlamaCpp => "engine-llamacpp",
                                },
                                16.,
                                TEXT,
                            ))
                        })
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
                            marked_chip(
                                SharedString::from(format!("ninja-harness-{label}")),
                                label,
                                icon(
                                    if h == ninja::Harness::Codex {
                                        "codex"
                                    } else {
                                        "claude"
                                    },
                                    16.,
                                    TEXT,
                                ),
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
                .child(field(&p.fields[5], "search").flex_1())
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
                .w_full()
                .min_w(px(0.))
                .flex_none()
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
                            marked_chip(
                                SharedString::from(format!("ninja-model-{id}")),
                                composer_design::display_label(
                                    model["name"].as_str().unwrap_or(&id),
                                ),
                                icon(
                                    composer_design::model_glyph(&id)
                                        .unwrap_or(agent_icon(p.profile.agent)),
                                    16.,
                                    if p.profile.agent == AgentKind::Ninja {
                                        ninja::lab_color(&id)
                                    } else {
                                        TEXT
                                    },
                                ),
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
                            .w_full()
                            .justify_start()
                        }),
                ),
        );
        setup = setup.child(
            div().w_full().flex().flex_col().gap_1().children(
                efforts
                    .iter()
                    .filter_map(Value::as_str)
                    .enumerate()
                    .map(|(i, effort)| {
                        let value = effort.to_owned();
                        let hover_value = value.clone();
                        let active = p.profile.effort.as_deref() == Some(effort);
                        let color = composer_design::effort_color(
                            effort,
                            i as f32 / efforts.len().saturating_sub(1).max(1) as f32,
                        );
                        let elapsed = p
                            .effort_preview
                            .as_ref()
                            .filter(|(v, _)| v == effort)
                            .map(|(_, at)| at.elapsed().as_secs_f32());
                        let motion = self.motion && !native::reduced_motion();
                        if motion && window.is_window_active() && elapsed.is_some_and(|t| t < 5.) {
                            request_motion_frame(window, cx);
                        }
                        div()
                            .id(SharedString::from(format!("ninja-effort-{value}")))
                            .debug_selector({
                                let v = value.clone();
                                move || format!("ninja-effort-{v}")
                            })
                            .w_full()
                            .h(px(32.))
                            .flex_none()
                            .relative()
                            .overflow_hidden()
                            .rounded_md()
                            .bg(rgba(if active { 0xffffff14 } else { 0xffffff05 }))
                            .cursor_pointer()
                            .child(composer_design::shimmer_tiles(elapsed, color, motion))
                            .child(
                                div()
                                    .relative()
                                    .px_3()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .text_size(theme::ui_px(12.))
                                    .text_color(rgb(color))
                                    .child(composer_design::title(effort))
                                    .when(active, |s| s.child("✓")),
                            )
                            .on_hover(cx.listener(move |app, hover: &bool, _, cx| {
                                if let Some(p) = &mut app.ninjas.panel {
                                    p.effort_preview = if *hover {
                                        Some((hover_value.clone(), Instant::now()))
                                    } else {
                                        p.profile.effort.clone().map(|v| (v, Instant::now()))
                                    };
                                }
                                cx.notify();
                            }))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(p) = &mut app.ninjas.panel {
                                    p.profile.effort = Some(value.clone());
                                    p.effort_preview = Some((value.clone(), Instant::now()));
                                    if let Some(n) = &mut p.profile.ninja {
                                        n.thinking = Some(value.clone());
                                    }
                                }
                                cx.notify();
                            }))
                    }),
            ),
        );
        setup=setup.child(div().flex().gap_1().children([("Read only",false,runtime::CodexAccess::Workspace),("Workspace",true,runtime::CodexAccess::Workspace),("Full access",true,runtime::CodexAccess::Full)].into_iter().map(|(label,edit,access)|marked_chip(SharedString::from(format!("ninja-access-{label}")),label,icon(if !edit{"lock-keyhole"}else if access==runtime::CodexAccess::Full{"lock-keyhole-open"}else{"shield"},14.,if !edit{TEXT}else if access==runtime::CodexAccess::Full{0xf38c82}else{0xf2b66e}),p.profile.edits==edit&&p.profile.access==access,cx,move|app,_,cx|{if let Some(p)=&mut app.ninjas.panel{p.profile.edits=edit;p.profile.access=access;}cx.notify();})))).child(div().flex().gap_1().children([("Ask me",runtime::CodexApprovals::Ask),("Never ask",runtime::CodexApprovals::Never)].into_iter().map(|(label,policy)|chip(SharedString::from(format!("ninja-approval-{label}")),label,p.profile.approvals==policy,cx,move|app,_,cx|{if let Some(p)=&mut app.ninjas.panel{p.profile.approvals=policy;}cx.notify();})))).child(div().text_size(theme::ui_px(10.)).text_color(rgb(MUTED)).child("Model and permission changes apply to future tasks. Memory follows this Ninja across chats.")).child(chip("ninja-save-profile",if p.busy{"Saving…"}else{"Save Ninja"},true,cx,|app,_,cx|app.ninja_save_profile(cx)));
        if p.profile.revision > 0 {
            setup=setup.child(div().text_size(theme::ui_px(11.)).child("Personal memory")).child(field(&p.fields[4],"file").min_h(px(90.))).child(chip("ninja-save-memory","Save memory",false,cx,|app,_,cx|{if let Some(p)=&app.ninjas.panel{let v=json!({"action":"memory","id":p.profile.id,"revision":p.memory_revision,"content":p.fields[4].read(cx).text()});app.ninja_operation(v,cx);}}));
        }
        let summaries = snapshot
            .and_then(|v| v["tasks"].as_array())
            .into_iter()
            .flatten()
            .filter(|t| t["profile"] == p.profile.id)
            .cloned()
            .collect::<Vec<_>>();
        let mut tasks = p.messages.clone();
        for t in summaries
            .iter()
            .filter(|t| p.chat.as_deref() == t["chat"].as_str())
        {
            if let Some(old) = tasks.iter_mut().find(|v| v["id"] == t["id"]) {
                // Full completed messages supersede clipped summaries; active
                // stream updates always supersede a previously loaded body.
                if old["finished"] != t["finished"]
                    || ["running", "approval", "queued"]
                        .contains(&t["state"].as_str().unwrap_or(""))
                {
                    *old = t.clone();
                }
            } else {
                tasks.push(t.clone());
            }
        }
        tasks.sort_by_key(|t| t["created"].as_i64().unwrap_or(0));
        let held = snapshot
            .and_then(|v| v["profiles"].as_array())
            .into_iter()
            .flatten()
            .find(|v| v["id"] == p.profile.id)
            .is_some_and(|v| v["held"] == true);
        let empty=tasks.is_empty();
        let mut work = div()
            .flex()
            .flex_col()
            .gap_2()
            .min_w(px(0.))
            .when(empty,|v|v.child(div().flex().justify_center().child(crate::ninja_art::art(&p.profile.style,80.,self.motion && !native::reduced_motion() && window.is_window_active()))))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(theme::ui_px(15.))
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(crate::ninja_art::art(&p.profile.style,28.,self.motion && !native::reduced_motion() && window.is_window_active()))
                            .child(if p.profile.name.is_empty() {
                                "New Ninja".to_owned()
                            } else {
                                p.profile.name.clone()
                            }),
                    )
                    .when(p.profile.revision > 0, |s| {
                        s.child(marked_chip(
                            "ninja-call",
                            if compact { "" } else if self.ninjas.call.is_some() {
                                "End call"
                            } else {
                                "Call"
                            },
                            icon("phone", 16., TEXT),
                            self.ninjas.call.is_some(),
                            cx,
                            |app, w, cx| app.ninja_call_toggle(w, cx),
                        ))
                    })
                    .when(
                        self.ninjas
                            .hosts
                            .iter()
                            .any(|h| h.id == p.host && h.endpoint.is_some()),
                        |s| {
                            s.child(marked_chip(
                                "ninja-screen",
                                "Computer",
                                icon("machine-desktop", 16., TEXT),
                                false,
                                cx,
                                |app, w, cx| app.ninja_screen(false, w, cx),
                            ))
                            .when(!compact, |s| s.child(chip(
                                "ninja-take-over",
                                "Take over",
                                false,
                                cx,
                                |app, w, cx| app.ninja_screen(true, w, cx),
                            )))
                        },
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
                                p.messages.clear();
                            }
                            cx.notify();
                        },
                    )),
            )
            .child(
                div()
                    .id("ninja-conversations")
                    .w_full()
                    .flex_none()
                    .overflow_x_scroll()
                    .flex()
                    .gap_1()
                    .children({
                        let mut seen = HashSet::new();
                        summaries
                            .iter()
                            .filter(|t| seen.insert(t["chat"].clone().to_string()))
                            .take(12)
                            .map(|t| {
                                let chat = t["chat"].as_str().unwrap_or("").to_owned();
                                let selected = p.chat.as_deref() == Some(&chat);
                                let id = chat.clone();
                                chip(
                                    SharedString::from(format!("ninja-conversation-{chat}")),
                                    short(t["prompt"].as_str().unwrap_or("Chat"), 26),
                                    selected,
                                    cx,
                                    move |app, _, cx| {
                                        if let Some(p) = &mut app.ninjas.panel {
                                            p.chat = Some(id.clone());
                                            p.task = None;
                                            p.messages.clear();
                                        }
                                        app.ninja_conversation(cx);
                                        cx.notify();
                                    },
                                )
                                .flex_none()
                            })
                            .collect::<Vec<_>>()
                    }),
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
                        let selected = p
                            .task
                            .as_ref()
                            .filter(|v| v["id"] == id && v["finished"] == t["finished"]);
                        let prompt = selected
                            .and_then(|v| v["prompt"].as_str())
                            .or_else(|| t["prompt"].as_str())
                            .unwrap_or("")
                            .to_owned();
                        let brief = selected
                            .filter(|_| {
                                !["running", "approval"]
                                    .contains(&t["state"].as_str().unwrap_or(""))
                            })
                            .and_then(|v| v["brief"].as_str())
                            .or_else(|| t["brief"].as_str())
                            .unwrap_or("")
                            .to_owned();
                        let brief=if self.motion && self.unified_text_morph && !native::reduced_motion(){
                            use std::hash::{Hash,Hasher};let mut hash=std::collections::hash_map::DefaultHasher::new();id.hash(&mut hash);
                            let (frame,active)=p.decode.borrow_mut().frame(hash.finish() as usize,&brief,Instant::now());if active{request_motion_frame(window,cx);}frame
                        }else{brief};
                        let state = t["state"].as_str().unwrap_or("");
                        let running = ["running", "approval", "queued"].contains(&state);
                        let scene = task_scene(&t);
                        let mut row = div()
                            .id(SharedString::from(format!("ninja-task-{id}")))
                            .w_full()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap_2();
                        if t["call"].is_string() {
                            row = row.child(
                                div()
                                    .text_size(theme::ui_px(10.))
                                    .text_color(rgb(MUTED))
                                    .child("Call"),
                            );
                            for line in t["utterances"].as_array().into_iter().flatten() {
                                row = row.child(bubble(
                                    line["role"] == "user",
                                    line["text"].as_str().unwrap_or("").to_owned(),
                                    &p.profile.style,
                                ));
                            }
                        } else {
                            row = row
                                .child(bubble(true, prompt, &p.profile.style))
                                .child(bubble(false, brief, &p.profile.style));
                        }
                        row = row.child(
                            div()
                                .pl(px(32.))
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(self.sidebar_status_mark(
                                    scene,
                                    running && window.is_window_active(),
                                ))
                                .child(
                                    div()
                                        .text_size(theme::ui_px(10.))
                                        .text_color(rgb(MUTED))
                                        .child(t["status"].as_str().unwrap_or("").to_owned()),
                                ),
                        );
                        if running {
                            let stop = id.clone();
                            row = row.child(chip(
                                SharedString::from(format!("ninja-stop-{id}")),
                                "Stop",
                                false,
                                cx,
                                move |app, _, cx| {
                                    app.ninja_operation(json!({"action":"stop","id":stop}), cx)
                                },
                            ));
                        }
                        let row = row.on_click(cx.listener(move |app, _, _, cx| {
                            cx.stop_propagation();
                            app.ninja_operation(json!({"action":"task","id":id}), cx);
                        }));
                        if self.motion && !native::reduced_motion() {
                            row.with_animation(
                                SharedString::from(format!(
                                    "ninja-message-{}",
                                    t["id"].as_str().unwrap_or("")
                                )),
                                Animation::new(Duration::from_millis(220)),
                                |s, t| s.opacity(t).relative().top(px((1. - t) * 5.)),
                            )
                            .into_any_element()
                        } else {
                            row.into_any_element()
                        }
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
            let child = r.get("childId").cloned().unwrap_or(Value::Null);
            if let Ok(request) = serde_json::from_value::<runtime::NativeRequest>(r) {
                work=work.child(div().p_3().rounded_lg().bg(rgba(0xffb46718)).flex().flex_col().gap_2().child(div().text_size(theme::ui_px(12.)).child(request.title.clone())).child(div().text_size(theme::ui_px(11.)).child(request.detail.clone())).when(matches!(request.kind,runtime::NativeRequestKind::Approval{..}),|s|{let choices=match &request.kind{runtime::NativeRequestKind::Approval{choices}=>choices.clone(),_=>vec![]};s.child(div().flex().gap_1().children(choices.into_iter().map(|c|{let task=task.clone();let token=request.id.clone();let child=child.clone();chip(SharedString::from(format!("ninja-response-{}-{}",token,c.id)),c.label,false,cx,move|app,_,cx|app.ninja_operation(json!({"action":"respond","id":task,"request":token,"child":child,"answer":runtime::NativeAnswer::Choice(c.id.clone())}),cx))})))}));
            }
        }
        if let Some(call) = &self.ninjas.call {
            work = work.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(rgba(0x9380ff18))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.sidebar_status_mark(call.scene(), true))
                    .child(div().flex_1().text_size(theme::ui_px(12.)).child(format!(
                        "{} · {} · {}",
                        call.name,
                        if call.connected && call.backend {
                            "On call"
                        } else {
                            "Calling…"
                        },
                        chat_timing::duration(call.started.elapsed().as_millis() as u64)
                    )))
                    .child(chip(
                        "ninja-call-mute",
                        if call.muted { "Unmute" } else { "Mute" },
                        call.muted,
                        cx,
                        |app, _, cx| {
                            if let Some(c) = &mut app.ninjas.call {
                                c.muted = !c.muted;
                                if let Err(e) = c.media.mute(c.muted) {
                                    app.notice = Some(e.to_string());
                                }
                            }
                            cx.notify();
                        },
                    ))
                    .child(chip("ninja-call-end", "End", false, cx, |app, _, cx| {
                        app.ninjas.call.take();
                        cx.notify();
                    })),
            );
        }
        if held {
            work = work.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(rgba(0xf2b66e18))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(theme::ui_px(12.))
                            .child("You have control · Ninja paused"),
                    )
                    .child(chip(
                        "ninja-resume",
                        "Resume Ninja",
                        true,
                        cx,
                        |app, _, cx| {
                            if let Some(p) = &app.ninjas.panel {
                                app.ninja_operation(
                                    json!({"action":"resume","id":p.profile.id}),
                                    cx,
                                );
                            }
                        },
                    )),
            );
        }
        work = work
            .child(field(&p.fields[3], "chat").min_h(px(68.)))
            .when(
                self.ninjas
                    .hosts
                    .iter()
                    .any(|h| h.id == p.host && h.endpoint.is_some()),
                |s| {
                    s.child(marked_chip(
                        "ninja-send-reference",
                        "Send reference",
                        icon("plus", 14., TEXT),
                        false,
                        cx,
                        |app, _, cx| app.ninja_send_reference(cx),
                    ))
                },
            )
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
        let width = (f32::from(window.viewport_size().width) - 24.).clamp(280., 1060.);
        let height = (f32::from(window.viewport_size().height) - 24.).clamp(280., 850.);
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
                app.remember_ninja_draft(cx);app.ninjas.desktop=None;app.ninjas.panel = None;
                cx.notify();
            }))
            .child(
                div()
                    .id("personal-ninjas-panel")
                    .debug_selector(|| "personal-ninjas-panel".into())
                    .w(px(width))
                    .h(px(height))
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgba(0xffffff20))
                    .bg(rgba(appearance::MENU_SURFACE))
                    .shadow_lg()
                    .occlude()
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
                                    .child(icon(
                                        self.ninjas
                                            .hosts
                                            .iter()
                                            .find(|h| h.id == p.host)
                                            .map(host_glyph)
                                            .unwrap_or("machine-desktop"),
                                        18.,
                                        TEXT,
                                    ))
                                    .when(
                                        self.ninjas
                                            .hosts
                                            .iter()
                                            .any(|h| h.id == p.host && h.endpoint.is_some()),
                                        |v| v.child(icon("tailscale", 14., MUTED)),
                                    )
                                    .child(
                                        div()
                                            .text_size(theme::ui_px(12.))
                                            .child(host_name.to_owned()),
                                    ),
                            )
                            .child(marked_chip(
                                "ninja-settings",
                                "",
                                icon("settings", 16., TEXT),
                                p.settings,
                                cx,
                                |app, _, cx| {
                                    if let Some(p) = &mut app.ninjas.panel {
                                        p.settings = !p.settings;
                                    }
                                    cx.notify();
                                },
                            ))
                            .child(chip(
                                "close-personal-ninjas",
                                "×",
                                false,
                                cx,
                                |app, _, cx| {
                                    app.remember_ninja_draft(cx);app.ninjas.desktop=None;app.ninjas.panel = None;
                                    cx.notify();
                                },
                            )),
                    )
                    .when_some(
                        p.error
                            .clone()
                            .or_else(|| self.ninjas.errors.get(&p.host).cloned()),
                        |s, e| {
                            s.child(
                                div()
                                    .text_size(theme::ui_px(11.))
                                    .text_color(rgb(0xffb18a))
                                    .child(e),
                            )
                        },
                    )
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .flex_1()
                            .min_h(px(0.))
                            .when(p.settings, |body| {
                                body.child(
                                    div()
                                        .id("ninja-persona-fields")
                                        .debug_selector(|| "ninja-persona-fields".into())
                                        .w(px(if compact { width - 32. } else { (width - 48.) * 0.45 }))
                                        .flex_none()
                                        .min_w(px(0.))
                                        .p_3()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(rgba(0xffffff14))
                                        .bg(rgba(GLASS_SURFACE))
                                        .h_full()
                                        .overflow_y_scroll()
                                        .child(setup),
                                )
                            })
                            .when(!p.settings || !compact, |body| body.child(
                                work.id("ninja-work-card")
                                    .debug_selector(|| "ninja-work-card".into())
                                    .flex_1()
                                    .h_full()
                                    .p_3()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(rgba(0xffffff14))
                                    .bg(rgba(GLASS_SURFACE)),
                            )),
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
                app.ninjas.snapshots.insert("local".into(),json!({"providers":[{"agent":"Codex","installed":true},{"agent":"Claude","installed":true},{"agent":"Ninja","installed":true}]}));
                let panel=app.ninjas.panel.as_mut().unwrap();panel.profile.agent=AgentKind::Ninja;
                panel.profile.ninja=Some(ninja::Selection{model:"gpt-oss:20b".into(),harness:ninja::Harness::Claude,..Default::default()});
                panel.profile.model=Some("gpt-oss:20b".into());
                panel.models=vec![json!({"id":"gpt-oss:20b","name":"gpt-oss:20b","efforts":["low","medium","high"]}),json!({"id":"gpt-6.1-sol","name":"GPT-6.1 Sol","efforts":[]}),json!({"id":"qwen3-coder:30b","name":"Qwen 3 Coder","efforts":[]})];
                panel.fields[2].update(cx,|input,cx|input.set_text("/Users/example/Library/Application Support/a-very-long-workspace-name/another-folder",cx));
            })
        });
        crate::ui_tests::draw_frame(&view, cx);
        let card = cx.debug_bounds("personal-ninjas-panel").unwrap();
        let fields = cx.debug_bounds("ninja-persona-fields").unwrap();
        let workspace = cx.debug_bounds("ninja-workspace-field").unwrap();
        assert!(fields.left() >= card.left() && fields.right() <= card.right());
        assert!(workspace.left() >= fields.left() && workspace.right() <= fields.right());
        let work = cx.debug_bounds("ninja-work-card").unwrap();
        assert!(work.left() >= fields.right() && work.right() <= card.right());
        for name in [
            "ninja-model-gpt-oss:20b-label",
            "ninja-model-qwen3-coder:30b-label",
            "ninja-model-gpt-6.1-sol-label",
        ] {
            let label = cx.debug_bounds(name).unwrap();
            assert!(
                label.size.width > px(180.),
                "Model name collapsed to an ellipsis: {label:?}"
            );
            assert!(
                label.size.height <= px(20.),
                "Model wrapped inside fixed-height row: {label:?}"
            );
        }
    }
    #[gpui::test]
    fn remote_reviews_keep_exact_owner_token_and_native_questions(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx,|app,cx|{
            let native=runtime::NativeRequest{id:"question-42".into(),method:"question".into(),title:"Choose deployment".into(),detail:"Exact remote request".into(),kind:runtime::NativeRequestKind::Questions{questions:vec![runtime::NativeQuestion{id:"target".into(),header:"Target".into(),question:"Which environment?".into(),options:vec![],secret:false,multiple:false,required:true}]}};
            app.ninjas.snapshots.insert("local".into(),json!({"profiles":[{"id":"personal","name":"Ice","style":"ice"}],"tasks":[{"id":"task-7","profile":"personal","state":"approval","requests":[native]}]}));
            let snapshot=app.unified_snapshot(cx);assert_eq!(snapshot["menuBarNinjas"][0]["id"],"local:personal");assert_eq!(snapshot["menuBarNinjas"][0]["style"],"ice");assert_eq!(snapshot["approvalThreadId"],"ninja:local:task-7");assert_eq!(snapshot["nativeReview"]["token"],"question-42");assert_eq!(snapshot["nativeReview"]["kind"],"questions");assert_eq!(snapshot["nativeReview"]["questions"][0]["id"],"target");assert_eq!(snapshot["activeTasks"][0]["scene"],"permission");assert_eq!(snapshot["approvalCount"],1);app.ninjas.errors.insert("local".into(),"Disconnected".into());let offline=app.unified_snapshot(cx);assert!(offline["nativeReview"].is_null());assert!(offline["activeTasks"][0]["text"].as_str().unwrap().contains("Reconnecting"));
        });
    }
    #[gpui::test]
    fn saved_ninjas_open_as_conversations_and_intensity_is_model_scoped(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window,cx|view.update(cx,|app,cx|{
            app.open_ninjas(Some("local".into()),None,window,cx);
            let p=app.ninjas.panel.as_mut().unwrap();p.profile.revision=1;p.profile.name="Ice".into();p.settings=false;
            p.profile.model=Some("fixture".into());p.models=vec![json!({"id":"fixture","name":"Fixture","efforts":["low","medium","high"]})];
            p.chat=Some("chat-one".into());let id=p.profile.id.clone();
            app.ninjas.snapshots.insert("local".into(),json!({"profiles":[],"tasks":[{"id":"one","profile":id,"chat":"chat-one","prompt":"First chat","brief":"Done","state":"completed"},{"id":"two","profile":id,"chat":"chat-two","prompt":"Other chat","brief":"Other","state":"completed"}]}));
        }));
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("ninja-work-card").unwrap().size.width > px(750.));
        crate::ui_tests::click_selector(cx, "ninja-settings");
        crate::ui_tests::draw_frame(&view, cx);
        crate::ui_tests::click_selector(cx, "ninja-effort-high");
        view.update(cx, |app, _| {
            let p = app.ninjas.panel.as_ref().unwrap();
            assert_eq!(p.profile.effort.as_deref(), Some("high"));
            assert_eq!(
                p.effort_preview.as_ref().map(|(v, _)| v.as_str()),
                Some("high")
            );
            assert_eq!(p.profile.model.as_deref(), Some("fixture"));
        });
        crate::ui_tests::click_selector(cx, "ninja-new-chat");
        view.update(cx, |app, _| {
            let p = app.ninjas.panel.as_ref().unwrap();
            assert!(p.chat.is_none());
            assert!(p.messages.is_empty());
        });
    }
    #[test]
    fn screen_hosts_and_statuses_are_exact() {
        let mut h = Host {
            id: "studio".into(),
            name: "Studio".into(),
            endpoint: Some("http://100.72.31.42:32187".into()),
        };
        assert_eq!(host_address(&h), Some("100.72.31.42"));
        h.endpoint = Some("http://user@evil:32187".into());
        assert!(host_address(&h).is_none());
        for (state, status, scene) in [
            ("approval", "Waiting", "permission"),
            ("running", "Terminal command", "terminal"),
            ("completed", "Completed", "success"),
            ("failed", "Error", "failure"),
        ] {
            assert_eq!(task_scene(&json!({"state":state,"status":status})), scene);
        }
        let mut state = State::default();
        state.hosts.push(Host { id: "studio".into(), name: "Studio".into(), endpoint: Some("http://100.72.31.42:32187".into()) });
        state.snapshots.insert("studio".into(), json!({"profiles":[{"id":"n","name":"Ice"}],"tasks":[{"id":"t","profile":"n","state":"running","status":"Terminal command"}]}));
        assert_eq!(state.active()[0]["scene"], "terminal");
        state.snapshots.get_mut("studio").unwrap()["tasks"][0]["state"] = json!("approval");
        assert_eq!(state.active()[0]["scene"], "permission");
        assert!(svg_assets::asset("phone").is_some());
    }
}
