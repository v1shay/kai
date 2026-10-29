//! Anchored, finite glass dropdowns. No scrim, shadow, or idle animation task.
use super::*;

pub(super) struct PanelPopover {
    pub kind: Modal,
    pub anchor: Point<Pixels>,
    pub upward: bool,
    pub started: Instant,
    pub closing: bool,
    closing_from: f32,
    // Keep the small confirmation surface stable during its exit after Cancel/retry
    // clears the actionable pending request in Orbit.
    trust_summary: Option<(AgentKind, String)>,
}
impl PanelPopover {
    fn reveal(&self, motion: bool) -> f32 {
        if !motion {
            return if self.closing { 0. } else { 1. };
        }
        let t =
            (self.started.elapsed().as_secs_f32() / if self.closing { 0.12 } else { 0.32 }).min(1.);
        if self.closing {
            self.closing_from * (1. - t * t)
        } else {
            presentation::spring(t)
        }
    }
    fn dismiss(&mut self, motion: bool) {
        if !self.closing {
            self.closing_from = self.reveal(motion);
            self.closing = true;
            self.started = Instant::now();
        }
    }
}

impl Orbit {
    pub(super) fn toggle_panel_from_click(
        &mut self,
        kind: Modal,
        anchor: Point<Pixels>,
        upward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.panel_at_pointer_down=None;
        if self.consumed_control_click(control_clicks::Control::Panel(kind),window) || self.modal==Some(kind) {
            self.dismiss_panel(window, cx);
        } else {
            self.show_panel_at(kind, anchor, upward, window, cx);
        }
    }
    pub(super) fn show_modal(&mut self, kind: Modal, window: &mut Window, cx: &mut Context<Self>) {
        // Keyboard invocations open beside the matching rail/header controls.
        let (position, upward) = match kind {
            Modal::Search => (
                point(
                    px(if self.sidebar {
                        8.
                    } else {
                        self.sidebar_width + 268.
                    }),
                    px(80.),
                ),
                false,
            ),
            Modal::Background | Modal::Settings => {
                (point(px(8.), window.viewport_size().height - px(48.)), true)
            }
            Modal::Status
            | Modal::Permissions
            | Modal::ComposerControls
            | Modal::Handoff
            | Modal::Realtime
            | Modal::Activity => (
                point(
                    px(self.sidebar_width
                        + if self.sidebar_split && self.conversations_visible {
                            280.
                        } else {
                            20.
                        }),
                    window.viewport_size().height - px(180.),
                ),
                true,
            ),
            Modal::Projects | Modal::WorkspaceTrust | Modal::SessionConflict => (
                point(
                    px(self.sidebar_width + 272.),
                    window.viewport_size().height - px(104.),
                ),
                true,
            ),
            #[cfg(test)]
            Modal::Agents => {
                self.toggle_agent_picker(
                    point(
                        px(self.sidebar_width + 282.),
                        window.viewport_size().height - px(370.),
                    ),
                    window,
                    cx,
                );
                return;
            }
        };
        self.show_panel_at(kind, position, upward, window, cx);
    }
    pub(super) fn show_panel_at(
        &mut self,
        kind: Modal,
        anchor: Point<Pixels>,
        upward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_slash(true, cx);
        if self.modal == Some(kind) {
            self.dismiss_panel(window, cx);
            return;
        }
        self.agent_picker = None;
        self.action_menu = None;
        self.overlay_window = Some(window.window_handle().window_id());
        self.modal = Some(kind);
        if kind == Modal::Settings {
            self.prepare_appearance_settings(window, cx);
        }
        if kind == Modal::Status {
            self.refresh_controls(false, cx);
        }
        self.panel_popover = Some(PanelPopover {
            kind,
            anchor,
            upward,
            started: Instant::now(),
            closing: false,
            closing_from: 1.,
            trust_summary: match kind {
                Modal::WorkspaceTrust => self.workspace_trust.as_ref(),
                Modal::SessionConflict => self.session_conflict.as_ref(),
                _ => None,
            }
            .map(|trust| {
                let project = trust
                    .project
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Project".into());
                (trust.agent, project)
            }),
        });
        if kind == Modal::Search {
            self.visible_sessions = self
                .sessions
                .iter()
                .filter(|s| !self.hidden_sessions.contains(&s.id))
                .cloned()
                .collect();
            self.search.update(cx, |input, cx| input.set_text("", cx));
            self.search.read(cx).focus_handle.focus(window);
        } else {
            self.focus.focus(window);
        }
        cx.notify();
    }
    pub(super) fn dismiss_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = None;
        if self.motion {
            if let Some(panel) = &mut self.panel_popover {
                panel.dismiss(true);
            }
        } else {
            self.panel_popover = None;
        }
        self.composer.read(cx).focus_handle.focus(window);
        cx.notify();
    }
    pub(super) fn tick_panel_popover(&mut self, window: &mut Window, cx: &Context<Self>) {
        let Some(panel) = &mut self.panel_popover else {
            return;
        };
        // Selection/send paths already clear the logical panel. Retain its small
        // surface only for the finite exit; stale search responses stay rejected.
        if self.modal != Some(panel.kind) {
            panel.dismiss(self.motion);
        }
        let duration = if panel.closing { 120 } else { 320 };
        if !self.motion || panel.started.elapsed() >= Duration::from_millis(duration) {
            if panel.closing {
                self.panel_popover = None;
            }
        } else {
            request_motion_frame(window, cx);
        }
    }
    pub(super) fn panel_popover_view(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(panel) = &self.panel_popover else {
            return div().into_any_element();
        };
        let reveal = panel.reveal(self.motion);
        let (title, glyph, selector, width, preferred_height) = match panel.kind {
            Modal::Search => ("Search", "search", "search-popover", 360., 340.),
            Modal::Projects => ("Workspace", "folder", "workspace-popover", 320., 332.),
            Modal::Background => ("Wallpaper", "file-image", "wallpaper-popover", 340., 440.),
            Modal::Settings => ("Appearance", "settings", "settings-popover", 420., 640.),
            Modal::Status => ("Status & context", "info", "status-popover", 360., 420.),
            Modal::Activity => ("Active chats", "inbox", "activity-popover", 320., 400.),
            Modal::Realtime => (
                "RealTime",
                "assets/kai/status/listening.svg",
                "realtime-popover",
                280.,
                300.,
            ),
            Modal::Permissions => (
                "Permissions",
                "lock",
                "permissions-popover",
                400.,
                if self.agent == AgentKind::Codex {
                    600.
                } else {
                    150.
                },
            ),
            Modal::ComposerControls => ("Composer controls", "more", "composer-more-popover", 320., 490.),
            Modal::Handoff => ("Continue with", "branch", "handoff-popover", 360., 420.),
            Modal::WorkspaceTrust => (
                "Trust project",
                "folder",
                "workspace-trust-popover",
                340.,
                230.,
            ),
            Modal::SessionConflict => (
                "Continue in Code",
                "branch",
                "session-conflict-popover",
                340.,
                230.,
            ),
            #[cfg(test)]
            Modal::Agents => unreachable!("agents have their own picker"),
        };
        let height = px(preferred_height).min(window.viewport_size().height - px(16.));
        let mut body = div()
            .id("panel-scroll")
            .w_full()
            .min_w(px(0.))
            .max_h(height - px(40.))
            .overflow_y_scroll();
        match panel.kind {
            Modal::ComposerControls => { body = body.child(self.composer_more_controls(cx)); }
            Modal::Activity => {
                body = body.child(self.activity_menu(cx));
            }
            Modal::Realtime => {
                body = body.child(self.realtime_settings(cx));
            }
            Modal::Handoff => {
                body = body.child(self.handoff_panel(cx));
            }
            Modal::Status => {
                body = body
                    .overflow_hidden()
                    .child(self.usage_panel(height - px(40.), cx));
            }
            Modal::Permissions => {
                let mut modes=vec![
                    (false,runtime::CodexAccess::Workspace,"Read only / plan","Read files; sandbox blocks edits and network."),
                    (true,runtime::CodexAccess::Workspace,"Workspace edits","Edit this workspace; network and writes outside it require approval."),
                ];
                if self.agent==AgentKind::Codex {
                    modes.push((true,runtime::CodexAccess::Full,"Full access","No Codex filesystem or network sandbox. Can modify files outside this workspace."));
                    body=body.child(div().mx_3().mb_2().text_size(crate::theme::ui_px(10.)).text_color(rgb(MUTED))
                        .child(div().text_ellipsis().child(format!("Workspace · {}",self.execution_root().as_ref().unwrap_or(&self.project).display())))
                        .child(div().child("Next invocation · running work keeps its permissions")));

                }
                for (index,(allow, access, label, detail)) in modes.into_iter().enumerate() {
                    let selected=self.allow_edits==allow && (!allow || self.agent!=AgentKind::Codex || self.codex_access==access);
                    body = body.child(
                        div().id(("permission-mode", index)).debug_selector(move || format!("workspace-access-{index}"))
                            .mx_2().p_3().rounded_md().cursor_pointer()
                            .bg(rgba(if selected {GLASS_HOVER}else{0xffffff00}))
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(div().flex().items_center().gap_2().text_size(crate::theme::ui_px(12.)).child(label).child(div().flex_1())
                                .when(selected,|s|s.child(icon("check",12.,ACCENT))))
                            .child(div().text_size(crate::theme::ui_px(10.)).text_color(rgb(MUTED)).child(detail))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.allow_edits = allow;
                                this.codex_access = access;
                                this.queue_workspace_save(cx);
                                this.dismiss_panel(window, cx);
                            })),
                    );
                }
                if self.agent == AgentKind::Codex {
                    body = body.child(
                        div()
                            .mx_3()
                            .mt_2()
                            .pt_2()
                            .border_t_1()
                            .border_color(rgba(0xebebeb16))
                            .text_size(crate::theme::ui_px(11.))
                            .text_color(rgb(MUTED))
                            .child("Command approvals · next Codex turn"),
                    );
                    for (mode, id, label, detail) in [
                        (
                            runtime::CodexApprovals::Ask,
                            "ask",
                            "Ask me",
                            "Review in Code when active; Kai’s notch when you’re in another app",
                        ),
                        (
                            runtime::CodexApprovals::AutoReview,
                            "auto-review",
                            "Automatic review",
                            "Codex reviews command requests automatically",
                        ),
                        (
                            runtime::CodexApprovals::Never,
                            "never",
                            "Never ask",
                            "No prompts; the workspace access above stays unchanged.",
                        ),
                    ] {
                        body = body.child(
                            div()
                                .id(SharedString::from(format!("codex-approval-{id}")))
                                .debug_selector(move || format!("codex-approval-{id}"))
                                .mx_2()
                                .mt_1()
                                .p_2()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(rgba(if self.codex_approvals == mode {
                                    GLASS_HOVER
                                } else {
                                    0xffffff00
                                }))
                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .text_size(crate::theme::ui_px(12.))
                                        .child(label)
                                        .child(div().flex_1())
                                        .when(self.codex_approvals == mode, |s| {
                                            s.child(icon("check", 12., ACCENT))
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(crate::theme::ui_px(10.))
                                        .text_color(rgb(MUTED))
                                        .child(detail),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.codex_approvals = mode;
                                    this.queue_workspace_save(cx);
                                    this.dismiss_panel(window, cx);
                                })),
                        );
                    }
                }
            }
            Modal::WorkspaceTrust | Modal::SessionConflict => {
                let conflict = panel.kind == Modal::SessionConflict;
                if let Some((agent, project)) = &panel.trust_summary {
                    body = body.child(
                        div()
                            .p_3()
                            .pt_1()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .text_size(crate::theme::ui_px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_ellipsis()
                                    .child(project.clone()),
                            )
                            .child(
                                div()
                                    .text_size(crate::theme::ui_px(12.))
                                    .line_height(crate::theme::ui_line_height(19.))
                                    .text_color(rgb(MUTED))
                                    .child(if conflict {
                                        "This chat is open in another app. Continue with a separate copy of its history; the original stays open.".to_owned()
                                    } else {
                                        format!("Allow {} to load this project’s settings and hooks? Native tool permissions stay unchanged.", agent_name(*agent))
                                    }),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id(if conflict { "cancel-session-conflict" } else { "cancel-workspace-trust" })
                                            .debug_selector(move || if conflict { "cancel-session-conflict".into() } else { "cancel-workspace-trust".into() })
                                            .flex_1()
                                            .h(px(30.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(rgba(0xebebeb1c))
                                            .text_size(crate::theme::ui_px(12.))
                                            .cursor_pointer()
                                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                            .child("Cancel")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                if conflict { this.session_conflict = None; }
                                                else { this.workspace_trust = None; }
                                                this.dismiss_panel(window, cx);
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id(if conflict { "continue-session-copy" } else { "approve-workspace-trust" })
                                            .debug_selector(move || if conflict { "continue-session-copy".into() } else { "approve-workspace-trust".into() })
                                            .flex_1()
                                            .h(px(30.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(rgba(0x459bf74d))
                                            .bg(rgba(0x459bf72a))
                                            .text_size(crate::theme::ui_px(12.))
                                            .cursor_pointer()
                                            .hover(|s| s.bg(rgba(0x459bf74d)))
                                            .child(if conflict { "Continue with copy" } else { "Trust & retry" })
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                if conflict { this.approve_session_fork(window, cx); }
                                                else { this.approve_workspace_trust(window, cx); }
                                            })),
                                    ),
                            ),
                    );
                }
            }
            Modal::Settings => {
                body = body.px_3().py_3().child(self.appearance_settings_panel(cx));
            }
            Modal::Background => {
                body = body.child(
                    self.background
                        .update(cx, |background, cx| background.settings_panel(cx)),
                )
            }
            Modal::Search => {
                body = body
                    .child(
                        div()
                            .mx_2()
                            .mb_2()
                            .p_2()
                            .rounded_md()
                            .bg(rgba(self.appearance.tint(GLASS_SURFACE)))
                            .border_1()
                            .border_color(rgba(0xebebeb12))
                            .child(self.search.clone()),
                    )
                    .when(self.visible_sessions.is_empty(), |s| {
                        s.child(
                            div()
                                .p_3()
                                .text_size(crate::theme::ui_px(12.))
                                .text_color(rgb(MUTED))
                                .child("No conversations found"),
                        )
                    })
                    .when(!self.visible_sessions.is_empty(), |s| {
                        s.child(
                            uniform_list(
                                "search-results",
                                self.visible_sessions.len(),
                                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                    range
                                        .map(|ix| {
                                            let session = this.visible_sessions[ix].clone();
                                            let project = session
                                                .project_path
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy()
                                                .into_owned();
                                            div()
                                                .id(("search-result", ix))
                                                .debug_selector(move || {
                                                    format!("search-result-{ix}")
                                                })
                                                .w_full()
                                                .h(px(48.))
                                                .px_2()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .rounded_md()
                                                .cursor_pointer()
                                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                                .child(
                                                    icon(
                                                        agent_icon(session.agent),
                                                        15.,
                                                        agent_color(session.agent),
                                                    )
                                                    .flex_none(),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w(px(0.))
                                                        .flex()
                                                        .flex_col()
                                                        .gap(px(2.))
                                                        .child(
                                                            div()
                                                                .text_size(crate::theme::ui_px(12.))
                                                                .overflow_hidden()
                                                                .text_ellipsis()
                                                                .child(this.chat_label(&session)),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(crate::theme::ui_px(10.))
                                                                .text_color(rgb(MUTED))
                                                                .overflow_hidden()
                                                                .text_ellipsis()
                                                                .child(project),
                                                        ),
                                                )
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.open_session(
                                                            session.clone(),
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                ))
                                                .into_any_element()
                                        })
                                        .collect::<Vec<_>>()
                                }),
                            )
                            .h((height - px(88.)).min(px(240.)))
                            .w_full()
                            .px_2(),
                        )
                    });
            }
            Modal::Projects => {
                body = body
                    .child(
                        div()
                            .id("create-project-folder")
                            .debug_selector(|| "create-project-folder".into())
                            .h(px(34.))
                            .mx_2()
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .text_size(crate::theme::ui_px(12.))
                            .text_color(rgb(ACCENT))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(icon("plus", 14., ACCENT))
                            .child("Create project…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_project_folder(window, cx)
                            })),
                    )
                    .child(
                        div()
                            .id("open-folder")
                            .debug_selector(|| "open-folder".into())
                            .h(px(34.))
                            .mx_2()
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .text_size(crate::theme::ui_px(12.))
                            .text_color(rgb(ACCENT))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(icon("plus", 14., ACCENT))
                            .child("Open folder…")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_folder(window, cx)),
                            ),
                    )
                    .child(
                        uniform_list(
                            "workspace-choices",
                            self.projects.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|ix| {
                                        let project = &this.projects[ix];
                                        let path = project.path.clone();
                                        div()
                                            .id(("workspace", ix))
                                            .debug_selector(move || {
                                                format!("workspace-choice-{ix}")
                                            })
                                            .w_full()
                                            .h(px(44.))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .rounded_md()
                                            .cursor_pointer()
                                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                            .child(icon("folder", 14., MUTED).flex_none())
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.))
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(2.))
                                                    .child(
                                                        div()
                                                            .text_size(crate::theme::ui_px(12.))
                                                            .overflow_hidden()
                                                            .text_ellipsis()
                                                            .child(this.project_label(project)),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(crate::theme::ui_px(10.))
                                                            .text_color(rgb(MUTED))
                                                            .overflow_hidden()
                                                            .text_ellipsis()
                                                            .child(
                                                                path.to_string_lossy().into_owned(),
                                                            ),
                                                    ),
                                            )
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.select_project(path.clone(), window, cx)
                                            }))
                                            .into_any_element()
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .h((height - px(112.)).min(px(218.)))
                        .w_full()
                        .px_2(),
                    );
            }
            #[cfg(test)]
            Modal::Agents => unreachable!(),
        }
        anchored()
            .anchor(if panel.upward {
                Corner::BottomLeft
            } else {
                Corner::TopLeft
            })
            .position(
                panel.anchor
                    + point(
                        px(0.),
                        px(if panel.upward { -1. } else { 1. } * 8. * (1. - reveal)),
                    ),
            )
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .id(selector)
                    .debug_selector(move || selector.into())
                    .w(px(width))
                    .max_h(height)
                    .flex()
                    .flex_col()
                    .rounded_lg()
                    .bg(rgba(appearance::MENU_SURFACE))
                    .border_1()
                    .border_color(rgba(0xebebeb1c))
                    .opacity(reveal.clamp(0., 1.))
                    .occlude()
                    .on_mouse_down_out(
                        cx.listener(|this, event:&MouseDownEvent, window, cx| { if let Some(kind)=this.modal {this.dismissed_on_press(control_clicks::Control::Panel(kind),event.position);} this.dismiss_panel(window, cx); }),
                    )
                    .child(
                        div()
                            .h(px(36.))
                            .flex_none()
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(crate::theme::ui_px(12.))
                            .text_color(rgb(TEXT))
                            .child(icon(glyph, 14., MUTED))
                            .child(div().flex_1().child(title))
                            .when(matches!(panel.kind, Modal::Search | Modal::Projects), |s| {
                                s.child(
                                    div()
                                        .id("refresh")
                                        .size(px(24.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                        .child(icon("refresh", 12., MUTED))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.discovering = true;
                                            this.engine.refresh();
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .id("close-panel")
                                    .debug_selector(|| "close-panel".into())
                                    .size(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .child(icon("close", 12., MUTED))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.dismiss_panel(window, cx)
                                    })),
                            ),
                    )
                    .child(body)
                    .pb_1(),
            )
            .into_any_element()
    }
}
