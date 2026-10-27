//! Compact Quick Chat surface. Runtime/history/state remain owned by Orbit;
//! its native input and transcript scroll geometry belong to the Quick window.
use super::*;

impl Orbit {
    pub(super) fn prepare_quick_shortcut_controls(&mut self, cx: &mut Context<Self>) {
        if self.shortcut_recorder.is_none() {
            let shortcut = self.quick_shortcut.clone();
            let recorder = cx.new(|cx| quick_chat::ShortcutRecorder::new(shortcut, cx));
            self.shortcut_subscription =
                Some(cx.subscribe(&recorder, |this, recorder, event, cx| {
                    let quick_chat::RecorderEvent::Recorded(candidate) = event else {
                        return;
                    };
                    let result = if cx.has_global::<quick_chat::Controller>() {
                        cx.update_global::<quick_chat::Controller, _>(|service, cx| {
                            service.bind(candidate.clone(), cx)
                        })
                    } else {
                        Err(anyhow::anyhow!(
                            "Quick Chat is still starting. Try recording the shortcut again."
                        ))
                    };
                    match result {
                        Ok(()) => {
                            this.quick_shortcut = candidate.clone();
                            recorder.update(cx, |recorder, cx| {
                                recorder.set_shortcut(candidate.clone(), cx)
                            });
                            this.queue_workspace_save(cx);
                        }
                        Err(error) => recorder.update(cx, |recorder, cx| {
                            recorder.set_error(&error.to_string(), cx)
                        }),
                    }
                    cx.notify();
                }));
            self.shortcut_recorder = Some(recorder);
        }
    }
    pub(super) fn quick_shortcut_controls(&self, _: &Context<Self>) -> AnyElement {
        div()
            .mt_3()
            .pt_3()
            .border_t_1()
            .border_color(rgba(0xebebeb12))
            .when_some(self.shortcut_recorder.clone(), |view, recorder| {
                view.child(recorder)
            })
            .into_any_element()
    }

    pub(super) fn quick_chat_surface(
        &mut self,
        composer: &Entity<TextInput>,
        scroll: &ScrollHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.advance_chat_surface(window, cx);
        // The main window may be hidden or inactive while this surface owns
        // the chat. Advance its transcript reveal here too; otherwise loaded
        // rows remain at zero opacity waiting for a main-window frame.
        if ListReveal::advance(
            &mut self.transcript_reveal,
            self.motion && !native::reduced_motion(),
            self.active.is_some() && !self.loading,
            window.is_window_active(),
        ) {
            request_motion_frame(window, cx);
        }
        self.tick_composer_controls(window, cx);
        self.tick_panel_popover(window, cx);
        self.tick_slash(window, cx);
        self.refresh_slash(window, cx);
        if self.motion
            && self
                .agent_picker
                .as_ref()
                .is_some_and(|picker| picker.animating())
        {
            request_motion_frame(window, cx);
        } else if self
            .agent_picker
            .as_ref()
            .is_some_and(|picker| picker.closing)
        {
            self.agent_picker = None;
        }
        composer.update(cx, |input, _| {
            input.set_command_navigation(self.slash.is_some())
        });
        let project = self
            .project
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let title = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|session| &session.id == id))
            .map(|session| session.title.clone())
            .unwrap_or_else(|| "New chat".into());
        let run = self.active.as_ref().and_then(|id| self.runs.get(id));
        let running = run.is_some();
        let status = if self.starting_current() {
            Some("Preparing workspace…".to_owned())
        } else if self.loading {
            Some("Opening conversation…".to_owned())
        } else {
            run.map(|run| run.status.clone())
        };
        let input = composer.clone();
        let send_focus = composer.clone();
        let rows = (self.rows.len().saturating_sub(8)..self.rows.len())
            .map(|ix| self.row(ix, window, cx))
            .collect::<Vec<_>>();
        let artwork = self
            .background
            .read(cx)
            .snapshot_layer(self.active.is_none());
        div()
            .id("quick-chat-surface")
            .debug_selector(|| "quick-chat-surface".into())
            .key_context("Orbit")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .rounded(px(20.))
            .overflow_hidden()
            .bg(rgba(self.appearance.tint(0x17171788)))
            .child(div().absolute().inset_0().child(artwork))
            .child(
                div()
                    .h(px(46.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgba(0xebebeb12))
                    .child(
                        div()
                            .id("quick-project")
                            .debug_selector(|| "quick-project".into())
                            .max_w(px(210.))
                            .h(px(28.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .text_size(theme::ui_px(12.))
                            .child(icon("folder", 14., MUTED))
                            .child(div().min_w(px(0.)).text_ellipsis().child(project))
                            .child(icon("chevron-down", 10., MUTED))
                            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                                this.toggle_panel_from_click(
                                    Modal::Projects,
                                    event.position() + point(px(-12.), px(14.)),
                                    false,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        div()
                            .id("quick-chat-select")
                            .debug_selector(|| "quick-chat-select".into())
                            .flex_1()
                            .min_w(px(0.))
                            .h(px(28.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .text_size(theme::ui_px(12.))
                            .child(div().flex_1().min_w(px(0.)).text_ellipsis().child(title))
                            .child(icon("chevron-down", 10., MUTED))
                            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                                this.toggle_panel_from_click(
                                    Modal::Search,
                                    event.position() + point(px(-12.), px(14.)),
                                    false,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        div()
                            .id("quick-new-chat")
                            .debug_selector(|| "quick-new-chat".into())
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(icon("plus", 14., MUTED))
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.new_conversation(window, cx)
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("quick-hide")
                            .debug_selector(|| "quick-hide".into())
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(icon("close", 13., MUTED))
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.dispatch_action(Box::new(CloseWindow), cx);
                            })),
                    )
                    .child(
                        div()
                            .id("quick-drag-handle")
                            .w(px(24.))
                            .h_full()
                            .on_mouse_down(MouseButton::Left, |_, window, _| native::drag(window)),
                    ),
            )
            .child(
                div()
                    .id("quick-transcript")
                    .debug_selector(|| "quick-transcript".into())
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .overflow_y_scroll()
                    .track_scroll(scroll)
                    .when(rows.is_empty(), |view| {
                        view.child(
                            div()
                                .h(px(150.))
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_3()
                                .child(icon(agent_icon(self.agent), 28., agent_color(self.agent)))
                                .child(
                                    div()
                                        .text_size(theme::ui_px(18.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child("What would you like to build?"),
                                ),
                        )
                    })
                    .children(rows),
            )
            .when_some(status, |surface, status| {
                surface.child(
                    div()
                        .px_4()
                        .py_1()
                        .text_size(theme::ui_px(11.))
                        .text_color(rgb(MUTED))
                        .child(short(&status, 72)),
                )
            })
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .pb_3()
                    .pt_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.native_request_tray(cx))
                    .child(self.queue_strip(cx))
                    .child(self.context_chips(cx))
                    .when_some(self.notice.as_ref(), |view, notice| {
                        view.child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgba(0x8b742526))
                                .text_size(theme::ui_px(11.))
                                .text_color(rgb(0xddc9ac))
                                .child(short(&notice.replace(['\n', '\r'], " "), 160)),
                        )
                    })
                    .child(
                        div()
                            .id("quick-composer-card")
                            .debug_selector(|| "quick-composer-card".into())
                            .w_full()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgba(0xebebeb22))
                            .bg(rgba(self.appearance.tint(GLASS_SURFACE)))
                            .drag_over::<ExternalPaths>(|s, _, _, _| s.border_color(rgb(ACCENT)))
                            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                                this.add_attachments(
                                    paths
                                        .paths()
                                        .iter()
                                        .take(attachments::MAX_ATTACHMENTS + 1)
                                        .cloned()
                                        .collect(),
                                    cx,
                                );
                            }))
                            .child(
                                div()
                                    .px_3()
                                    .pt_3()
                                    .pb_2()
                                    .min_h(px(66.))
                                    .max_h(px(150.))
                                    .child(input),
                            )
                            .child(self.attachment_strip(cx))
                            .child(
                                div()
                                    .px_2()
                                    .pb_2()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("quick-agent-model")
                                            .debug_selector(|| "quick-agent-model".into())
                                            .max_w(px(245.))
                                            .min_w(px(0.))
                                            .h(px(28.))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .rounded_md()
                                            .cursor_pointer()
                                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                            .text_size(theme::ui_px(11.))
                                            .child(icon(
                                                agent_icon(self.agent),
                                                14.,
                                                agent_color(self.agent),
                                            ))
                                            .child(
                                                div().min_w(px(0.)).text_ellipsis().child(
                                                    self.effective_model_label(),
                                                ),
                                            )
                                            .child(icon("chevron-down", 10., MUTED))
                                            .on_click(cx.listener(
                                                |this, event: &ClickEvent, window, cx| {
                                                    this.overlay_window =
                                                        Some(window.window_handle().window_id());
                                                    this.toggle_agent_picker(
                                                        event.position(),
                                                        window,
                                                        cx,
                                                    );
                                                },
                                            )),
                                    )
                                    .child(
                                        div()
                                            .id("quick-permissions")
                                            .debug_selector(|| "quick-permissions".into())
                                            .h(px(28.))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .rounded_md()
                                            .cursor_pointer()
                                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                            .text_size(theme::ui_px(11.))
                                            .child(icon("shield", 13., MUTED))
                                            .child(if self.allow_edits {
                                                if self.agent==AgentKind::Codex && self.codex_access==runtime::CodexAccess::Full { "Full access" } else { "Build" }
                                            } else {
                                                "Read only"
                                            })
                                            .on_click(cx.listener(
                                                |this, event: &ClickEvent, window, cx| {
                                                    this.show_panel_at(
                                                        Modal::Permissions,
                                                        event.position()
                                                            + point(px(-12.), px(-20.)),
                                                        true,
                                                        window,
                                                        cx,
                                                    );
                                                },
                                            )),
                                    )
                                    .child(div().flex_1())
                                    .child(self.attachment_button(cx))
                                    .child(self.dictation_button(cx))
                                    .when(
                                        self.agent == AgentKind::Codex || self.voice_call.is_some(),
                                        |view| view.child(self.voice_button(cx)),
                                    )
                                    .child(
                                        div()
                                            .id("quick-send")
                                            .debug_selector(|| "quick-send".into())
                                            .size(px(28.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_md()
                                            .bg(gpui::rgb(self.resolved_theme.accent))
                                            .cursor_pointer()
                                            .hover(|s| s.opacity(0.88))
                                            .child(
                                                icon(
                                                    if running
                                                        && composer
                                                            .read(cx)
                                                            .text()
                                                            .trim()
                                                            .is_empty()
                                                        && self.attachments.is_empty()
                                                    {
                                                        "stop"
                                                    } else {
                                                        "arrow"
                                                    },
                                                    16.,
                                                    self.resolved_theme.on_accent,
                                                )
                                                .text_color(theme::on_accent()),
                                            )
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                if !this.cancel_send_handoff(cx) {
                                                    this.send(window, cx);
                                                }
                                                send_focus.read(cx).focus_handle.focus(window);
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .px_1()
                            .text_size(theme::ui_px(10.))
                            .text_color(rgb(MUTED))
                            .child(self.dictation_status.unwrap_or(if running {
                                "Working locally"
                            } else {
                                "Quick Chat · Esc to hide"
                            })),
                    ),
            )
            .when(
                self.agent_picker.is_some() && self.owns_overlays(window),
                |surface| surface.child(self.agent_picker_view(cx)),
            )
            .when(
                self.panel_popover.is_some() && self.owns_overlays(window),
                |surface| surface.child(self.panel_popover_view(window, cx)),
            )
            .when(
                self.slash.is_some() && self.owns_overlays(window),
                |surface| surface.child(self.slash_view_for_input(window, composer, cx)),
            )
            .when(
                self.attachment_preview.is_some() && self.owns_overlays(window),
                |surface| surface.child(self.attachment_preview_view(window, cx)),
            )
            .into_any_element()
    }
}
