//! Preview first: exact bounded local context is never sent merely by browsing.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub(super) struct Picker {
    path: Entity<TextInput>,
    range: Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
    root: PathBuf,
    scope: String,
    anchor: Point<Pixels>,
    started: Instant,
    generation: uuid::Uuid,
    cancel: Arc<AtomicBool>,
    busy: bool,
    preview: Option<file_context::Preview>,
    error: Option<String>,
    selected: Option<(String, String)>,
    inspect_only: bool,
}
impl Drop for Picker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Orbit {
    pub(super) fn toggle_file_context(
        &mut self,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_context_picker.take().is_some() {
            self.composer.read(cx).focus_handle.focus(window);
            cx.notify();
            return;
        }
        let root = self
            .execution_root()
            .unwrap_or_else(|| self.project.clone());
        let selected = self.file_browser.as_ref().and_then(|tree| {
            tree.read(cx)
                .selected_key()
                .map(std::path::Path::to_path_buf)
        });
        let path = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("File or folder inside this checkout…", cx);
            if let Some(selected) = selected {
                input.set_text(
                    &selected
                        .strip_prefix(&root)
                        .unwrap_or(&selected)
                        .to_string_lossy(),
                    cx,
                );
            }
            input
        });
        let range = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Lines, e.g. 12-40 · optional", cx);
            input
        });
        let subscriptions = [&path, &range]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, event, cx| {
                    if matches!(event, InputEvent::Changed)
                        && let Some(picker) = &mut this.file_context_picker
                    {
                        picker.cancel.store(true, Ordering::Release);
                        picker.cancel = Arc::new(AtomicBool::new(false));
                        picker.generation = uuid::Uuid::new_v4();
                        picker.preview = None;
                        picker.error = None;
                        picker.busy = false;
                    }
                    cx.notify();
                })
            })
            .collect();
        path.read(cx).focus_handle.focus(window);
        self.context_picker = None;
        self.file_context_picker = Some(Picker {
            path,
            range,
            _subscriptions: subscriptions,
            root,
            scope: self.context_scope(),
            anchor,
            started: Instant::now(),
            generation: uuid::Uuid::new_v4(),
            cancel: Arc::new(AtomicBool::new(false)),
            busy: false,
            preview: None,
            error: None,
            selected: None,
            inspect_only: false,
        });
        cx.notify();
    }
    pub(super) fn show_context_snapshot(
        &mut self,
        reference: context_refs::Reference,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = cx.new(|cx| TextInput::new(window, cx));
        let range = cx.new(|cx| TextInput::new(window, cx));
        let icon = match reference.provider.as_str() {
            "Folder" => "folder",
            "File" => files::icon_for_path(std::path::Path::new(
                reference
                    .title
                    .split(':')
                    .next()
                    .unwrap_or(&reference.title),
            )),
            _ => "branch",
        };
        let lines = reference
            .excerpt
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
            .into();
        let omitted = reference.older_omitted;
        let root = PathBuf::from(&reference.project);
        self.file_context_picker = Some(Picker {
            path,
            range,
            _subscriptions: vec![],
            root,
            scope: self.context_scope(),
            anchor,
            started: Instant::now(),
            generation: uuid::Uuid::new_v4(),
            cancel: Arc::new(AtomicBool::new(false)),
            busy: false,
            preview: Some(file_context::Preview {
                reference,
                lines,
                files: 0,
                omitted,
                icon,
            }),
            error: None,
            selected: None,
            inspect_only: true,
        });
        cx.notify();
    }
    fn browse_file_context(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = &self.file_context_picker else {
            return;
        };
        let generation = picker.generation;
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Preview context".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = selection.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    if let Some(picker) = &mut this.file_context_picker
                        && picker.generation == generation
                    {
                        picker
                            .path
                            .update(cx, |input, cx| input.set_text(&path.to_string_lossy(), cx));
                    }
                });
            }
        })
        .detach();
    }
    fn preview_file_context(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.file_context_picker else {
            return;
        };
        let path = picker.path.read(cx).text();
        let range_text = picker.range.read(cx).text();
        let range = match file_context::Lines::parse(&range_text) {
            Ok(range) => range,
            Err(error) => {
                picker.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        picker.cancel.store(true, Ordering::Release);
        picker.cancel = Arc::new(AtomicBool::new(false));
        picker.generation = uuid::Uuid::new_v4();
        picker.selected = Some((path.clone(), range_text));
        picker.preview = None;
        picker.error = None;
        picker.busy = true;
        let token = picker.generation;
        let root = picker.root.clone();
        let cancel = picker.cancel.clone();
        let job = cx.background_executor().spawn(async move {
            file_context::prepare(&root, &PathBuf::from(path), range, &cancel)
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(picker) = &mut this.file_context_picker
                    && picker.generation == token
                {
                    picker.busy = false;
                    match result {
                        Ok(preview) => picker.preview = Some(preview),
                        Err(error) => picker.error = Some(error),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn attach_file_context(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = &self.file_context_picker else {
            return;
        };
        if picker.inspect_only {
            return;
        }
        if picker.scope != self.context_scope()
            || self
                .execution_root()
                .unwrap_or_else(|| self.project.clone())
                != picker.root
        {
            self.notice = Some("Return to the chat and checkout used by this preview, or reopen the context picker.".into());
            cx.notify();
            return;
        }
        if picker.selected.as_ref()
            != Some(&(picker.path.read(cx).text(), picker.range.read(cx).text()))
        {
            self.notice = Some("Preview the changed selection before attaching it.".into());
            cx.notify();
            return;
        }
        let Some(preview) = picker.preview.as_ref() else {
            return;
        };
        let scope = picker.scope.clone();
        let reference = preview.reference.clone();
        let mut refs = self.chat_context.get(&scope).cloned().unwrap_or_default();
        let pending = self
            .context_pending
            .values()
            .filter(|pending| pending.scope == scope)
            .count();
        if refs.len() + pending >= context_refs::MAX_REFERENCES
            || (self.chat_context.len() >= 64 && !self.chat_context.contains_key(&scope))
        {
            self.notice = Some("Remove an attached context item before adding another.".into());
            cx.notify();
            return;
        }
        if self
            .chat_context
            .values()
            .flatten()
            .map(|r| r.excerpt.len())
            .sum::<usize>()
            + reference.excerpt.len()
            > 256 * 1024
        {
            self.notice = Some(
                "Saved context reached its budget. Your preview and draft are preserved.".into(),
            );
            cx.notify();
            return;
        }
        refs.push(reference);
        if !self.workflow_budget_allows(None, None, Some((&scope, &refs)), cx) {
            return;
        }
        self.chat_context.insert(scope, refs);
        self.file_context_picker = None;
        self.queue_workspace_save(cx);
        cx.notify();
    }
    pub(super) fn tick_file_context(&self, window: &Window, cx: &Context<Self>) {
        if self.motion
            && self
                .file_context_picker
                .as_ref()
                .is_some_and(|picker| picker.started.elapsed() < Duration::from_millis(320))
        {
            request_motion_frame(window, cx);
        }
    }
    pub(super) fn file_context_view(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let Some(picker) = &self.file_context_picker else {
            return div().into_any_element();
        };
        let reveal = if self.motion {
            presentation::ease_out((picker.started.elapsed().as_secs_f32() / 0.32).min(1.))
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
                .text_size(crate::theme::ui_px(12.))
                .child(label)
        };
        let mut body =
            div()
                .id("file-context-content")
                .max_h((window.viewport_size().height - px(90.)).max(px(100.)))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(icon("file-code", 15., MUTED))
                        .child(div().flex_1().child(if picker.inspect_only {
                            "Attached snapshot"
                        } else {
                            "Attach local context"
                        }))
                        .child(button("close-file-context", "Close").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.file_context_picker = None;
                                this.composer.read(cx).focus_handle.focus(window);
                                cx.notify();
                            },
                        ))),
                )
                .child(
                    div()
                        .text_size(crate::theme::ui_px(10.))
                        .text_color(rgb(MUTED))
                        .text_ellipsis()
                        .child(picker.root.to_string_lossy().into_owned()),
                )
                .when(!picker.inspect_only, |body| {
                    body.child(
                        div()
                            .p_2()
                            .rounded_md()
                            .bg(rgba(GLASS_SURFACE))
                            .child(picker.path.clone()),
                    )
                    .child(
                        div()
                            .p_2()
                            .rounded_md()
                            .bg(rgba(GLASS_SURFACE))
                            .child(picker.range.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(button("browse-file-context", "Browse…").on_click(
                                cx.listener(|this, _, _, cx| this.browse_file_context(cx)),
                            ))
                            .child(button("preview-file-context", "Preview").on_click(
                                cx.listener(|this, _, _, cx| this.preview_file_context(cx)),
                            )),
                    )
                });
        if picker.busy {
            body = body.child(
                div()
                    .text_size(crate::theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .child("Reading selected context…"),
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
        if let Some(preview) = &picker.preview {
            let excerpt = preview.reference.excerpt.clone();
            body = body
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(icon(preview.icon, 14., MUTED))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .text_ellipsis()
                                .child(preview.reference.title.clone()),
                        ),
                )
                .child(
                    div()
                        .text_size(crate::theme::ui_px(10.))
                        .text_color(rgb(MUTED))
                        .child(if picker.inspect_only {
                            format!(
                                "{} bytes · saved quoted snapshot{}",
                                excerpt.len(),
                                if preview.omitted {
                                    " · some content omitted"
                                } else {
                                    ""
                                }
                            )
                        } else {
                            format!(
                                "{} file{} · {} bytes{}",
                                preview.files,
                                if preview.files == 1 { "" } else { "s" },
                                excerpt.len(),
                                if preview.omitted {
                                    " · bounded snapshot; some content omitted"
                                } else {
                                    ""
                                }
                            )
                        }),
                )
                .child(
                    div().rounded_md().bg(rgba(GLASS_SURFACE)).child(
                        uniform_list(
                            "file-context-preview-lines",
                            preview.lines.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, _cx| {
                                let Some(preview) = this
                                    .file_context_picker
                                    .as_ref()
                                    .and_then(|picker| picker.preview.as_ref())
                                else {
                                    return vec![];
                                };
                                range
                                    .filter_map(|index| {
                                        preview.lines.get(index).map(|line| (index, line))
                                    })
                                    .map(|(index, line)| {
                                        div()
                                            .id(("context-preview-line", index))
                                            .h(px(17.))
                                            .w_full()
                                            .overflow_x_scroll()
                                            .flex()
                                            .px_2()
                                            .text_size(crate::theme::ui_px(11.))
                                            .font_family("SF Mono")
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .whitespace_nowrap()
                                                    .child(line.clone()),
                                            )
                                            .into_any_element()
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .w_full()
                        .h(px(170.)),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(copy_control::CopyControl::text("copy-file-context", excerpt.to_string()).label("Copy exact context")
                        )
                        .when(!picker.inspect_only, |body| {
                            body.child(button("attach-file-context", "Attach snapshot").on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.attach_file_context(cx);
                                    this.composer.read(cx).focus_handle.focus(window);
                                }),
                            ))
                        }),
                );
        }
        anchored()
            .position(picker.anchor + point(px(0.), px(6. * (1. - reveal))))
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .id("file-context-picker")
                    .debug_selector(|| "file-context-picker".into())
                    .w(px(410.))
                    .p_3()
                    .rounded_lg()
                    .bg(rgba(appearance::MENU_SURFACE))
                    .border_1()
                    .border_color(rgba(0xebebeb1c))
                    .opacity(reveal)
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, event:&MouseDownEvent, _, cx| {
                        this.dismissed_on_press(control_clicks::Control::FileContext,event.position);
                        this.file_context_picker = None;
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
    fn file_preview_requires_explicit_attach_and_preserves_the_composer(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                std::fs::write(app.project.join("sample.rs"), "first\nselected\nthird\n").unwrap();
                app.composer
                    .update(cx, |input, cx| input.set_text("Keep my prompt", cx));
                app.toggle_file_context(point(px(450.), px(100.)), window, cx);
                let picker = app.file_context_picker.as_ref().unwrap();
                picker
                    .path
                    .update(cx, |input, cx| input.set_text("sample.rs", cx));
                picker.range.update(cx, |input, cx| input.set_text("2", cx));
            })
        });
        cx.run_until_parked();
        view.update(cx, |app, cx| app.preview_file_context(cx));
        cx.run_until_parked();
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("file-context-picker").is_some());
        assert!(cx.debug_bounds("attach-file-context").is_some());
        view.update(cx, |app, cx| {
            assert!(app.context_references().is_empty());
            assert!(app.runs.is_empty());
            let preview = app
                .file_context_picker
                .as_ref()
                .unwrap()
                .preview
                .as_ref()
                .unwrap();
            assert_eq!(
                preview.reference.excerpt.as_ref(),
                "File: sample.rs:2-2\n\nselected\n"
            );
            app.attach_file_context(cx);
            assert_eq!(app.context_references().len(), 1);
            assert_eq!(app.context_references()[0].provider, "File");
            assert_eq!(app.composer.read(cx).text(), "Keep my prompt");
            assert!(app.file_context_picker.is_none() && app.runs.is_empty());
            let stored = app.context_references()[0].clone();
            std::fs::write(app.project.join("sample.rs"), "Changed after attachment").unwrap();
            // Inspecting an attached snapshot uses saved bytes, not a fresh read.
            app.file_context_picker = None;
            assert_eq!(stored.excerpt.as_ref(), "File: sample.rs:2-2\n\nselected\n");
        });
    }
    #[gpui::test]
    fn stale_file_preview_cannot_attach_to_another_chat_or_a_changed_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                std::fs::write(app.project.join("sample.txt"), "bounded text").unwrap();
                app.toggle_file_context(point(px(450.), px(100.)), window, cx);
                app.file_context_picker
                    .as_ref()
                    .unwrap()
                    .path
                    .update(cx, |input, cx| input.set_text("sample.txt", cx));
            })
        });
        cx.run_until_parked();
        view.update(cx, |app, cx| app.preview_file_context(cx));
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            app.active = Some("other-chat".into());
            app.attach_file_context(cx);
            assert!(app.chat_context.is_empty());
            app.active = None;
            app.file_context_picker
                .as_ref()
                .unwrap()
                .range
                .update(cx, |input, cx| input.set_text("1-2", cx));
            app.attach_file_context(cx);
            assert!(app.chat_context.is_empty());
        });
        cx.run_until_parked();
        view.update(cx, |app, _| {
            assert!(app.file_context_picker.as_ref().unwrap().preview.is_none())
        });
    }
}
