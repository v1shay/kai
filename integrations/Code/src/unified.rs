//! In-process adapter: GPUI owns sessions, consent, persistence and every run.
//! Swift owns the notch's existing AppKit views. Commands wake an async receiver;
//! state is pushed only on owner notifications, with no polling or second server.
use super::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, ffi::c_void, sync::OnceLock};

type Publish = unsafe extern "C" fn(*const u8, usize);
type Collapse = unsafe extern "C" fn();
type Start = unsafe extern "C" fn(*mut c_void, extern "C" fn(*const u8, usize) -> bool);
static COMMANDS: OnceLock<async_channel::Sender<Value>> = OnceLock::new();
thread_local! {static HOST:RefCell<Option<Host>>=const{RefCell::new(None)};}
struct Host {
    _library: *mut c_void,
    publish: Publish,
    levels: unsafe extern "C" fn(*const f32, usize),
    voice_host:unsafe extern "C" fn()->*mut c_void,
    collapse: Collapse,
    _toggle: Collapse,
    last: Vec<u8>,
    history:Value,
    history_owner:String,
    completed: Option<String>,
    error: Option<String>,
    results: std::collections::VecDeque<(String, [u8; 32], Option<String>)>,
    reported_successes: std::collections::VecDeque<(String,String)>,
    finished: Vec<Value>,
    pet_rect:Option<[f32;4]>,
    workspace_motion:workspace_motion::Transition,
}

extern "C" fn command(bytes: *const u8, count: usize) -> bool {
    if bytes.is_null() || count == 0 || count > 32 * 1024 * 1024 {
        return false;
    }
    // Swift lends these bytes for this call only; decode/copy before returning.
    let data = unsafe { std::slice::from_raw_parts(bytes, count) };
    let Ok(request) = serde_json::from_slice::<Value>(data) else {
        return false;
    };
    if !request.is_object() || count > 128 * 1024 && request["action"] != "workspace_voice_prompt" {
        return false;
    }
    COMMANDS
        .get()
        .is_some_and(|queue| queue.try_send(request).is_ok())
}
pub(super) fn active() -> bool {
    HOST.with(|host| host.borrow().is_some())
}
pub(super) fn collapse() {
    HOST.with(|host| {
        if let Some(host) = host.borrow().as_ref() {
            unsafe { (host.collapse)() }
        }
    })
}
pub(super) fn reveal() {
    emit(&json!({"type":"workspaceReveal"}));
}
fn emit(value: &Value) {
    let Ok(data) = serde_json::to_vec(value) else {
        return;
    };
    if data.len() > 2 * 1024 * 1024 {
        return;
    }
    HOST.with(|host| {
        if let Some(host) = host.borrow().as_ref() {
            unsafe { (host.publish)(data.as_ptr(), data.len()) }
        }
    });
}

pub(super) fn attach(window: WindowHandle<Orbit>, cx: &mut App) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use objc2::{msg_send, runtime::AnyObject};
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let path = std::env::current_exe()?
            .parent()
            .unwrap()
            .join("../Resources/libKaiNotch.dylib");
        // An unbundled Code test/development run retains the normal Code UI.
        if !path.exists() {
            return Ok(());
        }
        let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
        let library = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        anyhow::ensure!(!library.is_null(), "Could not load the native Kai library");
        let start = unsafe { libc::dlsym(library, c"kai_unified_start".as_ptr()) };
        let publish = unsafe { libc::dlsym(library, c"kai_unified_publish".as_ptr()) };
        let collapse = unsafe { libc::dlsym(library, c"kai_unified_collapse".as_ptr()) };
        let toggle = unsafe { libc::dlsym(library, c"kai_unified_toggle".as_ptr()) };
        let ready = unsafe { libc::dlsym(library, c"kai_unified_ready".as_ptr()) };
        let levels = unsafe { libc::dlsym(library, c"kai_unified_levels".as_ptr()) };
        let voice_host=unsafe{libc::dlsym(library,c"kai_unified_voice_host".as_ptr())};
        anyhow::ensure!(
            !start.is_null()
                && !publish.is_null()
                && !collapse.is_null()
                && !toggle.is_null()
                && !ready.is_null()
                && !levels.is_null() && !voice_host.is_null(),
            "Incompatible Kai library"
        );
        let levels: unsafe extern "C" fn(*const f32, usize) =
            unsafe { std::mem::transmute(levels) };
        let voice_host:unsafe extern "C" fn()->*mut c_void=unsafe{std::mem::transmute(voice_host)};
        let ready: unsafe extern "C" fn() -> bool = unsafe { std::mem::transmute(ready) };
        let start: Start = unsafe { std::mem::transmute(start) };
        let publish: Publish = unsafe { std::mem::transmute(publish) };
        let toggle: Collapse = unsafe { std::mem::transmute(toggle) };
        let collapse: Collapse = unsafe { std::mem::transmute(collapse) };
        let (sender, receiver) = async_channel::bounded(32);
        anyhow::ensure!(COMMANDS.set(sender).is_ok(), "Kai is already attached");
        HOST.with(|host| {
            *host.borrow_mut() = Some(Host {
                _library: library,
                publish,
                levels,voice_host,
                collapse,
                _toggle:toggle,
                last: vec![],
                history:Value::Null,
                history_owner:String::new(),
                completed: None,
                error: None,
                results: Default::default(),
                reported_successes: Default::default(),
                finished: vec![],
                pet_rect:None,
                workspace_motion:Default::default(),
            })
        });
        let entity = window.update(cx, |_, window, cx| {
            let RawWindowHandle::AppKit(handle) =
                HasWindowHandle::window_handle(window).unwrap().as_raw()
            else {
                unreachable!()
            };
            let native: *mut AnyObject =
                unsafe { msg_send![handle.ns_view.as_ptr().cast::<AnyObject>(), window] };
            unsafe { start(native.cast(), command) };
            cx.entity()
        })?;
        // Entity notifications include background run, approval and transcript events.
        cx.observe(&entity, move |entity, cx| {
            entity.update(cx,|app,cx|{app.refresh_unified_models(cx);app.publish_unified(cx)});
        })
        .detach();
        entity.read(cx).publish_unified(cx);
        anyhow::ensure!(
            unsafe { ready() },
            "Kai did not accept the initial owner state"
        );
        eprintln!(
            "Kai unified: native layer and owner state ready in process {}",
            std::process::id()
        );
        if std::env::args().any(|arg|arg=="--unified-voice-probe"){
            anyhow::ensure!(std::env::args().any(|arg|arg=="--unified-smoke-test") && std::env::var("KAI_CODE_VOICE_DIAGNOSTIC").as_deref()==Ok("1"),"Live voice probe requires isolated, generated-audio diagnostic mode");
            start_voice_probe(window,cx)?;
        }else if std::env::args().any(|arg| arg == "--unified-smoke-test") {
            let diagnostics=unsafe{libc::dlsym(library,c"kai_unified_diagnostics".as_ptr())};
            anyhow::ensure!(!diagnostics.is_null(),"Missing presentation diagnostics");
            let diagnostics:unsafe extern "C" fn()->u32=unsafe{std::mem::transmute(diagnostics)};
            anyhow::ensure!(unsafe{diagnostics()} & 3 == 2,"App did not launch as notch only");
            reveal();
            cx.spawn(async move |cx| {
                Timer::after(Duration::from_millis(120)).await;
                let _=cx.update(|_|crate::unified::collapse());
                Timer::after(Duration::from_millis(80)).await;
                let _=cx.update(|_|crate::unified::reveal());
                Timer::after(Duration::from_millis(1100)).await;
                let _=cx.update(|_|{
                    let state=unsafe{diagnostics()};
                    assert_eq!(state & 31,9,"Only the live workspace and shared pet may be visible");
                    assert_ne!(state & 64,0,"Workspace must fill the menu-bar/Dock working area");
                    crate::unified::reveal();assert_eq!(unsafe{diagnostics()} & 31,9);
                    crate::unified::collapse();
                });
                Timer::after(Duration::from_millis(850)).await;
                let _=cx.update(|_|{
                    assert_eq!(unsafe{diagnostics()} & 27,2,"Fold must restore the extended notch and pet");
                    crate::unified::reveal();
                });
                Timer::after(Duration::from_millis(1000)).await;
                let _=cx.update(|_|emit(&json!({"type":"probeDoubleCommand"})));
                Timer::after(Duration::from_millis(950)).await;
                let _=cx.update(|cx|{
                    assert_eq!(unsafe{diagnostics()} & 3,0,"Double Command must dismiss the large workspace without reopening mini chat");
                    cx.update_global::<quick_chat::Controller,_>(|service,cx|service.toggle(cx).expect("Open native Quick Chat"));
                });
                Timer::after(Duration::from_millis(250)).await;
                let _=cx.update(|cx|{
                    let state=unsafe{diagnostics()};assert_ne!(state & 128,0,"Quick Chat was not opened");
                    assert_eq!(state & 3,0,"Quick Chat must not also open the notch or workspace");
                    cx.update_global::<quick_chat::Controller,_>(|service,cx|service.dismiss(cx));
                });
                Timer::after(Duration::from_millis(200)).await;
                let _=cx.update(|cx|{
                    assert_eq!(unsafe{diagnostics()} & 131,0,"Quick Chat dismissal must retire only its own surface");
                    eprintln!("Kai unified smoke: continuous morph reversal, maximized working area, shared pet, repeat open, extended-notch return, double-Command dismissal, and exclusive native Quick Chat passed");cx.quit();
                });
            }).detach();
        }
        cx.spawn(async move |cx| {
            while let Ok(request) = receiver.recv().await {
                if cx
                    .update(|cx| {
                        if request["action"] == "quick_dismiss"{
                            if cx.has_global::<quick_chat::Controller>(){cx.update_global::<quick_chat::Controller,_>(|service,cx|service.dismiss(cx));}
                            return Ok(())
                        }
                        window.update(cx, |this, window, cx| {
                            this.dispatch_unified(request, window, cx);
                        })
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
    Ok(())
}

fn native_id(value: &Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .and_then(|id| id.strip_prefix("code:"))
        .map(str::to_owned)
}
fn bounded(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}
const HISTORY_BYTES:usize=512*1024;
fn budget_history(mut rows:Vec<Value>)->Vec<Value>{
    let mut bytes=2;let mut kept=0;
    for row in rows.iter().rev(){let size=serde_json::to_vec(row).map_or(0,|v|v.len()+1);if bytes+size>HISTORY_BYTES{break}bytes+=size;kept+=1;}
    rows.drain(..rows.len().saturating_sub(kept));rows
}
pub(super) fn child_id(child:&runtime::broker::ChildSnapshot)->String{format!("child:{}:{}:{}",child.provider.command(),child.project.display(),child.id)}
fn scene(status: &str) -> &'static str {
    let value = status.to_lowercase();
    if value.contains("error") || value.contains("fail") {
        "failure"
    } else if value.contains("permission") || value.contains("approval") {
        "permission"
    } else if value.contains("search") {
        "search"
    } else if value.contains("file") || value.contains("edit") {
        "files"
    } else if value.contains("command") || value.contains("terminal") {
        "terminal"
    } else if value.contains("tool") {
        "tool"
    } else {
        "thinking"
    }
}
impl Orbit {
    fn effort_key(&self)->String{format!("{}:{}",self.agent.command(),self.model.as_deref().unwrap_or(""))}
    pub(super) fn unified_effort(&self)->Option<&str>{self.reasoning_efforts.get(&self.effort_key()).map(String::as_str)}
    pub(super) fn unified_token(&self) -> String {
        // Composer edits have their own exact draft-base check. They must not
        // invalidate navigation/model commands before the parent observes input.
        format!("{}:{}:{}:{}",self.active.clone().unwrap_or_else(||format!("draft-{}",self.generation)),self.project.display(),self.generation,self.agent.command())
    }

    pub(super) fn unified_send_ready(&self) -> bool {
        !self.loading
            && !self.loading_earlier
            && !self.starting
            && !self.quit_pending
            && self.active.as_ref().is_none_or(|id| {
                !self.runs.contains_key(id)
                    && !self.finalizing.contains(id)
                    && !self.settling.contains_key(id)
            })
            && self.history_offset == 0
            && self.attachment_pending == 0
            && self.handoff_pending.is_none()
            && self.workspace_trust.is_none()
            && self.session_conflict.is_none()
    }
    fn dispatch_unified(&mut self, request: Value, window: &mut Window, cx: &mut Context<Self>) {
        if request["action"] == "workspace_presentation"{
            HOST.with(|host|{if let Some(host)=host.borrow_mut().as_mut(){host.workspace_motion.set_at(request["phase"].as_str().unwrap_or("hidden"),request["started"].as_f64().filter(|t|t.is_finite()&&*t>0.).unwrap_or_else(workspace_motion::now))}});
            if matches!(request["phase"].as_str(),Some("expand"|"reopen")){
                self.project_reveal=self.motion.then(||ListReveal::new(Some(0)));
                self.sessions_reveal=self.motion.then(||ListReveal::new(Some(0)));
                self.transcript_reveal=self.motion.then(||ListReveal::new(None));
                self.unified_text_decode=Default::default();
            }
            cx.notify();return
        }
        let request_id = request["requestId"].as_str().unwrap_or("").to_owned();
        let fingerprint: [u8; 32] =
            Sha256::digest(serde_json::to_vec(&request).unwrap_or_default()).into();
        let cached = HOST.with(|host| {
            host.borrow().as_ref().and_then(|host| {
                host.results
                    .iter()
                    .find(|(id, _, _)| id == &request_id)
                    .cloned()
            })
        });
        let error = if let Some((_, body, error)) = cached {
            if body == fingerprint {
                error
            } else {
                Some("This request ID was already used for another action".into())
            }
        } else {
            self.unified_command(&request, window, cx).err()
        };
        HOST.with(|host| {
            if let Some(host) = host.borrow_mut().as_mut() {
                host.completed = (!request_id.is_empty()).then_some(request_id.clone());
                host.error = error.clone();
                if !request_id.is_empty()
                    && !host.results.iter().any(|(id, _, _)| id == &request_id)
                {
                    if host.results.len() >= 128 {
                        host.results.pop_front();
                    }
                    host.results
                        .push_back((request_id, fingerprint, error.clone()));
                }
            }
        });
        if let Some(error) = error {
            self.notice = Some(error)
        }
        self.publish_unified(cx);
        cx.notify();
    }
    pub(super) fn unified_command(
        &mut self,
        request: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let action = request["action"].as_str().ok_or("Missing action")?;
        let default_model = if matches!(action, "new" | "new_project") {
            if let Some(model) = request["defaultModel"].as_str() {
                if !self.unified_models().iter().any(|m|m.id==model)
                {
                    return Err("Saved default model is unavailable for this agent; choose an available model in Kai’s menu".into());
                }
                Some(model.to_owned())
            } else {
                None
            }
        } else {
            None
        };
        let navigates = matches!(
            action,
            "chat" | "project" | "new" | "new_project" | "workspace_expand" | "navigate"
        );
        // An approval carries its own owner/generation; it may belong to another chat.
        if !matches!(
            action,
            "approve"
                | "reject"
                | "approval_stop"
                | "native_answer"
                | "approval_next"
                | "refresh"
                | "workspace_settings"
        ) {
            if request["ownerToken"].as_str() != Some(self.unified_token().as_str()) {
                return Err("The workspace changed; your input is preserved. Try again.".into());
            }
        }
        if action == "chat" {
            let id = native_id(request, "id").ok_or("Invalid chat")?;
            if !self
                .sessions
                .iter()
                .any(|s| s.id == id && !self.hidden_sessions.contains(&id))
            {
                return Err("Chat is unavailable".into());
            }
        }
        if action == "project" {
            let path = PathBuf::from(request["path"].as_str().ok_or("Missing project")?);
            if !self.projects.iter().any(|p| p.path == path) {
                return Err("Project is unavailable".into());
            }
        }
        if navigates || action == "send" {
            if request["draftBase"].as_str() != Some(self.composer.read(cx).text().as_str()) {
                return Err("The IDE has a different draft. Both drafts are preserved; review the workspace before continuing.".into());
            }
        }
        if navigates || action == "send"{self.unified_history_end=None;self.unified_history_wait=false;}
        if navigates {
            if action != "workspace_expand" {
                self.unified_context = None;
                self.unified_attachment_expected = None;
                self.unified_notch_sources.clear();
            }
            if let Some(draft) = request["draft"].as_str() {
                if draft.len() > input::MAX_DRAFT_BYTES || draft.contains('\0') {
                    return Err("The draft is too large".into());
                }
                let draft = if action == "workspace_expand" {
                    self.unified_context
                        .take()
                        .map(|context| {
                            format!("Context captured for this prompt:\n{context}\n\n{draft}")
                        })
                        .unwrap_or_else(|| draft.to_owned())
                } else {
                    draft.to_owned()
                };
                self.composer
                    .update(cx, |input, cx| input.set_text(&draft, cx));
            }
        }
        match action {
            "chat" => {
                let id = native_id(request, "id").ok_or("Invalid chat")?;
                let session = self
                    .sessions
                    .iter()
                    .find(|s| s.id == id && !self.hidden_sessions.contains(&id))
                    .cloned()
                    .ok_or("Chat is unavailable")?;
                if self.active.as_ref() != Some(&session.id) {
                    self.open_session(session, window, cx);
                }
            }
            "project" => {
                let path = PathBuf::from(request["path"].as_str().ok_or("Missing project")?);
                if !self.projects.iter().any(|p| p.path == path) {
                    return Err("Project is unavailable".into());
                }
                self.select_project(path, window, cx);
            }
            "navigate"=>{
                let back=match request["direction"].as_str(){Some("back")=>true,Some("forward")=>false,_=>return Err("Invalid navigation direction".into())};
                let mut source=if back{std::mem::take(&mut self.unified_back)}else{std::mem::take(&mut self.unified_forward)};
                let target=loop{let s=source.pop().ok_or("No chat in that direction")?;if self.sessions.iter().any(|v|v.id==s.id)&&!self.hidden_sessions.contains(&s.id){break s}};
                if back{self.unified_back=source}else{self.unified_forward=source};
                if let Some(current)=self.active.as_ref().and_then(|id|self.sessions.iter().find(|s|&s.id==id)).cloned(){if back{self.unified_forward.push(current)}else{self.unified_back.push(current)}}
                self.unified_navigating=true;self.open_session(target,window,cx);self.unified_navigating=false;
            }
            "new" => self.new_unprojected_conversation(window, cx),
            "new_project" => self.new_conversation(window, cx),
            "send" => {
                if !self.unified_send_ready() {
                    return Err(
                        "The chat is busy or awaiting confirmation; your prompt is preserved."
                            .into(),
                    );
                }
                let text = request["text"].as_str().ok_or("Missing prompt")?;
                if text.len() > input::MAX_DRAFT_BYTES
                    || text.contains('\0')
                    || text.trim().is_empty() && self.attachments.is_empty()
                {
                    return Err("Enter a prompt under 64 KB".into());
                }
                // Draft contents were checked against the current owner before submission.
                self.composer
                    .update(cx, |input, cx| input.set_text(text, cx));
                let supplied = if let Some(context) = self.unified_context.take() {
                    format!("Context captured when dictation began:\n{context}\n\n{text}")
                } else {
                    text.to_owned()
                };
                self.composer
                    .update(cx, |input, cx| input.set_text(&supplied, cx));
                self.send(window, cx);
                if !self.starting && self.composer.read(cx).text() == supplied {
                    return Err(self.notice.clone().unwrap_or_else(|| {
                        "Prompt staged in the workspace; review its native confirmation".into()
                    }));
                }
            }
            "interrupt" => {
                let id = native_id(request, "targetThreadId").ok_or("Select a running chat")?;
                let run = self.runs.get(&id).ok_or("The run has finished")?;
                if request["runId"].as_str() != Some(run.parse_epoch.to_string().as_str()) {
                    return Err("The run changed; refresh before stopping".into());
                }
                run.handle.cancel();
            }
            "approval_next"=>{
                let tokens=self.unified_pending_ids();if tokens.len()>1{let current=self.unified_pending().and_then(|(_,token,_)|tokens.iter().position(|id|token.ends_with(&format!(":{id}"))));let next=current.map_or(0,|i|(i+1)%tokens.len());self.native_request_focus=Some(tokens[next].clone());}
            }
            "preferences"=>{if let Some(value)=request["textMorph"].as_bool(){self.unified_text_morph=value}if let Some(value)=request["motion"].as_bool(){self.motion=value}self.queue_workspace_save(cx);}
            "child_open"=>{
                let id=request["id"].as_str().ok_or("Missing child")?;
                let child=self.unified_children.iter().find(|c|child_id(c)==id&&c.connected).cloned().ok_or("Child is no longer connected")?;
                if self.broker_panel.is_none(){self.toggle_children(window,cx)}
                self.select_child(child.provider,child.id,cx);reveal();
            }
            "approve" | "reject" | "approval_stop" | "native_answer" => {
                self.unified_approval(request, cx)?
            }
            "settings" => {
                let model = request["model"].as_str().ok_or("Missing model")?;
                let known=model.is_empty() || self.unified_models().iter().any(|m|m.id==model);
                if !known {
                    return Err("That model is no longer available".into());
                }
                if let Some(effort)=request["effort"].as_str().filter(|s|!s.is_empty()){
                    let choice=if model.is_empty(){self.effort_model_choice()}else{self.unified_models().into_iter().find(|m|m.id==model)};
                    if !choice.is_some_and(|m|m.reasoning_efforts.iter().any(|v|v==effort)){return Err("That reasoning effort is not available for this model".into())}
                }
                if self.loading||self.starting||self.handoff_pending.is_some(){return Err("Finish the current handoff before changing the model".into())}
                self.choose_model((!model.is_empty()).then(||model.into()), cx);
                if let Some(effort)=request["effort"].as_str(){let key=self.effort_key();if effort.is_empty(){self.reasoning_efforts.remove(&key);}else{self.reasoning_efforts.insert(key,effort.into());}}
                self.queue_workspace_save(cx);
            }
            "agent" => {
                let name = request["agent"].as_str().ok_or("Missing agent")?;
                let agent = self
                    .agents
                    .iter()
                    .find(|a| a.kind.command() == name && a.available && a.executable.is_some())
                    .map(|a| a.kind)
                    .ok_or("Agent is unavailable")?;
                self.request_handoff(agent, window, cx);
            }
            "context" => {
                if !self.unified_send_ready() {
                    return Err("Wait for the current work before adding context".into());
                }
                let text = request["text"].as_str().unwrap_or("");
                if let Some(paths) = request["paths"].as_array() {
                    if paths.len() + self.attachments.len() > attachments::MAX_ATTACHMENTS {
                        return Err("Image slots are full".into());
                    }
                }
                self.unified_context = Some(bounded(text, 12000));
                if let Some(paths) = request["paths"].as_array().filter(|v| !v.is_empty()) {
                    if paths.len() + self.attachments.len() > attachments::MAX_ATTACHMENTS {
                        return Err("Image slots are full".into());
                    }
                    self.add_attachments(
                        paths
                            .iter()
                            .filter_map(Value::as_str)
                            .map(PathBuf::from)
                            .collect(),
                        cx,
                    );
                }
            }
            "attachments" => {
                if !self.unified_send_ready() {
                    return Err("Wait for this chat before attaching images".into());
                }
                let paths = request["paths"].as_array().ok_or("Missing images")?;
                if paths.len() > attachments::MAX_ATTACHMENTS {
                    return Err("Attach up to four images".into());
                }
                let paths: Vec<PathBuf> = paths
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(PathBuf::from)
                    .collect();
                let additions:Vec<_>=paths.iter().filter(|p|!self.attachments.iter().any(|a|&a.path==*p)&&!self.unified_notch_sources.contains(p)).cloned().collect();
                let retained=self.attachments.iter().filter(|a|paths.contains(&a.path)||self.unified_attachment_sources.iter().any(|(p,id)|*id==a.id&&paths.contains(p))).count();
                if additions.len()+retained>attachments::MAX_ATTACHMENTS{return Err("Image slots are full".into())}
                self.attachments.retain(|a|paths.contains(&a.path)||self.unified_attachment_sources.iter().any(|(p,id)|*id==a.id&&paths.contains(p)));
                self.unified_attachment_expected=Some(self.attachments.len()+additions.len());
                self.unified_notch_sources=paths;
                self.add_attachments(additions,cx);
            }
            "workspace_expand" => reveal(),
            "draft" => {
                let text=request["text"].as_str().ok_or("Missing draft")?;
                if text.len()>input::MAX_DRAFT_BYTES || text.contains('\0'){return Err("Draft is too large".into())}
                if request["draftBase"].as_str()!=Some(self.composer.read(cx).text().as_str()){return Err("The workspace draft changed; your notch draft is preserved".into())}
                if self.composer.read(cx).text()!=text{self.composer.update(cx,|input,cx|input.set_text(text,cx));self.queue_workspace_save(cx);}
            },
            "workspace_settings" => self.show_modal(Modal::Settings, window, cx),
            "workspace_voice" => self.toggle_voice(window, cx),
            "workspace_voice_prompt" => {
                if self.voice_call.is_some() {
                    return Err("A voice conversation is already running".into());
                }
                self.unified_voice_context = None;self.unified_voice_images.clear();self.voice_error=None;
                if let Some(context) = request["appshot"].as_object() {
                    let text = bounded(
                        context.get("text").and_then(Value::as_str).unwrap_or(""),
                        12000,
                    );
                    if let Some(paths) = context.get("paths").and_then(Value::as_array) {
                        if paths.len() > 4 {
                            return Err("Too many screen captures".into());
                        }
                        for path in paths {
                            let path = path
                                .as_str()
                                .filter(|p| {
                                    p.starts_with('/') && p.len() < 4096 && !p.contains('\0')
                                })
                                .ok_or("Invalid captured image path")?;
                            let image=PathBuf::from(path);let info=std::fs::metadata(&image).map_err(|_|"The captured image is no longer available")?;
                            if !info.is_file()||info.len()>25*1024*1024{return Err("The captured image must be a file under 25 MB".into())}
                            self.unified_voice_images.push(image);
                        }
                    }
                    if !text.is_empty() {
                        self.unified_voice_context = Some(format!(
                            "Screen context captured for this voice conversation:\n{text}"
                        ));
                    }
                }
                let prompt = request["audio"]
                    .as_str()
                    .filter(|v| v.len() <= 32 * 1024 * 1024)
                    .map(str::to_owned);
                if self.active.is_none() {
                    self.request_handoff(AgentKind::Codex, window, cx);
                }
                self.start_voice_prompt(prompt, window, cx);
                if self.voice_call.is_none(){return Err(self.notice.clone().unwrap_or_else(||"The native voice session could not start".into()))}
            }
            "workspace_voice_choice" => {
                let voice = request["voice"].as_str().ok_or("Missing voice")?;
                if ![
                    "default", "arbor", "breeze", "cove", "ember", "juniper", "maple", "sol",
                    "spruce", "vale",
                ]
                .contains(&voice)
                {
                    return Err("Unknown voice".into());
                }
                self.voice_choice = voice.into();
                self.queue_workspace_save(cx);
            }
            "voice_stop" => self.stop_voice(cx),
            "files" | "folder" => {
                self.unified_files(request, window, cx)?;
            }
            "history_more" => {
                let first=self.unified_history().first().and_then(|v|v["id"].as_str()).and_then(|s|s.strip_prefix("row-")).and_then(|v|v.parse::<usize>().ok()).unwrap_or(0);
                if first>0{self.unified_history_wait=false;self.unified_history_end=Some((self.unified_token(),first));}
                else if self.has_older{
                    self.load_history_page(true,cx);
                    if self.loading_earlier{self.unified_history_end=Some((self.unified_token(),0));self.unified_history_wait=true;}
                }
            }
            "history_latest"=>{
                self.unified_history_end=None;self.unified_history_wait=false;
                if self.history_offset>0 || self.navigation_history{if let Some(id)=self.active.clone(){if self.runs.contains_key(&id){self.restore_live_tail(&id,cx)}else{self.load_history_page(false,cx)}}}
            }
            "refresh" | "models" => {self.refresh_unified_models(cx);}
            // Settings stay explicit: unsupported actions cannot launch another runtime.
            _ => return Err(format!("Open the Code workspace to use {action}")),
        }
        if let Some(model) = default_model {
            self.choose_model(Some(model), cx);
        }
        Ok(())
    }
    fn publish_unified(&self, cx: &App) {
        if !active() {
            return;
        }
        if let Some(call) = &self.voice_call {
            let values: Vec<f32> = call
                .media
                .levels
                .iter()
                .map(|l| f32::from_bits(l.load(std::sync::atomic::Ordering::Relaxed)))
                .collect();
            HOST.with(|host| {
                if let Some(host) = host.borrow().as_ref() {
                    unsafe { (host.levels)(values.as_ptr(), values.len()) }
                }
            });
        }
        let mut state = self.unified_snapshot(cx);
        HOST.with(|host| {
            if let Some(host) = host.borrow_mut().as_mut() {
                let finished:Vec<_>=self.unified_successes.iter().filter(|(id,epoch)|!host.reported_successes.iter().any(|(seen,run)|seen==id&&run==epoch) && !self.finalizing.contains(id) && !self.settling.contains_key(id)).cloned().collect();
                for event in &finished{if host.reported_successes.len()>=128{host.reported_successes.pop_front();}host.reported_successes.push_back(event.clone());}
                let finished:Vec<_>=finished.iter().map(|(id,run)|json!({"id":if id.starts_with("child:"){id.clone()}else{format!("code:{id}")},"runId":run})).collect();
                if !finished.is_empty() {
                    host.finished = finished;
                }
                state["finishedTasks"] = json!(host.finished);
                state["completedRequestId"] = json!(host.completed);
                state["completedRequestError"] = json!(host.error);
                if self.attachment_pending > 0 || self.loading_earlier {
                    state["completedRequestId"] = Value::Null;
                } else if self
                    .unified_attachment_expected
                    .is_some_and(|expected| expected != self.attachments.len())
                {
                    state["completedRequestError"] = json!(self.notice.as_deref().unwrap_or(
                        "An image could not be imported. Review the draft before sending."
                    ));
                }
                let owner=state["threadId"].as_str().unwrap_or("").to_owned();
                let history=state["history"].clone();
                if host.history_owner==owner && host.history==history{state.as_object_mut().unwrap().remove("history");}
                else{host.history=history;host.history_owner=owner;}
                let Ok(data) = serde_json::to_vec(&state) else {
                    return;
                };
                if data == host.last || data.len() > 2 * 1024 * 1024 {
                    return;
                }
                host.last = data;
                unsafe { (host.publish)(host.last.as_ptr(), host.last.len()) };
            }
        });
    }
    pub(super) fn unified_snapshot(&self, cx: &App) -> Value {
        let running = self.active.as_ref().and_then(|id| self.runs.get(id));
        let mut tasks:Vec<_>=self.runs.iter().map(|(id,run)|{
            let name=self.sessions.iter().find(|s|s.id==*id).map(|s|s.title.as_str()).unwrap_or("Agent");
            json!({"id":format!("code:{id}"),"name":name,"runId":run.parse_epoch.to_string(),"scene":self.native_pending.values().find(|p|p.owner==*id&&p.current(&self.runs)).map(|p|p.indicator_scene()).unwrap_or(scene(&run.status)),"text":format!("{} · {}",run.agent.name(),run.status),"agent":run.agent.command()})
        }).collect();
        for child in self.unified_children.iter().filter(|c|c.connected && (matches!(c.status,chat_interactions::InteractionStatus::Started|chat_interactions::InteractionStatus::Working)||!c.requests.is_empty())){
            let parent=self.child_owner(child,&self.unified_children).map(|s|format!("code:{}",s.id));
            if parent.is_none(){continue}
            tasks.push(json!({"id":child_id(child),"name":format!("{} · {}",child.provider.name(),short(&child.id,10)),"runId":child.turn_id.clone().unwrap_or_else(||child.id.clone()),"scene":if child.requests.is_empty(){"thinking"}else{"permission"},"text":child.preview,"agent":child.provider.command(),"parentId":parent}));
        }
        tasks.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        let approval = self.unified_pending();
        let activity=approval.as_ref().map(|(owner,token,summary)|json!({"scene":"permission","text":summary,"owner":owner,"request":token})).or_else(||tasks.first().map(|t|json!({"scene":t["scene"],"text":t["text"]}))).unwrap_or(json!({"scene":"off","text":"Ready"}));
        let terminal = self
            .indicator_terminal
            .as_ref()
            .filter(|(id, _)| self.active.as_ref() == Some(id))
            .map(|(_, scene)| *scene);
        let selected_activity = running
            .map(|run| json!({"scene":scene(&run.status),"text":run.status}))
            .unwrap_or_else(|| match terminal {
                Some("failure") => {
                    json!({"scene":"failure","text":"Agent failed — review the workspace"})
                }
                Some("declined") => json!({"scene":"declined","text":"Stopped"}),
                _ => json!({"scene":"off","text":"Ready"}),
            });
        let selected = Some(
            self.active
                .as_ref()
                .map(|id| format!("code:{id}"))
                .unwrap_or_else(|| format!("code:draft-{}", self.generation)),
        );
        let chat = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id));
        let row=|s:&SessionSummary|json!({"id":format!("code:{}",s.id),"name":s.title,"agent":s.agent.command(),"status":if self.runs.contains_key(&s.id){"active"}else{"idle"}});
        let chats:Vec<_>=self.sessions.iter().filter(|s|!self.hidden_sessions.contains(&s.id)&&s.project_path==self.project).take(100).map(row).collect();
        let standalone:Vec<_>=self.sessions.iter().filter(|s|!self.hidden_sessions.contains(&s.id)&&s.project_path==self.unprojected_root()&&s.project_path!=self.project).take(60).map(row).collect();
        let models=self.unified_models();
        let (effective_model,effective_effort)=self.effective_model_settings();
        let history=if self.loading{vec![]}else{self.unified_history()};
        let local_older=history.first().and_then(|v|v["id"].as_str()).is_some_and(|v|v!="row-0");
        let mut snapshot=json!({"project":self.project,"projects":self.projects.iter().take(80).map(|p|json!({"path":p.path,"name":p.name})).collect::<Vec<_>>(),"projectsLoading":false,
            "chats":chats,"standaloneChats":standalone,"standalone":self.project==self.unprojected_root(),"codeChats":[],"codeConnection":{"connected":true,"enabled":true},
            "threadId":selected,"threadName":chat.map(|s|s.title.as_str()).unwrap_or("New chat"),"ownerToken":self.unified_token(),"composerDraft":self.composer.read(cx).text(),
            "historyLoading":self.loading || self.loading_earlier,"history":history,"historyHasMore":self.has_older || local_older,"files":self.unified_file_rows(cx),
            "readOnly":self.history_offset>0 || self.navigation_history,"composerBusy":!self.unified_send_ready(),"activeTurnId":running.map(|r|r.parse_epoch.to_string()),
            "nativeReview":self.unified_review(),"approval":approval.is_some(),"approvalThreadId":approval.as_ref().map(|a|format!("code:{}",a.0)),"approvalRequestId":approval.as_ref().map(|a|a.1.clone()),"approvalSummary":approval.as_ref().map(|a|a.2.clone()),
            "activity":if tasks.is_empty(){selected_activity.clone()}else{activity},"sessionActivity":self.voice_call.as_ref().filter(|v|v.owns_current_chat(self.active.as_deref())).map(|v|json!({"scene":v.scene(),"text":format!("{} · {}",v.caption(),voice::REALTIME_MODEL)})).unwrap_or(selected_activity),"activeTasks":tasks,"focusThreadId":approval.as_ref().map(|a|format!("code:{}",a.0)).or_else(||tasks.first().and_then(|t|t["id"].as_str().map(str::to_owned))),
            "notice":self.notice.as_deref().unwrap_or("Kai · Code"),"attachments":self.attachments.iter().filter(|a|!self.attachment_removals.contains_key(&a.id)).map(|a|a.path.to_string_lossy().into_owned()).collect::<Vec<_>>(),"modelsLoading":self.controls_pending.contains(self.agent.command()),"models":models.iter().map(|m|json!({"model":m.id,"displayName":m.label,"resolvedModel":self.controls.get(self.agent.command()).and_then(|(_,v)|v.defaults.aliases.get(&m.id)).unwrap_or(&m.id),"supportedReasoningEfforts":m.reasoning_efforts.iter().map(|v|json!({"reasoningEffort":v})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "selectedModel":self.model,"reasoningEffort":self.unified_effort().unwrap_or(""),"canGoBack":!self.unified_back.is_empty(),"canGoForward":!self.unified_forward.is_empty(),
            "agents":self.agents.iter().map(|a|json!({"id":a.kind.command(),"name":a.kind.name(),"available":a.available&&a.executable.is_some()})).collect::<Vec<_>>(),"selectedAgent":self.agent.command(),"workspaceVoiceChoice":self.voice_choice,"workspaceVoiceActive":self.voice_call.is_some(),"workspaceVoiceScene":self.voice_call.as_ref().map(|v|v.scene())});
        snapshot["providerDefaultModel"]=json!(self.provider_default_model());snapshot["effectiveModel"]=json!(effective_model);snapshot["effectiveReasoningEffort"]=json!(effective_effort);snapshot["voiceModel"]=json!(self.voice_call.as_ref().map(|_|voice::REALTIME_MODEL));
        snapshot["workspaceVoiceError"]=json!(self.voice_error);snapshot["historyEarlier"]=json!(self.unified_history_end.as_ref().is_some_and(|(owner,_)|owner==&self.unified_token()) || self.history_offset>0 || self.navigation_history);snapshot["approvalCount"]=json!(self.unified_pending_ids().len());snapshot["workspaceMotion"]=json!(self.motion);snapshot["workspaceTextMorph"]=json!(self.unified_text_morph);snapshot

    }
    fn unified_history(&self) -> Vec<Value> {
        let mut history = Vec::new();
        let mut role = "assistant".to_owned();
        let end = self.unified_history_end.as_ref().filter(|(owner,_)|owner==&self.unified_token() && !self.unified_history_wait).map_or(self.rows.len(),|(_,end)|(*end).min(self.rows.len()));
        let start = end.saturating_sub(100);
        for row in self.rows.iter().take(start) {
            if let Some(r) = &row.role {
                role = r.clone()
            }
        }
        for (index, row) in self.rows.iter().enumerate().take(end).skip(start) {
            if let Some(r) = &row.role {
                role = r.clone()
            }
            let (kind, text, style) = match &row.block {
                MessageBlock::Paragraph(t) | MessageBlock::Quote(t) => {
                    (role.as_str(), t.clone(), "text")
                }
                MessageBlock::Heading(t) => (role.as_str(), t.clone(), "heading"),
                MessageBlock::RichParagraph { text, highlights }
                | MessageBlock::ListItem {
                    text, highlights, ..
                } => (role.as_str(), unified_links(text, highlights), "text"),
                MessageBlock::Code { text, .. } => (role.as_str(), text.clone(), "code"),
                MessageBlock::Voice { speaker, text, .. } => (
                    if *speaker == chat_interactions::Speaker::User {
                        "user"
                    } else {
                        "assistant"
                    },
                    text.clone(),
                    "text",
                ),
                MessageBlock::Activity { label, .. } => ("activity", label.clone(), "activity"),
                MessageBlock::TableRow { cells, .. } => (role.as_str(), cells.join(" | "), "text"),
                MessageBlock::Attachment { name, target } => (
                    role.as_str(),
                    unified_links(
                        name,
                        &[(
                            0..name.len(),
                            InlineStyle::Link(std::sync::Arc::from(target.as_str())),
                        )],
                    ),
                    "text",
                ),
                MessageBlock::Interaction(card) => match card {
                    chat_interactions::ChatInteraction::Speaker { speaker, text } => (
                        if *speaker == chat_interactions::Speaker::User {
                            "user"
                        } else {
                            "assistant"
                        },
                        text.clone(),
                        "text",
                    ),
                    chat_interactions::ChatInteraction::Status { title, details, .. }
                    | chat_interactions::ChatInteraction::Delegation { title, details, .. } => {
                        ("activity", format!("{title} · {details}"), "activity")
                    }
                    chat_interactions::ChatInteraction::Question(q) => {
                        ("assistant", q.prompt.clone(), "heading")
                    }
                    chat_interactions::ChatInteraction::Answer { answer, .. } => {
                        ("assistant", answer.clone(), "text")
                    }
                    chat_interactions::ChatInteraction::NativeRequest(r) => {
                        ("activity", r.title.clone(), "activity")
                    }
                },
            };
            history.push(json!({"id":format!("row-{index}"),"kind":if kind.starts_with("user"){"user"}else if kind=="activity"{"activity"}else{"assistant"},"lines":[{"text":bounded(&text,4000),"style":style}],"agent":role.split(':').nth(1).unwrap_or(self.agent.command()),"speechText":if kind.starts_with("user") || kind=="activity" || style=="code" {String::new()}else{bounded(&text,4000)}}));
        }
        budget_history(history)
    }
}

impl Orbit {
    fn unified_file_rows(&self, cx: &App) -> Vec<Value> {
        self.file_browser
            .as_ref()
            .map(|b| b.read(cx).unified_rows())
            .unwrap_or_default()
    }
    fn unified_files(
        &mut self,
        request: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.files_visible = true;
        self.sync_file_root(window, cx);
        if request["action"] == "folder" {
            let path = PathBuf::from(request["path"].as_str().ok_or("Missing folder")?);
            if let Some(browser) = &self.file_browser {
                browser.update(cx, |b, cx| b.unified_folder(path, cx))?;
            }
        }
        Ok(())
    }
}

/// A separate app identity and data store protect the user's current Code build.
/// Provider login files stay with their CLIs; no credentials are copied.
pub(super) fn prepare_environment() {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    if !executable
        .parent()
        .is_some_and(|p| p.join("../Resources/libKaiNotch.dylib").exists())
    {
        return;
    }
    if std::env::args().any(|arg| arg == "--unified-smoke-test") {
        let path = std::env::temp_dir().join(format!("kai-unified-smoke-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&path);
        unsafe { std::env::set_var("ORBIT_DATA_DIR", path) };
        return;
    }
    if std::env::var_os("ORBIT_DATA_DIR").is_some() {
        return;
    }
    let Some(home) = dirs::home_dir() else { return };
    let source = home.join("Library/Application Support/Orbit");
    let destination = home.join("Library/Application Support/Kai Unified");
    if let Err(error) = std::fs::create_dir_all(&destination) {
        eprintln!("Kai data directory: {error}");
        return;
    }
    // One-time, bounded import of UI preferences, never the live SQLite database.
    let marker = destination.join(".seeded");
    if !marker.exists() {
        for name in ["workspace.json", "projects.json"] {
            let from = source.join(name);
            let to = destination.join(name);
            if !to.exists()
                && std::fs::metadata(&from).is_ok_and(|m| m.is_file() && m.len() <= 2 * 1024 * 1024)
            {
                let _ = std::fs::copy(from, to);
            }
        }
        let _ = std::fs::write(marker, b"1\n");
    }
    // Called before GPUI, workers, or any other application threads start.
    unsafe { std::env::set_var("ORBIT_DATA_DIR", destination) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    fn request(app: &Orbit, action: &str, cx: &App) -> Value {
        json!({"action":action,"ownerToken":app.unified_token(),"draftBase":app.composer.read(cx).text(),"targetThreadId":app.active.as_ref().map(|id|format!("code:{id}")),"requestId":uuid::Uuid::new_v4().to_string()})
    }
    #[gpui::test]
    fn unified_send_during_run_never_cancels_or_replaces_draft(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            app.active = Some("fixture-session".into());
            app.loading = false;
            app.composer
                .update(cx, |input, cx| input.set_text("IDE draft", cx));
            app.runs.insert(
                "fixture-session".into(),
                crate::ui_tests::idle_run("Working".into()),
            );
            let mut body = request(app, "send", cx);
            body["text"] = json!("New prompt");
            assert!(app.unified_command(&body, window, cx).is_err());
            assert_eq!(app.composer.read(cx).text(), "IDE draft");
            assert!(!app.runs["fixture-session"].handle.is_cancelled());
        });
    }
    #[gpui::test]
    fn unified_draft_handoff_rejects_a_newer_ide_draft(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            let mut body = request(app, "workspace_expand", cx);
            body["draft"] = json!("Notch draft");
            app.composer
                .update(cx, |input, cx| input.set_text("New IDE draft", cx));
            // Even with an updated selection token, a divergent draft is preserved.
            body["ownerToken"] = json!(app.unified_token());
            assert!(app.unified_command(&body, window, cx).is_err());
            assert_eq!(app.composer.read(cx).text(), "New IDE draft");
        });
    }
    #[gpui::test]
    fn unified_stop_is_bound_to_the_exact_run(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            app.active = Some("fixture-session".into());
            app.runs.insert(
                "fixture-session".into(),
                crate::ui_tests::idle_run("Working".into()),
            );
            let mut body = request(app, "interrupt", cx);
            body["runId"] = json!("old-run");
            assert!(app.unified_command(&body, window, cx).is_err());
            assert!(!app.runs["fixture-session"].handle.is_cancelled());
            body["runId"] = json!(app.runs["fixture-session"].parse_epoch.to_string());
            app.unified_command(&body, window, cx).unwrap();
            assert!(app.runs["fixture-session"].handle.is_cancelled());
        });
    }
    #[gpui::test]
    fn unified_background_approval_routes_once_and_rejects_stale_epoch(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let (handle, decisions) = RunHandle::interactive_test_handle();
        view.update_in(cx,|app,window,cx|{
            app.active=Some("other-chat".into());
            let mut run=crate::ui_tests::idle_run("Approval".into());run.handle=handle;
            let native=runtime::NativeRequest{id:"opaque".into(),method:"test".into(),title:"Run tests".into(),detail:"cargo test".into(),kind:runtime::NativeRequestKind::Approval{choices:vec![runtime::NativeOption{id:"accept".into(),label:"Allow once".into()}]}};
            app.native_pending.insert("opaque".into(),native_request_ui::LiveRequest::new("fixture-session",run.parse_epoch,native));
            app.runs.insert("fixture-session".into(),run);
            let pending=app.unified_pending().unwrap();
            let mut body=json!({"action":"approve","threadId":"code:fixture-session","approvalRequestId":"old:opaque"});
            assert!(app.unified_command(&body,window,cx).is_err());assert!(decisions.try_recv().is_err());
            body["approvalRequestId"]=json!(pending.1);
            app.unified_command(&body,window,cx).unwrap();
            assert!(matches!(decisions.try_recv().unwrap(),runtime::NativeControl::Respond{id,answer:runtime::NativeAnswer::Choice(choice)}if id=="opaque"&&choice=="accept"));
            assert!(app.unified_command(&body,window,cx).is_err());assert!(decisions.try_recv().is_err());
            assert_eq!(app.active.as_deref(),Some("other-chat"));
        });
    }
    #[gpui::test]
    fn unified_new_chat_and_selection_keep_the_previous_draft(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            app.active = Some("fixture-session".into());
            app.loading = false;
            let mut body = request(app, "new", cx);
            body["draft"] = json!("Keep my draft");
            app.unified_command(&body, window, cx).unwrap();
            assert_eq!(app.drafts["fixture-session"], "Keep my draft");
            assert!(app.active.is_none());
            assert!(app.runs.is_empty());
            assert!(
                app.unified_snapshot(cx)["threadId"]
                    .as_str()
                    .unwrap()
                    .starts_with("code:draft-")
            );
        });
    }
    #[gpui::test]
    fn unified_snapshot_preserves_roles_and_has_bounded_native_history(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.rows = (0..150)
                .map(|n| Row {
                    role: Some(
                        if n % 2 == 0 {
                            "user:codex"
                        } else {
                            "assistant:claude"
                        }
                        .into(),
                    ),
                    block: MessageBlock::Paragraph(format!("Message {n}")),
                })
                .collect();
            let value = app.unified_snapshot(cx);
            assert_eq!(value["history"].as_array().unwrap().len(), 100);
            assert_eq!(value["history"][0]["kind"], "user");
            assert_eq!(value["history"][1]["kind"], "assistant");
            assert_eq!(value["history"][0]["speechText"], "");
            assert_eq!(value["history"][1]["speechText"], "Message 51");
            assert_eq!(value["codeConnection"]["connected"], true);
            assert!(value["agents"].is_array());
        });
    }
}

fn unified_links(text: &str, highlights: &[(std::ops::Range<usize>, InlineStyle)]) -> String {
    let mut rendered = text.to_owned();
    let mut spans = Vec::new();
    for (range, style) in highlights {
        if let InlineStyle::Link(target) = style {
            if (target.starts_with("https://")
                || target.starts_with("http://")
                || target.starts_with('/'))
                && !target.contains(['"', '\n', '\r'])
                && range.start < range.end
                && range.end <= text.len()
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end)
            {
                spans.push((range.clone(), target.clone()))
            }
        }
    }
    spans.sort_by_key(|(r, _)| r.start);
    let mut boundary = text.len();
    for (range, target) in spans.into_iter().rev() {
        if range.end <= boundary {
            rendered.replace_range(
                range.clone(),
                &format!(":codex-file-citation{{path=\"{target}\" purpose=\"output\"}}"),
            );
            boundary = range.start
        }
    }
    rendered
}

#[cfg(test)]
mod final_regressions {
    use super::*;
    use std::prelude::v1::test;

    #[gpui::test]
    fn unified_completion_uses_the_runtime_outcome_and_keeps_failures_visible(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            for outcome in ["failed", "stopped", "complete"] {
                let events = if outcome == "failed" {
                    vec![RuntimeEvent::Failed("Fixture failure".into())]
                } else {
                    vec![RuntimeEvent::Finished]
                };
                let mut run = crate::ui_tests::idle_run("Saved result".into());
                run.handle = RunHandle::test_events(events);
                if outcome == "stopped" {
                    run.handle.cancel();
                }
                let epoch = run.parse_epoch.to_string();
                app.runs.insert("fixture-session".into(), run);
                app.poll("fixture-session", cx);
                let successful = app
                    .unified_successes
                    .iter()
                    .any(|(id, run)| id == "fixture-session" && run == &epoch);
                assert_eq!(successful, outcome == "complete");
                let snapshot = app.unified_snapshot(cx);
                if outcome == "failed" {
                    assert_eq!(snapshot["sessionActivity"]["scene"], "failure")
                }
                if outcome == "stopped" {
                    assert_eq!(snapshot["sessionActivity"]["scene"], "declined")
                }
            }
        });
    }

    #[gpui::test]
    fn unified_question_form_checks_required_answers_and_sends_once(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let (handle, answers) = RunHandle::interactive_test_handle();
        view.update_in(cx,|app,window,cx|{
            let mut run=crate::ui_tests::idle_run("Question".into());run.handle=handle;
            let request=runtime::NativeRequest{id:"form".into(),method:"question".into(),title:"Choose".into(),detail:"".into(),kind:runtime::NativeRequestKind::Questions{questions:vec![runtime::NativeQuestion{id:"q".into(),header:"Choice".into(),question:"Pick one".into(),options:vec![runtime::NativeOption{id:"a".into(),label:"A".into()}],secret:false,multiple:false,required:true}]}};
            app.native_pending.insert("form".into(),native_request_ui::LiveRequest::new("fixture-session",run.parse_epoch,request));
            app.runs.insert("fixture-session".into(),run);
            let token=app.unified_pending().unwrap().1;
            let mut body=json!({"action":"native_answer","threadId":"code:fixture-session","approvalRequestId":token,"answers":{"q":[]}});
            assert!(app.unified_command(&body,window,cx).is_err());assert!(answers.try_recv().is_err());
            body["answers"]=json!({"q":["a","another"]});
            assert!(app.unified_command(&body,window,cx).is_err());assert!(answers.try_recv().is_err());
            body["answers"]=json!({"q":["a"]});app.unified_command(&body,window,cx).unwrap();
            match answers.try_recv().unwrap(){runtime::NativeControl::Respond{id,answer:runtime::NativeAnswer::Answers(value)}=>{assert_eq!(id,"form");assert_eq!(value["q"],vec!["a"]);},_=>panic!("Expected native form response")}
            assert!(app.unified_command(&body,window,cx).is_err());assert!(answers.try_recv().is_err());
        });
    }
}

impl Orbit {
    pub(super) fn unified_models(&self)->Vec<agent_controls::ModelChoice>{
        let mut models=self.native_models.get(&self.agent).cloned().unwrap_or_default();
        if let Some((_,info))=self.controls.get(self.agent.command()){
            for model in &info.models{if let Some(existing)=models.iter_mut().find(|m|m.id==model.id){existing.reasoning_efforts=model.reasoning_efforts.clone();}else{models.push(model.clone());}}
        }
        models
    }
    fn refresh_unified_models(&mut self,cx:&mut Context<Self>){
        if !active() || self.controls_pending.contains(self.agent.command()){return}
        let project=self.model_project();
        if self.controls.get(self.agent.command()).is_some_and(|(at,info)|at.elapsed()<Duration::from_secs(60)&&info.defaults.project==project){return}
        let kind=self.agent;
        let Some(path)=self.agents.iter().find(|a|a.kind==kind&&a.available).and_then(|a|a.executable.clone())else{return};
        self.controls_pending.insert(kind.command());let home=self.controls_home.clone();
        cx.spawn(async move|this,cx|{
            let info=cx.background_executor().spawn(async move{agent_controls::inspect_project(kind,&path,&home,&project)}).await;
            let _=this.update(cx,|app,cx|{app.controls_pending.remove(kind.command());app.controls.insert(kind.command(),(Instant::now(),info));cx.notify();});
        }).detach();
    }
}

pub(super) fn decode_enabled(motion:bool)->bool{
    #[cfg(not(test))]{active()&&motion&&!native::reduced_motion()}
    #[cfg(test)]{let _=motion;false}
}
pub(super) fn pet_slot(bounds:Bounds<Pixels>){
    let rect=[f32::from(bounds.origin.x),f32::from(bounds.origin.y),f32::from(bounds.size.width),f32::from(bounds.size.height)];
    let changed=HOST.with(|host|{let mut host=host.borrow_mut();let Some(host)=host.as_mut()else{return false};if host.pet_rect==Some(rect){return false}host.pet_rect=Some(rect);true});
    if changed{emit(&json!({"type":"workspacePetSlot","rect":rect}));}
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use gpui::TestAppContext;
    use std::prelude::v1::test;
    fn request(app:&Orbit,action:&str,cx:&App)->Value{json!({"action":action,"ownerToken":app.unified_token(),"draftBase":app.composer.read(cx).text()})}

    #[test]
    fn unicode_and_escaped_history_has_a_total_encoded_byte_budget(){
        for text in ["文".repeat(4000),"\\\"\n".repeat(4000)]{
            let rows=(0..100).map(|i|json!({"id":format!("row-{i}"),"kind":"assistant","lines":[{"text":text,"style":"text"}],"speechText":text})).collect();
            let kept=budget_history(rows);let bytes=serde_json::to_vec(&kept).unwrap();
            assert!(bytes.len()<=HISTORY_BYTES);assert_eq!(kept.last().unwrap()["id"],"row-99");assert!(!kept.is_empty());
        }
    }
    #[gpui::test]
    fn bounded_mini_history_pages_every_loaded_row_and_returns_to_latest(cx:&mut TestAppContext){
        let (view,cx)=crate::ui_tests::fixture(cx);view.update_in(cx,|app,window,cx|{
            app.loading=false;app.rows=(0..240).map(|i|Row{role:Some("assistant:codex".into()),block:MessageBlock::Paragraph(format!("{i}:{}","文".repeat(4000)))}).collect();app.has_older=false;
            let mut seen=HashSet::new();let mut pages=0;
            loop{let state=app.unified_snapshot(cx);for row in state["history"].as_array().unwrap(){seen.insert(row["id"].as_str().unwrap().to_owned());}pages+=1;if state["historyHasMore"]==false{break}let body=request(app,"history_more",cx);app.unified_command(&body,window,cx).unwrap();assert!(pages<50,"History cursor did not move");}
            assert_eq!(seen.len(),240);assert!(app.unified_snapshot(cx)["historyEarlier"]==true);
            let body=request(app,"history_latest",cx);app.unified_command(&body,window,cx).unwrap();assert_eq!(app.unified_snapshot(cx)["history"].as_array().unwrap().last().unwrap()["id"],"row-239");assert!(app.unified_snapshot(cx)["historyEarlier"]==false);
        });
    }
    #[gpui::test]
    fn navigation_round_trips_exact_sessions_and_preserves_each_draft(cx:&mut TestAppContext){
        let (view,cx)=crate::ui_tests::fixture(cx);view.update_in(cx,|app,window,cx|{
            let first=app.sessions[0].clone();let mut second=first.clone();second.id="second-chat".into();second.title="Second".into();app.sessions.push(second.clone());app.active=Some(first.id.clone());app.loading=false;
            app.composer.update(cx,|input,cx|input.set_text("First draft",cx));let mut body=request(app,"chat",cx);body["id"]=json!(format!("code:{}",second.id));app.unified_command(&body,window,cx).unwrap();assert!(!app.unified_back.is_empty());
            app.loading=false;app.composer.update(cx,|input,cx|input.set_text("Second draft",cx));let mut body=request(app,"navigate",cx);body["direction"]=json!("back");app.unified_command(&body,window,cx).unwrap();assert_eq!(app.active.as_deref(),Some(first.id.as_str()));assert_eq!(app.composer.read(cx).text(),"First draft");
            app.loading=false;let mut body=request(app,"navigate",cx);body["direction"]=json!("forward");app.unified_command(&body,window,cx).unwrap();assert_eq!(app.active.as_deref(),Some(second.id.as_str()));assert_eq!(app.composer.read(cx).text(),"Second draft");
        });
    }
    #[gpui::test]
    fn delegated_task_snapshot_keeps_parent_provider_and_native_epoch(cx:&mut TestAppContext){
        let (view,cx)=crate::ui_tests::fixture(cx);view.update(cx,|app,cx|{
            let parent=&mut app.sessions[0];parent.agent=AgentKind::Codex;parent.external_id=Some("native-parent".into());let project=parent.execution_path.as_ref().unwrap_or(&parent.project_path).clone();let parent_id=parent.id.clone();
            app.unified_children=vec![runtime::broker::ChildSnapshot{provider:AgentKind::Codex,project,parent_id:"native-parent".into(),id:"native-child".into(),turn_id:Some("child-turn".into()),status:chat_interactions::InteractionStatus::Working,preview:"Working child".into(),requests:vec![],transcript_bytes:0,transcript_revision:0,connected:true}];
            let snapshot=app.unified_snapshot(cx);let child=snapshot["activeTasks"].as_array().unwrap().iter().find(|v|v["id"].as_str().unwrap().starts_with("child:")).unwrap();assert_eq!(child["parentId"],format!("code:{parent_id}"));assert_eq!(child["runId"],"child-turn");assert_eq!(child["agent"],"codex");
            app.unified_children[0].status=chat_interactions::InteractionStatus::Cancelled;assert!(!app.unified_snapshot(cx)["activeTasks"].as_array().unwrap().iter().any(|v|v["id"].as_str().unwrap().starts_with("child:")));
        });
    }
    #[gpui::test]
    fn effort_choices_come_from_catalog_and_reach_owner_state(cx:&mut TestAppContext){
        let (view,cx)=crate::ui_tests::fixture(cx);
        view.update_in(cx,|app,window,cx|{
            app.loading=false;app.starting=false;app.handoff_pending=None;app.agent=AgentKind::Codex;
            app.native_models.insert(AgentKind::Codex,vec![agent_controls::ModelChoice{id:"fixture-model".into(),label:"Fixture".into(),reasoning_efforts:vec!["low".into(),"high".into()]}]);
            let mut body=request(app,"settings",cx);body["model"]=json!("fixture-model");body["effort"]=json!("high");app.unified_command(&body,window,cx).unwrap();
            assert_eq!(app.unified_snapshot(cx)["reasoningEffort"],"high");assert_eq!(app.workspace_snapshot(cx).reasoning_efforts["codex:fixture-model"],"high");
            body["effort"]=json!("invented");assert!(app.unified_command(&body,window,cx).is_err());assert_eq!(app.unified_effort(),Some("high"));
            app.model=None;app.controls.insert("codex",(Instant::now(),agent_controls::ControlInfo{defaults:agent_controls::ModelDefaults{model:Some("fixture-model".into()),..Default::default()},auth:agent_controls::AuthState::Unknown,models:vec![],note:None,account:None,limits:vec![],catalog:None}));
            body=request(app,"settings",cx);body["model"]=json!("");body["effort"]=json!("low");app.unified_command(&body,window,cx).unwrap();assert!(app.model.is_none());assert_eq!(app.unified_effort(),Some("low"));
        });
    }
    #[gpui::test]
    fn inbox_cycles_native_requests_without_navigating_the_owner(cx:&mut TestAppContext){
        let (view,cx)=crate::ui_tests::fixture(cx);
        view.update_in(cx,|app,window,cx|{
            app.active=Some("fixture-session".into());let run=crate::ui_tests::idle_run("Approval".into());let epoch=run.parse_epoch;app.runs.insert("fixture-session".into(),run);
            for id in ["a","b"]{let request=runtime::NativeRequest{id:id.into(),method:"test".into(),title:id.into(),detail:"cargo test".into(),kind:runtime::NativeRequestKind::Approval{choices:vec![runtime::NativeOption{id:"accept".into(),label:"Allow once".into()}]}};app.native_pending.insert(id.into(),native_request_ui::LiveRequest::new("fixture-session",epoch,request));}
            assert_eq!(app.unified_pending_ids(),vec!["a","b"]);let initial=app.unified_pending().unwrap().1;
            let body=request(app,"approval_next",cx);app.unified_command(&body,window,cx).unwrap();assert_ne!(initial,app.unified_pending().unwrap().1);assert_eq!(app.active.as_deref(),Some("fixture-session"));app.unified_command(&body,window,cx).unwrap();assert_eq!(initial,app.unified_pending().unwrap().1);
        });
    }
    #[gpui::test]
    fn project_snapshot_and_models_follow_the_actual_owner(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update(cx,|app,cx|{
            let mut other=app.sessions[0].clone();other.id="other".into();other.project_path=app.unprojected_root();app.sessions.push(other);
            let first=app.unified_snapshot(cx);
            assert_eq!(first["chats"].as_array().unwrap().len(),1);
            assert_eq!(first["chats"][0]["id"],"code:fixture-session");
            assert_eq!(first["standaloneChats"][0]["id"],"code:other");assert_eq!(first["standalone"],false);
            app.project=app.unprojected_root();let second=app.unified_snapshot(cx);
            assert_eq!(second["chats"][0]["id"],"code:other");assert_eq!(second["standalone"],true);
            let choice=|id:&str|agent_controls::ModelChoice{ reasoning_efforts:vec![],id:id.into(),label:id.into()};
            app.controls.insert(app.agent.command(),(Instant::now(),agent_controls::ControlInfo{defaults:Default::default(),auth:agent_controls::AuthState::Unknown,models:vec![choice("catalog-model")],note:None,account:None,limits:vec![],catalog:None}));
            assert_eq!(app.unified_snapshot(cx)["models"][0]["model"],"catalog-model");
            app.native_models.insert(app.agent,vec![choice("catalog-model"),choice("live-model")]);
            assert_eq!(app.unified_snapshot(cx)["models"].as_array().unwrap().len(),2);
            app.agent=AgentKind::Gemini;assert!(app.unified_snapshot(cx)["models"].as_array().unwrap().is_empty());
        });
    }
    #[gpui::test]
    fn draft_changes_preserve_selection_token_but_reject_divergent_base(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update_in(cx,|app,window,cx|{
            let token=app.unified_token();let mut body=request(app,"draft",cx);body["text"]=json!("Notch draft");
            app.unified_command(&body,window,cx).unwrap();assert_eq!(app.unified_token(),token);
            let base=app.composer.read(cx).text();assert_eq!(base,"Notch draft");
            body["text"]=json!("Stale write");assert!(app.unified_command(&body,window,cx).is_err());assert_eq!(app.composer.read(cx).text(),base);
            let mut settings=request(app,"settings",cx);settings["model"]=json!("");app.unified_command(&settings,window,cx).unwrap();
        });
    }
    #[gpui::test]
    fn two_notch_submissions_reach_the_native_provider_and_finish(cx:&mut TestAppContext){
        use std::os::unix::fs::PermissionsExt;
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update_in(cx,|app,_,cx|{
            let executable=app.controls_home.parent().unwrap().join("claude-unified-fixture.py");
            std::fs::write(&executable,r#"#!/usr/bin/python3
import json,os,sys
def emit(value): print(json.dumps(value),flush=True)
v=json.loads(sys.stdin.readline())
emit({'type':'control_response','response':{'subtype':'success','request_id':v['request_id'],'response':{'commands':[]}}})
v=json.loads(sys.stdin.readline())
with open('requests.jsonl','a') as f: f.write(json.dumps(v)+'\n')
emit({'type':'system','subtype':'init','session_id':'unified-native','slash_commands':[]})
emit({'type':'assistant','message':{'id':'reply','content':[{'type':'text','text':'Native fixture reply'}]}})
emit({'type':'result','subtype':'success','is_error':False,'result':'Native fixture reply'})
"#).unwrap();std::fs::set_permissions(&executable,std::fs::Permissions::from_mode(0o700)).unwrap();
            app.active=None;app.loading=false;app.agent=AgentKind::Claude;app.agents=vec![Agent{kind:AgentKind::Claude,name:"Fixture".into(),executable:Some(executable),available:true}];
            app.composer.update(cx,|input,cx|input.set_text("",cx));
        });
        for prompt in ["First notch prompt","Second notch prompt"]{
            view.update_in(cx,|app,window,cx|{let mut body=request(app,"send",cx);body["text"]=json!(prompt);app.unified_command(&body,window,cx).unwrap();});
            let deadline=Instant::now()+Duration::from_secs(15);
            loop{
                cx.run_until_parked();
                view.update(cx,|app,cx|{while let Ok(event)=app.engine.events.try_recv(){if matches!(event,Event::Parsed(_,_)|Event::PagedFinished(_)|Event::FinishError(_,_)){app.receive(event,cx);}}});
                if cx.update(|_,cx|{let app=view.read(cx);!app.starting&&app.runs.is_empty()&&app.finalizing.is_empty()}){break}
                assert!(Instant::now()<deadline,"Native provider failed to finish");std::thread::sleep(Duration::from_millis(10));
            }
            view.update(cx,|app,cx|{assert!(!app.unified_successes.is_empty());assert!(app.unified_snapshot(cx)["history"].to_string().contains("Native fixture reply"));});
        }
        view.update(cx,|app,_|{let captured=std::fs::read_to_string(app.project.join("requests.jsonl")).unwrap();assert_eq!(captured.lines().count(),2);assert!(captured.contains("First notch prompt"));assert!(captured.contains("Second notch prompt"));});
    }
}


pub(super) fn workspace_frame(motion:bool)->workspace_motion::Frame{
    HOST.with(|host|host.borrow().as_ref().map_or_else(workspace_motion::Frame::identity,|host|host.workspace_motion.frame(motion&&!native::reduced_motion())))
}
pub(super) fn quick_visibility(visible:bool){if active(){emit(&json!({"type":"quickVisibility","visible":visible}));}}
pub(super) fn dismiss(){emit(&json!({"type":"workspaceDismiss"}));}

#[cfg(test)]mod shortcut_start_tests{
 use super::*;
 use std::prelude::v1::test;
 use gpui::TestAppContext;
 #[gpui::test]fn refused_voice_start_is_an_error_acknowledgement(cx:&mut TestAppContext){
  let(view,cx)=crate::ui_tests::fixture(cx);
  view.update_in(cx,|app,window,cx|{
   app.active=None;app.loading=false;app.agent=AgentKind::Codex;
   let request=json!({"action":"workspace_voice_prompt","ownerToken":app.unified_token(),"requestId":"voice"});
   let error=app.unified_command(&request,window,cx).unwrap_err();
   assert!(error.contains("Install Codex"));assert!(app.voice_call.is_none());
  });
 }
}

pub(super) fn voice_host()->Option<*mut c_void>{
    HOST.with(|host|host.borrow().as_ref().and_then(|h|{let view=unsafe{(h.voice_host)()};(!view.is_null()).then_some(view)}))
}

impl Orbit{
    pub(super) fn model_project(&self)->PathBuf{
        self.active.as_ref().and_then(|id|self.sessions.iter().find(|s|s.id==*id)).map(|s|s.execution_path.as_ref().unwrap_or(&s.project_path).clone()).filter(|p|p.is_dir()).unwrap_or_else(||if self.project.is_dir(){self.project.clone()}else{self.controls_home.clone()})
    }
    pub(super) fn effective_model_settings(&self)->(Option<String>,Option<String>){
        let controls=self.controls.get(self.agent.command()).map(|(_,v)|&v.defaults);
        let run=self.active.as_ref().and_then(|id|self.runs.get(id)).filter(|r|r.agent==self.agent);
        let usage=self.active.as_ref().filter(|id|self.sessions.iter().any(|s|s.id==**id&&s.agent==self.agent)).and_then(|id|self.usage.get(id));
        let alias=|id:&str|controls.and_then(|d|d.aliases.get(id)).cloned().unwrap_or_else(||id.into());
        let selected=self.model.as_deref().map(alias);
        let configured=controls.and_then(|d|d.model.as_deref()).map(alias);
        let reported=usage.and_then(|u|u.model.as_deref()).map(alias);
        let model=if run.is_some(){reported.clone().or_else(||run.and_then(|r|r.retry.as_ref()).and_then(|r|r.model.as_deref()).map(alias)).or(selected).or(configured)}else{selected.or(reported).or(configured)};
        let reported_matches=usage.and_then(|u|u.model.as_deref()).map(alias)==model;
        let default_effort=controls.and_then(|d|d.effort.clone().or_else(||model.as_ref().and_then(|m|d.efforts.get(m)).cloned()));
        let effort=if run.is_some(){usage.filter(|_|reported_matches).and_then(|u|u.reasoning_effort.clone()).or_else(||run.and_then(|r|r.retry.as_ref()).and_then(|r|r.reasoning_effort.clone())).or(default_effort)}else{self.unified_effort().map(str::to_owned).or_else(||usage.filter(|_|reported_matches).and_then(|u|u.reasoning_effort.clone())).or(default_effort)};
        (model,effort)
    }
    pub(super) fn effective_model_label(&self)->String{
        let (model,effort)=self.effective_model_settings();
        let name=model.unwrap_or_else(||"…".into());
        effort.filter(|v|!v.is_empty()).map_or_else(||name.clone(),|effort|format!("{name} · {effort}"))
    }
}

#[cfg(test)]mod effective_model_tests{
 use super::*;use gpui::TestAppContext;use std::prelude::v1::test;
 #[gpui::test]fn default_labels_use_real_config_and_active_reports_without_pinning_the_model(cx:&mut TestAppContext){
  let(view,cx)=crate::ui_tests::fixture(cx);view.update(cx,|app,cx|{
   let mut defaults=agent_controls::ModelDefaults{model:Some("configured-model".into()),effort:Some("high".into()),..Default::default()};defaults.efforts.insert("explicit-model".into(),"low".into());
   app.controls.insert("codex",(Instant::now(),agent_controls::ControlInfo{defaults,auth:agent_controls::AuthState::Unknown,models:vec![],note:None,account:None,limits:vec![],catalog:None}));
   app.agent=AgentKind::Codex;app.active=Some("fixture-session".into());app.model=None;app.runs.clear();app.usage.clear();assert_eq!(app.effective_model_settings(),(Some("configured-model".into()),Some("high".into())));assert_eq!(app.model,None);
   app.model=Some("explicit-model".into());assert_eq!(app.effective_model_settings(),(Some("explicit-model".into()),Some("high".into())));
   app.controls.get_mut("codex").unwrap().1.defaults.effort=None;assert_eq!(app.effective_model_settings().1.as_deref(),Some("low"));
   app.usage.insert("fixture-session".into(),usage::Usage{model:Some("native-rerouted-model".into()),reasoning_effort:Some("medium".into()),..Default::default()});app.runs.insert("fixture-session".into(),crate::ui_tests::idle_run("Thinking".into()));
   let snapshot=app.unified_snapshot(cx);assert_eq!(snapshot["effectiveModel"],"native-rerouted-model");assert_eq!(snapshot["effectiveReasoningEffort"],"medium");assert_eq!(snapshot["readOnly"],false);assert_eq!(snapshot["composerBusy"],true);
   app.agent=AgentKind::Claude;app.model=None;assert_ne!(app.effective_model_settings().0.as_deref(),Some("native-rerouted-model"));
  });
 }
}

impl Orbit{
    pub(super) fn effort_model_choice(&self)->Option<agent_controls::ModelChoice>{
        let defaults=self.controls.get(self.agent.command()).map(|(_,info)|&info.defaults);
        let resolved=self.effective_model_settings().0;
        let id=self.model.as_deref().or_else(||defaults.and_then(|d|d.model.as_deref())).or(resolved.as_deref())?;
        self.unified_models().into_iter().find(|m|m.id==id || defaults.and_then(|d|d.aliases.get(&m.id)).is_some_and(|resolved|resolved==id))
    }
}


/// Tests the complete packaged owner -> visible Kai host -> real native media
/// and Codex path. The fixture track is muted; no ambient speech is transmitted.
fn start_voice_probe(window:WindowHandle<Orbit>,cx:&mut App)->anyhow::Result<()>{
    let path=std::path::PathBuf::from(std::env::var("KAI_UNIFIED_VOICE_FIXTURE")?);
    anyhow::ensure!(std::fs::metadata(&path)?.len()<2*1024*1024,"Voice fixture exceeded bounds");
    let fixture:Value=serde_json::from_slice(&std::fs::read(&path)?)?;
    let project=path.parent().ok_or_else(||anyhow::anyhow!("Missing fixture directory"))?.to_owned();
    cx.spawn(async move|cx|{
        Timer::after(Duration::from_millis(800)).await;
        window.update(cx,|app,window,cx|{
            assert!(app.runs.is_empty(),"Isolated voice probe unexpectedly contained user runs");
            app.project=project;app.agent=AgentKind::Codex;app.model=None;app.active=None;app.loading=false;app.new_execution_path=None;
            let mut body=fixture;body["action"]=json!("workspace_voice_prompt");body["ownerToken"]=json!(app.unified_token());body["requestId"]=json!("native-voice-probe");
            app.unified_command(&body,window,cx).expect("Start real packaged voice");cx.notify();
        }).expect("Voice probe window");
        let mut complete=false;
        for _ in 0..240{
            Timer::after(Duration::from_millis(250)).await;
            let ready=window.update(cx,|app,_,cx|{
                if let Some(error)=&app.voice_error{panic!("Packaged voice failed: {error}")}
                let snapshot=app.unified_snapshot(cx);
                let user=app.rows.iter().any(|r|matches!(&r.block,MessageBlock::Interaction(chat_interactions::ChatInteraction::Speaker{speaker:chat_interactions::Speaker::User,text}) if !text.is_empty()));
                let assistant=app.rows.iter().any(|r|matches!(&r.block,MessageBlock::Interaction(chat_interactions::ChatInteraction::Speaker{speaker:chat_interactions::Speaker::Assistant,text}) if !text.is_empty()));
                let active=app.voice_call.as_ref().is_some_and(|call|call.active&&call.owns_current_chat(app.active.as_deref()));
                if active&&user&&assistant{
                    assert_eq!(snapshot["readOnly"],false);assert_eq!(snapshot["composerBusy"],true);
                    assert!(!snapshot["sessionActivity"]["text"].as_str().unwrap_or("").contains("Connecting"));
                    assert!(snapshot["effectiveModel"].as_str().is_some_and(|s|!s.is_empty()));assert!(snapshot["effectiveReasoningEffort"].as_str().is_some_and(|s|!s.is_empty()));assert!(app.model.is_none(),"Resolved defaults were incorrectly pinned");
                    app.stop_voice(cx);cx.notify();return true
                }false
            }).expect("Read packaged voice state");
            if ready{complete=true;break}
        }
        assert!(complete,"Packaged native voice did not produce both transcripts");
        Timer::after(Duration::from_secs(3)).await;
        window.update(cx,|app,_,_|assert!(app.voice_call.is_none(),"Packaged voice stop left its media alive")).expect("Check voice teardown");
        eprintln!("Kai packaged voice: real transcripts, live draft ownership, editable voice status, resolved model/effort, unpinned CLI defaults, and native stop passed");
        let _=cx.update(|cx|cx.quit());
    }).detach();Ok(())
}

impl Orbit{
    pub(super) fn provider_default_model(&self)->Option<String>{
        let defaults=&self.controls.get(self.agent.command())?.1.defaults;
        let id=defaults.model.as_ref()?;Some(defaults.aliases.get(id).unwrap_or(id).clone())
    }
}
