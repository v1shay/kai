//! A compact continuous transcript scrubber. Animation and pointer tracking live
//! in their own entity; dragging never schedules history reads.
use super::*;
use std::{cell::Cell, rc::Rc};

const TICKS: usize = 49;
const HEIGHT: f32 = 240.;

#[derive(Clone, Copy, Default)]
struct Spring {
    value: f32,
    velocity: f32,
    target: f32,
}
impl Spring {
    fn moving(&self) -> bool {
        (self.value - self.target).abs() > 0.0005 || self.velocity.abs() > 0.002
    }
    fn step(&mut self, dt: f32) {
        let mut remaining = dt.clamp(0., 0.05);
        while remaining > 0. {
            let dt = remaining.min(1. / 120.);
            self.velocity += (420. * (self.target - self.value) - 28. * self.velocity) * dt;
            self.value += self.velocity * dt;
            remaining -= dt;
        }
        if !self.moving() {
            self.value = self.target;
            self.velocity = 0.;
        }
    }
    fn snap(&mut self, target: f32) {
        self.value = target;
        self.target = target;
        self.velocity = 0.;
    }
}

pub(super) struct Seek(pub f64);
impl EventEmitter<Seek> for ScrollRail {}

pub(super) struct ScrollRail {
    position: Spring,
    focus: Spring,
    hover: Spring,
    dragging: bool,
    hovered: bool,
    motion: bool,
    last_frame: Instant,
    frame_pending: bool,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    // Source bounds for the currently represented, lazily loaded window.
    span: (f64, f64),
}
impl ScrollRail {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self {
            position: Spring {
                value: 1.,
                target: 1.,
                ..Default::default()
            },
            focus: Spring {
                value: 1.,
                target: 1.,
                ..Default::default()
            },
            hover: Spring::default(),
            dragging: false,
            hovered: false,
            motion: true,
            last_frame: Instant::now(),
            frame_pending: false,
            bounds: Rc::new(Cell::new(None)),
            span: (0., 1.),
        }
    }
    pub fn set_motion(&mut self, motion: bool, cx: &mut Context<Self>) {
        if self.motion != motion {
            self.motion = motion;
            if !motion {
                self.position.snap(self.position.target);
                self.focus.snap(self.focus.target);
                self.hover.snap(self.hover.target);
            }
            cx.notify();
        }
    }
    pub fn set_span(&mut self, start: f64, end: f64) {
        if start.is_finite() && end.is_finite() && start <= end {
            self.span = (start.clamp(0., 1.), end.clamp(0., 1.));
        }
    }
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.dragging = false;
        self.span = (0., 1.);
        self.set_position(1., cx);
    }
    pub fn scrolled(&mut self, row: usize, rows: usize, cx: &mut Context<Self>) {
        if self.dragging {
            return;
        }
        let local = if rows <= 1 {
            1.
        } else {
            row.min(rows - 1) as f64 / (rows - 1) as f64
        };
        self.set_position(self.span.0 + (self.span.1 - self.span.0) * local, cx);
    }
    pub fn set_position(&mut self, position: f64, cx: &mut Context<Self>) {
        let position = position.clamp(0., 1.) as f32;
        if (self.position.target - position).abs() < 0.0001 {
            return;
        }
        self.position.target = position;
        if !self.hovered && !self.dragging {
            self.focus.target = position;
        }
        self.changed(cx);
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        if !self.motion {
            self.position.snap(self.position.target);
            self.focus.snap(self.focus.target);
            self.hover.snap(self.hover.target);
        }
        if self.last_frame.elapsed() > Duration::from_millis(100) {
            self.last_frame = Instant::now() - Duration::from_millis(8);
        }
        cx.notify();
    }
    fn fraction(&self, position: Point<Pixels>) -> f32 {
        self.bounds.get().map_or(self.position.target, |bounds| {
            pointer_fraction(
                f32::from(position.y - bounds.origin.y),
                f32::from(bounds.size.height),
            )
        })
    }
    fn pointer(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let target = self.fraction(position);
        self.focus.target = target;
        if self.dragging {
            self.position.target = target;
        }
        self.changed(cx);
    }
    fn finish(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.dragging {
            return;
        }
        let target = self.fraction(position);
        self.dragging = false;
        self.position.target = target;
        self.focus.target = target;
        self.changed(cx);
        cx.emit(Seek(f64::from(target)));
    }
}

fn pointer_fraction(y: f32, height: f32) -> f32 {
    // First and last tick centers are real endpoints, not inaccessible margins.
    let inset = (height / TICKS as f32 * 0.5).min(height * 0.5);
    if height <= 2. * inset {
        return 0.;
    }
    ((y - inset) / (height - 2. * inset)).clamp(0., 1.)
}

impl Render for ScrollRail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        for spring in [&mut self.position, &mut self.focus, &mut self.hover] {
            spring.step(dt);
        }
        if self.motion
            && [self.position, self.focus, self.hover]
                .iter()
                .any(Spring::moving)
            && !self.frame_pending
        {
            self.frame_pending = true;
            let weak = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.frame_pending = false;
                    cx.notify();
                });
            });
        }
        let bounds = self.bounds.clone();
        let drag_owner = cx.weak_entity();
        let dragging = self.dragging;
        let position = self.position.value.clamp(0., 1.);
        let focus = self.focus.value.clamp(0., 1.);
        let hover = self.hover.value.clamp(0., 1.);
        div()
            .id("chat-navigation")
            .debug_selector(|| "chat-navigation".into())
            .w(px(32.))
            .h(px(HEIGHT))
            .max_h(relative(0.65))
            .flex_none()
            .cursor_pointer()
            .child(
                canvas(
                    move |area, _, _| {
                        bounds.set(Some(area));
                    },
                    move |area, _, window, _| {
                        if dragging {
                            let owner = drag_owner.clone();
                            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                                if phase == DispatchPhase::Capture
                                    && !area.contains(&event.position)
                                {
                                    let _ = owner.update(cx, |this, cx| {
                                        if this.dragging {
                                            this.pointer(event.position, cx);
                                        }
                                    });
                                }
                            });
                        }
                        for ix in 0..TICKS {
                            let fraction = ix as f32 / (TICKS - 1) as f32;
                            let selected =
                                (-((fraction - position) * (TICKS - 1) as f32).powi(2) / 5.).exp();
                            let lens = (-((fraction - focus) * (TICKS - 1) as f32).powi(2) / 10.)
                                .exp()
                                * hover;
                            let width = 5. + 16. * selected + 7. * lens;
                            let alpha = (68. + 150. * selected + 30. * lens).min(255.) as u32;
                            let y = area.origin.y
                                + area.size.height * ((ix as f32 + 0.5) / TICKS as f32);
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(area.origin.x + px(4.), y - px(0.75)),
                                    size(px(width), px(1.5)),
                                ),
                                rgba(0xe8dfc900 | alpha),
                            ));
                        }
                    },
                )
                .size_full(),
            )
            .on_hover(cx.listener(|this, hover: &bool, _, cx| {
                this.hovered = *hover;
                this.hover.target = if *hover { 1. } else { 0. };
                if !hover && !this.dragging {
                    this.focus.target = this.position.target;
                }
                this.changed(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.dragging = true;
                    this.hover.target = 1.;
                    this.pointer(event.position, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.dragging || this.hovered {
                    this.pointer(event.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| this.finish(event.position, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| this.finish(event.position, cx)),
            )
    }
}

impl Orbit {
    pub(super) fn seek_chat(&mut self, fraction: f64, cx: &mut Context<Self>) {
        let Some(session) = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|session| &session.id == id))
            .cloned()
        else {
            return;
        };
        if fraction >= 1. && self.runs.contains_key(&session.id) {
            self.restore_live_tail(&session.id, cx);
            return;
        }
        self.transcript_reveal = None;
        // Pause transcript insertion, not the agent transport. Buffered live
        // output is presented again when the user seeks back to the tail.
        self.navigation_previous
            .get_or_insert(self.navigation_history);
        self.navigation_history = true;
        self.restore_scroll = None;
        self.generation += 1;
        self.loading_earlier = true;
        self.history_anchor = None;
        self.engine.seek_history(session, self.generation, fraction);
        self.queue_workspace_save(cx);
        cx.notify();
    }

    pub(super) fn restore_live_tail(&mut self, id: &str, cx: &mut Context<Self>) {
        self.live_history=None;
        let Some(run) = self.runs.get_mut(id) else {
            return;
        };
        // Rebuild only the bounded current turn. Reading the native tail here
        // could include this same unfinished turn and duplicate its output.
        let mut start = run
            .submitted_prompt
            .len()
            .saturating_sub(runtime::history::PAGE_BYTES);
        while !run.submitted_prompt.is_char_boundary(start) {
            start += 1;
        }
        self.rows = vec![Row {
            role: Some("user".into()),
            block: MessageBlock::Paragraph(run.submitted_prompt[start..].to_owned()),
        }];
        if let Some(checkpoint) = self.checkpoint_runs.get(id) {
            self.rows.push(Row {
                role: None,
                block: MessageBlock::Activity {
                    label: "Prompt changes".into(),
                    details: checkpoint.clone(),
                },
            });
        }
        for attachment in &run.submitted_attachments {
            self.rows.push(Row {
                role: None,
                block: MessageBlock::Attachment {
                    name: attachment.name.clone(),
                    target: attachment.path.to_string_lossy().into_owned(),
                },
            });
        }
        run.stream_start = None;
        self.navigation_history = false;
        self.navigation_previous = None;
        self.transcript_reveal = None;
        self.pending_response_start = None;
        self.following_bottom = true;
        self.loading = false;
        self.loading_earlier = false;
        self.has_older = true;
        self.has_newer = false;
        self.history_offset = 0;
        self.history_anchor = None;
        self.history_pages.clear();
        self.deferred_rows.clear();
        self.history_pending.clear();
        self.history_failed.clear();
        self.expanded_tools.clear();
        self.interaction_animations.clear();
        self.question_choices.clear();
        self.history_visible = 0..self.rows.len();
        self.generation += 1;
        self.engine.load_model(id.to_owned(), self.generation);
        self.list.reset(self.rows.len());
        self.update_stream(id, cx);
        self.list.scroll_to(ListOffset {
            item_ix: self.rows.len(),
            offset_in_item: px(0.),
        });
        self.navigation.update(cx, |rail, cx| {
            rail.set_span(1., 1.);
            rail.set_position(1., cx);
        });
        self.queue_workspace_save(cx);
        cx.notify();
    }

    pub(super) fn chat_navigation(&self) -> impl IntoElement {
        div()
            .absolute()
            .left(px(0.))
            .top_0()
            .bottom_0()
            .w(px(32.))
            .flex()
            .flex_col()
            .justify_center()
            .child(self.navigation.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn physics_settles_and_preserves_velocity_when_retargeted() {
        let mut spring = Spring {
            target: 1.,
            ..Default::default()
        };
        for _ in 0..12 {
            spring.step(1. / 120.);
        }
        let velocity = spring.velocity;
        spring.target = 0.;
        assert_eq!(spring.velocity, velocity);
        for _ in 0..240 {
            spring.step(1. / 120.);
        }
        assert!(!spring.moving());
        assert_eq!(spring.value, 0.);
    }
    #[test]
    fn spring_has_small_physical_overshoot_and_then_stops() {
        let mut spring = Spring {
            target: 1.,
            ..Default::default()
        };
        let mut peak = 0_f32;
        for _ in 0..240 {
            spring.step(1. / 120.);
            peak = peak.max(spring.value);
        }
        assert!(peak > 1. && peak < 1.1);
        assert!(!spring.moving());
        assert_eq!(spring.value, 1.);
    }
    #[test]
    fn endpoints_and_fraction_are_continuous_not_prompt_steps() {
        assert_eq!(pointer_fraction(0., HEIGHT), 0.);
        assert_eq!(pointer_fraction(HEIGHT, HEIGHT), 1.);
        assert_eq!(pointer_fraction(HEIGHT * 0.5, HEIGHT), 0.5);
        assert!(pointer_fraction(HEIGHT * 0.501, HEIGHT) > 0.5);
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    use crate::ui_tests::{draw_frame, fixture, idle_run};
    use gpui::{TestAppContext, VisualTestContext};
    use std::prelude::v1::test;

    fn open(view: &Entity<Orbit>, cx: &mut VisualTestContext) {
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            app.receive(
                Event::Loaded(
                    "fixture-session".into(),
                    app.generation,
                    (0..1000)
                        .map(|ix| engine::ParsedMessage {
                            role: "assistant".into(),
                            blocks: vec![MessageBlock::Paragraph(format!("Message {ix}"))],
                        })
                        .collect(),
                    true,
                    false,
                ),
                cx,
            );
            app.composer
                .update(cx, |input, cx| input.set_text("Keep my draft", cx));
        });
        draw_frame(view, cx);
    }

    #[gpui::test]
    fn compact_rail_is_centered_on_left_and_drag_reads_only_on_release(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx);
        open(&view, cx);
        let rail = cx.debug_bounds("chat-navigation").unwrap();
        let generation = cx.update(|_, cx| {
            let app = view.read(cx);
            let viewport = app.list.viewport_bounds();
            assert_eq!(rail.size.height, px(HEIGHT));
            assert!(rail.right() <= viewport.left());
            assert!((f32::from(rail.center().y - viewport.center().y)).abs() < 1.);
            app.generation
        });
        cx.simulate_event(MouseDownEvent {
            position: rail.center(),
            button: MouseButton::Left,
            ..Default::default()
        });
        draw_frame(&view, cx);
        for step in 0..20 {
            cx.simulate_mouse_move(
                point(
                    rail.center().x,
                    rail.top() + rail.size.height * (step as f32 / 20.),
                ),
                Some(MouseButton::Left),
                Modifiers::none(),
            );
        }
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert_eq!(
                app.generation, generation,
                "Dragging must not enqueue history reads"
            );
            assert_eq!(app.rows.len(), 1000);
            assert_eq!(app.composer.read(cx).text(), "Keep my draft");
        });
        cx.simulate_event(MouseUpEvent {
            position: point(rail.center().x, rail.top()),
            button: MouseButton::Left,
            ..Default::default()
        });
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert_eq!(
                app.generation,
                generation + 1,
                "Release performs one global seek"
            );
            assert!(app.loading_earlier);
            assert!(
                !app.loading,
                "The old transcript stays visible while the target loads"
            );
        });
    }

    #[gpui::test]
    fn reading_history_does_not_interrupt_live_output_or_duplicate_it_on_return(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = fixture(cx);
        open(&view, cx);
        view.update(cx, |app, cx| {
            let mut run = idle_run("Current live response".into());
            run.submitted_prompt = "Current prompt".into();
            run.parsed_revision = run.revision;
            run.live_blocks = vec![MessageBlock::Paragraph("Current live response".into())];
            app.runs.insert("fixture-session".into(), run);
            app.seek_chat(0., cx);
            app.receive(Event::HistorySeeked("fixture-session".into(), app.generation, vec![engine::ParsedMessage {
                role: "user".into(), blocks: vec![MessageBlock::Paragraph("Oldest prompt".into())],
            }], false, true, 0., (0., 0.01)), cx);
            assert!(app.navigation_history);
            app.apply_live_rows("fixture-session");
            assert_eq!(app.rows.len(), 1, "Streaming must not overwrite the history being read");
            assert_eq!(app.runs.len(), 1);
            app.seek_chat(1., cx);
            assert!(!app.navigation_history);
            assert!(app.following_bottom);
            assert!(!app.loading_earlier);
            assert_eq!(app.rows.len(), 2);
            assert!(matches!(&app.rows[0].block, MessageBlock::Paragraph(s) if s == "Current prompt"));
            assert!(matches!(&app.rows[1].block, MessageBlock::Paragraph(s) if s == "Current live response"));
            assert_eq!(app.runs.len(), 1);
            assert_eq!(app.composer.read(cx).text(), "Keep my draft");
            app.seek_chat(0., cx);
            app.receive(Event::HistorySeeked("fixture-session".into(), app.generation, vec![engine::ParsedMessage {
                role: "user".into(), blocks: vec![MessageBlock::Paragraph("Oldest prompt".into())],
            }], false, true, 0., (0., 0.01)), cx);
            app.load_history_page(false, cx);
            assert_eq!(app.rows.len(), 2, "The existing latest control must also avoid duplicating native live output");
            assert!(!app.loading);
            assert!(!app.navigation_history);
            app.rendered_rows.set(0);
        });
        draw_frame(&view, cx);
        cx.update(|_, cx| assert!(view.read(cx).rendered_rows.get() < 100));
    }

    #[gpui::test]
    fn stale_seeks_and_completion_cannot_replace_the_history_being_read(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx);
        open(&view, cx);
        view.update(cx, |app, cx| {
            app.seek_chat(0.2, cx);
            let stale = app.generation;
            app.seek_chat(0.8, cx);
            app.receive(
                Event::HistorySeeked(
                    "fixture-session".into(),
                    stale,
                    vec![],
                    false,
                    false,
                    0.2,
                    (0.1, 0.2),
                ),
                cx,
            );
            assert_eq!(app.rows.len(), 1000);
            app.receive(
                Event::HistorySeeked(
                    "fixture-session".into(),
                    app.generation,
                    vec![engine::ParsedMessage {
                        role: "user".into(),
                        blocks: vec![MessageBlock::Paragraph("Historical point".into())],
                    }],
                    true,
                    true,
                    0.8,
                    (0.7, 0.8),
                ),
                cx,
            );
            app.receive(
                Event::Parsed(
                    "fixture-session".into(),
                    vec![MessageBlock::Paragraph("New completion".into())],
                ),
                cx,
            );
            app.receive(Event::PagedFinished("fixture-session".into()), cx);
            assert_eq!(app.rows.len(), 1);
            assert!(
                matches!(&app.rows[0].block, MessageBlock::Paragraph(s) if s == "Historical point")
            );
            assert!(!app.loading);
            assert!(app.has_newer);
        });
    }

    #[gpui::test]
    fn failed_seek_restores_live_display_and_keeps_draft(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx);
        open(&view, cx);
        view.update(cx, |app, cx| {
            app.runs
                .insert("fixture-session".into(), idle_run(String::new()));
            app.seek_chat(0., cx);
            app.receive(
                Event::LoadError(
                    "fixture-session".into(),
                    app.generation,
                    "Fixture read error".into(),
                ),
                cx,
            );
            assert!(!app.navigation_history);
            assert!(app.navigation_previous.is_none());
            assert!(!app.loading_earlier);
            assert_eq!(app.runs.len(), 1);
            assert_eq!(app.composer.read(cx).text(), "Keep my draft");
        });
    }

    #[gpui::test]
    fn retrying_prompt_after_failed_latest_reload_cannot_append_into_old_history(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = fixture(cx);
        open(&view, cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                let agent = app
                    .agents
                    .iter_mut()
                    .find(|agent| agent.kind == AgentKind::Codex)
                    .unwrap();
                agent.available = true;
                agent.executable = Some(PathBuf::from("/bin/false"));
                app.navigation_history = true;
                app.has_newer = true;
                for _ in 0..2 {
                    app.send(window, cx);
                    assert!(
                        app.send_after_load.is_some(),
                        "Send must reload latest before appending a prompt"
                    );
                    app.receive(
                        Event::LoadError(
                            "fixture-session".into(),
                            app.generation,
                            "Fixture read failure".into(),
                        ),
                        cx,
                    );
                    assert!(app.navigation_history);
                    assert!(app.has_newer);
                    assert!(app.send_after_load.is_none());
                    assert_eq!(app.rows.len(), 1000);
                    assert_eq!(app.composer.read(cx).text(), "Keep my draft");
                    assert!(app.runs.is_empty());
                }
            })
        });
    }

    #[gpui::test]
    fn reduced_motion_snaps_and_never_schedules_animation_frames(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx);
        open(&view, cx);
        view.update(cx, |app, cx| {
            app.navigation.update(cx, |rail, cx| {
                rail.set_motion(false, cx);
                rail.set_position(0.321, cx);
                assert_eq!(rail.position.value, rail.position.target);
                assert_eq!(rail.position.velocity, 0.);
                assert!(!rail.frame_pending);
            });
        });
        draw_frame(&view, cx);
        cx.update(|_, cx| assert!(!view.read(cx).navigation.read(cx).frame_pending));
    }
}
