//! Immutable per-response change cards; only visible completions request counts.
use super::*;
use std::sync::{Arc, atomic::AtomicBool};
#[derive(Default)]
pub(super) struct State {
    entries: HashMap<String, (Instant, Option<Arc<github::Summary>>)>,
    pending: HashSet<String>,
}
impl State {
    pub(super) fn invalidate(&mut self, id: &str) {
        self.entries.remove(id);
    }
}
impl Orbit {
    pub(super) fn prefetch_change_summary(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(id) = self.checkpoint_for_completion(ix) else {
            return;
        };
        if self.change_summaries.pending.len() >= 2
            || self.change_summaries.pending.contains(&id)
            || self
                .change_summaries
                .entries
                .get(&id)
                .is_some_and(|(at, summary)| {
                    summary.is_some() || at.elapsed() < Duration::from_secs(45)
                })
        {
            return;
        }
        let Some(session) = self.active.clone() else {
            return;
        };
        let Some(root) = self.checkpoint_scope(&session) else {
            return;
        };
        let Ok(repo) = self.checkpoint_repository.clone() else {
            return;
        };
        if self.change_summaries.entries.len() >= 128 {
            if let Some(old) = self
                .change_summaries
                .entries
                .iter()
                .min_by_key(|(_, v)| v.0)
                .map(|(k, _)| k.clone())
            {
                self.change_summaries.entries.remove(&old);
            }
        }
        self.change_summaries.pending.insert(id.clone());
        let lookup = id.clone();
        let expected = session.clone();
        let job = cx.background_executor().spawn(async move {
            let checkpoint = repo.get(&lookup)?;
            anyhow::ensure!(
                checkpoint.session == expected && std::fs::canonicalize(root)? == checkpoint.root,
                "Checkpoint owner changed"
            );
            repo.change_summary(&lookup, &AtomicBool::new(false))
                .map(|mut summary| {
                    summary.files.truncate(24);
                    summary
                })
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |app, cx| {
                app.change_summaries.pending.remove(&id);
                app.change_summaries
                    .entries
                    .insert(id, (Instant::now(), result.ok().map(Arc::new)));
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn completion_summary(&self, ix: usize) -> Option<Arc<github::Summary>> {
        let id = self.checkpoint_for_completion(ix)?;
        self.change_summaries.entries.get(&id)?.1.clone()
    }
    pub(super) fn compact_completion_diff(
        &self,
        ix: usize,
        target: checkpoint_ui::CompletionTarget,
        summary: Arc<github::Summary>,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .id(("completion-diff-counts", ix))
            .debug_selector(move || format!("completion-diff-counts-{ix}"))
            .h(px(26.))
            .pr_2()
            .flex_none()
            .flex()
            .items_center()
            .rounded_md()
            .font_family(theme::ui_font())
            .cursor_pointer()
            .hover(|s| s.bg(rgba(GLASS_HOVER)))
            .child(github_ui::counts(&github::Counts {
                added: summary.added,
                removed: summary.removed,
                binary: summary.binaries > 0 && summary.added == 0 && summary.removed == 0,
            }))
            .on_click(cx.listener(move |app, _, _, cx| {
                cx.stop_propagation();
                app.open_completion_target(target.clone(), false, cx);
            }))
            .into_any_element()
    }
    pub(super) fn completion_changes_card(
        &self,
        ix: usize,
        target: checkpoint_ui::CompletionTarget,
        summary: Arc<github::Summary>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let undo = self.completion_restore_action(&target);
        let open = target.clone();
        let undo_target = target.clone();
        let expanded = self.expanded_tools.contains(&ix);
        let reveal = self.interaction_reveal(ix);
        let shown = summary.files.len().min(24);
        let extra = shown.saturating_sub(3);
        let row = |file: &github::FileChange| {
            let path = file.path.clone();
            let target = target.clone();
            div()
                .id(SharedString::from(format!(
                    "completion-file-{ix}-{}",
                    file.path.to_string_lossy()
                )))
                .h(px(30.))
                .px_4()
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                .child(icon("file-code", 13., MUTED))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(theme::ui_px(11.))
                        .text_ellipsis()
                        .child(file.path.to_string_lossy().into_owned()),
                )
                .child(github_ui::counts(&file.counts))
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.open_completion_file(target.clone(), path.clone(), cx)
                }))
        };
        div()
            .id(("completion-changes", ix))
            .debug_selector(move || format!("completion-changes-{ix}"))
            .w_full()
            .mt_4()
            .mb_2()
            .pb_2()
            .font_family(theme::ui_font())
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(rgba(0xffffff14))
            .bg(linear_gradient(
                135.,
                linear_color_stop(rgba(0xffffff07), 0.),
                linear_color_stop(rgba(0xffffff02), 1.),
            ))
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .size(px(28.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("file-diff", 21., TEXT)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .id(("completion-label", ix))
                                    .debug_selector(move || format!("completion-label-{ix}"))
                                    .cursor_pointer()
                                    .on_click(cx.listener({
                                        let target = open.clone();
                                        move |app, _, _, cx| {
                                            cx.stop_propagation();
                                            app.open_completion_target(target.clone(), false, cx);
                                        }
                                    }))
                                    .text_size(theme::ui_px(12.))
                                    .child(format!(
                                        "Edited {} {}",
                                        summary.total,
                                        if summary.total == 1 { "file" } else { "files" }
                                    )),
                            )
                            .child(github_ui::counts(&github::Counts {
                                added: summary.added,
                                removed: summary.removed,
                                binary: summary.binaries > 0
                                    && summary.added == 0
                                    && summary.removed == 0,
                            })),
                    )
                    .when_some(undo, |s, direction| {
                        s.child(
                            div()
                                .id(("completion-undo", ix))
                                .debug_selector(move || format!("completion-undo-{ix}"))
                                .h(px(28.))
                                .px_2()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap_2()
                                .rounded_md()
                                .text_size(theme::ui_px(11.))
                                .text_color(rgb(TEXT))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                .child(icon(
                                    if direction == checkpoints::Direction::Undo {
                                        "undo"
                                    } else {
                                        "redo"
                                    },
                                    14.,
                                    TEXT,
                                ))
                                .child(if direction == checkpoints::Direction::Undo {
                                    "Undo"
                                } else {
                                    "Redo"
                                })
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.open_completion_target(undo_target.clone(), true, cx)
                                })),
                        )
                    })
                    .child(
                        div()
                            .id(("completion-review", ix))
                            .debug_selector(move || format!("completion-review-{ix}"))
                            .h(px(28.))
                            .px_3()
                            .flex_none()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .border_1()
                            .border_color(rgba(0xffffff18))
                            .text_size(theme::ui_px(11.))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child("View changes")
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.open_completion_target(open.clone(), false, cx)
                            })),
                    ),
            )
            .children(summary.files.iter().take(3).map(row))
            .when(extra > 0, |s| {
                s.child(
                    div()
                        .h(px(extra as f32 * 30. * reveal))
                        .overflow_hidden()
                        .child(
                            div()
                                .h(px(extra as f32 * 30.))
                                .children(summary.files.iter().skip(3).take(extra).map(row)),
                        ),
                )
            })
            .when(summary.total > 3, |s| {
                s.child(
                    div()
                        .id(("completion-more-files", ix))
                        .debug_selector(move || format!("completion-more-files-{ix}"))
                        .px_4()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .text_size(theme::ui_px(11.))
                        .hover(|s| s.bg(rgba(GLASS_HOVER)))
                        .child(if expanded {
                            "Show fewer files".into()
                        } else {
                            format!("Show {} more files", summary.total.saturating_sub(3))
                        })
                        .child(icon(
                            if expanded {
                                "chevron-down"
                            } else {
                                "chevron-right"
                            },
                            12.,
                            MUTED,
                        ))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            if summary.total > 24 && !expanded {
                                app.open_completion_target(target.clone(), false, cx);
                                return;
                            }
                            let from = app.interaction_reveal(ix);
                            let open = if app.expanded_tools.remove(&ix) {
                                false
                            } else {
                                app.expanded_tools.insert(ix);
                                true
                            };
                            app.interaction_animations
                                .insert(ix, (Instant::now(), open, from));
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
    pub(super) fn open_completion_file(
        &mut self,
        target: checkpoint_ui::CompletionTarget,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let root = target.root.clone();
        self.open_completion_target(target, false, cx);
        if let Some(review) = self.diff_view.as_ref() {
            review.update(cx, |r, cx| r.restore_selection(root.join(path), cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn exact_changes_card_survives_folded_work_under_the_visible_final_answer(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let id = uuid::Uuid::new_v4().to_string();
        let checkpoint = id.clone();
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            app.loading = false;
            let status = |title: &str| Row {
                role: Some("continuation:codex".into()),
                block: MessageBlock::Interaction(chat_interactions::ChatInteraction::Status {
                    title: title.into(),
                    details: String::new(),
                    status: chat_interactions::InteractionStatus::Completed,
                }),
            };
            app.rows = vec![
                Row {
                    role: Some("user".into()),
                    block: MessageBlock::Paragraph("Update the UI".into()),
                },
                Row {
                    role: Some("user".into()),
                    block: MessageBlock::Activity {
                        label: "Prompt changes".into(),
                        details: id.clone(),
                    },
                },
                Row {
                    role: Some("assistant:codex".into()),
                    block: MessageBlock::Paragraph("Inspecting the code".into()),
                },
                status("Ran commands"),
                status("Final answer"),
                Row {
                    role: Some("continuation:codex".into()),
                    block: MessageBlock::Paragraph("The update is complete.".into()),
                },
                status("Completed"),
            ];
            app.change_summaries.entries.insert(
                id.clone(),
                (
                    Instant::now(),
                    Some(Arc::new(github::Summary::from_files(
                        (0..6)
                            .map(|i| github::FileChange {
                                path: format!("src/file-{i}.rs").into(),
                                old_path: None,
                                status: "Modified".into(),
                                counts: github::Counts {
                                    added: 10,
                                    removed: 2,
                                    binary: false,
                                },
                            })
                            .collect(),
                    ))),
                ),
            );
            app.checkpoint_actions
                .insert(id.clone(), Some(checkpoints::Direction::Undo));
            app.list.reset(app.rows.len());
            app.sync_flow_layout();
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        view.update(cx, |app, _| {
            assert!(app.flow_index.hidden[6]);
            assert!(app.flow_index.completion_ends[5].is_some());
            assert_eq!(
                app.checkpoint_for_completion(5).as_deref(),
                Some(checkpoint.as_str())
            );
        });
        let card = cx
            .debug_bounds("completion-changes-5")
            .expect("Folded work hid the response's actual changes");
        let copy = cx.debug_bounds("message-copy-5").unwrap();
        assert!(card.bottom() <= copy.top() + px(2.));
        assert!(cx.debug_bounds("completion-undo-5").is_some());
        let control = cx.debug_bounds("completion-more-files-5").unwrap();
        let viewport = view.update(cx, |app, _| app.list.viewport_bounds());
        let pane = cx.debug_bounds("chat-pane").unwrap();
        assert!(viewport.intersect(&pane).contains(&control.center()),
            "More-files button is clipped: control={control:?}, viewport={viewport:?}, pane={pane:?}");
        crate::ui_tests::click_selector(cx, "completion-more-files-5");
        let selected = view.update(cx, |app, _| app.expanded_tools.contains(&5));
        crate::ui_tests::draw_frame(&view, cx);
        let after = cx.debug_bounds("completion-changes-5").unwrap();
        assert!(after.size.height > card.size.height,
            "More-files button failed: expanded={selected}, control={control:?}, viewport={viewport:?}, pane={pane:?}, before={card:?}, after={after:?}");
        assert!(cx.debug_bounds("response-sync-5").is_some());
        let expanded_row = cx.debug_bounds("transcript-row-5").unwrap();
        view.update(cx, |app, cx| {
            app.theme.compact_diff = true;
            let saved = serde_json::to_string(&app.theme).unwrap();
            assert!(
                serde_json::from_str::<theme::Theme>(&saved)
                    .unwrap()
                    .compact_diff
            );
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        let compact = cx
            .debug_bounds("completion-diff-counts-5")
            .expect("Compact mode lost its change counts");
        let copy = cx.debug_bounds("message-copy-5").unwrap();
        let push = cx.debug_bounds("response-sync-5").unwrap();
        assert!((compact.center().y - copy.center().y).abs() < px(1.));
        assert!((compact.center().y - push.center().y).abs() < px(1.));
        assert!(compact.right() <= copy.left());
        assert!(copy.right() <= push.left());
        assert!(
            cx.debug_bounds("transcript-row-5").unwrap().size.height < expanded_row.size.height,
            "Compact mode still reserves the expanded card's height"
        );
        crate::ui_tests::click_selector(cx, "completion-diff-counts-5");
        view.update(cx, |app, _| {
            assert!(
                app.diff_view.is_some(),
                "The compact counts must open the native diff viewer"
            );
            assert_eq!(
                app.checkpoint_for_completion(5).as_deref(),
                Some(checkpoint.as_str())
            );
        });
    }
}
