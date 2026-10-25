//! One bounded native editing buffer, retained by Files across navigation.
use crate::theme::{rgb, rgba};
use crate::{
    editor::{self, Document, Draft},
    input::{InputEvent, TextInput},
};
use gpui::{prelude::*, *};
pub struct Close;
pub struct FileEditor {
    document: Document,
    input: Entity<TextInput>,
    changed: bool,
    revision: u64,
    saving: bool,
    error: Option<String>,
    discard: bool,
    _input: Subscription,
}
impl EventEmitter<Close> for FileEditor {}
impl FileEditor {
    pub fn new(
        document: Document,
        text: Option<String>,
        restored: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = text.unwrap_or_else(|| document.text.to_string());
        let changed = !document.matches(&text);
        let input = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_editor_mode();
            input.set_text(&text, cx);
            input
        });
        input.read(cx).focus_handle.focus(window);
        let subscription = cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Save => this.save(cx),
            InputEvent::Changed => {
                this.changed = !this.document.matches(&this.input.read(cx).text());
                this.revision = this.revision.wrapping_add(1);
                this.discard = false;
                cx.notify();
            }
            InputEvent::LimitReached => {
                this.error =
                    Some("Editor limit is 64 KiB; the rest of the draft was not inserted.".into());
                cx.notify();
            }
            _ => {}
        });
        let this = Self {
            document,
            input,
            changed,
            revision: 0,
            saving: false,
            error: None,
            discard: false,
            _input: subscription,
        };
        if restored {
            let baseline = this.document.clone();
            let document = baseline.clone();
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { editor::verify(&document).map_err(|e| e.to_string()) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.document == baseline
                        && let Err(error) = result
                    {
                        this.error = Some(error);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        this
    }
    pub fn draft(&self, cx: &App) -> Draft {
        Draft {
            document: self.document.clone(),
            text: self.input.read(cx).text(),
        }
    }
    pub fn dirty(&self) -> bool {
        self.changed
    }
    pub fn busy(&self) -> bool {
        self.saving
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let text = self.input.read(cx).text();
        let document = self.document.clone();
        self.saving = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { editor::save(&document, &text).map_err(|e| e.to_string()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(document) => {
                        this.document = document;
                        this.changed = !this.document.matches(&this.input.read(cx).text());
                        this.revision = this.revision.wrapping_add(1);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
impl Render for FileEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filename = self
            .document
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        div()
            .id("native-file-editor")
            .debug_selector(|| "native-file-editor".into())
            .size_full()
            .key_context("FileEditor")
            .min_h(px(0.))
            .flex()
            .flex_col()
            .text_color(rgb(0xebebeb))
            .bg(rgba(0xffffff04))
            .child(
                div()
                    .h(px(36.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(crate::theme::ui_px(11.))
                    .child(crate::icon(
                        crate::files::icon_for_path(&self.document.path),
                        13.,
                        0x929292,
                    ))
                    .child(div().flex_1().min_w(px(0.)).text_ellipsis().child(format!(
                        "{filename}{}",
                        if self.changed { " · Edited" } else { "" }
                    )))
                    .child(
                        div()
                            .id("save-file-edit")
                            .debug_selector(|| "save-file-edit".into())
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgba(0xffffff12))
                            .cursor_pointer()
                            .child(if self.saving {
                                "Saving…"
                            } else {
                                "Save ⌘S"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
                    .child(
                        div()
                            .id("close-file-edit")
                            .debug_selector(|| "close-file-edit".into())
                            .p_1()
                            .cursor_pointer()
                            .child(crate::icon("close", 12., 0x929292))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.saving {
                                    return;
                                }
                                if this.changed {
                                    this.discard = true;
                                    cx.notify();
                                } else {
                                    cx.emit(Close);
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .pb_1()
                    .text_size(crate::theme::ui_px(9.))
                    .text_color(rgb(0x929292))
                    .child(format!(
                        "{} · 64 KiB editor · ⌘Z undo · ⇧⌘Z redo",
                        self.document.root().display()
                    )),
            )
            .when_some(self.error.as_ref(), |s, error| {
                s.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(crate::theme::ui_px(11.))
                        .text_color(rgb(0xf2aaaa))
                        .child(error.clone()),
                )
            })
            .when(self.discard, |s| {
                s.child(
                    div()
                        .px_3()
                        .py_2()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_size(crate::theme::ui_px(11.))
                        .child("Discard this file draft? The file on disk stays unchanged.")
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    div()
                                        .id("keep-file-draft")
                                        .p_1()
                                        .cursor_pointer()
                                        .child("Keep editing")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.discard = false;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .id("discard-file-draft")
                                        .debug_selector(|| "discard-file-draft".into())
                                        .p_1()
                                        .rounded_md()
                                        .bg(rgba(0xffffff12))
                                        .cursor_pointer()
                                        .child("Discard draft")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if !this.saving {
                                                cx.emit(Close);
                                            }
                                        })),
                                ),
                        ),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .font_family(crate::theme::mono_font())
                    .child(self.input.clone()),
            )
            .child(
                div()
                    .h(px(28.))
                    .px_3()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(crate::theme::ui_px(10.))
                    .text_color(rgb(0x929292))
                    .child(if self.changed {
                        "Draft kept across navigation and restart"
                    } else {
                        "Saved"
                    })
                    .child(
                        div()
                            .id("copy-file-draft")
                            .debug_selector(|| "copy-file-draft".into())
                            .cursor_pointer()
                            .child("Copy draft")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    this.input.read(cx).text(),
                                ))
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    use std::time::{Duration, Instant};
    fn click_selector(cx: &mut VisualTestContext, selector: &'static str) {
        let point = cx.debug_bounds(selector).expect(selector).center();
        cx.simulate_click(point, Modifiers::none());
        cx.run_until_parked();
    }
    fn wait(cx: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if ready(cx) {
                return;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[gpui::test]
    fn files_edit_button_loads_the_selected_text_file_on_demand(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let root = cx.update(|_, cx| view.read(cx).project.clone());
        let path = root.join("newly-opened.md");
        std::fs::write(&path, "# Native editor\n").unwrap();
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.files_visible = true;
                app.sync_file_root(window, cx);
                app.file_browser
                    .as_ref()
                    .unwrap()
                    .update(cx, |browser, cx| {
                        browser.open_citation(
                            crate::citations::FileCitation {
                                path: path.clone(),
                                line: None,
                                end_line: None,
                            },
                            cx,
                        )
                    });
            })
        });
        wait(cx, |cx| {
            crate::ui_tests::draw_frame(&view, cx);
            cx.debug_bounds("edit-preview-file").is_some()
        });
        click_selector(cx, "edit-preview-file");
        wait(cx, |cx| {
            crate::ui_tests::draw_frame(&view, cx);
            cx.debug_bounds("native-file-editor").is_some()
        });
        cx.update(|_, cx| {
            let draft = view
                .read(cx)
                .file_browser
                .as_ref()
                .unwrap()
                .read(cx)
                .workspace_edit(cx)
                .unwrap();
            assert_eq!(draft.document.path, path.canonicalize().unwrap());
            assert_eq!(draft.text, "# Native editor\n");
        });
    }
    #[gpui::test]
    fn editor_native_save_input_undo_redo_and_explicit_discard(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let root = cx.update(|_, cx| view.read(cx).project.clone());
        let path = root.join("editable.rs");
        std::fs::write(&path, "original").unwrap();
        let document = editor::load(&root, &path).unwrap();
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.files_visible = true;
                app.sync_file_root(window, cx);
                app.file_browser
                    .as_ref()
                    .unwrap()
                    .update(cx, |browser, cx| {
                        browser.restore_edit(
                            Draft {
                                document,
                                text: "original".into(),
                            },
                            cx,
                        )
                    });
            })
        });
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("native-file-editor").is_some());
        cx.simulate_keystrokes("cmd-a");
        // One paste is one edit even when other parallel fixtures delay event
        // processing beyond the normal typing-coalescing interval.
        cx.write_to_clipboard(ClipboardItem::new_string("edited 雪".into()));
        cx.simulate_keystrokes("cmd-v");
        cx.run_until_parked();
        let text = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| {
                view.read(cx)
                    .file_browser
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .workspace_edit(cx)
                    .unwrap()
                    .text
            })
        };
        assert_eq!(text(cx), "edited 雪");
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(text(cx), "original");
        cx.simulate_keystrokes("cmd-shift-z");
        assert_eq!(text(cx), "edited 雪");
        cx.simulate_keystrokes("cmd-s");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let editor = view
                .read(cx)
                .file_browser
                .as_ref()
                .unwrap()
                .read(cx)
                .test_editor()
                .unwrap();
            let editor = editor.read(cx);
            assert!(
                editor.saving || std::fs::read_to_string(&path).unwrap() == "edited 雪",
                "Save did not start: error={:?} draft={}",
                editor.error,
                editor.input.read(cx).text()
            );
        });
        wait(cx, |cx| {
            std::fs::read_to_string(&path).unwrap() == "edited 雪"
                && cx.update(|_, cx| {
                    !view
                        .read(cx)
                        .file_browser
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .editor_busy(cx)
                })
        });
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("do not save");
        crate::ui_tests::draw_frame(&view, cx);
        click_selector(cx, "close-file-edit");
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("native-file-editor").is_some());
        assert!(cx.debug_bounds("discard-file-draft").is_some());
        click_selector(cx, "discard-file-draft");
        cx.run_until_parked();
        assert!(cx.update(|_, cx| {
            view.read(cx)
                .file_browser
                .as_ref()
                .unwrap()
                .read(cx)
                .workspace_edit(cx)
                .is_none()
        }));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "edited 雪");
    }
    #[gpui::test]
    fn file_draft_survives_project_navigation_hide_and_restart_with_original_conflict_baseline(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let root = cx.update(|_, cx| view.read(cx).project.clone());
        let path = root.join("original.rs");
        std::fs::write(&path, "original").unwrap();
        let draft = Draft {
            document: editor::load(&root, &path).unwrap(),
            text: "unsaved draft".into(),
        };
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.files_visible = true;
                app.sync_file_root(window, cx);
                app.file_browser
                    .as_ref()
                    .unwrap()
                    .update(cx, |browser, cx| browser.restore_edit(draft, cx));
            })
        });
        crate::ui_tests::draw_frame(&view, cx);
        let other = root.join("other");
        std::fs::create_dir(&other).unwrap();
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.select_project(other, window, cx);
                app.toggle_files(window, cx);
            })
        });
        let saved = cx.update(|_, cx| {
            let app = view.read(cx);
            assert!(
                app.file_browser
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .is_suspended_and_lightweight()
            );
            app.workspace_snapshot(cx)
        });
        let state_path = root.join("restart-state.json");
        let store = crate::workspace_state::StateStore::new(state_path.clone()).unwrap();
        store.save(saved);
        store
            .flush()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let restarted = crate::workspace_state::StateStore::new(state_path).unwrap();
        let restored = restarted.initial().file_edit.clone().unwrap();
        assert_eq!(restored.text, "unsaved draft");
        assert_eq!(restored.document.path, path.canonicalize().unwrap());
        assert!(restored.valid());
        std::fs::write(&path, "external after restart").unwrap();
        assert!(editor::verify(&restored.document).is_err());
        assert!(editor::save(&restored.document, &restored.text).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "external after restart"
        );
    }
}
