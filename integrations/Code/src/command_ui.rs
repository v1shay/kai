//! Composer command picker: bounded catalog, native lists, finite glass motion.
use super::*;
use commands::{Action, Command, Entry};

pub(super) struct SlashPicker {
    query: String,
    agent: AgentKind,
    matches: Vec<Entry>,
    revision: u64,
    selected: usize,
    scroll: UniformListScrollHandle,
    started: Instant,
    pub closing: bool,
}
impl Orbit {
    fn native_scope(&self) -> (AgentKind, PathBuf) {
        (
            self.agent,
            self.execution_root()
                .unwrap_or_else(|| self.project.clone()),
        )
    }
    pub(super) fn native_command_available(&self, name: &str) -> bool {
        matches!(
            self.agent,
            AgentKind::Claude | AgentKind::Gemini | AgentKind::OpenCode | AgentKind::Cursor
        ) && commands::lookup(self.agent, name).is_none()
            && self
                .native_commands
                .get(&self.native_scope())
                .is_some_and(|commands| commands.iter().any(|c| c.as_ref() == name))
    }
    pub(super) fn refresh_slash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.slash.as_ref().is_some_and(|p| {
            !p.closing && (p.agent != self.agent || p.revision != self.native_command_revision)
        }) {
            self.update_slash(window, cx);
        }
    }
    #[cfg(test)]
    pub(super) fn selected_command(&self) -> Option<&str> {
        self.slash
            .as_ref()
            .and_then(|p| p.matches.get(p.selected))
            .map(Entry::name)
    }
    pub(super) fn remember_usage(&mut self, id: String, usage: usage::Usage) {
        if self.usage.len() >= 128
            && !self.usage.contains_key(&id)
            && let Some(old) = self
                .usage
                .keys()
                .find(|key| self.active.as_ref() != Some(key))
                .cloned()
        {
            self.usage.remove(&old);
        }
        self.usage.insert(id.clone(), usage);
        if self.usage.len() > 128
            && let Some(old) = self
                .usage
                .keys()
                .find(|key| **key != id && self.active.as_ref() != Some(key))
                .cloned()
        {
            self.usage.remove(&old);
        }
    }
    pub(super) fn update_slash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.composer.read(cx).command_query().map(str::to_owned);
        let Some(query) = query else {
            self.dismiss_slash(false, cx);
            self.slash_dismissed = None;
            return;
        };
        if self.slash_dismissed.as_deref() == Some(query.as_str()) {
            return;
        }
        self.slash_dismissed = None;
        if self.slash.as_ref().is_some_and(|p| {
            !p.closing
                && p.query == query
                && p.agent == self.agent
                && p.revision == self.native_command_revision
        }) {
            return;
        }
        let mut matches: Vec<Entry> = commands::matching(self.agent, &query)
            .into_iter()
            .map(Entry::Local)
            .collect();
        if matches!(
            self.agent,
            AgentKind::Claude | AgentKind::Gemini | AgentKind::OpenCode
        ) && let Some(native) = self.native_commands.get(&self.native_scope())
        {
            let filter = query.to_ascii_lowercase();
            matches.extend(
                native
                    .iter()
                    .filter(|name| {
                        commands::lookup(self.agent, name).is_none()
                            && commands::ascii_contains(name, &filter)
                    })
                    .cloned()
                    .map(Entry::Native),
            );
        }
        if let Some(picker) = &mut self.slash {
            picker.query = query;
            picker.agent = self.agent;
            picker.revision = self.native_command_revision;
            picker.scroll.scroll_to_item(0, ScrollStrategy::Top);
            if picker.closing {
                picker.started = Instant::now();
            }
            picker.matches = matches;
            picker.selected = 0;
            picker.closing = false;
        } else {
            self.slash = Some(SlashPicker {
                query,
                agent: self.agent,
                revision: self.native_command_revision,
                matches,
                selected: 0,
                scroll: UniformListScrollHandle::new(),
                started: Instant::now(),
                closing: false,
            });
        }
        self.composer
            .update(cx, |input, _| input.set_command_navigation(true));
        self.composer.read(cx).focus_handle.focus(window);
        cx.notify();
    }
    pub(super) fn dismiss_slash(&mut self, suppress: bool, cx: &mut Context<Self>) {
        if suppress {
            self.slash_dismissed = self.composer.read(cx).command_query().map(str::to_owned);
        }
        self.composer
            .update(cx, |input, _| input.set_command_navigation(false));
        if self.motion {
            if let Some(picker) = &mut self.slash
                && !picker.closing
            {
                picker.closing = true;
                picker.started = Instant::now();
            }
        } else {
            self.slash = None;
        }
        cx.notify();
    }
    pub(super) fn move_slash(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(picker) = &mut self.slash
            && !picker.matches.is_empty()
            && !picker.closing
        {
            picker.selected = (picker.selected as isize + delta)
                .rem_euclid(picker.matches.len() as isize) as usize;
            picker
                .scroll
                .scroll_to_item(picker.selected, ScrollStrategy::Center);
            cx.notify();
        }
    }
    pub(super) fn accept_slash(
        &mut self,
        complete: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self
            .slash
            .as_ref()
            .filter(|p| !p.closing)
            .and_then(|p| p.matches.get(p.selected))
            .cloned();
        let Some(command) = selected else {
            self.notice = Some(
                "This command is not available through this agent's headless integration.".into(),
            );
            cx.notify();
            return;
        };
        if complete {
            self.composer.update(cx, |input, cx| {
                input.set_text(&format!("/{} ", command.name()), cx)
            });
            return;
        }
        let text = self.composer.read(cx).text();
        let arguments = text
            .trim()
            .split_once(char::is_whitespace)
            .map_or("", |(_, args)| args.trim())
            .to_owned();
        match command {
            Entry::Local(command) => self.run_command(command, &arguments, window, cx),
            Entry::Native(name) => {
                self.composer.update(cx, |input, cx| {
                    input.set_text(format!("/{name} {arguments}").trim_end(), cx)
                });
                self.dismiss_slash(false, cx);
                self.send(window, cx);
            }
        }
    }
    pub(super) fn handle_command(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let text = self.composer.read(cx).text();
        let Some(query) = commands::name(text.trim()) else {
            return false;
        };
        if self.native_command_available(query) {
            if self.handoff_pending.is_some() {
                self.notice = Some(
                    "Finish or cancel the context transfer before invoking a native command."
                        .into(),
                );
                cx.notify();
                return true;
            }
            if !self.attachments.is_empty() || self.attachment_pending > 0 {
                self.notice = Some("Remove images before running a native CLI command. Your draft and attachments are preserved.".into());
                cx.notify();
                return true;
            }
            if self
                .active
                .as_ref()
                .is_some_and(|id| self.runs.contains_key(id) || self.finalizing.contains(id))
            {
                self.notice = Some(
                    "Native CLI commands run between turns. Finish or stop this turn first.".into(),
                );
                cx.notify();
                return true;
            }
            if query == "compact"
                && self
                    .active
                    .as_ref()
                    .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
                    .is_none_or(|s| s.external_id.is_none())
            {
                self.notice =
                    Some("Resume a native session with prior history before compacting.".into());
                cx.notify();
                return true;
            }
            if self.active.as_ref().is_some_and(|id| {
                self.handoff_packets.contains_key(id)
                    && self
                        .sessions
                        .iter()
                        .find(|s| &s.id == id)
                        .is_none_or(|s| s.external_id.is_none())
            }) {
                self.notice = Some(
                    "Send the initial context handoff before invoking native CLI commands.".into(),
                );
                cx.notify();
                return true;
            }
            self.dismiss_slash(false, cx);
            return false;
        }
        if let Some(command) = commands::lookup(self.agent, query) {
            let args = text
                .trim()
                .split_once(char::is_whitespace)
                .map_or("", |(_, args)| args.trim());
            self.run_command(command, args, window, cx);
        } else if self
            .slash
            .as_ref()
            .is_some_and(|p| !p.closing && !p.matches.is_empty())
        {
            self.accept_slash(false, window, cx);
        } else {
            self.notice = Some("This slash command is not supported by this headless CLI. Use /help for available controls.".into());
            cx.notify();
        }
        true
    }
    fn run_command(
        &mut self,
        command: Command,
        args: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !args.is_empty()
            && !matches!(
                command.action,
                Action::Models
                    | Action::Handoff
                    | Action::ReadOnly
                    | Action::Delegate
                    | Action::Steer
                    | Action::Terminal
                    | Action::Review
                    | Action::Goal
            )
        {
            self.notice = Some(format!("/{} takes no arguments.", command.name));
            cx.notify();
            return;
        }
        if command.action == Action::Handoff
            && !args.is_empty()
            && !self.agents.iter().any(|a| {
                a.available
                    && (a.kind.command().eq_ignore_ascii_case(args)
                        || a.kind.name().eq_ignore_ascii_case(args))
            })
        {
            self.notice = Some("Choose an installed agent from /handoff.".into());
            cx.notify();
            return;
        }
        if command.action == Action::Models && !args.is_empty() {
            let known = args == "default"
                || self
                    .native_models
                    .get(&self.agent)
                    .is_some_and(|models| models.iter().any(|model| model.id == args))
                || self
                    .controls
                    .get(self.agent.command())
                    .is_some_and(|(_, info)| info.models.iter().any(|model| model.id == args));
            if !known {
                self.notice = Some("Choose an available model from /model first.".into());
                cx.notify();
                return;
            }
            if self.loading || self.starting || self.handoff_pending.is_some() {
                self.notice =
                    Some("Wait for the conversation to finish opening or transferring.".into());
                cx.notify();
                return;
            }
        }
        if command.action == Action::ReadOnly
            && !args.is_empty()
            && self
                .active
                .as_ref()
                .is_some_and(|id| self.runs.contains_key(id))
        {
            self.notice = Some("Stop the current turn before sending a new plan prompt.".into());
            cx.notify();
            return;
        }
        self.dismiss_slash(false, cx);
        self.composer.update(cx, |input, cx| input.set_text("", cx));
        self.notice = None;
        match command.action {
            Action::ChatContext => self.toggle_context_picker(window, cx),
            Action::Team => self.toggle_team_panel(window, cx),
            Action::Shell => self.toggle_terminal(window, cx),
            Action::New => self.new_conversation(window, cx),
            Action::Search => self.show_modal(Modal::Search, window, cx),
            Action::Projects => self.show_modal(Modal::Projects, window, cx),
            Action::Settings => self.show_modal(Modal::Settings, window, cx),
            Action::Wallpaper => self.show_modal(Modal::Background, window, cx),
            Action::Files => self.toggle_files(window, cx),
            Action::Diffs => self.toggle_diffs(cx),
            Action::Children => self.toggle_children(window, cx),
            Action::Undo => self.preview_latest_checkpoint(
                checkpoints::Direction::Undo,
                point(px(460.), px(100.)),
                cx,
            ),
            Action::Redo => self.preview_latest_checkpoint(
                checkpoints::Direction::Redo,
                point(px(460.), px(100.)),
                cx,
            ),
            Action::Models if !args.is_empty() => {
                self.choose_model((args != "default").then(|| args.into()), cx)
            }
            Action::Models | Action::Agents => self.toggle_agent_picker(
                point(
                    px(self.sidebar_width
                        + if self.conversations_visible {
                            282.
                        } else {
                            22.
                        }),
                    window.viewport_size().height - px(245.),
                ),
                window,
                cx,
            ),
            Action::Status | Action::Context => self.show_modal(Modal::Status, window, cx),
            Action::Compact => self.compact_native_session(window, cx),
            Action::Review | Action::Goal => {
                let action = if command.action == Action::Review {
                    runtime::codex_actions::review(args)
                } else {
                    runtime::codex_actions::goal(args)
                };
                match action {
                    Ok(action) => {
                        if !self.run_codex_action(action, cx) {
                            self.composer.update(cx, |input, cx| {
                                input.set_text(format!("/{} {args}", command.name).trim_end(), cx)
                            });
                        }
                    }
                    Err(error) => {
                        self.composer.update(cx, |input, cx| {
                            input.set_text(&format!("/{} {args}", command.name), cx)
                        });
                        self.notice = Some(error.to_string());
                    }
                }
            }
            Action::Interrupt => self.interrupt_native_session(cx),
            Action::Terminal => {
                self.composer
                    .update(cx, |input, cx| input.set_text(args, cx));
                self.run_native_terminal(window, cx);
            }
            Action::Steer => {
                self.composer
                    .update(cx, |input, cx| input.set_text(args, cx));
                self.steer_native_session(window, cx);
            }
            Action::Inventory(kind) => {
                if self.modal != Some(Modal::Status) {
                    self.show_modal(Modal::Status, window, cx);
                }
                self.refresh_native_catalog(kind, cx);
            }
            Action::Permissions => self.show_modal(Modal::Permissions, window, cx),
            Action::Stop => {
                let canceled_handoff = self.cancel_send_handoff(cx);
                if canceled_handoff {
                    self.notice = Some(
                        "Context transfer canceled. The source conversation is preserved.".into(),
                    );
                }
                if let Some(run) = self.active.as_ref().and_then(|id| self.runs.get_mut(id)) {
                    run.handle.cancel();
                    run.status = "Stopping…".into();
                } else if !canceled_handoff {
                    self.notice = Some("This conversation has no running agent.".into());
                }
            }
            Action::ReadOnly | Action::Edit => {
                self.allow_edits = command.action == Action::Edit;
                if !args.is_empty() {
                    self.composer
                        .update(cx, |input, cx| input.set_text(args, cx));
                    self.send(window, cx);
                }
            }
            Action::CopySession => {
                if let Some(id) = self
                    .active
                    .as_ref()
                    .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
                    .and_then(|s| s.external_id.clone())
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(id));
                    self.notice = Some("Native session ID copied.".into());
                } else {
                    self.notice = Some("The CLI has not reported a native session ID yet.".into());
                }
            }
            Action::Latest => {
                self.load_history_page(false, cx);
            }
            Action::Motion => {
                self.motion = !self.motion;
            }
            Action::Sidebar => self.toggle_sidebar(),
            Action::Conversations => self.toggle_conversations(cx),
            Action::Attach => self.choose_images(cx),
            Action::Dictate => self.toggle_dictation(window, cx),
            Action::Help => self
                .composer
                .update(cx, |input, cx| input.set_text("/", cx)),
            Action::Handoff => self.open_handoff(args, window, cx),
            Action::Delegate => {
                self.composer.update(cx, |input, cx| {
                    input.set_text(&format!("/delegate {args}"), cx)
                });
                self.composer.read(cx).focus_handle.focus(window);
                if args.is_empty() {
                    self.notice = Some("Describe the task after /delegate. The current agent will use its own subagent tools and report the result here.".into());
                } else {
                    self.send(window, cx);
                }
            }
        }
        cx.notify();
    }
    pub(super) fn toggle_conversations(&mut self, cx: &mut Context<Self>) {
        self.conversations_visible = !self.conversations_visible;
        if !self.conversations_visible {
            self.sessions_revealed = false;
            self.sessions_reveal = None;
        }
        self.queue_workspace_save(cx);
        cx.notify();
    }
    pub(super) fn toggle_diffs(&mut self, cx: &mut Context<Self>) {
        if self.diff_view.is_some() {
            self.diff_view = None;
            self.diff_observer = None;
            self.diff_started = None;
        } else {
            self.open_changes(cx);
        }
        cx.notify();
    }
    pub(super) fn tick_slash(&mut self, window: &Window, cx: &Context<Self>) {
        let Some(picker) = &self.slash else {
            return;
        };
        let duration = if picker.closing { 120 } else { 320 };
        if !self.motion || picker.started.elapsed() >= Duration::from_millis(duration) {
            if picker.closing {
                self.slash = None;
            }
        } else {
            request_motion_frame(window, cx);
        }
    }
    pub(super) fn slash_view(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        self.slash_view_for_input(window, &self.composer, cx)
    }
    pub(super) fn slash_view_for_input(
        &self,
        window: &Window,
        composer: &Entity<TextInput>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(picker) = &self.slash else {
            return div().into_any_element();
        };
        let t = (picker.started.elapsed().as_secs_f32() / if picker.closing { 0.12 } else { 0.32 })
            .min(1.);
        let reveal = if !self.motion {
            if picker.closing { 0. } else { 1. }
        } else if picker.closing {
            1. - t * t
        } else {
            presentation::spring(t)
        };
        let caret = composer.read(cx).bounds();
        let width = px(440.).min(window.viewport_size().width - px(16.));
        let height = px(80. + (picker.matches.len().clamp(1, 7) * 46) as f32);
        let left = caret
            .map_or(px(self.sidebar_width + 282.), |b| b.left())
            .clamp(
                px(8.),
                (window.viewport_size().width - width - px(8.)).max(px(8.)),
            );
        let top = (caret.map_or(window.viewport_size().height - px(220.), |b| b.top())
            - height
            - px(18.))
        .max(px(8.));
        div()
            .id("slash-picker")
            .debug_selector(|| "slash-picker".into())
            .absolute()
            .left(left)
            .top(top + px((1. - reveal) * 6.))
            .w(width)
            .h(height)
            .flex()
            .flex_col()
            .rounded_xl()
            .overflow_hidden()
            .border_1()
            .border_color(rgba(0xebebeb24))
            .bg(rgba(appearance::MENU_SURFACE))
            .opacity(reveal.clamp(0., 1.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.dismiss_slash(true, cx)))
            .child(
                div()
                    .h(px(38.))
                    .px_3()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon(
                        provider_asset(self.agent),
                        16.,
                        agent_color(self.agent),
                    ))
                    .child(
                        div()
                            .text_size(crate::theme::ui_px(12.))
                            .child(agent_name(self.agent)),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(crate::theme::ui_px(10.))
                            .text_color(rgb(MUTED))
                            .child("Commands"),
                    ),
            )
            .when(picker.matches.is_empty(), |s| {
                s.child(
                    div()
                        .p_3()
                        .text_size(crate::theme::ui_px(12.))
                        .text_color(rgb(MUTED))
                        .child("No supported command matches"),
                )
            })
            .when(!picker.matches.is_empty(), |s| {
                s.child(
                    uniform_list(
                        "slash-command-list",
                        picker.matches.len(),
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .filter_map(|ix| {
                                    let picker = this.slash.as_ref()?;
                                    let command = picker.matches.get(ix)?.clone();
                                    let selector = format!("slash-command-{}", command.name());
                                    let selected = picker.selected == ix;
                                    Some(
                                        div()
                                            .id(("slash-command", ix))
                                            .debug_selector(move || selector.clone())
                                            .h(px(46.))
                                            .mx_1()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .rounded_md()
                                            .bg(rgba(if selected {
                                                0xebebeb1c
                                            } else {
                                                0x00000000
                                            }))
                                            .cursor_pointer()
                                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                            .child(
                                                div()
                                                    .min_w(px(0.))
                                                    .flex_1()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(2.))
                                                    .child(
                                                        div()
                                                            .text_size(crate::theme::ui_px(12.))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .child(format!("/{}", command.name())),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(crate::theme::ui_px(10.))
                                                            .text_color(rgb(MUTED))
                                                            .text_ellipsis()
                                                            .child(
                                                                command.description().to_owned(),
                                                            ),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .text_size(crate::theme::ui_px(9.))
                                                    .text_color(rgb(MUTED))
                                                    .child(command.source()),
                                            )
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                if let Some(picker) = &mut this.slash {
                                                    picker.selected = ix;
                                                }
                                                this.accept_slash(false, window, cx);
                                            }))
                                            .into_any_element(),
                                    )
                                })
                                .collect()
                        }),
                    )
                    .track_scroll(picker.scroll.clone())
                    .flex_1()
                    .min_h(px(0.))
                    .w_full(),
                )
            })
            .child(
                div()
                    .h(px(28.))
                    .px_3()
                    .flex_none()
                    .flex()
                    .items_center()
                    .text_size(crate::theme::ui_px(10.))
                    .text_color(rgb(MUTED))
                    .child("↑↓ Browse · ↵ Run · Tab Complete · Esc Dismiss"),
            )
            .into_any_element()
    }
    fn refresh_native_catalog(
        &mut self,
        kind: agent_controls::CatalogKind,
        cx: &mut Context<Self>,
    ) {
        if self.controls_pending.contains(kind.key()) {
            return;
        }
        let Some(path) = self
            .agents
            .iter()
            .find(|a| a.kind == self.agent)
            .and_then(|a| a.executable.clone())
        else {
            return;
        };
        self.controls_pending.insert(kind.key());
        let agent = self.agent;
        let project = self
            .execution_root()
            .unwrap_or_else(|| self.project.clone());
        let catalog_project = project.clone();
        let task = cx
            .background_executor()
            .spawn(async move { agent_controls::inspect_catalog(&path, &project, kind) });
        cx.spawn(async move |this,cx| {
            let result=task.await;
            let _=this.update(cx,|this,cx| {
                this.controls_pending.remove(kind.key());
                match result {
                    Ok(items) => {
                        let (_,info)=this.controls.entry(agent.command()).or_insert_with(||(Instant::now(),agent_controls::ControlInfo {auth:agent_controls::AuthState::Unknown,models:Vec::new(),note:None,account:None,limits:Vec::new(),catalog:None}));
                        info.catalog=Some((catalog_project,kind,items));
                    },
                    Err(_) => this.notice=Some(format!("{} could not be read from the installed Codex. Refresh or inspect it in the native CLI.",kind.title())),
                }
                if this.modal==Some(Modal::Status) {cx.notify();}
            });
        }).detach();
    }
    fn status_label(&self) -> &str {
        if self.handoff_pending.is_some() {
            return "Preparing context";
        }
        if self.active.as_ref().is_some_and(|id| {
            self.sessions
                .iter()
                .any(|session| &session.id == id && session.agent != self.agent)
        }) {
            return "Selected for next message";
        }
        if self.active.as_ref().is_some_and(|id| {
            self.native_pending
                .values()
                .any(|request| &request.owner == id && request.current(&self.runs))
        }) {
            "Needs your response"
        } else if let Some(run) = self.active.as_ref().and_then(|id| self.runs.get(id)) {
            if run.handle.is_cancelled() {
                "Stopping"
            } else {
                run.status.as_str()
            }
        } else if self.starting {
            "Connecting"
        } else if self
            .active
            .as_ref()
            .is_some_and(|id| self.broker_children.get(id).copied().unwrap_or(0) > 0)
        {
            "Subagents working"
        } else if self
            .active
            .as_ref()
            .is_some_and(|id| self.finalizing.contains(id) || self.settling.contains_key(id))
        {
            "Saving response"
        } else if self.loading {
            "Loading conversation"
        } else {
            "Idle"
        }
    }
    fn reported_limit_windows(&self) -> Vec<usage::LimitWindow> {
        let usage = self
            .active
            .as_ref()
            .filter(|id| {
                self.sessions
                    .iter()
                    .any(|session| &session.id == *id && session.agent == self.agent)
            })
            .and_then(|id| self.usage.get(id));
        let snapshot = self.controls.get(self.agent.command());
        let controls = snapshot.map(|(_, info)| info);
        // A newly opened account snapshot wins over persisted old chat usage.
        // Later rolling reports update only their reported windows, retaining the
        // other buckets from that snapshot. No percentage is inferred.
        let mut limits = controls.map(|info| info.limits.clone()).unwrap_or_default();
        let snapshot_time = snapshot.map(|(at, _)| {
            chrono::Utc::now()
                .timestamp_millis()
                .saturating_sub(at.elapsed().as_millis().min(i64::MAX as u128) as i64)
        });
        if let Some(usage) = usage {
            for window in usage
                .limit_windows
                .iter()
                .filter(|window| window.valid())
                .take(32)
            {
                if let Some(existing) = limits.iter_mut().find(|old| old.label == window.label) {
                    if window
                        .reported_at_ms
                        .is_some_and(|at| snapshot_time.is_none_or(|snapshot| at >= snapshot))
                    {
                        let mut current = window.clone();
                        current.duration_minutes =
                            current.duration_minutes.or(existing.duration_minutes);
                        current.resets_at = current.resets_at.or(existing.resets_at);
                        *existing = current;
                    }
                } else {
                    limits.push(window.clone());
                }
            }
            if limits.is_empty()
                && let Some(used) = usage
                    .limit_used_percent
                    .filter(|n| n.is_finite() && (0. ..=100.).contains(n))
            {
                limits.push(agent_controls::LimitWindow {
                    label: "codex primary".into(),
                    used_percent: used,
                    duration_minutes: None,
                    resets_at: usage.limit_resets_at,
                    reported_at_ms: None,
                });
            }
        }
        limits.retain(usage::LimitWindow::valid);
        limits.truncate(32);
        limits
    }
    pub(super) fn usage_panel(&self, height: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let session = self.active.as_ref().and_then(|id| {
            self.sessions
                .iter()
                .find(|s| &s.id == id && s.agent == self.agent)
        });
        let usage = session.and_then(|session| self.usage.get(&session.id));
        let snapshot = self.controls.get(self.agent.command());
        let controls = snapshot.map(|(_, info)| info);
        let pending = self.controls_pending.contains(self.agent.command());
        let field = |id: &'static str, name: &'static str, value: String| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .w_full()
                .min_w(px(0.))
                .flex()
                .items_start()
                .gap_3()
                .child(div().flex_none().text_color(rgb(MUTED)).child(name))
                .child(div().flex_1().min_w(px(0.)).text_right().child(value))
        };
        let status = self.status_label();
        let limits = self.reported_limit_windows();
        let quota = div()
            .id("status-quota-windows")
            .debug_selector(|| "status-quota-windows".into())
            .w_full()
            .min_w(px(0.))
            .max_h((height * 0.52).min(px(168.)))
            .overflow_y_scroll()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .children(limits.iter().enumerate().map(|(index, limit)| {
                let duration = limit.window_title();
                div()
                    .id(("status-quota", index))
                    .debug_selector(move || format!("status-quota-{index}"))
                    .w_full()
                    .min_w(px(0.))
                    .px_2()
                    .py_2()
                    .rounded_md()
                    .bg(rgba(GLASS_SURFACE))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .id(("status-quota-value", index))
                            .debug_selector(move || format!("status-quota-value-{index}"))
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id(("status-quota-title", index))
                                    .debug_selector(move || format!("status-quota-title-{index}"))
                                    .flex_1()
                                    .min_w(px(0.))
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .child(limit.provider_title(self.agent)),
                            )
                            .child(
                                div()
                                    .id(("status-quota-remaining", index))
                                    .debug_selector(move || {
                                        format!("status-quota-remaining-{index}")
                                    })
                                    .flex_none()
                                    .child(limit.remaining_text()),
                            ),
                    )
                    .when_some(duration, |row, duration| {
                        row.child(
                            div()
                                .text_size(crate::theme::ui_px(10.))
                                .text_color(rgb(MUTED))
                                .child(duration),
                        )
                    })
                    .when_some(
                        limit
                            .resets_at
                            .filter(|t| *t > 0)
                            .and_then(|t| chrono::DateTime::from_timestamp(t, 0)),
                        |row, reset| {
                            row.child(
                                div()
                                    .id(("status-reset", index))
                                    .debug_selector(move || format!("status-reset-{index}"))
                                    .w_full()
                                    .text_size(crate::theme::ui_px(10.))
                                    .text_color(rgb(MUTED))
                                    .child(format!(
                                        "Resets {}",
                                        reset
                                            .with_timezone(&chrono::Local)
                                            .format("%b %-d at %-I:%M %p")
                                    )),
                            )
                        },
                    )
            }));
        let mut details = div()
            .id("status-details")
            .debug_selector(|| "status-details".into())
            .w_full()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2();
        if let Some(auth) = controls
            .map(|info| info.auth)
            .filter(|auth| *auth != agent_controls::AuthState::Unknown)
        {
            details = details.child(field(
                "status-account",
                "Account",
                match auth {
                    agent_controls::AuthState::VerifiedSignedIn => "Signed in",
                    agent_controls::AuthState::SignedOut => "Sign in required",
                    agent_controls::AuthState::Configured => "Credentials configured",
                    agent_controls::AuthState::Unknown => unreachable!(),
                }
                .into(),
            ));
        }
        if let Some(account) = controls
            .and_then(|info| info.account.as_ref())
            .filter(|value| !value.trim().is_empty())
        {
            details = details.child(field("status-plan", "Plan", account.clone()));
        }
        if let Some(model) = &self.model {
            details = details.child(field(
                "status-selected-model",
                "Selected model",
                model.clone(),
            ));
        }
        if let Some(model) = usage.and_then(|usage| usage.model.as_ref()) {
            details = details.child(field(
                "status-reported-model",
                "Reported model",
                model.clone(),
            ));
        }
        if let Some(id) = session.and_then(|s| s.external_id.as_ref()) {
            details = details.child(field("status-session", "Session", short(id, 26)));
        }
        details = details.child(field(
            "status-mode",
            "Mode",
            if self.allow_edits {
                "Workspace edits"
            } else {
                "Read only / plan"
            }
            .into(),
        ));
        for (id, name, value) in [
            ("status-input", "Input tokens", usage.and_then(|u| u.input)),
            (
                "status-output",
                "Output tokens",
                usage.and_then(|u| u.output),
            ),
            (
                "status-cached",
                "Cached tokens",
                usage.and_then(|u| u.cached),
            ),
            (
                "status-context-used",
                "Context used",
                usage.and_then(|u| u.context_used),
            ),
            (
                "status-context-window",
                "Context window",
                usage.and_then(|u| u.context_limit),
            ),
        ] {
            if let Some(value) = value {
                details = details.child(field(id, name, value.to_string()));
            }
        }
        if let Some(cost) = usage
            .and_then(|u| u.cost_usd)
            .filter(|n| n.is_finite() && *n >= 0.)
        {
            details = details.child(field("status-cost", "Reported cost", format!("${cost:.4}")));
        }
        if let Some((root, kind, items)) =
            controls
                .and_then(|info| info.catalog.as_ref())
                .filter(|(root, _, _)| {
                    Some(root) == self.execution_root().as_ref().or(Some(&self.project))
                })
        {
            let _ = root;
            details = details
                .child(div().text_color(rgb(MUTED)).child(kind.title()))
                .children(items.iter().map(|item| {
                    div()
                        .w_full()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(item.label.clone())
                        .child(
                            div()
                                .text_size(crate::theme::ui_px(10.))
                                .text_color(rgb(MUTED))
                                .child(item.detail.clone()),
                        )
                }));
        }
        if let Some(note) = controls
            .and_then(|info| info.note.as_ref())
            .filter(|value| !value.trim().is_empty())
        {
            details = details.child(
                div()
                    .text_size(crate::theme::ui_px(10.))
                    .text_color(rgb(MUTED))
                    .child(note.clone()),
            );
        }
        div()
            .id("status-content")
            .debug_selector(|| "status-content".into())
            .w_full()
            .min_w(px(0.))
            .h(height)
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .text_size(crate::theme::ui_px(12.))
            .child(
                div()
                    .id("status-lifecycle")
                    .debug_selector(|| "status-lifecycle".into())
                    .w_full()
                    .min_w(px(0.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon(
                        provider_asset(self.agent),
                        18.,
                        agent_color(self.agent),
                    ))
                    .child(agent_name(self.agent))
                    .child(div().flex_1())
                    .child(div().min_w(px(0.)).text_ellipsis().child(status.to_owned()))
                    .child(
                        div()
                            .id("refresh-status")
                            .debug_selector(|| "refresh-status".into())
                            .size(px(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .opacity(if pending { 0.4 } else { 1. })
                            .child(icon("refresh", 12., MUTED))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.refresh_controls(true, cx)),
                            ),
                    ),
            )
            .when(!limits.is_empty(), |body| body.child(quota))
            .child(details)
            .into_any_element()
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;
    use std::prelude::v1::test;

    fn info() -> agent_controls::ControlInfo {
        agent_controls::ControlInfo {
            auth: agent_controls::AuthState::Unknown,
            models: vec![],
            note: None,
            account: None,
            limits: vec![],
            catalog: None,
        }
    }
    #[gpui::test]
    fn selecting_another_provider_never_relabels_the_old_agents_quota_or_context(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            app.active = Some("fixture-session".into());
            let mut reported = usage::Usage::codex_limits(&serde_json::json!({"rateLimits":{"primary":{"usedPercent":26,"windowDurationMins":10080}}}), false).unwrap();
            reported.input = Some(900);
            app.usage.insert("fixture-session".into(), reported);
            app.agent = AgentKind::Claude;
            assert!(app.reported_limit_windows().is_empty());
            assert_eq!(app.status_label(), "Selected for next message");
            app.show_modal(Modal::Status, window, cx);
        });
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("status-quota-0").is_none());
        assert!(cx.debug_bounds("status-input").is_none());
        view.update(cx, |app, cx| {
            app.agent = AgentKind::Codex;
            let windows = app.reported_limit_windows();
            assert_eq!(windows.len(), 1);
            assert_eq!(windows[0].remaining_text(), "74% left");
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("status-quota-0").is_some());
        assert!(cx.debug_bounds("status-input").is_some());
    }

    #[gpui::test]
    fn quota_windows_are_visible_above_long_inventory_with_resets_on_their_own_lines(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.active = Some("fixture-session".into());
                let mut controls = info();
                controls.limits = vec![
                    agent_controls::LimitWindow {
                        label: "codex primary".into(),
                        used_percent: 0.,
                        duration_minutes: Some(300),
                        resets_at: Some(1_800_000_000),
                        reported_at_ms: None,
                    },
                    agent_controls::LimitWindow {
                        label: "codex secondary".into(),
                        used_percent: 42.,
                        duration_minutes: Some(10080),
                        resets_at: Some(1_800_600_000),
                        reported_at_ms: None,
                    },
                ];
                controls.catalog = Some((
                    app.project.clone(),
                    agent_controls::CatalogKind::Skills,
                    (0..80)
                        .map(|i| agent_controls::CatalogItem {
                            label: format!("Native skill {i}"),
                            detail: "An actual reported detail".into(),
                        })
                        .collect(),
                ));
                app.controls.insert("codex", (Instant::now(), controls));
                app.usage.insert(
                    "fixture-session".into(),
                    usage::Usage {
                        input: Some(0),
                        ..Default::default()
                    },
                );
                app.show_modal(Modal::Status, window, cx);
            })
        });
        crate::ui_tests::draw_frame(&view, cx);
        let panel = cx.debug_bounds("status-popover").unwrap();
        let lifecycle = cx.debug_bounds("status-lifecycle").unwrap();
        let quota = cx.debug_bounds("status-quota-0").unwrap();
        let value = cx.debug_bounds("status-quota-value-0").unwrap();
        let title = cx.debug_bounds("status-quota-title-0").unwrap();
        let remaining = cx.debug_bounds("status-quota-remaining-0").unwrap();
        let reset = cx.debug_bounds("status-reset-0").unwrap();
        let second_reset = cx.debug_bounds("status-reset-1").unwrap();
        let details = cx.debug_bounds("status-details").unwrap();
        assert!(panel.size.height <= px(420.) && panel.size.width <= px(360.));
        assert!(quota.top() >= lifecycle.bottom());
        assert!(reset.top() >= value.bottom());
        assert!(title.right() <= remaining.left());
        assert!(remaining.right() <= quota.right());
        assert!(second_reset.bottom() <= details.top());
        assert!(reset.right() <= panel.right() && reset.left() >= panel.left());
        assert!(
            cx.debug_bounds("status-input").is_some(),
            "Actual zero usage remains visible"
        );
        assert!(cx.debug_bounds("status-output").is_none());
    }
    #[gpui::test]
    fn missing_native_fields_have_no_placeholder_account_or_quota_rows(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.controls.insert("codex", (Instant::now(), info()));
                app.show_modal(Modal::Status, window, cx);
            })
        });
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("status-lifecycle").is_some());
        assert!(cx.debug_bounds("status-mode").is_some());
        for missing in [
            "status-account",
            "status-plan",
            "status-quota-windows",
            "status-input",
            "status-cost",
            "status-session",
            "status-selected-model",
        ] {
            assert!(
                cx.debug_bounds(missing).is_none(),
                "Unreported row should be absent: {missing}"
            );
        }
        view.update(cx, |app, _| {
            app.active = Some("fixture-session".into());
            app.starting = true;
            assert_eq!(app.status_label(), "Connecting");
            app.starting = false;
            app.finalizing.insert("fixture-session".into());
            assert_eq!(app.status_label(), "Saving response");
            app.broker_children.insert("fixture-session".into(), 2);
            assert_eq!(app.status_label(), "Subagents working");
            app.broker_children.clear();
            app.finalizing.clear();
            let run = crate::ui_tests::idle_run("partial".into());
            app.runs.insert("fixture-session".into(), run);
            assert_eq!(app.status_label(), "Working");
            let request = runtime::NativeRequest {
                id: "old-request".into(),
                method: "item/commandExecution/requestApproval".into(),
                title: "Old request".into(),
                detail: "".into(),
                kind: runtime::NativeRequestKind::Approval { choices: vec![] },
            };
            app.native_pending.insert(
                "old-request".into(),
                native_request_ui::LiveRequest::new(
                    "fixture-session",
                    uuid::Uuid::new_v4(),
                    request,
                ),
            );
            assert_eq!(
                app.status_label(),
                "Working",
                "A stale request from another run epoch is not actionable"
            );
            app.runs["fixture-session"].handle.cancel();
            assert_eq!(app.status_label(), "Stopping");
        });
    }
    #[gpui::test]
    fn sparse_quota_updates_keep_fresh_other_buckets_and_known_reset_metadata(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, _| {
            app.active = Some("fixture-session".into());
            let mut controls = info();
            controls.limits = usage::codex_limit_windows(&serde_json::json!({"rateLimits":{"primary":{"usedPercent":20,"windowDurationMins":300,"resetsAt":1800000000}}}), false);
            app.controls.insert("codex", (Instant::now(), controls));
            let mut old = usage::Usage::codex_limits(&serde_json::json!({"rateLimits":{"primary":{"usedPercent":10}}}), false).unwrap();
            old.limit_windows[0].reported_at_ms = Some(chrono::Utc::now().timestamp_millis() - 60_000);
            old.merge(usage::Usage::codex_limits(&serde_json::json!({"rateLimits":{"limitId":"review","primary":{"usedPercent":7}}}), false).unwrap());
            app.usage.insert("fixture-session".into(), old);
            let windows = app.reported_limit_windows();
            assert_eq!(windows.iter().find(|w| w.label == "codex primary").unwrap().used_percent, 20.);
            assert_eq!(windows.iter().find(|w| w.label == "review primary").unwrap().used_percent, 7.);
            let mut newer = usage::Usage::codex_limits(&serde_json::json!({"rateLimits":{"primary":{"usedPercent":25,"resetsAt":null}}}), false).unwrap();
            newer.limit_windows[0].reported_at_ms = Some(chrono::Utc::now().timestamp_millis() + 1_000);
            app.usage.get_mut("fixture-session").unwrap().merge(newer);
            let windows = app.reported_limit_windows();
            let primary = windows.iter().find(|w| w.label == "codex primary").unwrap();
            assert_eq!(primary.used_percent, 25.);
            assert_eq!(primary.resets_at, Some(1800000000));
            assert_eq!(primary.duration_minutes, Some(300));
        });
    }
}
