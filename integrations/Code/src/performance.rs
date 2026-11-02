//! Opt-in latency evidence, measured on real owner/layout paths, never idle work.
use super::*;
pub(super) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KAI_PERFORMANCE_LOG").is_some())
}
pub(super) struct Span {
    at: Instant,
    name: &'static str,
}
impl Span {
    pub fn new(name: &'static str) -> Option<Self> {
        enabled().then(|| Self {
            at: Instant::now(),
            name,
        })
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        sample(self.name, self.at.elapsed());
    }
}
pub(super) fn sample(name: &str, elapsed: Duration) {
    if enabled() {
        eprintln!("KAI_METRIC {} {:.4}", name, elapsed.as_secs_f64() * 1000.);
    }
}
#[derive(Default)]
pub(super) struct Tracker {
    pending: Option<(String, Instant, bool)>,
    visited: HashSet<String>,
}
impl Tracker {
    pub fn begin(&mut self, id: &str) {
        if !enabled() {
            return;
        }
        if self.visited.len() > 128 {
            self.visited.clear()
        }
        let warm = !self.visited.insert(id.into());
        self.pending = Some((id.into(), Instant::now(), warm));
    }
    pub fn ready(&mut self, id: Option<&str>, loading: bool) -> Option<(Instant, bool)> {
        if loading
            || self
                .pending
                .as_ref()
                .is_none_or(|(owner, _, _)| Some(owner.as_str()) != id)
        {
            return None;
        }
        self.pending.take().map(|(_, at, warm)| (at, warm))
    }
}
impl Orbit {
    pub(super) fn performance_ready(&mut self, window: &mut Window) {
        if let Some((at, warm)) = self.performance.ready(self.active.as_deref(), self.loading) {
            window.on_next_frame(move |_, _| {
                sample(
                    if warm {
                        "warm_chat_switch"
                    } else {
                        "cold_chat_switch"
                    },
                    at.elapsed(),
                )
            });
        }
    }
}
pub(super) fn start_probe(window: WindowHandle<Orbit>, cx: &mut App) {
    if !std::env::args().any(|a| a == "--unified-smoke-test") {
        return;
    }
    if std::env::args().any(|a|a=="--unified-ninja-render-probe"){start_ninja_probe(window,cx);return}
    cx.spawn(async move |cx| {
        if std::env::args().any(|a|a=="--unified-background-idle-probe"){
            Timer::after(Duration::from_millis(1800)).await;let _=window.update(cx,|_,_,_|unified::collapse());
            Timer::after(Duration::from_secs(28)).await;eprintln!("KAI_BACKGROUND_IDLE_PROBE_OK");let _=cx.update(|cx|cx.quit());return
        }

        Timer::after(Duration::from_millis(1800)).await;
        // Measure warm navigation after startup restoration, not concurrently
        // with an initial discovery that can legitimately restore selection.
        let startup=Instant::now();
        loop {
            if window.update(cx,|app,_,_|!app.discovering&&app.preferences_ready&&app.restore_pending.is_none()&&app.controls_pending.is_empty()).unwrap_or(false){break}
            if startup.elapsed()>Duration::from_secs(20){eprintln!("KAI_LATENCY_PROBE_FAILED startup did not settle");let _=cx.update(|cx|cx.quit());return}
            Timer::after(Duration::from_millis(25)).await;
        }
        let chats = window
            .update(cx, |app, _, _| {
                (0..4)
                    .map(|i| {
                        let session = SessionSummary {
                            id: uuid::Uuid::new_v4().to_string(),
                            title: format!("Latency fixture {i}"),
                            agent: AgentKind::Codex,
                            project_path: app.project.clone(),
                            execution_path: None,
                            updated_at: chrono::Utc::now().timestamp(),
                            external_id: None,
                            transcript_path: None,
                        };
                        app.engine.update_session(session.clone());
                        app.engine.save(
                            session.clone(),
                            "user".into(),
                            "Inspect this fixture".into(),
                        );
                        app.engine.save(
                            session.clone(),
                            "assistant:codex".into(),
                            "A small but real saved conversation.\n".repeat(100),
                        );
                        app.sessions.push(session.clone());
                        session
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap();
        Timer::after(Duration::from_millis(200)).await;
        for index in 0..28 {
            let chat = chats[index % chats.len()].clone();
            let id = chat.id.clone();
            let start = Instant::now();
            let _ = window.update(cx, |app, window, cx| app.open_session(chat, window, cx));
            loop {
                Timer::after(Duration::from_millis(10)).await;
                let ready = window
                    .update(cx, |app, _, _| {
                        !app.loading && app.active.as_deref() == Some(&id)
                    })
                    .unwrap_or(false);
                if ready {
                    break;
                }
                if start.elapsed() > Duration::from_secs(5) {
                    eprintln!("KAI_LATENCY_PROBE_FAILED history did not load");
                    let _ = cx.update(|cx| cx.quit());
                    return;
                }
            }
            Timer::after(Duration::from_millis(100)).await;
        }
        for _ in 0..20 {
            let _ = window.update(cx, |_, _, _| unified::collapse());
            Timer::after(Duration::from_millis(720)).await;
            let _ = window.update(cx, |_, window, cx| {
                let at = Instant::now();
                unified::reveal();
                window.on_next_frame(move |_, _| sample("warm_reveal", at.elapsed()));
                cx.notify();
            });
            Timer::after(Duration::from_millis(950)).await;
        }
        eprintln!("KAI_LATENCY_PROBE_OK");
        let _ = cx.update(|cx| cx.quit());
    })
    .detach();
}

// Render the same production widgets with an isolated, deliberately long chat.
// No provider, workspace files, saved identities, or live worker are involved.
fn start_ninja_probe(owner:WindowHandle<Orbit>,cx:&mut App){
    fn frames(window:&mut Window,left:usize,at:Instant){
        window.refresh();window.on_next_frame(move|window,_|{
            sample("ninja_frame",at.elapsed());if left>1{frames(window,left-1,Instant::now());}
        });
    }
    cx.spawn(async move|cx|{
        Timer::after(Duration::from_secs(3)).await;
        let _=owner.update(cx,|app,window,cx|{
            app.ninjas=crate::ninjas_ui::State{started:true,..Default::default()};app.ninja.draft.node=false;
            app.open_ninjas(Some("validation".into()),None,window,cx);
            let p=app.ninjas.panel.as_mut().unwrap();p.profile.revision=1;p.profile.name="rendering validation".into();p.profile.style="ice".into();p.chat=Some("fixture".into());p.settings=false;
            let tool=chat_interactions::serialize_interaction(&chat_interactions::ChatInteraction::Delegation{title:"Terminal output".into(),details:"$ cargo check\nChecking project\nFinished successfully\n".repeat(100),status:chat_interactions::InteractionStatus::Completed,agent:None});
            let thought=chat_interactions::serialize_interaction(&chat_interactions::ChatInteraction::Status{title:"Thinking".into(),details:"Checking the changes and verifying the result.".into(),status:chat_interactions::InteractionStatus::Completed});
            p.messages=(0..24).map(|i|serde_json::json!({"id":format!("fixture-{i}"),"profile":p.profile.id,"chat":"fixture","created":i,"prompt":"Update the terminal and verify the result. Keep messages readable as the window changes size.","brief":format!("{thought}\n\n{tool}\n\n## Verified\n\nThe **terminal update** is ready. The full command output is available above.\n\n```rust\nlet result = verify_changes()?;\n```\n\n- readable messages\n- one tool result per command\n- smooth scrolling"),"state":"completed","finished":i+1})).collect();
            if let Ok(endpoint)=std::env::var("KAI_NINJA_IMAGE_PROBE_ENDPOINT"){
                for i in 0..6{p.messages[17+i]["brief"]=serde_json::json!(format!("![preview](/fixture/preview-{i}.svg)"));}
                p.messages[23]["prompt"]=serde_json::json!("hi");
                let approval=chat_interactions::serialize_interaction(&chat_interactions::ChatInteraction::NativeRequest(runtime::NativeRequest{id:"fixture-approval".into(),method:"tool/approval".into(),title:"Approved shell command".into(),detail:"echo preview".into(),kind:runtime::NativeRequestKind::Unsupported}));
                let completed=chat_interactions::serialize_interaction(&chat_interactions::ChatInteraction::Status{title:"Completed".into(),details:"".into(),status:chat_interactions::InteractionStatus::Completed});
                p.messages[23]["brief"]=serde_json::json!(format!("starting the build\n\n{thought}\n\nprogress update\n\n{tool}\n\n{approval}\n\n{tool}\n\nhere’s the image from your host.\n\n![generated preview](/fixture/preview-6.png)\n\n[vector preview](/fixture/preview-7.svg)\n\n{completed}"));
                p.messages[23]["started"]=serde_json::json!(1);p.messages[23]["status"]=serde_json::json!("Completed");
                app.ninjas.hosts.push(crate::ninjas_ui::Host{id:"validation".into(),name:"Preview fixture".into(),endpoint:Some(endpoint)});
                app.ninja_prefetch_images(cx);
            }
            app.sidebar_split=false;app.sidebar_layout_transition=None;app.conversations_visible=false;
            window.activate_window();cx.activate(true);frames(window,120,Instant::now());cx.notify();
        });
        for phase in ["wide","compact"]{
            if phase=="compact"{let _=owner.update(cx,|_,window,_|window.resize(size(px(620.),px(720.))));}
            Timer::after(Duration::from_secs(4)).await;
            let id=owner.update(cx,|_,window,_|native::window_number(window)).unwrap_or(0);
            if id>0{let destination=format!("/private/tmp/kai050-ninja-{phase}.png");let result=cx.background_executor().spawn(async move{std::process::Command::new("/usr/sbin/screencapture").args(["-x","-l",&id.to_string(),&destination]).status()}).await;if !result.is_ok_and(|s|s.success()){eprintln!("KAI_NINJA_RENDER_PROBE_FAILED capture");}}
        }
        if std::env::var_os("KAI_NINJA_IMAGE_PROBE_ENDPOINT").is_some(){
            let count=owner.update(cx,|app,_,_|app.ninjas.artifacts.values().filter(|a|a.as_ref().is_ok_and(|a|a.image.is_some()&&a.native_preview.is_some())).count()).unwrap_or(0);
            eprintln!("KAI_NINJA_IMAGE_PREVIEWS {count}");
            if count!=8{eprintln!("KAI_NINJA_RENDER_PROBE_FAILED image queue");let _=cx.update(|cx|cx.quit());return;}
        }
        eprintln!("KAI_NINJA_RENDER_PROBE_OK");let _=cx.update(|cx|cx.quit());
    }).detach();
}
