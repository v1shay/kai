// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AtlasTextureId, AtlasTile, Background, Bounds, ContentMask, Corners, Edges, Hsla, Pixels,
    Point, Radians, ScaledPixels, Size, bounds_tree::BoundsTree, point,
};
use std::{
    fmt::Debug,
    iter::Peekable,
    ops::{Add, Range, Sub},
    slice,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

#[allow(non_camel_case_types, unused)]
pub(crate) type PathVertex_ScaledPixels = PathVertex<ScaledPixels>;

pub(crate) type DrawOrder = u32;
#[path = "scene_retained_properties.rs"]
mod retained;
#[path = "scene_quad_shimmer.rs"]
mod quad_shimmer;
use quad_shimmer::QuadShimmerRecord;
use crate::retained_properties::GpuSpriteProperties;
use retained::{SPRITE_PROPERTIES_FLAG, SpritePropertyRecord};

pub(crate) const IMAGE_DITHER_REVEAL_FLAG: u32 = 0x8000_0000;
pub(crate) const IMAGE_DITHER_REVEAL_FULL_CUTOFF: u32 = 1 << 24;

/// A finite source-texel dither reveal anchored to its first submitted frame.
#[derive(Clone, Debug)]
pub struct ImageDitherReveal {
    not_before: Instant,
    duration: Duration,
    started: Arc<OnceLock<Instant>>,
}

impl ImageDitherReveal {
    /// Share one descriptor across image replacements and cached paint replay.
    pub fn new(not_before: Instant, duration: Duration) -> Self {
        Self {
            not_before,
            duration,
            started: Arc::new(OnceLock::new()),
        }
    }

    /// The first submitted frame's start, including any requested future delay.
    pub fn started(&self) -> Option<Instant> {
        self.started.get().copied()
    }

    /// The earliest allowed start of the reveal.
    pub fn not_before(&self) -> Instant {
        self.not_before
    }

    /// Time from a fully hidden image to its unchanged original pixels.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    fn effective_start_at(&self, now: Instant) -> Instant {
        self.started().unwrap_or_else(|| self.not_before.max(now))
    }

    pub(crate) fn cutoff_at(&self, now: Instant) -> u32 {
        if self.duration.is_zero() {
            return IMAGE_DITHER_REVEAL_FULL_CUTOFF;
        }
        let progress = now
            .saturating_duration_since(self.effective_start_at(now))
            .as_secs_f32()
            / self.duration.as_secs_f32();
        (progress.clamp(0.0, 1.0) * IMAGE_DITHER_REVEAL_FULL_CUTOFF as f32).ceil() as u32
    }
}

#[derive(Clone)]
struct ImageDitherRevealRecord {
    reveal: ImageDitherReveal,
    submitted_complete: bool,
}

/// An image-only cosine pulse. The epoch survives cached paint replay.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ImageOpacityPulse {
    pub started: Instant,
    pub duration: Duration,
    pub baseline: f32,
    pub amplitude: f32,
}

impl ImageOpacityPulse {
    pub(crate) fn parameters_valid(duration: Duration, baseline: f32, amplitude: f32) -> bool {
        !duration.is_zero()
            && baseline.is_finite()
            && amplitude.is_finite()
            && baseline - amplitude.abs() >= 0.0
            && baseline + amplitude.abs() <= 1.0
    }

    pub(crate) fn value_at(self, now: Instant) -> f32 {
        let mut progress =
            now.saturating_duration_since(self.started).as_secs_f32() / self.duration.as_secs_f32();
        if progress > 1.0 {
            progress %= 1.0;
        }
        self.baseline + self.amplitude * (std::f32::consts::TAU * progress).cos()
    }
}

#[derive(Clone, Copy)]
struct ImageOpacityPulseRecord {
    pulse: ImageOpacityPulse,
    base_opacity: f32,
}

#[derive(Default)]
pub(crate) struct Scene {
    pub(crate) paint_operations: Vec<PaintOperation>,
    primitive_bounds: BoundsTree<ScaledPixels>,
    layer_stack: Vec<DrawOrder>,
    pub(crate) shadows: Vec<Shadow>,
    pub(crate) quads: Vec<Quad>,
    #[cfg(target_os = "macos")]
    pub(crate) has_backdrop_blur: bool,
    pub(crate) paths: Vec<Path<ScaledPixels>>,
    pub(crate) underlines: Vec<Underline>,
    pub(crate) monochrome_sprites: Vec<MonochromeSprite>,
    pub(crate) polychrome_sprites: Vec<PolychromeSprite>,
    pub(crate) surfaces: Vec<PaintSurface>,
    quad_shimmers: Vec<QuadShimmerRecord>,
    quad_shimmer_targets: Vec<(usize, usize)>,
    sprite_properties: Vec<SpritePropertyRecord>,
    sprite_property_targets: Vec<(bool, usize, usize)>,
    pub(crate) gpu_sprite_properties: Vec<GpuSpriteProperties>,
    image_opacity_pulses: Vec<ImageOpacityPulseRecord>,
    image_opacity_targets: Vec<(usize, usize)>,
    image_dither_reveals: Vec<ImageDitherRevealRecord>,
    image_dither_targets: Vec<(usize, usize)>,
}

impl Scene {
    pub fn clear(&mut self) {
        self.paint_operations.clear();
        self.primitive_bounds.clear();
        self.layer_stack.clear();
        self.paths.clear();
        self.shadows.clear();
        self.quads.clear();
        #[cfg(target_os = "macos")]
        { self.has_backdrop_blur = false; }
        self.underlines.clear();
        self.monochrome_sprites.clear();
        self.polychrome_sprites.clear();
        self.surfaces.clear();
        self.quad_shimmers.clear();
        self.quad_shimmer_targets.clear();
        self.sprite_properties.clear();
        self.sprite_property_targets.clear();
        self.gpu_sprite_properties.clear();
        self.image_opacity_pulses.clear();
        self.image_opacity_targets.clear();
        self.image_dither_reveals.clear();
        self.image_dither_targets.clear();
    }

    pub fn len(&self) -> usize {
        self.paint_operations.len()
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        let order = self.primitive_bounds.insert(bounds);
        self.layer_stack.push(order);
        self.paint_operations
            .push(PaintOperation::StartLayer(bounds));
    }

    pub fn pop_layer(&mut self) {
        self.layer_stack.pop();
        self.paint_operations.push(PaintOperation::EndLayer);
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        let primitive = primitive.into();
        let clipped_bounds = primitive.bounds().intersect(&primitive.content_mask().bounds);
        self.insert_primitive_with_clipped_bounds(primitive, clipped_bounds);
    }

    fn insert_primitive_with_clipped_bounds(
        &mut self,
        mut primitive: Primitive,
        clipped_bounds: Bounds<ScaledPixels>,
    ) {
        if clipped_bounds.is_empty() {
            return;
        }

        let order = self
            .layer_stack
            .last()
            .copied()
            .unwrap_or_else(|| self.primitive_bounds.insert(clipped_bounds));
        match &mut primitive {
            Primitive::Shadow(shadow) => {
                shadow.order = order;
                self.shadows.push(shadow.clone());
            }
            Primitive::Quad(quad) => {
                quad.order = order;
                #[cfg(target_os = "macos")]
                if quad.background.tag == crate::color::BackgroundTag::BackdropBlur {
                    self.has_backdrop_blur = true;
                }
                self.quads.push(quad.clone());
            }
            Primitive::Path(path) => {
                path.order = order;
                path.id = PathId(self.paths.len());
                self.paths.push(path.clone());
            }
            Primitive::Underline(underline) => {
                underline.order = order;
                self.underlines.push(underline.clone());
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.order = order;
                self.monochrome_sprites.push(sprite.clone());
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.order = order;
                self.polychrome_sprites.push(sprite.clone());
            }
            Primitive::Surface(surface) => {
                surface.order = order;
                self.surfaces.push(surface.clone());
            }
        }
        self.paint_operations
            .push(PaintOperation::Primitive(primitive));
    }

    /// Keep replayable paint operations at their original inherited opacity.
    /// Only the finished render array is sampled by the presentation path.
    pub(crate) fn insert_image_opacity_pulse(
        &mut self,
        mut sprite: PolychromeSprite,
        pulse: ImageOpacityPulse,
    ) {
        if sprite
            .bounds
            .intersect(&sprite.content_mask.bounds)
            .is_empty()
        {
            return;
        }
        sprite.opacity_pulse_index = 0;
        if !ImageOpacityPulse::parameters_valid(pulse.duration, pulse.baseline, pulse.amplitude) {
            self.insert_primitive(sprite);
            return;
        }
        if pulse.amplitude == 0.0 || sprite.opacity == 0.0 {
            sprite.opacity *= pulse.baseline;
            self.insert_primitive(sprite);
            return;
        }
        let index = u32::try_from(self.image_opacity_pulses.len() + 1)
            .expect("Image opacity pulse count exceeds instance index capacity");
        assert!(
            index < IMAGE_DITHER_REVEAL_FLAG,
            "Image opacity pulse index exceeds tag capacity"
        );
        self.image_opacity_pulses.push(ImageOpacityPulseRecord {
            pulse,
            base_opacity: sprite.opacity,
        });
        sprite.opacity_pulse_index = index;
        self.insert_primitive(sprite);
    }

    pub(crate) fn insert_image_dither_reveal(
        &mut self,
        mut sprite: PolychromeSprite,
        reveal: ImageDitherReveal,
    ) {
        if sprite
            .bounds
            .intersect(&sprite.content_mask.bounds)
            .is_empty()
        {
            return;
        }
        sprite.opacity_pulse_index = 0;
        if reveal.duration.is_zero() || sprite.opacity == 0.0 {
            self.insert_primitive(sprite);
            return;
        }
        let index = u32::try_from(self.image_dither_reveals.len() + 1)
            .expect("Image dither reveal count exceeds instance capacity");
        assert!(
            index < IMAGE_DITHER_REVEAL_FLAG,
            "Image dither index exceeds tag capacity"
        );
        self.image_dither_reveals.push(ImageDitherRevealRecord {
            reveal,
            submitted_complete: false,
        });
        sprite.opacity_pulse_index = IMAGE_DITHER_REVEAL_FLAG | index;
        self.insert_primitive(sprite);
    }

    pub(crate) fn has_image_presentation_effects(&self) -> bool {
        if !self.quad_shimmer_targets.is_empty() { return true; }
        !self.sprite_property_targets.is_empty()
            || self.has_opacity_pulses() || !self.image_dither_targets.is_empty()
    }

    pub(crate) fn has_presentation_effects(&self) -> bool {
        if !self.quad_shimmer_targets.is_empty() { return true; }
        !self.sprite_property_targets.is_empty()
            || self.has_opacity_pulses()
            || self
                .image_dither_targets
                .iter()
                .any(|&(_, index)| !self.image_dither_reveals[index].submitted_complete)
    }

    pub(crate) fn has_active_presentation_effects(&self, now: Instant) -> bool {
        if self.quad_shimmers_active_at(now) { return true; }
        !self.sprite_property_targets.is_empty()
            || self.has_opacity_pulses()
            || self.image_dither_targets.iter().any(|&(_, index)| {
                let record = &self.image_dither_reveals[index];
                !record.submitted_complete && record.reveal.cutoff_at(now) < IMAGE_DITHER_REVEAL_FULL_CUTOFF
            })
    }

    pub(crate) fn sample_presentation_effects(&mut self, now: Instant) {
        self.sample_quad_shimmers(now);
        self.sample_sprite_properties(now);
        self.sample_opacity_pulses(now);
        for &(sprite_index, reveal_index) in &self.image_dither_targets {
            let record = &mut self.image_dither_reveals[reveal_index];
            let cutoff = record.reveal.cutoff_at(now);
            self.polychrome_sprites[sprite_index].opacity_pulse_index =
                IMAGE_DITHER_REVEAL_FLAG | cutoff;
        }
    }

    pub(crate) fn did_submit_presentation_effects(&mut self, submitted_at: Instant) {
        self.did_submit_quad_shimmers();
        for &(sprite_index, reveal_index) in &self.image_dither_targets {
            let record = &mut self.image_dither_reveals[reveal_index];
            record
                .reveal
                .started
                .get_or_init(|| record.reveal.not_before.max(submitted_at));
            let cutoff = self.polychrome_sprites[sprite_index].opacity_pulse_index
                & !IMAGE_DITHER_REVEAL_FLAG;
            if cutoff >= IMAGE_DITHER_REVEAL_FULL_CUTOFF {
                record.submitted_complete = true;
            }
        }
    }

    pub(crate) fn has_opacity_pulses(&self) -> bool {
        !self.sprite_property_targets.is_empty() || !self.image_opacity_targets.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn image_opacity_pulse_epoch(&self, index: usize) -> Option<Instant> {
        self.image_opacity_pulses
            .get(index)
            .map(|record| record.pulse.started)
            .or_else(|| self.sprite_property_epoch(index))
    }

    pub(crate) fn sample_opacity_pulses(&mut self, now: Instant) {
        for &(sprite_index, pulse_index) in &self.image_opacity_targets {
            let record = &self.image_opacity_pulses[pulse_index];
            self.polychrome_sprites[sprite_index].opacity =
                record.base_opacity * record.pulse.value_at(now);
        }
    }

    pub fn replay(&mut self, range: Range<usize>, prev_scene: &Scene) {
        for operation in &prev_scene.paint_operations[range] {
            if let PaintOperation::Primitive(primitive) = operation {
                if self.replay_quad_shimmer(primitive, prev_scene) || self.replay_sprite_properties(primitive, prev_scene) {
                    continue;
                }
            }
            match operation {
                PaintOperation::Primitive(Primitive::PolychromeSprite(sprite))
                    if sprite.opacity_pulse_index & IMAGE_DITHER_REVEAL_FLAG != 0 =>
                {
                    let mut sprite = sprite.clone();
                    let source_index =
                        (sprite.opacity_pulse_index & !IMAGE_DITHER_REVEAL_FLAG) as usize - 1;
                    if let Some(record) = prev_scene.image_dither_reveals.get(source_index) {
                        let inserted_index = self.image_dither_reveals.len();
                        self.insert_image_dither_reveal(sprite, record.reveal.clone());
                        if let Some(inserted) = self.image_dither_reveals.get_mut(inserted_index) {
                            inserted.submitted_complete = record.submitted_complete;
                        }
                    } else {
                        sprite.opacity_pulse_index = 0;
                        self.insert_primitive(sprite);
                    }
                }
                PaintOperation::Primitive(Primitive::PolychromeSprite(sprite))
                    if sprite.opacity_pulse_index != 0 =>
                {
                    let mut sprite = sprite.clone();
                    let source_index = sprite.opacity_pulse_index as usize - 1;
                    if let Some(record) = prev_scene.image_opacity_pulses.get(source_index) {
                        // The stored operation is base opacity, even after many presents.
                        sprite.opacity = record.base_opacity;
                        self.insert_image_opacity_pulse(sprite, record.pulse);
                    } else {
                        sprite.opacity_pulse_index = 0;
                        self.insert_primitive(sprite);
                    }
                }
                PaintOperation::Primitive(primitive) => self.insert_primitive(primitive.clone()),
                PaintOperation::StartLayer(bounds) => self.push_layer(*bounds),
                PaintOperation::EndLayer => self.pop_layer(),
            }
        }
    }

    pub fn finish(&mut self) {
        // Restore recipe tags before sorting an already sampled scene again.
        for &(sprite_index, reveal_index) in &self.image_dither_targets {
            self.polychrome_sprites[sprite_index].opacity_pulse_index =
                IMAGE_DITHER_REVEAL_FLAG | (reveal_index as u32 + 1);
        }
        self.shadows.sort_by_key(|shadow| shadow.order);
        self.quads.sort_by_key(|quad| quad.order);
        self.paths.sort_by_key(|path| path.order);
        self.underlines.sort_by_key(|underline| underline.order);
        self.monochrome_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.polychrome_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.surfaces.sort_by_key(|surface| surface.order);
        self.image_opacity_targets.clear();
        self.image_dither_targets.clear();
        for (sprite_index, sprite) in self.polychrome_sprites.iter().enumerate() {
            if sprite.opacity_pulse_index & IMAGE_DITHER_REVEAL_FLAG != 0 {
                let reveal_index =
                    (sprite.opacity_pulse_index & !IMAGE_DITHER_REVEAL_FLAG) as usize - 1;
                if reveal_index < self.image_dither_reveals.len() {
                    self.image_dither_targets.push((sprite_index, reveal_index));
                }
            } else if sprite.opacity_pulse_index != 0 && sprite.opacity_pulse_index & SPRITE_PROPERTIES_FLAG == 0
            {
                let pulse_index = sprite.opacity_pulse_index as usize - 1;
                if pulse_index < self.image_opacity_pulses.len() {
                    self.image_opacity_targets.push((sprite_index, pulse_index));
                }
            }
        }
    self.finish_quad_shimmers();
        self.finish_sprite_properties();
    }

    #[cfg_attr(
        all(
            any(target_os = "linux", target_os = "freebsd"),
            not(any(feature = "x11", feature = "wayland"))
        ),
        allow(dead_code)
    )]
    pub(crate) fn batches(&self) -> impl Iterator<Item = PrimitiveBatch<'_>> {
        BatchIterator {
            shadows: &self.shadows,
            shadows_start: 0,
            shadows_iter: self.shadows.iter().peekable(),
            quads: &self.quads,
            quads_start: 0,
            quads_iter: self.quads.iter().peekable(),
            paths: &self.paths,
            paths_start: 0,
            paths_iter: self.paths.iter().peekable(),
            underlines: &self.underlines,
            underlines_start: 0,
            underlines_iter: self.underlines.iter().peekable(),
            monochrome_sprites: &self.monochrome_sprites,
            monochrome_sprites_start: 0,
            monochrome_sprites_iter: self.monochrome_sprites.iter().peekable(),
            polychrome_sprites: &self.polychrome_sprites,
            polychrome_sprites_start: 0,
            polychrome_sprites_iter: self.polychrome_sprites.iter().peekable(),
            surfaces: &self.surfaces,
            surfaces_start: 0,
            surfaces_iter: self.surfaces.iter().peekable(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Default)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveKind {
    Shadow,
    #[default]
    Quad,
    Path,
    Underline,
    MonochromeSprite,
    PolychromeSprite,
    Surface,
}

pub(crate) enum PaintOperation {
    Primitive(Primitive),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
}

#[derive(Clone)]
pub(crate) enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Path(Path<ScaledPixels>),
    Underline(Underline),
    MonochromeSprite(MonochromeSprite),
    PolychromeSprite(PolychromeSprite),
    Surface(PaintSurface),
}

impl Primitive {
    pub fn bounds(&self) -> &Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.bounds,
            Primitive::Quad(quad) => &quad.bounds,
            Primitive::Path(path) => &path.bounds,
            Primitive::Underline(underline) => &underline.bounds,
            Primitive::MonochromeSprite(sprite) => &sprite.bounds,
            Primitive::PolychromeSprite(sprite) => &sprite.bounds,
            Primitive::Surface(surface) => &surface.bounds,
        }
    }

    pub fn content_mask(&self) -> &ContentMask<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.content_mask,
            Primitive::Quad(quad) => &quad.content_mask,
            Primitive::Path(path) => &path.content_mask,
            Primitive::Underline(underline) => &underline.content_mask,
            Primitive::MonochromeSprite(sprite) => &sprite.content_mask,
            Primitive::PolychromeSprite(sprite) => &sprite.content_mask,
            Primitive::Surface(surface) => &surface.content_mask,
        }
    }
}

#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
struct BatchIterator<'a> {
    shadows: &'a [Shadow],
    shadows_start: usize,
    shadows_iter: Peekable<slice::Iter<'a, Shadow>>,
    quads: &'a [Quad],
    quads_start: usize,
    quads_iter: Peekable<slice::Iter<'a, Quad>>,
    paths: &'a [Path<ScaledPixels>],
    paths_start: usize,
    paths_iter: Peekable<slice::Iter<'a, Path<ScaledPixels>>>,
    underlines: &'a [Underline],
    underlines_start: usize,
    underlines_iter: Peekable<slice::Iter<'a, Underline>>,
    monochrome_sprites: &'a [MonochromeSprite],
    monochrome_sprites_start: usize,
    monochrome_sprites_iter: Peekable<slice::Iter<'a, MonochromeSprite>>,
    polychrome_sprites: &'a [PolychromeSprite],
    polychrome_sprites_start: usize,
    polychrome_sprites_iter: Peekable<slice::Iter<'a, PolychromeSprite>>,
    surfaces: &'a [PaintSurface],
    surfaces_start: usize,
    surfaces_iter: Peekable<slice::Iter<'a, PaintSurface>>,
}

impl<'a> Iterator for BatchIterator<'a> {
    type Item = PrimitiveBatch<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut orders_and_kinds = [
            (
                self.shadows_iter.peek().map(|s| s.order),
                PrimitiveKind::Shadow,
            ),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (self.paths_iter.peek().map(|q| q.order), PrimitiveKind::Path),
            (
                self.underlines_iter.peek().map(|u| u.order),
                PrimitiveKind::Underline,
            ),
            (
                self.monochrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::MonochromeSprite,
            ),
            (
                self.polychrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::PolychromeSprite,
            ),
            (
                self.surfaces_iter.peek().map(|s| s.order),
                PrimitiveKind::Surface,
            ),
        ];
        orders_and_kinds.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));

        let first = orders_and_kinds[0];
        let second = orders_and_kinds[1];
        let (batch_kind, max_order_and_kind) = if first.0.is_some() {
            (first.1, (second.0.unwrap_or(u32::MAX), second.1))
        } else {
            return None;
        };

        match batch_kind {
            PrimitiveKind::Shadow => {
                let shadows_start = self.shadows_start;
                let mut shadows_end = shadows_start + 1;
                self.shadows_iter.next();
                while self
                    .shadows_iter
                    .next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(
                    &self.shadows[shadows_start..shadows_end],
                ))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self
                    .quads_iter
                    .next_if(|quad| (quad.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(&self.quads[quads_start..quads_end]))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                self.paths_iter.next();
                while self
                    .paths_iter
                    .next_if(|path| (path.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(&self.paths[paths_start..paths_end]))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self
                    .underlines_iter
                    .next_if(|underline| (underline.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(
                    &self.underlines[underlines_start..underlines_end],
                ))
            }
            PrimitiveKind::MonochromeSprite => {
                let texture_id = self.monochrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.monochrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.monochrome_sprites_iter.next();
                while self
                    .monochrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.monochrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    sprites: &self.monochrome_sprites[sprites_start..sprites_end],
                })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id = self.polychrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = self.polychrome_sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self
                    .polychrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    sprites: &self.polychrome_sprites[sprites_start..sprites_end],
                })
            }
            PrimitiveKind::Surface => {
                let surfaces_start = self.surfaces_start;
                let mut surfaces_end = surfaces_start + 1;
                self.surfaces_iter.next();
                while self
                    .surfaces_iter
                    .next_if(|surface| (surface.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    surfaces_end += 1;
                }
                self.surfaces_start = surfaces_end;
                Some(PrimitiveBatch::Surfaces(
                    &self.surfaces[surfaces_start..surfaces_end],
                ))
            }
        }
    }
}

#[derive(Debug)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveBatch<'a> {
    Shadows(&'a [Shadow]),
    Quads(&'a [Quad]),
    Paths(&'a [Path<ScaledPixels>]),
    Underlines(&'a [Underline]),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        sprites: &'a [MonochromeSprite],
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        sprites: &'a [PolychromeSprite],
    },
    Surfaces(&'a [PaintSurface]),
}

#[derive(Default, Debug, Clone)]
#[repr(C)]
pub(crate) struct Quad {
    pub order: DrawOrder,
    pub border_style: BorderStyle,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub background: Background,
    pub border_color: Hsla,
    pub corner_radii: Corners<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct Underline {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub thickness: ScaledPixels,
    pub wavy: u32,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
}

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
}

/// The style of a border.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub enum BorderStyle {
    /// A solid border.
    #[default]
    Solid = 0,
    /// A dashed border.
    Dashed = 1,
}

/// A data type representing a 2 dimensional transformation that can be applied to an element.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct TransformationMatrix {
    /// 2x2 matrix containing rotation and scale,
    /// stored row-major
    pub rotation_scale: [[f32; 2]; 2],
    /// translation vector
    pub translation: [f32; 2],
}

impl Eq for TransformationMatrix {}

impl TransformationMatrix {
    /// The unit matrix, has no effect.
    pub fn unit() -> Self {
        Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [0.0, 0.0],
        }
    }

    /// Move the origin by a given point
    pub fn translate(mut self, point: Point<ScaledPixels>) -> Self {
        self.compose(Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [point.x.0, point.y.0],
        })
    }

    /// Clockwise rotation in radians around the origin
    pub fn rotate(self, angle: Radians) -> Self {
        self.compose(Self {
            rotation_scale: [
                [angle.0.cos(), -angle.0.sin()],
                [angle.0.sin(), angle.0.cos()],
            ],
            translation: [0.0, 0.0],
        })
    }

    /// Scale around the origin
    pub fn scale(self, size: Size<f32>) -> Self {
        self.compose(Self {
            rotation_scale: [[size.width, 0.0], [0.0, size.height]],
            translation: [0.0, 0.0],
        })
    }

    /// Perform matrix multiplication with another transformation
    /// to produce a new transformation that is the result of
    /// applying both transformations: first, `other`, then `self`.
    #[inline]
    pub fn compose(self, other: TransformationMatrix) -> TransformationMatrix {
        if other == Self::unit() {
            return self;
        }
        // Perform matrix multiplication
        TransformationMatrix {
            rotation_scale: [
                [
                    self.rotation_scale[0][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][0],
                    self.rotation_scale[0][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][1],
                ],
                [
                    self.rotation_scale[1][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][0],
                    self.rotation_scale[1][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][1],
                ],
            ],
            translation: [
                self.translation[0]
                    + self.rotation_scale[0][0] * other.translation[0]
                    + self.rotation_scale[0][1] * other.translation[1],
                self.translation[1]
                    + self.rotation_scale[1][0] * other.translation[0]
                    + self.rotation_scale[1][1] * other.translation[1],
            ],
        }
    }

    /// Apply transformation to a point, mainly useful for debugging
    pub fn apply(&self, point: Point<Pixels>) -> Point<Pixels> {
        let input = [point.x.0, point.y.0];
        let mut output = self.translation;
        for (i, output_cell) in output.iter_mut().enumerate() {
            for (k, input_cell) in input.iter().enumerate() {
                *output_cell += self.rotation_scale[i][k] * *input_cell;
            }
        }
        Point::new(output[0].into(), output[1].into())
    }
}

impl Default for TransformationMatrix {
    fn default() -> Self {
        Self::unit()
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct MonochromeSprite {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub tile: AtlasTile,
    pub transformation: TransformationMatrix,
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct PolychromeSprite {
    pub order: DrawOrder,
    // Occupies the former unused padding slot. GPU shaders do not read it.
    pub opacity_pulse_index: u32,
    pub grayscale: bool,
    pub opacity: f32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub tile: AtlasTile,
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PaintSurface {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    #[cfg(target_os = "macos")]
    pub image_buffer: core_video::pixel_buffer::CVPixelBuffer,
}

impl From<PaintSurface> for Primitive {
    fn from(surface: PaintSurface) -> Self {
        Primitive::Surface(surface)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PathId(pub(crate) usize);

/// A line made up of a series of vertices and control points.
#[derive(Clone, Debug)]
pub struct Path<P: Clone + Debug + Default + PartialEq> {
    pub(crate) id: PathId,
    pub(crate) order: DrawOrder,
    pub(crate) bounds: Bounds<P>,
    pub(crate) content_mask: ContentMask<P>,
    pub(crate) vertices: Vec<PathVertex<P>>,
    pub(crate) color: Background,
    start: Point<P>,
    current: Point<P>,
    contour_count: usize,
}

impl Path<Pixels> {
    /// Create a new path with the given starting point.
    pub fn new(start: Point<Pixels>) -> Self {
        Self {
            id: PathId(0),
            order: DrawOrder::default(),
            vertices: Vec::new(),
            start,
            current: start,
            bounds: Bounds {
                origin: start,
                size: Default::default(),
            },
            content_mask: Default::default(),
            color: Default::default(),
            contour_count: 0,
        }
    }

    /// Scale this path by the given factor.
    pub fn scale(&self, factor: f32) -> Path<ScaledPixels> {
        Path {
            id: self.id,
            order: self.order,
            bounds: self.bounds.scale(factor),
            content_mask: self.content_mask.scale(factor),
            vertices: self
                .vertices
                .iter()
                .map(|vertex| vertex.scale(factor))
                .collect(),
            start: self.start.map(|start| start.scale(factor)),
            current: self.current.scale(factor),
            contour_count: self.contour_count,
            color: self.color,
        }
    }

    /// Move the start, current point to the given point.
    pub fn move_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        self.start = to;
        self.current = to;
    }

    /// Draw a straight line from the current point to the given point.
    pub fn line_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }
        self.current = to;
    }

    /// Draw a curve from the current point to the given point, using the given control point.
    pub fn curve_to(&mut self, to: Point<Pixels>, ctrl: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }

        self.push_triangle(
            (self.current, ctrl, to),
            (point(0., 0.), point(0.5, 0.), point(1., 1.)),
        );
        self.current = to;
    }

    /// Push a triangle to the Path.
    pub fn push_triangle(
        &mut self,
        xy: (Point<Pixels>, Point<Pixels>, Point<Pixels>),
        st: (Point<f32>, Point<f32>, Point<f32>),
    ) {
        self.bounds = self
            .bounds
            .union(&Bounds {
                origin: xy.0,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.1,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.2,
                size: Default::default(),
            });

        self.vertices.push(PathVertex {
            xy_position: xy.0,
            st_position: st.0,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.1,
            st_position: st.1,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.2,
            st_position: st.2,
            content_mask: Default::default(),
        });
    }
}

impl<T> Path<T>
where
    T: Clone + Debug + Default + PartialEq + PartialOrd + Add<T, Output = T> + Sub<Output = T>,
{
    #[allow(unused)]
    pub(crate) fn clipped_bounds(&self) -> Bounds<T> {
        self.bounds.intersect(&self.content_mask.bounds)
    }
}

impl From<Path<ScaledPixels>> for Primitive {
    fn from(path: Path<ScaledPixels>) -> Self {
        Primitive::Path(path)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub(crate) xy_position: Point<P>,
    pub(crate) st_position: Point<f32>,
    pub(crate) content_mask: ContentMask<P>,
}

impl PathVertex<Pixels> {
    pub fn scale(&self, factor: f32) -> PathVertex<ScaledPixels> {
        PathVertex {
            xy_position: self.xy_position.scale(factor),
            st_position: self.st_position,
            content_mask: self.content_mask.scale(factor),
        }
    }
}

#[cfg(test)]
#[path = "image_opacity_pulse_tests.rs"]
mod image_opacity_pulse_tests;

#[cfg(test)]
#[path = "image_dither_reveal_tests.rs"]
mod image_dither_reveal_tests;
