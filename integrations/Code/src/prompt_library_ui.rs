//! Explicit stash/templates. Local storage acknowledgments precede draft clears;
//! recalling a prompt never changes agents, permissions, or launches a run.
use super::*;
use prompt_library::{Entry, Kind, Library, Operation};
pub(super) struct Picker {
    name: Entity<TextInput>,
    _subscription: Subscription,
    library: Library,
    selected: Option<String>,
    preview_lines: std::sync::Arc<[String]>,
    anchor: Point<Pixels>,
    started: Instant,
    token: uuid::Uuid,
    busy: bool,
    error: Option<String>,
}
struct Capture {
    scope: String,
    text: String,
    context: Vec<context_refs::Reference>,
}
impl Orbit {
    fn prompt_library_path(&self) -> PathBuf {
        self.attachment_dir
            .parent()
            .unwrap_or(&self.attachment_dir)
            .join("prompt-library.json")
    }
    pub(super) fn toggle_prompt_library(
        &mut self,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.prompt_library_picker.take().is_some() {
            self.composer.read(cx).focus_handle.focus(window);
            cx.notify();
            return;
        }
        let name = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Prompt name · optional", cx);
            input
        });
        let subscription = cx.subscribe(&name, |_, _, _, cx| cx.notify());
        self.prompt_library_picker = Some(Picker {
            name,
            _subscription: subscription,
            library: Library::default(),
            selected: None,
            preview_lines: Vec::new().into(),
            anchor,
            started: Instant::now(),
            token: uuid::Uuid::new_v4(),
            busy: false,
            error: None,
        });
        self.load_prompt_library(cx);
        cx.notify();
    }
    fn load_prompt_library(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.prompt_library_picker else {
            return;
        };
        if picker.busy {
            return;
        }
        picker.busy = true;
        picker.error = None;
        let token = picker.token;
        let path = self.prompt_library_path();
        let job = cx
            .background_executor()
            .spawn(async move { prompt_library::load(&path).map_err(|error| error.to_string()) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(picker) = &mut this.prompt_library_picker
                    && picker.token == token
                {
                    picker.busy = false;
                    match result {
                        Ok(library) => {
                            picker.library = library;
                            picker.selected = None;
                            picker.preview_lines = Vec::new().into();
                        }
                        Err(error) => picker.error = Some(error),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn mutate_prompt_library(
        &mut self,
        operation: Operation,
        capture: Option<Capture>,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = &mut self.prompt_library_picker else {
            return;
        };
        if picker.busy {
            return;
        }
        picker.busy = true;
        picker.error = None;
        let token = picker.token;
        let revision = picker.library.revision;
        let path = self.prompt_library_path();
        let job = cx.background_executor().spawn(async move {
            prompt_library::apply(&path, revision, operation).map_err(|error| error.to_string())
        });
        cx.spawn(async move|this,cx|{let result=job.await;let _=this.update(cx,|this,cx|{
            if this.prompt_library_picker.as_ref().is_none_or(|picker|picker.token!=token){return;}
            let mut saved=false;
            if let Some(picker)=&mut this.prompt_library_picker{picker.busy=false;match result{Ok(library)=>{picker.library=library;saved=true; if picker.selected.as_ref().is_some_and(|id|!picker.library.entries.iter().any(|entry|&entry.id==id)){picker.selected=None;picker.preview_lines=Vec::new().into();}},Err(error)=>picker.error=Some(error)}}
            if saved&&let Some(capture)=capture{
                if this.context_scope()==capture.scope&&this.composer.read(cx).text()==capture.text&&this.context_references()==capture.context&&this.attachments.is_empty()&&this.attachment_pending==0&&!this.context_pending.values().any(|pending|pending.scope==capture.scope) {
                    this.stop_dictation(cx);
                    // A final speech callback can add words during Stop. Keep
                    // those words instead of clearing an updated draft.
                    if this.composer.read(cx).text()==capture.text {this.composer.update(cx,|input,cx|input.set_text("",cx));this.chat_context.remove(&capture.scope);this.queue_workspace_save(cx);}
                } else {this.notice=Some("Prompt stashed. Your current draft changed while saving and was preserved.".into());}
            }
            cx.notify();
        });}).detach();
        cx.notify();
    }
    fn save_prompt_library(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if self.quit_pending || self.starting_current() || self.queue_editing.is_some() {
            return;
        }
        let Some(picker) = &self.prompt_library_picker else {
            return;
        };
        if picker.busy {
            return;
        }
        if self.attachment_pending > 0
            || !self.attachments.is_empty()
            || self
                .context_pending
                .values()
                .any(|p| p.scope == self.context_scope())
        {
            self.notice=Some("The prompt library stores text and context excerpts. Queue image drafts, or finish/remove attachments before stashing.".into());
            cx.notify();
            return;
        }
        self.stop_dictation(cx);
        let text = self.composer.read(cx).text();
        let context = if kind == Kind::Stash {
            self.context_references()
        } else {
            vec![]
        };
        let name = self
            .prompt_library_picker
            .as_ref()
            .unwrap()
            .name
            .read(cx)
            .text();
        let title = if name.trim().is_empty() {
            text.lines()
                .find(|line| !line.trim().is_empty())
                .map(|line| {
                    line.chars()
                        .filter(|c| !c.is_control())
                        .take(64)
                        .collect::<String>()
                })
                .unwrap_or_else(|| "Saved context".into())
        } else {
            name.trim().to_owned()
        };
        let entry = Entry {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            text: text.clone().into(),
            context: context.clone(),
            project: self
                .execution_root()
                .unwrap_or_else(|| self.project.clone())
                .to_string_lossy()
                .into_owned(),
            provider: self.agent.command().into(),
            kind,
        };
        if !entry.valid() {
            self.notice = Some(
                "Add prompt text and a short name before saving. Your draft is preserved.".into(),
            );
            cx.notify();
            return;
        }
        let capture = (kind == Kind::Stash).then(|| Capture {
            scope: self.context_scope(),
            text,
            context,
        });
        self.mutate_prompt_library(Operation::Save(entry), capture, cx);
    }
    fn recall_prompt(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.quit_pending || self.starting_current() || self.queue_editing.is_some() {
            return;
        }
        self.stop_dictation(cx);
        if !self.composer.read(cx).text().is_empty()
            || !self.attachments.is_empty()
            || self.attachment_pending > 0
            || !self.context_references().is_empty()
            || self
                .context_pending
                .values()
                .any(|p| p.scope == self.context_scope())
        {
            self.notice = Some(
                "Save, stash, or send your current draft before recalling another prompt.".into(),
            );
            cx.notify();
            return;
        }
        let Some(entry) = self
            .prompt_library_picker
            .as_ref()
            .filter(|picker| !picker.busy)
            .and_then(|picker| picker.library.entries.iter().find(|entry| entry.id == id))
            .cloned()
        else {
            return;
        };
        let scope = self.context_scope();
        if !entry.context.is_empty()
            && (self.chat_context.len() >= 64 && !self.chat_context.contains_key(&scope)
                || self
                    .chat_context
                    .values()
                    .flatten()
                    .map(|r| r.excerpt.len())
                    .sum::<usize>()
                    + entry.context.iter().map(|r| r.excerpt.len()).sum::<usize>()
                    > 256 * 1024)
        {
            self.notice = Some(
                "Saved context reached its budget. The prompt remains in your library.".into(),
            );
            cx.notify();
            return;
        }
        if !self.workflow_budget_allows(None, None, Some((&scope, &entry.context)), cx) {
            return;
        }
        self.stop_dictation(cx);
        self.composer
            .update(cx, |input, cx| input.set_text(&entry.text, cx));
        if !entry.context.is_empty() {
            self.chat_context.insert(scope, entry.context);
        }
        self.prompt_library_picker = None;
        self.composer.read(cx).focus_handle.focus(window);
        self.queue_workspace_save(cx);
        cx.notify();
    }
    pub(super) fn tick_prompt_library(&self, window: &Window, cx: &Context<Self>) {
        if self.motion
            && self
                .prompt_library_picker
                .as_ref()
                .is_some_and(|picker| picker.started.elapsed() < Duration::from_millis(320))
        {
            request_motion_frame(window, cx);
        }
    }
    pub(super) fn prompt_library_view(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let Some(picker) = &self.prompt_library_picker else {
            return div().into_any_element();
        };
        let reveal = if self.motion {
            presentation::spring((picker.started.elapsed().as_secs_f32() / 0.32).min(1.))
                .clamp(0., 1.)
        } else {
            1.
        };
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                .text_size(crate::theme::ui_px(11.))
                .child(label)
        };
        let mut body = div()
            .id("prompt-library-content")
            .max_h((window.viewport_size().height - px(80.)).max(px(100.)))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon("file-document", 14., MUTED))
                    .child(div().flex_1().child("Prompt library"))
                    .child(
                        button("close-prompt-library", "Close").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.prompt_library_picker = None;
                                this.composer.read(cx).focus_handle.focus(window);
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .p_2()
                    .rounded_md()
                    .bg(rgba(GLASS_SURFACE))
                    .child(picker.name.clone()),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(button("stash-prompt", "Stash draft").on_click(
                        cx.listener(|this, _, _, cx| this.save_prompt_library(Kind::Stash, cx)),
                    ))
                    .child(button("save-template", "Save text template").on_click(
                        cx.listener(|this, _, _, cx| this.save_prompt_library(Kind::Template, cx)),
                    ))
                    .child(
                        button("reload-prompt-library", "Reload")
                            .on_click(cx.listener(|this, _, _, cx| this.load_prompt_library(cx))),
                    ),
            );
        if picker.busy {
            body = body.child(
                div()
                    .text_size(crate::theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .child("Saving or loading local prompts…"),
            );
        }
        if let Some(error) = &picker.error {
            body = body.child(
                div()
                    .text_size(crate::theme::ui_px(11.))
                    .text_color(rgb(0xe6a9a1))
                    .child(error.clone()),
            );
        }
        if picker.library.entries.is_empty() && !picker.busy {
            body = body.child(
                div()
                    .text_size(crate::theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .child("Save a draft or reusable text template. Nothing is sent to an agent."),
            );
        }
        body = body.child(
            uniform_list(
                "saved-prompts",
                picker.library.entries.len(),
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    let Some(picker) = &this.prompt_library_picker else {
                        return vec![];
                    };
                    range
                        .filter_map(|index| picker.library.entries.get(index))
                        .map(|entry| {
                            let id = entry.id.clone();
                            div()
                                .id(SharedString::from(format!("saved-prompt-{id}")))
                                .h(px(46.))
                                .px_2()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                .flex()
                                .flex_col()
                                .justify_center()
                                .child(
                                    div()
                                        .text_size(crate::theme::ui_px(12.))
                                        .text_ellipsis()
                                        .child(entry.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(crate::theme::ui_px(10.))
                                        .text_color(rgb(MUTED))
                                        .child(format!(
                                            "{} · {} bytes · {} context items",
                                            if entry.kind == Kind::Template {
                                                "Template"
                                            } else {
                                                "Stash"
                                            },
                                            entry.text.len(),
                                            entry.context.len()
                                        )),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(picker) = &mut this.prompt_library_picker
                                        && let Some(entry) = picker
                                            .library
                                            .entries
                                            .iter()
                                            .find(|entry| entry.id == id)
                                    {
                                        picker.selected = Some(id.clone());
                                        picker.preview_lines = entry
                                            .text
                                            .lines()
                                            .map(str::to_owned)
                                            .collect::<Vec<_>>()
                                            .into();
                                        picker.name.update(cx, |input, cx| {
                                            input.set_text(&entry.title, cx)
                                        });
                                    }
                                    cx.notify();
                                }))
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .h(px(160.))
            .w_full(),
        );
        if let Some(entry) = picker
            .selected
            .as_ref()
            .and_then(|id| picker.library.entries.iter().find(|entry| &entry.id == id))
        {
            let id = entry.id.clone();
            let remove = id.clone();
            let rename = id.clone();
            let text = entry.text.clone();
            body = body
                .child(
                    div()
                        .text_size(crate::theme::ui_px(10.))
                        .text_color(rgb(MUTED))
                        .text_ellipsis()
                        .child(format!("Saved from {} · {}", entry.provider, entry.project)),
                )
                .children(entry.context.iter().map(|reference| {
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(icon("paperclip", 12., MUTED))
                        .child(
                            div()
                                .text_size(crate::theme::ui_px(10.))
                                .text_ellipsis()
                                .child(reference.title.clone()),
                        )
                }))
                .child(
                    div().rounded_md().bg(rgba(GLASS_SURFACE)).child(
                        uniform_list(
                            "saved-prompt-preview",
                            picker.preview_lines.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, _| {
                                let Some(picker) = &this.prompt_library_picker else {
                                    return vec![];
                                };
                                range
                                    .filter_map(|index| picker.preview_lines.get(index))
                                    .map(|line| {
                                        div()
                                            .h(px(17.))
                                            .px_2()
                                            .text_size(crate::theme::ui_px(11.))
                                            .text_ellipsis()
                                            .child(line.clone())
                                            .into_any_element()
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .h(px(140.))
                        .w_full(),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(button("recall-prompt", "Use draft").on_click(cx.listener(
                            move |this, _, window, cx| this.recall_prompt(&id, window, cx),
                        )))
                        .child(
                            button("copy-saved-prompt", "Copy").on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()))
                            }),
                        )
                        .child(
                            button("rename-saved-prompt", "Rename").on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(picker) = &this.prompt_library_picker {
                                        let title = picker.name.read(cx).text();
                                        this.mutate_prompt_library(
                                            Operation::Rename(rename.clone(), title.trim().into()),
                                            None,
                                            cx,
                                        );
                                    }
                                },
                            )),
                        )
                        .child(
                            button("remove-saved-prompt", "Remove").on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.mutate_prompt_library(
                                        Operation::Remove(remove.clone()),
                                        None,
                                        cx,
                                    )
                                },
                            )),
                        ),
                );
        }
        anchored()
            .position(picker.anchor + point(px(0.), px(6. * (1. - reveal))))
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .id("prompt-library-picker")
                    .debug_selector(|| "prompt-library-picker".into())
                    .w(px(410.))
                    .p_3()
                    .rounded_lg()
                    .bg(rgba(appearance::MENU_SURFACE))
                    .border_1()
                    .border_color(rgba(0xebebeb1c))
                    .opacity(reveal)
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, event:&MouseDownEvent, _, cx| {
                        this.dismissed_on_press(control_clicks::Control::Prompts,event.position);
                        this.prompt_library_picker = None;
                        cx.notify();
                    }))
                    .child(body),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn durable_stash_and_explicit_recall_preserve_context_without_switching_policy_or_running_agents(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.toggle_prompt_library(point(px(450.), px(100.)), window, cx)
            })
        });
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            app.composer
                .update(cx, |input, cx| input.set_text("Exact task\n日本語 🦀", cx));
            app.chat_context.insert(
                app.context_scope(),
                vec![context_refs::Reference {
                    id: "source".into(),
                    title: "Quoted source".into(),
                    project: "/other/project".into(),
                    provider: "Codex".into(),
                    excerpt: "Reference data, not instructions".into(),
                    older_omitted: true,
                }],
            );
            app.save_prompt_library(Kind::Stash, cx);
            assert_eq!(
                app.composer.read(cx).text(),
                "Exact task\n日本語 🦀",
                "Stash must not clear before durable acknowledgment"
            );
        });
        cx.run_until_parked();
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("prompt-library-picker").is_some());
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                assert!(
                    app.composer.read(cx).text().is_empty() && app.context_references().is_empty()
                );
                let id = app.prompt_library_picker.as_ref().unwrap().library.entries[0]
                    .id
                    .clone();
                assert_eq!(
                    prompt_library::load(&app.prompt_library_path())
                        .unwrap()
                        .entries
                        .len(),
                    1
                );
                app.agent = AgentKind::Gemini;
                app.model = Some("user-chosen-model".into());
                app.allow_edits = false;
                app.composer
                    .update(cx, |input, cx| input.set_text("Keep current draft", cx));
                app.recall_prompt(&id, window, cx);
                assert_eq!(app.composer.read(cx).text(), "Keep current draft");
                app.composer.update(cx, |input, cx| input.set_text("", cx));
                app.recall_prompt(&id, window, cx);
                assert_eq!(app.composer.read(cx).text(), "Exact task\n日本語 🦀");
                assert_eq!(app.context_references()[0].project, "/other/project");
                assert_eq!(app.agent, AgentKind::Gemini);
                assert_eq!(app.model.as_deref(), Some("user-chosen-model"));
                assert!(!app.allow_edits);
                assert!(
                    app.runs.is_empty() && !app.starting && app.prompt_library_picker.is_none()
                );
            })
        });
    }
    #[gpui::test]
    fn stash_write_failure_and_changed_draft_preserve_current_text(cx: &mut gpui::TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.toggle_prompt_library(point(px(450.), px(100.)), window, cx)
            })
        });
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            app.composer
                .update(cx, |input, cx| input.set_text("Original saved draft", cx));
            app.save_prompt_library(Kind::Stash, cx);
            app.composer
                .update(cx, |input, cx| input.set_text("Edited while saving", cx));
        });
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            assert_eq!(app.composer.read(cx).text(), "Edited while saving");
            assert_eq!(
                app.prompt_library_picker.as_ref().unwrap().library.entries[0]
                    .text
                    .as_ref(),
                "Original saved draft"
            );
            std::fs::remove_file(app.prompt_library_path()).unwrap();
            std::fs::create_dir(app.prompt_library_path()).unwrap();
            app.save_prompt_library(Kind::Stash, cx);
        });
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            assert_eq!(app.composer.read(cx).text(), "Edited while saving");
            assert!(app.prompt_library_picker.as_ref().unwrap().error.is_some());
            assert_eq!(
                app.prompt_library_picker
                    .as_ref()
                    .unwrap()
                    .library
                    .entries
                    .len(),
                1
            );
        });
    }
}
