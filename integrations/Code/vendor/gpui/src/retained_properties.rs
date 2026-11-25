use crate::{ElementId, Pixels};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// The periodic wave applied to inherited element opacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpacityWaveform {
    /// baseline + amplitude * cos(TAU * progress).
    CosineTau,
    /// baseline + amplitude * sin(PI * progress).
    HalfSinePi,
}

/// Immutable coefficients for a retained opacity animation.
#[derive(Clone, Copy, Debug)]
pub struct OpacityAnimation {
    /// Duration of one complete repeating cycle.
    pub period: Duration,
    /// Constant component of the opacity factor.
    pub baseline: f32,
    /// Wave coefficient, with its full range constrained to zero through one.
    pub amplitude: f32,
    /// Trigonometric function evaluated at the normalized monotonic progress.
    pub waveform: OpacityWaveform,
}

impl OpacityAnimation {
    pub(crate) fn valid(self) -> bool {
        let low = match self.waveform {
            OpacityWaveform::CosineTau => self.baseline - self.amplitude.abs(),
            OpacityWaveform::HalfSinePi => self.baseline.min(self.baseline + self.amplitude),
        };
        let high = match self.waveform {
            OpacityWaveform::CosineTau => self.baseline + self.amplitude.abs(),
            OpacityWaveform::HalfSinePi => self.baseline.max(self.baseline + self.amplitude),
        };
        !self.period.is_zero()
            && self.baseline.is_finite()
            && self.amplitude.is_finite()
            && low >= 0.0
            && high <= 1.0
    }

    pub(crate) fn value(self, progress: f32) -> f32 {
        let wave = match self.waveform {
            OpacityWaveform::CosineTau => (std::f32::consts::TAU * progress).cos(),
            OpacityWaveform::HalfSinePi => (std::f32::consts::PI * progress).sin(),
        };
        self.baseline + self.amplitude * wave
    }
}

#[derive(Clone)]
pub(crate) struct SpriteFrames {
    pub columns: Arc<[u32]>,
    pub durations: Arc<[u64]>,
    pub total: u64,
    pub cell_size: Pixels,
}

impl SpriteFrames {
    pub(crate) fn new(columns: Vec<usize>, durations: Vec<u64>, cell_size: Pixels) -> Option<Self> {
        if columns.is_empty()
            || columns.len() > 128
            || columns.len() != durations.len()
            || columns.iter().any(|&column| column >= 8)
            || durations.contains(&0)
            || !cell_size.0.is_finite()
            || cell_size.0 <= 0.0
        {
            return None;
        }
        let total = durations
            .iter()
            .try_fold(0u64, |sum, &duration| sum.checked_add(duration))?;
        Some(Self {
            columns: columns.into_iter().map(|column| column as u32).collect(),
            durations: durations.into(),
            total,
            cell_size,
        })
    }

    pub(crate) fn column(&self, progress: f32) -> u32 {
        let mut at = (progress.clamp(0.0, 0.999999) * self.total as f32) as u64;
        for (&duration, &column) in self.durations.iter().zip(self.columns.iter()) {
            if at < duration {
                return column;
            }
            at -= duration;
        }
        self.columns[0]
    }

    pub(crate) fn animated(&self) -> bool {
        self.columns.iter().any(|column| *column != self.columns[0])
    }
}

#[derive(Clone, Default)]
pub(crate) struct ElementProperties {
    pub opacity: Option<(ElementId, OpacityAnimation)>,
    pub frames: Option<(ElementId, SpriteFrames)>,
}

#[derive(Default)]
pub(crate) struct PropertyEpochs {
    opacity: Option<(ElementId, Instant)>,
    frames: Option<(ElementId, Instant)>,
}

fn epoch(
    state: &mut Option<(ElementId, Instant)>,
    id: Option<&ElementId>,
    now: Instant,
) -> Option<Instant> {
    let Some(id) = id else {
        *state = None;
        return None;
    };
    if state.as_ref().is_none_or(|(previous, _)| previous != id) {
        *state = Some((id.clone(), now));
    }
    state.as_ref().map(|(_, started)| *started)
}

impl PropertyEpochs {
    pub(crate) fn resolve(
        &mut self,
        properties: &ElementProperties,
        now: Instant,
    ) -> ResolvedProperties {
        let opacity_started = epoch(
            &mut self.opacity,
            properties.opacity.as_ref().map(|(id, _)| id),
            now,
        );
        let frames_started = epoch(
            &mut self.frames,
            properties.frames.as_ref().map(|(id, _)| id),
            now,
        );
        ResolvedProperties {
            opacity: properties
                .opacity
                .as_ref()
                .zip(opacity_started)
                .map(|((_, animation), started)| (*animation, started)),
            frames: properties
                .frames
                .as_ref()
                .zip(frames_started)
                .map(|((_, frames), started)| (frames.clone(), started)),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct ResolvedProperties {
    pub opacity: Option<(OpacityAnimation, Instant)>,
    pub frames: Option<(SpriteFrames, Instant)>,
}

pub(crate) fn progress(now: Instant, started: Instant, duration: Duration) -> f32 {
    let mut progress =
        now.saturating_duration_since(started).as_secs_f32() / duration.as_secs_f32();
    if progress > 1.0 {
        progress %= 1.0;
    }
    progress
}

/// Scalar arrays keep the same four-byte alignment in all shader languages.
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct GpuSpriteProperties {
    pub baseline: f32,
    pub amplitude: f32,
    pub opacity_progress: f32,
    pub waveform: u32,
    pub frame_progress: f32,
    pub total: u32,
    pub frame_count: u32,
    pub selected_column: u32,
    pub origins: [[f32; 2]; 8],
    pub schedule: [[u32; 2]; 128],
}

impl Default for GpuSpriteProperties {
    fn default() -> Self {
        Self {
            baseline: 1.0,
            amplitude: 0.0,
            opacity_progress: 0.0,
            waveform: 0,
            frame_progress: 0.0,
            total: 0,
            frame_count: 0,
            selected_column: 0,
            origins: [[0.0; 2]; 8],
            schedule: [[0; 2]; 128],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::px;

    #[test]
    fn periodic_progress_preserves_legacy_division_remainder_and_exact_one() {
        let started = Instant::now();
        for period in [Duration::from_millis(900), Duration::from_millis(1300), Duration::from_secs(3), Duration::from_millis(u64::MAX)] {
            for elapsed in [Duration::ZERO, Duration::from_micros(1), Duration::from_millis(900), Duration::from_millis(1300), Duration::from_secs(60)] {
                let mut expected = elapsed.as_secs_f32() / period.as_secs_f32();
                if expected > 1.0 { expected %= 1.0; }
                assert_eq!(progress(started + elapsed, started, period).to_bits(), expected.to_bits());
            }
        }
        assert_eq!(progress(started + Duration::from_secs(3), started, Duration::from_secs(3)), 1.0);
    }

    #[test]
    fn half_sine_accepts_asymmetric_range_and_static_descriptors_park() {
        let animation = OpacityAnimation { period: Duration::from_millis(1300), baseline: 0.72, amplitude: 0.28, waveform: OpacityWaveform::HalfSinePi };
        assert!(animation.valid());
        let history = OpacityAnimation { baseline: 0.4, amplitude: 0.6, ..animation };
        assert!(history.valid());
        for tick in 0..=100 {
            let fraction = tick as f32 / 100.0;
            assert_eq!(history.value(fraction).to_bits(), (0.4 + 0.6 * (fraction * std::f32::consts::PI).sin()).to_bits());
        }
        assert!(!OpacityAnimation { baseline: 0.9, amplitude: 0.2, ..animation }.valid());
        assert!(!OpacityAnimation { period: Duration::ZERO, ..animation }.valid());
    }

    #[test]
    fn sheet_selection_covers_ordered_repeats_boundaries_and_large_duration_fallback() {
        for durations in [vec![100, 200, 100], vec![60_000; 128], vec![u64::MAX - 100, 50, 50]] {
            let columns = (0..durations.len()).map(|i| i % 8).collect::<Vec<_>>();
            let frames = SpriteFrames::new(columns.clone(), durations.clone(), px(18.5)).unwrap();
            for fraction in [0.0f32, 0.000001, 0.249999, 0.25, 0.250001, 0.5, 0.999999, 1.0] {
                let mut at = (fraction.clamp(0.0, 0.999999) * frames.total as f32) as u64;
                let mut expected = columns[0];
                for (&duration, &column) in durations.iter().zip(&columns) {
                    if at < duration { expected = column; break; }
                    at -= duration;
                }
                assert_eq!(frames.column(fraction), expected as u32);
            }
        }
        assert!(!SpriteFrames::new(vec![7; 128], vec![16; 128], px(18.5)).unwrap().animated());
        assert!(SpriteFrames::new(vec![0, 1], vec![u64::MAX, 1], px(18.5)).is_none());
        assert!(SpriteFrames::new(vec![8], vec![16], px(18.5)).is_none());
    }

    #[test]
    fn identities_preserve_loading_phase_and_changed_configuration_but_reset_on_disable() {
        let started = Instant::now();
        let mut epochs = PropertyEpochs::default();
        let animation = OpacityAnimation { period: Duration::from_secs(3), baseline: 0.88, amplitude: 0.12, waveform: OpacityWaveform::CosineTau };
        let mut properties = ElementProperties { opacity: Some(("pulse".into(), animation)), frames: Some(("sheet".into(), SpriteFrames::new(vec![0, 7], vec![100, 200], px(18.5)).unwrap())) };
        let first = epochs.resolve(&properties, started);
        properties.opacity.as_mut().unwrap().1.period = Duration::from_millis(1300);
        let again = epochs.resolve(&properties, started + Duration::from_secs(20));
        assert_eq!(first.opacity.unwrap().1, again.opacity.unwrap().1);
        assert_eq!(first.frames.unwrap().1, again.frames.unwrap().1);
        epochs.resolve(&ElementProperties::default(), started);
        let reopened = epochs.resolve(&properties, started + Duration::from_secs(21));
        assert_eq!(reopened.opacity.unwrap().1, started + Duration::from_secs(21));
    }

    #[test]
    fn gpu_record_has_identical_scalar_array_offsets_for_all_three_backends() {
        assert_eq!(std::mem::size_of::<GpuSpriteProperties>(), 1120);
        assert_eq!(std::mem::align_of::<GpuSpriteProperties>(), 4);
        assert_eq!(std::mem::offset_of!(GpuSpriteProperties, origins), 32);
        assert_eq!(std::mem::offset_of!(GpuSpriteProperties, schedule), 96);
    }
    #[test]
    fn plain_and_stateful_builders_preserve_explicit_img_and_svg_ids() {
        use crate::Element;
        let animation = OpacityAnimation { period: Duration::from_millis(900), baseline: 0.4, amplitude: 0.6, waveform: OpacityWaveform::HalfSinePi };
        let image = crate::InteractiveElement::id(crate::img("unused.png"), "image-id").sprite_frames("sheet-clock", vec![0, 7], vec![100, 200], px(18.0)).opacity_animation("opacity-clock", animation);
        assert_eq!(Element::id(&image), Some("image-id".into()));
        let svg = crate::InteractiveElement::id(crate::svg(), "svg-id").opacity_animation("svg-clock", animation);
        assert_eq!(Element::id(&svg), Some("svg-id".into()));
        assert_eq!(Element::id(&crate::svg().opacity_animation("plain-svg", animation)), Some("plain-svg".into()));
        assert_eq!(Element::id(&crate::img("unused.png").opacity_animation("plain-image", animation)), Some("plain-image".into()));
    }

}
