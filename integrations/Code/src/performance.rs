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
