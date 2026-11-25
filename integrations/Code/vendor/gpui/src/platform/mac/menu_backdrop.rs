//! Copy only the already-painted rectangle under a popup into GPU scratch
//! storage. The same drawable continues rendering the menu foreground.
use super::new_command_encoder;
use crate::{DevicePixels, Size, color::BackgroundTag, scene::Quad};
use metal::{
    CommandBufferRef, DeviceRef, LibraryRef, MTLLoadAction, MTLOrigin, MTLPixelFormat,
    MTLPrimitiveType, MTLSize, MTLStorageMode, MTLTextureUsage, RenderCommandEncoderRef,
    RenderPipelineDescriptor, RenderPipelineState, Texture, TextureDescriptor, TextureRef,
};
use std::ffi::c_void;

#[derive(Clone)]
pub(super) struct MenuBackdrop {
    sample: Texture,
}

impl MenuBackdrop {
    pub(super) fn is_marker(quad: &Quad) -> bool {
        quad.background.tag == BackgroundTag::BackdropBlur
    }

    pub(super) fn prepare<'a>(
        slot: &'a mut Option<Self>,
        device: &DeviceRef,
        drawable: &TextureRef,
        quads: &[Quad],
    ) -> Option<&'a Self> {
        let mut extent = [0, 0];
        for quad in quads.iter().filter(|quad| Self::is_marker(quad)) {
            if let Some(region) = Region::for_quad(quad, drawable) {
                extent[0] = extent[0].max(region.size.width);
                extent[1] = extent[1].max(region.size.height);
            }
        }
        if extent[0] == 0 || extent[1] == 0 {
            return None;
        }
        let format = drawable.pixel_format();
        if slot.as_ref().is_none_or(|target| {
            target.sample.width() < extent[0]
                || target.sample.height() < extent[1]
                || target.sample.pixel_format() != format
        }) {
            let previous = slot
                .as_ref()
                .filter(|target| target.sample.pixel_format() == format);
            let width = extent[0]
                .next_multiple_of(64)
                .max(previous.map_or(0, |target| target.sample.width()));
            let height = extent[1]
                .next_multiple_of(64)
                .max(previous.map_or(0, |target| target.sample.height()));
            *slot = Some(Self {
                sample: texture(device, width, height, format),
            });
        }
        slot.as_ref()
    }

    pub(super) fn blur<'a>(
        &self,
        command: &'a CommandBufferRef,
        encoder: &'a RenderCommandEncoderRef,
        destination: &TextureRef,
        viewport: Size<DevicePixels>,
        pipeline: &RenderPipelineState,
        quad: &Quad,
    ) -> &'a RenderCommandEncoderRef {
        if quad.background.solid.a == 0. {
            return encoder;
        }
        let Some(region) = Region::for_quad(quad, destination) else {
            return encoder;
        };
        encoder.end_encoding();
        let blit = command.new_blit_command_encoder();
        blit.copy_from_texture(
            destination,
            0,
            0,
            region.origin,
            region.size,
            &self.sample,
            0,
            0,
            MTLOrigin { x: 0, y: 0, z: 0 },
        );
        blit.end_encoding();

        let encoder = new_command_encoder(command, destination, viewport, |attachment| {
            attachment.set_load_action(MTLLoadAction::Load);
        });
        let bounds = quad.bounds;
        let clip = quad.content_mask.bounds;
        let info = [
            [
                destination.width() as f32,
                destination.height() as f32,
                0.,
                0.,
            ],
            [
                bounds.origin.x.0,
                bounds.origin.y.0,
                bounds.size.width.0,
                bounds.size.height.0,
            ],
            [
                region.origin.x as f32,
                region.origin.y as f32,
                region.size.width as f32,
                region.size.height as f32,
            ],
            [
                quad.background.gradient_angle_or_pattern_height,
                quad.corner_radii.top_left.0,
                quad.background.solid.a,
                0.,
            ],
            [
                clip.origin.x.0,
                clip.origin.y.0,
                clip.size.width.0,
                clip.size.height.0,
            ],
        ];
        draw(encoder, pipeline, &self.sample, &info);
        encoder
    }
}

struct Region {
    origin: MTLOrigin,
    size: MTLSize,
}

impl Region {
    fn for_quad(quad: &Quad, drawable: &TextureRef) -> Option<Self> {
        let bounds = quad.bounds;
        let padding = quad.background.gradient_angle_or_pattern_height.ceil() + 2.;
        let width = drawable.width() as f32;
        let height = drawable.height() as f32;
        let left = (bounds.origin.x.0.floor() - padding).clamp(0., width) as u64;
        let top = (bounds.origin.y.0.floor() - padding).clamp(0., height) as u64;
        let right =
            ((bounds.origin.x.0 + bounds.size.width.0).ceil() + padding).clamp(0., width) as u64;
        let bottom =
            ((bounds.origin.y.0 + bounds.size.height.0).ceil() + padding).clamp(0., height) as u64;
        (right > left && bottom > top).then(|| Self {
            origin: MTLOrigin {
                x: left,
                y: top,
                z: 0,
            },
            size: MTLSize {
                width: right - left,
                height: bottom - top,
                depth: 1,
            },
        })
    }
}

fn texture(device: &DeviceRef, width: u64, height: u64, format: MTLPixelFormat) -> Texture {
    let descriptor = TextureDescriptor::new();
    descriptor.set_width(width);
    descriptor.set_height(height);
    descriptor.set_pixel_format(format);
    descriptor.set_storage_mode(MTLStorageMode::Private);
    descriptor.set_usage(MTLTextureUsage::ShaderRead);
    device.new_texture(&descriptor)
}

fn draw(
    encoder: &RenderCommandEncoderRef,
    pipeline: &RenderPipelineState,
    source: &TextureRef,
    info: &[[f32; 4]; 5],
) {
    let bytes = info.as_ptr().cast::<c_void>();
    let size = std::mem::size_of_val(info) as u64;
    encoder.set_render_pipeline_state(pipeline);
    encoder.set_vertex_bytes(0, size, bytes);
    encoder.set_fragment_bytes(0, size, bytes);
    encoder.set_fragment_texture(0, Some(source));
    encoder.draw_primitives(MTLPrimitiveType::Triangle, 0, 6);
}

pub(super) fn pipeline(
    device: &DeviceRef,
    library: &LibraryRef,
    format: MTLPixelFormat,
) -> Result<RenderPipelineState, String> {
    let descriptor = RenderPipelineDescriptor::new();
    descriptor.set_label("Kai menu backdrop");
    let vertex = library.get_function("menu_backdrop_vertex", None)?;
    let fragment = library.get_function("menu_backdrop_fragment", None)?;
    descriptor.set_vertex_function(Some(&vertex));
    descriptor.set_fragment_function(Some(&fragment));
    descriptor
        .color_attachments()
        .object_at(0)
        .unwrap()
        .set_pixel_format(format);
    device.new_render_pipeline_state(&descriptor)
}

/// Verify that a real GPU pass reduces contrast inside the menu and preserves
/// pixels outside it, including the rounded corner.
pub(super) fn verify(device: &DeviceRef, library: &LibraryRef) -> anyhow::Result<()> {
    use crate::{Bounds, ContentMask, Corners, ScaledPixels, point, size};
    const DIMENSION: usize = 64;
    let format = MTLPixelFormat::BGRA8Unorm;
    let descriptor = TextureDescriptor::new();
    descriptor.set_width(DIMENSION as u64);
    descriptor.set_height(DIMENSION as u64);
    descriptor.set_pixel_format(format);
    descriptor.set_storage_mode(MTLStorageMode::Shared);
    descriptor.set_usage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
    let input = device.new_texture(&descriptor);
    let output = device.new_texture(&descriptor);
    let region = metal::MTLRegion::new_2d(0, 0, DIMENSION as u64, DIMENSION as u64);
    let pixels: Vec<u8> = (0..DIMENSION * DIMENSION)
        .flat_map(|index| {
            let shade = if (index % DIMENSION) % 8 < 4 { 0 } else { 255 };
            [shade, shade, shade, 255]
        })
        .collect::<Vec<_>>();
    input.replace_region(region, 0, pixels.as_ptr().cast(), (DIMENSION * 4) as u64);
    let pipeline = pipeline(device, library, format).map_err(anyhow::Error::msg)?;
    let viewport = size(
        DevicePixels(DIMENSION as i32),
        DevicePixels(DIMENSION as i32),
    );
    let rect = |edge, width| {
        Bounds::new(
            point(ScaledPixels(edge), ScaledPixels(edge)),
            size(ScaledPixels(width), ScaledPixels(width)),
        )
    };
    let quad = Quad {
        bounds: rect(12., 40.),
        content_mask: ContentMask {
            bounds: rect(0., DIMENSION as f32),
        },
        background: crate::Background::backdrop_blur(16.),
        corner_radii: Corners::all(ScaledPixels(10.)),
        ..Default::default()
    };
    let mut cache = None;
    let backdrop = MenuBackdrop::prepare(&mut cache, device, &output, &[quad.clone()]).unwrap();
    let queue = device.new_command_queue();
    let command = queue.new_command_buffer();
    let encoder = new_command_encoder(command, &output, viewport, |attachment| {
        attachment.set_load_action(MTLLoadAction::DontCare);
    });
    let extent = DIMENSION as f32;
    draw(
        encoder,
        &pipeline,
        &input,
        &[
            [extent, extent, 0., 0.],
            [0., 0., extent, extent],
            [0., 0., extent, extent],
            [0.; 4],
            [0., 0., extent, extent],
        ],
    );
    let encoder = backdrop.blur(command, encoder, &output, viewport, &pipeline, &quad);
    encoder.end_encoding();
    command.commit();
    command.wait_until_completed();
    anyhow::ensure!(
        command.status() == metal::MTLCommandBufferStatus::Completed,
        "The menu backdrop command did not complete"
    );
    let mut result = vec![0u8; pixels.len()];
    output.get_bytes(
        result.as_mut_ptr().cast(),
        (DIMENSION * 4) as u64,
        region,
        0,
    );
    let pixel = |x, y| ((y * DIMENSION + x) * 4) as usize;
    for (x, y) in [(10, 31), (14, 3), (12, 12)] {
        let offset = pixel(x, y);
        anyhow::ensure!(
            result[offset..offset + 4] == pixels[offset..offset + 4],
            "Menu blur changed a pixel outside the panel at {x},{y}"
        );
    }
    let row = result
        .chunks_exact(4)
        .skip(pixel(20, 32) / 4)
        .take(24)
        .map(|pixel| pixel[0])
        .collect::<Vec<_>>();
    let contrast = row.iter().max().unwrap() - row.iter().min().unwrap();
    anyhow::ensure!(
        contrast < 140,
        "Menu backdrop did not soften high-contrast stripes: {contrast}"
    );
    eprintln!("KAI_METAL_MENU_BLUR_OK contrast_before=255 contrast_after={contrast}");
    Ok(())
}
