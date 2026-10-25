//! Conversational Codex voice. Media is UI-thread owned; all tools and text use
//! the ordinary native run, approval and persistence paths of this same chat.
use super::*;
use crate::voice_bridge::{Activity, Media, MediaEvent};

pub(super) struct Call {
    pub owner: String,
    pub session_id: String,
    pub media: Media,
    pub activity: Activity,
    pub active: bool,
    pub muted: bool,
    backend_started: bool,
    media_connected: bool,
    launched: bool,
    stopping: bool,
    surface: Option<String>,
    pending_offer: Option<String>,
}
impl Call {
    pub fn connecting(&self, active: Option<&str>) -> bool {
        !self.active && !self.stopping && self.owns_current_chat(active)
    }

    pub fn owns_current_chat(&self, active: Option<&str>) -> bool {
        same_surface(self.surface.as_deref(), active)
    }
    pub fn scene(&self) -> &'static str {
        if self.stopping {
            "waiting"
        } else if !self.active {
            "thinking"
        } else if self.muted {
            "waiting"
        } else {
            match self.activity {
                Activity::Listening => "listening",
                Activity::Thinking => "thinking",
                Activity::Speaking => "listening",
            }
        }
    }
    pub fn caption(&self) -> &'static str {
        if self.stopping {
            "Ending voice…"
        } else if !self.active {
            "Connecting voice…"
        } else if self.muted {
            "Microphone muted"
        } else {
            match self.activity {
                Activity::Listening => "Listening",
                Activity::Thinking => "Thinking",
                Activity::Speaking => "Speaking",
            }
        }
    }
    fn activate(&mut self) -> anyhow::Result<()> {
        if self.backend_started && self.media_connected && !self.active && !self.stopping {
            self.media.activate()?;
            self.active = true;
            self.activity = Activity::Listening;
        }
        Ok(())
    }
}
fn same_surface(expected: Option<&str>, active: Option<&str>) -> bool {
    expected == active
}
struct LaunchGate<'a> {
    surface: Option<&'a str>,
    active: Option<&'a str>,
    pending: bool,
    occupied: bool,
    capacity: bool,
    historical: bool,
    codex: bool,
}
fn validate_launch(gate: LaunchGate<'_>) -> anyhow::Result<()> {
    anyhow::ensure!(
        same_surface(gate.surface, gate.active),
        "The conversation changed before voice connected"
    );
    anyhow::ensure!(
        !gate.pending && !gate.occupied && gate.capacity,
        "This conversation started other work before voice connected"
    );
    anyhow::ensure!(
        !gate.historical,
        "The latest conversation page could not be loaded before voice connected"
    );
    anyhow::ensure!(
        gate.codex,
        "Choose Codex before starting conversational voice"
    );
    Ok(())
}
impl Orbit {
    pub(super) fn toggle_voice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_voice_prompt(None, window, cx)
    }
    pub(super) fn start_voice_prompt(
        &mut self,
        initial: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.voice_call.is_some() {
            self.stop_voice(cx);
            return;
        }
        if self.quit_pending {
            return;
        }
        if self.loading
            || self.loading_earlier
            || self.starting
            || self.checkpoint_busy > 0
            || self.handoff_pending.is_some()
            || self.runs.len() + self.team_starting.len() >= 4
            || self.active.as_ref().is_some_and(|id| {
                self.runs.contains_key(id)
                    || self.settling.contains_key(id)
                    || self.finalizing.contains(id)
            })
        {
            self.notice =
                Some("Finish this conversation’s current work before starting voice.".into());
            cx.notify();
            return;
        }
        if self.agent != AgentKind::Codex
            || self
                .active
                .as_ref()
                .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
                .is_some_and(|s| s.agent != AgentKind::Codex)
        {
            self.notice =
                Some("Choose Codex in a Codex conversation to start conversational voice.".into());
            cx.notify();
            return;
        }
        if !self
            .agents
            .iter()
            .any(|a| a.kind == AgentKind::Codex && a.executable.is_some())
        {
            self.notice = Some("Install Codex and sign in to use conversational voice.".into());
            cx.notify();
            return;
        }
        self.stop_dictation(cx);
        // Load only the bounded latest page before starting a new live stream.
        // Offer delivery is deferred until this load settles; earlier history
        // can never splice over a newly started voice run.
        if (self.history_offset > 0 || self.navigation_history)
            && let Some(session) = self
                .active
                .as_ref()
                .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
                .cloned()
        {
            self.navigation_previous = Some(self.navigation_history);
            self.navigation_history = false;
            self.restore_scroll = None;
            self.history_anchor = None;
            self.loading = true;
            self.generation += 1;
            self.engine.load(session, self.generation);
        }
        let session_id = uuid::Uuid::new_v4().to_string();
        match Media::start(window, &session_id) {
            Ok(media) => {
                if let Some(prompt) = initial {
                    if let Err(error) = media.initial_prompt(&prompt) {
                        self.notice = Some(error.to_string());
                        cx.notify();
                        return;
                    }
                }
                let wake = media.wake.clone();
                let owner = self
                    .active
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                self.voice_call = Some(Call {
                    owner,
                    session_id: session_id.clone(),
                    media,
                    activity: Activity::Thinking,
                    active: false,
                    muted: false,
                    backend_started: false,
                    media_connected: false,
                    launched: false,
                    stopping: false,
                    surface: self.active.clone(),
                    pending_offer: None,
                });
                let generation = session_id.clone();
                cx.spawn_in(window, async move |this, cx| {
                    while wake.recv().await.is_ok() {
                        if !matches!(
                            this.update_in(cx, |this, _, cx| this
                                .drain_voice_media(&generation, cx)),
                            Ok(true)
                        ) {
                            break;
                        }
                    }
                })
                .detach();
                // Signaling has a startup deadline; a connected conversation
                // has no duration cap. The token rejects timers from old calls.
                cx.spawn(async move |this,cx| {
                    Timer::after(Duration::from_secs(45)).await;
                    let _=this.update(cx,|this,cx|{
                        if this.voice_call.as_ref().is_some_and(|call|call.session_id==session_id&&!call.active&&!call.stopping){
                            this.stop_voice(cx);this.notice=Some("Voice did not connect. Check microphone access and your Codex connection, then start voice again.".into());cx.notify();
                        }
                    });
                }).detach();
            }
            Err(error) => self.notice = Some(error.to_string()),
        }
        cx.notify();
    }
    fn drain_voice_media(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        if !self
            .voice_call
            .as_ref()
            .is_some_and(|call| call.session_id == id && !call.stopping)
        {
            return false;
        }
        let mut dirty = false;
        loop {
            let event = self
                .voice_call
                .as_ref()
                .and_then(|call| call.media.try_recv().ok());
            let Some(event) = event else { break };
            dirty = true;
            let result = match event {
                MediaEvent::Offer { session_id, sdp } if session_id == id => {
                    self.voice_call.as_mut().unwrap().pending_offer = Some(sdp);
                    Ok(())
                }
                MediaEvent::Connected { session_id } if session_id == id => {
                    let call = self.voice_call.as_mut().unwrap();
                    call.media_connected = true;
                    call.activate()
                }
                MediaEvent::Activity {
                    session_id,
                    activity,
                } if session_id == id => {
                    self.voice_call.as_mut().unwrap().activity = activity;
                    Ok(())
                }
                MediaEvent::Error {
                    session_id,
                    message,
                } if session_id == id => Err(anyhow::anyhow!(message)),
                _ => Ok(()),
            };
            if let Err(error) = result {
                self.stop_voice(cx);
                self.notice = Some(error.to_string());
                cx.notify();
                return false;
            }
        }
        if dirty {
            self.flush_unified_voice_context();
            self.advance_voice(cx);
            cx.notify();
        }
        true
    }
    pub(super) fn advance_voice(&mut self, cx: &mut Context<Self>) {
        let Some(call) = self.voice_call.as_ref() else {
            return;
        };
        if call.launched
            || call.stopping
            || call.pending_offer.is_none()
            || self.loading
            || self.loading_earlier
        {
            return;
        }
        let sdp = self
            .voice_call
            .as_mut()
            .unwrap()
            .pending_offer
            .take()
            .unwrap();
        if let Err(error) = self.launch_voice_offer(sdp, cx) {
            self.stop_voice(cx);
            self.notice = Some(error.to_string());
            cx.notify();
        }
    }
    fn launch_voice_offer(&mut self, sdp: String, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let call = self
            .voice_call
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Voice was stopped"))?;
        if call.launched {
            return Ok(());
        }
        let id = call.owner.clone();
        validate_launch(LaunchGate {
            surface: call.surface.as_deref(),
            active: self.active.as_deref(),
            pending: self.loading
                || self.loading_earlier
                || self.starting
                || self.handoff_pending.is_some()
                || self.checkpoint_busy > 0,
            occupied: self.runs.contains_key(&id)
                || self.settling.contains_key(&id)
                || self.finalizing.contains(&id),
            capacity: self.runs.len() + self.team_starting.len() < 4,
            historical: self.history_offset > 0 || self.navigation_history,
            codex: self.agent == AgentKind::Codex,
        })?;
        let params = voice::Start {
            session_id: call.session_id.clone(),
            sdp,
            voice: voice::configured_voice(&self.voice_choice),
        }
        .params()?;
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .unwrap_or_else(|| SessionSummary {
                id: id.clone(),
                title: "Voice conversation".into(),
                agent: AgentKind::Codex,
                project_path: self.project.clone(),
                updated_at: chrono::Utc::now().timestamp(),
                external_id: None,
                transcript_path: None,
                execution_path: self.new_execution_path.clone(),
            });
        anyhow::ensure!(
            session.agent == AgentKind::Codex,
            "The conversation’s agent changed before voice connected"
        );
        let project = session
            .execution_path
            .clone()
            .unwrap_or_else(|| session.project_path.clone());
        let executable = self
            .agents
            .iter()
            .find(|a| a.kind == AgentKind::Codex)
            .and_then(|a| a.executable.clone())
            .ok_or_else(|| anyhow::anyhow!("Codex is unavailable"))?;
        let request = RunRequest { reasoning_effort:None,
            agent: AgentKind::Codex,
            executable,
            project: project.clone(),
            prompt: String::new(),
            resume_id: session.external_id.clone(),
            fork_session: imported_codex_needs_fork(&session)
                || self.fork_next.as_deref() == Some(&id),
            model: self.model.clone(),
            images: Vec::new(),
            allow_edits: self.allow_edits,
            codex_approvals: self.codex_approvals,
            trust_workspace: self
                .trusted_workspaces
                .contains(&(AgentKind::Codex, project)),
        };
        let handle = runtime::launch_native_command(request, "thread/realtime/start", params)?;
        let wake = handle.wake.clone();
        if !self.sessions.iter().any(|s| s.id == id) {
            self.sessions.insert(0, session.clone());
            self.visible_sessions = self
                .sessions
                .iter()
                .filter(|s| !self.hidden_sessions.contains(&s.id))
                .cloned()
                .collect();
            self.rebuild_workspace_sessions();
            self.engine.update_session(session);
        }
        self.following_bottom = true;
        self.active = Some(id.clone());
        if !self.open_tabs.contains(&id) {
            self.open_tabs.push(id.clone())
        }
        self.runs.insert(
            id.clone(),
            Running {
                agent: AgentKind::Codex,
                submitted_prompt: String::new(),
                handle,
                text: String::new(),
                live_prefix_omitted: false,
                status: "Connecting Codex voice".into(),
                stream_start: None,
                revision: 0,
                parsed_revision: 0,
                parse_pending: false,
                live_blocks: Vec::new(),
                live_earlier: false,
                parse_epoch: uuid::Uuid::new_v4(),
                retry: None,
                submitted_attachments: Vec::new(),
            },
        );
        self.voice_call.as_mut().unwrap().launched = true;
        self.fork_next = None;
        cx.spawn(async move |this, cx| {
            loop {
                if wake.recv().await.is_ok() {
                    Timer::after(Duration::from_millis(16)).await;
                }
                if !matches!(this.update(cx, |this, cx| this.poll(&id, cx)), Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        Ok(())
    }
    pub(super) fn stop_voice(&mut self, cx: &mut Context<Self>) {
        let Some(call) = self.voice_call.as_mut() else {
            return;
        };
        if call.stopping {
            return;
        }
        call.media.stop();
        call.active = false;
        call.stopping = true;
        let session_id = call.session_id.clone();
        let owner = call.owner.clone();
        if !call.launched || !self.runs.contains_key(&owner) {
            self.voice_call.take();
            cx.notify();
            return;
        }
        let run = self.runs.get(&owner).unwrap();
        let epoch = run.parse_epoch;
        if let Err(error) = run
            .handle
            .native_command("thread/realtime/stop", serde_json::json!({}))
        {
            run.handle.cancel();
            self.voice_call.take();
            self.notice = Some(error.to_string());
            cx.notify();
            return;
        }
        // Close tracks immediately, then consume the authoritative transcript
        // flush normally. A backing coding turn may finish after voice closes.
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(2)).await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .voice_call
                    .as_ref()
                    .is_some_and(|call| call.session_id == session_id && call.stopping)
                {
                    if let Some(run) = this.runs.get(&owner).filter(|run| run.parse_epoch == epoch)
                    {
                        run.handle.cancel();
                    }
                    this.voice_call.take();
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn handle_voice_signal(&mut self, signal: voice::Signal, cx: &mut Context<Self>) {
        let Some(call) = &mut self.voice_call else {
            return;
        };
        if call.stopping && !matches!(signal, voice::Signal::Closed { .. }) {
            return;
        }
        let result = match signal {
            voice::Signal::Started { session_id } if session_id == call.session_id => {
                call.backend_started = true;
                call.activate()
            }
            voice::Signal::Answer { session_id, sdp } if session_id == call.session_id => {
                call.media.answer(&sdp)
            }
            // Native runtime text already streams into the chat transcript.
            // Do not keep a second copy for a duplicate composer card.
            voice::Signal::Transcript { .. } => Ok(()),
            voice::Signal::Closed { session_id, reason } if session_id == call.session_id => {
                // The backing coding turn may still be finishing; the native
                // run retains it and its requests after the microphone closes.
                self.voice_call.take();
                if let Some(reason) = reason.filter(|r| {
                    !matches!(
                        r.as_str(),
                        "userRequested" | "clientRequested" | "stopped" | "completed"
                    )
                }) {
                    self.notice = Some(reason)
                }
                cx.notify();
                return;
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.stop_voice(cx);
            self.notice = Some(error.to_string())
        }
        self.flush_unified_voice_context();
        cx.notify();
    }
    pub(super) fn voice_button(&self, cx: &Context<Self>) -> AnyElement {
        if self.agent != AgentKind::Codex && self.voice_call.is_none() {
            return div().into_any_element();
        }
        div()
            .id("conversation-voice")
            .debug_selector(|| "conversation-voice".into())
            .h(px(28.))
            .px_2()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .text_size(theme::ui_px(11.))
            .rounded_md()
            .cursor_pointer()
            .bg(rgba(if self.voice_call.is_some() {
                0x9fbaee26
            } else {
                0xffffff00
            }))
            .hover(|s| s.bg(rgba(GLASS_HOVER)))
            .child(icon(
                "assets/kai/status/listening.svg",
                14.,
                if self.voice_call.is_some() {
                    ACCENT
                } else {
                    MUTED
                },
            ))
            .child("RealTime")
            .child(icon("chevron-down", 9., MUTED))
            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                this.toggle_panel_from_click(
                    Modal::Realtime,
                    event.position() + point(px(-12.), px(-20.)),
                    true,
                    window,
                    cx,
                );
            }))
            .into_any_element()
    }
    pub(super) fn realtime_settings(&self, cx: &Context<Self>) -> AnyElement {
        let mut panel = div()
            .id("realtime-settings")
            .p_2()
            .flex()
            .flex_col()
            .gap_2()
            .text_size(theme::ui_px(12.));
        if let Some(call) = &self.voice_call {
            panel = panel
                .child(div().px_2().text_color(rgb(MUTED)).child(call.caption()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .id("voice-mute")
                                .debug_selector(|| "voice-mute".into())
                                .cursor_pointer()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                .child(if call.muted { "Unmute" } else { "Mute" })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let result = this.voice_call.as_mut().map(|call| {
                                        call.muted = !call.muted;
                                        call.media.mute(call.muted)
                                    });
                                    if let Some(Err(error)) = result {
                                        this.notice = Some(error.to_string());
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id("voice-interrupt")
                                .debug_selector(|| "voice-interrupt".into())
                                .cursor_pointer()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                .child("Interrupt")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(call) = &this.voice_call {
                                        let _ = call.media.interrupt();
                                        if let Some(run) = this.runs.get(&call.owner) {
                                            let _ = run.handle.interrupt();
                                        }
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .id("voice-end")
                                .debug_selector(|| "voice-end".into())
                                .cursor_pointer()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_color(rgb(0xe5a3a3))
                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                .child("End")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.stop_voice(cx);
                                    this.dismiss_panel(window, cx);
                                })),
                        ),
                );
        } else {
            panel = panel.child(
                div()
                    .id("voice-start")
                    .debug_selector(|| "voice-start".into())
                    .h(px(32.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon("assets/kai/status/listening.svg", 16., ACCENT))
                    .child("Start RealTime")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.dismiss_panel(window, cx);
                        this.toggle_voice(window, cx);
                    })),
            );
        }
        panel
            .child(
                div()
                    .mt_1()
                    .px_2()
                    .text_size(theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .child("Voice"),
            )
            .child(
                div().flex().flex_wrap().gap_1().children(
                    std::iter::once("default")
                        .chain(voice::VOICES.iter().copied())
                        .map(|name| {
                            div()
                                .id(SharedString::from(format!("voice-name-{name}")))
                                .debug_selector(move || format!("voice-name-{name}"))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(rgba(if self.voice_choice == name {
                                    GLASS_HOVER
                                } else {
                                    0xffffff00
                                }))
                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                .child(name)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.voice_choice = name.into();
                                    this.queue_workspace_save(cx);
                                    cx.notify();
                                }))
                        }),
                ),
            )
            .when(self.voice_call.is_some(), |panel| {
                panel.child(
                    div()
                        .px_2()
                        .text_size(theme::ui_px(10.))
                        .text_color(rgb(MUTED))
                        .child("Voice changes apply to your next session."),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn delayed_voice_offer_cannot_replace_work_or_move_between_chats() {
        let ready = || LaunchGate {
            surface: Some("chat"),
            active: Some("chat"),
            pending: false,
            occupied: false,
            capacity: true,
            historical: false,
            codex: true,
        };
        assert!(validate_launch(ready()).is_ok());
        assert!(
            validate_launch(LaunchGate {
                occupied: true,
                ..ready()
            })
            .is_err()
        );
        assert!(
            validate_launch(LaunchGate {
                active: Some("other"),
                ..ready()
            })
            .is_err()
        );
        assert!(
            validate_launch(LaunchGate {
                active: None,
                ..ready()
            })
            .is_err()
        );
        assert!(
            validate_launch(LaunchGate {
                surface: None,
                active: Some("newly-sent-chat"),
                ..ready()
            })
            .is_err()
        );
        assert!(
            validate_launch(LaunchGate {
                surface: None,
                active: None,
                ..ready()
            })
            .is_ok()
        );
        assert!(
            validate_launch(LaunchGate {
                pending: true,
                ..ready()
            })
            .is_err()
        );
        assert!(
            validate_launch(LaunchGate {
                historical: true,
                ..ready()
            })
            .is_err()
        );
        assert!(
            validate_launch(LaunchGate {
                capacity: false,
                ..ready()
            })
            .is_err()
        );
        assert!(
            validate_launch(LaunchGate {
                codex: false,
                ..ready()
            })
            .is_err()
        );
    }
}

impl Orbit {
    fn flush_unified_voice_context(&mut self) {
        let Some(call) = self.voice_call.as_ref().filter(|c| c.active) else {
            return;
        };
        let Some(run) = self.runs.get(&call.owner) else {
            return;
        };
        if let Some(context) = self.unified_voice_context.take() {
            if let Err(error) = run.handle.steer(&context) {
                self.notice = Some(format!("Screen context was not accepted: {error}"));
            }
        }
    }
}
