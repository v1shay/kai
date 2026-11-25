use super::*;
use crate::{AtlasTextureId, AtlasTextureKind, AtlasTile, DevicePixels, TileId, point, size};
use std::time::{Duration, Instant};

fn legacy_cutoff(step: u32) -> u32 {
    const CUTOFFS: [u32; 15] = [0,1198373,2396746,3595118,4793491,5991863,7190236,8388608,9586981,10785353,11983726,13182098,14380471,15578843,16777216];
    CUTOFFS[step as usize]
}

fn sprite(id: u32, opacity: f32) -> PolychromeSprite {
    PolychromeSprite {
        order: 0,
        opacity_pulse_index: 0,
        grayscale: false,
        opacity,
        bounds: Bounds::new(
            point(ScaledPixels(4.0), ScaledPixels(4.0)),
            size(ScaledPixels(24.0), ScaledPixels(24.0)),
        ),
        content_mask: ContentMask {
            bounds: Bounds::new(
                point(ScaledPixels(0.0), ScaledPixels(0.0)),
                size(ScaledPixels(64.0), ScaledPixels(64.0)),
            ),
        },
        corner_radii: Corners::all(ScaledPixels(4.0)),
        tile: AtlasTile {
            texture_id: AtlasTextureId {
                index: 0,
                kind: AtlasTextureKind::Polychrome,
            },
            tile_id: TileId(id),
            padding: 0,
            bounds: Bounds::new(
                point(DevicePixels(0), DevicePixels(0)),
                size(DevicePixels(8), DevicePixels(8)),
            ),
        },
    }
}

#[test]
fn dither_reveal_waits_for_first_submitted_frame_then_matches_legacy_checkpoints() {
    let original = Instant::now();
    let reveal = ImageDitherReveal::new(original, Duration::from_secs(14));
    let clone = reveal.clone();
    let mut scene = Scene::default();
    scene.insert_image_dither_reveal(sprite(1, 0.5), reveal);
    scene.finish();
    let first_submitted = original + Duration::from_secs(30);
    for now in [original, original + Duration::from_secs(5), first_submitted] {
        scene.sample_presentation_effects(now);
        assert_eq!(
            scene.polychrome_sprites[0].opacity_pulse_index,
            IMAGE_DITHER_REVEAL_FLAG
        );
        assert_eq!(clone.started(), None);
        assert!(scene.has_active_presentation_effects(now));
    }
    scene.did_submit_presentation_effects(first_submitted);
    assert_eq!(clone.started(), Some(first_submitted));
    for step in 0..=14u32 {
        let now = first_submitted + Duration::from_secs(step as u64);
        scene.sample_presentation_effects(now);
        assert_eq!(
            scene.polychrome_sprites[0].opacity_pulse_index,
            IMAGE_DITHER_REVEAL_FLAG | legacy_cutoff(step)
        );
        assert_eq!(scene.polychrome_sprites[0].opacity, 0.5);
    }
    assert!(
        scene.has_presentation_effects(),
        "Final sampled frame still needs submission"
    );
    assert!(!scene.has_active_presentation_effects(first_submitted + Duration::from_secs(14)));
    scene.did_submit_presentation_effects(first_submitted);
    assert!(!scene.has_presentation_effects());
}

#[test]
fn dither_reveal_preserves_future_delay_after_first_submission() {
    let now = Instant::now();
    let not_before = now + Duration::from_millis(80);
    let reveal = ImageDitherReveal::new(not_before, Duration::from_millis(600));
    let clone = reveal.clone();
    let mut scene = Scene::default();
    scene.insert_image_dither_reveal(sprite(1, 1.0), reveal);
    scene.finish();
    scene.sample_presentation_effects(now);
    scene.did_submit_presentation_effects(now);
    assert_eq!(clone.started(), Some(not_before));
    scene.sample_presentation_effects(now + Duration::from_millis(70));
    assert_eq!(
        scene.polychrome_sprites[0].opacity_pulse_index,
        IMAGE_DITHER_REVEAL_FLAG
    );
    scene.sample_presentation_effects(not_before + Duration::from_millis(300));
    assert_eq!(
        scene.polychrome_sprites[0].opacity_pulse_index,
        IMAGE_DITHER_REVEAL_FLAG | (IMAGE_DITHER_REVEAL_FULL_CUTOFF / 2)
    );
}

#[test]
fn dither_reveal_replay_remaps_recipe_and_shares_submitted_epoch_after_source_clear() {
    let start = Instant::now();
    let descriptor = ImageDitherReveal::new(start, Duration::from_secs(2));
    let mut source = Scene::default();
    source.insert_image_dither_reveal(sprite(8, 0.7), descriptor.clone());
    source.finish();
    source.sample_presentation_effects(start);
    source.did_submit_presentation_effects(start);
    source.sample_presentation_effects(start + Duration::from_secs(1));
    let mut destination = Scene::default();
    destination.insert_image_dither_reveal(
        sprite(3, 0.6),
        ImageDitherReveal::new(start, Duration::from_secs(4)),
    );
    destination.replay(0..source.len(), &source);
    destination.finish();
    source.clear();
    destination.sample_presentation_effects(start + Duration::from_secs(1));
    assert_eq!(destination.polychrome_sprites[1].tile.tile_id, TileId(8));
    assert_eq!(
        destination.polychrome_sprites[1].opacity_pulse_index,
        IMAGE_DITHER_REVEAL_FLAG | (IMAGE_DITHER_REVEAL_FULL_CUTOFF / 2)
    );
    assert_eq!(descriptor.started(), Some(start));
    destination.finish();
    destination.sample_presentation_effects(start + Duration::from_secs(2));
    assert_eq!(
        destination.polychrome_sprites[1].opacity_pulse_index,
        IMAGE_DITHER_REVEAL_FLAG | IMAGE_DITHER_REVEAL_FULL_CUTOFF
    );
    destination.did_submit_presentation_effects(start + Duration::from_secs(2));
    assert!(
        destination.has_presentation_effects(),
        "Independent second reveal keeps its own clock"
    );
}

#[test]
fn dither_reveal_sorting_and_resampling_preserve_pulse_static_opacity_and_geometry() {
    let start = Instant::now();
    let mut scene = Scene::default();
    scene.push_layer(Bounds::new(
        point(ScaledPixels(0.0), ScaledPixels(0.0)),
        size(ScaledPixels(64.0), ScaledPixels(64.0)),
    ));
    scene.insert_image_dither_reveal(
        sprite(9, 0.4),
        ImageDitherReveal::new(start, Duration::from_secs(2)),
    );
    scene.insert_primitive(sprite(3, 0.25));
    scene.insert_image_opacity_pulse(
        sprite(1, 0.7),
        ImageOpacityPulse {
            started: start,
            duration: Duration::from_secs(3),
            baseline: 0.88,
            amplitude: 0.12,
        },
    );
    scene.pop_layer();
    scene.finish();
    scene.sample_presentation_effects(start);
    scene.did_submit_presentation_effects(start);
    scene.sample_presentation_effects(start + Duration::from_secs(1));
    let original_geometry = scene.polychrome_sprites[2].bounds;
    assert_eq!(scene.polychrome_sprites[1].opacity, 0.25);
    assert_eq!(scene.polychrome_sprites[2].opacity, 0.4);
    assert_eq!(
        scene.polychrome_sprites[2].opacity_pulse_index,
        IMAGE_DITHER_REVEAL_FLAG | (IMAGE_DITHER_REVEAL_FULL_CUTOFF / 2)
    );
    scene.finish();
    scene.sample_presentation_effects(start + Duration::from_secs(2));
    scene.did_submit_presentation_effects(start);
    assert_eq!(scene.polychrome_sprites[2].bounds, original_geometry);
    assert!(
        scene.has_presentation_effects(),
        "Unchanged repeating opacity pulse remains live"
    );
    assert_eq!(scene.image_opacity_targets.len(), 1);
    assert_eq!(scene.image_dither_targets.len(), 1);
}

#[test]
fn dither_reveal_clipping_zero_opacity_zero_duration_and_clear_keep_no_frame_demand() {
    let start = Instant::now();
    for variant in 0..3 {
        let mut scene = Scene::default();
        let mut value = sprite(1, if variant == 1 { 0.0 } else { 1.0 });
        if variant == 0 {
            value.content_mask.bounds.origin = point(ScaledPixels(100.0), ScaledPixels(100.0));
        }
        let duration = if variant == 2 {
            Duration::ZERO
        } else {
            Duration::from_secs(2)
        };
        scene.insert_image_dither_reveal(value, ImageDitherReveal::new(start, duration));
        scene.finish();
        assert!(!scene.has_presentation_effects());
        assert!(!scene.has_image_presentation_effects());
        scene.sample_presentation_effects(start);
        scene.did_submit_presentation_effects(start);
    }
    let mut scene = Scene::default();
    scene.insert_image_dither_reveal(
        sprite(1, 1.0),
        ImageDitherReveal::new(start, Duration::from_secs(1)),
    );
    scene.finish();
    assert!(scene.has_presentation_effects());
    scene.clear();
    scene.finish();
    assert!(!scene.has_image_presentation_effects());
}

#[test]
fn dither_reveal_shader_cutoffs_match_original_f32_threshold_at_every_boundary() {
    let expected: [u32; 15] = [
        0, 1198373, 2396746, 3595118, 4793491, 5991863, 7190236, 8388608, 9586981, 10785353,
        11983726, 13182098, 14380471, 15578843, 16777216,
    ];
    for shader in [
        include_str!("platform/mac/shaders.metal"),
        include_str!("platform/blade/shaders.wgsl"),
        include_str!("platform/windows/shaders.hlsl"),
    ] {
        assert!(shader.contains("reveal_cutoff < 16777216u"),
            "All backends must preserve the full source-membership endpoint");
        assert!(shader.contains("sprite, reveal_cutoff, sample.a"),
            "All backends must pass the submitted cutoff directly to source alpha masking");
        assert!(!shader.contains("IMAGE_DITHER_REVEAL_CUTOFFS"),
            "Continuous presentation must not remap its cutoff through coarse steps");
    }
    for (step, cutoff) in expected.into_iter().enumerate() {
        let progress = step as f32 / 14.0;
        assert_eq!(cutoff, (progress * 16_777_216.0).ceil() as u32);
        for value in [
            0,
            cutoff.saturating_sub(1),
            cutoff,
            cutoff.saturating_add(1),
            16_777_215,
        ] {
            assert_eq!(value < cutoff, value as f32 / 16_777_216.0 < progress);
        }
    }
}

#[test]
fn continuous_dither_advances_each_60hz_presentation_during_600ms() {
    let started = Instant::now();
    let reveal = ImageDitherReveal::new(started, Duration::from_millis(600));
    let mut scene = Scene::default();
    scene.insert_image_dither_reveal(sprite(1, 1.0), reveal.clone());
    scene.finish();
    scene.sample_presentation_effects(started);
    scene.did_submit_presentation_effects(started);
    let mut previous = None;
    for frame in 0..=36u64 {
        let now = started + Duration::from_nanos(frame * 600_000_000 / 36);
        scene.sample_presentation_effects(now);
        let cutoff = scene.polychrome_sprites[0].opacity_pulse_index & !IMAGE_DITHER_REVEAL_FLAG;
        if let Some(previous) = previous { assert!(cutoff > previous, "Consecutive60Hz frames must advance the source-cell cutoff, frame={frame}"); }
        previous = Some(cutoff);
    }
    assert_eq!(previous, Some(IMAGE_DITHER_REVEAL_FULL_CUTOFF));
    assert!(scene.has_presentation_effects());
    scene.did_submit_presentation_effects(started + Duration::from_millis(600));
    assert!(!scene.has_presentation_effects());
}
