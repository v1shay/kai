//! Native display and explicit composer actions for typed agent interactions.
use super::*;
use chat_interactions::{ChatInteraction, InteractionStatus, Speaker};
pub(super) fn status_appearance(status: InteractionStatus) -> (&'static str, u32, &'static str) {
    match status {
        InteractionStatus::Started | InteractionStatus::Working => {
            ("assets/kai/status/tool.svg", ACCENT, "Working")
        }
        InteractionStatus::Completed => ("assets/kai/status/success.svg", 0x8fc89a, "Complete"),
        InteractionStatus::Cancelled => ("assets/kai/status/declined.svg", MUTED, "Stopped"),
        InteractionStatus::Failed => ("assets/kai/status/failure.svg", 0xe58c8c, "Needs attention"),
    }
}

/// Layout height must settle monotonically. A clamped underdamped spring can
/// hit zero, rebound and recreate a collapsed scroller on subsequent frames.
fn disclosure_fraction(progress: f32, open: bool, from: f32) -> f32 {
    let t = progress.clamp(0., 1.);
    let eased = t * t * (3. - 2. * t);
    let target = if open { 1. } else { 0. };
    from + (target - from) * eased
}

impl Orbit {
    pub(super) fn work_reveal(&self,ix:usize)->f32{
        self.work_animations.get(&ix).map(|(at,open,from)|disclosure_fraction(if self.motion{at.elapsed().as_secs_f32()/0.32}else{1.},*open,*from)).unwrap_or(if self.expanded_work.contains(&ix){1.}else{0.})
    }
    pub(super) fn work_summary(&self,ix:usize,cx:&Context<Self>)->AnyElement{
        let label=self.row_timing(ix).and_then(|t|t.elapsed_ms).map(|ms|format!("Worked for {}",chat_timing::duration(ms))).unwrap_or_else(||"Worked".into());
        div().id(("work-summary",ix)).debug_selector(move||format!("work-summary-{ix}")).flex().items_center().gap_2().py_2().text_size(theme::ui_px(11.)).text_color(rgb(MUTED)).cursor_pointer()
            .child(icon(if self.expanded_work.contains(&ix){"chevron-down"}else{"chevron-right"},12.,MUTED)).child(label)
            .on_click(cx.listener(move|this,_,_,cx|{let from=this.work_reveal(ix);let open=if this.expanded_work.remove(&ix){false}else{this.expanded_work.insert(ix);true};this.work_animations.insert(ix,(Instant::now(),open,from));let anchor=this.list.logical_scroll_top();this.list.reset(this.rows.len());this.list.scroll_to(anchor);cx.notify();})).into_any_element()
    }
    fn interaction_reveal(&self, ix: usize) -> f32 {
        let expanded = self.expanded_tools.contains(&ix);
        self.interaction_animations
            .get(&ix)
            .map(|(started, open, from)| {
                disclosure_fraction(
                    if self.motion {
                        started.elapsed().as_secs_f32() / 0.32
                    } else {
                        1.
                    },
                    *open,
                    *from,
                )
            })
            .unwrap_or(if expanded { 1. } else { 0. })
    }
    pub(super) fn interaction_card(
        &self,
        ix: usize,
        card: &ChatInteraction,
        provider: AgentKind,
        cx: &Context<Self>,
    ) -> AnyElement {
        let panel = div()
            .w_full()
            .rounded_lg()
            .border_1()
            .border_color(rgba(0xebebeb16))
            .bg(rgba(self.appearance.tint(GLASS_SURFACE)));
        match card {
            ChatInteraction::NativeRequest(request) => self.native_request_card(request, false, cx),
            ChatInteraction::Speaker { speaker, text } => panel
                .debug_selector(move || format!("interaction-speaker-{ix}-{}", provider.command()))
                .px_3()
                .py_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(crate::theme::ui_px(10.))
                        .text_color(rgb(MUTED))
                        .child(icon("assets/kai/status/listening.svg", 16., MUTED))
                        .child(if *speaker == Speaker::User {
                            "You"
                        } else {
                            agent_name(provider)
                        }),
                )
                .child(
                    div()
                        .mt_1()
                        .text_size(crate::theme::ui_px(14.))
                        .line_height(crate::theme::ui_line_height(23.))
                        .child(text.clone()),
                )
                .into_any_element(),
            ChatInteraction::Delegation {
                title,
                details,
                status,
                agent,
            } => self.status_card(ix, title, details, *status, agent.as_deref(), false, cx),
            ChatInteraction::Status {
                title,
                details,
                status,
            } => self.status_card(ix, title, details, *status, None, false, cx),
            ChatInteraction::Answer { question, answer } => panel
                .p_3()
                .child(
                    div()
                        .text_size(crate::theme::ui_px(11.))
                        .text_color(rgb(MUTED))
                        .child(question.clone()),
                )
                .child(
                    div()
                        .mt_1()
                        .text_size(crate::theme::ui_px(13.))
                        .line_height(crate::theme::ui_line_height(21.))
                        .child(answer.clone()),
                )
                .into_any_element(),
            ChatInteraction::Question(question) => {
                let key = chat_interactions::question_choice_key(question, ix);
                let selected = self.question_choices.get(&key);
                panel
                    .p_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .mb_2()
                            .text_size(crate::theme::ui_px(11.))
                            .text_color(rgb(MUTED))
                            .child(icon("chat", 14., MUTED))
                            .child(if question.header.is_empty() {
                                "Question".to_owned()
                            } else {
                                question.header.clone()
                            })
                            .child(div().flex_1())
                            .child(if question.historical {
                                "Earlier question"
                            } else {
                                "Choose a reply"
                            }),
                    )
                    .child(
                        div()
                            .text_size(crate::theme::ui_px(14.))
                            .line_height(crate::theme::ui_line_height(23.))
                            .child(question.prompt.clone()),
                    )
                    .when_some(question.answer.as_ref(), |s, answer| {
                        s.child(
                            div()
                                .mt_2()
                                .p_2()
                                .rounded_md()
                                .bg(rgba(0x8fc89a12))
                                .text_size(crate::theme::ui_px(12.))
                                .child(answer.clone()),
                        )
                    })
                    .children(
                        question
                            .options
                            .iter()
                            .enumerate()
                            .map(|(option_ix, option)| {
                                let label = option.label.clone();
                                let prompt = question.prompt.clone();
                                let key = key.clone();
                                let chosen = selected.is_some_and(|(choice, _)| *choice == label);
                                let bounce = selected
                                    .filter(|(choice, _)| *choice == label)
                                    .map(|(_, t)| {
                                        if self.motion {
                                            presentation::spring(
                                                (t.elapsed().as_secs_f32() / 0.28).min(1.),
                                            )
                                        } else {
                                            1.
                                        }
                                    })
                                    .unwrap_or(1.);
                                div()
                                    .id(("question-option", ix * 16 + option_ix))
                                    .debug_selector(move || {
                                        format!("question-option-{ix}-{option_ix}")
                                    })
                                    .mt_2()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgba(if chosen {
                                        0x459bf74d
                                    } else {
                                        0xebebeb12
                                    }))
                                    .bg(rgba(if chosen { 0x459bf715 } else { GLASS_SURFACE }))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .active(|s| s.bg(rgba(0x459bf720)))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_size(crate::theme::ui_px(12.))
                                            .child(div().flex_1().child(option.label.clone()))
                                            .when(chosen, |s| {
                                                s.child(
                                                    div()
                                                        .relative()
                                                        .top(px(2. * (1. - bounce)))
                                                        .child(icon("check", 12., ACCENT)),
                                                )
                                            }),
                                    )
                                    .when(!option.description.is_empty(), |s| {
                                        s.child(
                                            div()
                                                .mt_1()
                                                .text_size(crate::theme::ui_px(11.))
                                                .line_height(crate::theme::ui_line_height(18.))
                                                .text_color(rgb(MUTED))
                                                .child(option.description.clone()),
                                        )
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.stop_dictation(cx);
                                        this.question_choices
                                            .insert(key.clone(), (label.clone(), Instant::now()));
                                        this.composer.update(cx, |input, cx| {
                                            input.set_text(&format!("{prompt}\n\n{label}"), cx)
                                        });
                                        this.composer.read(cx).focus_handle.focus(window);
                                        cx.notify();
                                    }))
                            }),
                    )
                    .into_any_element()
            }
        }
    }
    pub(super) fn activity_card(&self,ix:usize,title:&str,details:&str,cx:&Context<Self>)->AnyElement{
        // Historical tool inputs have no authoritative completion status.
        self.status_card(ix,title,details,InteractionStatus::Completed,None,true,cx)
    }
    fn status_card(
        &self,
        ix: usize,
        title: &str,
        details: &str,
        status: InteractionStatus,
        agent: Option<&str>,
        neutral:bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (mut symbol, mut color, mut label) = status_appearance(status);
        let thinking=title=="Thinking";
        if matches!(title,"Terminal output"|"Terminal command"|"Tool output"|"Updated files"|"File changes"|"Changes") || title.starts_with("Used "){symbol=activity_icon(title);}
        let preview=presentation::card_detail_text("Thinking",details.lines().find(|l|!l.trim().is_empty()).unwrap_or("")).0;
        if thinking{symbol="assets/kai/status/thinking.svg";if matches!(status,InteractionStatus::Started|InteractionStatus::Working){label="Thinking"}else if status==InteractionStatus::Completed{label="Thought"}}

        if neutral{symbol=activity_icon(title);color=MUTED;label="Details";}
        let reviewable = !neutral && status == InteractionStatus::Completed && title == "Completed";
        let review_target = reviewable.then(|| self.completion_target(ix)).flatten();
        let undo_target = review_target.clone();
        let expanded = self.expanded_tools.contains(&ix);
        let reveal = self.interaction_reveal(ix);
        div()
            .overflow_hidden()
            .child(
                div()
                    .id(("interaction", ix))
                    .debug_selector(move || format!("interaction-{ix}"))
                    .min_h(px(24.))
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(crate::theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon(symbol, 14., color))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_ellipsis()
                            .child(title.to_owned()),
                    )
                    .when_some(agent, |s, agent| {
                        s.child(
                            div()
                                .text_size(crate::theme::ui_px(10.))
                                .text_color(rgb(MUTED))
                                .child(short(agent, 24)),
                        )
                    })
                    .child(
                        div()
                            .debug_selector(move || format!("completion-label-{ix}"))
                            .text_size(crate::theme::ui_px(10.))
                            .text_color(rgb(color))
                            .child(label),
                    )
                    .when_some(
                        review_target
                            .as_ref()
                            .and_then(|target| self.completion_restore_action(target)),
                        |s, direction| {
                            let target = undo_target
                                .clone()
                                .expect("Restore action has a validated completion target");
                            s.child(
                                div()
                                    .id(("completion-undo", ix))
                                    .debug_selector(move || format!("completion-undo-{ix}"))
                                    .rounded_md()
                                    .px_2()
                                    .py_1()
                                    .text_size(crate::theme::ui_px(10.))
                                    .text_color(rgb(MUTED))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .child(if direction == checkpoints::Direction::Redo {
                                        "Redo"
                                    } else {
                                        "Undo"
                                    })
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.open_completion_target(target.clone(), true, cx);
                                    })),
                            )
                        },
                    )
                    .when(reviewable || !details.is_empty(), |s| {
                        s.child(icon(
                            if expanded {
                                "chevron-down"
                            } else {
                                "chevron-right"
                            },
                            10.,
                            MUTED,
                        ))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if reviewable {
                            if let Some(target) = review_target.clone() {
                                this.open_completion_target(target, false, cx);
                            }
                            return;
                        }
                        // Reverse from precisely the same curve used to paint.
                        let from = this.interaction_reveal(ix);
                        let open = if this.expanded_tools.remove(&ix) {
                            false
                        } else {
                            this.expanded_tools.insert(ix);
                            true
                        };
                        this.interaction_animations
                            .insert(ix, (Instant::now(), open, from));
                        cx.notify();
                    })),
            )
            .when(thinking && reveal<0.01 && !details.is_empty(),|s|s.child(div().pb_1().text_size(crate::theme::ui_px(11.)).line_height(crate::theme::ui_line_height(18.)).text_color(rgb(MUTED)).child(short(&preview,120))))
            .when(reveal > 0. && !details.is_empty(), |s| {
                s.child(
                    div()
                        .debug_selector(move || format!("interaction-reveal-{ix}"))
                        .max_h(px(240. * reveal))
                        .overflow_hidden()
                        .opacity(reveal)
                        .child(
                            div()
                                .id(("interaction-detail", ix))
                                .debug_selector(move || format!("interaction-detail-{ix}"))
                                .max_h(px(240.))
                                .overflow_y_scroll()
                                .px_3()
                                .pb_3()
                                .text_size(crate::theme::ui_px(12.))
                                .line_height(crate::theme::ui_line_height(20.))
                                .text_color(rgb(MUTED))
                                .child({
                                    let copy = details.to_owned();
                                    div().flex().justify_end().mb_1().child(
                                        div()
                                            .id(("interaction-copy", ix))
                                            .debug_selector(move || {
                                                format!("interaction-copy-{ix}")
                                            })
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .text_size(crate::theme::ui_px(10.))
                                            .cursor_pointer()
                                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                            .active(|s| s.bg(rgba(0xffffff12)))
                                            .child("Copy")
                                            .on_click(move |_, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    copy.clone(),
                                                ));
                                            }),
                                    )
                                })
                                .child(
                                    div()
                                        .debug_selector(move || {
                                            format!("interaction-detail-text-{ix}")
                                        })
                                        .child({
                                            // Formatting is opt-in for known prose
                                            // cards. Arbitrary tool titles do not make
                                            // stdout, source or diffs into Markdown.
                                            let (text, highlights) = if status
                                                == InteractionStatus::Failed
                                            {
                                                (runtime::diagnostic_summary(details), Vec::new())
                                            } else {
                                                presentation::card_detail_text(title, details)
                                            };
                                            let owner = cx.weak_entity();
                                            rich_text(
                                                ("interaction-detail-rich", ix),
                                                &text,
                                                &highlights,
                                                move |url, window, cx| {
                                                    let _ = owner.update(cx, |this, cx| {
                                                        this.open_citation(url, window, cx)
                                                    });
                                                },
                                            )
                                        }),
                                ),
                        ),
                )
            })
            .into_any_element()
    }
    pub(super) fn attachment_history_card(
        &self,
        ix: usize,
        name: &str,
        target: &str,
        cx: &Context<Self>,
    ) -> AnyElement {
        let name = name.to_owned();
        let target = target.to_owned();
        div()
            .id(("saved-image", ix))
            .debug_selector(move || format!("saved-image-{ix}"))
            .px_3()
            .py_3()
            .rounded_lg()
            .border_1()
            .border_color(rgba(0xebebeb16))
            .bg(rgba(self.appearance.tint(GLASS_SURFACE)))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .hover(|s| s.bg(rgba(GLASS_HOVER)))
            .child(icon("file-image", 18., MUTED))
            .child(
                div()
                    .flex_1()
                    .text_size(crate::theme::ui_px(12.))
                    .text_ellipsis()
                    .child(name.clone()),
            )
            .child(icon("external", 12., MUTED))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_saved_image(name.clone(), target.clone(), window, cx)
            }))
            .into_any_element()
    }
    fn open_saved_image(
        &mut self,
        name: String,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(citations::Citation::File(citation)) = citations::classify(&target) else {
            self.open_citation(&target, window, cx);
            return;
        };
        if !citation.path.starts_with(&self.attachment_dir) {
            self.open_citation(&target, window, cx);
            return;
        }
        let root = self.attachment_dir.clone();
        self.attachment_preview_cancel
            .store(true, std::sync::atomic::Ordering::Release);
        self.attachment_preview_cancel =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel = self.attachment_preview_cancel.clone();
        let completion_cancel = cancel.clone();
        let job = cx
            .background_executor()
            .spawn(async move { attachments::load_saved(&citation.path, &name, &root, &cancel) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if completion_cancel.load(std::sync::atomic::Ordering::Acquire)
                    || !std::sync::Arc::ptr_eq(&completion_cancel, &this.attachment_preview_cancel)
                {
                    return;
                }
                match result {
                    Ok(attachment) => this.open_attachment_preview(attachment, cx),
                    Err(error) => {
                        this.notice = Some(error.to_string());
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn disclosure_height_never_rebounds_and_reversals_are_continuous() {
        for from in [0., 0.1, 0.5, 0.9, 1.] {
            for open in [false, true] {
                let mut previous = from;
                for frame in 0..=240 {
                    let value = disclosure_fraction(frame as f32 / 240., open, from);
                    assert!((0. ..=1.).contains(&value));
                    if open {
                        assert!(value >= previous);
                    } else {
                        assert!(value <= previous);
                    }
                    // An immediate direction reversal starts at the last painted value.
                    assert_eq!(disclosure_fraction(0., !open, value), value);
                    previous = value;
                }
                assert_eq!(previous, if open { 1. } else { 0. });
            }
        }
    }

    #[gpui::test]
    fn recollapse_clips_without_resizing_scroller_or_rebounding_chat_rows(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        for long in [false, true] {
            view.update(cx, |app, cx| {
                app.active = Some("fixture-session".into());
                app.rows = vec![
                    Row {
                        role: Some("assistant:codex".into()),
                        block: MessageBlock::Paragraph("Earlier output".into()),
                    },
                    Row {
                        role: None,
                        block: MessageBlock::Interaction(ChatInteraction::Status {
                            title: "Terminal command".into(),
                            details: if long {
                                (0..80).map(|i| format!("output line {i}\n")).collect()
                            } else {
                                "cargo check --all-targets".into()
                            },
                            status: InteractionStatus::Completed,
                        }),
                    },
                    Row {
                        role: None,
                        block: MessageBlock::Interaction(ChatInteraction::Status {
                            title: "Thinking".into(),
                            details: String::new(),
                            status: InteractionStatus::Working,
                        }),
                    },
                ];
                app.expanded_tools.clear();
                app.interaction_animations.clear();
                app.list.reset(app.rows.len());
                app.motion = false;
                cx.notify();
            });
            crate::ui_tests::draw_frame(&view, cx);
            cx.run_until_parked();
            let collapsed = cx.debug_bounds("interaction-1").unwrap();
            let collapsed_height = cx.debug_bounds("transcript-content-1").unwrap().size.height;
            let lower = cx.debug_bounds("interaction-2").unwrap();
            crate::ui_tests::click_selector(cx, "interaction-1");
            crate::ui_tests::draw_frame(&view, cx);
            cx.run_until_parked();
            let expanded_scroller = cx.debug_bounds("interaction-detail-1").unwrap();
            assert!(expanded_scroller.size.height <= px(240.));
            assert!(
                cx.debug_bounds("transcript-content-1").unwrap().size.height
                    > collapsed_height + px(20.)
            );
            // Exercise a scrolled detail too: collapse must clip its stable viewport.
            if long {
                cx.simulate_event(ScrollWheelEvent {
                    position: expanded_scroller.center(),
                    delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
                    modifiers: Modifiers::none(),
                    touch_phase: TouchPhase::Moved,
                });
                crate::ui_tests::draw_frame(&view, cx);
                cx.run_until_parked();
            }
            view.update(cx, |app, cx| {
                app.motion = true;
                cx.notify();
            });
            crate::ui_tests::click_selector(cx, "interaction-1");
            view.update(cx, |app, _| {
                assert!(
                    !app.expanded_tools.contains(&1),
                    "Collapse click must close the card; long={long}"
                )
            });
            let mut previous_height = px(241.);
            let mut previous_top = cx.debug_bounds("interaction-1").unwrap().top();
            // Keep one epoch: resetting to Instant::now() at each sample can
            // rewind the animation after a slow frame and falsely report a bounce.
            let epoch = Instant::now();
            for millis in [0, 40, 80, 120, 160, 200, 240, 280, 305, 319, 340] {
                view.update(cx, |app, cx| {
                    if millis < 320 {
                        app.interaction_animations
                            .insert(1, (epoch - Duration::from_millis(millis), false, 1.));
                    } else {
                        app.interaction_animations.clear();
                    }
                    cx.notify();
                });
                crate::ui_tests::draw_frame(&view, cx);
                cx.run_until_parked();
                let height =
                    cx.debug_bounds("transcript-content-1").unwrap().size.height - collapsed_height;
                assert!(
                    height <= previous_height + px(0.5),
                    "Detail reopened while collapsing: {millis}ms"
                );
                previous_height = height;
                if let Some(scroller) = cx.debug_bounds("interaction-detail-1") {
                    assert!(
                        (scroller.size.height - expanded_scroller.size.height).abs() <= px(0.5),
                        "Scroll viewport changed during clipping"
                    );
                }
                let top = cx.debug_bounds("interaction-1").unwrap().top();
                assert!(
                    top >= previous_top - px(0.5),
                    "Chat stack reversed direction: {millis}ms"
                );
                previous_top = top;
            }
            let final_header = cx.debug_bounds("interaction-1").unwrap();
            let final_lower = cx.debug_bounds("interaction-2").unwrap();
            assert!((final_header.top() - collapsed.top()).abs() <= px(0.5));
            assert!((final_lower.top() - lower.top()).abs() <= px(0.5));
            // GPUI 0.2.2 retains old debug selectors after unmount. Assert the
            // fresh row geometry and reveal state, rather than selector absence.
            assert_eq!(
                cx.debug_bounds("transcript-content-1").unwrap().size.height,
                collapsed_height
            );
            view.update(cx, |app, cx| {
                assert!(!app.expanded_tools.contains(&1));
                assert_eq!(app.interaction_reveal(1), 0.);
                assert!(app.interaction_animations.is_empty());
                app.motion = false;
                cx.notify();
            });
        }
    }

    #[test]
    fn inferred_file_links_roundtrip_literal_percent_filenames() {
        let rows = presentation::blocks("`src/100%real.rs:8` and `src/literal%20name.rs#L2`");
        let files = rows
            .iter()
            .filter_map(|row| {
                if let MessageBlock::RichParagraph { highlights, .. } = row {
                    Some(highlights)
                } else {
                    None
                }
            })
            .flatten()
            .filter_map(|(_, style)| {
                if let InlineStyle::Link(url) = style {
                    Some(url)
                } else {
                    None
                }
            })
            .map(|url| {
                let citations::Citation::File(file) = citations::classify(url).unwrap() else {
                    panic!("file")
                };
                (file.path, file.line)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            files,
            [
                (PathBuf::from("src/100%real.rs"), Some(8)),
                (PathBuf::from("src/literal%20name.rs"), Some(2))
            ]
        );
    }

    #[gpui::test]
    fn rendered_review_cards_expand_and_copy_without_launching_an_agent(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let source = "::code-comment{title=\"[P1] Guard empty input\" body=\"Check **length** before indexing.\" file=\"src/main.rs\" start=12 end=14}";
        view.update(cx, |app, _| {
            app.active = Some("fixture-session".into());
            app.rows = presentation::blocks(source)
                .into_iter()
                .map(|block| Row { role: None, block })
                .collect();
            app.list.reset(app.rows.len());
        });
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("interaction-copy-0").is_none());
        let position = cx.debug_bounds("interaction-0").unwrap().center();
        cx.simulate_click(position, Modifiers::none());
        crate::ui_tests::draw_frame(&view, cx);
        let position = cx.debug_bounds("interaction-copy-0").unwrap().center();
        cx.simulate_click(position, Modifiers::none());
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("Check **length** before indexing.".into())
            );
            assert!(view.read(cx).runs.is_empty());
        });
    }
}
