//! Experimental local contrast. Numeric backdrop samples only; no captured image
//! or text enters persistence, provider context, diagnostics, or network requests.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    pub target: f32,
    pub settle_ms: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            target: 4.5,
            settle_ms: 500,
        }
    }
}
impl Settings {
    pub fn normalized(&self) -> Self {
        Self {
            enabled: self.enabled,
            target: if self.target.is_finite() {
                self.target.clamp(4.5, 7.)
            } else {
                4.5
            },
            settle_ms: self.settle_ms.clamp(200, 1500),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ink {
    Light,
    Dark,
}
impl Ink {
    pub fn rgb(self) -> u32 {
        match self {
            Self::Light => 0xf9fafb,
            Self::Dark => 0x101318,
        }
    }
    pub fn dark(self) -> bool {
        self == Self::Light
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
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Decision {
    pub ink: Ink,
    pub tint: u32,
    pub alpha: f32,
    pub minimum: f32,
}
#[derive(Default)]
pub struct State {
    pub status: String,
    pub profiles: HashMap<String, Profile>,
    pub decisions: HashMap<String, Decision>,
    tracks: HashMap<String, Track>,
    pub revision: u64,
}
struct Track {
    ink: Ink,
    candidate: Option<(Ink, Instant)>,
    last_lower: Instant,
}

pub fn channels(rgb: u32) -> [f32; 3] {
    [
        ((rgb >> 16) & 255) as f32 / 255.,
        ((rgb >> 8) & 255) as f32 / 255.,
        (rgb & 255) as f32 / 255.,
    ]
}
pub fn composite(front: [f32; 3], alpha: f32, back: [f32; 3]) -> [f32; 3] {
    let a = alpha.clamp(0., 1.);
    std::array::from_fn(|i| front[i] * a + back[i] * (1. - a))
}
pub fn luminance(rgb: [f32; 3]) -> f32 {
    let linear = |v: f32| {
        let v = v.clamp(0., 1.);
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2])
}
pub fn ratio(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
pub fn valid_pixels(pixels: &[[f32; 3]]) -> bool {
    !pixels.is_empty()
        && pixels.len() <= 256
        && pixels
            .iter()
            .flatten()
            .all(|v| v.is_finite() && (0. ..=1.).contains(v))
}
pub fn minimum(ink: Ink, tint: u32, alpha: f32, pixels: &[[f32; 3]]) -> f32 {
    pixels
        .iter()
        .map(|p| ratio(channels(ink.rgb()), composite(channels(tint), alpha, *p)))
        .fold(f32::INFINITY, f32::min)
}
/// Binary search the least local scrim that protects every sample. White/black
/// are feasible endpoints even on split, saturated, or transparent backgrounds.
pub fn solve_for(ink: Ink, pixels: &[[f32; 3]], target: f32) -> Decision {
    let tint = match ink {
        Ink::Light => 0x080a0e,
        Ink::Dark => 0xfafafa,
    };
    let mut low = 0.;
    let mut high = 1.;
    if minimum(ink, tint, 0., pixels) >= target {
        high = 0.;
    } else {
        for _ in 0..18 {
            let mid = (low + high) * 0.5;
            if minimum(ink, tint, mid, pixels) >= target {
                high = mid
            } else {
                low = mid
            }
        }
    }
    Decision {
        ink,
        tint,
        alpha: high,
        minimum: minimum(ink, tint, high, pixels),
    }
}
pub fn solve(pixels: &[[f32; 3]], target: f32, previous: Option<Ink>) -> Decision {
    let a = solve_for(Ink::Light, pixels, target);
    let b = solve_for(Ink::Dark, pixels, target);
    let score = |d: &Decision| {
        d.alpha * 3. - d.minimum.min(14.).ln() * 0.12
            + if Some(d.ink) == previous { -0.08 } else { 0. }
    };
    if score(&a) <= score(&b) { a } else { b }
}
impl State {
    pub fn accept(&mut self, profiles: Vec<Profile>) {
        for p in profiles.into_iter().take(32) {
            if p.id.len() > 160 || p.role.len() > 32 || !valid_pixels(&p.pixels) {
                continue;
            }
            if self
                .profiles
                .get(&p.id)
                .is_some_and(|old| old.generation > p.generation)
            {
                continue;
            }
            if !self.profiles.contains_key(&p.id) && self.profiles.len() >= 32 {
                if let Some(oldest) = self
                    .profiles
                    .values()
                    .min_by_key(|v| v.generation)
                    .map(|v| v.id.clone())
                {
                    self.profiles.remove(&oldest);
                    self.decisions.remove(&oldest);
                    self.tracks.remove(&oldest);
                }
            }
            self.profiles.insert(p.id.clone(), p);
        }
    }
    pub fn update(
        &mut self,
        id: &str,
        pixels: &[[f32; 3]],
        settings: &Settings,
        now: Instant,
    ) -> Option<Decision> {
        if !valid_pixels(pixels) {
            return None;
        }
        let target = if settings.target.is_finite() {
            settings.target.clamp(4.5, 10.)
        } else {
            4.5
        };
        let settings = settings.normalized();
        let prior = self.tracks.get(id).map(|t| t.ink);
        let desired = solve(pixels, target, prior);
        let track = self.tracks.entry(id.to_owned()).or_insert_with(|| Track {
            ink: desired.ink,
            candidate: None,
            last_lower: now,
        });
        if desired.ink != track.ink {
            match track.candidate {
                Some((ink, at)) if ink == desired.ink => {
                    if now.saturating_duration_since(at)
                        >= Duration::from_millis(settings.settle_ms as u64)
                    {
                        track.ink = desired.ink;
                        track.candidate = None;
                        track.last_lower = now;
                    }
                }
                _ => track.candidate = Some((desired.ink, now)),
            }
        } else {
            track.candidate = None;
        }
        let mut next = solve_for(track.ink, pixels, target);
        if let Some(old) = self.decisions.get(id) {
            if old.ink == next.ink
                && next.alpha < old.alpha
                && now.saturating_duration_since(track.last_lower) < Duration::from_millis(1000)
            {
                next.alpha = old.alpha;
                next.minimum = minimum(next.ink, next.tint, next.alpha, pixels);
            } else if next.alpha > old.alpha {
                track.last_lower = now;
            }
        }
        // Never quantize away protection that is required by the new backdrop.
        // Small reductions may wait, but even a tiny unsafe increase is immediate.
        if let Some(old) = self.decisions.get(id) {
            if old.ink == next.ink
                && (old.alpha - next.alpha).abs() <= 0.012
                && minimum(old.ink, old.tint, old.alpha, pixels) >= target
            {
                next.alpha = old.alpha;
                next.minimum = minimum(next.ink, next.tint, next.alpha, pixels);
            }
        }
        let changed = self
            .decisions
            .get(id)
            .is_none_or(|old| old.ink != next.ink || old.alpha != next.alpha);
        self.decisions.insert(id.to_owned(), next.clone());
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        Some(next)
    }
    pub fn clear(&mut self) {
        self.profiles.clear();
        self.decisions.clear();
        self.tracks.clear();
        self.status.clear();
        self.revision = self.revision.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn black_white_and_split_have_consistent_readable_ink() {
        for target in [4.5, 7.] {
            for pixels in [
                vec![[0.; 3]; 64],
                vec![[1.; 3]; 64],
                (0..64)
                    .map(|i| if i < 32 { [0.; 3] } else { [1.; 3] })
                    .collect(),
                vec![[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.9, 0.7, 0.1]],
            ] {
                let d = solve(&pixels, target, None);
                assert!(d.minimum >= target - 0.001, "{d:?}");
                assert!(d.alpha <= 1.);
            }
        }
        assert_eq!(solve(&[[1.; 3]], 4.5, None).ink, Ink::Dark);
        assert_eq!(solve(&[[0.; 3]], 4.5, None).ink, Ink::Light);
    }
    #[test]
    fn all_opacities_and_saturated_colours_remain_readable() {
        for opacity in [0., 0.1, 0.4, 0.6, 0.99, 1.] {
            for r in [0., 0.5, 1.] {
                for g in [0., 0.5, 1.] {
                    for b in [0., 0.5, 1.] {
                        let samples = [
                            composite([r, g, b], opacity, [0.; 3]),
                            composite([r, g, b], opacity, [1.; 3]),
                        ];
                        let d = solve(&samples, 7., None);
                        assert!(d.minimum >= 6.999);
                    }
                }
            }
        }
    }
    #[test]
    fn changing_background_protects_ink_before_switching_and_never_flickers() {
        let mut state = State::default();
        let settings = Settings {
            enabled: true,
            ..Default::default()
        };
        let now = Instant::now();
        let a = state.update("chat", &[[0.; 3]], &settings, now).unwrap();
        let b = state
            .update(
                "chat",
                &[[1.; 3]],
                &settings,
                now + Duration::from_millis(10),
            )
            .unwrap();
        assert_eq!(a.ink, b.ink);
        assert!(b.minimum >= 4.499);
        let c = state
            .update(
                "chat",
                &[[1.; 3]],
                &settings,
                now + Duration::from_millis(600),
            )
            .unwrap();
        assert_eq!(c.ink, Ink::Dark);
    }
    #[test]
    fn bad_or_stale_samples_cannot_change_a_panel() {
        let mut state = State::default();
        state.accept(vec![
            Profile {
                id: "a".into(),
                role: "chat".into(),
                generation: 2,
                rect: [0., 0., 1., 1.],
                moving: false,
                pixels: vec![[1.; 3]],
            },
            Profile {
                id: "a".into(),
                role: "chat".into(),
                generation: 1,
                rect: [0., 0., 1., 1.],
                moving: false,
                pixels: vec![[0.; 3]],
            },
        ]);
        assert_eq!(state.profiles["a"].generation, 2);
        assert!(!valid_pixels(&[[f32::NAN, 0., 0.]]));
        assert!(!valid_pixels(&[]));
        assert!(!valid_pixels(&[[2., 0., 0.]]));
    }
    #[test]
    fn experiment_is_opt_in_and_normalizes_preferences() {
        assert!(!Settings::default().enabled);
        assert_eq!(
            Settings {
                enabled: true,
                target: f32::NAN,
                settle_ms: 0
            }
            .normalized()
            .target,
            4.5
        );
    }
}

#[cfg(test)]
mod safety_tests {
    use super::*;
    #[test]
    fn tiny_backdrop_changes_never_quantize_away_required_protection() {
        let mut state = State::default();
        let settings = Settings {
            enabled: true,
            ..Default::default()
        };
        let now = Instant::now();
        for i in 0..100 {
            let pixels = [[0.45 + i as f32 * 0.0005; 3]];
            let d = state
                .update(
                    "chat",
                    &pixels,
                    &settings,
                    now + Duration::from_millis(i * 10),
                )
                .unwrap();
            assert!(minimum(d.ink, d.tint, d.alpha, &pixels) >= 4.4999, "{d:?}");
            assert!((d.minimum - minimum(d.ink, d.tint, d.alpha, &pixels)).abs() < 0.0001);
        }
    }
    #[test]
    fn bounded_regions_evict_oldest_without_erasing_other_panels() {
        let mut state = State::default();
        for i in 0..40 {
            state.accept(vec![Profile {
                id: i.to_string(),
                role: "chat".into(),
                generation: i,
                pixels: vec![[0.; 3]],
                rect: [0.; 4],
                moving: false,
            }]);
        }
        assert_eq!(state.profiles.len(), 32);
        assert!(!state.profiles.contains_key("0"));
        assert!(state.profiles.contains_key("39"));
    }
    #[test]
    fn finite_settling_releases_video_guard_then_chooses_dark_ink() {
        let mut state = State::default();
        let settings = Settings {
            enabled: true,
            settle_ms: 1500,
            ..Default::default()
        };
        let now = Instant::now();
        let a = state.update("chat", &[[0.; 3]], &settings, now).unwrap();
        let b = state
            .update(
                "chat",
                &[[0.; 3], [1.; 3]],
                &settings,
                now + Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(a.ink, b.ink);
        state.update(
            "chat",
            &[[1.; 3]],
            &settings,
            now + Duration::from_millis(2300),
        );
        let d = state
            .update(
                "chat",
                &[[1.; 3]],
                &settings,
                now + Duration::from_millis(4000),
            )
            .unwrap();
        assert_eq!(d.ink, Ink::Dark);
        assert_eq!(d.alpha, 0.);
    }
    #[test]
    fn bounded_solver_cpu_budget() {
        let pixels: Vec<_> = (0..64)
            .map(|i| [i as f32 / 63., (i % 9) as f32 / 8., 0.4])
            .collect();
        let at = Instant::now();
        for _ in 0..32 {
            std::hint::black_box(solve(&pixels, 7., None));
        }
        assert!(
            at.elapsed() < Duration::from_secs(2),
            "32-panel unoptimized solver exceeded bounded validation budget"
        );
        eprintln!(
            "adaptive solver: 32 panels x 64 samples = {:?}",
            at.elapsed()
        );
    }
}
