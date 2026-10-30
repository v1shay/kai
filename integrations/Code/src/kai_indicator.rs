//! Native Kai scene playback. The bundled catalog comes from Kai's Swift engine.
//! Only the visible live indicator animates; the rest of the UI uses static SVGs.
use crate::theme::{rgb, rgba};
use gpui::{
    Context, IntoElement, PathBuilder, Pixels, Point, Render, Task, Timer, Window, canvas, div,
    point, prelude::*, px, svg,
};
use serde::Deserialize;
use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

const TRACKS: usize = 8;
const KEYS: usize = 4;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
enum Shape {
    #[default]
    Capsule,
    Ellipse,
    Question,
    HourglassLeft,
    HourglassRight,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Pose {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
    rotation: f32,
    opacity: f32,
    shape: Shape,
    line_width: f32,
}
impl Pose {
    const HIDDEN: Self = Self {
        x: 14.,
        y: 9.,
        width: 0.1,
        height: 0.1,
        radius: 0.05,
        rotation: 0.,
        opacity: 0.,
        shape: Shape::Capsule,
        line_width: 1.4,
    };
    fn blend(self, to: Self, t: f32) -> Self {
        let mix = |a, b| a + (b - a) * t;
        Self {
            x: mix(self.x, to.x),
            y: mix(self.y, to.y),
            width: mix(self.width, to.width),
            height: mix(self.height, to.height),
            radius: mix(self.radius, to.radius),
            rotation: mix(self.rotation, to.rotation),
            opacity: mix(self.opacity, to.opacity),
            shape: to.shape,
            line_width: mix(self.line_width, to.line_width),
        }
    }
    fn valid(self) -> bool {
        [
            self.x,
            self.y,
            self.width,
            self.height,
            self.radius,
            self.rotation,
            self.opacity,
            self.line_width,
        ]
        .iter()
        .all(|v| v.is_finite() && v.abs() <= 128.)
            && self.width > 0.
            && self.height > 0.
            && self.radius >= 0.
            && (0. ..=1.).contains(&self.opacity)
            && self.line_width > 0.
    }
}
#[derive(Clone, Copy)]
struct Track {
    poses: [Pose; KEYS],
    times: [f32; KEYS],
    count: usize,
    phase: f64,
}
impl Track {
    fn sample(self, time: f32) -> Pose {
        for i in 1..self.count {
            if time <= self.times[i] {
                let t = ((time - self.times[i - 1]) / (self.times[i] - self.times[i - 1]))
                    .clamp(0., 1.);
                return self.poses[i - 1].blend(self.poses[i], t);
            }
        }
        self.poses[self.count - 1]
    }
}
struct Scene {
    id: String,
    duration: f64,
    transition: f64,
    tracks: [Track; TRACKS],
}
impl Scene {
    fn first(&self) -> [Pose; TRACKS] {
        std::array::from_fn(|i| self.tracks[i].poses[0])
    }
    fn sample(&self, elapsed: f64, looping: bool) -> [Pose; TRACKS] {
        if !looping || self.duration == 0. {
            return self.first();
        }
        std::array::from_fn(|i| {
            let track = self.tracks[i];
            let local = elapsed - track.phase;
            if local <= 0. {
                return track.poses[0];
            }
            // CAAnimationGroup applies easeInEaseOut to the complete loop; its
            // keyframes interpolate linearly at the exported keyTimes.
            let progress = (local / self.duration).rem_euclid(1.) as f32;
            track.sample(bezier(progress, 0.42, 0., 0.58, 1.))
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCatalog {
    canvas: RawCanvas,
    format_version: u32,
    scenes: Vec<RawScene>,
}
#[derive(Deserialize)]
struct RawCanvas {
    width: usize,
    height: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawScene {
    id: String,
    loop_seconds: f64,
    transition_seconds: f64,
    tracks: Vec<RawTrack>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTrack {
    poses: Vec<Pose>,
    key_times: Vec<f32>,
    phase_seconds: f64,
}
fn parse_catalog(data: &str) -> Result<Vec<Scene>, &'static str> {
    let raw: RawCatalog = serde_json::from_str(data).map_err(|_| "invalid scene JSON")?;
    if raw.format_version != 1
        || raw.canvas.width != 28
        || raw.canvas.height != 18
        || raw.scenes.len() != 24
    {
        return Err("invalid scene canvas/count/version");
    }
    let mut scenes = Vec::with_capacity(24);
    for scene in raw.scenes {
        if scene.id.is_empty()
            || scenes.iter().any(|s: &Scene| s.id == scene.id)
            || !scene.loop_seconds.is_finite()
            || !(0. ..=10.).contains(&scene.loop_seconds)
            || !scene.transition_seconds.is_finite()
            || !(0.01..=5.).contains(&scene.transition_seconds)
            || scene.tracks.len() != TRACKS
        {
            return Err("invalid scene metadata");
        }
        let mut tracks = [Track {
            poses: [Pose::HIDDEN; KEYS],
            times: [0.; KEYS],
            count: 1,
            phase: 0.,
        }; TRACKS];
        for (out, raw) in tracks.iter_mut().zip(scene.tracks) {
            let count = raw.poses.len();
            if !(2..=KEYS).contains(&count)
                || raw.key_times.len() != count
                || raw.key_times[0] != 0.
                || raw.key_times[count - 1] != 1.
                || raw
                    .key_times
                    .windows(2)
                    .any(|w| !w[0].is_finite() || w[0] >= w[1])
                || !raw.poses.iter().all(|p| p.valid())
                || !raw.phase_seconds.is_finite()
                || !(0. ..=10.).contains(&raw.phase_seconds)
            {
                return Err("invalid scene track");
            }
            out.poses[..count].copy_from_slice(&raw.poses);
            out.times[..count].copy_from_slice(&raw.key_times);
            out.count = count;
            out.phase = raw.phase_seconds;
        }
        scenes.push(Scene {
            id: scene.id,
            duration: scene.loop_seconds,
            transition: scene.transition_seconds,
            tracks,
        });
    }
    Ok(scenes)
}
fn catalog() -> &'static [Scene] {
    static CATALOG: OnceLock<Vec<Scene>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        parse_catalog(include_str!("../assets/kai/scene_catalog.json"))
            .expect("bundled Kai catalog is validated by tests")
    })
}
/// Invert a cubic's x coordinate before sampling y (time is not its parameter).
fn bezier(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    if x <= 0. {
        return 0.;
    }
    if x >= 1. {
        return 1.;
    }
    let eval = |t: f32, a: f32, b: f32| {
        3. * (1. - t) * (1. - t) * t * a + 3. * (1. - t) * t * t * b + t * t * t
    };
    let (mut lo, mut hi) = (0., 1.);
    for _ in 0..15 {
        let mid = (lo + hi) * 0.5;
        if eval(mid, x1, x2) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    eval((lo + hi) * 0.5, y1, y2)
}
struct Player {
    scene: usize,
    poses: [Pose; TRACKS],
    from: [Pose; TRACKS],
    shapes: [[f32; 5]; TRACKS],
    from_shapes: [[f32; 5]; TRACKS],
    started: f64,
    morph_seconds: f64,
    morph_completed: bool,
    looping: bool,
    awake: bool,
}
impl Player {
    fn new() -> Self {
        let scene = catalog().iter().position(|s| s.id == "thinking").unwrap();
        let poses = catalog()[scene].first();
        Self {
            scene,
            poses,
            from: poses,
            shapes: std::array::from_fn(|i| shape_weights(poses[i].shape)),
            from_shapes: std::array::from_fn(|i| shape_weights(poses[i].shape)),
            started: 0.,
            morph_seconds: 0.,
            morph_completed: true,
            looping: false,
            awake: false,
        }
    }
    fn sample(&mut self, now: f64) {
        if !self.awake {
            return;
        }
        let elapsed = (now - self.started).max(0.);
        let scene = &catalog()[self.scene];
        if elapsed < self.morph_seconds {
            let t = bezier((elapsed / self.morph_seconds) as f32, 0.2, 0.72, 0.24, 1.);
            let target = scene.first();
            self.poses = std::array::from_fn(|i| self.from[i].blend(target[i], t));
            self.shapes = std::array::from_fn(|i| {
                let target = shape_weights(target[i].shape);
                std::array::from_fn(|j| {
                    self.from_shapes[i][j] + (target[j] - self.from_shapes[i][j]) * t
                })
            });
        } else if scene.id != "listening" || !self.morph_completed {
            self.poses = scene.sample(elapsed - self.morph_seconds, self.looping);
            self.shapes = std::array::from_fn(|i| shape_weights(self.poses[i].shape));
            self.morph_completed = true;
        }
    }
    fn set_scene(&mut self, id: &str, animate: bool, looping: bool, now: f64) -> bool {
        let Some(index) = catalog().iter().position(|s| s.id == id) else {
            return false;
        };
        if self.scene == index && self.looping == looping {
            return false;
        }
        self.sample(now); // Interrupted morphs restart at the displayed pose.
        self.from = self.poses;
        self.from_shapes = self.shapes;
        self.scene = index;
        self.looping = looping;
        self.started = now;
        self.morph_completed = false;
        self.morph_seconds = if animate && self.awake {
            catalog()[index].transition
        } else {
            0.
        };
        if self.morph_seconds == 0. {
            self.poses = catalog()[index].first();
            self.shapes = std::array::from_fn(|i| shape_weights(self.poses[i].shape));
        }
        true
    }
    fn moving(&self, now: f64) -> bool {
        self.awake
            && (now - self.started < self.morph_seconds
                || self.looping && catalog()[self.scene].duration > 0.)
    }
    fn set_awake(&mut self, awake: bool, now: f64) -> bool {
        if self.awake == awake {
            return false;
        }
        self.sample(now);
        self.awake = awake;
        // Hidden indicators never accrue frame callbacks; waking continues from
        // the frozen visible pose rather than jumping to a wall-clock phase.
        self.from = self.poses;
        self.from_shapes = self.shapes;
        self.started = now;
        self.morph_completed = false;
        self.morph_seconds = if awake {
            catalog()[self.scene].transition
        } else {
            0.
        };
        true
    }
}

#[derive(Clone, Copy, Default)]
struct Mark {
    path: Option<&'static str>,
    opacity: f32,
    from: f32,
}
struct Marks {
    slots: [Mark; 2],
    target: Option<&'static str>,
    started: f64,
    duration: f64,
}
impl Marks {
    fn new() -> Self {
        Self {
            slots: [Mark::default(); 2],
            target: None,
            started: 0.,
            duration: 0.,
        }
    }
    fn sample(&mut self, now: f64) {
        let progress = if self.duration > 0. {
            ((now - self.started) / self.duration).clamp(0., 1.) as f32
        } else {
            1.
        };
        let t = bezier(progress, 0.2, 0.72, 0.24, 1.);
        for slot in &mut self.slots {
            let target = if slot.path.is_some() && slot.path == self.target {
                1.
            } else {
                0.
            };
            slot.opacity = slot.from + (target - slot.from) * t;
            if progress >= 1. && target == 0. {
                *slot = Mark::default();
            }
        }
    }
    fn set(&mut self, path: Option<&'static str>, animate: bool, now: f64) -> bool {
        if self.target == path {
            return false;
        }
        self.sample(now);
        self.target = path;
        if let Some(path) = path
            && !self.slots.iter().any(|slot| slot.path == Some(path))
        {
            let index = if self.slots[0].opacity <= self.slots[1].opacity {
                0
            } else {
                1
            };
            self.slots[index] = Mark {
                path: Some(path),
                opacity: 0.,
                from: 0.,
            };
        }
        for slot in &mut self.slots {
            slot.from = slot.opacity;
        }
        self.started = now;
        self.duration = if animate { 0.42 } else { 0. };
        self.sample(now);
        true
    }
    fn moving(&self, now: f64) -> bool {
        now - self.started < self.duration
    }
    fn scene_opacity(&self) -> f32 {
        (1. - self.slots.iter().map(|s| s.opacity).sum::<f32>()).clamp(0., 1.)
    }
}

pub(crate) struct KaiIndicator {
    player: Player,
    marks: Marks,
    epoch: Instant,
    levels: Option<Arc<[AtomicU32; 4]>>,
    last_voice: f64,
    level_tick: Option<Task<()>>,
    #[cfg(test)]
    last_paint: std::rc::Rc<std::cell::Cell<(gpui::Bounds<Pixels>, usize)>>,
}
impl KaiIndicator {
    pub(crate) fn new(_: &mut Context<Self>) -> Self {
        Self {
            player: Player::new(),
            marks: Marks::new(),
            epoch: Instant::now(),
            levels: None,
            last_voice: 0.,
            level_tick: None,
            #[cfg(test)]
            last_paint: Default::default(),
        }
    }
    pub(crate) fn set_scene(
        &mut self,
        id: &str,
        animate: bool,
        loop_active: bool,
        cx: &mut Context<Self>,
    ) {
        if self
            .player
            .set_scene(id, animate, loop_active, self.epoch.elapsed().as_secs_f64())
        {
            cx.notify();
        }
    }
    pub(crate) fn set_mark(
        &mut self,
        path: Option<&'static str>,
        animate: bool,
        cx: &mut Context<Self>,
    ) {
        let now = self.epoch.elapsed().as_secs_f64();
        if self.marks.set(path, animate && self.player.awake, now) {
            cx.notify();
        }
    }
    pub(crate) fn set_levels(
        &mut self,
        levels: Option<Arc<[AtomicU32; 4]>>,
        cx: &mut Context<Self>,
    ) {
        let same = match (&self.levels, &levels) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        if same {
            return;
        }
        self.levels = levels;
        if self.levels.is_none() && catalog()[self.player.scene].id == "listening" {
            self.player.poses = catalog()[self.player.scene].first();
            self.player.shapes = std::array::from_fn(|i| shape_weights(self.player.poses[i].shape));
        }
        self.last_voice = self.epoch.elapsed().as_secs_f64();
        self.level_tick = None;
        cx.notify();
    }
    fn listening(&self) -> bool {
        self.player.awake
            && self.marks.target.is_none()
            && catalog()[self.player.scene].id == "listening"
            && self.levels.is_some()
    }
    pub(crate) fn set_awake(&mut self, awake: bool, cx: &mut Context<Self>) {
        let now = self.epoch.elapsed().as_secs_f64();
        if self.player.awake && !awake {
            self.marks.sample(now);
        }
        if self.player.set_awake(awake, now) {
            self.last_voice = now;
            self.level_tick = None;
            for slot in &mut self.marks.slots {
                slot.from = slot.opacity;
            }
            self.marks.started = now;
            self.marks.duration = if awake { 0.42 } else { 0. };
            cx.notify();
        }
    }
    fn moving(&self, now: f64) -> bool {
        self.player.awake && (self.listening() || self.marks.moving(now) || self.player.moving(now))
    }
    #[cfg(test)]
    pub(crate) fn current_scene(&self) -> &str {
        &catalog()[self.player.scene].id
    }
    #[cfg(test)]
    pub(crate) fn needs_frames(&self) -> bool {
        self.moving(self.epoch.elapsed().as_secs_f64())
    }
    #[cfg(test)]
    pub(crate) fn painted_geometry(&self) -> (gpui::Bounds<Pixels>, usize) {
        self.last_paint.get()
    }
    #[cfg(test)]
    pub(crate) fn advance_clock(&mut self, seconds: f64, cx: &mut Context<Self>) {
        self.epoch -= Duration::from_secs_f64(seconds);
        cx.notify();
    }
    #[cfg(test)]
    pub(crate) fn geometry(&self) -> [[f32; 4]; TRACKS] {
        self.player.poses.map(|p| [p.x, p.y, p.height, p.opacity])
    }
}
impl Render for KaiIndicator {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.epoch.elapsed().as_secs_f64();
        self.player.sample(now);
        if self.listening() && now - self.player.started >= self.player.morph_seconds {
            let dt = (now - self.last_voice).clamp(0., 0.05) as f32;
            if let Some(levels) = &self.levels {
                apply_levels(
                    &mut self.player.poses,
                    std::array::from_fn(|i| f32::from_bits(levels[i].load(Ordering::Relaxed))),
                    dt,
                );
            }
        }
        self.last_voice = now;
        if self.player.awake {
            self.marks.sample(now);
        }
        let only_meter = self.listening()
            && now - self.player.started >= self.player.morph_seconds
            && !self.marks.moving(now);
        if only_meter {
            if self.level_tick.is_none() {
                self.level_tick = Some(cx.spawn(async move |this, cx| {
                    Timer::after(Duration::from_secs_f64(1. / 30.)).await;
                    let _ = this.update(cx, |this, cx| {
                        this.level_tick = None;
                        if this.listening() {
                            cx.notify();
                        }
                    });
                }));
            }
        } else if self.moving(now) {
            self.level_tick = None;
            let entity = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                if let Some(entity) = entity.upgrade() {
                    cx.notify(entity.entity_id());
                }
            });
        }
        if !self.moving(now) {
            self.level_tick = None;
        }
        let poses = self.player.poses;
        let shapes = self.player.shapes;
        let scene_opacity = self.marks.scene_opacity();
        let (brand_opacity, brand_scale) = brand_breath(
            now - self.player.started,
            self.player.awake && self.player.looping,
        );
        #[cfg(test)]
        let last_paint = self.last_paint.clone();
        div()
            .w(px(28.))
            .h(px(18.))
            .flex_none()
            .relative()
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        #[cfg(test)]
                        let mut painted_paths = 0;
                        let scale = (f32::from(bounds.size.width) / 28.)
                            .min(f32::from(bounds.size.height) / 18.);
                        let origin = point(
                            bounds.origin.x
                                + px((f32::from(bounds.size.width) - 28. * scale) * 0.5),
                            bounds.origin.y
                                + px((f32::from(bounds.size.height) - 18. * scale) * 0.5),
                        );
                        for (pose, weights) in poses.into_iter().zip(shapes) {
                            for (shape, weight) in [
                                Shape::Capsule,
                                Shape::Ellipse,
                                Shape::Question,
                                Shape::HourglassLeft,
                                Shape::HourglassRight,
                            ]
                            .into_iter()
                            .zip(weights)
                            {
                                if weight <= 0.001 {
                                    continue;
                                }
                                let pose = Pose { shape, ..pose };
                                let opacity = pose.opacity * scene_opacity * weight;
                                if opacity <= 0.001 {
                                    continue;
                                }
                                let color = silver_color(pose.x, pose.y, opacity);
                                if let Ok(path) = pose_path(pose, origin, scale).build() {
                                    window.paint_path(path, color);
                                    #[cfg(test)]
                                    {
                                        painted_paths += 1;
                                    }
                                }
                                // GPUI's default stroke caps are butt caps. Swift uses round
                                // caps, reproduced with two tiny filled endpoint discs.
                                let endpoints = match pose.shape {
                                    Shape::Question => Some([
                                        (pose.width * 0.1, pose.height * 0.72),
                                        (pose.width * 0.5, pose.height * 0.22),
                                    ]),
                                    Shape::HourglassLeft => Some([(0., pose.height), (0., 0.)]),
                                    Shape::HourglassRight => {
                                        Some([(pose.width, pose.height), (pose.width, 0.)])
                                    }
                                    _ => None,
                                };
                                if let Some(endpoints) = endpoints {
                                    for (x, y) in endpoints {
                                        let (sin, cos) = pose.rotation.sin_cos();
                                        let dx = x - pose.width / 2.;
                                        let dy = y - pose.height / 2.;
                                        let cap = Pose {
                                            x: pose.x + dx * cos - dy * sin,
                                            y: pose.y + dx * sin + dy * cos,
                                            width: pose.line_width,
                                            height: pose.line_width,
                                            radius: pose.line_width / 2.,
                                            rotation: 0.,
                                            shape: Shape::Capsule,
                                            ..pose
                                        };
                                        if let Ok(path) = pose_path(cap, origin, scale).build() {
                                            window.paint_path(path, color);
                                            #[cfg(test)]
                                            {
                                                painted_paths += 1;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        #[cfg(test)]
                        last_paint.set((bounds, painted_paths));
                    },
                )
                .size_full(),
            )
            .children(self.marks.slots.iter().filter_map(|mark| {
                let path = mark.path?;
                if mark.opacity <= 0.001 {
                    return None;
                }
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(mark.opacity * brand_opacity)
                        .child(
                            svg()
                                .path(path)
                                .size(px(18. * (0.85 + 0.15 * mark.opacity) * brand_scale))
                                .text_color(rgb(0xebebeb)),
                        ),
                )
            }))
    }
}

fn brand_breath(elapsed: f64, active: bool) -> (f32, f32) {
    if !active {
        return (1., 1.);
    }
    let wave = (0.5 + 0.5 * (elapsed.rem_euclid(1.8) / 1.8 * std::f64::consts::TAU).cos()) as f32;
    (0.88 + 0.12 * wave, 0.96 + 0.04 * wave)
}

// Sample Kai's shared 3-stop silver gradient at each tiny primitive center.
// Keeping the shared 28×18 coordinates avoids repeating a separate gradient
// inside every bar and retains all three exact exported palette stops.
fn silver_color(x: f32, y: f32, opacity: f32) -> gpui::Rgba {
    let t = ((x * 28. + (18. - y) * 18.) / (28. * 28. + 18. * 18.)).clamp(0., 1.);
    let (a, b, t) = if t <= 0.5 {
        ([250., 250., 250.], [213., 214., 216.], t * 2.)
    } else {
        ([213., 214., 216.], [138., 141., 146.], (t - 0.5) * 2.)
    };
    let c: [u32; 3] = std::array::from_fn(|i| (a[i] + (b[i] - a[i]) * t).round() as u32);
    rgba(c[0] << 24 | c[1] << 16 | c[2] << 8 | (opacity.clamp(0., 1.) * 255.).round() as u32)
}

fn shape_weights(shape: Shape) -> [f32; 5] {
    let mut weights = [0.; 5];
    weights[shape as usize] = 1.;
    weights
}

fn apply_levels(poses: &mut [Pose; TRACKS], levels: [f32; 4], dt: f32) {
    let gains = [0.91, 0.94, 0.97, 1.];
    for i in 0..4 {
        let value = if levels[i].is_finite() {
            levels[i].clamp(0., 1.)
        } else {
            0.
        };
        let target = 2.5 + 14.5 * (value * gains[i]).powf(0.55);
        let rate = if target > poses[i].height { 22. } else { 9. };
        poses[i].height += (target - poses[i].height) * (1. - (-dt.clamp(0., 0.05) * rate).exp());
        poses[i].x = 3.5 + 7. * i as f32;
        poses[i].y = 9.;
        poses[i].width = 3.;
        poses[i].radius = 1.5;
        poses[i].rotation = 0.;
        poses[i].opacity = 1.;
    }
    poses[4..].fill(Pose::HIDDEN);
}

fn pose_path(p: Pose, origin: Point<Pixels>, scale: f32) -> PathBuilder {
    let transform = |x: f32, y: f32| {
        let dx = x - p.width / 2.;
        let dy = y - p.height / 2.;
        let (s, c) = p.rotation.sin_cos();
        point(
            origin.x + px((p.x + dx * c - dy * s) * scale),
            origin.y + px((18. - p.y - dx * s - dy * c) * scale),
        )
    };
    let mut path = if p.shape == Shape::Capsule {
        PathBuilder::fill()
    } else {
        PathBuilder::stroke(px(p.line_width * scale))
    };
    let w = p.width;
    let h = p.height;
    match p.shape {
        Shape::Capsule | Shape::Ellipse => {
            let inset = if p.shape == Shape::Ellipse {
                p.line_width * 0.5
            } else {
                0.
            };
            let (left, bottom, right, top) = (
                inset,
                inset,
                (w - inset).max(inset + 0.1),
                (h - inset).max(inset + 0.1),
            );
            let rx = if p.shape == Shape::Ellipse {
                (right - left) * 0.5
            } else {
                p.radius.min((right - left) * 0.5).min((top - bottom) * 0.5)
            };
            let ry = if p.shape == Shape::Ellipse {
                (top - bottom) * 0.5
            } else {
                rx
            };
            let k = 0.552_284_8;
            path.move_to(transform(left + rx, bottom));
            path.line_to(transform(right - rx, bottom));
            path.cubic_bezier_to(
                transform(right, bottom + ry),
                transform(right - rx + rx * k, bottom),
                transform(right, bottom + ry - ry * k),
            );
            path.line_to(transform(right, top - ry));
            path.cubic_bezier_to(
                transform(right - rx, top),
                transform(right, top - ry + ry * k),
                transform(right - rx + rx * k, top),
            );
            path.line_to(transform(left + rx, top));
            path.cubic_bezier_to(
                transform(left, top - ry),
                transform(left + rx - rx * k, top),
                transform(left, top - ry + ry * k),
            );
            path.line_to(transform(left, bottom + ry));
            path.cubic_bezier_to(
                transform(left + rx, bottom),
                transform(left, bottom + ry - ry * k),
                transform(left + rx - rx * k, bottom),
            );
            path.close();
        }
        Shape::Question => {
            path.move_to(transform(w * 0.1, h * 0.72));
            path.cubic_bezier_to(
                transform(w * 0.52, h * 0.96),
                transform(w * 0.16, h),
                transform(w * 0.46, h),
            );
            path.cubic_bezier_to(
                transform(w * 0.88, h * 0.66),
                transform(w * 0.78, h * 0.94),
                transform(w * 0.9, h * 0.82),
            );
            path.cubic_bezier_to(
                transform(w * 0.5, h * 0.38),
                transform(w * 0.86, h * 0.5),
                transform(w * 0.53, h * 0.54),
            );
            path.line_to(transform(w * 0.5, h * 0.22));
        }
        Shape::HourglassLeft => {
            path.move_to(transform(0., h));
            path.cubic_bezier_to(
                transform(w, h / 2.),
                transform(w * 0.08, h * 0.78),
                transform(w * 0.92, h * 0.62),
            );
            path.cubic_bezier_to(
                transform(0., 0.),
                transform(w * 0.92, h * 0.38),
                transform(w * 0.08, h * 0.22),
            );
        }
        Shape::HourglassRight => {
            path.move_to(transform(w, h));
            path.cubic_bezier_to(
                transform(0., h / 2.),
                transform(w * 0.92, h * 0.78),
                transform(w * 0.08, h * 0.62),
            );
            path.cubic_bezier_to(
                transform(w, 0.),
                transform(w * 0.08, h * 0.38),
                transform(w * 0.92, h * 0.22),
            );
        }
    }
    path
}

const FILE_CLOSED: [[f32; 2]; 9] = [
    [7., 2.],
    [21., 2.],
    [21., 12.],
    [17., 16.],
    [7., 16.],
    [7., 2.],
    [17., 16.],
    [17., 12.],
    [21., 12.],
];
const FILE_OPEN: [[f32; 2]; 9] = [
    [4., 4.],
    [24., 4.],
    [22., 13.],
    [12., 13.],
    [9., 16.],
    [4., 16.],
    [5., 13.],
    [20., 13.],
    [22., 11.],
];
struct FilePlayer {
    open: bool,
    awake: bool,
    vertices: [[f32; 2]; 9],
    from: [[f32; 2]; 9],
    started: f64,
    duration: f64,
}
impl FilePlayer {
    fn new() -> Self {
        Self {
            open: false,
            awake: false,
            vertices: FILE_CLOSED,
            from: FILE_CLOSED,
            started: 0.,
            duration: 0.,
        }
    }
    fn target(&self) -> [[f32; 2]; 9] {
        if self.open { FILE_OPEN } else { FILE_CLOSED }
    }
    fn moving(&self, now: f64) -> bool {
        self.awake && self.duration > 0. && now - self.started < self.duration
    }
    fn sample(&mut self, now: f64) {
        if !self.awake {
            return;
        }
        let target = self.target();
        if self.duration == 0. || now - self.started >= self.duration {
            self.vertices = target;
            return;
        }
        let t = bezier(
            ((now - self.started) / self.duration).clamp(0., 1.) as f32,
            0.16,
            0.7,
            0.25,
            1.,
        );
        self.vertices = std::array::from_fn(|i| {
            std::array::from_fn(|j| self.from[i][j] + (target[i][j] - self.from[i][j]) * t)
        });
    }
    fn set_open(&mut self, open: bool, animate: bool, now: f64) -> bool {
        if self.open == open {
            if !animate && self.vertices != self.target() {
                self.vertices = self.target();
                self.duration = 0.;
                return true;
            }
            return false;
        }
        self.sample(now);
        self.from = self.vertices;
        self.open = open;
        self.started = now;
        self.duration = if animate && self.awake { 0.42 } else { 0. };
        if self.duration == 0. {
            self.vertices = self.target();
        }
        true
    }
    fn set_awake(&mut self, awake: bool, now: f64) -> bool {
        if self.awake == awake {
            return false;
        }
        self.sample(now);
        self.awake = awake;
        self.from = self.vertices;
        self.started = now;
        self.duration = if awake && self.vertices != self.target() {
            0.42
        } else {
            0.
        };
        true
    }
}
/// Kai's file-open glyph morph. It animates finitely and owns no background loop.
pub(crate) struct KaiFileGlyph {
    player: FilePlayer,
    epoch: Instant,
}
impl KaiFileGlyph {
    pub(crate) fn new(_: &mut Context<Self>) -> Self {
        Self {
            player: FilePlayer::new(),
            epoch: Instant::now(),
        }
    }
    pub(crate) fn set_open(&mut self, open: bool, animate: bool, cx: &mut Context<Self>) {
        if self
            .player
            .set_open(open, animate, self.epoch.elapsed().as_secs_f64())
        {
            cx.notify();
        }
    }
    pub(crate) fn set_awake(&mut self, awake: bool, cx: &mut Context<Self>) {
        if self
            .player
            .set_awake(awake, self.epoch.elapsed().as_secs_f64())
        {
            cx.notify();
        }
    }
}
impl Render for KaiFileGlyph {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.epoch.elapsed().as_secs_f64();
        self.player.sample(now);
        if self.player.moving(now) {
            let entity = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                if let Some(entity) = entity.upgrade() {
                    cx.notify(entity.entity_id());
                }
            });
        }
        let vertices = self.player.vertices;
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let scale =
                    (f32::from(bounds.size.width) / 28.).min(f32::from(bounds.size.height) / 18.);
                let origin = point(
                    bounds.origin.x + px((f32::from(bounds.size.width) - 28. * scale) * 0.5),
                    bounds.origin.y + px((f32::from(bounds.size.height) - 18. * scale) * 0.5),
                );
                let point_for = |v: [f32; 2]| {
                    point(
                        origin.x + px(v[0] * scale),
                        origin.y + px((18. - v[1]) * scale),
                    )
                };
                // Independent straight segments plus circular joins exactly preserve
                // round caps/corners without relying on GPUI's miter stroke defaults.
                let mut path = PathBuilder::stroke(px(1.6 * scale));
                for (a, b) in [
                    (0, 1),
                    (1, 2),
                    (2, 3),
                    (3, 4),
                    (4, 5),
                    (5, 0),
                    (6, 7),
                    (7, 8),
                ] {
                    path.move_to(point_for(vertices[a]));
                    path.line_to(point_for(vertices[b]));
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, gpui::rgb(0xffffff));
                }
                for [x, y] in vertices {
                    let cap = Pose {
                        x,
                        y,
                        width: 1.6,
                        height: 1.6,
                        radius: 0.8,
                        rotation: 0.,
                        opacity: 1.,
                        shape: Shape::Capsule,
                        line_width: 1.6,
                    };
                    if let Ok(path) = pose_path(cap, origin, scale).build() {
                        window.paint_path(path, gpui::rgb(0xffffff));
                    }
                }
            },
        )
        .w(px(20.))
        .h(px(14.))
        .flex_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_catalog_contains_all_native_scenes_and_geometry() {
        assert_eq!(catalog().len(), 24);
        for scene in catalog() {
            for pose in scene.first() {
                assert!(pose.valid());
                assert!(pose_path(pose, point(px(0.), px(0.)), 1.).build().is_ok());
            }
        }
        assert!(catalog().iter().any(|s| s.id == "question"));
        assert!(catalog().iter().any(|s| s.id == "waiting"));
    }
    #[test]
    fn easing_inverts_x_and_has_exact_endpoints() {
        assert_eq!(bezier(0., 0.2, 0.72, 0.24, 1.), 0.);
        assert_eq!(bezier(1., 0.2, 0.72, 0.24, 1.), 1.);
        assert!((bezier(0.5, 0.42, 0., 0.58, 1.) - 0.5).abs() < 0.0001);
        assert!(bezier(0.5, 0.2, 0.72, 0.24, 1.) > 0.9);
    }
    #[test]
    fn interrupted_morph_starts_at_current_presentation() {
        let mut p = Player::new();
        p.set_awake(true, 0.);
        p.set_scene("terminal", true, true, 0.);
        p.sample(0.2);
        let before = p.poses;
        p.set_scene("success", true, false, 0.2);
        p.sample(0.2);
        for (a, b) in before.iter().zip(p.poses) {
            assert_eq!(a.x, b.x);
            assert_eq!(a.y, b.y);
            assert_eq!(a.width, b.width);
            assert_eq!(a.opacity, b.opacity);
        }
        p.sample(1.);
        assert_eq!(p.poses, catalog()[p.scene].first());
        assert!(!p.moving(1.));
    }
    #[test]
    fn sleeping_and_settled_scenes_stop_frames_and_repeated_updates_do_not_restart() {
        let mut p = Player::new();
        assert!(!p.moving(0.));
        p.set_awake(true, 0.);
        p.set_scene("search", true, true, 0.);
        assert!(p.moving(2.));
        p.sample(0.3);
        let before = p.poses;
        p.set_awake(false, 0.3);
        p.sample(200.);
        assert_eq!(p.poses, before);
        assert!(!p.moving(200.));
        p.set_awake(true, 201.);
        assert!(p.moving(201.));
        let started = p.started;
        assert!(!p.set_scene("search", true, true, 202.));
        assert_eq!(p.started, started);
        p.set_scene("success", true, false, 202.);
        assert!(p.moving(202.));
        assert!(!p.moving(203.));
    }
    #[test]
    fn phases_hold_first_pose_then_sample_exact_keys() {
        let scene = catalog().iter().find(|s| s.id == "thinking").unwrap();
        let track = scene.tracks[1];
        assert!(track.phase > 0.);
        assert_eq!(scene.sample(track.phase * 0.5, true)[1], track.poses[0]);
        let mid = scene.sample(track.phase + scene.duration * 0.5, true)[1];
        assert!((mid.y - track.poses[1].y).abs() < 0.0001);
        assert_eq!(
            scene.sample(track.phase + scene.duration, true)[1],
            track.poses[0]
        );
    }
    #[test]
    fn busy_brand_animates_but_idle_hidden_and_reduced_motion_do_not() {
        let mut indicator = KaiIndicator {
            player: Player::new(),
            marks: Marks::new(),
            epoch: Instant::now(),
            levels: None,
            last_voice: 0.,
            level_tick: None,
            last_paint: Default::default(),
        };
        indicator.player.set_awake(true, 0.);
        indicator.player.set_scene("tool", false, true, 0.);
        indicator
            .marks
            .set(Some("kai/integrations/github.svg"), false, 0.);
        assert!(indicator.moving(10.));
        assert_eq!(indicator.current_scene(), "tool");
        assert!(indicator.needs_frames());
        assert_ne!(brand_breath(0., true), brand_breath(0.9, true));
        // Caller disables looping for idle and system Reduce Motion.
        indicator.player.set_scene("tool", false, false, 10.);
        assert!(!indicator.moving(11.));
        assert_eq!(brand_breath(0.9, false), (1., 1.));
        indicator.player.set_scene("tool", false, true, 12.);
        indicator.player.set_awake(false, 12.);
        assert!(!indicator.moving(13.));
    }
    #[test]
    fn unlike_shapes_crossfade_continuously_even_when_interrupted() {
        let mut p = Player::new();
        p.set_awake(true, 0.);
        p.set_scene("question", false, false, 0.);
        let before = p.shapes;
        p.set_scene("waiting", true, false, 0.);
        p.sample(0.);
        assert_eq!(p.shapes, before);
        p.sample(0.3);
        let middle = p.shapes;
        assert!(
            middle
                .iter()
                .any(|w| w.iter().filter(|v| **v > 0.001).count() > 1)
        );
        for weights in middle {
            assert!((weights.iter().sum::<f32>() - 1.).abs() < 0.0001);
        }
        p.set_scene("terminal", true, false, 0.3);
        p.sample(0.3);
        assert_eq!(p.shapes, middle);
        p.sample(2.);
        assert_eq!(
            p.shapes,
            std::array::from_fn(|i| shape_weights(p.poses[i].shape))
        );
    }
    #[test]
    fn all_scene_loops_preserve_finite_bounded_native_geometry() {
        for scene in catalog() {
            for t in [0., 0.03, 0.4, 1., 10., 1000.] {
                for pose in scene.sample(t, true) {
                    assert!(pose.valid());
                    assert!(pose_path(pose, point(px(0.), px(0.)), 1.).build().is_ok());
                }
            }
        }
    }
    #[test]
    fn integration_marks_crossfade_without_unbounded_slots_or_restart() {
        let mut marks = Marks::new();
        assert!(marks.set(Some("kai/integrations/github.svg"), true, 0.));
        marks.sample(0.21);
        let first = marks
            .slots
            .iter()
            .find(|s| s.path == Some("kai/integrations/github.svg"))
            .unwrap()
            .opacity;
        marks.set(Some("kai/integrations/notion.svg"), true, 0.21);
        marks.sample(0.21);
        assert_eq!(
            marks
                .slots
                .iter()
                .find(|s| s.path == Some("kai/integrations/github.svg"))
                .unwrap()
                .opacity,
            first
        );
        marks.sample(1.);
        assert!(!marks.moving(1.));
        assert_eq!(marks.scene_opacity(), 0.);
        assert!(!marks.set(Some("kai/integrations/notion.svg"), true, 1.));
        marks.set(None, true, 1.);
        marks.sample(2.);
        assert_eq!(marks.scene_opacity(), 1.);
        assert!(marks.slots.iter().all(|s| s.path.is_none()));
        for i in 0..1000 {
            marks.set(
                Some(if i % 2 == 0 { "github" } else { "notion" }),
                true,
                2. + i as f64 * 0.001,
            );
        }
        assert_eq!(marks.slots.len(), 2);
    }
    #[test]
    fn real_voice_levels_use_native_attack_release_and_clamp_invalid_samples() {
        let mut poses = catalog()
            .iter()
            .find(|s| s.id == "listening")
            .unwrap()
            .first();
        let initial = poses[0].height;
        apply_levels(&mut poses, [1.; 4], 1. / 30.);
        assert!(poses[0].height > initial);
        assert!(poses[3].height > poses[0].height);
        for _ in 0..100 {
            apply_levels(&mut poses, [0., f32::NAN, f32::INFINITY, -1.], 1. / 30.);
        }
        assert!((poses[0].height - 2.5).abs() < 0.001);
        assert!(poses.iter().all(|p| p.valid()));
        assert!(poses[4..].iter().all(|p| p.opacity == 0.));
        let before = poses;
        apply_levels(&mut poses, [1.; 4], 0.);
        assert_eq!(before, poses);
    }
    #[test]
    fn listening_finishes_morph_exactly_before_retaining_smoothed_heights() {
        let mut p = Player::new();
        p.set_awake(true, 0.);
        p.set_scene("listening", true, false, 0.);
        p.sample(0.1);
        p.sample(2.);
        assert_eq!(p.poses, catalog()[p.scene].first());
        apply_levels(&mut p.poses, [1.; 4], 1. / 30.);
        let held = p.poses;
        p.sample(3.);
        assert_eq!(held, p.poses);
    }
    #[test]
    fn file_glyph_vertices_morph_exactly_and_interrupt_without_a_jump() {
        let mut p = FilePlayer::new();
        assert_eq!(p.vertices, FILE_CLOSED);
        p.set_awake(true, 0.);
        assert!(!p.moving(0.));
        p.set_open(true, true, 0.);
        p.sample(0.);
        assert_eq!(p.vertices, FILE_CLOSED);
        p.sample(0.2);
        let shown = p.vertices;
        assert_ne!(shown, FILE_CLOSED);
        assert_ne!(shown, FILE_OPEN);
        p.set_open(false, true, 0.2);
        p.sample(0.2);
        assert_eq!(p.vertices, shown);
        p.sample(1.);
        assert_eq!(p.vertices, FILE_CLOSED);
        assert!(!p.moving(1.));
        p.set_open(true, true, 1.);
        p.sample(1.43);
        assert_eq!(p.vertices, FILE_OPEN);
        assert!(!p.moving(1.43));
    }
    #[test]
    fn file_glyph_offscreen_freezes_and_reduced_motion_settles_without_frames() {
        let mut p = FilePlayer::new();
        p.set_awake(true, 0.);
        p.set_open(true, true, 0.);
        p.set_awake(false, 0.1);
        let frozen = p.vertices;
        p.sample(100.);
        assert_eq!(p.vertices, frozen);
        assert!(!p.moving(100.));
        p.set_awake(true, 100.);
        p.sample(100.);
        assert_eq!(p.vertices, frozen);
        assert!(p.moving(100.));
        p.set_awake(false, 100.1);
        assert!(p.set_open(true, false, 100.1));
        assert_eq!(p.vertices, FILE_OPEN);
        assert!(!p.moving(100.1));
        p.set_open(false, false, 100.);
        assert_eq!(p.vertices, FILE_CLOSED);
        assert!(!p.moving(100.));
        assert!(!p.set_open(false, true, 101.));
        p.set_awake(false, 102.);
        p.set_open(true, true, 102.);
        assert_eq!(p.vertices, FILE_OPEN);
        assert!(!p.moving(102.));
    }
    #[test]
    fn malformed_catalog_is_rejected_without_unbounded_playback() {
        let data = include_str!("../assets/kai/scene_catalog.json");
        assert!(
            parse_catalog(&data.replacen("\"formatVersion\" : 1", "\"formatVersion\" : 999", 1))
                .is_err()
        );
        let mut raw: serde_json::Value = serde_json::from_str(data).unwrap();
        raw["scenes"][0]["tracks"][0]["keyTimes"][1] = serde_json::json!(0.);
        assert!(parse_catalog(&raw.to_string()).is_err());
    }
}
