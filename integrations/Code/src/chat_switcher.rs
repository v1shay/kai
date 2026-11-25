//! A project-scoped picker which replaces one pane without closing its task.
use crate::appearance::GlassPopup as _;
use super::*;
pub(super) struct Picker {
    owner: Option<String>,
    project: PathBuf,
    anchor: Point<Pixels>,
    at: Instant,
    closing: Option<f32>,
    selection: usize,
    focus: FocusHandle,
    scroll: ScrollHandle,
}
impl Picker {
    fn progress(&self, motion: bool) -> f32 {
        if !motion || native::reduced_motion() {
            return if self.closing.is_some() { 0. } else { 1. };
        }
        let t = workspace_motion::ease_out(
            self.at.elapsed().as_secs_f32() / workspace_motion::pane_duration().as_secs_f32(),
        );
        self.closing.map_or(t, |from| from * (1. - t))
    }
}
impl Orbit {
    pub(super) fn toggle_chat_switcher(
        &mut self,
        owner: Option<String>,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .chat_switcher
            .as_ref()
            .is_some_and(|p| p.owner == owner)
        {
            self.dismiss_chat_switcher(cx);
            return;
        }
        let project = owner
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
            .map_or_else(|| self.project.clone(), |s| s.project_path.clone());
        self.dismiss_panel(window, cx);
        self.dismiss_agent_picker();
        self.action_menu = None;
        self.overlay_window = Some(window.window_handle().window_id());
        self.request_project_branch(project.clone());
        let focus = cx.focus_handle();
        focus.focus(window);
        let selection = self
            .project_chat_choices(&project)
            .iter()
            .position(|s| Some(&s.id) == owner.as_ref())
            .unwrap_or(0);
        self.chat_switcher = Some(Picker {
            selection,
            focus,
            scroll: ScrollHandle::new(),
            owner,
            project,
            anchor: anchor + point(px(-16.), px(20.)),
            at: Instant::now(),
            closing: None,
        });
        self.sync_chat_terminal_owner(cx);
        self.sync_split_terminal_visibility(cx);
        cx.notify();
    }
    pub(super) fn dismiss_chat_switcher(&mut self, cx: &mut Context<Self>) {
        if let Some(picker) = &mut self.chat_switcher {
            if picker.closing.is_none() {
                picker.closing = Some(picker.progress(self.motion));
                picker.at = Instant::now();
            }
        }
        cx.notify();
    }
    pub(super) fn tick_chat_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(picker) = &self.chat_switcher {
            if picker.closing.is_some() && picker.progress(self.motion) <= 0.001 {
                self.chat_switcher = None;
                self.composer.read(cx).focus_handle.focus(window);
                self.sync_chat_terminal_owner(cx);
                self.sync_split_terminal_visibility(cx);
            } else if self.motion
                && !native::reduced_motion()
                && picker.at.elapsed() < workspace_motion::pane_duration()
            {
                request_motion_frame(window, cx);
            }
        }
    }
    fn project_chat_choices(&self, project: &std::path::Path) -> Vec<SessionSummary> {
        let mut choices: Vec<_> = self
            .sessions
            .iter()
            .filter(|s| {
                s.project_path == project
                    && !self.hidden_sessions.contains(&s.id)
                    && !self.lifecycle.archived_chats.contains(&s.id)
            })
            .cloned()
            .collect();
        choices.sort_by_key(|s| (std::cmp::Reverse(self.activity.prompt_at(s)), s.id.clone()));
        choices
    }
    pub(super) fn choose_pane_chat(
        &mut self,
        owner: Option<String>,
        session: SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The origin is captured when the menu opens; another pane taking focus
        // cannot redirect the click. Existing peers are swapped, never duplicated.
        if self
            .chat_switcher
            .as_ref()
            .is_none_or(|p| p.owner != owner || p.project != session.project_path)
        {
            return;
        }
        self.remember_draft(cx);
        if let Some(owner) = owner.as_deref() {
            self.replace_split_chat(owner, &session.id);
        }
        self.dismiss_chat_switcher(cx);
        self.open_session(session, window, cx);
        let peers: Vec<_> = self.split_companions().map(str::to_owned).collect();
        for peer in peers { self.refresh_split_tail(&peer, cx); }
        self.sync_chat_terminal_owner(cx);
        self.sync_split_terminal_visibility(cx);
        self.queue_workspace_save(cx);
    }
    pub(super) fn render_chat_switcher(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let picker = self
            .chat_switcher
            .as_ref()
            .filter(|_| self.owns_overlays(window))?;
        let progress = picker.progress(self.motion);
        let choices = self.project_chat_choices(&picker.project);
        let current = picker.owner.clone();
        let height = (window.viewport_size().height - px(100.))
            .max(px(80.))
            .min(px(420.));
        Some(
            anchored()
                .position(picker.anchor + point(px(0.), px(8. * (1. - progress))))
                .snap_to_window_with_margin(px(8.))
                .child(
                    div()
                        .id("pane-chat-picker")
                        .debug_selector(|| "pane-chat-picker".into())
                        .w(px(320.).min(window.viewport_size().width - px(16.)))
                        .max_h(height)
                        .flex()
                        .flex_col()
                        .rounded_lg()
                        .glass_popup()
                        .border_1()
                        .border_color(rgba(0xebebeb1c))
                        .opacity(progress)
                        .occlude()
                        .track_focus(&picker.focus)
                        .on_key_down(cx.listener(|app, event: &KeyDownEvent, window, cx| {
                            let Some(picker) = app.chat_switcher.as_ref() else {
                                return;
                            };
                            let choices = app.project_chat_choices(&picker.project);
                            let owner = picker.owner.clone();
                            let selection = picker.selection.min(choices.len().saturating_sub(1));
                            match event.keystroke.key.as_str() {
                                "escape" => app.dismiss_chat_switcher(cx),
                                "enter" => {
                                    if let Some(session) = choices.get(selection) {
                                        app.choose_pane_chat(owner, session.clone(), window, cx);
                                    }
                                }
                                "up" | "down" => {
                                    if choices.is_empty() {
                                        return;
                                    }
                                    let next = if event.keystroke.key == "down" {
                                        (selection + 1) % choices.len()
                                    } else {
                                        (selection + choices.len() - 1) % choices.len()
                                    };
                                    let picker = app.chat_switcher.as_mut().unwrap();
                                    picker.selection = next;
                                    picker.scroll.scroll_to_item(next);
                                    cx.notify();
                                }
                                _ => return,
                            }
                            cx.stop_propagation();
                        }))
                        .on_mouse_down_out(
                            cx.listener(|app, _, _, cx| app.dismiss_chat_switcher(cx)),
                        )
                        .child(
                            div()
                                .h(px(36.))
                                .flex_none()
                                .px_3()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(theme::ui_px(12.))
                                .child(icon("folder", 14., MUTED))
                                .child(
                                    div()
                                        .min_w(px(0.))
                                        .truncate()
                                        .child(self.project_path_label(&picker.project)),
                                ),
                        )
                        .child(
                            div()
                                .id("pane-chat-choices")
                                .min_h(px(0.))
                                .max_h(height - px(36.))
                                .overflow_y_scroll()
                                .track_scroll(&picker.scroll)
                                .p_2()
                                .children(choices.into_iter().enumerate().map(
                                    |(index, session)| {
                                        let selected = current.as_ref() == Some(&session.id);
                                        let owner = current.clone();
                                        let key = session.id.clone();
                                        div()
                                            .id(SharedString::from(format!("pane-chat-{key}")))
                                            .debug_selector(move || format!("pane-chat-{key}"))
                                            .h(px(38.))
                                            .w_full()
                                            .flex_none()
                                            .px_2()
                                            .rounded_md()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .cursor_pointer()
                                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                            .when(index == picker.selection, |s| {
                                                s.bg(rgba(GLASS_HOVER))
                                            })
                                            .when(selected, |s| s.bg(rgba(0x88bfff22)))
                                            .child(provider_icon(
                                                agent_icon(session.agent),
                                                14.,
                                                agent_color(session.agent),
                                            ))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.))
                                                    .truncate()
                                                    .text_size(theme::ui_px(12.))
                                                    .child(self.chat_label(&session)),
                                            )
                                            .when(selected, |s| s.child(icon("check", 12., ACCENT)))
                                            .when(self.runs.contains_key(&session.id), |s| {
                                                s.child(
                                                    div()
                                                        .size(px(5.))
                                                        .rounded_full()
                                                        .bg(rgb(ACCENT)),
                                                )
                                            })
                                            .on_click(cx.listener(move |app, _, window, cx| {
                                                app.choose_pane_chat(
                                                    owner.clone(),
                                                    session.clone(),
                                                    window,
                                                    cx,
                                                );
                                            }))
                                    },
                                )),
                        ),
                )
                .into_any_element(),
        )
    }
}
