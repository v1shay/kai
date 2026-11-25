//! Continuous project selection for the new-chat title. A gesture previews
//! neighboring cards; only the settled card changes the active workspace.
use super::*;
use std::path::Path;

const CARD_HEIGHT: f32 = 34.;
const POINTS_PER_PROJECT: f32 = 44.;
const SCROLL_IDLE: Duration = Duration::from_millis(115);
const DRAG_SLOP: f32 = 4.;

#[derive(Default)]
pub(super) struct ProjectWheel {
    spin: Option<Spin>,
    pointer: Option<Point<Pixels>>,
    suppress_click: bool,
    direct_scroll: bool,
    arrival: Option<Arrival>,
}

struct Spin {
    origin: PathBuf,
    paths: Vec<PathBuf>,
    anchor: Option<i64>,
    shown: f64,
    goal: f64,
    direction: f64,
    sampled_at: Instant,
    duration: Duration,
    motion: bool,
    drive: Drive,
}

enum Drive {
    Input {
        until: Option<Instant>,
    },
    Pressure {
        speed: f64,
    },
    Snap {
        at: Instant,
        from: f64,
        to: f64,
        duration: Duration,
    },
}

#[derive(Clone, Copy)]
struct Arrival {
    at: Option<Instant>,
    duration: Duration,
    direction: f32,
}

impl Spin {
    fn new(
        origin: PathBuf,
        paths: Vec<PathBuf>,
        now: Instant,
        motion: bool,
        duration: Duration,
    ) -> Self {
        let anchor = paths
            .iter()
            .position(|path| *path == origin)
            .map(|index| index as i64);
        Self {
            origin,
            paths,
            anchor,
            shown: 0.,
            goal: 0.,
            direction: 1.,
            sampled_at: now,
            duration,
            motion,
            drive: Drive::Input {
                until: Some(now + SCROLL_IDLE),
            },
        }
    }

    fn path(&self, card: i64) -> &Path {
        let index = match self.anchor {
            Some(anchor) => anchor + card,
            None if card > 0 => card - 1,
            None if card < 0 => card,
            None => return &self.origin,
        };
        &self.paths[index.rem_euclid(self.paths.len() as i64) as usize]
    }

    fn cards(&self) -> impl Iterator<Item = (&Path, f32)> + '_ {
        let center = self.shown.round() as i64;
        (center - 1..=center + 1).filter_map(move |index| {
            let distance = (index as f64 - self.shown) as f32;
            if self.motion {
                (distance.abs() < 1.).then(|| (self.path(index), distance))
            } else {
                (index == center).then(|| (self.path(index), 0.))
            }
        })
    }

    fn nudge(&mut self, delta: f64, now: Instant, direct: bool) {
        self.sample(now);
        if matches!(self.drive, Drive::Snap { .. }) {
            self.goal = self.shown;
        }
        self.direction = delta.signum();
        self.goal += delta;
        self.drive = match self.drive {
            Drive::Pressure { speed } => Drive::Pressure {
                speed: speed.abs() * self.direction,
            },
            _ => Drive::Input {
                until: (!direct).then_some(now + SCROLL_IDLE),
            },
        };
        if !self.motion {
            self.shown = self.goal;
        }
    }

    fn pressure(&mut self, pressure: f32, now: Instant) {
        self.sample(now);
        self.goal = self.shown;
        let speed = (3.5 + 7.5 * f64::from(pressure.clamp(0., 1.))) * self.direction;
        self.drive = Drive::Pressure { speed };
    }

    fn release(&mut self, now: Instant) {
        self.sample(now);
        if let Drive::Pressure { speed } = self.drive {
            let duration = self.duration.mul_f64(3.8);
            let distance = if self.motion {
                speed * duration.as_secs_f64() / 3.
            } else {
                0.
            };
            self.snap(self.goal + distance, now, duration);
        } else if matches!(self.drive, Drive::Input { .. }) {
            self.snap(self.goal, now, self.duration);
        }
    }

    fn snap(&mut self, target: f64, at: Instant, duration: Duration) {
        self.goal = target.round();
        self.drive = Drive::Snap {
            at,
            from: self.shown,
            to: self.goal,
            duration,
        };
    }

    /// Samples real elapsed time so display rate and input event cadence do not
    /// affect the distance traveled. The platform supplies ordinary momentum.
    fn sample(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.sampled_at).as_secs_f64();
        self.sampled_at = now;
        match self.drive {
            Drive::Pressure { speed } => {
                self.goal += speed * elapsed;
                self.shown = self.goal;
            }
            Drive::Input { until } => {
                let follow = self.duration.as_secs_f64() * 0.13;
                let fraction = if self.motion {
                    1. - (-elapsed / follow).exp()
                } else {
                    1.
                };
                self.shown += (self.goal - self.shown) * fraction;
                if until.is_some_and(|deadline| now >= deadline) {
                    self.snap(self.goal, now, self.duration);
                }
            }
            Drive::Snap {
                at,
                from,
                to,
                duration,
            } => {
                let t = fraction_since(now, at, duration);
                self.shown = from + (to - from) * f64::from(ease(t));
                return !self.motion
                    || t >= 1.
                    || ((to - from).abs() < 0.002 && self.path(to as i64) == self.origin);
            }
        }
        false
    }

    fn needs_frame(&self) -> bool {
        !matches!(self.drive, Drive::Input { until: None })
            || (self.goal - self.shown).abs() > 0.002
    }

    fn departure(&self, now: Instant) -> (f32, f32) {
        let angle = (self.shown * std::f64::consts::TAU) as f32;
        let moving = (-angle.sin() * 5., 0.89 + 0.11 * angle.cos());
        if let Drive::Snap {
            at, to, duration, ..
        } = self.drive
            && self.path(to as i64) != self.origin
        {
            let phase = ease(((fraction_since(now, at, duration) - 0.60) / 0.40).clamp(0., 1.));
            return (
                moving.0 - self.direction as f32 * phase * 12.,
                moving.1 * (1. - phase * 0.88),
            );
        }
        moving
    }
}

use workspace_motion::ease_out as ease;

fn fraction_since(now: Instant, at: Instant, duration: Duration) -> f32 {
    (now.saturating_duration_since(at).as_secs_f32() / duration.as_secs_f32()).clamp(0., 1.)
}

fn projected_card(card: Div, distance: f32, extent: f32) -> Div {
    let angle = distance * std::f32::consts::FRAC_PI_2;
    let facing = angle.cos().max(0.);
    let height = extent * facing;
    let center = extent * (0.5 + angle.sin() * 0.52);
    card.absolute()
        .left_0()
        .right_0()
        .top(px(center - height * 0.5))
        .h(px(height))
        .opacity(facing.powf(1.3))
        .overflow_hidden()
}

impl Orbit {
    fn project_wheel_draft(&self)->bool{
        self.active.is_none() || (self.rows.is_empty() && !self.loading && !self.starting_current()
            && self.active.as_ref().is_some_and(|id|self.draft_sessions.contains(id)))
    }
    fn start_project_spin(&mut self, now: Instant) -> Option<&mut Spin> {
        if !self.project_wheel_draft()
            || self.active_chat_cli().is_some()
            || self.modal.is_some()
            || self.ninja_surface_owner().is_some()
            || self.projects.is_empty()
        {
            return None;
        }
        if self.projects.len() == 1 && self.projects[0].path == self.project {
            return None;
        }
        if self
            .project_wheel
            .spin
            .as_ref()
            .is_none_or(|spin| spin.origin != self.project)
        {
            let paths = self
                .projects
                .iter()
                .map(|project| project.path.clone())
                .collect();
            self.project_wheel.spin = Some(Spin::new(
                self.project.clone(),
                paths,
                now,
                self.motion && !native::reduced_motion(),
                workspace_motion::pane_duration(),
            ));
            self.project_wheel.arrival = None;
        }
        self.project_wheel.spin.as_mut()
    }

    fn scroll_project_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let (delta, stride) = match event.delta {
            ScrollDelta::Pixels(delta) => {
                match event.touch_phase {
                    TouchPhase::Started => self.project_wheel.direct_scroll = true,
                    TouchPhase::Ended => self.project_wheel.direct_scroll = false,
                    TouchPhase::Moved => {}
                }
                (delta.map(f32::from), POINTS_PER_PROJECT)
            }
            ScrollDelta::Lines(delta) => {
                self.project_wheel.direct_scroll = false;
                (delta, 1.)
            }
        };
        let direct = self.project_wheel.direct_scroll;
        let amount = if delta.x.abs() > delta.y.abs() {
            delta.x
        } else {
            delta.y
        };
        let now = Instant::now();
        if amount.is_finite() && amount.abs() >= 0.01 {
            if let Some(spin) = self.start_project_spin(now) {
                spin.nudge(f64::from((-amount / stride).clamp(-24., 24.)), now, direct);
                cx.stop_propagation();
                cx.notify();
            }
        }
    }

    fn end_project_scroll(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.project_wheel.direct_scroll) {
            return;
        }
        if let Some(spin) = &mut self.project_wheel.spin
            && matches!(spin.drive, Drive::Input { until: None })
        {
            spin.drive = Drive::Input {
                until: Some(Instant::now() + SCROLL_IDLE),
            };
            cx.notify();
        }
    }

    fn drag_project_wheel(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(previous) = self.project_wheel.pointer.filter(|_| event.dragging()) else {
            return;
        };
        let delta = event.position - previous;
        let amount = if delta.x.abs() > delta.y.abs() {
            delta.x
        } else {
            delta.y
        };
        let amount = f32::from(amount);
        if amount.abs()
            < if self.project_wheel.suppress_click {
                0.05
            } else {
                DRAG_SLOP
            }
        {
            return;
        }
        let now = Instant::now();
        if let Some(spin) = self.start_project_spin(now) {
            spin.nudge(f64::from(-amount / POINTS_PER_PROJECT), now, true);
            self.project_wheel.suppress_click = true;
            self.project_wheel.pointer = Some(event.position);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn press_project_wheel(&mut self, event: &PressureChangeEvent, cx: &mut Context<Self>) {
        if self.project_wheel.pointer.is_none() {
            return;
        }
        let now = Instant::now();
        if event.stage >= 2 && event.pressure.is_finite() {
            if let Some(spin) = self.start_project_spin(now) {
                spin.pressure(event.pressure, now);
                self.project_wheel.suppress_click = true;
                cx.stop_propagation();
                cx.notify();
            }
        } else if let Some(spin) = &mut self.project_wheel.spin
            && matches!(spin.drive, Drive::Pressure { .. })
        {
            spin.release(now);
            cx.notify();
        }
    }

    pub(super) fn tick_project_wheel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.project_wheel_draft()
            || self.ninjas.panel.is_some()
            || self.modal.is_some()
            || self.active_chat_cli().is_some()
        {
            self.project_wheel = ProjectWheel::default();
            return;
        }

        let now = Instant::now();
        if let Some(spin) = self.project_wheel.spin.as_mut() {
            if spin.origin != self.project {
                self.project_wheel.spin = None;
            } else if matches!(spin.drive, Drive::Pressure { .. }) && !window.is_window_active() {
                spin.release(spin.sampled_at);
                self.project_wheel.pointer = None;
                request_motion_frame(window, cx);
            } else if spin.sample(now) {
                self.commit_project_wheel(window, cx, true);
            } else if spin.needs_frame() {
                request_motion_frame(window, cx);
            }
        }
        if let Some(arrival) = &mut self.project_wheel.arrival {
            if arrival.at.is_none() {
                let reading_files = self.files_visible
                    && self
                        .file_browser
                        .as_ref()
                        .is_some_and(|files| files.read(cx).project_root_pending());
                let reading_branch = self
                    .diff_view
                    .as_ref()
                    .is_some_and(|review| review.read(cx).workspace_root_pending());
                if reading_files || reading_branch {
                    return;
                }
                arrival.at = Some(now);
            }
            if now.saturating_duration_since(arrival.at.unwrap_or(now)) < arrival.duration {
                request_motion_frame(window, cx);
            } else {
                self.project_wheel.arrival = None;
            }
        }
    }

    pub(super) fn finish_project_wheel_for_send(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(spin) = &mut self.project_wheel.spin {
            spin.goal = spin.shown.round();
            self.commit_project_wheel(window, cx, false);
        }
    }

    fn commit_project_wheel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        preserve_draft: bool,
    ) {
        let Some(spin) = self.project_wheel.spin.take() else {
            return;
        };
        let path = spin.path(spin.goal.round() as i64);
        if spin.origin != self.project
            || !self.project_wheel_draft()
            || path == self.project
            || !self.projects.iter().any(|project| project.path == path)
        {
            return;
        }
        let direction = if spin.goal < 0. { -1. } else { 1. };
        self.startup_owner_pristine = false;

        // Preserve a draft under its original root before changing ownership.
        if preserve_draft
            && (!self.composer.read(cx).text().trim().is_empty() || !self.attachments.is_empty())
        {
            // Archiving the draft changes chat ownership within the same home
            // surface. Keep the open inspector and the title's entrance epoch.
            let review = self.diff_view.take();
            let observer = self.diff_observer.take();
            let started = self.diff_started.take();
            let home_generation = self.home_generation;
            self.new_conversation(window, cx);
            self.diff_view = review;
            self.diff_observer = observer;
            self.diff_started = started;
            self.home_generation = home_generation;
        }
        // A restored empty tab already owns a draft session. Move that draft's
        // execution root with the selector instead of leaving it on the old project.
        if let Some(id)=self.active.clone().filter(|id|self.draft_sessions.contains(id)){
            if let Some(session)=self.sessions.iter_mut().find(|session|session.id==id){
                session.project_path=path.to_path_buf();session.execution_path=None;
                self.engine.update_session(session.clone());
            }
        }
        self.project = path.to_path_buf();
        self.new_execution_path = None;
        self.all_projects = false;
        self.archived_view = false;
        self.rebuild_workspace_sessions();
        self.sync_chat_terminal_owner(cx);
        self.sync_file_root(window, cx);
        self.queue_workspace_save(cx);
        self.project_wheel.arrival = spin.motion.then(|| Arrival {
            at: None,
            duration: spin.duration,
            direction,
        });
    }

    pub(super) fn project_wheel_file_names(&self) -> Option<text_decode::FileNameScramble> {
        if !self.motion || native::reduced_motion() {
            return None;
        }
        let now = Instant::now();
        let progress = if self
            .project_wheel
            .spin
            .as_ref()
            .is_some_and(|spin| spin.motion)
        {
            0.
        } else {
            let arrival = self.project_wheel.arrival?;
            arrival
                .at
                .map_or(0., |at| ease(fraction_since(now, at, arrival.duration)))
        };
        // Keep glyph changes slower than the display cadence. Use the same
        // clock through a reversal and when the destination first renders.
        let elapsed = Duration::from_secs_f64(workspace_motion::now().max(0.));
        text_decode::FileNameScramble::sample(elapsed, progress)
    }

    pub(super) fn project_wheel_pane(&self, content: Div) -> Div {
        let now = Instant::now();
        if let Some(spin) = &self.project_wheel.spin
            && spin.motion
        {
            let (offset, opacity) = spin.departure(now);
            return content.relative().top(px(offset)).opacity(opacity);
        }
        if let Some(arrival) = self.project_wheel.arrival {
            let p = arrival
                .at
                .map_or(0., |at| ease(fraction_since(now, at, arrival.duration)));
            return content
                .relative()
                .top(px(arrival.direction * (1. - p) * 12.))
                .opacity(0.12 + 0.88 * p);
        }
        content
    }

    pub(super) fn project_wheel_should_refresh(&self) -> bool {
        self.project_wheel
            .arrival
            .is_some_and(|arrival| arrival.at.is_none())
    }

    fn project_wheel_card(&self, path: &Path, distance: f32, cx: &Context<Self>) -> Div {
        let has_project = path != self.unprojected_root() && path != Path::new("/");
        let label = if has_project {
            self.project_path_label(path)
        } else {
            "No project".into()
        };
        let clearable = has_project && self.project_wheel.spin.is_none();
        let content = div()
            .relative()
            .max_w_full()
            .min_w(px(0.))
            .flex()
            .items_center()
            .gap_2()
            .when(clearable, |row| row.child(div().size(px(24.)).flex_none()))
            .when(has_project, |row| {
                row.child(project_tree::project_badge(&label, path, 22.).flex_none())
            })
            .child(div().min_w(px(0.)).truncate().child(label))
            .when(clearable, |row| {
                row.child(
                    div()
                        .id("home-clear-project")
                        .debug_selector(|| "home-clear-project".into())
                        .size(px(24.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(0.)
                        .group_hover("home-project", |button| button.opacity(1.))
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|button| button.bg(rgba(GLASS_HOVER)))
                        .child(icon("close", 12., MUTED))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|app, _, window, cx| {
                            cx.stop_propagation();
                            app.project_wheel = ProjectWheel::default();
                            app.new_unprojected_conversation(window, cx);
                        })),
                )
            });
        projected_card(div(), distance, CARD_HEIGHT)
            .rounded_md()
            .flex()
            .items_center()
            .justify_center()
            .px_2()
            .when(distance.abs() > 0.001, |card| {
                card.bg(rgba(self.appearance.tint(0xebebeb0e)))
            })
            .child(content)
    }

    pub(super) fn project_wheel_branch(&self, label: String, primary: bool) -> Div {
        let row = |label| {
            div()
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .text_size(theme::ui_px(11.))
                .text_color(rgb(MUTED))
                .child(icon("branch", 14., MUTED))
                .child(div().min_w(px(0.)).truncate().child(label))
        };
        if primary && let Some(spin) = &self.project_wheel.spin {
            return div()
                .relative()
                .h(px(30.))
                .overflow_hidden()
                .children(spin.cards().map(|(path, distance)| {
                    projected_card(row(self.project_path_label(path)), distance, 30.)
                }));
        }
        row(label).h(px(30.))
    }

    pub(super) fn home_project_control(&self, cx: &Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let input = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let drag = weak.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase.capture() {
                        let _ = drag.update(cx, |app, cx| app.drag_project_wheel(event, cx));
                    }
                });
                let scroll_end = weak.clone();
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                    if phase.capture() && matches!(event.touch_phase, TouchPhase::Ended) {
                        let _ = scroll_end.update(cx, |app, cx| app.end_project_scroll(cx));
                    }
                });
                let pressure = weak.clone();
                window.on_mouse_event(move |event: &PressureChangeEvent, phase, _, cx| {
                    if phase.capture() {
                        let _ = pressure.update(cx, |app, cx| app.press_project_wheel(event, cx));
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase.capture() && event.button == MouseButton::Left {
                        let _ = weak.update(cx, |app, cx| {
                            if app.project_wheel.pointer.take().is_some()
                                && app.project_wheel.suppress_click
                                && let Some(spin) = &mut app.project_wheel.spin
                                && !matches!(spin.drive, Drive::Snap { .. })
                            {
                                spin.release(Instant::now());
                                cx.notify();
                            }
                        });
                    }
                });
            },
        )
        .absolute()
        .inset_0();

        let mut cards = div()
            .id("home-project-title")
            .debug_selector(|| "home-project-title".into())
            .flex_1()
            .h_full()
            .min_w(px(0.))
            .relative()
            .overflow_hidden()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, event: &MouseDownEvent, _, _| {
                    app.project_wheel.pointer = Some(event.position);
                    app.project_wheel.suppress_click = false;
                }),
            )
            .on_click(cx.listener(|app, _, window, cx| {
                if !app.project_wheel.suppress_click {
                    app.project_wheel.spin = None;
                    app.show_modal(Modal::Projects, window, cx);
                }
            }));

        if let Some(spin) = &self.project_wheel.spin {
            cards = cards.children(
                spin.cards()
                    .map(|(path, distance)| self.project_wheel_card(path, distance, cx)),
            );
        } else {
            cards = cards.child(self.project_wheel_card(&self.project, 0., cx));
        }

        div()
            .id("home-project")
            .group("home-project")
            .w(relative(1.))
            .max_w(relative(1.))
            .h(px(CARD_HEIGHT))
            .flex()
            .items_center()
            .text_size(theme::ui_px(13.))
            .text_color(rgb(MUTED))
            .child(cards.child(input))
            .on_scroll_wheel(cx.listener(|app, event, _, cx| app.scroll_project_wheel(event, cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_tests::{draw_frame, fixture};
    use std::prelude::v1::test;

    fn wheel(now: Instant, motion: bool, duration: Duration) -> Spin {
        Spin::new(
            "/alpha".into(),
            ["/alpha", "/beta", "/gamma", "/delta"]
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            now,
            motion,
            duration,
        )
    }

    #[test]
    fn direct_manipulation_keeps_fractional_movement_and_parks_while_held() {
        let now = Instant::now();
        let mut spin = wheel(now, true, Duration::from_millis(220));
        spin.nudge(0.25, now, true);
        assert!(!spin.sample(now + Duration::from_millis(15)));
        let partial = spin.shown;
        assert!(partial > 0. && partial < 0.25);
        spin.nudge(-0.10, now + Duration::from_millis(15), true);
        assert_eq!(spin.shown, partial);
        assert!(!spin.sample(now + Duration::from_secs(2)));
        assert!((spin.shown - 0.15).abs() < 0.001);
        assert!(!spin.needs_frame());
        spin.release(now + Duration::from_secs(2));
        assert!(spin.sample(now + Duration::from_secs(3)));
        assert_eq!(spin.path(spin.goal as i64), Path::new("/alpha"));
    }

    #[test]
    fn pressure_spin_uses_elapsed_time_and_reverses_or_coasts() {
        let now = Instant::now();
        let duration = Duration::from_millis(220);
        let mut spin = wheel(now, true, duration);
        let mut dense = wheel(now, true, duration);
        spin.pressure(1., now);
        dense.pressure(1., now);
        for step in 1..=120 {
            dense.sample(now + Duration::from_millis(step * 10));
        }
        let released = now + Duration::from_millis(1200);
        spin.sample(released);
        assert!((spin.shown - dense.shown).abs() < 0.0001);
        assert!(spin.shown > 12.);
        let from = spin.shown;
        spin.release(released);
        spin.sample(released + Duration::from_millis(100));
        assert!(spin.shown > from && spin.shown < spin.goal);
        let reversing = spin.shown;
        spin.nudge(-0.9, released + Duration::from_millis(100), true);
        assert_eq!(spin.shown, reversing);
        spin.sample(released + Duration::from_millis(250));
        assert!(spin.shown < reversing);
    }

    #[test]
    fn short_and_relaxed_profiles_settle_at_different_times_and_wrap_projects() {
        let now = Instant::now();
        let quick = Duration::from_millis(160);
        let mut short = wheel(now, true, quick);
        let mut relaxed = wheel(now, true, Duration::from_millis(300));
        for spin in [&mut short, &mut relaxed] {
            spin.nudge(-1.1, now, true);
            spin.release(now);
            assert_eq!(spin.path(spin.goal as i64), Path::new("/delta"));
        }
        assert!(short.sample(now + quick));
        assert!(!relaxed.sample(now + quick));
        let mut unbound = Spin::new("/none".into(), short.paths, now, false, quick);
        assert_eq!(unbound.path(0), Path::new("/none"));
        assert_eq!(unbound.path(1), Path::new("/alpha"));
        assert_eq!(unbound.path(-1), Path::new("/delta"));
        unbound.nudge(0.9, now, true);
        unbound.release(now);
        assert!(unbound.sample(now));
    }

    #[gpui::test]
    fn deep_press_captures_the_title_and_release_starts_the_glide(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.new_conversation(window, cx);
                app.motion = true;
                app.projects.push(Project {
                    name: "Next".to_owned(),
                    path: app.project.join("next"),
                });
            })
        });
        draw_frame(&view, cx);
        let position = cx.debug_bounds("home-project-title").unwrap().center();
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position,
            click_count: 1,
            ..Default::default()
        });
        cx.simulate_event(PressureChangeEvent {
            position,
            stage: 2,
            pressure: 0.85,
        });
        view.update(cx, |app, _| {
            let spin = app.project_wheel.spin.as_ref().unwrap();
            assert!(matches!(spin.drive, Drive::Pressure { speed } if speed > 9.));
            assert!(app.project_wheel.suppress_click);
        });
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position,
            click_count: 1,
            ..Default::default()
        });
        view.update(cx, |app, _| {
            let spin = app.project_wheel.spin.as_ref().unwrap();
            assert!(matches!(spin.drive, Drive::Snap { duration, .. } if duration > spin.duration));
            assert!(
                app.modal.is_none(),
                "Pressure release opened the projects picker"
            );
        });
    }

    #[gpui::test]
    fn files_use_scrambled_labels_only_while_the_wheel_is_in_motion(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx);
        let (first, next) = cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.new_conversation(window, cx);
                app.motion = true;
                let first = app.project.clone();
                let next = first.join("Next project");
                std::fs::create_dir_all(&next).unwrap();
                app.projects.push(Project {
                    name: "Next project".into(),
                    path: next.clone(),
                });
                app.toggle_files(window, cx);
                (first, next)
            })
        });
        draw_frame(&view, cx);
        view.update(cx, |app, cx| {
            assert!(
                app.file_browser
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .name_scramble
                    .is_none()
            )
        });
        let position = cx.debug_bounds("home-project-title").unwrap().center();
        cx.simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), px(-48.))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Started,
        });
        draw_frame(&view, cx);
        view.update(cx, |app, cx| {
            assert_eq!(app.project, first);
            assert_eq!(
                app.file_browser
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .name_scramble
                    .unwrap()
                    .paint("original.txt")
                    .chars()
                    .count(),
                12
            );
            let spin = app.project_wheel.spin.as_mut().unwrap();
            spin.snap(
                spin.goal,
                Instant::now() - Duration::from_secs(1),
                spin.duration,
            );
        });
        draw_frame(&view, cx);
        view.update(cx, |app, cx| {
            assert_eq!(app.project, next);
            assert!(
                app.file_browser
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .name_scramble
                    .is_some()
            );
            app.project_wheel.arrival.as_mut().unwrap().at =
                Some(Instant::now() - Duration::from_secs(1));
        });
        draw_frame(&view, cx);
        view.update(cx, |app, cx| {
            assert!(
                app.file_browser
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .name_scramble
                    .is_none()
            )
        });
    }

    #[gpui::test]
    fn trackpad_preview_keeps_branch_root_and_draft_until_the_card_settles(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = fixture(cx);
        let (first, next) = cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.new_conversation(window, cx);
                app.motion = true;
                let first = app.project.clone();
                let next = first.join("Second project");
                std::fs::create_dir_all(&next).unwrap();
                app.projects.push(Project {
                    name: "Second project".to_owned(),
                    path: next.clone(),
                });
                app.open_changes(cx);
                app.composer.update(cx, |input, cx| {
                    input.set_text("Draft for the first project", cx)
                });
                (first, next)
            })
        });
        draw_frame(&view, cx);
        let position = cx.debug_bounds("home-project-title").unwrap().center();
        let scroll = |phase, y| ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), px(y))),
            modifiers: Modifiers::default(),
            touch_phase: phase,
        };
        // One native-routed event pins the integrated viewport test to this
        // frame. Fractional accumulation and reversal are covered above.
        cx.simulate_event(scroll(TouchPhase::Started, -48.));
        view.update(cx, |app, cx| {
            let spin = app.project_wheel.spin.as_ref().unwrap();
            assert!((spin.goal - 48. / f64::from(POINTS_PER_PROJECT)).abs() < 0.001);
            assert_eq!(app.project, first);
            assert_eq!(app.diff_view.as_ref().unwrap().read(cx).test_root(), first);
        });
        cx.simulate_event(scroll(TouchPhase::Ended, 0.));
        view.update(cx, |app, _| {
            let spin = app.project_wheel.spin.as_mut().unwrap();
            assert!(matches!(spin.drive, Drive::Input { until: Some(_) }));
            let backdated = Instant::now() - Duration::from_secs(1);
            spin.sampled_at = backdated;
            spin.snap(spin.goal, backdated, spin.duration);
        });
        draw_frame(&view, cx);
        view.update(cx, |app, cx| {
            assert_eq!(app.project, next);
            assert_eq!(app.diff_view.as_ref().unwrap().read(cx).test_root(), next);
            assert!(app.project_wheel.spin.is_none());
            assert_eq!(app.draft_sessions.len(), 1);
            let original = app
                .sessions
                .iter()
                .find(|session| app.draft_sessions.contains(&session.id))
                .unwrap();
            assert_eq!(original.project_path, first);
            assert!(app.composer.read(cx).text().is_empty());
        });
    }
}
