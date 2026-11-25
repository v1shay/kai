//! Opt-in latency evidence, measured on real owner/layout paths, never idle work.
use super::*;
pub(super) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KAI_PERFORMANCE_LOG").is_some())
}
pub(super) fn trace_motion() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KAI_TRACE_MOTION").is_some())
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
    if std::env::args().any(|arg| arg == "--unified-chat-split-probe") {
        start_chat_split_probe(window, cx);
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
        let native_fixture = std::env::var_os("KAI_LATENCY_NATIVE_FIXTURE").is_some();
        let chat_count = if native_fixture { 24 } else { 4 };
        let chats = window
            .update(cx, |app, _, _| {
                let chats = (0..chat_count)
                    .map(|i| {
                        let id = uuid::Uuid::new_v4().to_string();
                        let transcript_path = native_fixture.then(|| native_history_fixture(&id));
                        let session = SessionSummary {
                            id,
                            title: format!("Latency fixture {i}"),
                            agent: AgentKind::Codex,
                            project_path: app.project.clone(),
                            execution_path: None,
                            updated_at: chrono::Utc::now().timestamp(),
                            external_id: None,
                            transcript_path,
                        };
                        app.engine.update_session(session.clone());
                        if !native_fixture {
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
                        }
                        app.sessions.push(session.clone());
                        session
                    })
                    .collect::<Vec<_>>();
                app.rebuild_workspace_sessions();
                chats
            })
            .unwrap();
        Timer::after(Duration::from_millis(200)).await;
        for index in 0..chat_count + 24 {
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
        for _ in 0..if native_fixture { 0 } else { 20 } {
            let _ = window.update(cx, |_, _, _| unified::collapse());
            Timer::after(Duration::from_millis(720)).await;
            let _ = window.update(cx, |_, window, cx| {
                let at = Instant::now();
                window.on_next_frame(move |_, _| sample("warm_frame_wake", at.elapsed()));
                window.on_frame_submitted(move |_, _| sample("warm_reveal", at.elapsed()));
                unified::reveal();
                cx.notify();
            });
            Timer::after(Duration::from_millis(950)).await;
        }
        eprintln!("KAI_LATENCY_PROBE_OK");
        let _ = cx.update(|cx| cx.quit());
    })
    .detach();
}

pub(super) fn chat_split_probe_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::args().any(|arg| arg == "--unified-chat-split-probe"))
}

thread_local! {
    static SPLIT_PROBE_BOUNDS: std::cell::Cell<[Option<Bounds<Pixels>>; 3]> =
        const { std::cell::Cell::new([None; 3]) };
}

pub(super) fn chat_split_probe_area(index: usize) -> impl IntoElement {
    canvas(move |bounds, _, _| {
        SPLIT_PROBE_BOUNDS.with(|areas| {
            let mut positions = areas.get();
            positions[index] = Some(bounds);
            areas.set(positions);
        });
    }, |_, _, _, _| {}).absolute().size_full()
}

async fn probe_chat_drag(owner: WindowHandle<Orbit>, destination: usize, cx: &mut AsyncApp) {
    let (from, to) = SPLIT_PROBE_BOUNDS.with(|areas| {
        let positions = areas.get();
        (positions[0].expect("First chat tab has no bounds").center(),
         positions[destination].expect("Drop target has no bounds").center())
    });
    for event in [
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left, position: from, click_count: 1, ..Default::default()
        }),
        PlatformInput::MouseMove(MouseMoveEvent {
            pressed_button: Some(MouseButton::Left), position: to, ..Default::default()
        }),
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left, position: to, click_count: 1, ..Default::default()
        }),
    ] {
        AnyWindowHandle::from(owner).update(cx, |_, window, cx| {
            window.dispatch_event(event, cx);
        }).unwrap();
        Timer::after(Duration::from_millis(80)).await;
    }
}

async fn probe_split_prompt_click(owner: WindowHandle<Orbit>, cx: &mut AsyncApp) {
    let position = owner.update(cx, |app, _, cx| {
        let bounds = app.split_prompt_bounds(cx).expect("Split prompt bar has no bounds");
        point(bounds.left() + px(8.), bounds.center().y)
    }).unwrap();
    for event in [
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left, position, click_count: 1, ..Default::default()
        }),
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left, position, click_count: 1, ..Default::default()
        }),
    ] {
        AnyWindowHandle::from(owner).update(cx, |_, window, cx| {
            window.dispatch_event(event, cx);
        }).unwrap();
        Timer::after(Duration::from_millis(80)).await;
    }
}

pub(super) fn metal_split_probe_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("KAI_PROBE_METAL_TERMINAL").as_deref() == Ok("1")
        && std::env::args().any(|arg| arg == "--unified-smoke-test")
        && std::env::args().any(|arg| arg == "--unified-chat-split-probe"))
}

fn start_chat_split_probe(owner: WindowHandle<Orbit>, cx: &mut App) {
    cx.spawn(async move |cx| {
        Timer::after(Duration::from_secs(3)).await;
        let (first, second, first_panel, second_panel) = owner.update(cx, |app, window, cx| {
            let session = |title: &str| {
                let project = PathBuf::from(std::env::var_os("ORBIT_DATA_DIR").unwrap()).join(title);
                std::fs::create_dir_all(&project).unwrap();
                SessionSummary {
                id: uuid::Uuid::new_v4().to_string(),
                title: title.to_owned(),
                agent: AgentKind::Codex,
                project_path: project,
                execution_path: None,
                updated_at: chrono::Utc::now().timestamp(),
                external_id: None,
                transcript_path: None,
                }
            };
            let first = session("Split One");
            let second = session("Split Two");
            for session in [&first, &second] {
                app.sessions.push(session.clone());
                app.engine.save(session.clone(), "user".into(), session.title.clone());
            }
            app.open_session(first.clone(), window, cx);
            app.composer.update(cx, |input, cx| input.set_text("First independent draft", cx));
            app.new_terminal(window, cx);
            let first_panel = app.active_terminal_panel().unwrap().clone();
            app.open_session(second.clone(), window, cx);
            app.composer.update(cx, |input, cx| input.set_text("Second independent draft", cx));
            assert!(app.active_terminal_panel().is_none(), "Terminal followed navigation to a different chat");
            app.new_terminal(window, cx);
            let second_panel = app.active_terminal_panel().unwrap().clone();
            (first, second, first_panel, second_panel)
        }).unwrap();
        Timer::after(Duration::from_millis(350)).await;
        for destination in [1, 2] {
            probe_chat_drag(owner, destination, cx).await;
            owner.update(cx, |app, _, cx| {
                assert_eq!(app.split_peer(), Some(first.id.as_str()), "Chat drag to target {destination} did not create a split");
                if destination == 1 { app.close_chat_split(cx); }
            }).unwrap();
            Timer::after(Duration::from_millis(150)).await;
        }
        eprintln!("KAI_CHAT_DRAG_PROBE_OK");
        for (session, panel, draft) in [
            (&first, &first_panel, "First independent draft"),
            (&second, &second_panel, "Second independent draft"),
        ] {
            let peer_bounds = owner.update(cx, |app, _, cx| {
                let peer = app.split_prompt_bounds(cx).expect("Peer prompt is visible");
                let active = app.composer.read(cx).bounds().expect("Focused prompt is visible");
                assert!(!active.intersects(&peer), "Both prompts must occupy separate panes");
                peer
            }).unwrap();
            probe_split_prompt_click(owner, cx).await;
            owner.update(cx, |app, _, cx| {
                assert_eq!(app.active.as_deref(), Some(session.id.as_str()));
                assert_eq!(app.composer.read(cx).text(), draft);
                let active = app.composer.read(cx).bounds().expect("Focused prompt is visible");
                assert!((active.origin.x - peer_bounds.origin.x).abs() <= px(3.)
                    && (active.origin.y - peer_bounds.origin.y).abs() <= px(3.)
                    && (active.size.width - peer_bounds.size.width).abs() <= px(3.),
                    "Focusing a split chat changed its prompt layout: {peer_bounds:?} to {active:?}");
                assert_eq!(app.active_terminal_panel().unwrap().entity_id(), panel.entity_id());
            }).unwrap();
        }
        eprintln!("KAI_SPLIT_PROMPTS_PROBE_OK");

        let deadline = Instant::now() + Duration::from_secs(14);
        for (panel, marker) in [(&first_panel, "KAI_PTY_ONE"), (&second_panel, "KAI_PTY_TWO")] {
            // Exercise the actual shell and PTY without provider installation,
            // login state, or a command echo that already contains the marker.
            let (prefix, suffix) = marker.split_at(8);
            let command = format!("kai_probe_a='{prefix}'; kai_probe_b='{suffix}'; printf '%s%s\\n' '{prefix}' '{suffix}'\r");
            loop {
                let sent = cx.update(|cx| panel.read(cx).probe_command(&command)).unwrap();
                if sent { break; }
                assert!(Instant::now() < deadline, "Split shell did not start");
                Timer::after(Duration::from_millis(50)).await;
            }
        }
        loop {
            let running = cx.update(|cx| {
                first_panel.read(cx).probe_has_text("KAI_PTY_ONE")
                    && second_panel.read(cx).probe_has_text("KAI_PTY_TWO")
            }).unwrap();
            if running { break; }
            assert!(Instant::now() < deadline, "Both split PTYs must publish their own output");
            Timer::after(Duration::from_millis(50)).await;
        }
        let metal_ids=if metal_split_probe_enabled(){
            Some(cx.update(|cx|(
                first_panel.read(cx).metal_id().expect("First native Metal shell did not start").to_owned(),
                second_panel.read(cx).metal_id().expect("Second native Metal shell did not start").to_owned(),
            )).unwrap())
        }else{None};
        eprintln!("KAI_SPLIT_PTYS_PROBE_OK");
        for index in 0..12 {
            owner.update(cx, |app, window, cx| {
                let at = Instant::now();
                window.resize(size(px(if index % 2 == 0 { 1120. } else { 1440. }), px(760.)));
                window.on_frame_submitted(move |_, _| sample("codex_split_resize_frame", at.elapsed()));
                if index == 4 {
                    app.open_session(first.clone(), window, cx);
                    assert_eq!(app.active_terminal_panel().unwrap().entity_id(), first_panel.entity_id());
                    assert_eq!(app.split_peer(), Some(second.id.as_str()));
                }
            }).unwrap();
            Timer::after(Duration::from_millis(120)).await;
        }
        if metal_split_probe_enabled() {
            owner.update(cx, |app, _, cx| {
                assert_eq!(app.active_terminal_panel().unwrap().entity_id(), first_panel.entity_id());
                assert_eq!(app.split_peer(), Some(second.id.as_str()));
                // Read a new marker from shell state after the resize.
                // Earlier output may have scrolled out of the viewport.
                for panel in [&first_panel, &second_panel] {
                    assert!(panel.read(cx).probe_command(
                        "printf '%s%s%s\\n' \"$kai_probe_a\" \"$kai_probe_b\" _ALIVE\r"
                    ));
                }
            }).unwrap();
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                let retained = cx.update(|cx| {
                    first_panel.read(cx).probe_has_text("KAI_PTY_ONE_ALIVE")
                        && second_panel.read(cx).probe_has_text("KAI_PTY_TWO_ALIVE")
                }).unwrap();
                if retained { break; }
                assert!(Instant::now() < deadline, "Both Metal shells must retain state after resizing");
                Timer::after(Duration::from_millis(50)).await;
            }
            let (first_metal,second_metal)=metal_ids.expect("Native Metal identities were not captured");
            cx.update(|cx|{
                assert_eq!(first_panel.read(cx).metal_id().unwrap().to_owned(),first_metal,"First Metal backend changed during resize");
                assert_eq!(second_panel.read(cx).metal_id().unwrap().to_owned(),second_metal,"Second Metal backend changed during resize");
            }).unwrap();
            // Both real native Metal shells have acknowledged fresh text after
            // all resize/navigation checks. Preserve this actual terminal phase
            // for owned-window capture before retaining the original cleanup.
            eprintln!("KAI_METAL_SPLIT_SETTLED pid={} native_metal=true text_verified=true resize_verified=true hold_milliseconds=4000",std::process::id());
            Timer::after(Duration::from_millis(4000)).await;
            owner.update(cx,|app,_,_|{
                assert_eq!(app.active_terminal_panel().unwrap().entity_id(),first_panel.entity_id());
                assert_eq!(app.split_peer(),Some(second.id.as_str()));
            }).unwrap();
            cx.update(|cx|{
                for (panel,marker,metal) in [(&first_panel,"KAI_PTY_ONE_ALIVE",&first_metal),(&second_panel,"KAI_PTY_TWO_ALIVE",&second_metal)]{
                    assert_eq!(&panel.read(cx).metal_id().expect("Native Metal backend disappeared during capture hold").to_owned(),metal,"Native Metal backend changed during capture hold");
                    assert!(panel.read(cx).probe_has_text(marker),"Native Metal shell text changed during capture hold");
                }
            }).unwrap();
            if std::env::var_os("KAI_PROBE_TERMINAL_TOGGLE").is_some() {
                owner.update(cx, |app, window, cx| {
                    app.motion = false;
                    window.activate_window();
                    let terminal = app.active_terminal_panel().unwrap().entity_id();
                    app.select_terminal(terminal, window, cx);
                }).unwrap();
                let until = Instant::now() + Duration::from_secs(8);
                while !cx.update(|_| unified::probe_focused_terminal_toggle()).unwrap() {
                    assert!(Instant::now() < until, "Control-J did not route from the native terminal responder");
                    Timer::after(Duration::from_millis(50)).await;
                }
                loop {
                    let hidden = owner.update(cx, |app, _, _| !app.terminal_visible).unwrap();
                    if hidden { break; }
                    assert!(Instant::now() < until, "Control-J did not hide the native terminal");
                    Timer::after(Duration::from_millis(50)).await;
                }
                owner.update(cx, |app, window, cx| {
                    app.execute_shortcut("terminal", window, cx);
                    assert!(app.terminal_visible);
                    assert_eq!(app.active_terminal_panel().unwrap().entity_id(), first_panel.entity_id());
                }).unwrap();
                eprintln!("KAI_NATIVE_TERMINAL_CONTROL_J_OK");
            }
            cx.update(|cx| {
                for panel in [&first_panel, &second_panel] {
                    panel.update(cx, |panel, _| panel.stop());
                }
            }).unwrap();
            eprintln!("KAI_METAL_SPLIT_PROBE_OK");
            eprintln!("KAI_CHAT_SPLIT_PROBE_OK");
            let _ = cx.update(|cx| cx.quit());
            return;
        }
        owner.update(cx, |app, window, cx| {
            app.close_chat_split(cx);
            app.open_session(second, window, cx);
            assert_eq!(app.active_terminal_panel().unwrap().entity_id(), second_panel.entity_id());
            for panel in [&first_panel, &second_panel] { panel.update(cx, |panel, _| panel.stop()); }
        }).unwrap();
        Timer::after(Duration::from_millis(150)).await;
        let load_deadline = Instant::now() + Duration::from_secs(5);
        while owner.update(cx, |app, _, _| app.loading).unwrap() {
            assert!(Instant::now() < load_deadline, "Fixture chat did not finish loading");
            Timer::after(Duration::from_millis(25)).await;
        }
        let (chat_id, native_id, expected_root) = owner.update(cx, |app, window, cx| {
            use std::os::unix::fs::PermissionsExt;
            let id = app.active.clone().unwrap();
            let native_id = uuid::Uuid::new_v4().to_string();
            let session = app.sessions.iter_mut().find(|session| session.id == id).unwrap();
            session.external_id = Some(native_id.clone());
            let root = session.project_path.clone();
            let cli = root.join("codex-handoff-fixture");
            std::fs::write(&cli, "#!/bin/sh\nread ready\nprintf 'KAI_RESUME:%s:%s\\n' \"$1\" \"$2\"\nprintf 'KAI_CWD:%s\\n' \"$PWD\"\nread answer\n").unwrap();
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
            app.agents.iter_mut().find(|agent| agent.kind == AgentKind::Codex).unwrap().executable = Some(cli);
            app.model = None;
            app.composer.update(cx, |input, cx| input.set_text("Keep this GUI draft", cx));
            app.terminalize_chat(window, cx);
            assert!(app.chat_clis.contains_key(&id));
            (id, native_id, root)
        }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let sent = owner.update(cx, |app, _, cx| app.chat_clis.get(&chat_id).and_then(|cli| cli.fallback.as_ref()).is_some_and(|(panel, _)| panel.read(cx).probe_command("ready\r"))).unwrap();
            if sent { break; }
            assert!(Instant::now() < deadline, "Chat CLI process did not start");
            Timer::after(Duration::from_millis(50)).await;
        }
        loop {
            let ready = owner.update(cx, |app, _, cx| app.chat_clis.get(&chat_id).and_then(|cli| cli.fallback.as_ref()).is_some_and(|(panel, _)| {
                let panel = panel.read(cx);
                panel.probe_has_text(&format!("KAI_RESUME:resume:{native_id}"))
                    && panel.probe_has_text("KAI_CWD:")
                    && panel.probe_has_text(expected_root.file_name().unwrap().to_str().unwrap())
            })).unwrap();
            if ready { break; }
            assert!(Instant::now() < deadline, "Chat CLI did not resume the same conversation in its project");
            Timer::after(Duration::from_millis(50)).await;
        }
        owner.update(cx, |app, window, cx| {
            app.terminalize_chat(window, cx);
            assert!(!app.chat_clis[&chat_id].selected);
            assert_eq!(app.composer.read(cx).text(), "Keep this GUI draft");
            app.terminalize_chat(window, cx);
            assert!(app.chat_clis[&chat_id].selected);
            app.new_conversation(window, cx);
            app.model = None;
            app.composer.update(cx, |input, cx| input.set_text("Unsent new chat draft", cx));
            app.terminalize_chat(window, cx);
            assert_eq!(app.chat_clis.len(), 2);
            assert!(!app.chat_clis[keyboard::CLI_DRAFT].resumes_chat);
        }).unwrap();
        loop {
            if owner.update(cx, |app, _, cx| app.chat_clis.get(keyboard::CLI_DRAFT)
                .and_then(|cli| cli.fallback.as_ref()).is_some_and(|(panel, _)| panel.read(cx).probe_command("ready\r"))).unwrap() { break; }
            assert!(Instant::now() < deadline, "Blank CLI did not start");
            Timer::after(Duration::from_millis(50)).await;
        }
        loop {
            if owner.update(cx, |app, _, cx| {
                let panel = &app.chat_clis[keyboard::CLI_DRAFT].fallback.as_ref().unwrap().0;
                panel.read(cx).probe_has_text("KAI_RESUME::") && panel.read(cx).probe_has_text("KAI_CWD:")
                    && panel.read(cx).probe_has_text(expected_root.file_name().unwrap().to_str().unwrap())
            }).unwrap() { break; }
            assert!(Instant::now() < deadline, "Blank CLI did not start in the project without resume arguments");
            Timer::after(Duration::from_millis(50)).await;
        }
        let promoted_id = owner.update(cx, |app, window, cx| {
            let session = app.sessions.iter().find(|session| session.id == chat_id).unwrap().clone();
            app.open_session(session, window, cx);
            assert_eq!(app.composer.read(cx).text(), "Keep this GUI draft");
            assert!(app.chat_clis[&chat_id].selected);
            let first = &app.chat_clis[&chat_id].fallback.as_ref().unwrap().0;
            let promoted = app.sessions.iter().find(|session| session.title == "Unsent new chat draft").unwrap().clone();
            assert_ne!(promoted.id, keyboard::CLI_DRAFT);
            let draft = &app.chat_clis[&promoted.id].fallback.as_ref().unwrap().0;
            assert!(first.read(cx).is_visible());
            assert!(!draft.read(cx).is_visible());
            app.open_session(promoted.clone(), window, cx);
            assert_eq!(app.composer.read(cx).text(), "Unsent new chat draft");
            assert!(app.chat_clis[&chat_id].fallback.as_ref().unwrap().0.read(cx).probe_command("done\r"));
            promoted.id
        }).unwrap();
        loop {
            if owner.update(cx, |app, _, _| !app.chat_clis.contains_key(&chat_id)).unwrap() { break; }
            assert!(Instant::now() < deadline, "CLI exit did not release its chat");
            Timer::after(Duration::from_millis(50)).await;
        }
        owner.update(cx, |app, window, cx| {
            assert!(app.chat_clis.contains_key(&promoted_id));
            app.terminalize_chat(window, cx);
            assert_eq!(app.composer.read(cx).text(), "Unsent new chat draft");
            app.close_tab(&promoted_id, window, cx);
            assert!(app.chat_clis.is_empty());
        }).unwrap();
        eprintln!("KAI_CHAT_CLI_PROBE_OK");
        eprintln!("KAI_CHAT_SPLIT_PROBE_OK");
        let _ = cx.update(|cx| cx.quit());
    }).detach();
}

// Import a provider-shaped transcript through the normal history and model paths.
fn native_history_fixture(id: &str) -> PathBuf {
    use std::io::{BufWriter, Write};
    let path = PathBuf::from(std::env::var_os("ORBIT_DATA_DIR").unwrap()).join(format!("{id}.jsonl"));
    let mut file = BufWriter::new(std::fs::File::create(&path).unwrap());
    let event = serde_json::json!({
        "type": "response_item", "payload": {
            "type": "message", "role": "assistant",
            "content": [{"type": "output_text", "text": "A realistic persisted transcript. ".repeat(120)}]
        }
    });
    let line = format!("{event}\n");
    for _ in 0..3000 {
        file.write_all(line.as_bytes()).unwrap();
    }
    writeln!(file, "{}", serde_json::json!({
        "type": "turn_context", "payload": {"model": "fixture-model", "effort": "medium"}
    })).unwrap();
    for _ in 0..10 {
        file.write_all(line.as_bytes()).unwrap();
    }
    file.flush().unwrap();
    path
}

// Render the same production widgets with an isolated, deliberately long chat.
// No provider, workspace files, saved identities, or live worker are involved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NinjaProbeCapture {
    Wide,
    ResizeCompact,
    Compact,
}

async fn schedule_ninja_captures<Wait, WaitFuture, Progress, Now, Capture, CaptureFuture>(
    mut wait: Wait,
    progress: Progress,
    now: Now,
    frame_deadline: Instant,
    mut capture: Capture,
) -> bool
where
    Wait: FnMut(Duration) -> WaitFuture,
    WaitFuture: std::future::Future,
    Progress: Fn() -> usize,
    Now: Fn() -> Instant,
    Capture: FnMut(NinjaProbeCapture) -> CaptureFuture,
    CaptureFuture: std::future::Future<Output = ()>,
{
    wait(Duration::from_secs(4)).await;
    while progress() < 120 {
        if now() >= frame_deadline {
            return false;
        }
        wait(Duration::from_millis(16)).await;
    }
    // Let the final measured submission report its presentation before capture.
    wait(Duration::from_millis(1100)).await;
    capture(NinjaProbeCapture::Wide).await;
    capture(NinjaProbeCapture::ResizeCompact).await;
    wait(Duration::from_secs(4)).await;
    capture(NinjaProbeCapture::Compact).await;
    true
}

fn start_ninja_probe(owner:WindowHandle<Orbit>,cx:&mut App){
    fn frames(window:&mut Window,left:usize,at:Instant,progress:std::rc::Rc<std::cell::Cell<usize>>){
        window.refresh();window.on_next_frame(move|window,_|{
            sample("ninja_frame",at.elapsed());progress.set(progress.get()+1);
            if left>1{frames(window,left-1,Instant::now(),progress);}
        });
    }
    cx.spawn(async move|cx|{
        Timer::after(Duration::from_secs(3)).await;
        let progress=std::rc::Rc::new(std::cell::Cell::new(0));
        let frame_deadline=Instant::now()+Duration::from_secs(15);
        let _=owner.update(cx,|app,window,cx|{
            app.ninjas=crate::ninjas_ui::State::default();app.ninja.draft.node=false;
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
            window.activate_window();cx.activate(true);frames(window,120,Instant::now(),progress.clone());cx.notify();
        });
        let capture_cx = cx.clone();
        let completed = schedule_ninja_captures(
            Timer::after,
            || progress.get(),
            Instant::now,
            frame_deadline,
            move |action| {
                let mut cx = capture_cx.clone();
                async move {
                    if action == NinjaProbeCapture::Wide {
                        gpui::flush_diagnostic_frame_timing();
                        crate::unified::flush_diagnostic_native_frame_timing();
                    }
                    if action == NinjaProbeCapture::ResizeCompact {
                        let _ = owner.update(&mut cx, |_, window, _| window.resize(size(px(620.), px(720.))));
                        return;
                    }
                    let phase = if action == NinjaProbeCapture::Wide { "wide" } else { "compact" };
                    let id = owner.update(&mut cx, |_, window, _| native::window_number(window)).unwrap_or(0);
                    if id == 0 {
                        eprintln!("KAI_NINJA_RENDER_PROBE_FAILED capture window");
                        return;
                    }
                    let destination = format!("/private/tmp/kai050-ninja-{phase}.png");
                    let result = cx.background_executor().spawn(async move {
                        std::process::Command::new("/usr/sbin/screencapture")
                            .args(["-x", "-l", &id.to_string(), &destination]).status()
                    }).await;
                    if !result.is_ok_and(|status| status.success()) {
                        eprintln!("KAI_NINJA_RENDER_PROBE_FAILED capture");
                    }
                }
            },
        ).await;
        if !completed {
            eprintln!("KAI_NINJA_RENDER_PROBE_FAILED frame completion");
            let _ = cx.update(|cx| cx.quit());
            return;
        }
        // Retain a foreground bracket for the 1 Hz read-only host observer and
        // let already-submitted presentation callbacks settle before teardown.
        Timer::after(Duration::from_millis(1100)).await;
        if std::env::var_os("KAI_NINJA_IMAGE_PROBE_ENDPOINT").is_some(){
            let count=owner.update(cx,|app,_,_|app.ninjas.artifacts.values().filter(|a|a.as_ref().is_ok_and(|a|a.image.is_some()&&a.native_preview.is_some())).count()).unwrap_or(0);
            eprintln!("KAI_NINJA_IMAGE_PREVIEWS {count}");
            if count!=8{eprintln!("KAI_NINJA_RENDER_PROBE_FAILED image queue");let _=cx.update(|cx|cx.quit());return;}
        }
        eprintln!("KAI_NINJA_RENDER_PROBE_OK");let _=cx.update(|cx|cx.quit());
    }).detach();
}

// Opt-in visual regression fixture. Always use an isolated ORBIT_DATA_DIR;
// this never invokes a provider or reads a user's project files.
pub(super) fn start_daily_driver_probe(owner: WindowHandle<Orbit>, cx: &mut App) {
    let Some(output) = std::env::var_os("KAI_DAILY_DRIVER_EVIDENCE").map(PathBuf::from) else { return; };
    cx.spawn(async move |cx| {
        Timer::after(Duration::from_millis(1800)).await;
        let fixture = output.join("workspace");
        std::fs::create_dir_all(&fixture).unwrap();
        owner.update(cx, |app, window, cx| {
            app.restore_pending = None; app.discovering = false;
            app.source_projects.clear(); app.custom_projects = vec![Project { name: "Kai workspace".into(), path: fixture.clone() }];
            app.projects = app.custom_projects.clone(); app.project = fixture.clone();
            app.sidebar = true; app.sidebar_split = false; app.sidebar_sizes[0] = 224.; app.sidebar_width = 224.;
            app.ninjas.hosts.clear(); app.ninjas.snapshots.clear();
            let session = SessionSummary { id: "daily-driver-preview".into(), title: "Workspace improvements".into(), agent: AgentKind::Codex,
                project_path: fixture.clone(), execution_path: None, updated_at: 1_800_000_000, external_id: None, transcript_path: None };
            let mut second = session.clone(); second.id = "daily-driver-review".into(); second.title = "Review the finished changes".into();
            app.sessions = vec![session.clone(), second]; app.visible_sessions = app.sessions.clone();
            app.engine.save(session.clone(), "user", "Make the agent workspace easier to use every day.".into());
            app.engine.save(session.clone(), "assistant", "The sidebar remembers its width. Click the chat title to switch conversations inside this project.\n\nYour prompt and attached image stay in separate rows above the controls.".into());
            app.open_session(session, window, cx);
            app.rebuild_workspace_sessions();
            app.expanded_projects.insert(fixture.clone()); app.rebuild_project_tree();
        }).unwrap();
        Timer::after(Duration::from_millis(800)).await;
        owner.update(cx, |app, _, cx| {
            let path = output.join("attachment.png");
            image::RgbaImage::from_pixel(160, 100, image::Rgba([100, 140, 210, 255])).save(&path).unwrap();
            app.attachments.push(attachments::import(&path, &app.attachment_dir, &std::sync::atomic::AtomicBool::new(false)).unwrap());
            app.composer.update(cx, |input, cx| input.set_text(&"Please preserve the conversation, keep my draft readable, and review the changes when the task finishes. ".repeat(22), cx));
            cx.notify();
        }).unwrap();
        for (phase, picker) in [("composer", false), ("chat-picker", true), ("usage", false)] {
            if picker { owner.update(cx, |app, window, cx| {
                app.toggle_chat_switcher(app.active.clone(), point(px(app.sidebar_width + 90.), px(72.)), window, cx);
            }).unwrap(); }
            if phase=="usage"{
                owner.update(cx,|app,window,cx|{app.chat_switcher=None;app.open_usage_dashboard(window,cx);}).unwrap();
                Timer::after(Duration::from_secs(8)).await;
            }
            Timer::after(Duration::from_millis(650)).await;
            let number = owner.update(cx, |_, window, _| native::window_number(window)).unwrap();
            let path = output.join(format!("{phase}.png"));
            let result = cx.background_executor().spawn(async move {
                std::process::Command::new("/usr/sbin/screencapture").args(["-x", "-l", &number.to_string(), path.to_str().unwrap()]).status()
            }).await;
            assert!(result.is_ok_and(|status| status.success()), "Could not capture fixture window");
        }
        eprintln!("KAI_DAILY_DRIVER_PROBE_OK");
        let _ = owner.update(cx, |app, window, cx| app.quit_after_save(window, cx));
    }).detach();
}

#[cfg(test)]
mod ninja_capture_schedule_tests {
    use super::{schedule_ninja_captures, NinjaProbeCapture};
    use std::time::{Duration, Instant};
    use std::{cell::Cell, cell::RefCell, future::Future, pin::Pin, rc::Rc, task::{Context, Poll, Waker}};

    struct HeldTimer {
        clock: Rc<Cell<Duration>>,
        due: Duration,
    }
    impl Future for HeldTimer {
        type Output = ();
        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            if self.clock.get() >= self.due { Poll::Ready(()) } else { Poll::Pending }
        }
    }

    #[test]
    fn ninja_capture_scheduler_holds_screenshots_until_all_callbacks_and_drain() {
        let clock = Rc::new(Cell::new(Duration::ZERO));
        let progress = Cell::new(0);
        let events = Rc::new(RefCell::new(Vec::new()));
        let base = Instant::now();
        let wait_clock = clock.clone();
        let event_log = events.clone();
        let event_clock = clock.clone();
        let mut task = std::pin::pin!(schedule_ninja_captures(
            move |delay| HeldTimer { clock: wait_clock.clone(), due: wait_clock.get() + delay },
            || progress.get(), || base + clock.get(), base + Duration::from_secs(15),
            move |action| { event_log.borrow_mut().push((action, event_clock.get())); std::future::ready(()) },
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(task.as_mut().poll(&mut cx).is_pending());
        clock.set(Duration::from_secs(4));
        progress.set(119);
        assert!(task.as_mut().poll(&mut cx).is_pending());
        assert!(events.borrow().is_empty(), "Screenshot or resize entered the measured callback phase");
        clock.set(Duration::from_millis(4016));
        progress.set(120);
        assert!(task.as_mut().poll(&mut cx).is_pending());
        assert!(events.borrow().is_empty(), "Final submission needs its presentation drain");
        clock.set(Duration::from_millis(5115));
        assert!(task.as_mut().poll(&mut cx).is_pending());
        assert!(events.borrow().is_empty());
        clock.set(Duration::from_millis(5116));
        assert!(task.as_mut().poll(&mut cx).is_pending());
        assert_eq!(*events.borrow(), vec![(NinjaProbeCapture::Wide, Duration::from_millis(5116)), (NinjaProbeCapture::ResizeCompact, Duration::from_millis(5116))]);
        clock.set(Duration::from_millis(9116));
        assert_eq!(task.as_mut().poll(&mut cx), Poll::Ready(true));
        assert_eq!(events.borrow().last(), Some(&(NinjaProbeCapture::Compact, Duration::from_millis(9116))));
    }

    #[test]
    fn ninja_capture_scheduler_retains_wide_minimum_when_callbacks_finish_early() {
        let clock = Rc::new(Cell::new(Duration::ZERO));
        let events = Rc::new(RefCell::new(Vec::new()));
        let base = Instant::now();
        let wait_clock = clock.clone();
        let event_log = events.clone();
        let mut task = std::pin::pin!(schedule_ninja_captures(
            move |delay| HeldTimer { clock: wait_clock.clone(), due: wait_clock.get() + delay },
            || 120, || base + clock.get(), base + Duration::from_secs(15),
            move |action| { event_log.borrow_mut().push(action); std::future::ready(()) },
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(task.as_mut().poll(&mut cx).is_pending());
        clock.set(Duration::from_millis(3999));
        assert!(task.as_mut().poll(&mut cx).is_pending());
        assert!(events.borrow().is_empty());
        clock.set(Duration::from_secs(4));
        assert!(task.as_mut().poll(&mut cx).is_pending());
        assert!(events.borrow().is_empty());
        clock.set(Duration::from_millis(5100));
        assert!(task.as_mut().poll(&mut cx).is_pending());
        assert_eq!(*events.borrow(), vec![NinjaProbeCapture::Wide, NinjaProbeCapture::ResizeCompact]);
    }

    #[test]
    fn ninja_capture_scheduler_deadline_aborts_before_any_screenshot() {
        let clock = Rc::new(Cell::new(Duration::ZERO));
        let events = Rc::new(RefCell::new(Vec::new()));
        let base = Instant::now();
        let wait_clock = clock.clone();
        let event_log = events.clone();
        let mut task = std::pin::pin!(schedule_ninja_captures(
            move |delay| HeldTimer { clock: wait_clock.clone(), due: wait_clock.get() + delay },
            || 119, || base + clock.get(), base + Duration::from_secs(15),
            move |action| { event_log.borrow_mut().push(action); std::future::ready(()) },
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(task.as_mut().poll(&mut cx).is_pending());
        clock.set(Duration::from_secs(15));
        assert_eq!(task.as_mut().poll(&mut cx), Poll::Ready(false));
        assert!(events.borrow().is_empty());
    }
}
