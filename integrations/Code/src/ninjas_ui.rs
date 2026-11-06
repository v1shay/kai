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
    pub expanded:HashSet<String>,
    pub fold_motions:HashMap<String,(Instant,f32)>,
    pub hosts: Vec<Host>,
    pub snapshots: HashMap<String, Value>,
    pub watching: HashSet<String>,
    pub panel: Option<Panel>,
    pub errors: HashMap<String, String>,
    pub reviews:HashMap<String,crate::native_request_ui::LiveRequest>,
    pub drafts: HashMap<String,String>,
    pub title_attempted:HashSet<String>,
    pub artifacts:HashMap<String,Result<crate::ninja_chat::Artifact,String>>,
    pub artifact_pending:HashSet<String>,
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
    pub(super) reference_pending:usize,
    reference_submit:bool,
    pub settings: bool,
    delete_confirm:bool,
    pub messages: Vec<Value>,
    pub history_loading: bool,
    pub history_scroll:ListState,
    pub history_follow:std::cell::Cell<bool>,
    pub history_tasks:std::cell::RefCell<std::sync::Arc<Vec<Value>>>,
    history_scroll_owner:std::cell::RefCell<Option<String>>,
    pub models_requested: bool,
    pub models_loading:Option<uuid::Uuid>,
    pub history_owner: Option<String>,
    pub effort_preview: Option<(String, Instant)>,
    pub decode: std::cell::RefCell<crate::text_decode::TextDecode>,
    pub control:Option<crate::ninja_chat::Control>,
    pub control_cursor:usize,
    pub dismissed_control:Option<(crate::ninja_chat::Control,Instant)>,
    pub controls_dirty:bool,
    pub control_at:Instant,
    pub blocks:std::cell::RefCell<HashMap<String,(String,std::sync::Arc<Vec<MessageBlock>>)>>,
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
            expanded:serde_json::from_value(value["expanded"].clone()).unwrap_or_default(),
            snapshots: serde_json::from_value::<HashMap<String,Value>>(value["snapshots"].clone()).unwrap_or_default().into_iter().map(|(id,mut v)|{
                // Cached task history is useful offline, cached liveness is not.
                v["tasks"]=Value::Array(v["tasks"].as_array().cloned().unwrap_or_default().into_iter().filter(|t|!crate::ninja_chat::live(t)).collect());(id,v)
            }).collect(),
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
        *pending.lock().unwrap() = Some(json!({"hosts":self.hosts,"snapshots":self.snapshots,"drafts":self.drafts,"expanded":self.expanded}));
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
    pub fn live_snapshot(&self,host:&str)->Option<&Value>{
        if self.errors.contains_key(host){None}else{self.snapshots.get(host)}
    }
    pub fn tasks(&self,host:&str)->Vec<std::borrow::Cow<'_,Value>>{
        let history=self.panel.as_ref().filter(|p|p.host==host).into_iter().flat_map(|p|p.messages.iter().chain(p.task.iter()));
        let cached=self.snapshots.get(host).into_iter().flat_map(|s|s["tasks"].as_array().into_iter().flatten());
        let current=self.live_snapshot(host).into_iter().flat_map(|s|s["tasks"].as_array().into_iter().flatten());
        crate::ninja_chat::reconcile_tasks(history.chain(cached),current)
    }
    pub fn active(&self) -> Vec<Value> {
        let mut out = vec![];
        for h in &self.hosts {
            let Some(v) = self.live_snapshot(&h.id) else {
                continue;
            };
            for t in
                self.tasks(&h.id).iter().filter(|t| {
                    crate::ninja_chat::live(t) && t["state"] != "queued"
                })
            {
                let p = v["profiles"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|p| p["id"] == t["profile"]);
                let name = p.and_then(|p| p["name"].as_str()).unwrap_or("Ninja");
                out.push(json!({"id":format!("ninja:{}:{}",h.id,t["id"].as_str().unwrap_or("")),"runId":t["id"],"scene":task_scene(t),"text":format!("{name} · {}",t["status"].as_str().unwrap_or("Working")),"name":name,"agent":"ninja","ninjaStyle":p.and_then(|p|p["style"].as_str()).unwrap_or("fire")}));
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
    pub fn pending_count(&self)->usize{
        self.hosts.iter().map(|h|self.tasks(&h.id).iter().filter(|t|crate::ninja_chat::live(t)).map(|t|t["requests"].as_array().map_or(0,Vec::len)).sum::<usize>()).sum()
    }
    pub fn pending(&self) -> Option<(Host, String, Value)> {
        self.hosts.iter().find_map(|h|self.tasks(&h.id).iter().filter(|t|crate::ninja_chat::live(t)).find_map(|t|{
            Some((h.clone(),t["id"].as_str()?.into(),t["requests"].as_array()?.first()?.clone()))
        }))
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
        #[cfg(test)]
        anyhow::bail!("UI fixtures must not start the user's persistent worker");
        #[cfg(not(test))]
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

pub(super) fn chip(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on: bool,
    cx: &Context<Orbit>,
    action: impl Fn(&mut Orbit, &mut Window, &mut Context<Orbit>) + 'static,
) -> Stateful<Div> {
    chip_content(id, label.into(), None, on, cx, action)
}
pub(super) fn marked_chip(
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
    let icon_only=label.is_empty();
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
        .bg(rgba(if icon_only {0} else if on { 0x9380ff35 } else { 0xffffff09 }))
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
fn tasks_usage(messages:&[Value],snapshot:Option<&Value>,p:&Panel)->Option<String>{
    messages.iter().chain(snapshot.into_iter().flat_map(|s|s["tasks"].as_array().into_iter().flatten())).filter(|t|t["chat"].as_str()==p.chat.as_deref()).filter_map(|t|crate::ninja_chat::usage_label(&t["usage"]).map(|s|(t["created"].as_i64().unwrap_or(0),s))).max_by_key(|(at,_)|*at).map(|(_,label)|label)
}
pub(super) fn task_scene(task: &Value) -> &'static str {
    match task["state"].as_str().unwrap_or("") {
        "approval" => "permission",
        "completed" => "success",
        "failed" | "interrupted" => "failure",
        "stopped" => "declined",
        "queued" => "waiting",
        "unknown" => "off",
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
        self.send_ninja_response(host,wire,Some(value.clone()),cx);
        Ok(true)
    }

    pub(super) fn send_ninja_response(&mut self,host:Host,wire:Value,issued:Option<Value>,cx:&mut Context<Self>){
        if let Some(issued)=&issued{self.defer_unified_request(issued);}
        let owner_host=host.id.clone();let owner_task=wire["id"].clone();let token=wire["request"].clone();
        cx.spawn(async move|this,cx|{
            let result=cx.background_executor().spawn(async move{call(&host,&wire)}).await;
            let _=this.update(cx,|app,cx|{
                if result.is_ok(){
                    if let Some(task)=app.ninjas.snapshots.get_mut(&owner_host).and_then(|s|s["tasks"].as_array_mut()).and_then(|ts|ts.iter_mut().find(|t|t["id"]==owner_task)){
                        if let Some(requests)=task["requests"].as_array_mut(){requests.retain(|r|r["id"]!=token);}
                        if task["state"]=="approval"&&task["requests"].as_array().is_some_and(Vec::is_empty){task["state"]=json!("running");task["status"]=json!("Working");}
                    }
                }
                for pending in app.ninjas.reviews.values_mut(){if pending.matches_ninja(&owner_host,owner_task.as_str().unwrap_or(""),token.as_str().unwrap_or("")){pending.retry();}}
                let error=result.err().map(|e|e.to_string());
                if let Some(issued)=issued{app.complete_unified_request(&issued,error,cx);}else{if let Some(error)=error{app.notice=Some(error);}app.publish_unified(cx);cx.notify();}
            });
        }).detach();
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
        let token=p.token;let tint=crate::ninja_art::color(&p.profile.style);
        let desktop=self.ninjas.desktop.clone().unwrap_or_else(||cx.new(|cx|crate::remote_desktop::RemoteDesktop::new(address,host.name.clone(),username,window,cx)));
        self.ninjas.desktop=Some(desktop.clone());
        if !take_over {cx.notify();return;}
        if !desktop.read(cx).connected(){desktop.update(cx,|v,cx|v.takeover_error("Connect to this Mac before taking control".into(),cx));cx.notify();return;}
        let epoch=desktop.update(cx,|v,cx|v.takeover_pending(tint,cx));
        self.ninjas.call.take();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { call(&host, &json!({"action":"take_over","id":id})) })
                .await;
            let _ = this.update(cx, |app, cx| {
                if app.ninjas.panel.as_ref().is_some_and(|p|p.token==token) && app.ninjas.desktop.as_ref()==Some(&desktop){desktop.update(cx,|view,cx|view.takeover_finished(epoch,result.map(|_|()).map_err(|e|e.to_string()),cx));}
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn ninja_reference_image(&mut self,bytes:std::sync::Arc<[u8]>,cx:&mut Context<Self>){
        let Some(p)=self.ninjas.panel.as_mut() else{return};p.reference_pending+=1;let token=p.token;
        cx.spawn(async move|this,cx|{let result=cx.background_executor().spawn(async move{attachments::import_clipboard(&bytes,&ninjas::root().join("references"),&std::sync::atomic::AtomicBool::new(false))}).await;
            let _=this.update(cx,|app,cx|{let Some(p)=app.ninjas.panel.as_mut().filter(|p|p.token==token) else{return};p.reference_pending=p.reference_pending.saturating_sub(1);match result{Ok(file)=>{file.mark_submitted();app.ninja_reference_paths(vec![file.path.clone()],cx)},Err(e)=>{p.reference_submit=false;app.notice=Some(e.to_string());}}cx.notify();});}).detach();
    }
    pub(super) fn ninja_reference_paths(&mut self,paths:Vec<PathBuf>,cx:&mut Context<Self>){
        let Some(p)=self.ninjas.panel.as_mut() else{return};p.reference_pending+=1;let token=p.token;
        let Some(host)=self.ninjas.hosts.iter().find(|h|h.id==p.host).cloned() else{return};
        cx.spawn(async move|this,cx|{
            let remote=host.clone();
            let result=cx.background_executor().spawn(async move{(||->anyhow::Result<Vec<String>>{
                anyhow::ensure!(!paths.is_empty()&&paths.len()<=8,"Send up to eight reference files at once");
                if remote.endpoint.is_none(){return Ok(paths.iter().map(|p|p.to_string_lossy().into_owned()).collect());}
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
                if !app.ninjas.panel.as_ref().is_some_and(|p|p.token==token){return;}if let Some(p)=app.ninjas.panel.as_mut(){p.reference_pending=p.reference_pending.saturating_sub(1);match result{Ok(names)=>{
                    let note=format!("\nReference files (remote files are in this host's Downloads; keep working in your configured workspace): {}",names.join(", "));
                    let draft=p.fields[3].read(cx).text();p.fields[3].update(cx,|input,cx|input.set_text(&format!("{draft}{note}"),cx));
                },Err(e)=>{p.reference_submit=false;app.notice=Some(e.to_string());}}}let send=app.ninjas.panel.as_mut().is_some_and(|p|{let send=p.reference_submit && p.reference_pending==0;if send{p.reference_submit=false;}send});if send{app.ninja_submit_task(cx);}cx.notify();
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
                                        && (previous.is_some_and(|old|old["tasks"].as_array().into_iter().flatten().any(|t|t["profile"]==p.profile.id&&t["chat"].as_str()==p.chat.as_deref()&&crate::ninja_chat::live(t)&&!value["tasks"].as_array().into_iter().flatten().any(|next|next["id"]==t["id"]))) || value["tasks"].as_array().into_iter().flatten().any(
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
                                        ))
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
                                    app.queue_ninja_titles(cx);
                                    app.ninja_prefetch_images(cx);
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
        let mut p=existing.unwrap_or_else(||Profile{id:uuid::Uuid::new_v4().to_string(),name:String::new(),persona:"A resourceful personal partner. Complete tasks, verify results, and remember useful long-term context.".into(),style:"fire".into(),project:if selected=="local"{self.execution_root().unwrap_or_else(||self.project.clone())}else{snapshot.and_then(|v|v["home"].as_str()).map(PathBuf::from).unwrap_or_default()},agent:self.agent,model:self.request_model(),effort:self.request_effort(),ninja:(self.agent==AgentKind::Ninja).then(||self.ninja_selection()),edits:self.allow_edits,access:self.codex_access,approvals:self.codex_approvals,revision:0,orchestrator:false});
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
                input.set_image_paste(fields.len()==3);
                input
            });
            let task_input = fields.len() == 3;
            subscriptions.push(cx.subscribe_in(&field, window, move |app, _, event, window, cx| {
                if task_input {app.ninja_input_event(event,window,cx);}
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
            reference_pending:0,
            reference_submit:false,
            settings,
            delete_confirm:false,
            messages: vec![],
            history_loading: false,
            history_scroll:ListState::new(0,ListAlignment::Bottom,px(200.)),
            history_follow:std::cell::Cell::new(true),history_tasks:Default::default(),
            history_scroll_owner:Default::default(),
            models_requested: false,
            models_loading:None,
            history_owner: None,
            effort_preview,
            decode:Default::default(),
            control:None,control_cursor:0,dismissed_control:None,controls_dirty:false,control_at:Instant::now(),blocks:Default::default(),
            at: Instant::now(),
            token: uuid::Uuid::new_v4(),
        });
        if let Some(p)=&self.ninjas.panel{let token=p.token;p.history_scroll.set_scroll_handler(cx.listener(move|app,event:&ListScrollEvent,_,_|{if let Some(p)=app.ninjas.panel.as_ref().filter(|p|p.token==token){p.history_follow.set(!event.is_scrolled);}}));}
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
        self.queue_ninja_titles(cx);
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
                                p.messages = v["messages"].as_array().cloned().unwrap_or_default();
                                let prefix=format!("{}:{}:",p.host,p.profile.id);app.ninjas.artifacts.retain(|key,result|!key.starts_with(&prefix)||result.is_ok());
                            }
                            Err(e) => p.error = Some(e.to_string()),
                        }
                    }
                }
                app.ninja_prefetch_images(cx);
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn ninja_operation(&mut self, request: Value, cx: &mut Context<Self>) {
        self.ninja_operation_reply(request,None,cx);
    }
    fn ninja_operation_reply(&mut self, request: Value, issued:Option<Value>, cx: &mut Context<Self>) {
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
        let submitted_prompt = (request["action"] == "submit").then(||panel.fields[3].read(cx).text());
        let submitted_chat=panel.chat.clone();
        let draft_key=format!("{}:{}:{}",panel.host,panel.profile.id,panel.chat.as_deref().unwrap_or("draft"));
        let captured_agent = panel.profile.agent;
        let captured_model = panel.profile.ninja.as_ref().map(|n| n.model.clone());
        let saved_profile=request.get("profile").cloned();
        let action = request["action"].as_str().unwrap_or("").to_owned();
        let model_owner=(action=="models").then(uuid::Uuid::new_v4);
        if let Some(owner)=model_owner{panel.models_loading=Some(owner);}else{panel.busy=true;}
        if let Some(issued)=&issued{self.defer_unified_request(issued);}
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { call(&host, &request) })
                .await;
            let _ = this.update(cx, |app, cx| {
                let reply_error=result.as_ref().err().map(ToString::to_string);
                if result.is_ok() && submitted_prompt.as_ref().is_some_and(|text|app.ninjas.drafts.get(&draft_key)==Some(text)){
                    app.ninjas.drafts.remove(&draft_key);app.ninjas.save();
                }
                let Some(panel) = app.ninjas.panel.as_mut().filter(|p| p.token == token) else {
                    if let Some(issued)=&issued{app.complete_unified_request(issued,reply_error,cx);}
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
                                    let mut current=json!(panel.profile);current["revision"]=Value::Null;
                                    let mut sent=saved_profile.clone().unwrap_or(Value::Null);sent["revision"]=Value::Null;
                                    panel.controls_dirty=current!=sent;
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
                            // Still settle the request and save pending settings
                            // below, but never navigate back after the user left.
                            "submit" if panel.chat!=submitted_chat => {},
                            "submit" => {
                                if let Some(revision) = value["profile"]["revision"].as_u64() {
                                    panel.profile.revision = revision;
                                }
                                panel.chat = value["chat"].as_str().map(str::to_owned);
                                panel.history_follow.set(true);panel.history_scroll.scroll_to(ListOffset{item_ix:panel.history_scroll.item_count(),offset_in_item:px(0.)});
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
                                let host=panel.host.clone();let id=panel.profile.id.clone();
                                app.ninja_forget_profile(&host,&id);
                            }
                            _ => {}
                        }
                    }
                    Err(e) => {
                        panel.error = Some(e.to_string());
                        panel.controls_dirty=false;
                    }
                }
                if let Some(issued)=&issued{app.complete_unified_request(issued,reply_error,cx);}
                if app.ninjas.panel.as_ref().is_some_and(|p|p.controls_dirty&&!p.busy) { app.ninja_persist_controls(cx); }
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
    fn ninja_forget_profile(&mut self,host:&str,id:&str){
        // Do not save the deleted draft again when dismissing its surface.
        self.ninjas.panel=None;self.ninjas.desktop=None;
        let prefix=format!("{host}:{id}:");
        self.ninjas.drafts.retain(|key,_|!key.starts_with(&prefix));
        if let Some(snapshot)=self.ninjas.snapshots.get_mut(host){
            if let Some(profiles)=snapshot["profiles"].as_array_mut(){profiles.retain(|p|p["id"]!=id);}
            if let Some(tasks)=snapshot["tasks"].as_array_mut(){tasks.retain(|t|t["profile"]!=id);}
        }
        self.ninjas.save();
    }
    pub(super) fn ninja_load_models(&mut self, cx: &mut Context<Self>) {
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
        self.ninja_submit_task_reply(None,cx);
    }
    pub(super) fn ninja_submit_task_reply(&mut self,issued:Option<Value>,cx:&mut Context<Self>){
        if let Some(p)=self.ninjas.panel.as_mut(){if p.reference_pending>0{p.reference_submit=true;cx.notify();return;}}
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
        let mut prompt = p.fields[3].read(cx).text();
        if p.profile.orchestrator {prompt=format!("Orchestrate this task using your available native tools and subagents. Delegate independent work, verify it, and give a concise completion brief.\n\n{prompt}");}
        if prompt.trim().is_empty() {
            return;
        }
        let mut profile = p.profile.clone();
        profile.name = p.fields[0].read(cx).text();
        profile.persona = p.fields[1].read(cx).text();
        profile.project = PathBuf::from(p.fields[2].read(cx).text());
        let request = json!({"action":"submit","id":profile.id,"profile":profile,"prompt":prompt,"chat":p.chat,"due":due,"repeat":repeat});
        self.ninja_operation_reply(request,issued,cx);
    }
    pub(super) fn ninja_chat_rows(&self)->Vec<Value>{
        let Some(p)=self.ninjas.panel.as_ref().filter(|p|p.profile.revision>0)else{return vec![]};self.ninja_chat_rows_for(&p.host,&p.profile.id)
    }
    pub(super) fn ninja_chat_rows_for(&self,host:&str,profile:&str)->Vec<Value>{
        let mut tasks=self.ninjas.tasks(host).into_iter().filter(|t|t["profile"]==profile).map(std::borrow::Cow::into_owned).collect::<Vec<_>>();
        tasks.sort_by_key(|t|std::cmp::Reverse(t["created"].as_i64().unwrap_or(0)));
        let mut seen=HashSet::new();tasks.retain(|t|t["chat"].is_string()&&seen.insert(t["chat"].clone().to_string()));tasks
    }
    fn ninja_fold_fraction(&self,key:&str)->f32{
        let target=if self.ninjas.expanded.contains(key){1.}else{0.};
        self.ninjas.fold_motions.get(key).map_or(target,|(at,from)|{let t=(at.elapsed().as_secs_f32()/0.24).min(1.);from+(target-from)*(1.-(1.-t).powi(3))})
    }
    fn toggle_ninja_fold(&mut self,host:&str,profile:&str,cx:&mut Context<Self>){
        let key=format!("{host}:{profile}");let from=self.ninja_fold_fraction(&key);
        if !self.ninjas.expanded.remove(&key){self.ninjas.expanded.insert(key.clone());}
        if self.motion&&!native::reduced_motion(){self.ninjas.fold_motions.insert(key,(Instant::now(),from));}else{self.ninjas.fold_motions.remove(&key);}
        self.ninjas.save();cx.notify();
    }
    fn select_ninja_sidebar(&mut self,host:String,profile:String,window:&mut Window,cx:&mut Context<Self>){
        let selected=self.ninjas.panel.as_ref().is_some_and(|p|p.host==host&&p.profile.id==profile);
        if selected{self.toggle_ninja_fold(&host,&profile,cx)}else{
            self.open_ninjas(Some(host.clone()),Some(profile.clone()),window,cx);
            if !self.ninjas.expanded.contains(&format!("{host}:{profile}")){self.toggle_ninja_fold(&host,&profile,cx)}
        }
    }
    pub(super) fn ninja_chat_key(host:&str,profile:&str,chat:&str)->String{format!("ninja-chat:{host}:{profile}:{chat}")}
    pub(super) fn ninja_chat_seed(&self,id:&str)->Option<String>{
        let raw=id.strip_prefix("ninja-chat:")?;let mut parts=raw.rsplitn(3,':');let chat=parts.next()?;let profile=parts.next()?;let host=parts.next()?;
        self.ninjas.snapshots.get(host)?["tasks"].as_array()?.iter().filter(|t|t["profile"]==profile && t["chat"]==chat).min_by_key(|t|t["created"].as_i64().unwrap_or(0)).and_then(|t|t["prompt"].as_str()).map(str::to_owned)
    }
    pub(super) fn ninja_chat_label(&self,host:&str,profile:&str,t:&Value)->String{
        let key=Self::ninja_chat_key(host,profile,t["chat"].as_str().unwrap_or(""));
        self.lifecycle.chat_names.get(&key).cloned().or_else(||self.ninja_chat_seed(&key)).unwrap_or_else(||t["prompt"].as_str().unwrap_or("New conversation").into())
    }
    pub(super) fn open_ninja_chat(&mut self,chat:String,cx:&mut Context<Self>){
        self.remember_ninja_draft(cx);
        if let Some(p)=&mut self.ninjas.panel{p.chat=Some(chat);p.task=None;p.messages.clear();let key=format!("{}:{}:{}",p.host,p.profile.id,p.chat.as_deref().unwrap());let draft=self.ninjas.drafts.get(&key).cloned().unwrap_or_default();p.fields[3].update(cx,|input,cx|input.set_text(&draft,cx));}
        self.ninja_conversation(cx);cx.notify();
    }
    pub(super) fn ninja_chat_sidebar(&self, inline: bool, cx: &Context<Self>) -> AnyElement {
        if self.ninjas.panel.is_none() { return div().into_any_element() }
        let tasks = self.ninja_chat_rows();
        let fraction = self.sidebar_section_fraction(project_tree::SidebarSection::Chats);
        let count = if fraction > 0. { tasks.len() } else { 0 };
        let body_height = tasks.len().min(6) as f32 * 32. * fraction;
        let rows = uniform_list("ninja-chat-list", count, cx.processor(move |app, range: std::ops::Range<usize>, _, cx| {
            let p = app.ninjas.panel.as_ref().unwrap();
            range.map(|i| {
                let t = &tasks[i];
                let chat = t["chat"].as_str().unwrap_or("").to_owned();
                let label = app.ninja_chat_label(&p.host, &p.profile.id, t);
                let selected = p.chat.as_deref() == Some(&chat);
                let active = crate::ninja_chat::live(t);
                let selector = format!("ninja-chat-{chat}");
                div().id(SharedString::from(selector.clone())).debug_selector(move || selector.clone())
                    .h(px(32.)).w_full().px_2().flex().items_center().gap_2().rounded_md()
                    .when(selected, |v| v.bg(rgba(0xffffff14)))
                    .hover(|v| v.bg(rgba(GLASS_HOVER))).cursor_pointer()
                    .child(div().flex_1().min_w(px(0.)).text_size(theme::ui_px(12.))
                        .child(app.animated_ninja_chat_title(&Self::ninja_chat_key(&p.host, &p.profile.id, &chat), label)))
                    .when(active, |v| v.child(crate::ninja_art::status(&p.profile.style, task_scene(t), app.motion && !native::reduced_motion())))
                    .on_click(cx.listener(move |app, _, _, cx| { cx.stop_propagation(); app.open_ninja_chat(chat.clone(), cx); }))
                    .into_any_element()
            }).collect()
        })).size_full().min_h(px(0.)).px_1();
        let body = div().debug_selector(|| "ninja-chat-viewport".into()).w_full().min_h(px(0.)).overflow_hidden().opacity(fraction)
            .when(inline, |v| v.h(px(body_height)).flex_none())
            .when(!inline, |v| v.flex_1().flex_basis(px(0.)).map(|mut v| { v.style().flex_grow = Some(fraction); v }))
            .child(rows);
        div().id("ninja-chat-sidebar").debug_selector(|| "ninja-chat-sidebar".into()).w_full().min_h(px(0.)).flex().flex_col()
            .when(inline, |v| v.flex_none())
            .when(!inline, |v| v.h_full())
            .child(div().id("ninja-chat-section-header").debug_selector(|| "ninja-chat-section-header".into())
                .h(px(if inline { 28. } else { 36. })).flex_none().px_2().flex().items_center().gap_2()
                .text_size(theme::ui_px(11.)).text_color(rgb(MUTED)).cursor_pointer()
                .on_click(cx.listener(|app, _, _, cx| { cx.stop_propagation(); app.toggle_sidebar_section(project_tree::SidebarSection::Chats, cx); }))
                .child(self.sidebar_section_chevron(project_tree::SidebarSection::Chats))
                .child("Chats").child(div().flex_1())
                .child(div().id("ninja-sidebar-new-chat").debug_selector(|| "ninja-sidebar-new-chat".into())
                    .size(px(24.)).flex_none().flex().items_center().justify_center().rounded_md().cursor_pointer()
                    .hover(|v| v.bg(rgba(GLASS_HOVER)).text_color(rgb(TEXT))).child(icon("plus", 14., MUTED))
                    .on_click(cx.listener(|app, _, _, cx| {
                        cx.stop_propagation();
                        if app.ninjas.panel.as_ref().is_some_and(|p| p.busy) { return }
                        app.remember_ninja_draft(cx);
                        if let Some(p) = &mut app.ninjas.panel { p.chat = None; p.task = None; p.messages.clear(); p.fields[3].update(cx, |i, cx| i.set_text("", cx)); }
                        cx.notify();
                    }))))
            .child(body)
            .when(!inline, |v| v.child(div().flex_basis(px(0.)).map(|mut v| { v.style().flex_grow = Some(1. - fraction); v })))
            .into_any_element()
    }
    pub(super) fn ninjas_sidebar(&self, cx: &Context<Self>) -> AnyElement {
        let profiles=self.ninjas.hosts.iter().flat_map(|h|self.ninjas.snapshots.get(&h.id).into_iter().flat_map(move|s|s["profiles"].as_array().into_iter().flatten().map(move|p|(h.id.clone(),p.clone())))).collect::<Vec<_>>();
        div().mx_2().mb_2().flex_none().flex().flex_col().gap_1()
            .child(div().px_2().flex().items_center().justify_between().child(div().flex().items_center().gap_2().text_size(theme::ui_px(11.)).text_color(rgb(MUTED)).child(icon("ninja",15.,TEXT)).child("Ninjas"))
                .child(marked_chip("new-personal-ninja","+",icon("ninja",14.,TEXT),false,cx,|app,w,cx|app.open_ninjas(None,None,w,cx))))
            .children(profiles.into_iter().map(|(host,p)|{
                let id=p["id"].as_str().unwrap_or("").to_owned();let name=p["name"].as_str().unwrap_or("Ninja").to_owned();let style=p["style"].as_str().unwrap_or("fire").to_owned();
                let key=format!("{host}:{id}");let fraction=self.ninja_fold_fraction(&key);let rows=self.ninja_chat_rows_for(&host,&id);
                let selected=self.ninjas.panel.as_ref().is_some_and(|v|v.host==host&&v.profile.id==id);
                let active=rows.iter().find(|t|crate::ninja_chat::live(t));
                let fold_host=host.clone();let fold_id=id.clone();let select_host=host.clone();let select_id=id.clone();let new_host=host.clone();let new_id=id.clone();
                let header=div().id(SharedString::from(format!("personal-ninja-{key}"))).debug_selector({let key=key.clone();move||format!("personal-ninja-{key}")})
                    .h(px(36.)).w_full().px_2().flex().items_center().gap_2().rounded_md().bg(rgba(if selected{0x9380ff22}else{0xffffff05})).hover(|v|v.bg(rgba(GLASS_HOVER))).cursor_pointer()
                    .child(div().id(SharedString::from(format!("ninja-fold-{key}"))).debug_selector({let key=key.clone();move||format!("ninja-fold-{key}")}).size(px(14.)).flex_none().flex().items_center().justify_center()
                        .child(icon("chevron-right",10.,MUTED).with_transformation(Transformation::rotate(radians(fraction*std::f32::consts::FRAC_PI_2))))
                        .on_click(cx.listener(move|app,_,_,cx|{cx.stop_propagation();app.toggle_ninja_fold(&fold_host,&fold_id,cx);})))
                    .child(avatar(&style,24.)).child(div().flex_1().min_w(px(0.)).child(crate::ninja_art::nameplate(name,&style,active.is_some(),self.motion&&!native::reduced_motion()&&cx.active_window().is_some())))
                    .when_some(active,|v,t|v.child(crate::ninja_art::status(&style,task_scene(t),self.motion&&!native::reduced_motion()&&cx.active_window().is_some())))
                    .child(div().id(SharedString::from(format!("ninja-new-{key}"))).debug_selector({let key=key.clone();move||format!("ninja-new-{key}")}).size(px(22.)).flex_none().flex().items_center().justify_center().child(icon("plus",12.,MUTED))
                        .on_click(cx.listener(move|app,_,w,cx|{cx.stop_propagation();if !app.ninjas.panel.as_ref().is_some_and(|p|p.host==new_host&&p.profile.id==new_id){app.open_ninjas(Some(new_host.clone()),Some(new_id.clone()),w,cx);}app.remember_ninja_draft(cx);if let Some(p)=&mut app.ninjas.panel{p.chat=None;p.task=None;p.messages.clear();p.fields[3].update(cx,|i,cx|i.set_text("",cx));}let key=format!("{new_host}:{new_id}");if !app.ninjas.expanded.contains(&key){app.toggle_ninja_fold(&new_host,&new_id,cx);}cx.notify();})))
                    .on_click(cx.listener(move|app,_,w,cx|{cx.stop_propagation();app.select_ninja_sidebar(select_host.clone(),select_id.clone(),w,cx);}));
                // Every identity owns its own children. No detached selected-chat
                // section after the other Ninjas, and no duplicated split list.
                let show=fraction>0. && !(self.sidebar_split&&self.conversations_visible&&selected);
                let count=if show{rows.len()}else{0};let height=count.min(6)as f32*32.*fraction;
                let children=div().id(SharedString::from(format!("ninja-children-{key}"))).debug_selector({let key=key.clone();move||format!("ninja-children-{key}")}).w_full().h(px(height)).flex_none().overflow_hidden().opacity(fraction)
                    .child(div().id(SharedString::from(format!("ninja-children-scroll-{key}"))).size_full().overflow_y_scroll().flex().flex_col().children(rows.into_iter().take(if show{usize::MAX}else{0}).map(|t|{
                        let chat=t["chat"].as_str().unwrap_or("").to_owned();let label=self.ninja_chat_label(&host,&id,&t);let active=crate::ninja_chat::live(&t);let chosen=selected&&self.ninjas.panel.as_ref().is_some_and(|p|p.chat.as_deref()==Some(&chat));let host=host.clone();let id=id.clone();
                        div().id(SharedString::from(format!("ninja-nested-{key}-{chat}"))).debug_selector({let key=key.clone();let chat=chat.clone();move||format!("ninja-nested-{key}-{chat}")}).h(px(32.)).w_full().flex_none().pl(px(36.)).pr_2().flex().items_center().gap_2().rounded_md().when(chosen,|v|v.bg(rgba(0xffffff12))).hover(|v|v.bg(rgba(GLASS_HOVER))).cursor_pointer()
                            .child(div().flex_1().min_w(px(0.)).text_size(theme::ui_px(12.)).truncate().child(self.animated_ninja_chat_title(&Self::ninja_chat_key(&host,&id,&chat),label)))
                            .when(active,|v|v.child(crate::ninja_art::status(&style,task_scene(&t),self.motion&&!native::reduced_motion())))
                            .on_click(cx.listener(move|app,_,w,cx|{cx.stop_propagation();if !app.ninjas.panel.as_ref().is_some_and(|p|p.host==host&&p.profile.id==id){app.open_ninjas(Some(host.clone()),Some(id.clone()),w,cx);}app.open_ninja_chat(chat.clone(),cx);}))
                    })));
                div().w_full().flex_none().flex().flex_col().child(header).child(children).into_any_element()
            })).into_any_element()
    }
    pub(super) fn ninjas_conversation(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        div().debug_selector(||"chat-pane".into()).flex_1().size_full().min_w(px(0.)).min_h(px(0.)).flex().flex_col().relative().overflow_hidden()
            .child(div().flex_1().min_h(px(0.)).child(self.ninjas_panel(window,cx))).into_any_element()
    }
    fn ninja_task_row(&self,t:&Value,window:&Window,cx:&Context<Self>)->AnyElement{
        let p=self.ninjas.panel.as_ref().unwrap();

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
                        let running = crate::ninja_chat::live(&t);
                        let mut row = div()
                            .id(SharedString::from(format!("ninja-task-{id}")))
                            .debug_selector({let id=id.clone();move||format!("ninja-task-{id}")})
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
                                row = row.child(self.ninja_bubble(line["role"] == "user",line["text"].as_str().unwrap_or("").to_owned(),&format!("{id}-{}",line["id"]),window,cx));
                            }
                        } else {
                            row = row
                                .child(self.ninja_bubble(true, prompt, &id,window,cx))
                                .child(self.ninja_bubble(false, brief, &id,window,cx));
                        }
                        if t["call"].is_string(){row = row.child(
                            div()
                                .pl(px(32.))
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(crate::ninja_art::status(&p.profile.style,task_scene(t),running && self.motion && !native::reduced_motion() && window.is_window_active()))
                                .child(
                                    div()
                                        .text_size(theme::ui_px(10.))
                                        .text_color(rgb(MUTED))
                                        .child(t["status"].as_str().unwrap_or("").to_owned()),
                                ),
                        );}
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

    }
    pub(super) fn ninjas_panel(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let compact = f32::from(window.viewport_size().width) < 640.;
        let Some(p) = &self.ninjas.panel else {
            return div().into_any_element();
        };
        if let Some(desktop)=&self.ninjas.desktop {
            return div().id("ninja-desktop-viewer").debug_selector(||"ninja-desktop-viewer".into()).relative().size_full().min_h(px(0.)).p_3().bg(rgba(GLASS_SURFACE)).occlude().flex().flex_col().gap_2().on_mouse_down(MouseButton::Left,|_,_,cx|cx.stop_propagation()).on_click(|_,_,cx|cx.stop_propagation())
                .child(div().flex().justify_end().items_center().gap_2()
                    .child(marked_chip("desktop-observe","",icon("eye",16.,TEXT),!desktop.read(cx).is_controlling(),cx,|app,_,cx|{if let Some(v)=&app.ninjas.desktop{v.update(cx,|v,cx|v.controlling(false,cx));}cx.notify();}))
                    .child(marked_chip("desktop-takeover","",icon("mouse-pointer",16.,TEXT),desktop.read(cx).is_controlling(),cx,|app,w,cx|app.ninja_screen(true,w,cx)))
                    .child(marked_chip("desktop-close","",icon("close",16.,TEXT),false,cx,|app,_,cx|{app.ninjas.desktop=None;cx.notify();})))
                .child(desktop.clone()).into_any_element();
        }
        if self.motion && window.is_window_active() && self.interaction_animations.values().any(|(at,_,_)|at.elapsed()<Duration::from_millis(340)){request_motion_frame(window,cx);}
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
        let setup=if p.settings||p.profile.revision==0{
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
            .when(p.profile.revision>0,|s|s.child(
                div().flex().items_center().justify_between().gap_2()
                    .child(div().text_size(theme::ui_px(11.)).text_color(rgb(MUTED)).child("Identity & personal memory"))
                    .child(marked_chip("ninja-delete","",icon("trash",16.,0xf38c82),p.delete_confirm,cx,|app,_,cx|{if let Some(p)=&mut app.ninjas.panel{p.delete_confirm=!p.delete_confirm;}cx.notify();}))))
            .when(p.delete_confirm,|s|s.child(
                div().debug_selector(||"ninja-delete-confirmation".into()).p_3().rounded_lg().bg(rgba(0xf38c8212)).flex().flex_col().gap_2()
                    .child(div().text_size(theme::ui_px(12.)).child("Delete this Ninja and its personal memory? Workspace files stay untouched. Stop any active or scheduled tasks first."))
                    .child(div().flex().justify_end().gap_2()
                        .child(chip("ninja-delete-cancel","Cancel",false,cx,|app,_,cx|{if let Some(p)=&mut app.ninjas.panel{p.delete_confirm=false;}cx.notify();}))
                        .child(marked_chip("ninja-delete-confirm",if p.busy{"Deleting…"}else{"Delete Ninja"},icon("trash",14.,0xf38c82),false,cx,|app,_,cx|{if let Some(p)=&app.ninjas.panel{if p.delete_confirm{let request=json!({"action":"delete_profile","id":p.profile.id});app.ninja_operation(request,cx);}}})))))
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
                            provider_icon(agent_icon(agent), 16., agent_color(agent)),
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
                                provider_icon(
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
        setup
        }else{div()};
        let tasks=crate::ninja_chat::merged_tasks(&p.messages,self.ninjas.live_snapshot(&p.host),&p.profile.id,p.chat.as_deref());
        let history_owner=format!("{}:{}:{}",p.host,p.profile.id,p.chat.as_deref().unwrap_or("draft"));
        let changed=p.history_scroll_owner.borrow().as_ref()!=Some(&history_owner);
        let mut cached=p.history_tasks.borrow_mut();
        if changed || cached.len()!=tasks.len() || cached.iter().zip(&tasks).any(|(a,b)|a!=b.as_ref()){
            let old=cached.len();
            if changed{p.history_scroll.reset(tasks.len());p.history_follow.set(true);}else{
                let first=cached.iter().zip(&tasks).position(|(a,b)|a!=b.as_ref()).unwrap_or(old.min(tasks.len()));
                p.history_scroll.splice(first..old,tasks.len()-first);
            }
            *cached=std::sync::Arc::new(tasks.into_iter().map(std::borrow::Cow::into_owned).collect());
            if p.history_follow.get(){p.history_scroll.scroll_to(ListOffset{item_ix:cached.len(),offset_in_item:px(0.)});}
        }
        let tasks=cached.clone();drop(cached);
        *p.history_scroll_owner.borrow_mut()=Some(history_owner.clone());
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
                    .debug_selector(|| "ninja-chat-header".into())
                    .w_full().min_w(px(0.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(theme::ui_px(15.))
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(crate::ninja_art::art(&p.profile.style,28.,self.motion && !native::reduced_motion() && window.is_window_active()))
                            .child(div().debug_selector(||"ninja-chat-title".into()).min_w(px(0.)).flex_1().truncate().child(if p.profile.name.is_empty() {
                                "New Ninja".to_owned()
                            } else {
                                p.profile.name.clone()
                            })),
                    )
                    .when(p.profile.revision > 0, |s| {
                        s.child(marked_chip(
                            "ninja-call",
                            "",
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
                                "",
                                icon("machine-desktop", 16., TEXT),
                                false,
                                cx,
                                |app, w, cx| app.ninja_screen(false, w, cx),
                            ))

                        },
                    )
                    .child(marked_chip(
                        "ninja-new-chat",
                        "",
                        icon("plus",16.,TEXT),
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
                    ))
                    .when(p.profile.revision>0,|s|s
                        .child(marked_chip("ninja-settings","",icon("settings",16.,TEXT),p.settings,cx,|app,_,cx|{if let Some(p)=&mut app.ninjas.panel{p.settings=!p.settings;}cx.notify();}))
                        .child(marked_chip("close-personal-ninjas","",icon("close",16.,TEXT),false,cx,|app,_,cx|{app.remember_ninja_draft(cx);app.ninjas.desktop=None;app.ninjas.panel=None;cx.notify();})))
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .id(SharedString::from(format!("ninja-task-history-{history_owner}")))
                    .debug_selector(||"ninja-task-history".into())
                    .min_w(px(0.))
                    .child(list(p.history_scroll.clone(),cx.processor(|app,index,window,cx|{
                        let task=app.ninjas.panel.as_ref().unwrap().history_tasks.borrow().get(index).cloned();
                        task.map(|t|app.ninja_task_row(&t,window,cx)).unwrap_or_else(||div().into_any_element())
                    })).size_full()),
            );
        for task in snapshot.and_then(|v|v["tasks"].as_array()).into_iter().flatten().filter(|t|t["profile"]==p.profile.id&&t["chat"].as_str()==p.chat.as_deref()&&crate::ninja_chat::live(t)){
            for request in task["requests"].as_array().into_iter().flatten(){
                let key=crate::native_request_ui::ninja_request_key(&p.host,task["id"].as_str().unwrap_or(""),request["id"].as_str().unwrap_or(""));
                if let Some(pending)=self.ninjas.reviews.get(&key){work=work.child(self.native_request_card(&pending.ninja_request(),true,cx));}
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
                    .child(crate::ninja_art::status(&p.profile.style,call.scene(),self.motion && !native::reduced_motion() && window.is_window_active()))
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
        let effort=p.profile.effort.as_deref().unwrap_or("auto");
        let color=composer_design::effort_color(effort,0.5);
        let usage=tasks_usage(&p.messages,snapshot,p);
        work=work.child(div().id("ninja-composer").debug_selector(||"ninja-composer".into()).relative().p_3().rounded_xl().border_1().border_color(rgba(0xffffff20)).flex().flex_col().gap_3()
            .drag_over::<ExternalPaths>(|s,_,_,_|s.border_color(rgb(ACCENT)))
            .on_drop(cx.listener(|app,paths:&ExternalPaths,_,cx|app.ninja_reference_paths(paths.paths().to_vec(),cx)))
            .child(div().min_h(px(60.)).child(p.fields[3].clone()))
            .child(div().flex().items_center().gap_1().flex_wrap()
                .child(self.ninja_animated_control("ninja-composer-workspace",String::new(),"workspace-folder-open",TEXT,false,Some(crate::ninja_chat::Control::Workspace),window,cx))
                .child(self.ninja_animated_control("ninja-composer-permissions",String::new(),if !p.profile.edits{"lock-keyhole"}else if p.profile.access==runtime::CodexAccess::Full{"lock-keyhole-open"}else{"shield"},if p.profile.access==runtime::CodexAccess::Full{0xf38c82}else{0xf2b66e},false,Some(crate::ninja_chat::Control::Permissions),window,cx))
                .child(self.ninja_animated_control("ninja-composer-model",if compact{String::new()}else{composer_design::display_label(p.profile.model.as_deref().unwrap_or(p.profile.agent.name()))},agent_icon(p.profile.agent),TEXT,false,Some(crate::ninja_chat::Control::Models),window,cx))
                .child(self.ninja_animated_control("ninja-composer-effort",composer_design::title(effort),"sparkles",color,true,Some(crate::ninja_chat::Control::Effort),window,cx))
                .child(self.ninja_animated_control("ninja-orchestrate",if compact{String::new()}else{if p.profile.orchestrator{"Orchestrator"}else{"Orchestrate"}.into()},"branch",0xc3a4ff,p.profile.orchestrator,None,window,cx))
                .child(div().flex_1())
                .child(marked_chip("ninja-submit-task","",icon("arrow-up",20.,TEXT),false,cx,|app,w,cx|app.ninja_input_event(&InputEvent::Submit,w,cx)))))
            .when_some(usage,|s,label|s.child(div().px_3().text_size(theme::ui_px(10.)).text_color(rgb(MUTED)).child(label)));
        let work=work.relative().child(self.ninja_control_view(window,cx));
        let inline=p.profile.revision>0;
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
            .relative().size_full().min_h(px(0.))
            .when(!inline,|s|s.absolute().inset_0().items_center().justify_center().bg(rgba(0x00000035)))
            .flex()
            .on_click(cx.listener(|app, _, _, cx| {
                app.remember_ninja_draft(cx);app.ninjas.desktop=None;app.ninjas.panel = None;
                cx.notify();
            }))
            .child(
                div()
                    .id("personal-ninjas-panel")
                    .debug_selector(|| "personal-ninjas-panel".into()).relative().overflow_hidden()
                    .when(inline,|s|s.size_full().min_h(px(0.)).relative().overflow_hidden())
                    .when(!inline,|s|s.w(px(width)).h(px(height)).rounded_xl().border_1().border_color(rgba(0xffffff20)).bg(rgba(appearance::MENU_SURFACE)).shadow_lg())
                    .p_4()
                    .occlude()
                    .opacity(progress)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .when(!inline, |s|s.child(
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
                                            .child(host_name.to_owned()).flex_1(),
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
                            .child(marked_chip(
                                "close-personal-ninjas",
                                "",
                                icon("close",16.,TEXT),
                                false,
                                cx,
                                |app, _, cx| {
                                    app.remember_ninja_draft(cx);app.ninjas.desktop=None;app.ninjas.panel = None;
                                    cx.notify();
                                },
                            )))
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
                                        .w(px(if compact { width - 32. } else if inline { 300. } else { (width - 48.) * 0.45 }))
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
                                    .when(!inline,|s|s.rounded_lg().border_1().border_color(rgba(0xffffff14)).bg(rgba(GLASS_SURFACE))),
                            )),
                    ),
            )
            .child(crate::ninja_art::wallpaper(&p.profile.style,self.motion && !native::reduced_motion() && window.is_window_active() && tasks.iter().any(crate::ninja_chat::live)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn late_submit_reply_cannot_navigate_or_erase_another_ninja_chat(cx:&mut TestAppContext){
        use std::io::{BufRead,Read,Write};
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint=format!("http://{}",listener.local_addr().unwrap());
        let server=std::thread::spawn(move||{
            let(mut stream,_)=listener.accept().unwrap();stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader=std::io::BufReader::new(stream.try_clone().unwrap());let mut length=0;
            loop{let mut line=String::new();reader.read_line(&mut line).unwrap();if line=="\r\n"{break}if let Some((key,value))=line.split_once(':'){if key.eq_ignore_ascii_case("content-length"){length=value.trim().parse::<usize>().unwrap();}}}
            let mut body=vec![0;length];reader.read_exact(&mut body).unwrap();let body:Value=serde_json::from_slice(&body).unwrap();assert_eq!(body["action"],"submit");
            let result=json!({"id":"task","chat":"old-chat","state":"queued","prompt":body["prompt"]}).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",result.len(),result).unwrap();
        });
        let(view,cx)=crate::ui_tests::fixture(cx);
        cx.update(|w,cx|view.update(cx,|app,cx|{
            app.open_ninjas(Some("fixture".into()),None,w,cx);
            app.ninjas.hosts.push(Host{id:"fixture".into(),name:"Fixture".into(),endpoint:Some(endpoint)});
            let p=app.ninjas.panel.as_mut().unwrap();p.profile.revision=1;p.chat=Some("old-chat".into());p.fields[3].update(cx,|i,cx|i.set_text("submitted draft",cx));
            app.remember_ninja_draft(cx);app.ninja_submit_task(cx);
            let p=app.ninjas.panel.as_mut().unwrap();p.chat=Some("new-chat".into());p.fields[3].update(cx,|i,cx|i.set_text("new unsent draft",cx));
        }));
        let deadline=Instant::now()+Duration::from_secs(8);
        loop{cx.run_until_parked();if !view.read_with(cx,|app,_|app.ninjas.panel.as_ref().unwrap().busy){break}assert!(Instant::now()<deadline,"Host response did not settle");std::thread::sleep(Duration::from_millis(10));}
        view.read_with(cx,|app,cx|{let p=app.ninjas.panel.as_ref().unwrap();assert_eq!(p.chat.as_deref(),Some("new-chat"));assert_eq!(p.fields[3].read(cx).text(),"new unsent draft");assert!(!app.ninjas.drafts.values().any(|v|v=="submitted draft"));});
        server.join().unwrap();
    }
    #[gpui::test]
    fn failed_ninja_submit_preserves_the_original_draft(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        cx.update(|w,cx|view.update(cx,|app,cx|{app.open_ninjas(Some("local".into()),None,w,cx);let p=app.ninjas.panel.as_mut().unwrap();p.profile.revision=1;p.profile.orchestrator=true;p.fields[3].update(cx,|i,cx|i.set_text("keep this on failure",cx));app.ninja_submit_task(cx);}));
        cx.run_until_parked();
        view.read_with(cx,|app,cx|{let p=app.ninjas.panel.as_ref().unwrap();assert!(!p.busy);assert!(p.error.is_some());assert_eq!(p.fields[3].read(cx).text(),"keep this on failure");});
    }
    #[gpui::test]
    fn long_ninja_titles_keep_corner_controls_inside_narrow_chat(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| view.update(cx, |app, cx| {
            app.sidebar_split = false;
            app.sidebar_layout_transition = None;
            app.sidebar_width = 280.;
            app.open_ninjas(Some("remote".into()), None, window, cx);
            let p = app.ninjas.panel.as_mut().unwrap();
            p.profile.revision = 1;
            p.profile.name = "a persistent engineering partner with a long name".into();
            p.settings = false;
            app.ninjas.hosts.push(Host { id: "remote".into(), name: "Studio".into(), endpoint: Some("http://127.0.0.1:1".into()) });
        }));
        for width in [620., 900., 1180.] {
            cx.simulate_resize(size(px(width), px(790.)));
            cx.run_until_parked();
            cx.draw(point(px(0.), px(0.)), size(px(width), px(790.)), |_, _| view.clone().into_any_element());
            let header = cx.debug_bounds("ninja-chat-header").unwrap();
            let close = cx.debug_bounds("close-personal-ninjas").unwrap();
            assert!(close.right() <= header.right() + px(1.), "Close escaped the header at {width}: {close:?}, {header:?}");
            assert!(close.right() <= px(width), "Close must remain visible");
        }
    }
    #[gpui::test]
    fn each_ninja_has_independent_foldable_chats_and_preserves_its_draft(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        cx.update(|w,cx|view.update(cx,|app,cx|{
            app.sidebar_split=false;app.sidebar_width=280.;app.motion=false;app.open_ninjas(Some("local".into()),None,w,cx);
            let p=app.ninjas.panel.as_mut().unwrap();p.profile.id="water".into();p.profile.revision=1;p.profile.name="nya".into();p.profile.style="water".into();p.settings=false;p.chat=Some("one".into());p.fields[3].update(cx,|i,cx|i.set_text("keep this draft",cx));
            app.ninjas.snapshots.insert("local".into(),json!({"profiles":[p.profile,{"id":"fire","name":"kai","style":"fire"}],"tasks":[{"id":"a","profile":"water","chat":"one","prompt":"Review rendering","state":"completed","created":2},{"id":"b","profile":"water","chat":"two","prompt":"Review setup","state":"completed","created":1},{"id":"c","profile":"fire","chat":"three","prompt":"Fix tests","state":"completed","created":3}]}));
            app.ninjas.expanded.extend(["local:water".into(),"local:fire".into()]);
        }));
        crate::ui_tests::draw_frame(&view,cx);
        let water=cx.debug_bounds("personal-ninja-local:water").unwrap();let fire=cx.debug_bounds("personal-ninja-local:fire").unwrap();
        let one=cx.debug_bounds("ninja-nested-local:water-one").unwrap();let two=cx.debug_bounds("ninja-nested-local:water-two").unwrap();let three=cx.debug_bounds("ninja-nested-local:fire-three").unwrap();
        assert_eq!(one.size.height,px(32.));assert!((two.top()-one.bottom()).abs()<px(1.));assert!(one.top()>=water.bottom()&&two.bottom()<=fire.top());assert!(three.top()>=fire.bottom());
        assert!(cx.debug_bounds("ninja-chat-sidebar").is_none(),"No detached chat list below all Ninjas");
        crate::ui_tests::click_selector(cx,"ninja-fold-local:water");crate::ui_tests::draw_frame(&view,cx);
        assert_eq!(cx.debug_bounds("ninja-children-local:water").unwrap().size.height,px(0.));assert!(cx.debug_bounds("ninja-children-local:fire").is_some());
        view.update(cx,|app,cx|assert_eq!(app.ninjas.panel.as_ref().unwrap().fields[3].read(cx).text(),"keep this draft"));
        crate::ui_tests::click_selector(cx,"ninja-new-local:water");crate::ui_tests::draw_frame(&view,cx);
        view.update(cx,|app,cx|{let p=app.ninjas.panel.as_ref().unwrap();assert_eq!(p.profile.id,"water");assert!(p.chat.is_none());assert!(p.fields[3].read(cx).text().is_empty());assert!(app.ninjas.drafts.values().any(|d|d=="keep this draft"));assert!(app.ninjas.expanded.contains("local:water"));});
    }
    #[gpui::test]
    fn ninja_sidebars_own_only_their_chats_names_and_drafts(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        cx.update(|w,cx|view.update(cx,|app,cx|{
            app.sidebar_split=true;app.conversations_visible=true;app.sidebar=true;
            app.open_ninjas(Some("local".into()),None,w,cx);
            let p=app.ninjas.panel.as_mut().unwrap();p.profile.id="ice-personal".into();p.profile.revision=1;p.profile.name="zane".into();p.profile.style="ice".into();p.settings=false;p.chat=Some("one".into());
            p.fields[3].update(cx,|i,cx|i.set_text("unsent first",cx));
            app.ninjas.drafts.insert("local:ice-personal:two".into(),"second draft".into());
            app.ninjas.snapshots.insert("local".into(),json!({"profiles":[{"id":"ice-personal","name":"zane","style":"ice"},{"id":"fire-personal","name":"kai","style":"fire"}],"tasks":[
                {"id":"a","profile":"ice-personal","chat":"one","prompt":"Fix the renderer alignment","state":"completed","created":1},
                {"id":"b","profile":"ice-personal","chat":"one","prompt":"Second turn must not rename chat","state":"completed","created":4},
                {"id":"c","profile":"ice-personal","chat":"two","prompt":"Review animation performance","state":"running","status":"Terminal","created":3},
                {"id":"other","profile":"fire-personal","chat":"unrelated","prompt":"Wrong Ninja","state":"completed","created":8}]}));
            assert_eq!(app.ninja_chat_rows().len(),2);
            assert_eq!(app.ninja_chat_rows()[0]["chat"],"one");
            assert_eq!(app.ninja_chat_label("local","ice-personal",&app.ninja_chat_rows()[0]),"Fix the renderer alignment");
            assert!(app.apply_chat_title("ninja-chat:local:ice-personal:one","Fix the renderer alignment","Renderer alignment repair",cx));
            let snapshot=app.unified_snapshot(cx);assert_eq!(snapshot["threadName"],"Renderer alignment repair");
            assert!(snapshot["ninjaGroups"].as_array().unwrap().iter().filter(|g|g["name"]=="zane").flat_map(|g|g["chats"].as_array().unwrap()).any(|c|c["name"]=="Renderer alignment repair"),"Native surfaces must use generated Ninja chat names");
        }));
        crate::ui_tests::draw_frame(&view,cx);
        let pane=cx.debug_bounds("workspace-pane").unwrap();let row=cx.debug_bounds("ninja-chat-two").unwrap();assert!(row.left()>=pane.left() && row.right()<=pane.right());
        view.update(cx,|app,_|assert!(app.ninja_chat_rows().iter().all(|t|t["profile"]=="ice-personal")));assert!(cx.debug_bounds("ninja-conversations").is_none(),"No duplicate in-conversation chat strip");assert!(cx.debug_bounds("ninja-chat-unrelated").is_none());
        crate::ui_tests::click_selector(cx,"ninja-chat-two");
        view.update(cx,|app,cx|{let p=app.ninjas.panel.as_ref().unwrap();assert_eq!(p.chat.as_deref(),Some("two"));assert_eq!(p.fields[3].read(cx).text(),"second draft");assert_eq!(app.ninjas.drafts["local:ice-personal:one"],"unsent first");app.sidebar_split=false;app.sidebar_layout_transition=None;});
        view.update(cx,|app,cx|{app.ninjas.expanded.insert("local:ice-personal".into());cx.notify();});crate::ui_tests::draw_frame(&view,cx);let nested=cx.debug_bounds("ninja-nested-local:ice-personal-two").unwrap();assert!(nested.right()<=px(300.),"Ninja children must move into the primary sidebar when split mode closes");
        view.update(cx,|app,cx|{app.open_ninja_chat("one".into(),cx);assert_eq!(app.ninjas.panel.as_ref().unwrap().fields[3].read(cx).text(),"unsent first");app.ninjas.panel=None;app.sidebar_split=true;});
        crate::ui_tests::draw_frame(&view,cx);assert!(cx.debug_bounds("session-0").is_some(),"Project sessions return outside Ninja");
    }
    #[gpui::test]
    fn closing_code_tab_preserves_the_independent_ninja_and_compact_composer(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        cx.update(|w,cx|view.update(cx,|app,cx|{app.open_ninjas(Some("local".into()),None,w,cx);app.ninjas.panel.as_mut().unwrap().profile.revision=1;let owner=app.ninja_surface_owner();let active=app.active.clone().unwrap_or_else(||"old-code-tab".into());app.active=Some(active.clone());app.open_tabs=vec![active.clone()];app.close_tab(&active,w,cx);assert_eq!(app.ninja_surface_owner(),owner);assert!(app.open_tabs.is_empty());assert!(app.active.is_none(),"Closed Code chat must not reappear beneath the Ninja");}));
        crate::ui_tests::draw_frame(&view,cx);
        assert!(cx.debug_bounds("ninja-composer").is_some());
        assert!(cx.debug_bounds("ninja-submit-task").is_some());
        assert!(cx.debug_bounds("ninja-schedule-0").is_none());
        assert!(cx.debug_bounds("ninja-send-reference").is_none());
    }
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
            let snapshot=app.unified_snapshot(cx);assert_eq!(snapshot["menuBarNinjas"][0]["id"],"local:personal");assert_eq!(snapshot["menuBarNinjas"][0]["style"],"ice");assert_eq!(snapshot["approvalThreadId"],"ninja:local:task-7");assert_eq!(snapshot["nativeReview"]["token"],"question-42");assert_eq!(snapshot["nativeReview"]["kind"],"questions");assert_eq!(snapshot["nativeReview"]["questions"][0]["id"],"target");assert_eq!(snapshot["activeTasks"][0]["scene"],"permission");assert_eq!(snapshot["approvalCount"],1);app.ninjas.errors.insert("local".into(),"Disconnected".into());let offline=app.unified_snapshot(cx);assert!(offline["nativeReview"].is_null());assert!(offline["activeTasks"].as_array().unwrap().is_empty());
        });
    }
    #[gpui::test]
    fn ninja_history_opens_at_bottom_but_preserves_reading_earlier_messages(cx:&mut TestAppContext){
        let (view,cx)=crate::ui_tests::fixture(cx);
        cx.update(|w,cx|view.update(cx,|app,cx|{
            app.open_ninjas(Some("local".into()),None,w,cx);let p=app.ninjas.panel.as_mut().unwrap();p.profile.revision=1;p.settings=false;p.chat=Some("long".into());
            p.messages=(0..24).map(|i|json!({"id":format!("long-{i}"),"chat":"long","profile":p.profile.id,"created":i,"prompt":"User request. ".repeat(15),"brief":"The completed response. ".repeat(24),"state":"completed","finished":i+1})).collect();
        }));
        crate::ui_tests::draw_frame(&view,cx);crate::ui_tests::draw_frame(&view,cx);
        view.update(cx,|app,_|{let p=app.ninjas.panel.as_ref().unwrap();assert_eq!(p.history_scroll.item_count(),24);assert!(p.history_scroll.logical_scroll_top().item_ix>0);p.history_follow.set(false);p.history_scroll.scroll_to(ListOffset{item_ix:0,offset_in_item:px(0.)});});
        crate::ui_tests::draw_frame(&view,cx);
        view.update(cx,|app,_|{let p=app.ninjas.panel.as_mut().unwrap();assert_eq!(p.history_scroll.logical_scroll_top().item_ix,0);p.messages.last_mut().unwrap()["brief"]=json!("More streaming text. ".repeat(80));});
        crate::ui_tests::draw_frame(&view,cx);
        view.update(cx,|app,_|{let p=app.ninjas.panel.as_mut().unwrap();assert_eq!(p.history_scroll.logical_scroll_top().item_ix,0,"Streaming must not yank a reader back down");p.chat=Some("short".into());p.messages=vec![json!({"id":"short-last","profile":p.profile.id,"chat":"short","created":1,"prompt":"Quick question","brief":"Quick reply","state":"completed","finished":2})];});
        crate::ui_tests::draw_frame(&view,cx);crate::ui_tests::draw_frame(&view,cx);
        let viewport=cx.debug_bounds("ninja-task-history").unwrap();let row=cx.debug_bounds("ninja-task-short-last").unwrap();
        assert!((viewport.bottom()-row.bottom()).abs()<px(3.),"Short history must sit above the composer: {row:?} in {viewport:?}");
        view.update(cx,|app,_|{let p=app.ninjas.panel.as_ref().unwrap();assert_eq!(p.history_scroll.item_count(),1);assert!(p.history_follow.get());});
    }
    #[gpui::test]
    fn ninja_deletion_is_explicit_cancelable_and_cleans_only_its_own_surface(cx:&mut TestAppContext){
        let (view,cx)=crate::ui_tests::fixture(cx);
        let id=cx.update(|w,cx|view.update(cx,|app,cx|{
            app.open_ninjas(Some("local".into()),None,w,cx);let p=app.ninjas.panel.as_mut().unwrap();p.profile.revision=1;p.settings=true;let id=p.profile.id.clone();
            app.ninjas.drafts.insert(format!("local:{id}:chat"),"draft".into());app.ninjas.drafts.insert("local:other:chat".into(),"keep".into());
            app.ninjas.snapshots.insert("local".into(),json!({"profiles":[{"id":id},{"id":"other"}],"tasks":[{"profile":id,"state":"completed"},{"profile":"other","state":"completed"}]}));id
        }));
        crate::ui_tests::draw_frame(&view,cx);assert!(cx.debug_bounds("ninja-delete-confirmation").is_none());
        crate::ui_tests::click_selector(cx,"ninja-delete");crate::ui_tests::draw_frame(&view,cx);assert!(cx.debug_bounds("ninja-delete-confirmation").is_some());
        crate::ui_tests::click_selector(cx,"ninja-delete-cancel");crate::ui_tests::draw_frame(&view,cx);view.update(cx,|app,_|assert!(!app.ninjas.panel.as_ref().unwrap().delete_confirm,"Cancel must dismiss confirmation"));
        view.update(cx,|app,_|{assert!(app.ninjas.panel.is_some());app.ninja_forget_profile("local",&id);assert!(app.ninjas.panel.is_none());assert_eq!(app.ninjas.snapshots["local"]["profiles"],json!([{"id":"other"}]));assert_eq!(app.ninjas.snapshots["local"]["tasks"],json!([{"profile":"other","state":"completed"}]));assert_eq!(app.ninjas.drafts.len(),1);});
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
        let pane=cx.debug_bounds("chat-pane").unwrap();let chat=cx.debug_bounds("ninja-work-card").unwrap();
        assert!(chat.origin.x>=pane.origin.x && chat.size.width>=pane.size.width-px(64.),"Ninja chat must fill its normal chat column: {chat:?} in {pane:?}");
        assert!(cx.debug_bounds("ninja-wallpaper").is_some());
        crate::ui_tests::click_selector(cx, "ninja-settings");
        crate::ui_tests::draw_frame(&view, cx);
        let scroll=cx.debug_bounds("ninja-persona-fields").unwrap();
        let effort=cx.debug_bounds("ninja-effort-high").unwrap();
        if effort.bottom()>scroll.bottom(){cx.simulate_event(ScrollWheelEvent{position:scroll.center(),delta:ScrollDelta::Pixels(point(px(0.),-(effort.bottom()-scroll.bottom()+px(80.)))),modifiers:Modifiers::none(),touch_phase:TouchPhase::Moved});crate::ui_tests::draw_frame(&view,cx);}
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
