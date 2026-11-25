use super::*;
use crate::QuadShimmer;

#[derive(Clone, Copy)]
pub(super) struct QuadShimmerRecord {
    clock: QuadShimmer,
    elapsed: f32,
    submitted_complete: bool,
}

impl Scene {
    #[cfg(test)]
    pub(crate) fn expire_quad_shimmers_for_test(&mut self) {
        for record in &mut self.quad_shimmers {
            record.clock.started = Instant::now() - Duration::from_secs(6);
        }
    }

    pub(crate) fn insert_quad_shimmer(&mut self, quad: Quad, clock: QuadShimmer) {
        self.insert_quad_shimmer_with_pending_final(quad, clock, false);
    }

    fn insert_quad_shimmer_with_pending_final(&mut self, mut quad: Quad, clock: QuadShimmer, pending_final: bool) {
        if quad.bounds.intersect(&quad.content_mask.bounds).is_empty()
            || quad.background.solid.a == 0.0
        {
            return;
        }
        let elapsed = clock.elapsed_at(Instant::now());
        if !pending_final && !clock.repeating() && elapsed >= 5.0 {
            return;
        }
        // Consecutive tiles share the most recently registered group without a scan.
        let index = if self
            .quad_shimmers
            .last()
            .is_some_and(|record| record.clock == clock)
        {
            self.quad_shimmers.len() - 1
        } else if let Some(index) = self
            .quad_shimmers
            .iter()
            .position(|record| record.clock == clock)
        {
            index
        } else {
            self.quad_shimmers.push(QuadShimmerRecord {
                clock,
                elapsed,
                submitted_complete: false,
            });
            self.quad_shimmers.len() - 1
        };
        if self.quad_shimmers[index].submitted_complete {
            return;
        }
        quad.background.set_shimmer_recipe(index, elapsed);
        self.insert_primitive(quad);
    }

    pub(super) fn sample_quad_shimmers(&mut self, now: Instant) {
        for record in &mut self.quad_shimmers {
            record.elapsed = record.clock.elapsed_at(now);
        }
        for &(quad_index, clock_index) in &self.quad_shimmer_targets {
            self.quads[quad_index]
                .background
                .set_shimmer_elapsed(self.quad_shimmers[clock_index].elapsed);
        }
    }

    pub(super) fn quad_shimmers_active_at(&self, now: Instant) -> bool {
        self.quad_shimmer_targets.iter().any(|&(_, index)| {
            let record = &self.quad_shimmers[index];
            !record.submitted_complete
                && (record.clock.repeating() || record.clock.elapsed_at(now) < 5.0)
        })
    }

    pub(super) fn finish_quad_shimmers(&mut self) {
        self.quad_shimmer_targets.clear();
        for (quad_index, quad) in self.quads.iter().enumerate() {
            if let Some(clock_index) = quad.background.shimmer_recipe() {
                self.quad_shimmer_targets.push((quad_index, clock_index));
            }
        }
    }

    pub(super) fn did_submit_quad_shimmers(&mut self) {
        let mut retired = false;
        for record in &mut self.quad_shimmers {
            if !record.clock.repeating() && record.elapsed >= 5.0 && !record.submitted_complete {
                record.submitted_complete = true;
                retired = true;
            }
        }
        if retired {
            self.quads.retain(|quad| {
                quad.background
                    .shimmer_recipe()
                    .is_none_or(|index| !self.quad_shimmers[index].submitted_complete)
            });
            self.finish_quad_shimmers();
        }
    }

    pub(super) fn replay_quad_shimmer(&mut self, primitive: &Primitive, previous: &Scene) -> bool {
        let Primitive::Quad(quad) = primitive else {
            return false;
        };
        let Some(index) = quad.background.shimmer_recipe() else {
            return false;
        };
        let record = &previous.quad_shimmers[index];
        if !record.submitted_complete {
            self.insert_quad_shimmer_with_pending_final(quad.clone(), record.clock, true);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QuadShimmerTile, QuadShimmerTiming, rgb, size, solid_background};

    fn tile(alpha: f32) -> Quad {
        Quad {
            bounds: Bounds::new(
                point(ScaledPixels(8.0), ScaledPixels(8.0)),
                size(ScaledPixels(4.0), ScaledPixels(4.0)),
            ),
            content_mask: ContentMask {
                bounds: Bounds::new(
                    point(ScaledPixels(0.0), ScaledPixels(0.0)),
                    size(ScaledPixels(64.0), ScaledPixels(64.0)),
                ),
            },
            background: solid_background(crate::Hsla::from(rgb(0x9ac3df)).opacity(alpha))
                .with_shimmer_tile(QuadShimmerTile {
                    delay: 0.42,
                    filled: true,
                    feather: 0.65,
                }),
            ..Default::default()
        }
    }

    #[test]
    fn one_clock_group_services_160_tiles_without_sprite_property_records() {
        let started = Instant::now();
        let clock = QuadShimmer {
            started,
            timing: QuadShimmerTiming::FiniteNormalized,
        };
        let mut scene = Scene::default();
        for _ in 0..160 {
            scene.insert_quad_shimmer(tile(0.37), clock);
        }
        scene.finish();
        assert_eq!(scene.quad_shimmers.len(), 1);
        assert_eq!(scene.quad_shimmer_targets.len(), 160);
        assert!(scene.gpu_sprite_properties.is_empty());
        assert!(scene.has_presentation_effects());
        for frame in 0..120 {
            let now = started + Duration::from_micros(frame * 16_667);
            scene.sample_presentation_effects(now);
            assert_eq!(
                scene.quads[0]
                    .background
                    .gradient_angle_or_pattern_height
                    .to_bits(),
                clock.elapsed_at(now).to_bits()
            );
            let PaintOperation::Primitive(Primitive::Quad(base)) = &scene.paint_operations[0]
            else {
                panic!()
            };
            assert_eq!(base.background.solid.a.to_bits(), 0.37f32.to_bits());
        }
    }

    #[test]
    fn finite_tiles_retire_after_final_submission_and_completed_replay_parks() {
        let started = Instant::now();
        let mut scene = Scene::default();
        scene.insert_quad_shimmer(
            tile(1.0),
            QuadShimmer {
                started,
                timing: QuadShimmerTiming::ElapsedSeconds,
            },
        );
        scene.finish();
        scene.sample_presentation_effects(started + Duration::from_secs(5));
        assert!(scene.has_presentation_effects());
        assert!(!scene.has_active_presentation_effects(started + Duration::from_secs(5)));
        assert_eq!(
            scene.quads.len(),
            1,
            "Deferred final frames must keep their original geometry"
        );
        scene.did_submit_presentation_effects(started + Duration::from_secs(5));
        assert!(scene.quads.is_empty());
        assert!(!scene.has_presentation_effects());
        let mut replay = Scene::default();
        replay.replay(0..scene.len(), &scene);
        replay.finish();
        assert!(replay.quads.is_empty());
        assert!(!replay.has_presentation_effects());
    }

    #[test]
    fn repeating_clock_replay_preserves_epoch_and_exact_five_second_blank_endpoint() {
        let started = Instant::now();
        let clock = QuadShimmer {
            started,
            timing: QuadShimmerTiming::RepeatNormalized,
        };
        let mut scene = Scene::default();
        scene.insert_quad_shimmer(tile(0.5), clock);
        scene.finish();
        for generation in 0..4 {
            let mut next = Scene::default();
            next.replay(0..scene.len(), &scene);
            next.finish();
            next.sample_presentation_effects(started + Duration::from_secs(5 + generation * 5));
            next.did_submit_presentation_effects(started + Duration::from_secs(5 + generation * 5));
            assert_eq!(next.quad_shimmers[0].clock.started, started);
            assert_eq!(next.quads.len(), 1);
            assert!(next.has_active_presentation_effects(started + Duration::from_secs(30)));
            scene = next;
        }
    }

    #[test]
    fn invisible_tiles_do_not_register_clocks_and_clear_retires_all_references() {
        let clock = QuadShimmer {
            started: Instant::now(),
            timing: QuadShimmerTiming::RepeatNormalized,
        };
        let mut scene = Scene::default();
        scene.insert_quad_shimmer(tile(0.0), clock);
        let mut clipped = tile(1.0);
        clipped.content_mask.bounds.size = size(ScaledPixels(0.0), ScaledPixels(0.0));
        scene.insert_quad_shimmer(clipped, clock);
        scene.finish();
        assert!(scene.quad_shimmers.is_empty());
        assert!(!scene.has_presentation_effects());
        scene.insert_quad_shimmer(tile(1.0), clock);
        scene.finish();
        scene.clear();
        assert!(scene.quad_shimmers.is_empty());
        assert!(scene.quad_shimmer_targets.is_empty());
    }
    #[test]
    fn expired_unsubmitted_clock_survives_cached_replay_until_final_submission() {
        let started = Instant::now();
        let mut scene = Scene::default();
        scene.insert_quad_shimmer(tile(0.5), QuadShimmer { started, timing: QuadShimmerTiming::ElapsedSeconds });
        scene.finish();
        // Backdate the retained epoch to model time spent without a drawable.
        scene.quad_shimmers[0].clock.started = started - Duration::from_secs(6);
        scene.sample_presentation_effects(started);
        assert!(scene.has_presentation_effects());
        let mut next = Scene::default();
        next.replay(0..scene.len(), &scene);
        next.finish();
        assert_eq!(next.quads.len(), 1);
        assert!(next.has_presentation_effects());
        next.sample_presentation_effects(started);
        next.did_submit_presentation_effects(started);
        assert!(next.quads.is_empty());
        assert!(!next.has_presentation_effects());
    }

}
