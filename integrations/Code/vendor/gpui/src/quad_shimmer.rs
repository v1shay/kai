use std::time::Instant;

/// Time conversion used by a retained five-second tile shimmer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuadShimmerTiming {
    /// Match a finite AnimationElement's normalized progress multiplied by five.
    FiniteNormalized,
    /// Match repeating AnimationElement progress, retaining exact progress one.
    RepeatNormalized,
    /// Preserve a picker or control event's direct elapsed seconds.
    ElapsedSeconds,
}

/// A monotonic clock shared by the visible tiles in one retained shimmer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuadShimmer {
    /// Original element or event epoch, preserved through cached paint replay.
    pub started: Instant,
    /// The original caller's elapsed-time conversion.
    pub timing: QuadShimmerTiming,
}

impl QuadShimmer {
    pub(crate) fn elapsed_at(self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32();
        match self.timing {
            QuadShimmerTiming::ElapsedSeconds => elapsed,
            QuadShimmerTiming::FiniteNormalized => (elapsed / 5.0).min(1.0) * 5.0,
            QuadShimmerTiming::RepeatNormalized => {
                let mut progress = elapsed / 5.0;
                if progress > 1.0 {
                    progress %= 1.0;
                }
                progress * 5.0
            }
        }
    }

    pub(crate) fn repeating(self) -> bool {
        self.timing == QuadShimmerTiming::RepeatNormalized
    }
}

/// Immutable wave parameters for one tile of a retained shimmer.
#[derive(Clone, Copy, Debug)]
pub struct QuadShimmerTile {
    /// Nonnegative distance-derived delay in seconds.
    pub delay: f32,
    /// Filled tiles use the pulse, while unfilled tiles retain the faint grid.
    pub filled: bool,
    /// Edge attenuation between zero and one, applied before byte quantization.
    pub feather: f32,
}

impl QuadShimmerTile {
    pub(crate) fn valid(self) -> bool {
        self.delay.is_finite()
            && self.delay >= 0.0
            && self.feather.is_finite()
            && (0.0..=1.0).contains(&self.feather)
    }

    #[cfg(test)]
    pub(crate) fn alpha_at(self, elapsed: f32) -> u32 {
        if elapsed >= 5.0 {
            return 0;
        }
        let fade = ((5.0 - elapsed) / 0.4).clamp(0.0, 1.0);
        let phase = ((elapsed - self.delay).max(0.0) % 1.4) / 1.4;
        let pulse = if phase < 0.26 {
            phase / 0.26
        } else if phase < 0.52 {
            1.0 - (phase - 0.26) / 0.26 * 0.82
        } else {
            0.18 * (1.0 - (phase - 0.52) / 0.48)
        };
        (((if self.filled { pulse } else { 0.09 }) * fade * self.feather) * 210.0) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn shimmer_clock_preserves_original_f32_time_conversions_and_repeat_endpoint() {
        let started = Instant::now();
        for micros in [
            0u64, 1, 16_667, 364_000, 728_000, 1_400_000, 4_599_999, 4_600_000, 4_999_999,
            5_000_000, 5_000_001, 10_000_000, 60_000_000,
        ] {
            let elapsed = Duration::from_micros(micros).as_secs_f32();
            for timing in [
                QuadShimmerTiming::FiniteNormalized,
                QuadShimmerTiming::RepeatNormalized,
                QuadShimmerTiming::ElapsedSeconds,
            ] {
                let expected = match timing {
                    QuadShimmerTiming::ElapsedSeconds => elapsed,
                    QuadShimmerTiming::FiniteNormalized => (elapsed / 5.0).min(1.0) * 5.0,
                    QuadShimmerTiming::RepeatNormalized => {
                        let mut t = elapsed / 5.0;
                        if t > 1.0 {
                            t %= 1.0;
                        }
                        t * 5.0
                    }
                };
                assert_eq!(
                    QuadShimmer { started, timing }
                        .elapsed_at(started + Duration::from_micros(micros))
                        .to_bits(),
                    expected.to_bits()
                );
            }
        }
        assert_eq!(
            QuadShimmer {
                started,
                timing: QuadShimmerTiming::RepeatNormalized
            }
            .elapsed_at(started + Duration::from_secs(5)),
            5.0
        );
    }

    #[test]
    fn tile_parameters_validate_and_finite_quantized_alpha_reaches_zero() {
        let tile = QuadShimmerTile {
            delay: 0.42,
            filled: true,
            feather: 0.65,
        };
        assert!(tile.valid());
        assert_eq!(tile.alpha_at(5.0), 0);
        assert_eq!(tile.alpha_at(6.0), 0);
        assert!(
            !QuadShimmerTile {
                delay: f32::NAN,
                ..tile
            }
            .valid()
        );
        assert!(
            !QuadShimmerTile {
                feather: -0.01,
                ..tile
            }
            .valid()
        );
        assert!(
            !QuadShimmerTile {
                feather: 1.01,
                ..tile
            }
            .valid()
        );
    }
}
