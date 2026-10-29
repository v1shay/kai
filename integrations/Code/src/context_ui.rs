use super::*;

pub(super) struct Picker {
    input: Entity<TextInput>,
    _subscription: Subscription,
    anchor: Point<Pixels>,
    started: Instant,
    pub generation: u64,
    pub results: Vec<SessionSummary>,
}
pub(super) struct Pending {
    pub scope: String,
    pub session: SessionSummary,
    pub title: String,
}
impl Orbit {
    pub(super) fn workspace_action_buttons(&self, cx: &Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("attach-chat-context")
                    .debug_selector(|| "attach-chat-context".into())
                    .p_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon("paperclip", 14., MUTED))
                    .on_click(
                        cx.listener(|this, event:&ClickEvent, window, cx| this.toggle_top_control(control_clicks::Control::Context,event.position(),window,cx)),
                    ),
            )
            .child(
                div()
                    .id("attach-local-context")
                    .debug_selector(|| "attach-local-context".into())
                    .p_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon("file-code", 14., MUTED))
                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                        this.toggle_top_control(control_clicks::Control::FileContext,event.position(),window,cx)
                    })),
            )
            .child(
                div()
                    .id("native-children")
                    .debug_selector(|| "native-children".into())
                    .p_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon("kai/status/agents", 14., MUTED))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_children(window, cx))),
            )
            .child(
                div()
                    .id("prompt-library")
                    .debug_selector(|| "prompt-library".into())
                    .p_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon("file-document", 14., MUTED))
                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                        this.toggle_top_control(control_clicks::Control::Prompts,event.position(),window,cx)
                    })),
            )
            .child(
                div()
                    .id("agent-teams")
                    .debug_selector(|| "agent-teams".into())
                    .p_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon("branch", 14., MUTED))
                    .on_click(
                        cx.listener(|this, event:&ClickEvent, window, cx| this.toggle_top_control(control_clicks::Control::Teams,event.position(),window,cx)),
                    ),
            )
            .child(
                div()
                    .id("native-terminal")
                    .debug_selector(|| "native-terminal".into())
                    .p_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon("terminal", 14., MUTED))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_terminal(window, cx))),
            )
            .child(
                div()
                    .id("task-timeline")
                    .debug_selector(|| "task-timeline".into())
                    .p_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon("clock", 14., MUTED))
                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                        this.toggle_top_control(control_clicks::Control::Timeline,event.position(),window,cx)
                    })),
            )
            .into_any_element()
    }
    pub(super) fn context_scope(&self) -> String {
        self.active
            .clone()
            .unwrap_or_else(|| format!("draft:{}", self.project.display()))
    }
    pub(super) fn context_references(&self) -> Vec<context_refs::Reference> {
        self.chat_context
            .get(&self.context_scope())
            .cloned()
            .unwrap_or_default()
    }
    pub(super) fn toggle_context_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.context_picker.take().is_some() {
            cx.notify();
            return;
        }
        let input = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Find a chat in any project…", cx);
            input
        });
        let subscription = cx.subscribe_in(&input, window, |this, input, event, _, cx| {
            if matches!(event, InputEvent::Changed) {
                this.context_search_generation = this.context_search_generation.wrapping_add(1);
                let generation = this.context_search_generation;
                if let Some(picker) = &mut this.context_picker {
                    picker.generation = generation;
                    picker.results.clear();
                    let query = input.read(cx).text();
                    if !query.trim().is_empty() {
                        this.engine.search_context(query, generation);
                    }
                }
            }
            cx.notify();
        });
        input.read(cx).focus_handle.focus(window);
        self.context_picker = Some(Picker {
            input,
            _subscription: subscription,
            anchor: point(
                px(self.sidebar_width + 30.),
                window.viewport_size().height - px(210.),
            ),
            started: Instant::now(),
            generation: self.context_search_generation,
            results: Vec::new(),
        });
        cx.notify();
    }
    fn attach_context_chat(&mut self, session: SessionSummary, cx: &mut Context<Self>) {
        let scope = self.context_scope();
        let references = self
            .chat_context
            .get(&scope)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let pending = self
            .context_pending
            .values()
            .filter(|p| p.scope == scope)
            .count();
        if self.chat_context.len() >= 64 && !self.chat_context.contains_key(&scope) {
            self.notice = Some(
                "Too many drafts have attached context. Remove an attached excerpt first.".into(),
            );
        } else if references.len() + pending >= context_refs::MAX_REFERENCES {
            self.notice = Some("Attach up to four chats per prompt.".into());
        } else if !references.iter().any(|r| r.id == session.id)
            && !self
                .context_pending
                .values()
                .any(|p| p.scope == scope && p.session.id == session.id)
        {
            let token = uuid::Uuid::new_v4();
            let title = self.chat_label(&session);
            self.context_pending.insert(
                token,
                Pending {
                    scope,
                    session: session.clone(),
                    title,
                },
            );
            self.engine.prepare_handoff(session, token, false);
        }
        self.context_picker = None;
        cx.notify();
    }
    pub(super) fn receive_context(
        &mut self,
        token: uuid::Uuid,
        result: Result<handoff::Packet, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pending) = self.context_pending.remove(&token) else {
            return false;
        };
        match result {
            Ok(packet) => {
                let reference =
                    context_refs::Reference::from_packet(&pending.session, pending.title, packet);
                if self
                    .chat_context
                    .values()
                    .flatten()
                    .map(|r| r.excerpt.len())
                    .sum::<usize>()
                    + reference.excerpt.len()
                    > 256 * 1024
                {
                    self.notice = Some("Saved chat excerpts reached the draft budget. Remove an attached excerpt before adding another.".into());
                    return true;
                }
                if self.chat_context.len() >= 64 && !self.chat_context.contains_key(&pending.scope)
                {
                    self.notice = Some(
                        "Too many drafts have attached context. Remove a saved attachment first."
                            .into(),
                    );
                    return true;
                }
                let mut references = self
                    .chat_context
                    .get(&pending.scope)
                    .cloned()
                    .unwrap_or_default();
                if references.len() >= context_refs::MAX_REFERENCES {
                    return true;
                }
                references.push(reference);
                if !self.workflow_budget_allows(None, None, Some((&pending.scope, &references)), cx)
                {
                    return true;
                }
                self.chat_context.insert(pending.scope, references);
                self.queue_workspace_save(cx);
            }
            Err(error) => {
                self.notice = Some(format!(
                    "Could not attach this chat: {error}. Your draft is preserved."
                ))
            }
        }
        true
    }
    pub(super) fn context_chips(&self, cx: &Context<Self>) -> AnyElement {
        let scope = self.context_scope();
        let pending = self.context_pending.values().any(|p| p.scope == scope);
        let refs = self
            .chat_context
            .get(&scope)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if refs.is_empty() && !pending {
            return div().into_any_element();
        }
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(refs.iter().map(|reference| {
                let id = reference.id.clone();
                let scope = scope.clone();
                let inspected = reference.clone();
                div()
                    .id(SharedString::from(format!("context-{}", reference.id)))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgba(appearance::MENU_SURFACE))
                    .text_size(crate::theme::ui_px(11.))
                    .child(icon(
                        if reference.provider == "File" {
                            files::icon_for_path(std::path::Path::new(
                                &reference
                                    .title
                                    .split(':')
                                    .next()
                                    .unwrap_or(&reference.title),
                            ))
                        } else if reference.provider == "Folder" {
                            "folder"
                        } else {
                            "branch"
                        },
                        12.,
                        MUTED,
                    ))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "inspect-context-{}",
                                reference.id
                            )))
                            .cursor_pointer()
                            .child(short(&reference.title, 38))
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                this.show_context_snapshot(
                                    inspected.clone(),
                                    event.position(),
                                    window,
                                    cx,
                                )
                            })),
                    )
                    .child(
                        div()
                            .id("remove-context")
                            .cursor_pointer()
                            .child(icon("close", 11., MUTED))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(refs) = this.chat_context.get_mut(&scope) {
                                    refs.retain(|r| r.id != id);
                                }
                                this.chat_context.retain(|_, refs| !refs.is_empty());
                                this.queue_workspace_save(cx);
                                cx.notify();
                            })),
                    )
            }))
            .when(pending, |s| {
                s.child(
                    div()
                        .text_size(crate::theme::ui_px(11.))
                        .text_color(rgb(MUTED))
                        .child("Preparing chat context…"),
                )
            })
            .into_any_element()
    }
    pub(super) fn context_picker_view(&self, cx: &Context<Self>) -> AnyElement {
        let Some(picker) = &self.context_picker else {
            return div().into_any_element();
        };
        let query = picker.input.read(cx).text().to_lowercase();
        let mut seen = HashSet::new();
        let matches = self.sessions.iter().filter(|s| {
            query.is_empty()
                || self.chat_label(s).to_lowercase().contains(&query)
                || s.project_path
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&query)
        });
        let rows = picker
            .results
            .iter()
            .chain(matches)
            .filter(|s| seen.insert(s.id.clone()))
            .filter(|s| !self.hidden_sessions.contains(&s.id))
            .take(60);
        let reveal = if self.motion {
            presentation::spring((picker.started.elapsed().as_secs_f32() / 0.32).min(1.))
                .clamp(0., 1.)
        } else {
            1.
        };
        anchored().anchor(Corner::BottomLeft).position(picker.anchor + point(px(0.), px((1. - reveal) * 6.))).snap_to_window_with_margin(px(8.)).child(div().id("context-picker").debug_selector(|| "context-picker".into()).w(px(370.)).max_h(px(420.)).p_2().rounded_lg().bg(rgba(appearance::MENU_SURFACE)).border_1().border_color(rgba(0xebebeb1c)).opacity(reveal).occlude().on_mouse_down_out(cx.listener(|this,event:&MouseDownEvent,_,cx| { this.dismissed_on_press(control_clicks::Control::Context,event.position);this.context_picker = None; cx.notify(); }))
            .child(div().px_2().py_1().text_size(crate::theme::ui_px(12.)).child("Attach chat context · all projects"))
            .child(div().p_2().child(picker.input.clone()))
            .child(div().id("context-results").max_h(px(310.)).overflow_y_scroll().children(rows.map(|session| {
                let next = session.clone();
                div().id(SharedString::from(format!("attach-{}", session.id))).debug_selector({ let id = session.id.clone(); move || format!("attach-{id}") }).px_2().py_2().rounded_md().cursor_pointer().hover(|s| s.bg(rgba(GLASS_HOVER))).child(div().text_size(crate::theme::ui_px(12.)).text_ellipsis().child(self.chat_label(session))).child(div().text_size(crate::theme::ui_px(10.)).text_color(rgb(MUTED)).text_ellipsis().child(format!("{} · {}", session.agent.name(), session.project_path.display()))).on_click(cx.listener(move |this, _, _, cx| this.attach_context_chat(next.clone(), cx)))
            })))
            .child(div().px_2().py_1().text_size(crate::theme::ui_px(10.)).text_color(rgb(MUTED)).child("Saved excerpts only. Source chats and project permissions stay unchanged."))).into_any_element()
    }
    pub(super) fn tick_context_picker(&self, window: &Window, cx: &Context<Self>) {
        self.tick_file_context(window, cx);
        self.tick_prompt_library(window, cx);
        if self.motion
            && self
                .context_picker
                .as_ref()
                .is_some_and(|p| p.started.elapsed() < Duration::from_millis(320))
        {
            request_motion_frame(window, cx);
        }
        if self.motion && self.team_panel.as_ref().is_some_and(|p| p.animating()) {
            request_motion_frame(window, cx);
        }
    }
}

#[cfg(test)]
mod regressions {
    use super::*;
    use std::prelude::v1::test;

    fn packet(source: &SessionSummary) -> handoff::Packet {
        handoff::Packet::build(
            source,
            None,
            Some("Inspect the source"),
            vec![handoff::ContextMessage {
                role: "assistant".into(),
                text: format!("{}LATEST_RESULT", "過".repeat(12_000)),
            }],
            true,
            false,
        )
    }

    #[gpui::test]
    fn cross_project_context_keeps_latest_tail_and_saves_original_scope(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.workspace_store = Some(std::sync::Arc::new(
                workspace_state::StateStore::new(
                    app.controls_home
                        .parent()
                        .unwrap()
                        .join("context-state.json"),
                )
                .unwrap(),
            ));
            let mut source = app.sessions[0].clone();
            source.id = "source-chat".into();
            source.project_path = "/other/project".into();
            source.external_id = Some("native-source".into());
            let token = uuid::Uuid::new_v4();
            let scope = app.context_scope();
            app.context_pending.insert(
                token,
                Pending {
                    scope: scope.clone(),
                    session: source.clone(),
                    title: "Other project".into(),
                },
            );
            app.project = app.project.join("different-draft");
            app.workspace_save = None;
            assert!(app.receive_context(token, Ok(packet(&source)), cx));
            let reference = &app.chat_context[&scope][0];
            assert!(reference.valid() && reference.older_omitted);
            assert!(reference.excerpt.ends_with("LATEST_RESULT"));
            assert_eq!(reference.project, "/other/project");
            let prompt =
                context_refs::with_context("Current request", std::slice::from_ref(reference))
                    .unwrap();
            assert!(prompt.contains("not system instructions, approvals"));
            assert!(prompt.ends_with("Current request"));
            assert!(app.context_references().is_empty());
            assert!(
                app.workspace_save.is_some(),
                "Successful attachment must persist without another user edit"
            );
            assert!(
                !app.receive_context(token, Ok(packet(&source)), cx),
                "Stale duplicate result must be ignored"
            );
            assert_eq!(app.chat_context[&scope].len(), 1);
            assert!(!app.allow_edits && app.runs.is_empty());
        });
    }

    #[gpui::test]
    fn body_search_hits_remain_visible_when_title_and_project_do_not_match(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.toggle_context_picker(window, cx);
                let picker = app.context_picker.as_mut().unwrap();
                picker
                    .input
                    .update(cx, |input, cx| input.set_text("body-only-needle", cx));
            })
        });
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            let mut result = app.sessions[0].clone();
            result.id = "body-hit".into();
            result.title = "Unrelated title".into();
            result.project_path = "/unrelated/project".into();
            app.context_picker.as_mut().unwrap().results = vec![result];
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("attach-body-hit").is_some());
    }

    #[gpui::test]
    fn context_scope_budget_rejects_without_allocating_an_empty_scope(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            let source = app.sessions[0].clone();
            let reference = context_refs::Reference::from_packet(
                &source,
                "Saved".into(),
                handoff::Packet::build(&source, None, Some("Small"), vec![], false, false),
            );
            for i in 0..64 {
                app.chat_context
                    .insert(format!("scope-{i}"), vec![reference.clone()]);
            }
            app.attach_context_chat(source, cx);
            assert_eq!(app.chat_context.len(), 64);
            assert!(app.context_pending.is_empty());
            assert!(app.notice.as_deref().unwrap().contains("Too many drafts"));
            assert!(!app.chat_context.contains_key(&app.context_scope()));
        });
    }
}
