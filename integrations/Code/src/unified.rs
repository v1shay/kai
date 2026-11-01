//! In-process adapter: GPUI owns sessions, consent, persistence and every run.
//! Swift owns the notch's existing AppKit views. Commands wake an async receiver;
//! state is pushed only on owner notifications, with no polling or second server.
use super::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, ffi::c_void, sync::OnceLock};

type Publish = unsafe extern "C" fn(*const u8, usize);
type Collapse = unsafe extern "C" fn();
type Wallpaper=unsafe extern "C" fn(*const u8,usize,usize,usize,f32);
type QuickSurface=unsafe extern "C" fn(*mut c_void,bool)->bool;
type Start = unsafe extern "C" fn(*mut c_void, extern "C" fn(*const u8, usize) -> bool);
static COMMANDS: OnceLock<async_channel::Sender<Value>> = OnceLock::new();
thread_local! {static HOST:RefCell<Option<Host>>=const{RefCell::new(None)};}
struct Host {
    _library: *mut c_void,
    publish: Publish,
    levels: unsafe extern "C" fn(*const f32, usize),
    voice_host:unsafe extern "C" fn()->*mut c_void,
    wallpaper:Wallpaper,
    quick_surface:QuickSurface,
    wallpaper_visible:bool,
    last_wallpaper:Option<(ImageId,u32)>,
    collapse: Collapse,
    _toggle: Collapse,
    last: std::sync::Arc<[u8]>,
    history:Value,
    history_owner:String,
    completed: Option<String>,
    error: Option<String>,
    results: std::collections::VecDeque<(String, [u8; 32], Option<String>)>,
    reported_successes: std::collections::VecDeque<(String,String)>,
    finished: Vec<Value>,
    pet_rect:Option<[f32;4]>,
    quick_pet_rect:Option<[f32;4]>,
    workspace_motion:workspace_motion::Transition,
    publish_pending:bool,
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
    let callback=HOST.with(|host|host.borrow().as_ref().map(|h|h.collapse));
    if let Some(callback)=callback{unsafe{callback()}}
}
pub(super) fn quick_surface(window:*mut c_void,visible:bool)->bool{
    let callback=HOST.with(|host|host.borrow().as_ref().map(|h|h.quick_surface));
    callback.is_some_and(|callback|unsafe{callback(window,visible)})
}
pub(super) fn reveal() {
    emit(&json!({"type":"workspaceReveal"}));
}
fn wallpaper_visible()->bool{HOST.with(|host|host.borrow().as_ref().is_some_and(|h|h.wallpaper_visible))}
fn publish_wallpaper(background:&background::ChatBackground){
    let surface=background.native_surface();let key=surface.as_ref().map(|(_,opacity,id)|(*id,opacity.to_bits()));
    let callback=HOST.with(|host|{let mut host=host.borrow_mut();let h=host.as_mut()?;if !h.wallpaper_visible || h.last_wallpaper==key{return None}h.last_wallpaper=key;Some(h.wallpaper)});
    if let Some(callback)=callback{if let Some((pixels,opacity,_))=surface{unsafe{callback(pixels.as_ptr(),pixels.len(),pixels.width()as usize,pixels.height()as usize,opacity)}}else{unsafe{callback(std::ptr::null(),0,0,0,0.)}}}
}
pub(super) fn emit(value: &Value) {
    let Ok(data) = serde_json::to_vec(value) else {
        return;
    };
    if data.len() > 2 * 1024 * 1024 {
        return;
    }
    let callback=HOST.with(|host|host.borrow().as_ref().map(|h|h.publish));
    if let Some(callback)=callback{unsafe{callback(data.as_ptr(),data.len())}}
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
        let wallpaper=unsafe{libc::dlsym(library,c"kai_unified_wallpaper".as_ptr())};
        let quick_surface=unsafe{libc::dlsym(library,c"kai_unified_quick_surface".as_ptr())};
        anyhow::ensure!(
            !start.is_null()
                && !publish.is_null()
                && !collapse.is_null()
                && !toggle.is_null()
                && !ready.is_null()
                && !levels.is_null() && !voice_host.is_null() && !wallpaper.is_null() && !quick_surface.is_null(),
            "Incompatible Kai library"
        );
        let levels: unsafe extern "C" fn(*const f32, usize) =
            unsafe { std::mem::transmute(levels) };
        let voice_host:unsafe extern "C" fn()->*mut c_void=unsafe{std::mem::transmute(voice_host)};
        let wallpaper:Wallpaper=unsafe{std::mem::transmute(wallpaper)};
        let quick_surface:QuickSurface=unsafe{std::mem::transmute(quick_surface)};
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
                wallpaper,quick_surface,wallpaper_visible:false,last_wallpaper:None,
                collapse,
                _toggle:toggle,
                last: Vec::new().into(),
                history:Value::Null,
                history_owner:String::new(),
                completed: None,
                error: None,
                results: Default::default(),
                reported_successes: Default::default(),
                finished: vec![],
                pet_rect:None,quick_pet_rect:None,
                workspace_motion:Default::default(),publish_pending:false,
            })
        });
        let entity = window.update(cx, |app, window, cx| {
            let RawWindowHandle::AppKit(handle) =
                HasWindowHandle::window_handle(window).unwrap().as_raw()
            else {
                unreachable!()
            };
            let native: *mut AnyObject =
                unsafe { msg_send![handle.ns_view.as_ptr().cast::<AnyObject>(), window] };
            unsafe { start(native.cast(), command) };
            app.tick_adaptive(window,false,cx);
            cx.entity()
        })?;
        // Entity notifications include background run, approval and transcript events.
        cx.observe(&entity, move |_, cx| {
            // Chat progress belongs to the owner, including while both GPUI
            // windows are hidden behind the native notch. Never wait for paint
            // or the next keystroke to advance a queued send or handoff.
            let _=window.update(cx,|app,window,cx|{app.advance_shared_chat(window,cx);app.sync_flow_layout();app.refresh_unified_models(cx);let empty=app.active.is_none();let visible=wallpaper_visible()&&app.motion;app.background.update(cx,|background,cx|background.set_native_context(empty,visible,cx));});
            queue_publish(window,cx);
        })
        .detach();
        let background=entity.read(cx).background.clone();
        cx.observe(&background,|background,cx|publish_wallpaper(background.read(cx))).detach();
        entity.read(cx).publish_unified(cx);
        launch_metrics::mark("native-owner-ready");
        anyhow::ensure!(
            unsafe { ready() },
            "Kai did not accept the initial owner state"
        );
        eprintln!(
            "Kai unified: native layer and owner state ready in process {}",
            std::process::id()
        );
        if std::env::args().any(|arg|matches!(arg.as_str(),"--unified-latency-probe"|"--unified-background-idle-probe")){
            anyhow::ensure!(std::env::args().any(|arg|arg=="--unified-smoke-test"),"Performance probes require isolated data");
        }else if std::env::args().any(|arg|arg=="--unified-menu-bar-probe"){
            anyhow::ensure!(std::env::args().any(|arg|arg=="--unified-smoke-test"),"Menu probe requires isolated app data");
            let diagnostics=unsafe{libc::dlsym(library,c"kai_unified_diagnostics".as_ptr())};anyhow::ensure!(!diagnostics.is_null(),"No native diagnostics");let diagnostics:unsafe extern "C" fn()->u32=unsafe{std::mem::transmute(diagnostics)};
            reveal();
            cx.spawn(async move|cx|{
                Timer::after(Duration::from_millis(600)).await;
                for _ in 0..3 {
                    let _=cx.update(|_|emit(&json!({"type":"probeMenuBarClick"})));
                    Timer::after(Duration::from_millis(700)).await;
                    assert_ne!(unsafe{diagnostics()}&128,0,"The actual status-item handler did not open mini Code");
                    assert_eq!(unsafe{diagnostics()}&1,0,"The workspace remained live beside menu mini Code");
                    let _=cx.update(|_|emit(&json!({"type":"probeMenuBarClick"})));
                    Timer::after(Duration::from_millis(700)).await;
                    assert_eq!(unsafe{diagnostics()}&128,0,"A repeated status-item click did not close mini Code");
                }
                eprintln!("KAI_MENU_BAR_PROBE_OK");let _=cx.update(|cx|cx.quit());
            }).detach();
        }else if std::env::args().any(|arg|arg=="--unified-startup-probe"){
            anyhow::ensure!(std::env::args().any(|arg|arg=="--unified-smoke-test"),"Startup probe requires isolated data");
            let diagnostics=unsafe{libc::dlsym(library,c"kai_unified_diagnostics".as_ptr())};anyhow::ensure!(!diagnostics.is_null(),"No native diagnostics");let diagnostics:unsafe extern "C" fn()->u32=unsafe{std::mem::transmute(diagnostics)};
            let surface=entity.read(cx).startup_surface;
            cx.spawn(async move|cx|{Timer::after(Duration::from_millis(std::env::var("KAI_STARTUP_WAIT_MS").ok().and_then(|s|s.parse::<u64>().ok()).unwrap_or(1400).clamp(1400,60000))).await;let state=unsafe{diagnostics()};match surface{
                startup::Surface::Workspace=>{assert_eq!(state&31,9,"Default startup must show only the workspace and its shared pet");assert_ne!(state&64,0,"Startup workspace must fill the working area");},
                startup::Surface::Extended=>{assert_eq!(state&3,2);assert_ne!(state&512,0,"Extended startup must really expand the notch");},
                startup::Surface::Compact=>{assert_eq!(state&3,2);assert_eq!(state&512,0,"Compact startup must not show the mini chat");}
            }eprintln!("KAI_STARTUP_PROBE_OK {surface:?}");let _=cx.update(|cx|cx.quit());}).detach();
        }else if std::env::args().any(|arg|arg=="--unified-voice-probe"){
            anyhow::ensure!(std::env::args().any(|arg|arg=="--unified-smoke-test") && std::env::var("KAI_CODE_VOICE_DIAGNOSTIC").as_deref()==Ok("1"),"Live voice probe requires isolated, generated-audio diagnostic mode");
            start_voice_probe(window,cx)?;
        }else if std::env::args().any(|arg|arg=="--unified-orchestrator-probe"){
            anyhow::ensure!(std::env::args().any(|arg|arg=="--unified-smoke-test"),"Orchestrator probe requires isolated app data");
            #[cfg(not(test))]
            crate::orchestrator::start_probe(window,cx)?;
        }else if std::env::args().any(|arg|arg=="--unified-agent-probe"){
            anyhow::ensure!(std::env::args().any(|arg|arg=="--unified-smoke-test"),"Agent probe requires isolated app data");
            start_agent_probe(window,cx)?;
        }else if std::env::args().any(|arg| arg == "--unified-smoke-test") {
            let diagnostics=unsafe{libc::dlsym(library,c"kai_unified_diagnostics".as_ptr())};
            anyhow::ensure!(!diagnostics.is_null(),"Missing presentation diagnostics");
            let diagnostics:unsafe extern "C" fn()->u32=unsafe{std::mem::transmute(diagnostics)};
            anyhow::ensure!(unsafe{diagnostics()} & 3 == 2,"App did not launch as notch only");
            #[cfg(not(test))]
            if std::env::args().any(|arg|arg=="--unified-glass-probe"){
                window.update(cx,|app,window,_|crate::native::verify_glass(window,&app.theme))?.map_err(anyhow::Error::msg)?;
            }
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
                    // A foreground repeat is a no-op. Without actual OS focus,
                    // repeat reveal legitimately replays the entrance; both
                    // must retain the live workspace/pet and exclude the notch.
                    crate::unified::reveal();assert_eq!(unsafe{diagnostics()} & 15,9);
                    crate::unified::collapse();
                });
                Timer::after(Duration::from_millis(850)).await;
                let _=cx.update(|_|{
                    assert_eq!(unsafe{diagnostics()} & 27,2,"Fold must restore the extended notch and pet");
                    if std::env::args().any(|arg|arg=="--unified-wallpaper-probe"){assert_ne!(unsafe{diagnostics()} & 256,0,"The extended notch did not receive Code's real prepared wallpaper");}
                    crate::unified::reveal();
                });
                Timer::after(Duration::from_millis(1000)).await;
                let _=cx.update(|_|emit(&json!({"type":"probeDoubleCommand"})));
                Timer::after(Duration::from_millis(950)).await;
                let _=cx.update(|cx|{
                    assert_eq!(unsafe{diagnostics()} & 3,0,"Double Command must dismiss the large workspace without reopening mini chat");
                    cx.update_global::<quick_chat::Controller,_>(|service,cx|service.toggle(cx).expect("Open native Quick Chat"));
                });
                Timer::after(Duration::from_millis(1000)).await;
                let _=cx.update(|cx|{
                    let state=unsafe{diagnostics()};assert_ne!(state & 128,0,"Quick Chat was not opened");
                    assert_eq!(state & 3,0,"Quick Chat must not also open the notch or workspace");
                    cx.update_global::<quick_chat::Controller,_>(|service,cx|service.dismiss(cx));
                });
                Timer::after(Duration::from_millis(1000)).await;
                let _=cx.update(|cx|{
                    assert_eq!(unsafe{diagnostics()} & 131,0,"Quick Chat dismissal must retire only its own surface");
                    eprintln!("Kai unified smoke: continuous morph reversal, maximized working area, shared pet, repeat open, extended-notch return, double-Command dismissal, and exclusive native Quick Chat passed");cx.quit();
                });
            }).detach();
        }
        emit(&json!({"type":"keyboardCatalog","commands":keyboard::catalog()}));
        if !std::env::args().any(|arg|arg=="--unified-smoke-test") || std::env::args().any(|arg|matches!(arg.as_str(),"--unified-startup-probe"|"--unified-latency-probe"|"--unified-background-idle-probe")) {
            let surface=entity.read(cx).startup_surface;
            // The native owner already has a state packet. Present immediately;
            // discovery and provider catalogs continue independently.
            cx.defer(move|_|{emit(&json!({"type":surface.packet()}));launch_metrics::mark(surface.packet());});
        }
        cx.spawn(async move |cx| {
            while let Ok(request) = receiver.recv().await {
                if cx
                    .update(|cx| {
                        if request["action"] == "quick_dismiss"{
                            let issued=request["issuedAt"].as_f64().filter(|v|v.is_finite());
                            if cx.has_global::<quick_chat::Controller>(){cx.update_global::<quick_chat::Controller,_>(|service,cx|service.dismiss_at(issued,cx));}
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

// Progress bursts share one publication per display interval. Command replies
// still publish synchronously below, so request acknowledgements cannot vanish.
#[cfg(target_os="macos")]
fn queue_publish(window:WindowHandle<Orbit>,cx:&mut App){
    let queued=HOST.with(|host|{let mut host=host.borrow_mut();let Some(host)=host.as_mut()else{return false};if host.publish_pending{return false}host.publish_pending=true;true});
    if !queued{return}
    cx.spawn(async move|cx|{Timer::after(Duration::from_millis(16)).await;let _=cx.update(|cx|{
        HOST.with(|host|{if let Some(host)=host.borrow_mut().as_mut(){host.publish_pending=false;}});
        let _=window.update(cx,|app,_,cx|app.publish_unified(cx));
    });}).detach();
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
    pub(super) fn effort_key(&self)->String{format!("{}:{}:{}",self.agent.command(),self.active.as_deref().unwrap_or("draft"),self.model.as_deref().unwrap_or(""))}
    pub(super) fn unified_effort(&self)->Option<&str>{if self.agent==AgentKind::Ninja{return self.active.as_ref().and_then(|id|self.ninja.chats.get(id)).unwrap_or(&self.ninja.draft).thinking.as_deref()}self.reasoning_efforts.get(&self.effort_key()).or_else(||self.active.is_none().then(||self.reasoning_efforts.get(&format!("{}:{}",self.agent.command(),self.model.as_deref().unwrap_or("")))).flatten()).map(String::as_str)}
    pub(super) fn unified_token(&self) -> String {
        // Composer edits have their own exact draft-base check. They must not
        // invalidate navigation/model commands before the parent observes input.
        if let Some(owner)=self.ninja_surface_owner(){return owner;}
        format!("{}:{}:{}:{}",self.active.clone().unwrap_or_else(||format!("draft-{}",self.generation)),self.project.display(),self.generation,self.agent.command())
    }

    pub(super) fn unified_send_ready(&self) -> bool {
        !self.loading && !self.loading_earlier && !self.quit_pending
            && self.attachment_pending==0 && self.handoff_pending.is_none()
            && self.workspace_trust.is_none() && self.session_conflict.is_none()
    }
    fn dispatch_unified(&mut self, request: Value, window: &mut Window, cx: &mut Context<Self>) {
        // Presentation updates must never replace the acknowledgement of a
        // prompt or decision that the native controls are still waiting for.
        if matches!(request["action"].as_str(),Some("pet_visuals"|"adaptive_text_samples")){
            if let Err(error)=self.unified_command(&request,window,cx){eprintln!("Kai pet presentation: {error}")}
            return;
        }
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
        if self.ninja_surface_command(request,window,cx)? { return Ok(()); }
        if self.ninja_remote_command(request,window,cx)? { return Ok(()); }
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
        let navigates = (action=="ui_command"&&request["command"].as_str().and_then(keyboard::tab_index).is_some_and(|index|self.open_tab_sessions().get(index).is_some_and(|session|self.active.as_ref()!=Some(&session.id)))) || (action=="ui_command"&&matches!(request["command"].as_str(),Some("new"|"close_tab"|"next_tab"|"previous_tab"|"next_project"|"previous_project"|"orchestrator"|"open_project"))) || matches!(
            action,
            "chat" | "project" | "new" | "new_project" | "orchestrator" | "workspace_expand" | "navigate"
        );
        // An approval carries its own owner/generation; it may belong to another chat.
        if !matches!(
            action,
            "approve"
                | "reject"
                | "approval_stop"
                | "native_answer"
                | "native_cancel"
                | "approval_next"
                | "refresh"
                | "workspace_settings"
                | "workspace_wallpaper_visibility"
                | "adaptive_text_samples"
                | "typography"
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
        if navigates || matches!(action,"send"|"composer_command"|"ui_command") {
            if request["draftBase"].as_str() != Some(self.composer.read(cx).text().as_str()) {
                return Err("The IDE has a different draft. Both drafts are preserved; review the workspace before continuing.".into());
            }
        }
        if action=="ui_command"&&!navigates{if let Some(draft)=request["draft"].as_str(){if draft.len()>input::MAX_DRAFT_BYTES||draft.contains('\0'){return Err("The draft is too large".into())}self.composer.update(cx,|input,cx|input.set_text(draft,cx));}}
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
            "pet_visuals"=>{
                let visuals:HashMap<String,crate::task_pets::Visual>=serde_json::from_value(request["pets"].clone()).map_err(|_|"Invalid pet visuals")?;
                if visuals.len()>128 || visuals.values().any(|v|!v.valid()){return Err("Invalid pet visual dimensions".into())}
                self.task_pet_visuals=visuals;cx.notify();
            }
            "typography"=>{
                let interface=request["interfaceFont"].as_str().ok_or("Missing interface font")?;
                let code=request["codeFont"].as_str().ok_or("Missing code font")?;
                let fonts=cx.text_system().all_font_names();
                let resolve=|name:&str,default:&str|->Result<String,String>{
                    if name == "System" || name == theme::UI_FONT_DEFAULT{return Ok(default.into())}
                    fonts.iter().find(|f|f.eq_ignore_ascii_case(name)).cloned().ok_or_else(||"Choose an installed font family".into())
                };
                let interface=resolve(interface,theme::UI_FONT_DEFAULT)?;
                let code=resolve(code,theme::MONO_FONT_DEFAULT)?;
                self.theme.ui_font=interface;self.theme.mono_font=code;self.apply_personalization(window,cx);self.queue_workspace_save(cx);
            }
            "composer_command"=>{
                let name=request["name"].as_str().ok_or("Missing command")?;
                let draft=request["draft"].as_str().filter(|v|v.len()<=input::MAX_DRAFT_BYTES && !v.contains('\0')).ok_or("Invalid draft")?;
                let caret=request["caret"].as_u64().filter(|n|*n<=draft.encode_utf16().count() as u64).ok_or("Invalid caret")? as usize;
                self.composer.update(cx,|input,cx|input.set_text(draft,cx));
                if !self.composer.update(cx,|input,cx|input.set_command_caret_utf16(caret,cx)){return Err("Invalid caret".into());}
                self.accept_named_slash(name,window,cx)?;
                if request["pickerSurface"]=="notch" && matches!(name,"agent"|"model") && self.agent_picker.is_some(){self.close_picker_with_focus(window,cx);self.agent_picker=None;}
                else if self.modal.is_some() || self.agent_picker.is_some() || self.github.panel.is_some() || self.files_visible{reveal();}
            }
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
            "adaptive_text_samples"=>{let profiles=request["profiles"].as_array().filter(|v|v.len()<=32).map(|v|v.iter().filter_map(|p|serde_json::from_value::<adaptive_text::Profile>(p.clone()).ok()).collect()).unwrap_or_default();self.accept_adaptive(profiles,request["status"].as_str().map(str::to_owned),cx);}
            "ui_command"=>{let id=request["command"].as_str().filter(|id|keyboard::COMMANDS.iter().any(|(_,known,_)|id==known)).ok_or("Unknown keyboard command")?;self.execute_shortcut(id,window,cx);if id=="github"{reveal();}},
            "new" => self.new_unprojected_conversation(window, cx),
            "new_project" => self.new_conversation(window, cx),
            "send" => {
                // Exactly the same submission path as both Code composers:
                // queue active work, and restore the live tail before sending
                // from history. A presentation snapshot is not a send lock.
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
                if !self.starting_current() && self.send_after_load.is_none() && self.handoff_pending.is_none() && self.composer.read(cx).text() == supplied {
                    return Err(self.notice.clone().unwrap_or_else(|| {
                        "Prompt staged in the workspace; review its native confirmation".into()
                    }));
                }
            }
            "interrupt" => {
                let id = native_id(request, "targetThreadId").ok_or("Select a running chat")?;
                if let Some(launch)=self.launches.get(&id){
                    if request["runId"].as_str()!=Some(launch.key.as_str()){return Err("The launch changed; refresh before stopping".into())}
                    launch.cancel.store(true,std::sync::atomic::Ordering::Release);return Ok(())
                }
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
            "external_urls"=>{if let Some(urls)=request["urls"].as_array(){daily_driver::open_urls(urls.iter().filter_map(Value::as_str).map(str::to_owned).collect());}}
            "child_open"=>{
                let id=request["id"].as_str().ok_or("Missing child")?;
                let child=self.unified_children.iter().find(|c|child_id(c)==id&&c.connected).cloned().ok_or("Child is no longer connected")?;
                if let Some(owner)=self.child_owner(&child,&self.unified_children).cloned(){if self.active.as_deref()!=Some(&owner.id){self.workspace_open(Some(owner),None,window,cx);}}
                if self.broker_panel.is_none(){self.toggle_children(window,cx)}
                self.select_child(child.provider,child.id,cx);reveal();
            }
            "approve" | "reject" | "approval_stop" | "native_answer" | "native_cancel" => {
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
                if self.loading||self.starting_current()||self.handoff_pending.is_some(){return Err("Finish the current handoff before changing the model".into())}
                self.choose_model((!model.is_empty()).then(||model.into()), cx);
                if self.agent==AgentKind::Ninja{if let Some(effort)=request["effort"].as_str().filter(|s|!s.is_empty()){self.ninja_set("thinking",effort,cx)?;}}
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
                if agent==AgentKind::Ninja{self.ninja.native_query.clear();self.ninja_refresh(false,cx);}
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
            "interaction_details"=>{
                let id=request["id"].as_str().ok_or("Missing interaction")?;
                if let Some(index)=id.strip_prefix("work-").and_then(|s|s.parse::<usize>().ok()){
                    if !self.transcript_flow().work.get(index).is_some_and(|r|r.as_ref().is_some_and(|r|r.start==index)){return Err("This turn changed".into())}
                    self.toggle_work(index,cx);return Ok(())
                }
                let index=id.split_once('-').and_then(|(n,_)|n.parse::<usize>().ok()).ok_or("Invalid interaction")?;
                if self.unified_detail_id(index).as_deref()!=Some(id){return Err("This interaction changed; select its current card".into())}
                if let Some(MessageBlock::Interaction(chat_interactions::ChatInteraction::Delegation{agent:Some(child),..}))=self.rows.get(index).map(|r|&r.block) && let Some(child)=self.unified_children.iter().find(|c|c.id==*child).cloned(){if self.broker_panel.is_none(){self.toggle_children(window,cx)}self.select_child(child.provider,child.id,cx);return Ok(())}
                if self.expanded_tools.insert(index){self.interaction_animations.insert(index,(Instant::now(),true,0.));}
                self.list.scroll_to_reveal_item(index);
            }
            "workspace_expand" => reveal(),
            "draft" => {
                let text=request["text"].as_str().ok_or("Missing draft")?;
                if text.len()>input::MAX_DRAFT_BYTES || text.contains('\0'){return Err("Draft is too large".into())}
                if request["draftBase"].as_str()!=Some(self.composer.read(cx).text().as_str()){return Err("The workspace draft changed; your notch draft is preserved".into())}
                if self.composer.read(cx).text()!=text{self.composer.update(cx,|input,cx|input.set_text(text,cx));self.queue_workspace_save(cx);}
            },
            "workspace_permissions" => {
                let access=request["access"].as_str();let approvals=request["approvals"].as_str();
                if access.is_some_and(|value|!["read-only","workspace","full"].contains(&value)){return Err("Unknown workspace access".into())}
                if approvals.is_some_and(|value|!["ask","auto-review","never"].contains(&value)){return Err("Unknown approval policy".into())}
                if !matches!(self.agent,AgentKind::Codex|AgentKind::Ninja) && (access==Some("full") || approvals.is_some()){return Err("These controls apply to Codex workspace access and approvals".into())}
                if let Some(access)=access{self.allow_edits=access!="read-only";if matches!(self.agent,AgentKind::Codex|AgentKind::Ninja){self.codex_access=if access=="full"{runtime::CodexAccess::Full}else{runtime::CodexAccess::Workspace};}}
                if let Some(policy)=approvals{self.codex_approvals=match policy{"ask"=>runtime::CodexApprovals::Ask,"auto-review"=>runtime::CodexApprovals::AutoReview,_=>runtime::CodexApprovals::Never};}
                self.queue_workspace_save(cx);
            },
            "workspace_wallpaper_visibility"=>{
                let visible=request["visible"].as_bool().ok_or("Missing wallpaper visibility")?;
                HOST.with(|host|{if let Some(host)=host.borrow_mut().as_mut(){host.wallpaper_visible=visible;}});
                let empty=self.active.is_none();let active=visible&&self.motion;
                self.background.update(cx,|background,cx|background.set_native_context(empty,active,cx));
                publish_wallpaper(self.background.read(cx));
            },
            "workspace_settings" => self.show_modal(Modal::Settings, window, cx),
            "workspace_voice" => self.toggle_voice(window, cx),
            "workspace_voice_prompt" => {
                if self.voice_call.is_some() || self.ninjas.call.is_some() {
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
                self.start_voice_prompt(prompt, window, cx);
                if self.voice_call.is_none() && self.ninjas.call.is_none(){return Err(self.notice.clone().unwrap_or_else(||"The native voice session could not start".into()))}
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
            "usage_reset"=>{self.consume_usage_reset(request["creditId"].as_str().ok_or("Missing reset credit")?,cx)?;}
            "orchestrator"=>self.toggle_orchestrator(window,cx),
            "fast_mode"=>{self.toggle_fast(cx);}
            "usage_refresh"=>{self.controls.remove(self.agent.command());self.refresh_unified_models(cx);}
            "ninja_select"=>{self.ninja_set(request["field"].as_str().ok_or("Missing option")?,request["value"].as_str().ok_or("Missing value")?,cx)?;}
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
            let callback=HOST.with(|host|host.borrow().as_ref().map(|h|h.levels));
            if let Some(callback)=callback{unsafe{callback(values.as_ptr(),values.len())}}
        }
        let mut state = self.unified_snapshot(cx);
        let delivery=HOST.with(|host| {
            if let Some(host) = host.borrow_mut().as_mut() {
                let finished:Vec<_>=self.unified_successes.iter().filter(|(id,epoch)|!host.reported_successes.iter().any(|(seen,run)|seen==id&&run==epoch) && !self.finalizing.contains(id) && !self.settling.contains_key(id)).cloned().collect();
                for event in &finished{if host.reported_successes.len()>=128{host.reported_successes.pop_front();}host.reported_successes.push_back(event.clone());}
                let finished:Vec<_>=finished.iter().map(|(id,run)|json!({"id":if id.starts_with("child:")||id.starts_with("ninja:"){id.clone()}else{format!("code:{id}")},"runId":run,"parentId":self.orchestrator_parent(id)})).collect();
                if !finished.is_empty() {
                    if std::env::args().any(|arg|arg=="--unified-orchestrator-probe"){eprintln!("KAI_ORCHESTRATOR_FINISHED {finished:?}");}
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
                    return None;
                };
                if data.as_slice() == host.last.as_ref() || data.len() > 2 * 1024 * 1024 {
                    return None;
                }
                host.last = data.into();
                return Some((host.publish,host.last.clone()));
            }
            None
        });
        // AppKit can synchronously reenter GPUI when state orders a window or
        // changes focus. No RefCell borrow may cross that native boundary. Arc
        // keeps the exact bytes alive even if reentry replaces the cached state.
        if let Some((callback,data))=delivery{unsafe{callback(data.as_ptr(),data.len())}}
    }
    pub(super) fn unified_snapshot(&self, cx: &App) -> Value {
        let running = self.active.as_ref().and_then(|id| self.runs.get(id));
        let mut tasks:Vec<_>=self.runs.iter().map(|(id,run)|{
            let name=self.sessions.iter().find(|s|s.id==*id).map(|s|s.title.as_str()).unwrap_or("Agent");
            json!({"id":format!("code:{id}"),"name":name,"runId":run.parse_epoch.to_string(),"scene":self.native_pending.values().find(|p|p.owner==*id&&p.current(&self.runs)).map(|p|p.indicator_scene()).unwrap_or(scene(&run.status)),"text":format!("{} · {}",run.agent.name(),run.status),"agent":run.agent.command(),"parentId":self.orchestrator_parent(id)})
        }).collect();
        for (id,launch) in &self.launches{
            if let Some(chat)=self.sessions.iter().find(|s|s.id==*id){tasks.push(json!({"id":format!("code:{id}"),"name":chat.title,"runId":launch.key,"scene":"thinking","text":format!("{} · {}",launch.caption,chat_timing::duration(launch.started.elapsed().as_millis() as u64)),"agent":chat.agent.command()}));}
        }
        for child in self.unified_children.iter().filter(|c|c.connected && (matches!(c.status,chat_interactions::InteractionStatus::Started|chat_interactions::InteractionStatus::Working)||!c.requests.is_empty())){
            let parent=self.child_owner(child,&self.unified_children).map(|s|format!("code:{}",s.id));
            if parent.is_none(){continue}
            tasks.push(json!({"id":child_id(child),"name":child.name.clone().unwrap_or_else(||format!("{} · {}",child.provider.name(),short(&child.id,10))),"runId":child.turn_id.clone().unwrap_or_else(||child.id.clone()),"scene":if child.requests.is_empty(){"thinking"}else{"permission"},"text":if child.preview.is_empty(){child.task.as_deref().or(child.scope.as_deref()).unwrap_or("")}else{&child.preview},"agent":child.provider.command(),"parentId":parent}));
        }
        tasks.extend(self.ninjas.active());
        let selected=self.active.as_ref().map(|id|format!("code:{id}"));
        tasks.sort_by_key(|t|{let urgent=matches!(t["scene"].as_str(),Some("permission"|"question"));let current=t["id"].as_str()==selected.as_deref()||t["parentId"].as_str()==selected.as_deref();(!urgent,!current,t["id"].as_str().unwrap_or("").to_owned())});
        let approval = self.unified_pending();
        let activity=approval.as_ref().map(|(owner,token,summary)|json!({"scene":"permission","text":summary,"owner":owner,"request":token})).or_else(||tasks.first().map(|t|json!({"scene":t["scene"],"text":t["text"]}))).unwrap_or(json!({"scene":"off","text":"Ready"}));
        let terminal = self
            .indicator_terminal
            .as_ref()
            .filter(|(id, _)| self.active.as_ref() == Some(id))
            .map(|(_, scene)| *scene);
        let selected_activity = running
            .map(|run| json!({"scene":scene(&run.status),"text":run.status}))
            .unwrap_or_else(|| if self.starting_current(){json!({"scene":"thinking","text":self.launch_caption()})}else{match terminal {
                Some("failure") => {
                    json!({"scene":"failure","text":"Agent failed — review the workspace"})
                }
                Some("declined") => json!({"scene":"declined","text":"Stopped"}),
                _ => json!({"scene":"off","text":"Ready"}),
            }});
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
        let models=if self.agent==AgentKind::Ninja{self.ninja_snapshot_choices()}else{self.unified_models()};
        let (effective_model,effective_effort)=self.effective_model_settings();
        let history=if self.loading{vec![]}else{self.unified_history()};
        let local_older=history.first().and_then(|v|v["id"].as_str()).is_some_and(|v|v!="row-0");
        let mut snapshot=json!({"project":self.project,"projects":self.projects.iter().take(80).map(|p|json!({"path":p.path,"name":p.name})).collect::<Vec<_>>(),"projectsLoading":false,
            "chats":chats,"standaloneChats":standalone,"standalone":self.project==self.unprojected_root(),"codeChats":[],"codeConnection":{"connected":true,"enabled":true},
            "threadId":selected,"threadName":chat.map(|s|s.title.as_str()).unwrap_or("New chat"),"ownerToken":self.unified_token(),"composerDraft":self.composer.read(cx).text(),
            "historyLoading":self.loading || self.loading_earlier,"history":history,"historyHasMore":self.has_older || local_older,"files":self.unified_file_rows(cx),
            "readOnly":self.history_offset>0 || self.navigation_history,"composerBusy":!self.unified_send_ready(),"activeTurnId":running.map(|r|r.parse_epoch.to_string()).or_else(||self.active.as_ref().and_then(|id|self.launches.get(id)).map(|l|l.key.clone())),
            "nativeReview":self.unified_review(),"approval":approval.is_some(),"approvalThreadId":approval.as_ref().map(|a|format!("code:{}",a.0)),"approvalRequestId":approval.as_ref().map(|a|a.1.clone()),"approvalSummary":approval.as_ref().map(|a|a.2.clone()),
            "activity":if tasks.is_empty(){selected_activity.clone()}else{activity},"sessionActivity":self.voice_call.as_ref().filter(|v|v.owns_current_chat(self.active.as_deref())).map(|v|json!({"scene":v.scene(),"text":format!("{} · {}",v.caption(),voice::REALTIME_MODEL)})).unwrap_or(selected_activity),"activeTasks":tasks,"focusThreadId":approval.as_ref().map(|a|format!("code:{}",a.0)).or_else(||tasks.first().and_then(|t|t["id"].as_str().map(str::to_owned))),
            "notice":self.notice.as_deref().unwrap_or("Kai · Code"),"attachments":self.attachments.iter().filter(|a|!self.attachment_removals.contains_key(&a.id)).map(|a|a.path.to_string_lossy().into_owned()).collect::<Vec<_>>(),"modelsLoading":self.controls_pending.contains(self.agent.command()),"models":models.iter().map(|m|json!({"model":m.id,"displayName":m.label,"resolvedModel":self.controls.get(self.agent.command()).and_then(|(_,v)|v.defaults.aliases.get(&m.id)).unwrap_or(&m.id),"supportedReasoningEfforts":m.reasoning_efforts.iter().map(|v|json!({"reasoningEffort":v})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "selectedModel":self.model,"reasoningEffort":self.unified_effort().unwrap_or(""),"canGoBack":!self.unified_back.is_empty(),"canGoForward":!self.unified_forward.is_empty(),
            "agents":self.agents.iter().map(|a|json!({"id":a.kind.command(),"name":a.kind.name(),"available":a.available&&a.executable.is_some()})).collect::<Vec<_>>(),"selectedAgent":self.agent.command(),"workspaceVoiceChoice":self.voice_choice,"workspaceVoiceActive":self.voice_call.is_some()||self.ninjas.call.is_some(),"workspaceVoiceScene":self.ninjas.call.as_ref().map(|v|v.scene()).or_else(||self.voice_call.as_ref().map(|v|v.scene()))});
        if approval.is_none(){let review=self.ninjas.review();if !review.is_null(){
            snapshot["approval"]=json!(true);snapshot["approvalThreadId"]=review["owner"].clone();snapshot["approvalRequestId"]=review["token"].clone();snapshot["approvalSummary"]=review["title"].clone();snapshot["focusThreadId"]=review["owner"].clone();snapshot["nativeReview"]=review;
        }}
        snapshot["menuBarNinjas"]=json!(self.ninjas.menu_identities());snapshot["menuBarNinja"]=self.ninjas.current_menu_identity();
        snapshot["slashCommands"]=self.unified_slash_commands();
        snapshot["interfaceFont"]=json!(self.theme.ui_font);snapshot["codeFont"]=json!(self.theme.mono_font);
        snapshot["borderlessComposer"]=json!(self.theme.borderless_composer);
        snapshot["horizontalAgentPicker"]=json!(self.theme.horizontal_agent_picker);
        snapshot["workspaceAccess"]=json!(if !self.allow_edits{"read-only"}else if !matches!(self.agent,AgentKind::Codex|AgentKind::Ninja){"native"}else if self.codex_access==runtime::CodexAccess::Full{"full"}else{"workspace"});
        snapshot["workspaceApprovalPolicy"]=json!(match self.codex_approvals{runtime::CodexApprovals::Ask=>"ask",runtime::CodexApprovals::AutoReview=>"auto-review",runtime::CodexApprovals::Never=>"never"});
        snapshot["workspaceExecutionPath"]=json!(self.execution_root().unwrap_or_else(||self.project.clone()));
        snapshot["providerDefaultModel"]=json!(self.provider_default_model());snapshot["effectiveModel"]=json!(effective_model);snapshot["effectiveReasoningEffort"]=json!(effective_effort);snapshot["voiceModel"]=json!(self.voice_call.as_ref().map(|_|voice::REALTIME_MODEL));
        if let Some(elapsed)=self.elapsed_label(){if let Some(text)=snapshot["sessionActivity"]["text"].as_str().map(str::to_owned){snapshot["sessionActivity"]["text"]=json!(format!("{text} · {elapsed}"));}}
        snapshot["usageResets"]=json!(self.controls.get(self.agent.command()).and_then(|(_,c)|c.defaults.reset_credits.as_ref()));
        snapshot["orchestratorMode"]=json!(self.orchestrator_active());snapshot["orchestratorAvailable"]=json!(true);
        snapshot["contextWindow"]=json!(self.context_usage().map(|(used,limit)|json!({"used":used,"limit":limit,"fraction":(used as f64/limit as f64).clamp(0.,1.)})));
        if self.agent==AgentKind::Ninja{let selection=self.ninja_selection();snapshot["selectedModel"]=json!(selection.model);snapshot["ninja"]=json!({"selection":selection,"machines":self.ninja.machines,"progress":self.ninja.progress,"error":self.ninja.error,"loading":!self.ninja.pending.is_empty(),"canCancel":self.ninja.download_cancel.is_some()});}
        snapshot["fastMode"]=json!(self.fast_enabled());snapshot["fastSupported"]=json!(self.fast_supported());snapshot["usageSummary"]=json!(self.usage_summary());snapshot["usageWindows"]=json!(self.usage_windows().iter().map(|w|json!({"label":w.window_title().unwrap_or_else(||w.label.clone()),"remaining":w.remaining_text(),"fraction":((100.-w.used_percent)/100.).clamp(0.,1.),"reset":w.resets_at.and_then(|t|chrono::DateTime::from_timestamp(t,0)).map(|t|format!("Resets {}",t.with_timezone(&chrono::Local).format("%b %-d at %-I:%M %p")))})).collect::<Vec<_>>());snapshot["usageAccount"]=json!(self.controls.get(self.agent.command()).and_then(|(_,c)|c.account.clone()));snapshot["usageUpdated"]=json!(self.controls.get(self.agent.command()).map(|(at,_)|format!("Updated {}s ago",at.elapsed().as_secs())));
        snapshot["workspaceVoiceError"]=json!(self.voice_error);snapshot["historyEarlier"]=json!(self.unified_history_end.as_ref().is_some_and(|(owner,_)|owner==&self.unified_token()) || self.history_offset>0 || self.navigation_history);snapshot["approvalCount"]=json!(self.unified_pending_ids().len()+self.ninjas.snapshots.values().flat_map(|s|s["tasks"].as_array().into_iter().flatten()).map(|t|t["requests"].as_array().map_or(0,Vec::len)).sum::<usize>());snapshot["workspaceMotion"]=json!(self.motion);snapshot["workspaceTextMorph"]=json!(self.unified_text_morph);self.ninja_surface_snapshot(&mut snapshot,cx);snapshot

    }
    fn unified_detail_id(&self,index:usize)->Option<String>{
        use std::hash::{Hash,Hasher};
        let (title,details)=match &self.rows.get(index)?.block{
            MessageBlock::Activity{label,details}=>(label,details),
            MessageBlock::Interaction(chat_interactions::ChatInteraction::Status{title,details,..}|chat_interactions::ChatInteraction::Delegation{title,details,..})=>(title,details),
            _=>return None,
        };
        if details.is_empty(){return None}
        let mut hash=std::collections::hash_map::DefaultHasher::new();title.hash(&mut hash);details.hash(&mut hash);
        Some(format!("{index}-{:016x}",hash.finish()))
    }
    pub(super) fn unified_history(&self) -> Vec<Value> {
        let mut history = Vec::new();
        let flow=self.transcript_flow();
        let mut role = "assistant".to_owned();
        let end = self.unified_history_end.as_ref().filter(|(owner,_)|owner==&self.unified_token() && !self.unified_history_wait).map_or(self.rows.len(),|(_,end)|(*end).min(self.rows.len()));
        let start = flow.tail_start(end,100,&self.expanded_work);
        for row in self.rows.iter().take(start) {
            if let Some(r) = &row.role {
                role = r.clone()
            }
        }
        for (index, row) in self.rows.iter().enumerate().take(end).skip(start) {
            // Roles belong to source messages, including folded-away rows.
            // A final paragraph with no explicit role still inherits assistant.
            if let Some(r)=&row.role{role=r.clone()}
            if flow.hidden.get(index)==Some(&true){continue}
            if let Some(span)=flow.work.get(index).and_then(|r|r.as_ref()){
                let expanded=self.expanded_work.contains(&span.start);
                if index==span.start.max(start){history.push(json!({"id":format!("work-summary-{}",span.start),"kind":"activity","lines":[{"text":if expanded{"Hide work"}else{"Show work"},"style":"activity"}],"detailId":format!("work-{}",span.start),"speechText":""}));}
                if !expanded{continue}
            }
            if let Some(span)=flow.commands.get(index).and_then(|r|r.as_ref()){
                if span.start!=index{continue}
                let working=self.rows[span.clone()].iter().any(|r|chat_flow::command(&r.block).is_some_and(|(_,_,s)|matches!(s,chat_interactions::InteractionStatus::Started|chat_interactions::InteractionStatus::Working)));
                history.push(json!({"id":format!("row-{index}"),"kind":"activity","lines":[{"text":format!("{} · {}",if working{"Running commands"}else{"Ran commands"},span.len()),"style":"activity"}],"detailId":self.unified_detail_id(index),"model":self.row_timing(index).and_then(|t|t.model.as_deref()),"effort":self.row_timing(index).and_then(|t|t.effort.as_deref()),"agent":self.agent.command(),"speechText":""}));continue
            }
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
                    "voice",
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
                        ("activity", if matches!(title.as_str(),"Thinking"|"Plan"|"Agent plan"|"Completed"|"Stopped"|"Agent failed"){
                            let preview=presentation::card_detail_text(title,details.lines().find(|l|!l.trim().is_empty()).unwrap_or("")).0;
                            if preview.is_empty(){title.clone()}else{format!("{title} · {}",bounded(&preview,140))}
                        }else{title.clone()}, "activity")
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
            let child=match &row.block{MessageBlock::Interaction(chat_interactions::ChatInteraction::Delegation{agent:Some(id),..})=>self.unified_children.iter().find(|c|c.id==*id).map(child_id),_=>None};
            history.push(json!({"id":format!("row-{index}"),"kind":if kind.starts_with("user"){"user"}else if kind=="activity"{"activity"}else{"assistant"},"lines":[{"text":bounded(&text,4000),"style":style}],"detailId":self.unified_detail_id(index),"model":self.row_timing(index).and_then(|t|t.model.as_deref()),"effort":self.row_timing(index).and_then(|t|t.effort.as_deref()),"childId":child,"agent":role.split(':').nth(1).unwrap_or(self.agent.command()),"speechText":if kind.starts_with("user") || kind=="activity" || style=="code" {String::new()}else{bounded(&text,4000)}}));
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
        if std::env::args().any(|arg|arg=="--unified-startup-probe"){
            if let Ok(surface)=std::env::var("KAI_STARTUP_SURFACE"){
                if matches!(surface.as_str(),"workspace"|"extended"|"compact"){let _=std::fs::write(path.join("workspace.json"),serde_json::to_vec(&json!({"startup_surface":surface})).unwrap());}
            }
        }
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
    #[gpui::test]fn nonnavigating_native_keyboard_command_keeps_context_and_synchronizes_typed_draft(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);cx.update(|window,cx|view.update(cx,|app,cx|{
            app.unified_context=Some("Captured app context".into());
            let mut body=request(app,"ui_command",cx);body["command"]=json!("fold_work");body["draft"]=json!("The current native draft");
            app.unified_command(&body,window,cx).unwrap();assert_eq!(app.composer.read(cx).text(),"The current native draft");assert_eq!(app.unified_context.as_deref(),Some("Captured app context"));
            assert!(app.unified_command(&body,window,cx).is_err(),"A stale draft base must never overwrite newer input");
        }));
    }
    fn request(app: &Orbit, action: &str, cx: &App) -> Value {
        json!({"action":action,"ownerToken":app.unified_token(),"draftBase":app.composer.read(cx).text(),"targetThreadId":app.active.as_ref().map(|id|format!("code:{id}")),"requestId":uuid::Uuid::new_v4().to_string()})
    }
    #[gpui::test]
    fn unified_send_during_run_queues_the_captured_draft_without_cancelling(cx: &mut TestAppContext) {
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
            app.unified_command(&body, window, cx).unwrap();
            assert_eq!(app.composer.read(cx).text(), "");
            assert_eq!(app.prompt_queue.last().unwrap().text,"New prompt");
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
    #[ignore="Replay one selected saved reply through Code and export its actual native-notch packet"]
    fn existing_saved_reply_exports_compact_native_history(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update(cx,|app,cx|{
            let source=std::fs::read_to_string(std::env::var("KAI_SAVED_HISTORY_FIXTURE").expect("Select saved fixture")).unwrap();
            let store=crate::store::Store::open(std::path::Path::new(":memory:")).unwrap();
            let session=SessionSummary{id:"existing-saved-probe".into(),title:"Existing saved reply".into(),agent:AgentKind::Codex,project_path:app.project.clone(),execution_path:None,updated_at:0,external_id:None,transcript_path:None};
            store.upsert_session(&session).unwrap();store.save_message(&session.id,"assistant",&source).unwrap();app.active=Some(session.id.clone());app.sessions.push(session.clone());
            let mut cursor=crate::store::MessageCursor::new(false);let mut messages=vec![];
            while cursor.has_older(){let mut page=store.lazy_messages(&session.id,&mut cursor,200).unwrap();page.append(&mut messages);messages=page;}
            app.loading=false;app.rows=messages.into_iter().flat_map(|m|presentation::message_blocks(&m.role,&m.text).into_iter().enumerate().map(move|(i,block)|Row{role:(i==0).then(||m.role.clone()),block})).collect();app.list.reset(app.rows.len());
            let snapshot=app.unified_snapshot(cx);let history=snapshot["history"].as_array().unwrap();
            assert!(history.iter().any(|r|r["detailId"].is_string()));
            for row in history{for line in row["lines"].as_array().unwrap(){let text=line["text"].as_str().unwrap();assert!(!text.contains("\\u003c")&&!text.contains("\"orbit_interaction\"")&&!text.contains("diff --git"),"Saved tool source spilled into native notch");if row["kind"]=="activity"{assert!(text.chars().count()<200);}}}
            let path=std::env::var("KAI_SAVED_HISTORY_PACKET").expect("Select native packet output");std::fs::write(path,serde_json::to_vec(&snapshot).unwrap()).unwrap();
            eprintln!("Actual existing saved reply exported through Code into {} compact native-notch rows",history.len());
        });
    }
    #[gpui::test]
    fn saved_tool_status_is_compact_in_notch_and_opens_exact_full_card(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update_in(cx,|app,window,cx|{
            app.loading=false;app.composer.update(cx,|input,cx|input.set_text("Keep my draft",cx));
            app.rows=vec![Row{role:Some("assistant:codex".into()),block:MessageBlock::Interaction(chat_interactions::ChatInteraction::Status{title:"Changes".into(),details:"diff --git a/View.swift b/View.swift\n+let text = \"actual source\"\n".repeat(1000),status:chat_interactions::InteractionStatus::Completed})}];
            let snapshot=app.unified_snapshot(cx);let row=&snapshot["history"][0];assert_eq!(row["lines"][0]["text"],"Changes");assert!(!row.to_string().contains("diff --git"));
            let mut command=request(app,"interaction_details",cx);command["id"]=row["detailId"].clone();
            app.unified_command(&command,window,cx).unwrap();assert!(app.expanded_tools.contains(&0));assert_eq!(app.composer.read(cx).text(),"Keep my draft");
            app.rows[0].block=MessageBlock::Activity{label:"New tool".into(),details:"Different source".into()};assert!(app.unified_command(&command,window,cx).is_err());
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
    fn notch_and_code_queue_through_the_same_owner_while_running_or_finalizing(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update_in(cx,|app,window,cx|{
            app.loading=false;app.active=Some("fixture-session".into());
            for phase in ["running","finalizing","settling"]{
                app.runs.clear();app.finalizing.clear();app.settling.clear();
                match phase{
                    "running"=>{app.runs.insert("fixture-session".into(),crate::ui_tests::idle_run(String::new()));},
                    "finalizing"=>{app.finalizing.insert("fixture-session".into());},
                    _=>{let(_,receiver)=async_channel::bounded(1);app.settling.insert("fixture-session".into(),receiver);},
                }
                let prompt=format!("Notch follow-up during {phase}");
                let request=json!({"action":"send","ownerToken":app.unified_token(),"draftBase":app.composer.read(cx).text(),"text":prompt});
                assert_eq!(app.unified_snapshot(cx)["composerBusy"],false);
                app.unified_command(&request,window,cx).unwrap();
                assert_eq!(app.prompt_queue.last().unwrap().text,prompt);assert_eq!(app.composer.read(cx).text(),"");
                app.composer.update(cx,|input,cx|input.set_text(&format!("Code follow-up during {phase}"),cx));app.send(window,cx);
                assert_eq!(app.prompt_queue.last().unwrap().text,format!("Code follow-up during {phase}"));assert_eq!(app.composer.read(cx).text(),"");
            }
            assert_eq!(app.prompt_queue.len(),6);
        });
    }

    #[gpui::test]
    fn notch_history_send_restores_live_history_like_code_instead_of_refusing(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update_in(cx,|app,window,cx|{
            app.active=Some("fixture-session".into());app.loading=false;app.history_offset=2;app.navigation_history=true;
            app.agents=vec![Agent{kind:AgentKind::Codex,name:"Fixture".into(),available:true,executable:Some(PathBuf::from("/usr/bin/true"))}];
            let request=json!({"action":"send","ownerToken":app.unified_token(),"draftBase":app.composer.read(cx).text(),"text":"Continue this same chat"});
            assert_eq!(app.unified_snapshot(cx)["composerBusy"],false);
            app.unified_command(&request,window,cx).unwrap();
            assert!(app.loading);assert!(app.send_after_load.is_some());assert!(!app.navigation_history);assert_eq!(app.history_offset,0);
            assert_eq!(app.send_after_load.as_ref().unwrap().original_prompt,"Continue this same chat");
        });
    }

    #[gpui::test]
    fn native_notch_inline_command_uses_the_shared_picker_and_keeps_the_draft(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update_in(cx,|app,window,cx|{
            let draft="🦀 please /motion continue";
            let mut body=json!({"action":"composer_command","ownerToken":"stale","draftBase":app.composer.read(cx).text(),"draft":draft,"caret":"🦀 please /motion".encode_utf16().count(),"name":"motion"});
            assert!(app.unified_command(&body,window,cx).is_err());
            body["ownerToken"]=json!(app.unified_token());let before=app.motion;
            app.unified_command(&body,window,cx).unwrap();assert_ne!(app.motion,before);
            assert_eq!(app.composer.read(cx).text(),"🦀 please  continue");assert!(app.runs.is_empty());
            assert!(app.unified_command(&body,window,cx).is_err());assert_eq!(app.motion,!before);
        });
    }

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
    #[gpui::test]
    fn unified_cancel_question_answers_the_request_without_stopping_the_agent(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);let(handle,answers)=RunHandle::interactive_test_handle();
        view.update_in(cx,|app,window,cx|{
            let mut run=crate::ui_tests::idle_run("Question".into());run.handle=handle;
            let native=runtime::NativeRequest{id:"form".into(),method:"mcpServer/elicitation/request".into(),title:"Connected tool needs input".into(),detail:"Allow Gmail to update the draft?".into(),kind:runtime::NativeRequestKind::Questions{questions:vec![]}};
            app.native_pending.insert("form".into(),native_request_ui::LiveRequest::new("fixture-session",run.parse_epoch,native));app.runs.insert("fixture-session".into(),run);
            assert_eq!(app.unified_review()["kind"],"questions");
            let token=app.unified_pending().unwrap().1;
            let mut body=json!({"action":"native_cancel","threadId":"code:fixture-session","approvalRequestId":format!("stale:{token}")});
            assert!(app.unified_command(&body,window,cx).is_err());assert!(answers.try_recv().is_err());
            body["approvalRequestId"]=json!(token);app.unified_command(&body,window,cx).unwrap();
            assert!(matches!(answers.try_recv().unwrap(),runtime::NativeControl::Respond{id,answer:runtime::NativeAnswer::Cancel}if id=="form"));
            assert!(!app.runs["fixture-session"].handle.is_cancelled());
            assert!(app.unified_command(&body,window,cx).is_err());
        });
    }
}

impl Orbit {
    pub(super) fn unified_models(&self)->Vec<agent_controls::ModelChoice>{
        if self.agent==AgentKind::Ninja{return self.ninja_choices()}
        let mut models=self.native_models.get(&self.agent).cloned().unwrap_or_default();
        if let Some((_,info))=self.controls.get(self.agent.command()){
            for model in &info.models{if let Some(existing)=models.iter_mut().find(|m|m.id==model.id){if existing.reasoning_efforts.is_empty(){existing.reasoning_efforts=model.reasoning_efforts.clone();}}else{models.push(model.clone());}}
        }
        models
    }
    pub(super) fn refresh_unified_models(&mut self,cx:&mut Context<Self>){
        if self.agent==AgentKind::Ninja{self.ninja_refresh(false,cx);return}
        if self.controls_pending.contains(self.agent.command()){return}
        let project=self.model_project();
        if self.controls.get(self.agent.command()).is_some_and(|(at,info)|at.elapsed()<Duration::from_secs(60)&&info.defaults.project==project && (!matches!(self.agent,AgentKind::Gemini|AgentKind::Cursor|AgentKind::OpenCode|AgentKind::Grok) || (info.defaults.capabilities_checked && info.defaults.capabilities_model==self.model))){return}
        let kind=self.agent;
        let Some(path)=self.agents.iter().find(|a|a.kind==kind&&a.available).and_then(|a|a.executable.clone())else{return};
        self.controls_pending.insert(kind.command());let home=self.controls_home.clone();let selected=self.model.clone();
        cx.spawn(async move|this,cx|{
            let info=cx.background_executor().spawn(async move{agent_controls::inspect_model_project(kind,&path,&home,&project,selected.as_deref())}).await;
            let _=this.update(cx,|app,cx|{app.controls_pending.remove(kind.command());app.controls.insert(kind.command(),(Instant::now(),info));if kind==app.agent{app.refresh_unified_models(cx);}cx.notify();});
        }).detach();
    }
}

pub(super) fn decode_enabled(motion:bool)->bool{
    #[cfg(not(test))]{active()&&motion&&!native::reduced_motion()}
    #[cfg(test)]{let _=motion;false}
}
pub(super) fn atmosphere_slot(bounds:Bounds<Pixels>,quick:bool){
    thread_local!{static SLOTS:std::cell::RefCell<[Option<[f32;4]>;2]>=const{std::cell::RefCell::new([None,None])};}
    if !active(){return}let rect=[f32::from(bounds.origin.x),f32::from(bounds.origin.y),f32::from(bounds.size.width),f32::from(bounds.size.height)];
    let changed=SLOTS.with(|slots|{let mut slots=slots.borrow_mut();let s=&mut slots[usize::from(quick)];if *s==Some(rect){false}else{*s=Some(rect);true}});
    if changed{emit(&json!({"type":"ninjaAtmosphereSlot","rect":rect,"quick":quick}));}
}
pub(super) fn pet_slot(bounds:Bounds<Pixels>){
    let rect=[f32::from(bounds.origin.x),f32::from(bounds.origin.y),f32::from(bounds.size.width),f32::from(bounds.size.height)];
    let changed=HOST.with(|host|{let mut host=host.borrow_mut();let Some(host)=host.as_mut()else{return false};if host.pet_rect==Some(rect){return false}host.pet_rect=Some(rect);true});
    if changed{emit(&json!({"type":"workspacePetSlot","rect":rect}));}
}
pub(super) fn quick_pet_slot(bounds:Bounds<Pixels>){
    let rect=[f32::from(bounds.origin.x),f32::from(bounds.origin.y),f32::from(bounds.size.width),f32::from(bounds.size.height)];
    let changed=HOST.with(|host|{let mut host=host.borrow_mut();let Some(host)=host.as_mut()else{return false};if host.quick_pet_rect==Some(rect){return false}host.quick_pet_rect=Some(rect);true});
    if changed{emit(&json!({"type":"quickPetSlot","rect":rect}));}
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
            app.unified_children=vec![runtime::broker::ChildSnapshot{name:None,scope:None,task:None,model:None,effort:None,workspace:None,provider:AgentKind::Codex,project,parent_id:"native-parent".into(),id:"native-child".into(),turn_id:Some("child-turn".into()),status:chat_interactions::InteractionStatus::Working,preview:"Working child".into(),requests:vec![],transcript_bytes:0,transcript_revision:0,connected:true}];
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
            assert_eq!(app.unified_snapshot(cx)["reasoningEffort"],"high");assert_eq!(app.workspace_snapshot(cx).reasoning_efforts[&app.effort_key()],"high");
            body["effort"]=json!("invented");assert!(app.unified_command(&body,window,cx).is_err());assert_eq!(app.unified_effort(),Some("high"));
            app.model=None;app.controls.insert("codex",(Instant::now(),agent_controls::ControlInfo{defaults:agent_controls::ModelDefaults{model:Some("fixture-model".into()),..Default::default()},auth:agent_controls::AuthState::Unknown,models:vec![],note:None,account:None,limits:vec![],catalog:None}));
            body=request(app,"settings",cx);body["model"]=json!("");body["effort"]=json!("low");app.unified_command(&body,window,cx).unwrap();assert!(app.model.is_none());assert_eq!(app.unified_effort(),Some("low"));
        });
    }
    #[gpui::test]
    fn reasoning_override_stays_with_its_chat_and_native_settings_resume(cx:&mut TestAppContext){
        let (view,cx)=crate::ui_tests::fixture(cx);
        view.update(cx,|app,_|{
            app.agent=AgentKind::Codex;app.model=None;app.active=Some("fixture-session".into());
            app.sessions[0].external_id=Some("native-thread".into());
            app.reasoning_efforts.insert(app.effort_key(),"high".into());
            app.usage.insert("fixture-session".into(),usage::Usage{model:Some("chat-model".into()),reasoning_effort:Some("low".into()),..Default::default()});
            assert_eq!(app.request_model().as_deref(),Some("chat-model"));assert_eq!(app.request_effort().as_deref(),Some("high"));
            app.native_models.insert(AgentKind::Codex,vec![agent_controls::ModelChoice{id:"chat-model".into(),label:"Chat".into(),reasoning_efforts:vec!["high".into()]}]);
            app.controls.insert("codex",(Instant::now(),agent_controls::ControlInfo{defaults:agent_controls::ModelDefaults{model:Some("other-default".into()),..Default::default()},auth:agent_controls::AuthState::Unknown,models:vec![],note:None,account:None,limits:vec![],catalog:None}));
            assert_eq!(app.effort_model_choice().unwrap().id,"chat-model");
            app.active=Some("other-chat".into());assert_eq!(app.unified_effort(),None);assert_eq!(app.request_effort(),None);
            app.reasoning_efforts.insert("codex:".into(),"medium".into());assert_eq!(app.unified_effort(),None);
            app.active=None;assert_eq!(app.unified_effort(),Some("medium"));
            app.active=Some("fixture-session".into());assert_eq!(app.request_effort().as_deref(),Some("high"));
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
pub(super) fn quick_prepare(){if active(){emit(&json!({"type":"quickPrepare"}));}}
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
    let callback=HOST.with(|host|host.borrow().as_ref().map(|h|h.voice_host))?;
    let view=unsafe{callback()};(!view.is_null()).then_some(view)
}

impl Orbit{
    pub(super) fn request_model(&self)->Option<String>{if self.agent==AgentKind::Ninja{let m=self.ninja_selection().model;return (!m.is_empty()).then_some(m)}self.model.clone().or_else(||self.active.as_ref().filter(|id|self.sessions.iter().any(|s|s.id==**id&&s.agent==self.agent&&s.external_id.is_some())).and_then(|id|self.usage.get(id)).and_then(|u|u.model.clone())).or_else(||self.unified_effort().and_then(|_|self.effort_model_choice()).map(|model|model.id))}
    pub(super) fn request_effort(&self)->Option<String>{self.unified_effort().map(str::to_owned).or_else(||self.active.as_ref().filter(|id|self.sessions.iter().any(|s|s.id==**id&&s.agent==self.agent&&s.external_id.is_some())).and_then(|id|self.usage.get(id)).and_then(|u|u.reasoning_effort.clone()))}

    pub(super) fn model_project(&self)->PathBuf{
        self.active.as_ref().and_then(|id|self.sessions.iter().find(|s|s.id==*id)).map(|s|s.execution_path.as_ref().unwrap_or(&s.project_path).clone()).filter(|p|p.is_dir()).unwrap_or_else(||if self.project.is_dir(){self.project.clone()}else{self.controls_home.clone()})
    }
    pub(super) fn effective_model_settings(&self)->(Option<String>,Option<String>){
        if self.agent==AgentKind::Ninja{let s=self.active.as_ref().and_then(|id|self.runs.get(id)).and_then(|r|r.retry.as_ref()).and_then(|r|r.ninja.clone()).unwrap_or_else(||self.ninja_selection());return ((!s.model.is_empty()).then_some(s.model),s.thinking)}
        let controls=self.controls.get(self.agent.command()).map(|(_,v)|&v.defaults);
        let run=self.active.as_ref().and_then(|id|self.runs.get(id)).filter(|r|r.agent==self.agent);
        let usage=self.active.as_ref().filter(|id|self.sessions.iter().any(|s|s.id==**id&&s.agent==self.agent)).and_then(|id|self.usage.get(id));
        let alias=|id:&str|controls.and_then(|d|d.aliases.get(id)).cloned().unwrap_or_else(||id.into());
        let selected=self.model.as_deref().map(alias);
        let configured=self.active.is_none().then(||controls.and_then(|d|d.model.as_deref()).map(alias)).flatten();
        let reported=usage.and_then(|u|u.model.as_deref()).map(alias);
        let model=if run.is_some(){reported.clone().or_else(||run.and_then(|r|r.retry.as_ref()).and_then(|r|r.model.as_deref()).map(alias)).or(selected).or(configured)}else{selected.or(reported).or(configured)};
        let reported_matches=usage.and_then(|u|u.model.as_deref()).map(alias)==model;
        let default_effort=self.active.is_none().then(||controls.and_then(|d|d.effort.clone().or_else(||model.as_ref().and_then(|m|d.efforts.get(m)).cloned()))).flatten();
        let effort=if run.is_some(){usage.filter(|_|reported_matches).and_then(|u|u.reasoning_effort.clone()).or_else(||run.and_then(|r|r.retry.as_ref()).and_then(|r|r.reasoning_effort.clone())).or(default_effort)}else{self.unified_effort().map(str::to_owned).or_else(||usage.filter(|_|reported_matches).and_then(|u|u.reasoning_effort.clone())).or(default_effort)};
        (model,effort)
    }
    pub(super) fn effective_model_label(&self)->String{
        let (model,effort)=self.effective_model_settings();
        let name=model.unwrap_or_else(||"…".into());
        effort.filter(|v|!v.is_empty()).map_or_else(||name.clone(),|effort|format!("{name} · {effort}"))
    }
}

#[cfg(test)]mod native_reentry_tests{
 use super::*;use gpui::TestAppContext;use std::prelude::v1::test;
 thread_local!{static REENTERED:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};static STABLE:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}
 unsafe extern "C" fn publish(bytes:*const u8,len:usize){
  let before=unsafe{std::slice::from_raw_parts(bytes,len)}.to_vec();
  REENTERED.set(HOST.with(|h|h.try_borrow_mut().map(|mut h|{h.as_mut().unwrap().last=Vec::new().into();}).is_ok()));
  STABLE.set(before==unsafe{std::slice::from_raw_parts(bytes,len)});
 }
 unsafe extern "C" fn voice_host()->*mut c_void{REENTERED.set(HOST.with(|h|h.try_borrow_mut().is_ok()));std::ptr::null_mut()}
 unsafe extern "C" fn levels(_: *const f32,_:usize){}
 unsafe extern "C" fn wallpaper(_: *const u8,_:usize,_:usize,_:usize,_:f32){}
 unsafe extern "C" fn surface(_: *mut c_void,_:bool)->bool{false}
 unsafe extern "C" fn noop(){}
 struct Restore(Option<Host>);
 impl Drop for Restore{fn drop(&mut self){HOST.with(|h|{*h.borrow_mut()=self.0.take()});}}
 #[gpui::test]fn native_state_callback_can_reenter_and_replace_cached_bytes(cx:&mut TestAppContext){
  let(view,cx)=crate::ui_tests::fixture(cx);
  let _restore=Restore(HOST.with(|h|h.replace(Some(Host{_library:std::ptr::null_mut(),publish,levels,voice_host,wallpaper,quick_surface:surface,wallpaper_visible:false,last_wallpaper:None,collapse:noop,_toggle:noop,last:Vec::new().into(),history:Value::Null,history_owner:String::new(),completed:None,error:None,results:Default::default(),reported_successes:Default::default(),finished:vec![],pet_rect:None,quick_pet_rect:None,workspace_motion:Default::default(),publish_pending:false}))));
  REENTERED.set(false);STABLE.set(false);
  view.update(cx,|app,cx|app.publish_unified(cx));
  assert!(REENTERED.get(),"State delivery held the owner borrow across AppKit reentry");
  assert!(STABLE.get(),"Replacing cached state invalidated the lent native bytes");
  REENTERED.set(false);assert!(super::voice_host().is_none());assert!(REENTERED.get(),"Media host lookup held the owner borrow across AppKit reentry");
  HOST.with(|h|{let mut h=h.borrow_mut();h.as_mut().unwrap().completed=Some("approval-response".into());h.as_mut().unwrap().error=Some("original-result".into());});
  cx.update(|window,cx|view.update(cx,|app,cx|app.dispatch_unified(json!({"action":"pet_visuals","ownerToken":app.unified_token(),"pets":{}}),window,cx)));
  HOST.with(|h|{let h=h.borrow();let h=h.as_ref().unwrap();assert_eq!(h.completed.as_deref(),Some("approval-response"),"Pet presentation consumed the native decision acknowledgement");assert_eq!(h.error.as_deref(),Some("original-result"));});
 }
}

#[cfg(test)]mod effective_model_tests{
 use super::*;use gpui::TestAppContext;use std::prelude::v1::test;
 #[gpui::test]fn default_labels_use_real_config_and_active_reports_without_pinning_the_model(cx:&mut TestAppContext){
  let(view,cx)=crate::ui_tests::fixture(cx);view.update(cx,|app,cx|{
   let mut defaults=agent_controls::ModelDefaults{model:Some("configured-model".into()),effort:Some("high".into()),..Default::default()};defaults.efforts.insert("explicit-model".into(),"low".into());
   app.controls.insert("codex",(Instant::now(),agent_controls::ControlInfo{defaults,auth:agent_controls::AuthState::Unknown,models:vec![],note:None,account:None,limits:vec![],catalog:None}));
   app.agent=AgentKind::Codex;app.active=None;app.model=None;app.runs.clear();app.usage.clear();assert_eq!(app.effective_model_settings(),(Some("configured-model".into()),Some("high".into())));assert_eq!(app.model,None);
   app.active=Some("fixture-session".into());assert_eq!(app.effective_model_settings(),(None,None));app.active=None;
   app.model=Some("explicit-model".into());assert_eq!(app.effective_model_settings(),(Some("explicit-model".into()),Some("high".into())));
   app.controls.get_mut("codex").unwrap().1.defaults.effort=None;assert_eq!(app.effective_model_settings().1.as_deref(),Some("low"));
   app.active=Some("fixture-session".into());app.usage.insert("fixture-session".into(),usage::Usage{model:Some("native-rerouted-model".into()),reasoning_effort:Some("medium".into()),..Default::default()});app.runs.insert("fixture-session".into(),crate::ui_tests::idle_run("Thinking".into()));
   let snapshot=app.unified_snapshot(cx);assert_eq!(snapshot["effectiveModel"],"native-rerouted-model");assert_eq!(snapshot["effectiveReasoningEffort"],"medium");assert_eq!(snapshot["readOnly"],false);assert_eq!(snapshot["composerBusy"],false);
   app.agent=AgentKind::Claude;app.model=None;assert_ne!(app.effective_model_settings().0.as_deref(),Some("native-rerouted-model"));
  });
 }
}

impl Orbit{
    pub(super) fn effort_model_choice(&self)->Option<agent_controls::ModelChoice>{
        let defaults=self.controls.get(self.agent.command()).map(|(_,info)|&info.defaults);
        let resolved=self.effective_model_settings().0;
        let id=self.model.as_deref().or(resolved.as_deref()).or_else(||defaults.and_then(|d|d.model.as_deref()))?;
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
        window.update(cx,|app,_,cx|{
            assert!(app.runs.is_empty(),"Isolated voice probe unexpectedly contained user runs");
            app.project=project;app.agent=AgentKind::Codex;app.model=None;app.active=None;app.loading=false;app.new_execution_path=None;
            if std::env::var_os("KAI_VOICE_GEMINI_SOURCE").is_some() {
                let source=SessionSummary{id:uuid::Uuid::new_v4().to_string(),title:"Saved Gemini voice handoff".into(),agent:AgentKind::Gemini,project_path:app.project.clone(),execution_path:None,external_id:Some("preserved-gemini-native-source".into()),transcript_path:None,updated_at:chrono::Utc::now().timestamp()};
                app.engine.update_session(source.clone());
                app.engine.save_prompt(source.clone(),"We agreed the lantern is cobalt blue. Remember its color for our voice conversation.".into(),None);
                app.active=Some(source.id.clone());app.sessions.insert(0,source);app.agent=AgentKind::Gemini;app.model=Some("gemini-source-only".into());
            } else { app.agent=AgentKind::Gemini;app.model=Some("gemini-source-only".into()); }
            cx.notify();
        }).expect("Prepare voice probe conversation");
        let quick_start=std::env::var_os("KAI_VOICE_QUICK_START").is_some();
        if quick_start{
            cx.update(|cx|cx.update_global::<quick_chat::Controller,_>(|service,cx|service.open(cx).expect("Open mini Code before voice"))).expect("Mini Code voice surface");
            Timer::after(Duration::from_millis(1000)).await;
        }
        window.update(cx,|app,window,cx|{
            if std::env::var_os("KAI_VOICE_UI_BUTTON").is_some() {
                if !quick_start{reveal();}app.toggle_voice(window,cx);
                app.voice_call.as_ref().expect("UI voice call").media.initial_prompt(fixture["audio"].as_str().expect("Generated audio")).unwrap();
            } else {
                let mut body=fixture;body["action"]=json!("workspace_voice_prompt");body["ownerToken"]=json!(app.unified_token());body["requestId"]=json!("native-voice-probe");
                app.unified_command(&body,window,cx).expect("Start real packaged voice");
            }
            assert_eq!(app.agent,AgentKind::Codex);assert_ne!(app.model.as_deref(),Some("gemini-source-only"));cx.notify();
        }).expect("Voice probe window");
        let mut complete=false;
        for _ in 0..240{
            Timer::after(Duration::from_millis(250)).await;
            let ready=window.update(cx,|app,_,cx|{
                if let Some(error)=&app.voice_error{panic!("Packaged voice failed: {error}")}
                let snapshot=app.unified_snapshot(cx);
                let user=app.rows.iter().any(|r|matches!(&r.block,MessageBlock::Voice{speaker:chat_interactions::Speaker::User,text,..}|MessageBlock::Interaction(chat_interactions::ChatInteraction::Speaker{speaker:chat_interactions::Speaker::User,text}) if !text.is_empty()));
                let assistant=app.rows.iter().any(|r|matches!(&r.block,MessageBlock::Voice{speaker:chat_interactions::Speaker::Assistant,text,..}|MessageBlock::Interaction(chat_interactions::ChatInteraction::Speaker{speaker:chat_interactions::Speaker::Assistant,text}) if !text.is_empty()));
                let active=app.voice_call.as_ref().is_some_and(|call|call.active&&call.owns_current_chat(app.active.as_deref()));
                if active&&user&&assistant{
                    assert_eq!(snapshot["readOnly"],false);assert_eq!(snapshot["composerBusy"],false);
                    assert!(!snapshot["sessionActivity"]["text"].as_str().unwrap_or("").contains("Connecting"));
                    assert!(snapshot["effectiveModel"].as_str().is_some_and(|s|!s.is_empty()));assert!(snapshot["effectiveReasoningEffort"].as_str().is_some_and(|s|!s.is_empty()));assert!(app.model.is_none(),"Resolved defaults were incorrectly pinned");
                    if std::env::var_os("KAI_VOICE_GEMINI_SOURCE").is_some() {
                        assert!(app.handoff_packets.values().any(|p|p.recent.contains("cobalt blue")),"Saved source context was not transferred");
                        let context_spoken=app.rows.iter().any(|r|matches!(&r.block,MessageBlock::Voice{speaker:chat_interactions::Speaker::Assistant,text,..}|MessageBlock::Interaction(chat_interactions::ChatInteraction::Speaker{speaker:chat_interactions::Speaker::Assistant,text}) if text.to_lowercase().contains("cobalt")));
                        if !context_spoken { return false; }
                    }
                    cx.notify();return true
                }false
            }).expect("Read packaged voice state");
            if ready{complete=true;break}
        }
        assert!(complete,"Packaged native voice did not produce both transcripts");
        // Exercise the actual surface sequence with one live WebRTC session.
        let (session,owner)=window.update(cx,|app,_,_|{let call=app.voice_call.as_ref().unwrap();(call.session_id.clone(),call.owner.clone())}).expect("Voice identity");
        let _=cx.update(|_|reveal());Timer::after(Duration::from_secs(1)).await;
        let _=cx.update(|_|emit(&json!({"type":"probeDoubleCommand"})));Timer::after(Duration::from_secs(1)).await;
        window.update(cx,|app,_,_|{let call=app.voice_call.as_ref().expect("Double Command hung up voice");assert_eq!(call.session_id,session);assert!(call.active);assert!(!app.runs[&owner].handle.is_cancelled());}).expect("Compacted live voice");
        let _=cx.update(|_|reveal());Timer::after(Duration::from_secs(1)).await;
        let _=cx.update(|_|collapse());Timer::after(Duration::from_millis(850)).await;
        let _=cx.update(|cx|cx.update_global::<quick_chat::Controller,_>(|service,cx|service.toggle(cx).expect("Open Quick Chat during voice")));
        Timer::after(Duration::from_millis(500)).await;
        let _=cx.update(|cx|cx.update_global::<quick_chat::Controller,_>(|service,cx|service.dismiss(cx)));
        Timer::after(Duration::from_millis(500)).await;
        let before_audio=window.update(cx,|app,_,_|{let call=app.voice_call.as_ref().expect("Quick Chat dismissal hung up voice");assert_eq!(call.session_id,session);assert_eq!(call.owner,owner);assert!(call.active);app.runs[&owner].handle.steer("Please say exactly: lifecycle continuity verified. Do not use tools.").expect("Speak after surface transitions");call.media.received_audio_bytes()}).expect("Voice continued in original chat");
        let mut received=false;
        for _ in 0..160{Timer::after(Duration::from_millis(250)).await;if window.update(cx,|app,_,_|{assert!(app.voice_error.is_none(),"Voice failed across surfaces: {:?}",app.voice_error);let call=app.voice_call.as_ref().expect("Voice vanished during surface transition");assert_eq!(call.session_id,session);call.active && call.media.received_audio_bytes()>before_audio}).expect("Live audio after collapse"){received=true;break}}
        assert!(received,"The same WebRTC session did not receive audio after notch/workspace/Quick Chat transitions");
        let workspace_before_stop=cx.update(|_|{
            let function=HOST.with(|host|unsafe{libc::dlsym(host.borrow().as_ref().unwrap()._library,c"kai_unified_diagnostics".as_ptr())});
            let diagnostics:unsafe extern "C" fn()->u32=unsafe{std::mem::transmute(function)};
            (unsafe{diagnostics()}) & 1
        }).expect("Read existing background workspace before stop");
        window.update(cx,|app,_,cx|app.stop_voice(cx)).expect("Explicit voice stop");
        Timer::after(Duration::from_secs(3)).await;
        window.update(cx,|app,_,_|assert!(app.voice_call.is_none(),"Packaged voice stop left its media alive")).expect("Check voice teardown");
        // A realtime close can precede its backing coding turn's completion.
        // That real work must keep its status; only a quiescent owner may retire.
        let mut quiescent=false;
        for _ in 0..80 {
            let idle=window.update(cx,|app,_,_|app.runs.is_empty() && app.launches.is_empty() && app.settling.is_empty() && app.finalizing.is_empty() && app.native_pending.is_empty()).expect("Read voice backing-work ownership");
            if idle {quiescent=true;break}
            Timer::after(Duration::from_millis(250)).await;
        }
        assert!(quiescent,"The generated voice fixture left backing work running after explicit stop");
        Timer::after(Duration::from_secs(1)).await;
        let _=cx.update(|_|{
            let function=HOST.with(|host|unsafe{libc::dlsym(host.borrow().as_ref().unwrap()._library,c"kai_unified_diagnostics".as_ptr())});
            let diagnostics:unsafe extern "C" fn()->u32=unsafe{std::mem::transmute(function)};
            let surfaces=unsafe{diagnostics()} & 3;
            assert_eq!(surfaces & 2,0,"Quiescent voice stop left a ghost notch");
            // Background Code intentionally remains open when the notch is
            // over another app. Stop must never create/reopen a workspace, but
            // it must not close the user's pre-existing background window.
            assert!(surfaces & 1 == 0 || workspace_before_stop != 0,"Voice stop reopened Code");
        });
        eprintln!("Kai packaged voice: real transcripts, live draft ownership, editable voice status, resolved model/effort, unpinned CLI defaults, and same-session notch/workspace/Double-Command/Quick Chat continuity, real incoming audio after transitions, and explicit native stop passed");
        let _=cx.update(|cx|cx.quit());
    }).detach();Ok(())
}

impl Orbit{
    pub(super) fn provider_default_model(&self)->Option<String>{
        let defaults=&self.controls.get(self.agent.command())?.1.defaults;
        let id=defaults.model.as_ref()?;Some(defaults.aliases.get(id).unwrap_or(id).clone())
    }
}

/// Real composer -> checkpoint -> native Codex -> persisted chat, in an isolated
/// temporary workspace. No user chats, prompts or project files are touched.
fn start_agent_probe(window:WindowHandle<Orbit>,cx:&mut App)->anyhow::Result<()>{
    let root=std::path::PathBuf::from(std::env::var("KAI_AGENT_FIXTURE")?).canonicalize()?;
    anyhow::ensure!(root.parent()==Some(std::path::Path::new("/private/tmp")) && root.file_name().and_then(|s|s.to_str()).is_some_and(|s|s.starts_with("kai-agent-ui-")),"Agent probe requires its own temporary workspace");
    cx.spawn(async move|cx|{
        let started=Instant::now();
        for _ in 0..100{
            let ready=window.update(cx,|app,_,_|app.agents.iter().any(|a|a.kind==AgentKind::Codex&&a.available)).unwrap_or(false);
            if ready{break}Timer::after(Duration::from_millis(100)).await;
        }
        let first=window.update(cx,|app,window,cx|{
            assert!(app.runs.is_empty()&&app.launches.is_empty());
            app.project=root.clone();app.new_execution_path=None;app.agent=AgentKind::Codex;app.model=std::env::var("KAI_AGENT_PROBE_MODEL").ok();app.active=None;app.loading=false;app.loading_earlier=false;app.allow_edits=true;
            app.trusted_workspaces.insert((AgentKind::Codex,root.clone()));
            app.composer.update(cx,|input,cx|input.set_text("Create kai-proof.txt containing exactly KAI_AGENT_OK. Do not edit any other file. Reply KAI_AGENT_OK.",cx));
            app.send(window,cx);
            let id=app.active.clone().expect("UI send did not create its chat");
            assert!(app.launches.contains_key(&id),"UI send did not reserve its own launch: {:?}",app.notice);
            app.new_conversation(window,cx);
            assert!(!app.starting_current(),"A background launch contaminated a new chat");
            assert!(app.unified_send_ready(),"A background launch blocked an unrelated composer");
            assert!(!app.unified_snapshot(cx)["sessionActivity"]["text"].as_str().unwrap_or("").contains("Preparing workspace"));
            eprintln!("Kai agent UI probe: new-chat submission and independent draft passed");id
        }).expect("Probe window");
        let mut reasoning_seen=false;
        for turn in 0..2{
            let mut done=false;
            for _ in 0..480{
                Timer::after(Duration::from_millis(250)).await;
                let state=window.update(cx,|app,_,_|{
                    let observed=app.runs.get(&first).is_some_and(|run|presentation::message_blocks("assistant",&run.text).iter().any(|block|matches!(block,MessageBlock::Interaction(chat_interactions::ChatInteraction::Status{title,details,..})if title=="Thinking"&&!details.is_empty())));
                    let busy=app.launches.contains_key(&first)||app.runs.contains_key(&first)||app.finalizing.contains(&first)||app.settling.contains_key(&first);
                    let finished=app.turn_timings.iter().filter(|r|r.session==first&&r.finished_ms.is_some()).count();
                    if !busy&&finished>turn{
                        let record=app.turn_timings.iter().rev().find(|r|r.session==first).unwrap();
                        assert_eq!(record.outcome.as_deref(),Some("completed"),"Agent failed in actual UI: {:?}",app.notice);
                        assert!(record.elapsed_ms.is_some());
                        assert!(app.usage.get(&first).and_then(|u|u.model.as_ref()).is_some(),"Native run never reported its actual model");return (true,observed)
                    }(false,observed)
                }).expect("Probe state");
                reasoning_seen |= state.1;
                if state.0{done=true;break}
            }
            assert!(done,"Actual UI agent turn did not settle");
            let body=std::fs::read_to_string(root.join("kai-proof.txt")).expect("Native agent did not write its fixture");
            assert!(body.contains(if turn==0{"KAI_AGENT_OK"}else{"SECOND_OK"}),"Native edit was not performed");
            if turn==0{
                window.update(cx,|app,window,cx|{
                    let session=app.sessions.iter().find(|s|s.id==first).cloned().expect("Owned chat disappeared");app.open_session(session,window,cx);
                }).expect("Open existing chat");
                for _ in 0..100{Timer::after(Duration::from_millis(100)).await;if window.update(cx,|app,_,_|!app.loading).unwrap_or(false){break}}
                window.update(cx,|app,window,cx|{
                    assert_eq!(app.active.as_ref(),Some(&first));assert!(!app.loading);
                    assert!(app.rows.iter().any(|r|r.role.as_deref()==Some("user")),"Returning to the chat lost its submitted prompt");
                    app.composer.update(cx,|input,cx|input.set_text("Read kai-proof.txt and append SECOND_OK on a new line. Do not edit any other file. Reply SECOND_OK.",cx));
                    app.send(window,cx);
                    assert!(app.launches.contains_key(&first),"Existing-chat UI send did not launch: {:?}",app.notice);
                    eprintln!("Kai agent UI probe: first native edit, exact model report, and resumed-chat submission passed");
                }).expect("Resume existing chat");
            }
        }
        if std::env::var_os("KAI_AGENT_REASONING_PROBE").is_some(){assert!(reasoning_seen,"Native Codex never streamed a readable reasoning summary into its live chat card");eprintln!("Kai agent UI probe: real Codex reasoning summary streamed into the live typed transcript");}
        if let Ok(path)=std::env::var("KAI_AGENT_VOICE_FIXTURE"){
            assert_eq!(std::env::var("KAI_CODE_VOICE_DIAGNOSTIC").as_deref(),Ok("1"),"Voice UI probe must disable ambient microphone audio");
            assert!(std::fs::metadata(&path).unwrap().len()<2*1024*1024);
            let fixture:Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();let audio=fixture["audio"].as_str().expect("Generated speech fixture").to_owned();
            let _=cx.update(|_|reveal());Timer::after(Duration::from_secs(1)).await;
            window.update(cx,|app,window,cx|{
                app.toggle_voice(window,cx);
                app.voice_call.as_ref().expect("Actual UI voice button did not open a call").media.initial_prompt(&audio).expect("Generated voice prompt");
                eprintln!("Kai agent UI probe: actual workspace voice-button handler started media");
            }).expect("Voice UI");
            let mut spoken=false;
            for _ in 0..360{
                Timer::after(Duration::from_millis(250)).await;
                if window.update(cx,|app,_,cx|{
                    assert!(app.voice_error.is_none(),"Actual UI voice failed: {:?}",app.voice_error);
                    let user=app.rows.iter().any(|r|matches!(&r.block,MessageBlock::Voice{speaker:chat_interactions::Speaker::User,text,..}|MessageBlock::Interaction(chat_interactions::ChatInteraction::Speaker{speaker:chat_interactions::Speaker::User,text})if !text.is_empty()));
                    let assistant=app.rows.iter().any(|r|matches!(&r.block,MessageBlock::Voice{speaker:chat_interactions::Speaker::Assistant,text,..}|MessageBlock::Interaction(chat_interactions::ChatInteraction::Speaker{speaker:chat_interactions::Speaker::Assistant,text})if !text.is_empty()));
                    if app.voice_call.as_ref().is_some_and(|c|c.active&&c.owns_current_chat(app.active.as_deref()))&&user&&assistant{app.stop_voice(cx);return true}false
                }).expect("Voice UI state"){spoken=true;break}
            }
            assert!(spoken,"Actual UI voice did not produce both speaker transcripts");Timer::after(Duration::from_secs(3)).await;
            window.update(cx,|app,_,_|assert!(app.voice_call.is_none(),"Actual UI voice stop did not close media")).expect("Voice stop");
            eprintln!("Kai agent UI probe: workspace UI voice connected, exchanged real speech transcripts, and stopped");
        }
        eprintln!("Kai agent UI probe: new and resumed UI agents edited the fixture, per-chat startup stayed isolated, native model was reported, and both durations persisted in {:.1}s",started.elapsed().as_secs_f64());
        let _=cx.update(|cx|cx.quit());
    }).detach();Ok(())
}
