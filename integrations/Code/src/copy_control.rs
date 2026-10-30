//! Shared clipboard feedback, with state scoped to the visible control.
//! No permanent timer, global map, or transcript copies while painting.
use super::*;

#[derive(Default)]
struct Feedback {
    copied: bool,
    generation: u64,
}
#[derive(IntoElement)]
pub(super) struct CopyControl {
    id: SharedString,
    scope: SharedString,
    text: Box<dyn Fn(&App) -> String>,
    label: Option<&'static str>,
    motion: bool,
}
impl CopyControl {
    pub fn text(id: impl Into<SharedString>, text: impl Into<SharedString>) -> Self {
        let text = text.into();
        Self::source(id, move |_| text.to_string())
    }
    pub fn source(id: impl Into<SharedString>, source: impl Fn(&App) -> String + 'static) -> Self {
        Self {
            id: id.into(),
            scope: "".into(),
            text: Box::new(source),
            label: None,
            motion: true,
        }
    }
    pub fn scope(mut self, scope: impl Into<SharedString>) -> Self {
        self.scope = scope.into();
        self
    }
    pub fn label(mut self, label: &'static str) -> Self {
        self.label = Some(label);
        self
    }
    pub fn motion(mut self, motion: bool) -> Self {
        self.motion = motion;
        self
    }
}
impl RenderOnce for CopyControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(
            SharedString::from(format!("{}:{}", self.scope, self.id)),
            cx,
            |_, _| Feedback::default(),
        );
        let copied = state.read(cx).copied;
        let generation = state.read(cx).generation;
        let symbol = icon(
            if copied { "check" } else { "copy" },
            13.,
            if copied { 0x9ccea7 } else { MUTED },
        );
        let symbol = if generation > 0 && self.motion && !native::reduced_motion() {
            symbol
                .with_animation(
                    ("copy-feedback", generation),
                    Animation::new(Duration::from_millis(180)),
                    |s, p| s.opacity(0.3 + 0.7 * p).relative().top(px(3. * (1. - p))),
                )
                .into_any_element()
        } else {
            symbol.into_any_element()
        };
        let selector = self.id.to_string();
        let feedback_selector = format!("{}-{}", self.id, if copied { "check" } else { "copy" });
        div()
            .id(self.id)
            .debug_selector(move || selector.clone())
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(rgba(GLASS_HOVER)))
            .text_size(theme::ui_px(10.))
            .text_color(rgb(MUTED))
            .child(
                div()
                    .debug_selector(move || feedback_selector.clone())
                    .child(symbol),
            )
            .when_some(self.label, |s, label| {
                s.child(if copied { "Copied" } else { label })
            })
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                cx.write_to_clipboard(ClipboardItem::new_string((self.text)(cx)));
                let generation = state.update(cx, |state, cx| {
                    state.copied = true;
                    state.generation = state.generation.wrapping_add(1);
                    cx.notify();
                    state.generation
                });
                let state = state.downgrade();
                cx.spawn(async move |cx| {
                    Timer::after(Duration::from_millis(1500)).await;
                    let _ = state.update(cx, |state, cx| {
                        if state.generation == generation {
                            state.copied = false;
                            state.generation = state.generation.wrapping_add(1);
                            cx.notify();
                        }
                    });
                })
                .detach();
            })
    }
}

pub(super) fn message_text(rows: &[Row]) -> String {
    rows.iter()
        .filter_map(|row| match &row.block {
            MessageBlock::Paragraph(text)
            | MessageBlock::RichParagraph { text, .. }
            | MessageBlock::Voice { text, .. } => Some(text.clone()),
            MessageBlock::Heading(text) => Some(format!("## {text}")),
            MessageBlock::Quote(text) => Some(
                text.lines()
                    .map(|s| format!("> {s}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            MessageBlock::ListItem { marker, text, .. } => Some(format!("{marker} {text}")),
            MessageBlock::Code { language, text, .. } => {
                Some(format!("```{language}\n{text}\n```"))
            }
            MessageBlock::TableRow { cells, .. } => Some(format!("| {} |", cells.join(" | "))),
            MessageBlock::Attachment { name, target } => Some(format!("[{name}]({target})")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn message_copy_keeps_blocks_and_does_not_copy_tool_payloads() {
        let rows = vec![
            Row {
                role: Some("assistant:codex".into()),
                block: MessageBlock::Paragraph("Answer".into()),
            },
            Row {
                role: Some("continuation:codex".into()),
                block: MessageBlock::Code {
                    language: "rust".into(),
                    text: "let x = 1;".into(),
                    highlights: vec![],
                },
            },
            Row {
                role: None,
                block: MessageBlock::Activity {
                    label: "Tool".into(),
                    details: "private tool payload".into(),
                },
            },
        ];
        assert_eq!(message_text(&rows), "Answer\n\n```rust\nlet x = 1;\n```");
    }
    #[test]
    fn one_footer_per_message_not_per_markdown_block() {
        let rows = Orbit::history_rows(vec![
            engine::ParsedMessage {
                role: "user".into(),
                blocks: vec![
                    MessageBlock::Paragraph("Question".into()),
                    MessageBlock::Paragraph("More context".into()),
                ],
            },
            engine::ParsedMessage {
                role: "assistant:codex".into(),
                blocks: vec![
                    MessageBlock::Paragraph("Answer".into()),
                    MessageBlock::Code {
                        language: "".into(),
                        text: "code".into(),
                        highlights: vec![],
                    },
                    MessageBlock::Paragraph("Explanation".into()),
                ],
            },
            engine::ParsedMessage {
                role: "user".into(),
                blocks: vec![MessageBlock::Paragraph("Next".into())],
            },
        ]);
        let index = chat_flow::Index::new(&rows);
        assert_eq!(
            index
                .message_ends
                .iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>(),
            vec![0..2, 2..5, 5..6]
        );
    }
    #[gpui::test]
    fn message_footer_copies_entire_reply_and_code_control_has_same_feedback(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            app.loading = false;
            app.rows = Orbit::history_rows(vec![
                engine::ParsedMessage {
                    role: "user".into(),
                    blocks: vec![MessageBlock::Paragraph("Question".into())],
                },
                engine::ParsedMessage {
                    role: "assistant:codex".into(),
                    blocks: vec![
                        MessageBlock::Paragraph("Answer".into()),
                        MessageBlock::Code {
                            language: "rust".into(),
                            text: "let x = 1;".into(),
                            highlights: vec![],
                        },
                    ],
                },
            ]);
            app.sync_flow_layout();
            app.list.reset(app.rows.len());
            cx.notify();
        });
        ui_tests::draw_frame(&view, cx);
        cx.run_until_parked();
        ui_tests::draw_frame(&view, cx);
        ui_tests::click_selector(cx, "message-copy-2");
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|v| v.text()),
                Some("Answer\n\n```rust\nlet x = 1;\n```".into())
            )
        });
        ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("message-copy-2-check").is_some());
        ui_tests::click_selector(cx, "code-copy-2");
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|v| v.text()),
                Some("let x = 1;".into())
            )
        });
        ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("code-copy-2-check").is_some());
    }
    #[gpui::test]
    fn priority_toggles_and_prompt_recency_moves_project_without_viewing_it(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.sidebar_split = false;
            let mut second = app.sessions[0].clone();
            second.id = "second".into();
            second.project_path = app.project.join("second");
            let second_root = second.project_path.clone();
            app.sessions.push(second);
            app.projects.push(Project {
                name: "Second".into(),
                path: second_root.clone(),
            });
            app.activity.prompted("fixture-session", 1000);
            app.activity.prompted("second", 2000);
            app.rebuild_workspace_sessions();
            assert_eq!(app.projects[0].path, second_root);
            app.all_projects = true;
            app.rebuild_workspace_sessions();
            assert_eq!(app.workspace_sessions[0].id, "second");
            app.sessions
                .iter_mut()
                .find(|s| s.id == "fixture-session")
                .unwrap()
                .updated_at = 999999;
            app.activity.open("fixture-session");
            app.rebuild_workspace_sessions();
            assert_eq!(app.workspace_sessions[0].id, "second");
            app.activity.prompted("fixture-session", 3000);
            app.rebuild_workspace_sessions();
            assert_eq!(app.workspace_sessions[0].id, "fixture-session");
            assert_eq!(app.projects[0].path, app.project);
            let saved = serde_json::to_string(&app.activity.saved).unwrap();
            let restored: sidebar_activity::Saved = serde_json::from_str(&saved).unwrap();
            let mut state = sidebar_activity::Activity::default();
            state.saved = restored;
            assert_eq!(state.prompt_at(&app.sessions[0]), 3000);
            app.all_projects = false;
            cx.notify();
        });
        ui_tests::draw_frame(&view, cx);
        ui_tests::click_selector(cx, "inbox");
        view.read_with(cx, |app, _| assert!(app.all_projects));
        ui_tests::draw_frame(&view, cx);
        ui_tests::click_selector(cx, "inbox");
        view.read_with(cx, |app, _| assert!(!app.all_projects));
    }
}
