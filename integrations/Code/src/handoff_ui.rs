//! Composer-only agent selection; context transfer begins with explicit Send.
use super::*;

/// Immutable work captured by one explicit Send. Async handoff preparation
/// resumes this value, never the current composer or mutable provider settings.
pub(super) struct Submission {
    pub session: SessionSummary,
    pub request: RunRequest,
    pub prompt: String,
    pub original_prompt: String,
    pub saved_prompt: String,
    pub attachments: Vec<attachments::Attachment>,
    pub references: Vec<context_refs::Reference>,
    pub context_scope: String,
    pub native_command: bool,
    pub queued_prompt_id: Option<String>,
}
pub(super) struct Pending {
    pub token: uuid::Uuid,
    pub source: SessionSummary,
    target: AgentKind,
    pub preparing: bool,
    pub ready: Option<handoff::Packet>,
    binding: bool,
    bound: Option<SessionSummary>,
    submission: Submission,
}
impl Orbit {
    pub(super) fn remember_handoff(&mut self, id: String, packet: std::sync::Arc<handoff::Packet>) {
        self.handoff_packets.insert(id.clone(), packet);
        if self.handoff_packets.len() > 8
            && let Some(old) = self
                .handoff_packets
                .keys()
                .find(|key| **key != id && self.active.as_ref() != Some(key))
                .cloned()
        {
            self.handoff_packets.remove(&old);
        }
    }

    pub(super) fn open_handoff(
        &mut self,
        argument: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !argument.is_empty() {
            if let Some(target) = self
                .agents
                .iter()
                .find(|agent| {
                    agent.available
                        && (agent.kind.command().eq_ignore_ascii_case(argument)
                            || agent.kind.name().eq_ignore_ascii_case(argument))
                })
                .map(|a| a.kind)
            {
                self.request_handoff(target, window, cx);
            } else {
                self.notice = Some("Choose an installed agent from /handoff.".into());
                cx.notify();
            }
        } else {
            self.show_modal(Modal::Handoff, window, cx);
        }
    }
    /// Choosing a destination only edits the composer. It must not read saved
    /// context, bind a session, cancel a run, or start provider inference.
    pub(super) fn request_handoff(
        &mut self,
        target: AgentKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .agents
            .iter()
            .any(|agent| agent.kind == target && agent.available && agent.executable.is_some())
        {
            self.notice = Some(format!(
                "Install and sign in to {} before continuing there.",
                target.name()
            ));
            cx.notify();
            return;
        }
        if self.agent != target {
            self.stop_voice(cx);
            self.agent_models
                .insert(self.agent.command(), self.model.clone());
            self.agent = target;
            self.model = self.agent_models.get(target.command()).cloned().flatten();
        }
        self.dismiss_panel(window, cx);
        self.dismiss_agent_picker();
        cx.notify();
    }
    pub(super) fn begin_send_handoff(
        &mut self,
        submission: Submission,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = submission.session.clone();
        let target = submission.request.agent;
        self.handoff_pending = Some(Pending {
            token: uuid::Uuid::new_v4(),
            source,
            target,
            preparing: false,
            ready: None,
            binding: false,
            bound: None,
            submission,
        });
        self.notice = Some(format!(
            "Sending the captured message to {}… Any edits stay in your draft.",
            target.name()
        ));
        self.advance_handoff(window, cx);
        cx.notify();
    }
    pub(super) fn cancel_send_handoff(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pending) = self.handoff_pending.take() else {
            return false;
        };
        self.hold_queue(&pending.source.id, cx);
        self.notice =
            Some("Pending send canceled. Your draft and source conversation are preserved.".into());
        cx.notify();
        true
    }
    pub(super) fn track_settling(&mut self, id: &str, handle: &RunHandle, cx: &mut Context<Self>) {
        if handle.settled.is_closed() {
            return;
        }
        let settled = handle.settled.clone();
        self.settling.insert(id.to_owned(), settled.clone());
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let _ = settled.recv().await;
            let _ = this.update(cx, |this, cx| {
                this.settling.remove(&id);
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn handoff_bound(
        &mut self,
        token: uuid::Uuid,
        result: Result<(SessionSummary, std::sync::Arc<handoff::Packet>), String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok((session, packet)) => {
                self.remember_handoff(session.id.clone(), packet);
                let previous = self
                    .sessions
                    .iter()
                    .find(|s| s.id == session.id)
                    .map(|s| s.agent);
                if self.active.as_deref() == Some(&session.id)
                    && let Some(previous) = previous
                {
                    stamp_row_providers(&mut self.rows, previous);
                }
                // Model/agent selections made while preparation was in flight
                // belong to the next composer turn, not this captured request.
                if let Some(pending) = self.handoff_pending.as_mut().filter(|p| p.token == token) {
                    self.session_models
                        .insert(session.id.clone(), pending.submission.request.model.clone());
                    pending.bound = Some(session.clone());
                }
                self.usage.remove(&session.id);
                self.sessions.retain(|s| s.id != session.id);
                self.sessions.insert(0, session);
                self.rebuild_workspace_sessions();
            }
            Err(error)
                if self
                    .handoff_pending
                    .as_ref()
                    .is_some_and(|p| p.token == token) =>
            {
                if let Some(pending) = self.handoff_pending.take() {
                    self.hold_queue(&pending.source.id, cx);
                }
                self.notice = Some(format!(
                    "Could not switch agents: {error}. Your conversation and draft are preserved."
                ));
            }
            Err(_) => {}
        }
        cx.notify();
    }
    pub(super) fn advance_handoff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = &mut self.handoff_pending else {
            return;
        };
        if self.active.as_deref() != Some(&pending.source.id) {
            let id = pending.source.id.clone();
            self.handoff_pending = None;
            self.hold_queue(&id, cx);
            self.notice = Some("Pending send canceled because the source conversation changed. Its draft is preserved.".into());
            return;
        }
        // Selection never stops the source. Explicit Send uses the ordinary
        // queue/busy guards; if source cleanup races, wait without cancellation.
        if self.runs.contains_key(&pending.source.id)
            || self.starting
            || self.finalizing.contains(&pending.source.id)
            || self.settling.contains_key(&pending.source.id)
        {
            return;
        }
        if !pending.binding
            && let Some(packet) = pending.ready.as_ref()
        {
            let mut session = pending.source.clone();
            session.agent = pending.target;
            session.external_id = None;
            session.transcript_path = None;
            session.updated_at = chrono::Utc::now().timestamp();
            pending.binding = true;
            self.engine.switch_agent(
                session,
                std::sync::Arc::new(packet.clone()),
                pending.submission.request.model.clone(),
                pending.token,
            );
            return;
        }
        if pending.bound.is_some() {
            let mut pending = self.handoff_pending.take().unwrap();
            pending.submission.session = pending.bound.take().unwrap();
            pending.submission.request.resume_id = pending.submission.session.external_id.clone();
            pending.submission.request.fork_session =
                imported_codex_needs_fork(&pending.submission.session);
            self.submit_captured(pending.submission, window, cx);
            return;
        }
        if !pending.preparing {
            pending.preparing = true;
            self.engine
                .prepare_handoff(pending.source.clone(), pending.token, false);
        }
    }
    pub(super) fn handoff_panel(&self, cx: &Context<Self>) -> AnyElement {
        let source = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id));
        let mut body = div().p_2().flex().flex_col().gap_1().child(div().p_2().text_size(crate::theme::ui_px(11.)).text_color(rgb(MUTED)).child(if source.is_some() {
            "Continue in the same workspace with saved objective and recent context. Your next Send transfers context with your instruction. Running turns continue unchanged. Full native sessions remain saved; hidden state and live tools cannot move between providers."
        } else { "Choose an installed coding agent." }));
        for agent in self.agents.iter().filter(|a| {
            a.available && a.executable.is_some() && source.is_none_or(|s| s.agent != a.kind)
        }) {
            let kind = agent.kind;
            body = body.child(
                div()
                    .id(SharedString::from(format!(
                        "handoff-agent-{}",
                        kind.command()
                    )))
                    .debug_selector(move || format!("handoff-agent-{}", kind.command()))
                    .h(px(38.))
                    .px_3()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_3()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon(provider_asset(kind), 18., agent_color(kind)))
                    .child(
                        div()
                            .text_size(crate::theme::ui_px(12.))
                            .child(format!("Continue with {}", kind.name())),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.request_handoff(kind, window, cx)
                    })),
            );
        }
        if !self.agents.iter().any(|a| {
            a.available && a.executable.is_some() && source.is_none_or(|s| s.agent != a.kind)
        }) {
            body = body.child(
                div()
                    .p_3()
                    .text_size(crate::theme::ui_px(12.))
                    .text_color(rgb(MUTED))
                    .child("No other installed CLI was detected."),
            );
        }
        body.into_any_element()
    }
}
