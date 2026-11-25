use super::*;
use crate::{AtlasTextureId, AtlasTextureKind, AtlasTile, DevicePixels, TileId, point, size};
use std::time::{Duration, Instant};

fn pulse(started: Instant) -> ImageOpacityPulse {
    ImageOpacityPulse {
        started,
        duration: Duration::from_secs(3),
        baseline: 0.88,
        amplitude: 0.12,
    }
}

fn legacy_value(started: Instant, now: Instant) -> f32 {
    let mut progress = now.saturating_duration_since(started).as_secs_f32() / 3.0;
    if progress > 1.0 {
        progress %= 1.0;
    }
    0.88 + 0.12 * (std::f32::consts::TAU * progress).cos()
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

fn base_opacity(scene: &Scene, id: u32) -> f32 {
    scene
        .paint_operations
        .iter()
        .find_map(|operation| match operation {
            PaintOperation::Primitive(Primitive::PolychromeSprite(value))
                if value.tile.tile_id == TileId(id) =>
            {
                Some(value.opacity)
            }
            _ => None,
        })
        .expect("original primitive retained")
}

#[test]
fn image_opacity_pulse_matches_legacy_f32_progress_wrap_and_epoch() {
    let started = Instant::now();
    let recipe = pulse(started);
    for micros in [
        0, 1, 16_667, 250_000, 750_000, 1_500_000, 2_999_999, 3_000_000, 3_000_001, 6_000_000,
        9_250_000, 60_000_000,
    ] {
        let now = started + Duration::from_micros(micros);
        assert_eq!(
            recipe.value_at(now).to_bits(),
            legacy_value(started, now).to_bits(),
            "elapsed {micros}"
        );
    }
    assert_eq!(
        recipe.value_at(started - Duration::from_secs(1)).to_bits(),
        legacy_value(started, started).to_bits()
    );
}

#[test]
fn image_opacity_pulse_ancestor_opacity_is_not_compounded_or_written_to_base() {
    let started = Instant::now();
    for base in [0.0, 0.17, 0.5, 1.0] {
        let mut scene = Scene::default();
        scene.insert_image_opacity_pulse(sprite(1, base), pulse(started));
        scene.finish();
        assert_eq!(
            scene.has_opacity_pulses(),
            base > 0.0,
            "zero parent opacity retains no active pulse demand"
        );
        for micros in [1_500_000, 750_000, 1_500_000, 3_000_000, 1_500_000] {
            let now = started + Duration::from_micros(micros);
            scene.sample_opacity_pulses(now);
            assert_eq!(
                scene.polychrome_sprites[0].opacity.to_bits(),
                (base * legacy_value(started, now)).to_bits()
            );
            assert_eq!(base_opacity(&scene, 1).to_bits(), base.to_bits());
        }
    }
}

#[test]
fn image_opacity_pulse_sorted_targets_preserve_static_sprites_and_geometry() {
    let started = Instant::now();
    let mut scene = Scene::default();
    scene.push_layer(Bounds::new(
        point(ScaledPixels(0.0), ScaledPixels(0.0)),
        size(ScaledPixels(64.0), ScaledPixels(64.0)),
    ));
    scene.insert_image_opacity_pulse(sprite(9, 0.4), pulse(started));
    scene.insert_primitive(sprite(3, 0.25));
    let mut second = pulse(started + Duration::from_millis(400));
    second.duration = Duration::from_secs(5);
    scene.insert_image_opacity_pulse(sprite(1, 0.7), second);
    scene.pop_layer();
    let geometry: Vec<_> = scene
        .polychrome_sprites
        .iter()
        .map(|s| {
            (
                s.tile.tile_id,
                s.bounds,
                s.content_mask.clone(),
                s.corner_radii,
            )
        })
        .collect();
    scene.finish();
    assert_eq!(
        scene
            .polychrome_sprites
            .iter()
            .map(|sprite| sprite.tile.tile_id.0)
            .collect::<Vec<_>>(),
        vec![1, 3, 9],
        "Actual batching sort must reorder pulsed and ordinary sprite instances"
    );
    let now = started + Duration::from_millis(1500);
    scene.sample_opacity_pulses(now);
    for value in &scene.polychrome_sprites {
        let expected = match value.tile.tile_id.0 {
            9 => 0.4 * pulse(started).value_at(now),
            1 => 0.7 * second.value_at(now),
            3 => 0.25,
            _ => unreachable!(),
        };
        assert_eq!(value.opacity.to_bits(), expected.to_bits());
        assert!(geometry.contains(&(
            value.tile.tile_id,
            value.bounds,
            value.content_mask.clone(),
            value.corner_radii
        )));
    }
}

#[test]
fn image_opacity_pulse_clipping_prevents_retained_demand_and_clear_retires_metadata() {
    let started = Instant::now();
    let mut scene = Scene::default();
    let mut outside = sprite(1, 1.0);
    outside.content_mask.bounds = Bounds::new(
        point(ScaledPixels(80.0), ScaledPixels(80.0)),
        size(ScaledPixels(8.0), ScaledPixels(8.0)),
    );
    scene.insert_image_opacity_pulse(outside, pulse(started));
    scene.finish();
    assert!(!scene.has_opacity_pulses());
    assert_eq!(scene.len(), 0);
    let partial = sprite(2, 0.6);
    scene.insert_image_opacity_pulse(partial, pulse(started));
    scene.finish();
    assert!(scene.has_opacity_pulses());
    scene.clear();
    scene.insert_primitive(sprite(3, 0.2));
    scene.finish();
    assert!(!scene.has_opacity_pulses());
    scene.sample_opacity_pulses(started + Duration::from_secs(1));
    assert_eq!(scene.polychrome_sprites.len(), 1);
    assert_eq!(scene.polychrome_sprites[0].opacity, 0.2);
}

#[test]
fn image_opacity_pulse_cached_replay_remaps_recipes_and_keeps_original_base_epoch() {
    let started = Instant::now();
    let mut source = Scene::default();
    source.insert_image_opacity_pulse(sprite(7, 0.35), pulse(started));
    source.finish();
    source.sample_opacity_pulses(started + Duration::from_millis(1500));
    let mut target = Scene::default();
    let other = ImageOpacityPulse {
        baseline: 0.5,
        amplitude: 0.25,
        duration: Duration::from_secs(5),
        started: started + Duration::from_millis(100),
    };
    target.insert_image_opacity_pulse(sprite(8, 0.8), other);
    target.replay(0..source.len(), &source);
    target.finish();
    let now = started + Duration::from_millis(750);
    target.sample_opacity_pulses(now);
    for value in &target.polychrome_sprites {
        let expected = if value.tile.tile_id == TileId(7) {
            0.35 * pulse(started).value_at(now)
        } else {
            0.8 * other.value_at(now)
        };
        assert_eq!(value.opacity.to_bits(), expected.to_bits());
    }
    assert_eq!(base_opacity(&target, 7), 0.35);
    source.clear();
    target.sample_opacity_pulses(now + Duration::from_secs(1));
    assert!(
        target.has_opacity_pulses(),
        "destination owns its recipe after source retirement"
    );
}

#[test]
fn image_opacity_pulse_index_reuses_original_gpu_abi_padding() {
    #[repr(C)]
    struct OriginalPolychromeSprite {
        order: DrawOrder,
        pad: u32,
        grayscale: bool,
        opacity: f32,
        bounds: Bounds<ScaledPixels>,
        content_mask: ContentMask<ScaledPixels>,
        corner_radii: Corners<ScaledPixels>,
        tile: AtlasTile,
    }
    assert_eq!(
        std::mem::size_of::<PolychromeSprite>(),
        std::mem::size_of::<OriginalPolychromeSprite>()
    );
    assert_eq!(
        std::mem::align_of::<PolychromeSprite>(),
        std::mem::align_of::<OriginalPolychromeSprite>()
    );
    assert_eq!(
        std::mem::offset_of!(PolychromeSprite, opacity_pulse_index),
        std::mem::offset_of!(OriginalPolychromeSprite, pad)
    );
    assert_eq!(
        std::mem::offset_of!(PolychromeSprite, opacity),
        std::mem::offset_of!(OriginalPolychromeSprite, opacity)
    );
    assert_eq!(
        std::mem::offset_of!(PolychromeSprite, bounds),
        std::mem::offset_of!(OriginalPolychromeSprite, bounds)
    );
    assert_eq!(
        std::mem::offset_of!(PolychromeSprite, tile),
        std::mem::offset_of!(OriginalPolychromeSprite, tile)
    );
}

#[test]
fn image_opacity_pulse_invalid_recipes_remain_original_and_constant_recipes_flatten() {
    let started = Instant::now();
    let mut constant = pulse(started);
    constant.amplitude = 0.0;
    let mut scene = Scene::default();
    scene.insert_image_opacity_pulse(sprite(1, 0.5), constant);
    scene.finish();
    assert!(!scene.has_opacity_pulses());
    assert_eq!(
        scene.polychrome_sprites[0].opacity.to_bits(),
        (0.5 * 0.88f32).to_bits()
    );
    let mut invalid = Vec::new();
    let mut value = pulse(started);
    value.duration = Duration::ZERO;
    invalid.push(value);
    for (baseline, amplitude) in [
        (f32::NAN, 0.12),
        (0.88, f32::INFINITY),
        (-0.1, 0.12),
        (1.1, 0.12),
        (0.05, 0.1),
        (0.9, 0.2),
    ] {
        let mut value = pulse(started);
        value.baseline = baseline;
        value.amplitude = amplitude;
        invalid.push(value);
    }
    for recipe in invalid {
        let mut scene = Scene::default();
        scene.insert_image_opacity_pulse(sprite(1, 0.5), recipe);
        scene.finish();
        scene.sample_opacity_pulses(started + Duration::from_secs(1));
        assert!(!scene.has_opacity_pulses());
        assert_eq!(scene.polychrome_sprites[0].opacity, 0.5);
    }
}
