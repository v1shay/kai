use super::*;
use crate::retained_properties::{GpuSpriteProperties, ResolvedProperties, progress};

pub(crate) const SPRITE_PROPERTIES_FLAG: u32 = 0x4000_0000;

#[derive(Clone)]
pub(super) struct SpritePropertyRecord {
    properties: ResolvedProperties,
    base_alpha: f32,
    gpu: GpuSpriteProperties,
}

impl Scene {
    #[cfg(test)]
    pub(super) fn sprite_property_epoch(&self, index: usize) -> Option<Instant> {
        self.sprite_properties
            .get(index)?
            .properties
            .opacity
            .map(|(_, started)| started)
    }

    pub(crate) fn insert_sprite_properties(
        &mut self,
        mut primitive: Primitive,
        properties: ResolvedProperties,
        origins: [[f32; 2]; 8],
    ) {
        // A contained image can start outside the viewport and enter it at a later
        // scheduled column. Keep its actual bounds while ordering by visible union.
        let content_mask = primitive.content_mask().bounds;
        let clipped_bounds = if let Some((frames, _)) = &properties.frames {
            let mut visible: Option<Bounds<ScaledPixels>> = None;
            for &column in frames.columns.iter() {
                let origin = origins[column as usize];
                let bounds = Bounds::new(point(ScaledPixels(origin[0]), ScaledPixels(origin[1])), primitive.bounds().size);
                let clipped = bounds.intersect(&content_mask);
                if !clipped.is_empty() {
                    visible = Some(visible.map_or(clipped, |previous| previous.union(&clipped)));
                }
            }
            let Some(visible) = visible else { return; };
            visible
        } else {
            primitive.bounds().intersect(&content_mask)
        };
        if clipped_bounds.is_empty() { return; }
        let alpha = match &primitive {
            Primitive::MonochromeSprite(sprite) => sprite.color.a,
            Primitive::PolychromeSprite(sprite) => sprite.opacity,
            _ => unreachable!("Retained properties require a sprite"),
        };
        let opacity_active = properties
            .opacity
            .is_some_and(|(animation, _)| animation.amplitude != 0.0);
        let frames_active = properties
            .frames
            .as_ref()
            .is_some_and(|(frames, _)| frames.animated());
        if alpha == 0.0 || (!opacity_active && !frames_active) {
            let factor = properties
                .opacity
                .map_or(1.0, |(animation, _)| animation.baseline);
            let origin = properties
                .frames
                .as_ref()
                .map(|(frames, _)| origins[frames.columns[0] as usize]);
            match &mut primitive {
                Primitive::MonochromeSprite(sprite) => sprite.color.a *= factor,
                Primitive::PolychromeSprite(sprite) => {
                    sprite.opacity *= factor;
                    if let Some(origin) = origin {
                        sprite.bounds.origin =
                            point(ScaledPixels(origin[0]), ScaledPixels(origin[1]));
                    }
                }
                _ => unreachable!(),
            }
            self.insert_primitive(primitive);
            return;
        }
        let mut gpu = GpuSpriteProperties {
            origins,
            ..Default::default()
        };
        if let Some((animation, _)) = properties.opacity {
            gpu.baseline = animation.baseline;
            gpu.amplitude = animation.amplitude;
            gpu.waveform = match animation.waveform {
                crate::OpacityWaveform::CosineTau => 1,
                crate::OpacityWaveform::HalfSinePi => 2,
            };
        }
        if let Some((frames, _)) = &properties.frames {
            gpu.selected_column = 0x8000_0000 | frames.columns[0];
            if frames.total <= (1 << 24) {
                gpu.total = frames.total as u32;
                gpu.frame_count = frames.columns.len() as u32;
                for (entry, (&duration, &column)) in gpu
                    .schedule
                    .iter_mut()
                    .zip(frames.durations.iter().zip(frames.columns.iter()))
                {
                    *entry = [duration as u32, column];
                }
            }
        }
        let index =
            u32::try_from(self.sprite_properties.len() + 1).expect("Too many retained sprites");
        assert!(index < SPRITE_PROPERTIES_FLAG);
        match &mut primitive {
            Primitive::MonochromeSprite(sprite) => sprite.pad = SPRITE_PROPERTIES_FLAG | index,
            Primitive::PolychromeSprite(sprite) => {
                sprite.opacity_pulse_index = SPRITE_PROPERTIES_FLAG | index
            }
            _ => unreachable!(),
        }
        self.sprite_properties
            .push(SpritePropertyRecord { properties, base_alpha: alpha, gpu });
        self.insert_primitive_with_clipped_bounds(primitive, clipped_bounds);
    }

    pub(super) fn sample_sprite_properties(&mut self, now: Instant) {
        for (record, gpu) in self
            .sprite_properties
            .iter()
            .zip(self.gpu_sprite_properties.iter_mut().skip(1))
        {
            if let Some((animation, started)) = record.properties.opacity {
                gpu.opacity_progress = progress(now, started, animation.period);
            }
            if let Some((frames, started)) = &record.properties.frames {
                gpu.frame_progress =
                    progress(now, *started, Duration::from_millis(frames.total));
                if gpu.frame_count == 0 {
                    gpu.selected_column =
                        0x8000_0000 | frames.column(gpu.frame_progress);
                }
            }
        }
        #[cfg(any(not(target_os = "macos"), feature = "macos-blade"))]
        self.sample_sprite_properties_scalar();
    }

    #[cfg(any(test, not(target_os = "macos"), feature = "macos-blade"))]
    fn sample_sprite_properties_scalar(&mut self) {
        for &(monochrome, sprite_index, record_index) in &self.sprite_property_targets {
            let record = &self.sprite_properties[record_index];
            let gpu = &self.gpu_sprite_properties[record_index + 1];
            let factor = record.properties.opacity.map_or(1.0, |(animation, _)| {
                animation.value(gpu.opacity_progress)
            });
            if monochrome {
                self.monochrome_sprites[sprite_index].color.a =
                    record.base_alpha * factor;
            } else {
                let sprite = &mut self.polychrome_sprites[sprite_index];
                sprite.opacity = record.base_alpha * factor;
                if let Some((frames, _)) = &record.properties.frames {
                    let origin =
                        record.gpu.origins[frames.column(gpu.frame_progress) as usize];
                    sprite.bounds.origin = point(ScaledPixels(origin[0]), ScaledPixels(origin[1]));
                }
            }
        }
    }

    pub(super) fn finish_sprite_properties(&mut self) {
        self.sprite_property_targets.clear();
        self.gpu_sprite_properties.clear();
        if self.sprite_properties.is_empty() { return; }
        self.gpu_sprite_properties
            .push(GpuSpriteProperties::default());
        self.gpu_sprite_properties
            .extend(self.sprite_properties.iter().map(|record| record.gpu));
        for (index, sprite) in self.monochrome_sprites.iter().enumerate() {
            if sprite.pad & SPRITE_PROPERTIES_FLAG != 0 {
                let record_index = (sprite.pad & !SPRITE_PROPERTIES_FLAG) as usize - 1;
                self.sprite_property_targets
                    .push((true, index, record_index));
            }
        }
        for (index, sprite) in self.polychrome_sprites.iter().enumerate() {
            if sprite.opacity_pulse_index & SPRITE_PROPERTIES_FLAG != 0
                && sprite.opacity_pulse_index & IMAGE_DITHER_REVEAL_FLAG == 0
            {
                let record_index =
                    (sprite.opacity_pulse_index & !SPRITE_PROPERTIES_FLAG) as usize - 1;
                self.sprite_property_targets
                    .push((false, index, record_index));
            }
        }
    }

    pub(super) fn replay_sprite_properties(
        &mut self,
        primitive: &Primitive,
        previous: &Scene,
    ) -> bool {
        let tag = match primitive {
            Primitive::MonochromeSprite(sprite) => sprite.pad,
            Primitive::PolychromeSprite(sprite) => sprite.opacity_pulse_index,
            _ => return false,
        };
        if tag & IMAGE_DITHER_REVEAL_FLAG != 0 || tag & SPRITE_PROPERTIES_FLAG == 0 {
            return false;
        }
        let record = &previous.sprite_properties[(tag & !SPRITE_PROPERTIES_FLAG) as usize - 1];
        self.insert_sprite_properties(
            primitive.clone(),
            record.properties.clone(),
            record.gpu.origins,
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AtlasTextureKind, DevicePixels, TileId, size, px, OpacityAnimation, OpacityWaveform};
    use crate::retained_properties::SpriteFrames;

    fn sprite(alpha: f32) -> PolychromeSprite {
        PolychromeSprite {
            order: 0, opacity_pulse_index: 0, grayscale: false, opacity: alpha,
            bounds: Bounds::new(point(ScaledPixels(10.0), ScaledPixels(8.0)), size(ScaledPixels(144.0), ScaledPixels(18.0))),
            content_mask: ContentMask { bounds: Bounds::new(point(ScaledPixels(10.0), ScaledPixels(8.0)), size(ScaledPixels(18.0), ScaledPixels(18.0))) },
            corner_radii: Corners::all(ScaledPixels(0.0)),
            tile: AtlasTile { texture_id: AtlasTextureId { index: 0, kind: AtlasTextureKind::Polychrome }, tile_id: TileId(1), padding: 0,
                bounds: Bounds::new(point(DevicePixels(0), DevicePixels(0)), size(DevicePixels(128), DevicePixels(16))) },
        }
    }

    fn properties(started: Instant, amplitude: f32, columns: Vec<usize>) -> ResolvedProperties {
        let count = columns.len();
        ResolvedProperties {
            opacity: Some((OpacityAnimation { period: Duration::from_millis(1300), baseline: 0.72, amplitude, waveform: OpacityWaveform::HalfSinePi }, started)),
            frames: Some((SpriteFrames::new(columns, vec![100; count], px(18.0)).unwrap(), started)),
        }
    }

    fn origins() -> [[f32; 2]; 8] {
        std::array::from_fn(|column| [10.0 - 18.0 * column as f32, 8.0])
    }

    #[test]
    fn repeated_presentation_preserves_original_primitives_and_replay_epochs() {
        let started = Instant::now();
        let mut scene = Scene::default();
        scene.insert_sprite_properties(sprite(0.5).into(), properties(started, 0.28, vec![0, 7, 2]), origins());
        scene.finish();
        assert!(scene.has_presentation_effects());
        for frame in 0..120 {
            scene.sample_presentation_effects(started + Duration::from_micros(frame * 16_667));
            let PaintOperation::Primitive(Primitive::PolychromeSprite(base)) = &scene.paint_operations[0] else { panic!() };
            assert_eq!(base.opacity, 0.5);
            assert_eq!(base.bounds.origin.x.0, 10.0);
        }
        let mut next = Scene::default();
        next.replay(0..scene.len(), &scene);
        next.finish();
        assert_eq!(next.sprite_properties.len(), 1);
        assert_eq!(next.sprite_property_epoch(0), Some(started));
        assert_eq!(next.gpu_sprite_properties.len(), 2);
        next.clear();
        assert!(!next.has_presentation_effects());
        assert!(next.gpu_sprite_properties.is_empty());
    }

    #[test]
    fn zero_alpha_identical_columns_constant_opacity_and_full_clipping_park() {
        let started = Instant::now();
        for (alpha, amplitude, columns, clipped) in [
            (0.0, 0.28, vec![0, 7], false),
            (0.5, 0.0, vec![7, 7, 7], false),
            (0.5, 0.0, vec![2], false),
            (0.5, 0.28, vec![0, 7], true),
        ] {
            let mut primitive = sprite(alpha);
            if clipped { primitive.content_mask.bounds.size = size(ScaledPixels(0.0), ScaledPixels(0.0)); }
            let mut scene = Scene::default();
            scene.insert_sprite_properties(primitive.into(), properties(started, amplitude, columns), origins());
            scene.finish();
            assert!(!scene.has_presentation_effects());
            assert!(scene.sprite_properties.is_empty());
            assert!(scene.gpu_sprite_properties.is_empty());
        }
    }

    #[test]
    fn huge_sheet_durations_use_exact_retained_column_without_a_layout_callback() {
        let started = Instant::now();
        let frames = SpriteFrames::new(vec![7, 0], vec![u64::MAX - 1, 1], px(18.0)).unwrap();
        let mut scene = Scene::default();
        scene.insert_sprite_properties(sprite(1.0).into(), ResolvedProperties { opacity: None, frames: Some((frames.clone(), started)) }, origins());
        scene.finish();
        scene.sample_presentation_effects(started + Duration::from_secs(60));
        let gpu = &scene.gpu_sprite_properties[1];
        assert_eq!(gpu.frame_count, 0);
        assert_eq!(gpu.selected_column & 7, frames.column(gpu.frame_progress));
        assert_eq!(gpu.selected_column & 0x8000_0000, 0x8000_0000);
        assert!(scene.has_presentation_effects());
    }
    #[test]
    fn scalar_backend_fallback_preserves_base_opacity_across_resort() {
        let started = Instant::now();
        let mut scene = Scene::default();
        scene.insert_sprite_properties(sprite(0.5).into(), properties(started, 0.28, vec![0, 7, 2]), origins());
        scene.finish();
        for micros in [0u64, 150_000, 290_000, 1_300_000, 2_600_001] {
            let now = started + Duration::from_micros(micros);
            scene.sample_presentation_effects(now);
            scene.sample_sprite_properties_scalar();
            let record = &scene.sprite_properties[0];
            let gpu = &scene.gpu_sprite_properties[1];
            let expected = 0.5 * record.properties.opacity.unwrap().0.value(gpu.opacity_progress);
            assert_eq!(scene.polychrome_sprites[0].opacity.to_bits(), expected.to_bits());
            let frames = &record.properties.frames.as_ref().unwrap().0;
            assert_eq!(scene.polychrome_sprites[0].bounds.origin.x.0,
                origins()[frames.column(gpu.frame_progress) as usize][0]);
            scene.finish();
        }
    }

    #[test]
    fn sheet_visibility_includes_later_columns_and_static_nonzero_columns() {
        let started = Instant::now();
        let mut table = [[100.0, 8.0]; 8];
        table[7] = [10.0, 8.0];
        for columns in [vec![0, 7], vec![7]] {
            let mut initial = sprite(1.0);
            initial.bounds.origin.x = ScaledPixels(100.0);
            initial.bounds.size.width = ScaledPixels(10.0);
            assert!(initial.bounds.intersect(&initial.content_mask.bounds).is_empty());
            let expected_animated = columns.len() > 1;
            let mut scene = Scene::default();
            scene.insert_sprite_properties(initial.into(), properties(started, 0.0, columns), table);
            scene.finish();
            assert_eq!(scene.polychrome_sprites.len(), 1);
            assert_eq!(scene.has_presentation_effects(), expected_animated);
            if !expected_animated { assert_eq!(scene.polychrome_sprites[0].bounds.origin.x.0, 10.0); }
            let mut replay = Scene::default();
            replay.replay(0..scene.len(), &scene);
            replay.finish();
            assert_eq!(replay.polychrome_sprites.len(), 1);
            assert_eq!(replay.has_presentation_effects(), expected_animated);
        }
        let mut scene = Scene::default();
        scene.insert_sprite_properties(sprite(1.0).into(), properties(started, 0.0, vec![0, 1]), table);
        scene.finish();
        assert!(scene.polychrome_sprites.is_empty());
        assert!(!scene.has_presentation_effects());
    }

    #[test]
    fn scalar_normal_scene_replay_preserves_mono_and_poly_base_across_generations() {
        let started = Instant::now();
        let mut scene = Scene::default();
        let source = sprite(0.5);
        let mut mono_tile = source.tile.clone();
        mono_tile.texture_id.kind = AtlasTextureKind::Monochrome;
        let mono = MonochromeSprite {
            order: 0, pad: 0, bounds: source.bounds, content_mask: source.content_mask.clone(),
            color: crate::Hsla::from(crate::rgb(0x91b3d2)).opacity(0.37),
            tile: mono_tile, transformation: crate::TransformationMatrix::unit(),
        };
        let mut mono_properties = properties(started, 0.28, vec![0]);
        mono_properties.frames = None;
        scene.insert_sprite_properties(mono.into(), mono_properties, [[0.0; 2]; 8]);
        scene.insert_sprite_properties(source.into(), properties(started, 0.28, vec![0, 7, 2]), origins());
        scene.finish();
        for generation in 0..8 {
            let now = started + Duration::from_millis(175 + generation * 167);
            scene.sample_presentation_effects(now);
            scene.sample_sprite_properties_scalar();
            let expected_wave = properties(started, 0.28, vec![0]).opacity.unwrap().0.value(
                progress(now, started, Duration::from_millis(1300)));
            assert_eq!(scene.monochrome_sprites[0].color.a.to_bits(), (0.37 * expected_wave).to_bits());
            assert_eq!(scene.polychrome_sprites[0].opacity.to_bits(), (0.5 * expected_wave).to_bits());
            let frames = SpriteFrames::new(vec![0, 7, 2], vec![100; 3], px(18.0)).unwrap();
            let expected_column = frames.column(progress(now, started, Duration::from_millis(300)));
            assert_eq!(scene.polychrome_sprites[0].bounds.origin.x.0, origins()[expected_column as usize][0]);
            // Exercise the production entry point. Its immutable paint operations
            // remain the replay source after scalar presentation changes instances.
            let mut next = Scene::default();
            next.replay(0..scene.len(), &scene);
            next.finish();
            assert_eq!(next.sprite_properties.len(), 2);
            assert_eq!(next.sprite_properties[0].base_alpha.to_bits(), 0.37f32.to_bits());
            assert_eq!(next.sprite_properties[1].base_alpha.to_bits(), 0.5f32.to_bits());
            assert_eq!(next.sprite_property_epoch(0), Some(started));
            assert_eq!(next.sprite_property_epoch(1), Some(started));
            scene = next;
        }
    }

}
