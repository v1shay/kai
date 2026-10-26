//! Requests stay attached to their owning run, outside the rolling transcript.
use super::*;
use runtime::{NativeAnswer, NativeRequest, NativeRequestKind};
use std::collections::BTreeMap;

pub(super) struct LiveRequest {
    pub owner: String,
    pub broker: Option<broker_ui::RequestOwner>,
    epoch: uuid::Uuid,
    request: NativeRequest,
    inputs: HashMap<String, Entity<TextInput>>,
    selected: BTreeMap<String, Vec<String>>,
    subscriptions: Vec<Subscription>,
    sending: bool,
    started: Instant,
}
impl LiveRequest {
    pub(super) fn indicator_scene(&self) -> &'static str {
        if matches!(self.request.kind, NativeRequestKind::Questions { .. }) {
            "question"
        } else {
            "permission"
        }
    }

    pub(super) fn retry(&mut self) {
        self.sending = false;
    }
    pub fn new(owner: &str, epoch: uuid::Uuid, request: NativeRequest) -> Self {
        Self {
            owner: owner.into(),
            broker: None,
            epoch,
            request,
            inputs: HashMap::new(),
            selected: BTreeMap::new(),
            subscriptions: Vec::new(),
            sending: false,
            started: Instant::now(),
        }
    }
    pub(super) fn for_child(
        owner: &str,
        request: NativeRequest,
        binding: broker_ui::RequestOwner,
    ) -> Self {
        let mut pending = Self::new(owner, uuid::Uuid::new_v4(), request);
        pending.broker = Some(binding);
        pending
    }
    pub(super) fn current(&self, runs: &HashMap<String, Running>) -> bool {
        if let Some(binding) = &self.broker {
            return binding.live.load(std::sync::atomic::Ordering::Acquire);
        }
        runs.get(&self.owner)
            .is_some_and(|r| r.parse_epoch == self.epoch)
    }
}

fn question_answers(
    request: &NativeRequest,
    selected: &BTreeMap<String, Vec<String>>,
    drafts: &HashMap<String, String>,
) -> Result<NativeAnswer, String> {
    let NativeRequestKind::Questions { questions } = &request.kind else {
        return Err("This request does not accept a text answer.".into());
    };
    let mut answers = BTreeMap::new();
    for question in questions {
        let mut values = selected.get(&question.id).cloned().unwrap_or_default();
        if let Some(text) = drafts.get(&question.id).filter(|v| !v.trim().is_empty()) {
            if !question.multiple {
                values.clear();
            }
            values.push(text.clone());
        }
        if question.required && values.is_empty() {
            return Err(format!(
                "Answer {} before continuing.",
                if question.header.is_empty() {
                    &question.question
                } else {
                    &question.header
                }
            ));
        }
        answers.insert(question.id.clone(), values);
    }
    Ok(NativeAnswer::Answers(answers))
}

impl Orbit {
    pub(super) fn interrupt_native_session(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = self.active.as_ref().and_then(|id| self.runs.get(id)) {
            if let Err(error) = run.handle.interrupt() {
                self.notice = Some(error.to_string());
            }
        } else {
            self.notice = Some("There is no active turn to interrupt.".into());
        }
        cx.notify();
    }

    pub(super) fn run_native_terminal(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.quit_pending {
            return;
        }
        if self.checkpoint_busy > 0 {
            self.notice =
                Some("Wait for file checkpoint work before running a sandbox command.".into());
            cx.notify();
            return;
        }
        let command = self.composer.read(cx).text();
        if command.trim().is_empty() {
            self.notice = Some("Use /terminal followed by a command. It runs in Codex’s selected read-only or workspace-write sandbox with network access blocked.".into());
            cx.notify();
            return;
        }
        if self.loading
            || self.loading_earlier
            || self.starting_current()
            || self.handoff_pending.is_some()
            || self.runs.len() + self.team_starting.len() + self.launches.len() >= 4
            || self.active.as_ref().is_some_and(|id| {
                self.runs.contains_key(id)
                    || self.settling.contains_key(id)
                    || self.finalizing.contains(id)
            })
        {
            self.notice = Some(
                "Wait for this conversation to finish before running a sandbox command.".into(),
            );
            cx.notify();
            return;
        }
        let Some(executable) = self
            .agents
            .iter()
            .find(|a| a.kind == AgentKind::Codex)
            .and_then(|a| a.executable.clone())
        else {
            self.notice = Some("Codex is required for this sandbox command.".into());
            cx.notify();
            return;
        };
        let now = chrono::Utc::now().timestamp();
        let session = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
            .cloned()
            .unwrap_or_else(|| SessionSummary {
                id: uuid::Uuid::new_v4().to_string(),
                title: short(&command, 46),
                agent: AgentKind::Codex,
                project_path: self.project.clone(),
                updated_at: now,
                external_id: None,
                transcript_path: None,
                execution_path: self.new_execution_path.clone(),
            });
        let id = session.id.clone();
        let project = session
            .execution_path
            .clone()
            .unwrap_or_else(|| session.project_path.clone());
        let request = RunRequest { reasoning_effort:None,
            agent: AgentKind::Codex,
            executable,
            project,
            prompt: command.clone(),
            resume_id: None,
            fork_session: false,
            model: None,
            images: Vec::new(),
            allow_edits: self.allow_edits,
            codex_approvals: self.codex_approvals,
            trust_workspace: false,
        };
        if !self.sessions.iter().any(|s| s.id == id) {
            self.sessions.insert(0, session.clone());
            self.visible_sessions = self
                .sessions
                .iter()
                .filter(|s| !self.hidden_sessions.contains(&s.id))
                .cloned()
                .collect();
            self.rebuild_workspace_sessions();
        }
        self.active = Some(id.clone());
        if !self.open_tabs.contains(&id) {
            self.open_tabs.push(id.clone());
        }

        let old = self.rows.len();
        self.rows.push(Row {
            role: Some("user".into()),
            block: MessageBlock::Code {
                language: "sh".into(),
                text: command,
                highlights: Vec::new(),
            },
        });
        self.list.splice(old..old, 1);
        self.list.scroll_to_reveal_item(old);
        self.composer.update(cx, |input, cx| input.set_text("", cx));
        self.drafts.remove(&id);
        self.start_native_maintenance(
            id,
            request,
            "command/exec",
            serde_json::json!({}),
            "Running sandbox command",
            cx,
        );
    }

    pub(super) fn steer_native_session(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text();
        let instruction = text.trim();
        let result = self
            .active
            .as_ref()
            .and_then(|id| self.runs.get(id))
            .ok_or_else(|| anyhow::anyhow!("Start a Codex turn before steering it."))
            .and_then(|run| {
                anyhow::ensure!(
                    !instruction.is_empty(),
                    "Use /steer followed by your instruction."
                );
                run.handle.steer(instruction)
            });
        match result {
            Ok(()) => {
                self.composer.update(cx, |input, cx| input.set_text("", cx));
            }
            Err(error) => self.notice = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(super) fn compact_native_session(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.quit_pending {
            return;
        }
        let Some(session) = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
            .cloned()
        else {
            self.notice = Some("Open a Codex conversation to compact its context.".into());
            cx.notify();
            return;
        };
        if session.agent != AgentKind::Codex
            || session.external_id.is_none()
            || imported_codex_needs_fork(&session)
        {
            self.notice =
                Some("Compact is available after continuing this conversation in Code.".into());
            cx.notify();
            return;
        }
        if self.loading
            || self.starting_current()
            || self.handoff_pending.is_some()
            || self.runs.len() + self.team_starting.len() + self.launches.len() >= 4
            || self.finalizing.contains(&session.id)
            || self.settling.contains_key(&session.id)
            || self.runs.contains_key(&session.id)
        {
            self.notice =
                Some("Wait for this turn to finish before compacting its context.".into());
            cx.notify();
            return;
        }
        let Some(executable) = self
            .agents
            .iter()
            .find(|a| a.kind == AgentKind::Codex)
            .and_then(|a| a.executable.clone())
        else {
            return;
        };
        let id = session.id.clone();
        let project = session
            .execution_path
            .clone()
            .unwrap_or_else(|| session.project_path.clone());
        let request = RunRequest { reasoning_effort:None,
            agent: AgentKind::Codex,
            executable,
            project,
            prompt: String::new(),
            resume_id: session.external_id,
            fork_session: false,
            model: self.model.clone(),
            images: Vec::new(),
            allow_edits: self.allow_edits,
            codex_approvals: self.codex_approvals,
            trust_workspace: false,
        };
        self.start_native_maintenance(
            id,
            request,
            "thread/compact/start",
            serde_json::json!({}),
            "Compacting context",
            cx,
        );
    }

    pub(super) fn run_codex_action(
        &mut self,
        action: runtime::codex_actions::Action,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.quit_pending {
            return false;
        }
        let Some(session) = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
            .cloned()
        else {
            self.notice = Some("Open a Codex conversation first.".into());
            return false;
        };
        if session.agent != AgentKind::Codex
            || session.external_id.is_none()
            || imported_codex_needs_fork(&session)
        {
            self.notice = Some("Continue this conversation in Code before changing its native goal or starting a review.".into());
            return false;
        }
        if self.loading
            || self.loading_earlier
            || self.starting_current()
            || self.handoff_pending.is_some()
            || self.runs.len() + self.team_starting.len() + self.launches.len() >= 4
            || self.finalizing.contains(&session.id)
            || self.settling.contains_key(&session.id)
            || self.runs.contains_key(&session.id)
        {
            self.notice =
                Some("Wait for this conversation to finish its current work first.".into());
            return false;
        }
        let Some(executable) = self
            .agents
            .iter()
            .find(|a| a.kind == AgentKind::Codex)
            .and_then(|a| a.executable.clone())
        else {
            return false;
        };
        let request = RunRequest { reasoning_effort:None,
            agent: AgentKind::Codex,
            executable,
            project: session
                .execution_path
                .clone()
                .unwrap_or_else(|| session.project_path.clone()),
            prompt: String::new(),
            resume_id: session.external_id,
            fork_session: false,
            model: self.model.clone(),
            images: Vec::new(),
            allow_edits: false,
            codex_approvals: self.codex_approvals,
            trust_workspace: false,
        };
        self.start_native_maintenance(
            session.id,
            request,
            action.method,
            action.params,
            action.status,
            cx,
        );
        true
    }

    pub(super) fn start_native_maintenance(
        &mut self,
        id: String,
        request: RunRequest,
        method: &'static str,
        params: serde_json::Value,
        status: &'static str,
        cx: &mut Context<Self>,
    ) {
        if self.checkpoint_busy > 0 {
            self.notice = Some(
                "Wait for file checkpoint work to finish before starting this operation.".into(),
            );
            cx.notify();
            return;
        }
        self.starting = true;
        let agent = request.agent;
        let checkpoint_repository = self.checkpoint_repository.clone();
        let cleanup_repository = checkpoint_repository.clone();
        let checkpoint_session = id.clone();
        let terminal_prompt = (method == "command/exec").then(|| request.prompt.clone());
        let saved_terminal_prompt = terminal_prompt.clone();
        let terminal_session = terminal_prompt
            .as_ref()
            .and_then(|_| self.sessions.iter().find(|s| s.id == id))
            .cloned();
        let checkpoint_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.checkpoint_cancel = checkpoint_cancel.clone();
        let launch_key=uuid::Uuid::new_v4().to_string();
        self.launches.insert(id.clone(),chat_timing::Launch{caption:status,preview:Vec::new(),cancel:checkpoint_cancel.clone(),started:Instant::now(),key:launch_key.clone()});
        let task = cx.background_executor().spawn(async move {
            let checkpoint = if method == "command/exec" {
                let repository = checkpoint_repository.map_err(anyhow::Error::msg)?;
                let key = uuid::Uuid::new_v4().to_string();
                let checkpoint = if request.allow_edits {
                    repository.capture_before(
                        &request.project,
                        &checkpoint_session,
                        &key,
                        &request.prompt,
                        &checkpoint_cancel,
                    )?
                } else {
                    repository.record_read_only(
                        &request.project,
                        &checkpoint_session,
                        &key,
                        &request.prompt,
                    )?
                };
                Some((repository, checkpoint))
            } else {
                None
            };
            if checkpoint_cancel.load(std::sync::atomic::Ordering::Acquire) {
                if let Some((repository, id)) = &checkpoint {
                    repository.abandon(id)?;
                }
                anyhow::bail!("Native operation canceled before launch");
            }
            match runtime::launch_native_command(request, method, params) {
                Ok(handle) => Ok((handle, checkpoint.map(|(_, id)| id))),
                Err(error) => {
                    if let Some((repository, id)) = checkpoint {
                        let _ = repository.abandon(&id);
                    }
                    Err(error)
                }
            }
        });
        cx.spawn(async move |this, cx| match task.await {
            Ok((handle,checkpoint_id)) => {
                let mut cleanup = cleanup_repository.ok().and_then(|repository|checkpoint_id.clone().map(|id|repository.cleanup_on_orphan(id,handle.settled.clone())));
                let wake = handle.wake.clone();
                if !this
                    .update(cx, |this, cx| {
                        if this.quit_pending || this.launches.get(&id).is_none_or(|l|l.key!=launch_key||l.cancel.load(std::sync::atomic::Ordering::Acquire)){
                            handle.cancel();this.launches.remove(&id);this.starting=!this.launches.is_empty();cx.notify();return false
                        }
                        this.launches.remove(&id);this.starting=!this.launches.is_empty();
                        if let Some(checkpoint)=checkpoint_id {
                            this.checkpoint_runs.insert(id.clone(),checkpoint.clone());
                            if let (Some(session),Some(command))=(terminal_session,saved_terminal_prompt.as_ref()) {
                                this.engine.save(session,"user",format!("Sandbox command:\n```sh\n{command}\n```\n\n<!--code-checkpoint:{checkpoint}-->"));
                                if this.active.as_deref()==Some(&id) {
                                    let at=this.rows.len();this.rows.push(Row{role:None,block:MessageBlock::Activity{label:"Prompt changes".into(),details:checkpoint}});this.list.splice(at..at,1);
                                }
                            }
                        }
                        this.runs.insert(
                            id.clone(),
                            Running { timing:None,
                                agent,
                                submitted_prompt: String::new(),
                                handle,
                                text: String::new(),
                                live_prefix_omitted: false,
                                status: status.into(),
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
                        cx.notify();true
                    })
                    .unwrap_or(false)
                {
                    return;
                }
                if let Some(cleanup)=cleanup.as_mut(){cleanup.disarm();}
                loop {
                    if wake.recv().await.is_ok() {
                        Timer::after(Duration::from_millis(16)).await;
                    }
                    if !matches!(this.update(cx, |this, cx| this.poll(&id, cx)), Ok(true)) {
                        break;
                    }
                }
            }
            Err(error) => {
                let _ = this.update(cx, |this, cx| {
                    this.launches.remove(&id);this.starting=!this.launches.is_empty();
                    if let Some(command)=terminal_prompt {
                        let draft=format!("/terminal {command}");
                        this.drafts.entry(id.clone()).or_insert_with(||draft.clone());
                        if this.active.as_deref()==Some(&id)&&this.composer.read(cx).text().is_empty(){this.composer.update(cx,|input,cx|input.set_text(&draft,cx));}
                    }
                    if this.active.as_deref()==Some(&id){this.notice=Some(error.to_string());}
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    pub(super) fn prepare_native_requests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active.clone();
        self.native_pending
            .retain(|_, pending| pending.current(&self.runs));
        let selected = self.visible_native_request().map(|p| p.request.id.clone());
        for (token, pending) in &mut self.native_pending {
            if active.as_ref() != Some(&pending.owner) || selected.as_ref() != Some(token) {
                continue;
            }
            if self.motion && pending.started.elapsed() < Duration::from_millis(320) {
                window.request_animation_frame();
            }
            let NativeRequestKind::Questions { questions } = &pending.request.kind else {
                continue;
            };
            for question in questions {
                if pending.inputs.contains_key(&question.id) {
                    continue;
                }
                let input = cx.new(|cx| {
                    let mut input = TextInput::new(window, cx);
                    input.set_placeholder(
                        if question.options.is_empty() {
                            "Your answer…"
                        } else {
                            "Or type an answer…"
                        },
                        cx,
                    );
                    input.set_secret(question.secret, cx);
                    input
                });
                let token = token.clone();
                let subscription = cx.subscribe_in(&input, window, move |this, _, event, _, cx| {
                    if matches!(event, InputEvent::Submit) {
                        this.submit_native_answers(&token, cx);
                    }
                    cx.notify();
                });
                pending.inputs.insert(question.id.clone(), input);
                pending.subscriptions.push(subscription);
            }
        }
    }

    fn respond_native(&mut self, token: &str, answer: NativeAnswer, cx: &mut Context<Self>) {
        let Some(pending) = self.native_pending.get_mut(token) else {
            return;
        };
        if pending.sending
            || !pending.current(&self.runs)
            || self.active.as_ref() != Some(&pending.owner)
        {
            return;
        }
        let result = if let Some(binding) = &pending.broker {
            runtime::broker::control_child(
                &binding.project,
                binding.provider,
                &binding.child,
                runtime::broker::ChildAction::Respond {
                    id: token.into(),
                    answer,
                },
            )
        } else {
            self.runs[&pending.owner].handle.respond(token, answer)
        };
        match result {
            Ok(()) => {
                pending.sending = true;
            }
            Err(error) => {
                self.notice = Some(format!("Could not send this decision: {error}"));
            }
        }
        cx.notify();
    }

    fn submit_native_answers(&mut self, token: &str, cx: &mut Context<Self>) {
        let Some(pending) = self.native_pending.get(token) else {
            return;
        };
        let drafts = pending
            .inputs
            .iter()
            .map(|(id, input)| (id.clone(), input.read(cx).text()))
            .collect();
        match question_answers(&pending.request, &pending.selected, &drafts) {
            Ok(answer) => self.respond_native(token, answer, cx),
            Err(error) => {
                self.notice = Some(error);
                cx.notify();
            }
        }
    }

    fn choose_native_option(
        &mut self,
        token: &str,
        question_id: &str,
        option: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.native_pending.get_mut(token) else {
            return;
        };
        if pending.sending || !pending.current(&self.runs) {
            return;
        }
        let NativeRequestKind::Questions { questions } = &pending.request.kind else {
            return;
        };
        let Some(question) = questions.iter().find(|q| q.id == question_id) else {
            return;
        };
        if !question.options.iter().any(|o| o.id == option) {
            return;
        }
        let selected = pending.selected.entry(question_id.into()).or_default();
        if question.multiple {
            if selected.iter().any(|v| v == option) {
                selected.retain(|v| v != option);
            } else {
                selected.push(option.into());
            }
        } else {
            *selected = vec![option.into()];
        }
        if let Some(input) = pending.inputs.get(question_id) {
            input.update(cx, |input, cx| input.set_text("", cx));
        }
        cx.notify();
    }

    fn visible_native_request(&self) -> Option<&LiveRequest> {
        let live =
            |p: &&LiveRequest| self.active.as_ref() == Some(&p.owner) && p.current(&self.runs);
        self.native_request_focus
            .as_ref()
            .and_then(|id| self.native_pending.get(id))
            .filter(live)
            .or_else(|| {
                self.native_pending
                    .values()
                    .filter(live)
                    .min_by_key(|p| p.started)
            })
    }

    fn cycle_native_request(&mut self, direction: isize, cx: &mut Context<Self>) {
        let mut pending: Vec<_> = self
            .native_pending
            .values()
            .filter(|p| self.active.as_ref() == Some(&p.owner) && p.current(&self.runs))
            .collect();
        pending.sort_by_key(|p| p.started);
        if pending.is_empty() {
            return;
        }
        let selected = self.visible_native_request().map(|p| &p.request.id);
        let current = pending
            .iter()
            .position(|p| Some(&p.request.id) == selected)
            .unwrap_or_default();
        let next = (current as isize + direction).rem_euclid(pending.len() as isize) as usize;
        self.native_request_focus = Some(pending[next].request.id.clone());
        cx.notify();
    }

    pub(super) fn native_request_tray(&self, cx: &Context<Self>) -> AnyElement {
        let Some(pending) = self.visible_native_request() else {
            return div().into_any_element();
        };
        let count = self
            .native_pending
            .values()
            .filter(|p| self.active.as_ref() == Some(&p.owner) && p.current(&self.runs))
            .count();
        div()
            .id("native-request-tray")
            .debug_selector(|| "native-request-tray".into())
            .w_full()
            .max_w(px(800.))
            .max_h(px(300.))
            .overflow_y_scroll()
            .flex_none()
            .when(count > 1, |s| {
                s.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .p_2()
                        .text_size(crate::theme::ui_px(11.))
                        .text_color(rgb(MUTED))
                        .child(format!("{count} requests need your decision"))
                        .child(div().flex_1())
                        .child(
                            div()
                                .id("previous-native-request")
                                .cursor_pointer()
                                .child("← Previous")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.cycle_native_request(-1, cx)),
                                ),
                        )
                        .child(
                            div()
                                .id("next-native-request")
                                .cursor_pointer()
                                .child("Next →")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.cycle_native_request(1, cx)),
                                ),
                        ),
                )
            })
            .child(self.native_request_card(&pending.request, true, cx))
            .into_any_element()
    }

    pub(super) fn native_request_card(
        &self,
        request: &NativeRequest,
        interactive: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let pending = self
            .native_pending
            .get(&request.id)
            .filter(|p| p.current(&self.runs));
        let live = interactive && pending.is_some_and(|p| self.active.as_ref() == Some(&p.owner));
        let sending = pending.is_some_and(|p| p.sending);
        let phase = pending
            .map(|p| {
                if self.motion {
                    presentation::spring((p.started.elapsed().as_secs_f32() / 0.32).min(1.))
                } else {
                    1.
                }
            })
            .unwrap_or(1.);
        let token = request.id.clone();
        let mut panel = div()
            .relative()
            .top(px(if live { 5. * (1. - phase) } else { 0. }))
            .w_full()
            .mb_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgba(0xebebeb25))
            .bg(rgba(self.appearance.tint(0xebebeb14)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(crate::theme::ui_px(12.))
                    .child(icon("assets/kai/status/tool.svg", 16., ACCENT))
                    .child(div().flex_1().min_w(px(0.)).child(request.title.clone()))
                    .child(
                        div()
                            .text_size(crate::theme::ui_px(10.))
                            .text_color(rgb(MUTED))
                            .child(if live {
                                if sending {
                                    "Sending decision…"
                                } else {
                                    "Your decision"
                                }
                            } else if pending.is_some() {
                                "Controls below"
                            } else {
                                "Earlier request"
                            }),
                    ),
            )
            .when(!request.detail.is_empty(), |s| {
                s.child(
                    div()
                        .mt_2()
                        .text_size(crate::theme::ui_px(12.))
                        .line_height(crate::theme::ui_line_height(19.))
                        .child(if live {
                            request.detail.clone()
                        } else {
                            short(&request.detail, 220)
                        }),
                )
            });
        if !live {
            return panel.into_any_element();
        }
        for url in request
            .detail
            .split_whitespace()
            .filter(|word| word.starts_with("https://") || word.starts_with("http://"))
            .take(4)
        {
            let url = url.trim_matches(['"', '<', '>', ',']).to_owned();
            panel = panel.child(
                div()
                    .id(SharedString::from(format!("native-request-url-{url}")))
                    .mt_2()
                    .text_size(crate::theme::ui_px(12.))
                    .text_color(rgb(ACCENT))
                    .cursor_pointer()
                    .child("Open link in browser ↗")
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        }
        match &request.kind {
            NativeRequestKind::Approval { choices } => {
                panel = panel.child(div().flex().flex_wrap().gap_2().mt_3().children(
                    choices.iter().map(|choice| {
                        let token = token.clone();
                        let value = choice.id.clone();
                        div()
                            .id(SharedString::from(format!("native-choice-{token}-{value}")))
                            .debug_selector({
                                let value = value.clone();
                                move || format!("native-approval-{value}")
                            })
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .text_size(crate::theme::ui_px(12.))
                            .bg(rgba(GLASS_HOVER))
                            .when(!sending, |s| {
                                s.cursor_pointer()
                                    .hover(|s| s.bg(rgba(0xebebeb28)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.respond_native(
                                            &token,
                                            NativeAnswer::Choice(value.clone()),
                                            cx,
                                        )
                                    }))
                            })
                            .child(choice.label.clone())
                    }),
                ));
            }
            NativeRequestKind::Questions { questions } => {
                for question in questions {
                    panel = panel.child(
                        div()
                            .mt_3()
                            .text_size(crate::theme::ui_px(12.))
                            .child(question.question.clone()),
                    );
                    panel = panel.child(div().flex().flex_wrap().gap_2().mt_2().children(
                        question.options.iter().map(|option| {
                            let token = token.clone();
                            let id = question.id.clone();
                            let value = option.id.clone();
                            let selected = pending.is_some_and(|p| {
                                p.selected.get(&id).is_some_and(|s| s.contains(&value))
                            });
                            div()
                                .id(SharedString::from(format!(
                                    "native-option-{token}-{id}-{value}"
                                )))
                                .debug_selector({
                                    let id = id.clone();
                                    let value = value.clone();
                                    move || format!("native-option-{id}-{value}")
                                })
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .text_size(crate::theme::ui_px(12.))
                                .bg(rgba(if selected { 0x459bf735 } else { GLASS_HOVER }))
                                .when(!sending, |s| {
                                    s.cursor_pointer()
                                        .hover(|s| s.bg(rgba(0xebebeb28)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.choose_native_option(&token, &id, &value, cx)
                                        }))
                                })
                                .child(option.label.clone())
                        }),
                    ));
                    if let Some(input) = pending.and_then(|p| p.inputs.get(&question.id)) {
                        panel = panel.child(
                            div()
                                .mt_2()
                                .p_2()
                                .rounded_md()
                                .bg(rgba(GLASS_SURFACE))
                                .child(input.clone()),
                        );
                    }
                }
                let cancel = token.clone();
                panel = panel.child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .mt_3()
                        .child(
                            div()
                                .id(SharedString::from(format!("native-cancel-{token}")))
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .text_size(crate::theme::ui_px(12.))
                                .child("Cancel")
                                .when(!sending, |s| {
                                    s.cursor_pointer()
                                        .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.respond_native(&cancel, NativeAnswer::Cancel, cx)
                                        }))
                                }),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("native-submit-{token}")))
                                .debug_selector(|| "native-question-submit".into())
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .text_size(crate::theme::ui_px(12.))
                                .bg(rgba(0x459bf730))
                                .child("Send answer")
                                .when(!sending, |s| {
                                    s.cursor_pointer()
                                        .hover(|s| s.bg(rgba(0x459bf750)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.submit_native_answers(&token, cx)
                                        }))
                                }),
                        ),
                );
            }
            NativeRequestKind::Unsupported => {
                panel = panel.child(div().mt_2().text_size(crate::theme::ui_px(12.)).child(
                    "This provider request is not supported. Stop the turn to cancel it safely.",
                ));
            }
        }
        panel.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn questions_validate_required_and_preserve_multiple_answers() {
        let request = NativeRequest {
            id: "opaque".into(),
            method: "test".into(),
            title: "Questions".into(),
            detail: String::new(),
            kind: NativeRequestKind::Questions {
                questions: vec![runtime::NativeQuestion {
                    id: "q".into(),
                    header: "Choice".into(),
                    question: "Choose".into(),
                    options: vec![],
                    secret: false,
                    multiple: true,
                    required: true,
                }],
            },
        };
        assert!(question_answers(&request, &BTreeMap::new(), &HashMap::new()).is_err());
        let selected = BTreeMap::from([("q".into(), vec!["a".into(), "b".into()])]);
        let drafts = HashMap::from([("q".into(), "Other".into())]);
        assert_eq!(
            question_answers(&request, &selected, &drafts),
            Ok(NativeAnswer::Answers(BTreeMap::from([(
                "q".into(),
                vec!["a".into(), "b".into(), "Other".into()]
            )])))
        );
    }

    fn approval(id: &str) -> NativeRequest {
        NativeRequest {
            id: id.into(),
            method: "item/commandExecution/requestApproval".into(),
            title: "Approve terminal command".into(),
            detail: "printf test".into(),
            kind: NativeRequestKind::Approval {
                choices: vec![runtime::NativeOption {
                    id: "decline".into(),
                    label: "Decline".into(),
                }],
            },
        }
    }

    #[gpui::test]
    fn background_child_decision_survives_parent_completion_and_expires_with_ownership(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let live = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            app.native_pending.insert(
                "child-request".into(),
                LiveRequest::for_child(
                    "fixture-session",
                    approval("child-request"),
                    broker_ui::RequestOwner {
                        project: app.project.clone(),
                        provider: AgentKind::Codex,
                        child: "background-child".into(),
                        live: live.clone(),
                    },
                ),
            );
            assert!(app.runs.is_empty());
            assert!(app.visible_native_request().is_some());
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("native-request-tray").is_some());
        live.store(false, std::sync::atomic::Ordering::Release);
        view.update(cx, |app, cx| {
            assert!(app.visible_native_request().is_none());
            app.respond_native("child-request", NativeAnswer::Choice("decline".into()), cx);
            assert!(!app.native_pending["child-request"].sending);
            assert!(!app.native_pending["child-request"].current(&app.runs));
        });
    }

    #[gpui::test]
    fn live_question_choices_submit_on_the_native_control_channel_once(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let (handle, decisions) = RunHandle::interactive_test_handle();
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            let mut run = crate::ui_tests::idle_run(String::new());
            run.handle = handle;
            let request = NativeRequest {
                id: "live-question".into(),
                method: "item/tool/requestUserInput".into(),
                title: "Choose how to continue".into(),
                detail: String::new(),
                kind: NativeRequestKind::Questions {
                    questions: vec![runtime::NativeQuestion {
                        id: "restart".into(),
                        header: "Restart".into(),
                        question: "Restart Kai and measure, or keep the current session?".into(),
                        options: vec![runtime::NativeOption {
                            id: "Keep running".into(),
                            label: "Keep running".into(),
                        }],
                        secret: false,
                        multiple: false,
                        required: true,
                    }],
                },
            };
            app.native_pending.insert(
                request.id.clone(),
                LiveRequest::new("fixture-session", run.parse_epoch, request),
            );
            app.runs.insert("fixture-session".into(), run);
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("native-request-tray").is_some());
        crate::ui_tests::click_selector(cx, "native-option-restart-Keep running");
        crate::ui_tests::draw_frame(&view, cx);
        crate::ui_tests::click_selector(cx, "native-question-submit");
        let runtime::NativeControl::Respond { id, answer } = decisions.try_recv().unwrap() else {
            panic!("Question was sent as prose instead of a native response");
        };
        assert_eq!(id, "live-question");
        assert_eq!(
            answer,
            NativeAnswer::Answers(BTreeMap::from([(
                "restart".into(),
                vec!["Keep running".into()]
            )]))
        );
        crate::ui_tests::click_selector(cx, "native-question-submit");
        assert!(decisions.try_recv().is_err());
        view.update(cx, |app, cx| {
            assert!(app.composer.read(cx).text().is_empty());
            assert!(app.native_pending["live-question"].sending);
        });
    }

    #[gpui::test]
    fn approval_tray_routes_once_and_rejects_stale_run_callbacks(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let (handle, decisions) = RunHandle::interactive_test_handle();
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            let mut run = crate::ui_tests::idle_run("x".repeat(64 * 1024));
            run.handle = handle;
            app.native_pending.insert(
                "opaque".into(),
                LiveRequest::new("fixture-session", run.parse_epoch, approval("opaque")),
            );
            app.runs.insert("fixture-session".into(), run);
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        let bounds = cx.debug_bounds("native-request-tray").unwrap();
        assert!(bounds.size.height <= px(300.));
        assert!(bounds.bottom() <= cx.debug_bounds("composer-card").unwrap().top());
        let button = cx.debug_bounds("native-approval-decline").unwrap();
        cx.simulate_click(button.center(), Modifiers::none());
        assert!(
            matches!(decisions.try_recv(), Ok(runtime::NativeControl::Respond { id, answer: NativeAnswer::Choice(choice) }) if id == "opaque" && choice == "decline")
        );
        view.update(cx, |app, cx| {
            app.respond_native("opaque", NativeAnswer::Choice("decline".into()), cx);
            assert!(
                decisions.try_recv().is_err(),
                "Decision must not be sent twice"
            );
            app.native_pending.get_mut("opaque").unwrap().retry();
            app.runs.get_mut("fixture-session").unwrap().parse_epoch = uuid::Uuid::new_v4();
            app.respond_native("opaque", NativeAnswer::Choice("decline".into()), cx);
            assert!(
                decisions.try_recv().is_err(),
                "Old tool controls must not answer a new run"
            );
        });
        crate::ui_tests::draw_frame(&view, cx);
        view.update(cx, |app, _| assert!(app.native_pending.is_empty()));
    }

    #[gpui::test]
    fn request_queue_only_creates_visible_form_and_keeps_drafts_ephemeral(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let (handle, decisions) = RunHandle::interactive_test_handle();
        let mut request = approval("first");
        request.kind = NativeRequestKind::Questions {
            questions: vec![runtime::NativeQuestion {
                id: "secret".into(),
                header: "Password".into(),
                question: "Enter a secret".into(),
                options: vec![],
                secret: true,
                multiple: false,
                required: true,
            }],
        };
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            let mut run = crate::ui_tests::idle_run(String::new());
            run.handle = handle;
            let epoch = run.parse_epoch;
            app.runs.insert("fixture-session".into(), run);
            app.native_pending.insert(
                "first".into(),
                LiveRequest::new("fixture-session", epoch, request.clone()),
            );
            request.id = "second".into();
            app.native_pending.insert(
                "second".into(),
                LiveRequest::new("fixture-session", epoch, request),
            );
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        view.update(cx, |app, cx| {
            assert_eq!(app.native_pending["first"].inputs.len(), 1);
            assert!(app.native_pending["second"].inputs.is_empty());
            app.submit_native_answers("first", cx);
            assert!(decisions.try_recv().is_err());
            app.native_pending["first"].inputs["secret"].update(cx, |input, cx| input.set_text("private-👩🏽‍💻", cx));
            app.submit_native_answers("first", cx);
            assert!(matches!(decisions.try_recv(), Ok(runtime::NativeControl::Respond { id, answer: NativeAnswer::Answers(values) }) if id == "first" && values["secret"] == ["private-👩🏽‍💻"]));
            assert!(app.drafts.values().all(|value| !value.contains("private-")));
            app.cycle_native_request(1, cx);
        });
        crate::ui_tests::draw_frame(&view, cx);
        view.update(cx, |app, _| {
            assert_eq!(app.native_pending["second"].inputs.len(), 1);
            app.runs.clear();
        });
        crate::ui_tests::draw_frame(&view, cx);
        view.update(cx, |app, _| assert!(app.native_pending.is_empty()));
    }
}

// The notch reviews only live, explicitly scoped requests. Opaque IDs include
// the exact invocation epoch so provider request IDs cannot survive a restart.
impl Orbit {
    pub(super) fn unified_pending_ids(&self)->Vec<String>{let mut ids:Vec<_>=self.native_pending.iter().filter(|(_,p)|p.current(&self.runs)&&!p.sending).map(|(id,_)|id.clone()).collect();ids.sort();ids}
    pub(super) fn unified_pending(&self) -> Option<(String, String, String)> {
        let mut pending: Vec<_> = self
            .native_pending
            .iter()
            .filter(|(_, p)| p.current(&self.runs) && !p.sending)
            .collect();
        pending.sort_by_key(|(id, p)| (self.native_request_focus.as_ref()!=Some(*id),self.active.as_ref() != Some(&p.owner), (*id).clone()));
        pending.first().map(|(id, p)| {
            let token = format!("{}:{id}", p.epoch);
            let body = if p.request.detail.is_empty() {
                p.request.title.clone()
            } else {
                format!("{}\n\n{}", p.request.title, p.request.detail)
            };
            (p.owner.clone(), token, body)
        })
    }
    pub(super) fn unified_approval(
        &mut self,
        value: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let owner = value["threadId"]
            .as_str()
            .and_then(|s| s.strip_prefix("code:"))
            .ok_or("Missing approval owner")?;
        let token = value["approvalRequestId"]
            .as_str()
            .ok_or("Review the current request before deciding")?;
        let (id, pending) = self
            .native_pending
            .iter()
            .find(|(id, p)| {
                p.owner == owner
                    && format!("{}:{id}", p.epoch) == token
                    && p.current(&self.runs)
                    && !p.sending
            })
            .ok_or("This approval is no longer pending")?;
        let id = id.clone();
        if value["action"] == "approval_stop" {
            if let Some(binding) = &pending.broker {
                runtime::broker::control_child(
                    &binding.project,
                    binding.provider,
                    &binding.child,
                    runtime::broker::ChildAction::Respond {
                        id: id.clone(),
                        answer: NativeAnswer::Cancel,
                    },
                )
                .map_err(|e| e.to_string())?;
            } else {
                self.runs.get(owner).ok_or("Run finished")?.handle.cancel();
            }
            return Ok(());
        }
        if value["action"] == "native_answer" {
            let answer = match &pending.request.kind {
                NativeRequestKind::Approval { choices } => {
                    let choice = value["choice"].as_str().ok_or("Choose a response")?;
                    if !choices.iter().any(|c| c.id == choice) {
                        return Err("This choice is no longer available".into());
                    }
                    NativeAnswer::Choice(choice.into())
                }
                NativeRequestKind::Questions { questions } => {
                    let answers: BTreeMap<String, Vec<String>> =
                        serde_json::from_value(value["answers"].clone())
                            .map_err(|_| "Invalid answers")?;
                    if answers
                        .keys()
                        .any(|id| !questions.iter().any(|q| &q.id == id))
                    {
                        return Err("Unknown question".into());
                    }
                    for question in questions {
                        let values = answers.get(&question.id).map(Vec::as_slice).unwrap_or(&[]);
                        if question.required && values.iter().all(|v| v.trim().is_empty()) {
                            return Err("Answer each required question".into());
                        }
                        if !question.multiple && values.len() > 1 {
                            return Err("Choose one answer for this question".into());
                        }
                        if values.len() > 32
                            || values.iter().any(|v| v.len() > 16000 || v.contains('\0'))
                        {
                            return Err("An answer is too large".into());
                        }
                    }
                    NativeAnswer::Answers(answers)
                }
                _ => return Err("This request needs its workspace controls".into()),
            };
            let result = if let Some(binding) = &pending.broker {
                runtime::broker::control_child(
                    &binding.project,
                    binding.provider,
                    &binding.child,
                    runtime::broker::ChildAction::Respond {
                        id: id.clone(),
                        answer,
                    },
                )
            } else {
                self.runs[owner].handle.respond(&id, answer)
            };
            result.map_err(|e| e.to_string())?;
            self.native_pending.get_mut(&id).unwrap().sending = true;
            cx.notify();
            return Ok(());
        }
        let NativeRequestKind::Approval { choices } = &pending.request.kind else {
            return Err(
                "Open the workspace to answer this question using its native controls".into(),
            );
        };
        let candidates: &[&str] = if value["action"] == "approve" {
            &[
                "accept",
                "allow",
                "once",
                "allow_once",
                "approved",
                "accepted",
            ]
        } else {
            &["decline", "deny", "reject", "rejected", "denied", "cancel"]
        };
        let choice = candidates
            .iter()
            .find_map(|id| choices.iter().find(|c| c.id == *id))
            .ok_or("Review the available choices in the workspace")?;
        let answer = NativeAnswer::Choice(choice.id.clone());
        // Same runtime route as Code's native tray, including delegated children.
        let result = if let Some(binding) = &pending.broker {
            runtime::broker::control_child(
                &binding.project,
                binding.provider,
                &binding.child,
                runtime::broker::ChildAction::Respond {
                    id: id.clone(),
                    answer,
                },
            )
        } else {
            self.runs[owner].handle.respond(&id, answer)
        };
        result.map_err(|e| e.to_string())?;
        self.native_pending.get_mut(&id).unwrap().sending = true;
        cx.notify();
        Ok(())
    }
}

impl Orbit {
    pub(super) fn unified_review(&self) -> serde_json::Value {
        let Some((owner, token, _)) = self.unified_pending() else {
            return serde_json::Value::Null;
        };
        let Some((_, p)) = self
            .native_pending
            .iter()
            .find(|(id, p)| p.owner == owner && format!("{}:{id}", p.epoch) == token)
        else {
            return serde_json::Value::Null;
        };
        let (choices, questions) = match &p.request.kind {
            NativeRequestKind::Approval { choices } => {
                (serde_json::json!(choices), serde_json::json!([]))
            }
            NativeRequestKind::Questions { questions } => {
                (serde_json::json!([]), serde_json::json!(questions))
            }
            _ => (serde_json::json!([]), serde_json::json!([])),
        };
        serde_json::json!({"owner":format!("code:{owner}"),"token":token,"title":p.request.title,"detail":p.request.detail,"choices":choices,"questions":questions})
    }
}
