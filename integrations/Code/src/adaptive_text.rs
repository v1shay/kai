//! Text-only contrast: no tint, background, alpha, blur or geometry outputs.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    pub settle_ms: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            settle_ms: 500,
        }
    }
}
impl Settings {
    pub fn normalized(&self) -> Self {
        Self {
            enabled: self.enabled,
            settle_ms: self.settle_ms.clamp(200, 1500),
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct Profile {
    pub id: String,
    pub role: String,
    pub generation: u64,
    pub pixels: Vec<[f32; 3]>,
    #[serde(default)]
    pub rect: [f32; 4],
    #[serde(default)]
    pub moving: bool,
}
pub fn channels(rgb: u32) -> [f32; 3] {
    [
        ((rgb >> 16) & 255) as f32 / 255.,
        ((rgb >> 8) & 255) as f32 / 255.,
        (rgb & 255) as f32 / 255.,
    ]
}
pub fn composite(front: [f32; 3], alpha: f32, back: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| front[i] * alpha + back[i] * (1. - alpha))
}
pub fn luminance(c: [f32; 3]) -> f32 {
    let linear = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c[0]) + 0.7152 * linear(c[1]) + 0.0722 * linear(c[2])
}
pub fn ratio(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
pub fn valid(p: &Profile) -> bool {
    p.id.len() <= 160
        && p.role.len() <= 32
        && !p.pixels.is_empty()
        && p.pixels.len() <= 256
        && p.pixels
            .iter()
            .flatten()
            .all(|v| v.is_finite() && (0. ..=1.).contains(v))
        && p.rect.iter().all(|v| v.is_finite())
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub color: [f32; 3],
    pub range: (f32, f32),
}
impl Palette {
    pub fn filter(&self, old: gpui::Hsla) -> gpui::Hsla {
        let c = gpui::Rgba::from(old);
        let source = [c.r, c.g, c.b];
        let spread = source.iter().copied().fold(0f32, f32::max)
            - source.iter().copied().fold(1f32, f32::min);
        let result = if spread < 0.10 {
            self.color
        } else {
            let body = luminance(self.color);
            let achievable = if body >= self.range.0 && body <= self.range.1 {
                1.
            } else {
                ratio(body, self.range.0).min(ratio(body, self.range.1))
            };
            let target = achievable.min(4.5);
            let good = |rgb: [f32; 3]| {
                let l = luminance(rgb);
                (l < self.range.0 || l > self.range.1)
                    && ratio(l, self.range.0).min(ratio(l, self.range.1)) >= target
            };
            if good(source) {
                source
            } else {
                let mut low = 0.;
                let mut high = 1.;
                for _ in 0..12 {
                    let mid = (low + high) * 0.5;
                    let next =
                        std::array::from_fn(|i| source[i] * (1. - mid) + self.color[i] * mid);
                    if good(next) { high = mid } else { low = mid }
                }
                std::array::from_fn(|i| source[i] * (1. - high) + self.color[i] * high)
            }
        };
        gpui::Rgba {
            r: result[0],
            g: result[1],
            b: result[2],
            a: c.a,
        }
        .into()
    }
    pub fn rgb(&self) -> u32 {
        ((self.color[0] * 255.).round() as u32) << 16
            | ((self.color[1] * 255.).round() as u32) << 8
            | (self.color[2] * 255.).round() as u32
    }
}
pub fn range(pixels: &[[f32; 3]]) -> (f32, f32) {
    pixels
        .iter()
        .map(|p| luminance(*p))
        .fold((1f32, 0f32), |(low, high), v| (low.min(v), high.max(v)))
}
pub fn choose(range: (f32, f32), previous: Option<bool>) -> bool {
    let score = |light: bool| {
        let ink = luminance(channels(if light { 0xf9fafb } else { 0x101318 }));
        if ink >= range.0 && ink <= range.1 {
            1.
        } else {
            ratio(ink, range.0).min(ratio(ink, range.1))
        }
    };
    let light = score(true);
    let dark = score(false);
    if let Some(old) = previous {
        if (light - dark).abs() < 0.45 {
            return old;
        }
    }
    light >= dark
}
struct Track {
    light: bool,
    candidate: Option<(bool, Instant)>,
    from: [f32; 3],
    to: [f32; 3],
    started: Instant,
    range: (f32, f32),
}
#[derive(Default)]
pub struct State {
    pub status: String,
    pub profiles: HashMap<String, Profile>,
    tracks: HashMap<String, Track>,
}
impl State {
    pub fn update(
        &mut self,
        p: Profile,
        pixels: &[[f32; 3]],
        settings: &Settings,
        now: Instant,
        animated: bool,
    ) -> bool {
        if !valid(&p)
            || self
                .profiles
                .get(&p.id)
                .is_some_and(|old| old.generation > p.generation)
        {
            return false;
        }
        if self.profiles.len() >= 32 && !self.profiles.contains_key(&p.id) {
            if let Some(id) = self
                .profiles
                .values()
                .min_by_key(|v| v.generation)
                .map(|v| v.id.clone())
            {
                self.profiles.remove(&id);
                self.tracks.remove(&id);
            }
        }
        let range = range(pixels);
        let desired = choose(range, self.tracks.get(&p.id).map(|t| t.light));
        let next = channels(if desired { 0xf9fafb } else { 0x101318 });
        let track = self.tracks.entry(p.id.clone()).or_insert(Track {
            light: desired,
            candidate: None,
            from: next,
            to: next,
            started: now,
            range,
        });
        if !p.moving {
            track.range = range;
        }
        if !p.moving && desired != track.light {
            match track.candidate {
                Some((v, at)) if v == desired => {
                    if now.saturating_duration_since(at)
                        >= Duration::from_millis(settings.settle_ms as u64)
                    {
                        let current = interpolate(track, now);
                        track.from = if animated { current } else { next };
                        track.to = next;
                        track.started = now;
                        track.light = desired;
                        track.candidate = None;
                    }
                }
                _ => track.candidate = Some((desired, now)),
            }
        } else {
            track.candidate = None;
        }
        self.profiles.insert(p.id.clone(), p);
        true
    }
    pub fn palette(&self, id: &str, now: Instant) -> Option<Palette> {
        self.tracks.get(id).map(|t| Palette {
            color: interpolate(t, now),
            range: t.range,
        })
    }
    pub fn animating(&self, id: &str, now: Instant) -> bool {
        self.tracks.get(id).is_some_and(|t| {
            t.from != t.to && now.saturating_duration_since(t.started) < Duration::from_millis(240)
        })
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}
fn interpolate(t: &Track, now: Instant) -> [f32; 3] {
    let x = (now.saturating_duration_since(t.started).as_secs_f32() / 0.240).clamp(0., 1.);
    let x = x * x * (3. - 2. * x);
    std::array::from_fn(|i| t.from[i] + (t.to[i] - t.from[i]) * x)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn p(v: f32) -> Profile {
        Profile {
            id: "chat".into(),
            role: "chat".into(),
            generation: 1,
            pixels: vec![[v; 3]],
            rect: [0., 0., 100., 100.],
            moving: false,
        }
    }
    #[test]
    fn chooses_readable_text_without_any_surface_output() {
        assert!(choose((0., 0.), None));
        assert!(!choose((1., 1.), None));
        let split = choose((0., 1.), Some(true));
        assert!(split);
    }
    #[test]
    fn switching_settles_then_eases_and_respects_reduced_motion() {
        let mut s = State::default();
        let at = Instant::now();
        let settings = Settings::default();
        s.update(p(0.), &[[0.; 3]], &settings, at, true);
        s.update(
            p(1.),
            &[[1.; 3]],
            &settings,
            at + Duration::from_millis(10),
            true,
        );
        assert!(!s.animating("chat", at));
        s.update(
            p(1.),
            &[[1.; 3]],
            &settings,
            at + Duration::from_millis(600),
            true,
        );
        assert!(s.animating("chat", at + Duration::from_millis(600)));
        let a = s.palette("chat", at + Duration::from_millis(600)).unwrap();
        let b = s.palette("chat", at + Duration::from_millis(720)).unwrap();
        let c = s.palette("chat", at + Duration::from_millis(840)).unwrap();
        assert!(a.color[0] > b.color[0] && b.color[0] > c.color[0]);
        assert!(!s.animating("chat", at + Duration::from_millis(841)));
    }
    #[test]
    fn off_by_default_and_bad_samples_do_nothing() {
        assert!(!Settings::default().enabled);
        let mut bad = p(0.);
        bad.pixels[0][0] = f32::NAN;
        assert!(!valid(&bad));
    }
    #[test]
    fn filtering_preserves_glyph_alpha() {
        let p = Palette {
            color: channels(0x101318),
            range: (0.8, 1.),
        };
        let original = gpui::Hsla::from(gpui::Rgba {
            r: 1.,
            g: 1.,
            b: 1.,
            a: 0.4,
        });
        let c = gpui::Rgba::from(p.filter(original));
        assert_eq!(c.a, 0.4);
        assert!(c.r < 0.1);
    }
}
