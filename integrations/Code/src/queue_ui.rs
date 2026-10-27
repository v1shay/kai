use super::*;

pub(super) struct Editing {
    id: String,
    session: String,
    pub error: Option<String>,
}
impl Orbit {
    pub(super) fn enqueue_followup(&mut self, cx: &mut Context<Self>) {
        if self.queue_editing.is_some() {
            self.save_queued_edit(cx);
            return;
        }
        let Some(session) = self.active.clone() else {
            return;
        };
        if self.attachment_pending > 0
            || self
                .context_pending
                .values()
                .any(|p| p.scope == self.context_scope())
        {
            self.notice = Some("Wait for attachments to finish before queueing.".into());
            cx.notify();
            return;
        }
        let prompt = prompt_queue::Prompt {
            id: uuid::Uuid::new_v4().to_string(),
            session,
            agent: self.agent,
            model: self.model.clone(),
            allow_edits: self.allow_edits,
            codex_access: self.codex_access,
            text: self.composer.read(cx).text(),
            images: self.attachments.iter().map(|a| a.path.clone()).collect(),
            context: self.context_references(),
            held: false,
        };
        let context_bytes: usize = self
            .prompt_queue
            .iter()
            .flat_map(|p| &p.context)
            .map(|r| r.excerpt.len())
            .sum::<usize>()
            + prompt
                .context
                .iter()
                .map(|r| r.excerpt.len())
                .sum::<usize>();
        if self.prompt_queue.len() >= prompt_queue::MAX_ITEMS
            || context_bytes > 256 * 1024
            || !prompt.valid()
        {
            self.notice = Some("The follow-up queue is full, or the queued prompt exceeds 8 KB. Your draft is preserved.".into());
            cx.notify();
            return;
        }
        if !self.workflow_budget_allows(Some(&prompt), None, Some((&self.context_scope(), &[])), cx)
        {
            return;
        }
        for attachment in &self.attachments {
            attachment.mark_submitted();
        }
        self.prompt_queue.push(prompt);
        self.attachments.clear();
        self.chat_context.remove(&self.context_scope());
        self.composer.update(cx, |input, cx| input.set_text("", cx));
        self.save_workspace_now(cx);
        cx.notify();
    }
    pub(super) fn hold_queue(&mut self, session: &str, cx: &mut Context<Self>) {
        for prompt in &mut self.prompt_queue {
            if prompt.session == session {
                prompt.held = true;
            }
        }
        if self.queue_dispatch.as_ref().is_some_and(|id| {
            self.prompt_queue
                .iter()
                .any(|p| p.id == *id && p.session == session)
        }) {
            self.queue_dispatch = None;
        }
        self.queue_workspace_save(cx);
    }
    pub(super) fn queue_submitted(&mut self, submitted: Option<&str>, cx: &mut Context<Self>) {
        if let Some(id) = submitted {
            self.prompt_queue.retain(|p| p.id != id);
            if self.queue_dispatch.as_deref() == Some(id) {
                self.queue_dispatch = None;
            }
            self.queue_workspace_save(cx);
        }
    }
    pub(super) fn advance_queue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.queue_editing.is_some() {
            return;
        }
        if self.quit_pending
            || self.starting
            || self.loading
            || self.loading_earlier
            || self.handoff_pending.is_some()
            || self.send_after_load.is_some()
            || self.checkpoint_busy > 0
        {
            return;
        }
        let Some(session) = self.active.clone() else {
            return;
        };
        if self.runs.contains_key(&session)
            || self.finalizing.contains(&session)
            || self.settling.contains_key(&session)
        {
            return;
        }
        if let Some(dispatch) = self.queue_dispatch.clone() {
            let Some(prompt) = self.prompt_queue.iter().find(|p| p.id == dispatch).cloned() else {
                self.queue_dispatch = None;
                return;
            };
            if prompt.session != session {
                self.hold_queue(&prompt.session, cx);
                return;
            }
            if prompt.agent != self.agent
                || prompt.model != self.model
                || prompt.allow_edits != self.allow_edits
                || prompt.codex_access != self.codex_access
            {
                self.hold_queue(&session, cx);
                self.notice = Some("Queued prompt held because its agent, model, or permissions changed. Review the preserved draft before sending.".into());
                cx.notify();
                return;
            }
            if self
                .context_pending
                .values()
                .any(|pending| pending.scope == session)
                || self
                    .chat_context
                    .get(&session)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    != prompt.context.as_slice()
            {
                self.hold_queue(&session, cx);
                self.notice = Some("Queued prompt held because its attached context changed. Your context and draft are preserved.".into());
                cx.notify();
                return;
            }
            if self.attachment_pending > 0 {
                return;
            }
            if self.composer.read(cx).text() != prompt.text {
                self.hold_queue(&session, cx);
                return;
            }
            if self.attachments.len() != prompt.images.len() {
                self.hold_queue(&session, cx);
                self.notice = Some(
                    "A queued image could not be restored. Review the draft before sending.".into(),
                );
                cx.notify();
                return;
            }
            self.send(window, cx);
            if !self.starting && self.handoff_pending.is_none() && self.send_after_load.is_none() {
                self.hold_queue(&session, cx);
            }
            return;
        }
        if !self.composer.read(cx).text().is_empty()
            || !self.attachments.is_empty()
            || self
                .chat_context
                .get(&session)
                .is_some_and(|refs| !refs.is_empty())
            || self
                .context_pending
                .values()
                .any(|pending| pending.scope == session)
        {
            return;
        }
        let Some(prompt) = self
            .prompt_queue
            .iter()
            .find(|p| p.session == session && !p.held)
            .cloned()
        else {
            return;
        };
        if prompt.agent != self.agent {
            self.hold_queue(&session, cx);
            self.notice = Some("Queued prompts are held because this chat changed agents.".into());
            return;
        }
        if prompt.allow_edits && !self.allow_edits {
            self.hold_queue(&session, cx);
            self.notice = Some(
                "Queued workspace edits are held because you switched to read-only mode.".into(),
            );
            return;
        }
        if !self.workflow_budget_allows(None, None, Some((&session, &prompt.context)), cx) {
            self.hold_queue(&session, cx);
            return;
        }
        if prompt.agent==AgentKind::Codex && prompt.codex_access!=self.codex_access {
            self.hold_queue(&session,cx);
            self.notice=Some("Queued prompt held because workspace access changed. Review its permissions before sending.".into());
            return;
        }
        self.model = prompt.model.clone();
        self.allow_edits = prompt.allow_edits;
        self.composer
            .update(cx, |input, cx| input.set_text(&prompt.text, cx));
        self.chat_context.insert(session, prompt.context.clone());
        self.queue_dispatch = Some(prompt.id);
        self.save_workspace_now(cx);
        if prompt.images.iter().any(|p| {
            p.components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        }) {
            self.hold_queue(&prompt.session, cx);
            self.notice = Some(
                "A queued image is outside Code's attachment cache. Reattach it explicitly.".into(),
            );
            cx.notify();
            return;
        }
        if !prompt.images.is_empty() {
            self.add_saved_attachments(prompt.images, cx);
        } else {
            self.send(window, cx);
            if !self.starting && self.handoff_pending.is_none() && self.send_after_load.is_none() {
                self.hold_queue(&prompt.session, cx);
            }
        }
    }
    fn edit_queued_prompt(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.starting || self.quit_pending {
            return;
        }
        if self.queue_editing.is_some()
            || self.attachment_pending > 0
            || !self.composer.read(cx).text().is_empty()
            || !self.attachments.is_empty()
            || self
                .context_pending
                .values()
                .any(|p| p.scope == self.context_scope())
            || !self.context_references().is_empty()
        {
            self.notice =
                Some("Save or send the current draft before editing a queued prompt.".into());
            cx.notify();
            return;
        }
        let Some(prompt) = self.prompt_queue.iter().find(|p| p.id == id).cloned() else {
            return;
        };
        if self.active.as_deref() != Some(prompt.session.as_str()) {
            return;
        }
        if !self.workflow_budget_allows(None, None, Some((&prompt.session, &prompt.context)), cx) {
            return;
        }
        if let Some(saved) = self.prompt_queue.iter_mut().find(|p| p.id == id) {
            saved.held = true;
        }
        self.stop_dictation(cx);
        let invalid = prompt.images.iter().any(|path| {
            path.components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        });
        self.queue_editing = Some(Editing {
            id: prompt.id,
            session: prompt.session.clone(),
            error: invalid.then(|| "An image is outside Code's managed cache.".into()),
        });
        self.composer
            .update(cx, |input, cx| input.set_text(&prompt.text, cx));
        self.chat_context.insert(prompt.session, prompt.context);
        if invalid {
            self.notice = Some("Queued images were not imported. Remove unavailable images explicitly, or Cancel to preserve the original queue.".into());
        } else {
            self.add_saved_attachments(prompt.images, cx);
        }
        self.save_workspace_now(cx);
        cx.notify();
    }
    pub(super) fn save_queued_edit(&mut self, cx: &mut Context<Self>) {
        if self.quit_pending {
            return;
        }
        let Some(edit) = &self.queue_editing else {
            return;
        };
        if self.active.as_deref() != Some(edit.session.as_str()) {
            self.notice = Some("Return to the chat being edited, or cancel that edit.".into());
            cx.notify();
            return;
        }
        if self.attachment_pending > 0
            || edit.error.is_some()
            || self
                .context_pending
                .values()
                .any(|p| p.scope == self.context_scope())
        {
            self.notice = Some("Queued edit is preserved. Wait for images, or explicitly remove unavailable images before saving.".into());
            cx.notify();
            return;
        }
        self.stop_dictation(cx);
        self.finish_attachment_removals();
        let Some(index) = self
            .prompt_queue
            .iter()
            .position(|p| p.id == self.queue_editing.as_ref().unwrap().id)
        else {
            self.queue_editing = None;
            return;
        };
        let mut prompt = self.prompt_queue[index].clone();
        prompt.text = self.composer.read(cx).text();
        prompt.images = self.attachments.iter().map(|a| a.path.clone()).collect();
        prompt.context = self.context_references();
        if !prompt.valid() {
            self.notice = Some("Queued edit must contain text or images and fit the queue budget. Your draft is preserved.".into());
            cx.notify();
            return;
        }
        let total = self
            .prompt_queue
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .flat_map(|(_, p)| &p.context)
            .map(|r| r.excerpt.len())
            .sum::<usize>()
            + prompt
                .context
                .iter()
                .map(|r| r.excerpt.len())
                .sum::<usize>();
        if total > 256 * 1024 {
            self.notice =
                Some("Attached context exceeds the queue budget. Your edit is preserved.".into());
            cx.notify();
            return;
        }
        if !self.workflow_budget_allows(Some(&prompt), None, Some((&self.context_scope(), &[])), cx)
        {
            return;
        }
        for attachment in &self.attachments {
            attachment.mark_submitted();
        }
        self.prompt_queue[index] = prompt;
        self.reset_attachment_imports();
        self.attachments.clear();
        self.chat_context.remove(&self.context_scope());
        self.composer.update(cx, |input, cx| input.set_text("", cx));
        self.queue_editing = None;
        self.save_workspace_now(cx);
        cx.notify();
    }
    fn cancel_queued_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.queue_editing.take() else {
            return;
        };
        if self.active.as_deref() == Some(edit.session.as_str()) {
            self.stop_dictation(cx);
            self.reset_attachment_imports();
            self.attachments.clear();
            self.chat_context.remove(&edit.session);
            self.composer.update(cx, |input, cx| input.set_text("", cx));
        }
        self.save_workspace_now(cx);
        cx.notify();
    }
    pub(super) fn queue_strip(&self, cx: &Context<Self>) -> AnyElement {
        let Some(active) = &self.active else {
            return div().into_any_element();
        };
        let entries: Vec<_> = self
            .prompt_queue
            .iter()
            .filter(|p| &p.session == active)
            .collect();
        if entries.is_empty() {
            return div().into_any_element();
        }
        div()
            .w_full()
            .max_w(px(800.))
            .max_h(px(160.))
            .id("followup-queue")
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1()
            .children(entries.iter().map(|prompt| {
                let id = prompt.id.clone();
                let busy = self.queue_dispatch.as_ref() == Some(&id)
                    || self
                        .queue_editing
                        .as_ref()
                        .is_some_and(|edit| edit.id == id);
                let mut row = div()
                    .id(SharedString::from(format!("queue-{id}")))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgba(appearance::MENU_SURFACE))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(crate::theme::ui_px(11.))
                    .child(div().flex_1().min_w(px(0.)).text_ellipsis().child(format!(
                        "{}{}",
                        if prompt.held {
                            "Held · "
                        } else {
                            "Queued · "
                        },
                        short(&prompt.text.replace('\n', " "), 80)
                    )));
                if self
                    .queue_editing
                    .as_ref()
                    .is_some_and(|edit| edit.id == id)
                {
                    row = row
                        .child(
                            div()
                                .id("save-queued-edit")
                                .cursor_pointer()
                                .child("Save")
                                .on_click(cx.listener(|this, _, _, cx| this.save_queued_edit(cx))),
                        )
                        .child(
                            div()
                                .id("cancel-queued-edit")
                                .cursor_pointer()
                                .child("Cancel")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.cancel_queued_edit(cx)),
                                ),
                        );
                    if self
                        .queue_editing
                        .as_ref()
                        .is_some_and(|edit| edit.error.is_some())
                    {
                        row = row.child(
                            div()
                                .id("discard-unavailable-queue-images")
                                .cursor_pointer()
                                .child("Remove unavailable images")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(edit) = &mut this.queue_editing {
                                        edit.error = None;
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                }
                if !busy {
                    let move_id = id.clone();
                    row = row.child(div().id("queue-up").cursor_pointer().child("↑").on_click(
                        cx.listener(move |this, _, _, cx| {
                            if let Some(index) =
                                this.prompt_queue.iter().position(|p| p.id == move_id)
                                && let Some(previous) = this.prompt_queue[..index]
                                    .iter()
                                    .rposition(|p| p.session == this.prompt_queue[index].session)
                            {
                                this.prompt_queue.swap(previous, index);
                                this.queue_workspace_save(cx);
                                cx.notify();
                            }
                        }),
                    ));
                    let edit_id = id.clone();
                    row = row.child(
                        div()
                            .id("queue-edit")
                            .cursor_pointer()
                            .child("Edit")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit_queued_prompt(&edit_id, cx);
                            })),
                    );
                    if prompt.held {
                        let resume_id = id.clone();
                        row = row.child(
                            div()
                                .id("queue-resume")
                                .cursor_pointer()
                                .child("Resume")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(prompt) =
                                        this.prompt_queue.iter_mut().find(|p| p.id == resume_id)
                                    {
                                        prompt.held = false;
                                    }
                                    this.queue_workspace_save(cx);
                                    cx.notify();
                                })),
                        );
                    }
                    row = row.child(
                        div()
                            .id("queue-remove")
                            .cursor_pointer()
                            .child(icon("close", 11., MUTED))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.prompt_queue.retain(|p| p.id != id);
                                this.queue_workspace_save(cx);
                                cx.notify();
                            })),
                    );
                }
                row
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod regressions {
    use super::*;
    use std::prelude::v1::test;

    fn queued(allow_edits: bool) -> prompt_queue::Prompt {
        prompt_queue::Prompt {
            id: uuid::Uuid::new_v4().to_string(),
            session: "fixture-session".into(),
            agent: AgentKind::Codex,
            model: None,
            allow_edits,
            codex_access: Default::default(),
            text: "Review when ready".into(),
            images: vec![],
            context: vec![],
            held: false,
        }
    }

    #[gpui::test]
    fn queueing_during_a_run_preserves_live_response_and_never_cancels_it(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            app.runs.insert(
                "fixture-session".into(),
                crate::ui_tests::idle_run("Still working".into()),
            );
            app.composer
                .update(cx, |input, cx| input.set_text("Next request", cx));
            app.enqueue_followup(cx);
            assert_eq!(app.prompt_queue.len(), 1);
            assert_eq!(app.prompt_queue[0].text, "Next request");
            assert!(app.composer.read(cx).text().is_empty());
            assert_eq!(app.runs["fixture-session"].text, "Still working");
            assert!(!app.runs["fixture-session"].handle.is_cancelled());
            app.hold_queue("fixture-session", cx);
            assert!(app.prompt_queue[0].held);
            assert!(!app.runs["fixture-session"].handle.is_cancelled());
        });
    }

    #[gpui::test]
    fn queued_edits_never_escalate_a_read_only_chat(cx: &mut gpui::TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.active = Some("fixture-session".into());
                app.allow_edits = false;
                app.prompt_queue.push(queued(true));
                app.advance_queue(window, cx);
                assert!(!app.allow_edits && app.runs.is_empty() && !app.starting);
                assert!(app.prompt_queue[0].held);
                assert!(app.composer.read(cx).text().is_empty());
                assert!(app.notice.as_deref().unwrap().contains("read-only"));
            })
        });
    }

    #[gpui::test]
    fn failed_queued_image_restore_holds_original_prompt_and_preserves_recovered_text(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.active = Some("fixture-session".into());
                let mut prompt = queued(false);
                prompt.images.push(app.attachment_dir.join("missing.png"));
                app.prompt_queue.push(prompt);
                app.advance_queue(window, cx);
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.advance_queue(window, cx);
                assert_eq!(app.composer.read(cx).text(), "Review when ready");
                assert_eq!(app.prompt_queue.len(), 1);
                assert!(app.prompt_queue[0].held);
                assert!(app.queue_dispatch.is_none());
                assert!(app.runs.is_empty() && !app.starting && app.attachments.is_empty());
            })
        });
    }

    #[gpui::test]
    fn restart_holds_queued_prompts_before_any_dispatch(cx: &mut gpui::TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.restore_pending = Some(workspace_state::WorkspaceState {
                    prompt_queue: vec![queued(false)],
                    ..Default::default()
                });
                app.restore_workspace(window, cx);
                assert!(app.prompt_queue[0].held);
                assert!(app.queue_dispatch.is_none() && app.runs.is_empty() && !app.starting);
            })
        });
    }
    #[gpui::test]
    fn editing_unreadable_images_keeps_the_original_until_an_explicit_decision(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            let mut prompt = queued(false);
            prompt.images.push(app.attachment_dir.join("missing.png"));
            let id = prompt.id.clone();
            app.prompt_queue.push(prompt);
            app.edit_queued_prompt(&id, cx);
            assert_eq!(app.prompt_queue.len(), 1);
            assert!(app.prompt_queue[0].held);
        });
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            assert!(app.queue_editing.as_ref().unwrap().error.is_some());
            app.composer
                .update(cx, |input, cx| input.set_text("Corrected request", cx));
            app.save_queued_edit(cx);
            assert_eq!(app.prompt_queue[0].text, "Review when ready");
            assert_eq!(app.prompt_queue[0].images.len(), 1);
            assert_eq!(app.composer.read(cx).text(), "Corrected request");
            assert!(app.queue_editing.is_some());
            app.queue_editing.as_mut().unwrap().error = None; // explicit Remove unavailable images
            app.save_queued_edit(cx);
            assert_eq!(app.prompt_queue[0].text, "Corrected request");
            assert!(app.prompt_queue[0].held && app.prompt_queue[0].images.is_empty());
            assert!(app.queue_editing.is_none() && app.composer.read(cx).text().is_empty());
            assert!(app.runs.is_empty() && !app.starting);
        });
    }

    #[gpui::test]
    fn queue_edit_does_not_overwrite_existing_drafts_or_import_outside_cache(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            let mut prompt = queued(false);
            prompt.images.push("/tmp/outside.png".into());
            let id = prompt.id.clone();
            app.prompt_queue.push(prompt);
            app.composer
                .update(cx, |input, cx| input.set_text("Existing draft", cx));
            app.edit_queued_prompt(&id, cx);
            assert_eq!(app.composer.read(cx).text(), "Existing draft");
            assert!(app.queue_editing.is_none());
            app.composer.update(cx, |input, cx| input.set_text("", cx));
            app.edit_queued_prompt(&id, cx);
        });
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            assert_eq!(app.attachment_pending, 0);
            assert!(app.queue_editing.as_ref().unwrap().error.is_some());
            app.cancel_queued_edit(cx);
            assert_eq!(
                app.prompt_queue[0].images,
                [PathBuf::from("/tmp/outside.png")]
            );
            assert!(app.prompt_queue[0].held && app.queue_editing.is_none());
        });
    }
    #[gpui::test]
    fn editing_queued_images_reuses_managed_bytes_and_never_follows_cache_symlinks(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let original = view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            let source = app.controls_home.join("image.png");
            image::RgbaImage::from_pixel(4, 4, image::Rgba([40, 90, 120, 255]))
                .save(&source)
                .unwrap();
            let item = attachments::import(
                &source,
                &app.attachment_dir,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
            item.mark_submitted();
            let path = item.path.clone();
            let mut prompt = queued(false);
            prompt.images.push(path.clone());
            let id = prompt.id.clone();
            app.prompt_queue.push(prompt);
            app.edit_queued_prompt(&id, cx);
            path
        });
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            assert_eq!(app.attachments.len(), 1);
            assert_eq!(app.attachments[0].path, original.canonicalize().unwrap());
            assert_eq!(std::fs::read_dir(&app.attachment_dir).unwrap().count(), 1);
            app.save_queued_edit(cx);
            assert_eq!(
                app.prompt_queue[0].images,
                [original.canonicalize().unwrap()]
            );
            assert!(app.prompt_queue[0].held && app.queue_editing.is_none());
            let outside = app.controls_home.join("image.png");
            let link = app.attachment_dir.join("escaped.png");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            app.prompt_queue[0].images = vec![link];
            let id = app.prompt_queue[0].id.clone();
            app.edit_queued_prompt(&id, cx);
        });
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            assert!(app.attachments.is_empty());
            assert!(app.queue_editing.as_ref().unwrap().error.is_some());
            app.save_queued_edit(cx);
            assert_eq!(app.prompt_queue[0].images.len(), 1);
            assert!(app.queue_editing.is_some());
        });
    }
    #[gpui::test]
    fn encoded_metadata_budget_rejects_before_clearing_draft_or_existing_work(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            let reference = context_refs::Reference {
                id: "source".into(),
                title: "Reference".into(),
                project: "/another/project".into(),
                provider: "Codex".into(),
                excerpt: "\u{1}".repeat(context_refs::MAX_EXCERPT).into(),
                older_omitted: true,
            };
            app.teams = (0..4)
                .map(|i| teams::Team {
                    id: uuid::Uuid::new_v4().to_string(),
                    title: "Saved team".into(),
                    project: app.project.clone(),
                    allow_edits: false,
                    created_at: 0,
                    context: vec![reference.clone(); if i == 3 { 2 } else { 4 }],
                    members: (0..2)
                        .map(|_| teams::Member {
                            session: uuid::Uuid::new_v4().to_string(),
                            agent: AgentKind::Codex,
                            model: None,
                            task: "Read".into(),
                            status: teams::Status::Held,
                            attempted: false,
                            cancel: Default::default(),
                        })
                        .collect(),
                })
                .collect();
            assert!(workspace_state::workflow_fits(
                &app.prompt_queue,
                &app.teams,
                &app.chat_context
            ));
            let draft = "\u{1}".repeat(prompt_queue::MAX_TEXT);
            app.composer
                .update(cx, |input, cx| input.set_text(&draft, cx));
            app.enqueue_followup(cx);
            assert!(app.prompt_queue.is_empty());
            assert_eq!(app.composer.read(cx).text(), draft);
            assert_eq!(app.teams.len(), 4);
            assert!(app.notice.as_deref().unwrap().contains("workflow metadata"));
            assert!(app.runs.is_empty() && !app.starting);
        });
    }
    #[gpui::test]
    fn queued_dispatch_revalidates_agent_model_and_both_permission_changes(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        for (agent, model, queued_edits, current_edits) in [
            (AgentKind::Claude, None, false, false),
            (
                AgentKind::Codex,
                Some("different-model".to_owned()),
                false,
                false,
            ),
            (AgentKind::Codex, None, true, false),
            (AgentKind::Codex, None, false, true),
        ] {
            cx.update(|window, cx| {
                view.update(cx, |app, cx| {
                    app.active = Some("fixture-session".into());
                    app.agent = agent;
                    app.model = model.clone();
                    app.allow_edits = current_edits;
                    let prompt = queued(queued_edits);
                    app.composer
                        .update(cx, |input, cx| input.set_text(&prompt.text, cx));
                    app.queue_dispatch = Some(prompt.id.clone());
                    app.prompt_queue = vec![prompt];
                    app.advance_queue(window, cx);
                    assert!(app.prompt_queue[0].held && app.queue_dispatch.is_none());
                    assert_eq!(app.agent, agent);
                    assert_eq!(app.model, model);
                    assert_eq!(app.allow_edits, current_edits);
                    assert_eq!(app.composer.read(cx).text(), "Review when ready");
                    assert!(
                        app.notice
                            .as_deref()
                            .unwrap()
                            .contains("agent, model, or permissions changed")
                    );
                    assert!(app.runs.is_empty() && !app.starting);
                })
            });
        }
    }
    fn context(excerpt: &str) -> context_refs::Reference {
        context_refs::Reference {
            id: "same-source".into(),
            title: "Saved findings".into(),
            project: "/other/project".into(),
            provider: "Codex".into(),
            excerpt: excerpt.into(),
            older_omitted: false,
        }
    }

    #[gpui::test]
    fn fresh_queued_dispatch_preserves_context_only_and_pending_context_drafts(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        for pending in [false, true] {
            cx.update(|window, cx| {
                view.update(cx, |app, cx| {
                    app.active = Some("fixture-session".into());
                    app.prompt_queue = vec![queued(false)];
                    app.chat_context.clear();
                    app.context_pending.clear();
                    if pending {
                        app.context_pending.insert(
                            uuid::Uuid::new_v4(),
                            context_ui::Pending {
                                scope: "fixture-session".into(),
                                session: app.sessions[0].clone(),
                                title: "Incoming context".into(),
                            },
                        );
                    } else {
                        app.chat_context.insert(
                            "fixture-session".into(),
                            vec![context("New context-only draft")],
                        );
                    }
                    app.advance_queue(window, cx);
                    assert!(app.queue_dispatch.is_none() && !app.prompt_queue[0].held);
                    assert!(app.composer.read(cx).text().is_empty());
                    assert!(app.runs.is_empty() && !app.starting);
                    if pending {
                        assert_eq!(app.context_pending.len(), 1);
                    } else {
                        assert_eq!(
                            app.chat_context["fixture-session"][0].excerpt.as_ref(),
                            "New context-only draft"
                        );
                    }
                })
            });
        }
    }

    #[gpui::test]
    fn queued_dispatch_holds_if_context_changes_or_is_pending_during_image_restore(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        for pending in [false, true] {
            cx.update(|window, cx| {
                view.update(cx, |app, cx| {
                    app.active = Some("fixture-session".into());
                    let mut prompt = queued(false);
                    prompt.context = vec![context("Captured findings")];
                    app.queue_dispatch = Some(prompt.id.clone());
                    app.chat_context
                        .insert("fixture-session".into(), prompt.context.clone());
                    app.context_pending.clear();
                    if pending {
                        app.context_pending.insert(
                            uuid::Uuid::new_v4(),
                            context_ui::Pending {
                                scope: "fixture-session".into(),
                                session: app.sessions[0].clone(),
                                title: "Incoming context".into(),
                            },
                        );
                    } else {
                        app.chat_context.get_mut("fixture-session").unwrap()[0].excerpt =
                            "Revised findings, same source ID".into();
                    }
                    app.composer
                        .update(cx, |input, cx| input.set_text(&prompt.text, cx));
                    app.prompt_queue = vec![prompt];
                    app.attachment_pending = 1; // restoration has not completed yet
                    app.advance_queue(window, cx);
                    assert!(app.queue_dispatch.is_none() && app.prompt_queue[0].held);
                    assert_eq!(app.composer.read(cx).text(), "Review when ready");
                    assert!(
                        app.notice
                            .as_deref()
                            .unwrap()
                            .contains("attached context changed")
                    );
                    assert!(app.runs.is_empty() && !app.starting);
                    if pending {
                        assert_eq!(app.context_pending.len(), 1);
                    } else {
                        assert_eq!(
                            app.chat_context["fixture-session"][0].excerpt.as_ref(),
                            "Revised findings, same source ID"
                        );
                    }
                    app.attachment_pending = 0;
                })
            });
        }
    }
}
