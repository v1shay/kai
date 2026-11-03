//! Native provider rotation; no per-frame parsing, discovery, or history work.
use crate::theme::rgb;
use gpui::{prelude::*, *};
use std::{f64::consts::TAU, time::Instant};

#[derive(Default)]
struct Rotor {
    angle: f64,
    velocity: f64,
    busy: bool,
    target: f64,
}
impl Rotor {
    fn set_busy(&mut self, busy: bool) {
        if self.busy == busy {
            return;
        }
        self.busy = busy;
        if !busy {
            self.target = if self.velocity < 1. {
                (self.angle / TAU).round() * TAU
            } else {
                (self.angle / TAU).ceil() * TAU
            };
        }
    }
    fn moving(&self) -> bool {
        self.busy || self.velocity.abs() > 0.015 || (self.angle - self.target).abs() > 0.001
    }
    fn step(&mut self, dt: f64) {
        if !dt.is_finite() || dt <= 0. {
            return;
        }
        if self.busy {
            let speed = TAU / 1.45;
            let tau = 0.24;
            let decay = (-dt / tau).exp();
            self.angle += speed * dt + (self.velocity - speed) * tau * (1. - decay);
            self.velocity = speed + (self.velocity - speed) * decay;
            self.angle = self.angle.rem_euclid(TAU);
        } else if self.moving() {
            // Analytic underdamped spring: interruption preserves angular momentum.
            let omega = 8.;
            let damping = 0.84;
            let damped = omega * (1f64 - damping * damping).sqrt();
            let (sin, cos) = (damped * dt).sin_cos();
            let decay = (-damping * omega * dt).exp();
            let y = self.angle - self.target;
            let v = self.velocity;
            self.angle = self.target + decay * (y * cos + (v + damping * omega * y) / damped * sin);
            self.velocity =
                decay * (v * cos - (damping * omega * v + omega * omega * y) / damped * sin);
            if !self.moving() {
                *self = Self::default();
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Config {
    pub path: &'static str,
    pub color: u32,
    pub size: u8,
    pub busy: bool,
    pub motion: bool,
    pub awake: bool,
}
pub(crate) struct Settled;
pub(crate) struct LoadingLogo {
    config: Config,
    rotor: Rotor,
    last: Instant,
}
impl EventEmitter<Settled> for LoadingLogo {}
impl LoadingLogo {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self {
            config: Config {
                path: "codex",
                color: 0xa39cfa,
                size: 34,
                busy: false,
                motion: false,
                awake: false,
            },
            rotor: Rotor::default(),
            last: Instant::now(),
        }
    }
    fn sample(&mut self) {
        let dt = self.last.elapsed().as_secs_f64().min(0.05);
        self.last = Instant::now();
        if self.config.motion && self.config.awake {
            self.rotor.step(dt);
        }
    }
    pub fn configure(&mut self, config: Config, cx: &mut Context<Self>) {
        if self.config == config {
            return;
        }
        self.sample();
        if self.config.path != config.path || !config.motion {
            self.rotor = Rotor::default();
        }
        self.rotor.set_busy(config.busy && config.motion);
        self.config = config;
        self.last = Instant::now();
        cx.notify();
    }
    pub fn moving(&self) -> bool {
        self.config.motion && self.rotor.moving()
    }
    #[cfg(test)]
    pub fn advance(&mut self, seconds: f64, cx: &mut Context<Self>) {
        if self.config.motion && self.config.awake {
            for _ in 0..(seconds * 120.).round() as usize {
                self.rotor.step(1. / 120.);
            }
        }
        self.last = Instant::now();
        if !self.rotor.moving() {
            cx.emit(Settled);
        }
        cx.notify();
    }
    #[cfg(test)]
    pub fn angle(&self) -> f64 {
        self.rotor.angle
    }
    #[cfg(test)]
    pub fn needs_frames(&self) -> bool {
        self.config.awake && self.moving()
    }
}
impl Render for LoadingLogo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let was_moving = self.rotor.moving();
        self.sample();
        if was_moving && !self.rotor.moving() {
            cx.emit(Settled);
        }
        if self.config.awake && self.moving() {
            let entity = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                if let Some(entity) = entity.upgrade() {
                    cx.notify(entity.entity_id());
                }
            });
        }
        div()
            .size(px(self.config.size as f32))
            .flex_none()
            .debug_selector(|| "loading-provider-logo".into())
            .child(
                svg()
                    .debug_selector(|| "loading-provider-svg".into())
                    .path(if matches!(self.config.path,"codex"|"assets/providers/codex.svg"|"assets/codex.svg"){"codex-mask"}else{self.config.path})
                    .size_full()
                    .text_color(rgb(self.config.color))
                    .with_transformation(Transformation::rotate(radians(self.rotor.angle as f32))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn rotation_accelerates_and_settles_upright_from_every_phase() {
        for duration in [0.02, 0.2, 0.6, 1.4, 4.] {
            let mut rotor = Rotor::default();
            rotor.set_busy(true);
            rotor.step(0.01);
            assert!(rotor.velocity > 0. && rotor.velocity < TAU / 1.45);
            for _ in 0..(duration * 120.) as usize {
                rotor.step(1. / 120.);
            }
            let before = (rotor.angle, rotor.velocity);
            rotor.set_busy(false);
            assert_eq!((rotor.angle, rotor.velocity), before);
            for _ in 0..360 {
                rotor.step(1. / 120.);
            }
            assert!(!rotor.moving() && rotor.angle == 0. && rotor.velocity == 0.);
        }
    }
    #[test]
    fn interrupted_settle_preserves_pose_and_frames_do_not_change_the_physics() {
        let mut a = Rotor::default();
        a.set_busy(true);
        let mut b = Rotor::default();
        b.set_busy(true);
        for _ in 0..60 {
            a.step(1. / 60.);
        }
        for _ in 0..120 {
            b.step(1. / 120.);
        }
        assert!((a.angle - b.angle).abs() < 1e-10);
        a.set_busy(false);
        a.step(0.1);
        let before = (a.angle, a.velocity);
        a.set_busy(true);
        assert_eq!((a.angle, a.velocity), before);
    }
}
