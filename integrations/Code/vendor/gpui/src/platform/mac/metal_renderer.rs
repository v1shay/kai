use super::metal_atlas::MetalAtlas;
#[path = "async_drawable.rs"]
mod async_drawable;
use async_drawable::{Mailbox, Readiness};
#[path = "menu_backdrop.rs"]
mod menu_backdrop;
use menu_backdrop::MenuBackdrop;
use crate::{
    AtlasTextureId, Background, Bounds, ContentMask, DevicePixels, MonochromeSprite, PaintSurface,
    Path, Point, PolychromeSprite, PrimitiveBatch, Quad, ScaledPixels, Scene, Shadow, Size,
    Surface, Underline, point, size,
};
use anyhow::Result;
use block::ConcreteBlock;
use cocoa::{
    base::{NO, YES},
    foundation::{NSSize, NSUInteger},
    quartzcore::AutoresizingMask,
};

use core_foundation::base::TCFType;
use core_video::{
    metal_texture::CVMetalTextureGetTexture, metal_texture_cache::CVMetalTextureCache,
    pixel_buffer::kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
};
use foreign_types::{ForeignType, ForeignTypeRef};
use metal::{
    CAMetalLayer, CommandQueue, MTLPixelFormat, MTLResourceOptions, NSRange,
    RenderPassColorAttachmentDescriptorRef,
};
use objc::{self, msg_send, sel, sel_impl};
use parking_lot::Mutex;

use std::{cell::Cell, ffi::c_void, mem, ptr, sync::{Arc, OnceLock, atomic::{AtomicUsize, Ordering}}, time::Instant};

// Exported to metal
pub(crate) type PointF = crate::Point<f32>;

#[cfg(not(feature = "runtime_shaders"))]
const SHADERS_METALLIB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shaders.metallib"));
#[cfg(feature = "runtime_shaders")]
const SHADERS_SOURCE_FILE: &str = include_str!(concat!(env!("OUT_DIR"), "/stitched_shaders.metal"));
// Use 4x MSAA, all devices support it.
// https://developer.apple.com/documentation/metal/mtldevice/1433355-supportstexturesamplecount
const PATH_SAMPLE_COUNT: u32 = 4;

/// One application's immutable Metal state. Only the instance-buffer pool is
/// shared mutable storage; command queues, atlases, layers and render targets
/// remain owned by each renderer.
#[derive(Clone)]
pub struct Context(Arc<SharedGpuContext>);

struct SharedGpuContext {
    instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
    resources: Mutex<Option<Arc<DeviceResources>>>,
    opaque: Mutex<Option<Arc<PipelineSet>>>,
    transparent: Mutex<Option<Arc<PipelineSet>>>,
    builds: [AtomicUsize; 3],
}

struct DeviceResources {
    device: metal::Device,
    library: metal::Library,
    paths_rasterization: metal::RenderPipelineState,
}

struct PipelineSet {
    path_sprites: metal::RenderPipelineState,
    shadows: metal::RenderPipelineState,
    quads: metal::RenderPipelineState,
    underlines: metal::RenderPipelineState,
    monochrome_sprites: metal::RenderPipelineState,
    polychrome_sprites: metal::RenderPipelineState,
    surfaces: metal::RenderPipelineState,
    menu_backdrop: metal::RenderPipelineState,
}

fn initialization_start() -> Option<Instant> {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    ENABLED.get_or_init(|| std::env::var_os("KAI_PERFORMANCE_LOG").is_some()).then(Instant::now)
}
fn initialization_metric(name: &str, started: Option<Instant>) {
    if let Some(started) = started {
        eprintln!("KAI_METRIC {name} {:.4}", started.elapsed().as_secs_f64() * 1000.);
    }
}

// Failed warmup leaves the cell empty, so the UI can retry the normal required
// Metal initialization. There is no poisoned cache, software renderer, or silent
// removal of an effect. Successful values are initialized exactly once.
fn cached_resource<T>(slot: &Mutex<Option<Arc<T>>>, build: impl FnOnce() -> std::result::Result<T, String>) -> std::result::Result<Arc<T>, String> {
    let mut slot = slot.lock();
    if let Some(value) = slot.as_ref() { return Ok(value.clone()); }
    let value = Arc::new(build()?);
    *slot = Some(value.clone());
    Ok(value)
}

impl Default for Context {
    fn default() -> Self {
        let context = Self::cold();
        let warmup = context.clone();
        if let Err(error) = std::thread::Builder::new().name("gpui-metal-warmup".into()).spawn(move || {
            // Metal creates autoreleased Objective-C descriptors and function
            // objects. This worker must have its own pool, never an AppKit view.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                objc::rc::autoreleasepool(|| warmup.prepare_all())
            }));
            match result {
                Ok(Ok(())) => {},
                Ok(Err(error)) => log::warn!("Metal warmup failed; window initialization will retry: {error}"),
                Err(_) => log::warn!("Metal warmup panicked; window initialization will retry"),
            }
        }) {
            log::warn!("Could not start Metal warmup; window initialization remains synchronous: {error}");
        }
        context
    }
}

impl Context {
    pub(super) fn without_prewarm() -> Self { Self::cold() }
    fn cold() -> Self {
        Self(Arc::new(SharedGpuContext {
            instance_buffer_pool: Arc::new(Mutex::new(InstanceBufferPool::default())),
            resources: Mutex::new(None), opaque: Mutex::new(None), transparent: Mutex::new(None),
            builds: std::array::from_fn(|_| AtomicUsize::new(0)),
        }))
    }
    fn resources(&self) -> std::result::Result<Arc<DeviceResources>, String> {
        cached_resource(&self.0.resources, || {
            let started = initialization_start();
            self.0.builds[0].fetch_add(1, Ordering::Relaxed);
            // Preserve the existing low-power/removable-device selection policy.
            let mut devices = metal::Device::all();
            devices.sort_by_key(|device| (device.is_removable(), device.is_low_power()));
            let device = devices.pop().ok_or("Unable to access a compatible Metal graphics device")?;
            #[cfg(feature = "runtime_shaders")]
            let library = device.new_library_with_source(SHADERS_SOURCE_FILE, &metal::CompileOptions::new())?;
            #[cfg(not(feature = "runtime_shaders"))]
            let library = device.new_library_with_data(SHADERS_METALLIB)?;
            let paths_rasterization = try_build_path_rasterization_pipeline_state(
                &device, &library, "paths_rasterization", "path_rasterization_vertex",
                "path_rasterization_fragment", MTLPixelFormat::BGRA8Unorm, PATH_SAMPLE_COUNT,
            )?;
            if started.is_some() {
                eprintln!("KAI_GPU_CACHE initializer_thread={}", std::thread::current().name().unwrap_or("unnamed"));
            }
            initialization_metric("metal_device_library_init", started);
            Ok(DeviceResources { device, library, paths_rasterization })
        })
    }
    fn pipelines(&self, transparent: bool) -> std::result::Result<Arc<PipelineSet>, String> {
        let slot = if transparent { &self.0.transparent } else { &self.0.opaque };
        cached_resource(slot, || {
            let resources = self.resources()?;
            let started = initialization_start();
            self.0.builds[if transparent { 2 } else { 1 }].fetch_add(1, Ordering::Relaxed);
            let pixel_format = compositing_format(transparent);
            let pipeline = |label, vertex, fragment| try_build_pipeline_state(&resources.device, &resources.library, label, vertex, fragment, pixel_format);
            let set = PipelineSet {
                path_sprites: try_build_path_sprite_pipeline_state(&resources.device, &resources.library, "path_sprites", "path_sprite_vertex", "path_sprite_fragment", pixel_format)?,
                shadows: pipeline("shadows", "shadow_vertex", "shadow_fragment")?,
                quads: pipeline("quads", "quad_vertex", "quad_fragment")?,
                underlines: pipeline("underlines", "underline_vertex", "underline_fragment")?,
                monochrome_sprites: pipeline("monochrome_sprites", "monochrome_sprite_vertex", "monochrome_sprite_fragment")?,
                polychrome_sprites: pipeline("polychrome_sprites", "polychrome_sprite_vertex", "polychrome_sprite_fragment")?,
                surfaces: pipeline("surfaces", "surface_vertex", "surface_fragment")?,
                menu_backdrop: menu_backdrop::pipeline(&resources.device, &resources.library, pixel_format)?,
            };
            initialization_metric(if transparent { "metal_transparent_pipelines_init" } else { "metal_opaque_pipelines_init" }, started);
            Ok(set)
        })
    }
    fn prepare_all(&self) -> std::result::Result<(), String> {
        self.pipelines(true)?;
        self.pipelines(false)?;
        Ok(())
    }
}
pub type Renderer = MetalRenderer;

pub unsafe fn new_renderer(
    context: self::Context,
    _native_window: *mut c_void,
    _native_view: *mut c_void,
    _bounds: crate::Size<f32>,
    transparent: bool,
) -> Renderer {
    MetalRenderer::new(context, transparent)
}

pub(crate) struct InstanceBufferPool {
    buffer_size: usize,
    buffers: Vec<metal::Buffer>,
}

impl Default for InstanceBufferPool {
    fn default() -> Self {
        Self {
            buffer_size: 2 * 1024 * 1024,
            buffers: Vec::new(),
        }
    }
}

pub(crate) struct InstanceBuffer {
    metal_buffer: metal::Buffer,
    size: usize,
}

impl InstanceBufferPool {
    pub(crate) fn reset(&mut self, buffer_size: usize) {
        self.buffer_size = buffer_size;
        self.buffers.clear();
    }

    pub(crate) fn acquire(&mut self, device: &metal::Device) -> InstanceBuffer {
        let buffer = self.buffers.pop().unwrap_or_else(|| {
            device.new_buffer(
                self.buffer_size as u64,
                MTLResourceOptions::StorageModeManaged,
            )
        });
        InstanceBuffer {
            metal_buffer: buffer,
            size: self.buffer_size,
        }
    }

    pub(crate) fn release(&mut self, buffer: InstanceBuffer) {
        // Cache up to 16 MiB. Retain one larger buffer for steady reuse.
        let cache_limit = (16 * 1024 * 1024 / self.buffer_size.max(1)).clamp(1, 8);
        if buffer.size == self.buffer_size && self.buffers.len() < cache_limit {
            self.buffers.push(buffer.metal_buffer)
        }
    }
}

// Diagnostic-only, bounded per-renderer presentation evidence. CPU frame
// callbacks and GPU completion are not evidence that a drawable was displayed.
const PRESENTATION_SAMPLE_LIMIT: usize = 512;

struct PresentationSample {
    drawable_id: u64,
    presented_time: Option<f64>,
}

struct PresentationProbe {
    renderer_id: usize,
    samples: Vec<PresentationSample>,
    duplicate_callbacks: usize,
    capped_submissions: usize,
}

impl PresentationProbe {
    fn new() -> Self {
        static NEXT_RENDERER: AtomicUsize = AtomicUsize::new(1);
        Self {
            renderer_id: NEXT_RENDERER.fetch_add(1, Ordering::Relaxed),
            samples: Vec::with_capacity(PRESENTATION_SAMPLE_LIMIT),
            duplicate_callbacks: 0,
            capped_submissions: 0,
        }
    }

    fn register(&mut self, drawable_id: u64) -> Option<usize> {
        if self.samples.len() == PRESENTATION_SAMPLE_LIMIT {
            if self.capped_submissions == 0 {
                eprintln!("KAI_PRESENTATION_LIMIT renderer={} limit={}",
                    self.renderer_id, PRESENTATION_SAMPLE_LIMIT);
            }
            self.capped_submissions = self.capped_submissions.saturating_add(1);
            return None;
        }
        let sequence = self.samples.len();
        self.samples.push(PresentationSample { drawable_id, presented_time: None });
        eprintln!("KAI_PRESENTATION_SUBMIT renderer={} sequence={} drawable_id={}",
            self.renderer_id, sequence, drawable_id);
        Some(sequence)
    }

    fn record(&mut self, sequence: usize, presented_time: f64) {
        let Some(sample) = self.samples.get_mut(sequence) else { return; };
        if sample.presented_time.is_some() {
            self.duplicate_callbacks = self.duplicate_callbacks.saturating_add(1);
            return;
        }
        sample.presented_time = Some(presented_time);
        let status = if presented_time == 0. { "zero" }
            else if !presented_time.is_finite() || presented_time < 0. { "invalid" }
            else { "positive" };
        eprintln!("KAI_PRESENTATION renderer={} sequence={} drawable_id={} presented_time={:.9} status={}",
            self.renderer_id, sequence, sample.drawable_id, presented_time, status);
    }
}

impl Drop for PresentationProbe {
    fn drop(&mut self) {
        let callbacks = self.samples.iter().filter(|s| s.presented_time.is_some()).count();
        let zero = self.samples.iter().filter(|s| s.presented_time == Some(0.)).count();
        let invalid = self.samples.iter().filter(|s| s.presented_time
            .is_some_and(|t| !t.is_finite() || t < 0.)).count();
        let mut positive: Vec<f64> = self.samples.iter().filter_map(|s| s.presented_time)
            .filter(|t| t.is_finite() && *t > 0.).collect();
        let positive_count = positive.len();
        // Metal callback arrival order need not be display order. Retain raw
        // records above, then derive intervals only from sorted display times.
        positive.sort_by(f64::total_cmp);
        positive.dedup();
        for times in positive.windows(2) {
            eprintln!("KAI_PRESENTED_INTERVAL renderer={} milliseconds={:.4}",
                self.renderer_id, (times[1] - times[0]) * 1000.);
        }
        eprintln!("KAI_PRESENTATION_SUMMARY renderer={} submitted={} callbacks={} positive={} zero={} invalid={} unconfirmed={} duplicate_callbacks={} duplicate_timestamps={} capped={}",
            self.renderer_id, self.samples.len(), callbacks, positive_count, zero, invalid,
            self.samples.len() - callbacks, self.duplicate_callbacks,
            positive_count - positive.len(), self.capped_submissions);
    }
}

pub(crate) struct MetalRenderer {
    device: metal::Device,
    context: Context,
    layer: metal::MetalLayer,
    presents_with_transaction: bool,
    presentation_probe: Option<Arc<Mutex<PresentationProbe>>>,
    transaction_pending: bool,
    applied_presents_with_transaction: bool,
    desired_size: Size<DevicePixels>,
    desired_scale: Option<f64>,
    size_configured: bool,
    desired_transparent: bool,
    layer_configuration_dirty: bool,
    surface_visible: bool,
    mailbox: Arc<Mailbox<metal::MetalDrawable>>,
    drawable_ready: Option<Arc<dyn Fn() + Send + Sync>>,
    worker_started: bool,
    command_queue: CommandQueue,
    paths_rasterization_pipeline_state: metal::RenderPipelineState,
    path_sprites_pipeline_state: metal::RenderPipelineState,
    shadows_pipeline_state: metal::RenderPipelineState,
    quads_pipeline_state: metal::RenderPipelineState,
    underlines_pipeline_state: metal::RenderPipelineState,
    monochrome_sprites_pipeline_state: metal::RenderPipelineState,
    polychrome_sprites_pipeline_state: metal::RenderPipelineState,
    surfaces_pipeline_state: metal::RenderPipelineState,
    menu_backdrop_pipeline_state: metal::RenderPipelineState,
    menu_backdrop: Option<MenuBackdrop>,
    unit_vertices: metal::Buffer,
    #[allow(clippy::arc_with_non_send_sync)]
    instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
    sprite_atlas: Arc<MetalAtlas>,
    core_video_texture_cache: core_video::metal_texture_cache::CVMetalTextureCache,
    path_texture_size: Option<Size<DevicePixels>>,
    path_intermediate_texture: Option<metal::Texture>,
    path_intermediate_msaa_texture: Option<metal::Texture>,
    path_sample_count: u32,
    resource_snapshots_remaining: u8,
    diagnostic_path_allocations: u64,
}

#[repr(C)]
pub struct PathRasterizationVertex {
    pub xy_position: Point<ScaledPixels>,
    pub st_position: Point<f32>,
    pub color: Background,
    pub bounds: Bounds<ScaledPixels>,
}

fn supports_menu_sampling(transparent: bool) -> bool {
    transparent && objc::runtime::Class::get("NSGlassEffectView").is_some()
}

fn compositing_format(transparent: bool) -> MTLPixelFormat {
    if transparent { MTLPixelFormat::RGBA16Float } else { MTLPixelFormat::BGRA8Unorm }
}

impl MetalRenderer {
    pub fn new(context: Context, transparent: bool) -> Self {
        let started = initialization_start();
        let warm_at_entry = context.0.resources.try_lock().is_some_and(|slot| slot.is_some())
            && context.0.transparent.try_lock().is_some_and(|slot| slot.is_some())
            && context.0.opaque.try_lock().is_some_and(|slot| slot.is_some());
        // Both formats are ready before presentation, so opening a second
        // window or switching appearance does not compile on the input path.
        let pipelines = context.pipelines(transparent).unwrap_or_else(|error| panic!("Required Metal initialization failed: {error}"));
        initialization_metric("metal_initial_format_wait", started);
        let other_started = initialization_start();
        context.pipelines(!transparent).unwrap_or_else(|error| panic!("Required Metal appearance initialization failed: {error}"));
        initialization_metric("metal_other_format_wait", other_started);
        let resources = context.resources().expect("Metal device was initialized");
        let device = resources.device.clone();
        let instance_buffer_pool = context.0.instance_buffer_pool.clone();
        initialization_metric("metal_context_wait", started);
        if initialization_start().is_some() {
            eprintln!("KAI_GPU_CACHE warm_at_renderer={} device_builds={} opaque_builds={} transparent_builds={}",
                usize::from(warm_at_entry), context.0.builds[0].load(Ordering::Relaxed),
                context.0.builds[1].load(Ordering::Relaxed), context.0.builds[2].load(Ordering::Relaxed));
        }

        let layer = metal::MetalLayer::new();
        layer.set_device(&device);
        // Kai: preserve fractional coverage until Core Animation composites the
        // native backdrop. Opaque windows keep the smaller 8-bit drawable.
        let pixel_format = compositing_format(transparent);
        layer.set_pixel_format(pixel_format);
        layer.set_opaque(!transparent);
        layer.set_framebuffer_only(!supports_menu_sampling(transparent));
        if let Some(space) = unsafe { core_graphics::color_space::CGColorSpace::create_with_name(core_graphics::color_space::kCGColorSpaceSRGB) } {
            unsafe { let _: () = msg_send![&*layer, setColorspace: space.as_ptr()]; }
        }
        layer.set_maximum_drawable_count(3);
        unsafe {
            let _: () = msg_send![&*layer, setAllowsNextDrawableTimeout: YES];
            let _: () = msg_send![&*layer, setNeedsDisplayOnBoundsChange: YES];
            let _: () = msg_send![
                &*layer,
                setAutoresizingMask: AutoresizingMask::WIDTH_SIZABLE
                    | AutoresizingMask::HEIGHT_SIZABLE
            ];
        }
        fn to_float2_bits(point: PointF) -> u64 {
            let mut output = point.y.to_bits() as u64;
            output <<= 32;
            output |= point.x.to_bits() as u64;
            output
        }

        let unit_vertices = [
            to_float2_bits(point(0., 0.)),
            to_float2_bits(point(1., 0.)),
            to_float2_bits(point(0., 1.)),
            to_float2_bits(point(0., 1.)),
            to_float2_bits(point(1., 0.)),
            to_float2_bits(point(1., 1.)),
        ];
        let unit_vertices = device.new_buffer_with_data(
            unit_vertices.as_ptr() as *const c_void,
            mem::size_of_val(&unit_vertices) as u64,
            MTLResourceOptions::StorageModeManaged,
        );

        let paths_rasterization_pipeline_state = resources.paths_rasterization.clone();
        let path_sprites_pipeline_state = pipelines.path_sprites.clone();
        let shadows_pipeline_state = pipelines.shadows.clone();
        let quads_pipeline_state = pipelines.quads.clone();
        let underlines_pipeline_state = pipelines.underlines.clone();
        let monochrome_sprites_pipeline_state = pipelines.monochrome_sprites.clone();
        let polychrome_sprites_pipeline_state = pipelines.polychrome_sprites.clone();
        let surfaces_pipeline_state = pipelines.surfaces.clone();

        let command_queue = device.new_command_queue();
        let sprite_atlas = Arc::new(MetalAtlas::new(device.clone()));
        let core_video_texture_cache =
            CVMetalTextureCache::new(None, device.clone(), None).unwrap();

        Self {
            device,
            context,
            layer,
            presents_with_transaction: false,
            presentation_probe: started.map(|_| Arc::new(Mutex::new(PresentationProbe::new()))),
            transaction_pending: false,
            applied_presents_with_transaction: false,
            desired_size: size(DevicePixels(0), DevicePixels(0)),
            desired_scale: None,
            size_configured: false,
            desired_transparent: transparent,
            layer_configuration_dirty: false,
            surface_visible: true,
            mailbox: Arc::new(Mailbox::default()),
            drawable_ready: None,
            worker_started: false,
            command_queue,
            paths_rasterization_pipeline_state,
            path_sprites_pipeline_state,
            shadows_pipeline_state,
            quads_pipeline_state,
            underlines_pipeline_state,
            monochrome_sprites_pipeline_state,
            polychrome_sprites_pipeline_state,
            surfaces_pipeline_state,
            menu_backdrop_pipeline_state: pipelines.menu_backdrop.clone(),
            menu_backdrop: None,
            unit_vertices,
            instance_buffer_pool,
            sprite_atlas,
            core_video_texture_cache,
            path_texture_size: None,
            path_intermediate_texture: None,
            path_intermediate_msaa_texture: None,
            path_sample_count: PATH_SAMPLE_COUNT,
            resource_snapshots_remaining: if started.is_some() { 64 } else { 0 },
            diagnostic_path_allocations: 0,
        }
    }

    pub fn layer(&self) -> &metal::MetalLayerRef {
        &self.layer
    }

    pub fn layer_ptr(&self) -> *mut CAMetalLayer {
        self.layer.as_ptr()
    }

    pub fn sprite_atlas(&self) -> &Arc<MetalAtlas> {
        &self.sprite_atlas
    }

    // Must be called with the acquisition mailbox held idle, or before any
    // worker exists. Keep a cache so ordinary frames do not write CA properties.
    fn write_presentation_mode(&mut self, transaction: bool) {
        if self.applied_presents_with_transaction != transaction {
            self.layer.set_presents_with_transaction(transaction);
            self.applied_presents_with_transaction = transaction;
        }
    }

    fn apply_presentation_mode(&mut self, transaction: bool) -> bool {
        let mailbox = self.mailbox.clone();
        mailbox.while_idle(|| self.write_presentation_mode(transaction))
    }

    pub fn set_presents_with_transaction(&mut self, transaction: bool) {
        self.presents_with_transaction = transaction;
        // End of the original AppKit transaction must not discard the intent
        // of a frame whose drawable has not arrived yet.
        self.transaction_pending |= transaction;
        let _ = self.apply_presentation_mode(transaction);
    }

    pub fn set_drawable_ready_callback(&mut self, callback: Arc<dyn Fn() + Send + Sync>) {
        static SYNC_DIAGNOSTIC: OnceLock<bool> = OnceLock::new();
        let sync = *SYNC_DIAGNOSTIC.get_or_init(|| {
            std::env::var_os("KAI_PERFORMANCE_LOG").is_some()
                && std::env::var("KAI_DIAGNOSTIC_SYNC_DRAWABLE").ok().as_deref() == Some("1")
        });
        if initialization_start().is_some() {
            eprintln!("KAI_METAL_ACQUISITION {}", if sync { "sync" } else { "async" });
        }
        if !sync { self.drawable_ready = Some(callback); }
        self.mailbox.set_visible(self.surface_visible);
    }

    pub fn set_surface_visible(&mut self, visible: bool) {
        let changed = self.surface_visible != visible;
        self.surface_visible = visible;
        self.mailbox.set_visible(visible);
        if !changed { return; }
        if !visible {
            self.resource_snapshot("hide-before-trim");
            self.menu_backdrop = None;
            // These are scratch render targets, never persistent scene pixels.
            // Every path batch clears and rasterizes them again before sampling.
            // Standard commandBuffer retains encoded resources until completion;
            // release only our reusable references, without waiting for the GPU.
            self.path_texture_size = None;
            self.path_intermediate_texture = None;
            self.path_intermediate_msaa_texture = None;
            self.resource_snapshot("hide-after-trim");
        } else {
            self.resource_snapshot("show");
        }
    }

    fn resource_snapshot(&mut self, reason: &'static str) {
        // Opt-in only: allocation queries and locking never run normally.
        // Bound output even when an app repeatedly switches surface visibility.
        if self.resource_snapshots_remaining == 0 { return; }
        self.resource_snapshots_remaining -= 1;
        let Some(probe) = self.presentation_probe.as_ref() else { return; };
        let renderer_id = probe.lock().renderer_id;
        let resolve = self.path_intermediate_texture.as_ref().map_or(0, |t| t.allocated_size());
        let msaa = self.path_intermediate_msaa_texture.as_ref().map_or(0, |t| t.allocated_size());
        let pool = self.instance_buffer_pool.lock();
        eprintln!("KAI_METAL_RESOURCE renderer={} reason={} visible={} device_global_current_bytes={} renderer_path_resolve_bytes={} renderer_path_msaa_bytes={} path_allocations={} pool_buffer_size={} pool_free_count={} pool_free_bytes={} snapshots_remaining={} msaa_storage={:?} allocation_counter=MTLResource.allocatedSize tile_memory_accounted=false",
            renderer_id, reason, self.surface_visible, self.device.current_allocated_size(),
            resolve, msaa, self.diagnostic_path_allocations, pool.buffer_size,
            pool.buffers.len(), pool.buffer_size.saturating_mul(pool.buffers.len()), self.resource_snapshots_remaining, self.path_intermediate_msaa_texture.as_ref().map(|texture| texture.storage_mode()));
    }

    pub fn surface_visible(&self) -> bool { self.surface_visible && self.mailbox.is_visible() }

    pub fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        if self.desired_size == size { return; }
        self.desired_size = size;
        self.size_configured = true;
        self.layer_configuration_dirty = true;
        self.mailbox.invalidate();
        if !self.worker_started { self.apply_layer_configuration(); }
    }

    pub fn update_contents_scale(&mut self, scale: f64) {
        if self.desired_scale == Some(scale) { return; }
        self.desired_scale = Some(scale);
        self.layer_configuration_dirty = true;
        self.mailbox.invalidate();
        if !self.worker_started { self.apply_layer_configuration(); }
    }

    fn apply_layer_configuration(&mut self) {
        if !self.layer_configuration_dirty { return; }
        self.layer.set_pixel_format(compositing_format(self.desired_transparent));
        self.layer.set_opaque(!self.desired_transparent);
        let framebuffer_only = !supports_menu_sampling(self.desired_transparent);
        if self.layer.framebuffer_only() != framebuffer_only {
            self.layer.set_framebuffer_only(framebuffer_only);
        }
        unsafe {
            if self.size_configured {
                let _: () = msg_send![self.layer(), setDrawableSize: NSSize {
                    width: self.desired_size.width.0 as f64,
                    height: self.desired_size.height.0 as f64,
                }];
            }
            if let Some(scale) = self.desired_scale {
                let _: () = msg_send![self.layer(), setContentsScale: scale];
            }
        }
        self.layer_configuration_dirty = false;
    }

    pub fn prepare_frame(&mut self) -> crate::FrameReadiness {
        if !self.surface_visible || self.desired_size.width.0 <= 0 || self.desired_size.height.0 <= 0 {
            return crate::FrameReadiness::Suspended;
        }
        let Some(ready) = self.drawable_ready.clone() else {
            return crate::FrameReadiness::Ready;
        };
        if !self.worker_started {
            let mailbox = self.mailbox.clone();
            let layer = self.layer.clone();
            // One worker for this window. Its retained layer keeps Metal alive
            // through close, but it owns neither AppKit objects nor the window.
            let worker = std::thread::Builder::new().name("gpui-drawable".into()).spawn(move || {
                let mut backoff_ms = 8u64;
                while let Some(generation) = mailbox.next_request() {
                    let started = initialization_start();
                    let drawable = objc::rc::autoreleasepool(|| layer.next_drawable().map(|d| d.to_owned()));
                    initialization_metric("metal_async_drawable_wait", started);
                    if drawable.is_none() {
                        // Still marked in-flight while backing off: main is
                        // nonblocking and cannot create a busy retry loop.
                        std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                        backoff_ms = (backoff_ms * 2).min(100);
                    } else { backoff_ms = 8; }
                    if !mailbox.complete(generation, drawable) { break; }
                    ready();
                }
            });
            if let Err(error) = worker {
                log::error!("Metal drawable worker failed to start; retaining synchronous GPU rendering: {error}");
                if initialization_start().is_some() { eprintln!("KAI_METAL_ACQUISITION sync-worker-start-failed"); }
                self.drawable_ready = None;
                return crate::FrameReadiness::Ready;
            }
            self.worker_started = true;
        }
        let mailbox = self.mailbox.clone();
        match mailbox.prepare(|| {
            self.apply_layer_configuration();
            // Main still owns the mailbox; no worker can take the request
            // before the latest logical mode is restored/configured.
            self.write_presentation_mode(self.presents_with_transaction);
        }) {
            Readiness::Ready => crate::FrameReadiness::Ready,
            Readiness::Waiting => crate::FrameReadiness::Waiting,
            Readiness::Suspended => crate::FrameReadiness::Suspended,
        }
    }

    fn prepare_path_intermediate_textures(&mut self, size: Size<DevicePixels>) {
        if self.path_texture_size == Some(size) {
            return;
        }
        self.path_texture_size = Some(size);
        let texture_descriptor = metal::TextureDescriptor::new();
        texture_descriptor.set_width(size.width.0 as u64);
        texture_descriptor.set_height(size.height.0 as u64);
        texture_descriptor.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm);
        texture_descriptor
            .set_usage(metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead);
        self.path_intermediate_texture = Some(self.device.new_texture(&texture_descriptor));

        if self.path_sample_count > 1 {
            let mut msaa_descriptor = texture_descriptor;
            msaa_descriptor.set_texture_type(metal::MTLTextureType::D2Multisample);
            let storage = path_msaa_storage_mode(self.device.supports_family(metal::MTLGPUFamily::Apple2));
            msaa_descriptor.set_storage_mode(storage);
            if storage == metal::MTLStorageMode::Memoryless {
                // Contents live only in this clear+resolve render pass. The
                // separate backed resolve is what later shader passes sample.
                msaa_descriptor.set_usage(metal::MTLTextureUsage::RenderTarget);
            }
            msaa_descriptor.set_sample_count(self.path_sample_count as _);
            self.path_intermediate_msaa_texture = Some(self.device.new_texture(&msaa_descriptor));
        } else {
            self.path_intermediate_msaa_texture = None;
        }
        if self.resource_snapshots_remaining > 0 {
            self.diagnostic_path_allocations = self.diagnostic_path_allocations.saturating_add(1);
            self.resource_snapshot("path-targets-created");
        }
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        if self.desired_transparent == transparent { return; }
        self.desired_transparent = transparent;
        self.layer_configuration_dirty = true;
        self.mailbox.invalidate();
        if !self.worker_started { self.apply_layer_configuration(); }
        let pipelines = self.context.pipelines(transparent).expect("Required Metal appearance pipelines failed");
        self.path_sprites_pipeline_state = pipelines.path_sprites.clone();
        self.shadows_pipeline_state = pipelines.shadows.clone();
        self.quads_pipeline_state = pipelines.quads.clone();
        self.underlines_pipeline_state = pipelines.underlines.clone();
        self.monochrome_sprites_pipeline_state = pipelines.monochrome_sprites.clone();
        self.polychrome_sprites_pipeline_state = pipelines.polychrome_sprites.clone();
        self.surfaces_pipeline_state = pipelines.surfaces.clone();
        self.menu_backdrop_pipeline_state = pipelines.menu_backdrop.clone();
        self.menu_backdrop = None;
    }

    pub fn destroy(&self) {
        self.mailbox.close(); // no joining an uncancellable nextDrawable on main
    }

    pub fn draw(&mut self, scene: &Scene) {
        // CVDisplayLink dispatches frames without an NSEvent-owned pool. Metal
        // lends autoreleased drawables/command objects; retaining them until
        // AppKit's outer pool drains can exhaust the layer's drawable queue and
        // block the next UI-thread acquisition even after GPU completion.
        let _ = self.try_draw(scene);
    }

    pub fn try_draw(&mut self, scene: &Scene) -> crate::FrameSubmission {
        objc::rc::autoreleasepool(|| self.draw_frame(scene))
    }

    fn draw_frame(&mut self, scene: &Scene) -> crate::FrameSubmission {
        if !self.surface_visible { return crate::FrameSubmission::NoSurface; }
        let layer = self.layer.clone();
        let drawable = if self.worker_started {
            let Some(drawable) = self.mailbox.take_ready() else { return crate::FrameSubmission::Deferred; };
            let texture = drawable.texture();
            if texture.width() != self.desired_size.width.0 as u64
                || texture.height() != self.desired_size.height.0 as u64
                || texture.pixel_format() != compositing_format(self.desired_transparent)
            {
                self.mailbox.invalidate();
                return crate::FrameSubmission::Deferred;
            }
            drawable
        } else {
            let size = layer.drawable_size();
            if size.width <= 0. || size.height <= 0. { return crate::FrameSubmission::NoSurface; }
            let started = initialization_start();
            let drawable = layer.next_drawable().map(|d| d.to_owned());
            initialization_metric("metal_drawable_wait", started);
            let Some(drawable) = drawable else { return crate::FrameSubmission::Deferred; };
            drawable
        };
        let transaction = self.transaction_pending || self.presents_with_transaction;
        // Acquirer is parked after handing off this drawable. Serialize every
        // application-owned layer write; no main lock waits on nextDrawable.
        if !self.apply_presentation_mode(transaction) {
            return crate::FrameSubmission::Deferred;
        }
        // Deferred AppKit display callbacks have ended their original scope.
        // This RAII transaction encompasses the actual current presentation.
        struct PresentTransaction(bool);
        impl Drop for PresentTransaction {
            fn drop(&mut self) {
                if self.0 {
                    let started = initialization_start();
                    unsafe { let _: () = msg_send![objc::class!(CATransaction), commit]; }
                    initialization_metric("metal_transaction_commit", started);
                }
            }
        }
        if transaction { unsafe { let _: () = msg_send![objc::class!(CATransaction), begin]; } }
        let transaction_scope = PresentTransaction(transaction);

        // During a resize only allocate for the drawable being submitted.
        // Many intermediate AppKit sizes never receive a frame, and most
        // scenes use quads and sprites without needing path rasterization.
        let texture = drawable.texture();
        let viewport_size = size(
            DevicePixels(texture.width() as i32),
            DevicePixels(texture.height() as i32),
        );
        if !scene.paths.is_empty() {
            self.prepare_path_intermediate_textures(viewport_size);
        }

        loop {
            let mut instance_buffer = self.instance_buffer_pool.lock().acquire(&self.device);

            let encode_started = initialization_start();
            let command_buffer =
                self.draw_primitives(scene, &mut instance_buffer, &drawable, viewport_size);
            initialization_metric("metal_encode", encode_started);

            match command_buffer {
                Ok(command_buffer) => {
                    static TIMINGS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                    let timings = *TIMINGS.get_or_init(|| std::env::var_os("KAI_PERFORMANCE_LOG").is_some());
                    let instance_buffer_pool = self.instance_buffer_pool.clone();
                    let instance_buffer = Cell::new(Some(instance_buffer));
                    let block = ConcreteBlock::new(move |completed: &metal::CommandBufferRef| {
                        if completed.status() != metal::MTLCommandBufferStatus::Completed {
                            eprintln!("KAI_METRIC gpu_command_error 1");
                        }
                        if timings {
                            // Read after completion, when Metal's GPU timestamps are valid.
                            // This is execution time, not scanout or queueing latency.
                            let start: f64 = unsafe { msg_send![completed, GPUStartTime] };
                            let end: f64 = unsafe { msg_send![completed, GPUEndTime] };
                            if start > 0. && end >= start {
                                eprintln!("KAI_METRIC gpu_execution {:.4}", (end - start) * 1000.);
                            }
                        }
                        if let Some(instance_buffer) = instance_buffer.take() {
                            instance_buffer_pool.lock().release(instance_buffer);
                        }
                    });
                    let block = block.copy();
                    command_buffer.add_completed_handler(&block);

                    // Install before either normal or transactional present.
                    // The callback owns only a bounded diagnostic Arc and an
                    // index: never a renderer, drawable, window or UI context.
                    let presentation = self.presentation_probe.as_ref().and_then(|probe| {
                        let sequence = probe.lock().register(drawable.drawable_id() as u64);
                        sequence.map(|sequence| (Arc::clone(probe), sequence))
                    });
                    if let Some((probe, sequence)) = presentation {
                        let block = ConcreteBlock::new(move |shown: &metal::DrawableRef| {
                            let presented_time = shown.presented_time();
                            probe.lock().record(sequence, presented_time);
                        }).copy();
                        drawable.add_presented_handler(&block);
                    }


                    if transaction {
                        let commit_started = initialization_start();
                        command_buffer.commit();
                        initialization_metric("metal_commit", commit_started);
                        let scheduled_started = initialization_start();
                        command_buffer.wait_until_scheduled();
                        initialization_metric("metal_schedule_wait", scheduled_started);
                        let present_started = initialization_start();
                        drawable.present();
                        initialization_metric("metal_present", present_started);
                    } else {
                        let present_started = initialization_start();
                        command_buffer.present_drawable(&drawable);
                        command_buffer.commit();
                        initialization_metric("metal_present_commit", present_started);
                    }
                    self.transaction_pending = false;
                    drop(transaction_scope); // commit before restoring layer mode
                    let restored = self.apply_presentation_mode(self.presents_with_transaction);
                    debug_assert!(restored, "acquirer must remain parked through presentation");
                    return crate::FrameSubmission::Submitted;
                }
                Err(err) => {
                    log::error!(
                        "failed to render: {}. retrying with larger instance buffer size",
                        err
                    );
                    let mut instance_buffer_pool = self.instance_buffer_pool.lock();
                    let buffer_size = instance_buffer_pool.buffer_size;
                    if buffer_size >= 256 * 1024 * 1024 {
                        log::error!("instance buffer size grew too large: {}", buffer_size);
                        break;
                    }
                    instance_buffer_pool.reset(buffer_size * 2);
                    log::info!(
                        "increased instance buffer size to {}",
                        instance_buffer_pool.buffer_size
                    );
                }
            }
        }
        drop(transaction_scope);
        let restored = self.apply_presentation_mode(self.presents_with_transaction);
        debug_assert!(restored, "acquirer must remain parked through presentation");
        crate::FrameSubmission::Deferred
    }

    fn draw_primitives(
        &mut self,
        scene: &Scene,
        instance_buffer: &mut InstanceBuffer,
        drawable: &metal::MetalDrawableRef,
        viewport_size: Size<DevicePixels>,
    ) -> Result<metal::CommandBuffer> {
        let command_queue = self.command_queue.clone();
        let command_buffer = command_queue.new_command_buffer();
        let alpha = if self.layer.is_opaque() { 1. } else { 0. };
        let mut instance_offset = 0;

        let property_offset = instance_offset;
        let property_bytes = mem::size_of_val(scene.gpu_sprite_properties.as_slice());
        if property_bytes > instance_buffer.size {
            anyhow::bail!("Retained sprite properties exceed instance buffer");
        }
        if property_bytes != 0 {
            unsafe {
                ptr::copy_nonoverlapping(
                    scene.gpu_sprite_properties.as_ptr() as *const u8,
                    instance_buffer.metal_buffer.contents() as *mut u8,
                    property_bytes,
                );
            }
        }
        instance_offset += property_bytes;
        align_offset(&mut instance_offset);
        let property_offset = (property_bytes != 0).then_some(property_offset);

        let destination = drawable.texture();
        let backdrop = if scene.has_backdrop_blur && !destination.framebuffer_only() {
            MenuBackdrop::prepare(&mut self.menu_backdrop, &self.device, destination, &scene.quads).cloned()
        } else {
            None
        };
        let mut command_encoder = new_command_encoder(
            command_buffer,
            destination,
            viewport_size,
            |color_attachment| {
                color_attachment.set_load_action(metal::MTLLoadAction::Clear);
                color_attachment.set_clear_color(metal::MTLClearColor::new(0., 0., 0., alpha));
            },
        );

        for batch in scene.batches() {
            let ok = match batch {
                PrimitiveBatch::Shadows(shadows) => self.draw_shadows(
                    shadows,
                    instance_buffer,
                    &mut instance_offset,
                    viewport_size,
                    command_encoder,
                ),
                PrimitiveBatch::Quads(quads) if !scene.has_backdrop_blur => self.draw_quads(
                    quads, instance_buffer, &mut instance_offset, viewport_size, command_encoder,
                ),
                PrimitiveBatch::Quads(quads) => {
                    let mut start = 0;
                    let mut drawn = true;
                    for (index, quad) in quads.iter().enumerate().filter(|(_, quad)| MenuBackdrop::is_marker(quad)) {
                        if start < index && !self.draw_quads(&quads[start..index], instance_buffer,
                            &mut instance_offset, viewport_size, command_encoder)
                        {
                            drawn = false;
                            break;
                        }
                        if let Some(backdrop) = &backdrop {
                            command_encoder = backdrop.blur(command_buffer, command_encoder, destination, viewport_size,
                                &self.menu_backdrop_pipeline_state, quad);
                        }
                        start = index + 1;
                    }
                    drawn && (start == quads.len() || self.draw_quads(&quads[start..], instance_buffer,
                        &mut instance_offset, viewport_size, command_encoder))
                }
                PrimitiveBatch::Paths(paths) => {
                    command_encoder.end_encoding();

                    let did_draw = self.draw_paths_to_intermediate(
                        paths,
                        instance_buffer,
                        &mut instance_offset,
                        viewport_size,
                        command_buffer,
                    );

                    command_encoder = new_command_encoder(
                        command_buffer,
                        destination,
                        viewport_size,
                        |color_attachment| {
                            color_attachment.set_load_action(metal::MTLLoadAction::Load);
                        },
                    );

                    if did_draw {
                        self.draw_paths_from_intermediate(
                            paths,
                            instance_buffer,
                            &mut instance_offset,
                            viewport_size,
                            command_encoder,
                        )
                    } else {
                        false
                    }
                }
                PrimitiveBatch::Underlines(underlines) => self.draw_underlines(
                    underlines,
                    instance_buffer,
                    &mut instance_offset,
                    viewport_size,
                    command_encoder,
                ),
                PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    sprites,
                } => self.draw_monochrome_sprites_with_properties(
                    texture_id,
                    sprites,
                    instance_buffer,
                    &mut instance_offset,
                    viewport_size,
                    property_offset,
                    command_encoder,
                ),
                PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    sprites,
                } => self.draw_polychrome_sprites_with_properties(
                    texture_id,
                    sprites,
                    instance_buffer,
                    &mut instance_offset,
                    viewport_size,
                    property_offset,
                    command_encoder,
                ),
                PrimitiveBatch::Surfaces(surfaces) => self.draw_surfaces(
                    surfaces,
                    instance_buffer,
                    &mut instance_offset,
                    viewport_size,
                    command_encoder,
                ),
            };
            if !ok {
                command_encoder.end_encoding();
                anyhow::bail!(
                    "scene too large: {} paths, {} shadows, {} quads, {} underlines, {} mono, {} poly, {} surfaces",
                    scene.paths.len(),
                    scene.shadows.len(),
                    scene.quads.len(),
                    scene.underlines.len(),
                    scene.monochrome_sprites.len(),
                    scene.polychrome_sprites.len(),
                    scene.surfaces.len(),
                );
            }
        }

        command_encoder.end_encoding();
        instance_buffer.metal_buffer.did_modify_range(NSRange {
            location: 0,
            length: instance_offset as NSUInteger,
        });
        Ok(command_buffer.to_owned())
    }

    fn draw_paths_to_intermediate(
        &self,
        paths: &[Path<ScaledPixels>],
        instance_buffer: &mut InstanceBuffer,
        instance_offset: &mut usize,
        viewport_size: Size<DevicePixels>,
        command_buffer: &metal::CommandBufferRef,
    ) -> bool {
        if paths.is_empty() {
            return true;
        }
        let Some(intermediate_texture) = &self.path_intermediate_texture else {
            return false;
        };

        let render_pass_descriptor = metal::RenderPassDescriptor::new();
        let color_attachment = render_pass_descriptor
            .color_attachments()
            .object_at(0)
            .unwrap();
        color_attachment.set_load_action(metal::MTLLoadAction::Clear);
        color_attachment.set_clear_color(metal::MTLClearColor::new(0., 0., 0., 0.));

        if let Some(msaa_texture) = &self.path_intermediate_msaa_texture {
            color_attachment.set_texture(Some(msaa_texture));
            color_attachment.set_resolve_texture(Some(intermediate_texture));
            color_attachment.set_store_action(metal::MTLStoreAction::MultisampleResolve);
        } else {
            color_attachment.set_texture(Some(intermediate_texture));
            color_attachment.set_store_action(metal::MTLStoreAction::Store);
        }

        let command_encoder = command_buffer.new_render_command_encoder(render_pass_descriptor);
        command_encoder.set_render_pipeline_state(&self.paths_rasterization_pipeline_state);

        align_offset(instance_offset);
        let mut vertices = Vec::new();
        for path in paths {
            vertices.extend(path.vertices.iter().map(|v| PathRasterizationVertex {
                xy_position: v.xy_position,
                st_position: v.st_position,
                color: path.color,
                bounds: path.bounds.intersect(&path.content_mask.bounds),
            }));
        }
        let vertices_bytes_len = mem::size_of_val(vertices.as_slice());
        let next_offset = *instance_offset + vertices_bytes_len;
        if next_offset > instance_buffer.size {
            command_encoder.end_encoding();
            return false;
        }
        command_encoder.set_vertex_buffer(
            PathRasterizationInputIndex::Vertices as u64,
            Some(&instance_buffer.metal_buffer),
            *instance_offset as u64,
        );
        command_encoder.set_vertex_bytes(
            PathRasterizationInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_fragment_buffer(
            PathRasterizationInputIndex::Vertices as u64,
            Some(&instance_buffer.metal_buffer),
            *instance_offset as u64,
        );
        let buffer_contents =
            unsafe { (instance_buffer.metal_buffer.contents() as *mut u8).add(*instance_offset) };
        unsafe {
            ptr::copy_nonoverlapping(
                vertices.as_ptr() as *const u8,
                buffer_contents,
                vertices_bytes_len,
            );
        }
        command_encoder.draw_primitives(
            metal::MTLPrimitiveType::Triangle,
            0,
            vertices.len() as u64,
        );
        *instance_offset = next_offset;

        command_encoder.end_encoding();
        true
    }

    fn draw_shadows(
        &self,
        shadows: &[Shadow],
        instance_buffer: &mut InstanceBuffer,
        instance_offset: &mut usize,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) -> bool {
        if shadows.is_empty() {
            return true;
        }
        align_offset(instance_offset);

        command_encoder.set_render_pipeline_state(&self.shadows_pipeline_state);
        command_encoder.set_vertex_buffer(
            ShadowInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            ShadowInputIndex::Shadows as u64,
            Some(&instance_buffer.metal_buffer),
            *instance_offset as u64,
        );
        command_encoder.set_fragment_buffer(
            ShadowInputIndex::Shadows as u64,
            Some(&instance_buffer.metal_buffer),
            *instance_offset as u64,
        );

        command_encoder.set_vertex_bytes(
            ShadowInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        let shadow_bytes_len = mem::size_of_val(shadows);
        let buffer_contents =
            unsafe { (instance_buffer.metal_buffer.contents() as *mut u8).add(*instance_offset) };

        let next_offset = *instance_offset + shadow_bytes_len;
        if next_offset > instance_buffer.size {
            return false;
        }

        unsafe {
            ptr::copy_nonoverlapping(
                shadows.as_ptr() as *const u8,
                buffer_contents,
                shadow_bytes_len,
            );
        }

        command_encoder.draw_primitives_instanced(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            shadows.len() as u64,
        );
        *instance_offset = next_offset;
        true
    }

    fn draw_quads(
        &self,
        quads: &[Quad],
        instance_buffer: &mut InstanceBuffer,
        instance_offset: &mut usize,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) -> bool {
        if quads.is_empty() {
            return true;
        }
        align_offset(instance_offset);

        command_encoder.set_render_pipeline_state(&self.quads_pipeline_state);
        command_encoder.set_vertex_buffer(
            QuadInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            QuadInputIndex::Quads as u64,
            Some(&instance_buffer.metal_buffer),
            *instance_offset as u64,
        );
        command_encoder.set_fragment_buffer(
            QuadInputIndex::Quads as u64,
            Some(&instance_buffer.metal_buffer),
            *instance_offset as u64,
        );

        command_encoder.set_vertex_bytes(
            QuadInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        let quad_bytes_len = mem::size_of_val(quads);
        let buffer_contents =
            unsafe { (instance_buffer.metal_buffer.contents() as *mut u8).add(*instance_offset) };

        let next_offset = *instance_offset + quad_bytes_len;
        if next_offset > instance_buffer.size {
            return false;
        }

        unsafe {
            ptr::copy_nonoverlapping(quads.as_ptr() as *const u8, buffer_contents, quad_bytes_len);
        }

        command_encoder.draw_primitives_instanced(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            quads.len() as u64,
        );
        *instance_offset = next_offset;
        true
    }

    fn draw_paths_from_intermediate(
        &self,
        paths: &[Path<ScaledPixels>],
        instance_buffer: &mut InstanceBuffer,
        instance_offset: &mut usize,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) -> bool {
        let Some(first_path) = paths.first() else {
            return true;
        };

        let Some(ref intermediate_texture) = self.path_intermediate_texture else {
            return false;
        };

        command_encoder.set_render_pipeline_state(&self.path_sprites_pipeline_state);
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        command_encoder.set_fragment_texture(
            SpriteInputIndex::AtlasTexture as u64,
            Some(intermediate_texture),
        );

        // When copying paths from the intermediate texture to the drawable,
        // each pixel must only be copied once, in case of transparent paths.
        //
        // If all paths have the same draw order, then their bounds are all
        // disjoint, so we can copy each path's bounds individually. If this
        // batch combines different draw orders, we perform a single copy
        // for a minimal spanning rect.
        let sprites;
        if paths.last().unwrap().order == first_path.order {
            sprites = paths
                .iter()
                .map(|path| PathSprite {
                    bounds: path.clipped_bounds(),
                })
                .collect();
        } else {
            let mut bounds = first_path.clipped_bounds();
            for path in paths.iter().skip(1) {
                bounds = bounds.union(&path.clipped_bounds());
            }
            sprites = vec![PathSprite { bounds }];
        }

        align_offset(instance_offset);
        let sprite_bytes_len = mem::size_of_val(sprites.as_slice());
        let next_offset = *instance_offset + sprite_bytes_len;
        if next_offset > instance_buffer.size {
            return false;
        }

        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&instance_buffer.metal_buffer),
            *instance_offset as u64,
        );

        let buffer_contents =
            unsafe { (instance_buffer.metal_buffer.contents() as *mut u8).add(*instance_offset) };
        unsafe {
            ptr::copy_nonoverlapping(
                sprites.as_ptr() as *const u8,
                buffer_contents,
                sprite_bytes_len,
            );
        }

        command_encoder.draw_primitives_instanced(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            sprites.len() as u64,
        );
        *instance_offset = next_offset;

        true
    }

    fn draw_underlines(
        &self,
        underlines: &[Underline],
        instance_buffer: &mut InstanceBuffer,
        instance_offset: &mut usize,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) -> bool {
        if underlines.is_empty() {
            return true;
        }
        align_offset(instance_offset);

        command_encoder.set_render_pipeline_state(&self.underlines_pipeline_state);
        command_encoder.set_vertex_buffer(
            UnderlineInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            UnderlineInputIndex::Underlines as u64,
            Some(&instance_buffer.metal_buffer),
            *instance_offset as u64,
        );
        command_encoder.set_fragment_buffer(
            UnderlineInputIndex::Underlines as u64,
            Some(&instance_buffer.metal_buffer),
            *instance_offset as u64,
        );

        command_encoder.set_vertex_bytes(
            UnderlineInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        let underline_bytes_len = mem::size_of_val(underlines);
        let buffer_contents =
            unsafe { (instance_buffer.metal_buffer.contents() as *mut u8).add(*instance_offset) };

        let next_offset = *instance_offset + underline_bytes_len;
        if next_offset > instance_buffer.size {
            return false;
        }

        unsafe {
            ptr::copy_nonoverlapping(
                underlines.as_ptr() as *const u8,
                buffer_contents,
                underline_bytes_len,
            );
        }

        command_encoder.draw_primitives_instanced(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            underlines.len() as u64,
        );
        *instance_offset = next_offset;
        true
    }

    fn draw_monochrome_sprites(
        &self,
        texture_id: AtlasTextureId,
        sprites: &[MonochromeSprite],
        instance_buffer: &mut InstanceBuffer,
        instance_offset: &mut usize,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) -> bool {
        self.draw_monochrome_sprites_with_properties(
            texture_id,
            sprites,
            instance_buffer,
            instance_offset,
            viewport_size,
            None,
            command_encoder,
        )
    }

    fn draw_monochrome_sprites_with_properties(
        &self,
        texture_id: AtlasTextureId,
        sprites: &[MonochromeSprite],
        instance_buffer: &mut InstanceBuffer,
        instance_offset: &mut usize,
        viewport_size: Size<DevicePixels>,
        property_offset: Option<usize>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) -> bool {
        if sprites.is_empty() {
            return true;
        }
        align_offset(instance_offset);

        let sprite_bytes_len = mem::size_of_val(sprites);
        let buffer_contents =
            unsafe { (instance_buffer.metal_buffer.contents() as *mut u8).add(*instance_offset) };

        let next_offset = *instance_offset + sprite_bytes_len;
        if next_offset > instance_buffer.size {
            return false;
        }

        let texture = self.sprite_atlas.metal_texture(texture_id);
        let texture_size = size(
            DevicePixels(texture.width() as i32),
            DevicePixels(texture.height() as i32),
        );
        command_encoder.set_render_pipeline_state(&self.monochrome_sprites_pipeline_state);
        if let Some(offset) = property_offset {
            command_encoder.set_vertex_buffer(
                SpriteInputIndex::Properties as u64,
                Some(&instance_buffer.metal_buffer),
                offset as u64,
            );
        } else {
            let dummy = crate::retained_properties::GpuSpriteProperties::default();
            command_encoder.set_vertex_bytes(
                SpriteInputIndex::Properties as u64,
                mem::size_of_val(&dummy) as u64,
                &dummy as *const _ as *const _,
            );
        }
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&instance_buffer.metal_buffer),
            *instance_offset as u64,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::AtlasTextureSize as u64,
            mem::size_of_val(&texture_size) as u64,
            &texture_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_fragment_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&instance_buffer.metal_buffer),
            *instance_offset as u64,
        );
        command_encoder.set_fragment_texture(SpriteInputIndex::AtlasTexture as u64, Some(&texture));

        unsafe {
            ptr::copy_nonoverlapping(
                sprites.as_ptr() as *const u8,
                buffer_contents,
                sprite_bytes_len,
            );
        }

        command_encoder.draw_primitives_instanced(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            sprites.len() as u64,
        );
        *instance_offset = next_offset;
        true
    }

    fn draw_polychrome_sprites(
        &self,
        texture_id: AtlasTextureId,
        sprites: &[PolychromeSprite],
        instance_buffer: &mut InstanceBuffer,
        instance_offset: &mut usize,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) -> bool {
        self.draw_polychrome_sprites_with_properties(
            texture_id,
            sprites,
            instance_buffer,
            instance_offset,
            viewport_size,
            None,
            command_encoder,
        )
    }

    fn draw_polychrome_sprites_with_properties(
        &self,
        texture_id: AtlasTextureId,
        sprites: &[PolychromeSprite],
        instance_buffer: &mut InstanceBuffer,
        instance_offset: &mut usize,
        viewport_size: Size<DevicePixels>,
        property_offset: Option<usize>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) -> bool {
        if sprites.is_empty() {
            return true;
        }
        align_offset(instance_offset);

        let texture = self.sprite_atlas.metal_texture(texture_id);
        let texture_size = size(
            DevicePixels(texture.width() as i32),
            DevicePixels(texture.height() as i32),
        );
        command_encoder.set_render_pipeline_state(&self.polychrome_sprites_pipeline_state);
        if let Some(offset) = property_offset {
            command_encoder.set_vertex_buffer(
                SpriteInputIndex::Properties as u64,
                Some(&instance_buffer.metal_buffer),
                offset as u64,
            );
        } else {
            let dummy = crate::retained_properties::GpuSpriteProperties::default();
            command_encoder.set_vertex_bytes(
                SpriteInputIndex::Properties as u64,
                mem::size_of_val(&dummy) as u64,
                &dummy as *const _ as *const _,
            );
        }
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&instance_buffer.metal_buffer),
            *instance_offset as u64,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::AtlasTextureSize as u64,
            mem::size_of_val(&texture_size) as u64,
            &texture_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_fragment_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&instance_buffer.metal_buffer),
            *instance_offset as u64,
        );
        command_encoder.set_fragment_texture(SpriteInputIndex::AtlasTexture as u64, Some(&texture));

        let sprite_bytes_len = mem::size_of_val(sprites);
        let buffer_contents =
            unsafe { (instance_buffer.metal_buffer.contents() as *mut u8).add(*instance_offset) };

        let next_offset = *instance_offset + sprite_bytes_len;
        if next_offset > instance_buffer.size {
            return false;
        }

        unsafe {
            ptr::copy_nonoverlapping(
                sprites.as_ptr() as *const u8,
                buffer_contents,
                sprite_bytes_len,
            );
        }

        command_encoder.draw_primitives_instanced(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            sprites.len() as u64,
        );
        *instance_offset = next_offset;
        true
    }

    fn draw_surfaces(
        &mut self,
        surfaces: &[PaintSurface],
        instance_buffer: &mut InstanceBuffer,
        instance_offset: &mut usize,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) -> bool {
        command_encoder.set_render_pipeline_state(&self.surfaces_pipeline_state);
        command_encoder.set_vertex_buffer(
            SurfaceInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_bytes(
            SurfaceInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        for surface in surfaces {
            let texture_size = size(
                DevicePixels::from(surface.image_buffer.get_width() as i32),
                DevicePixels::from(surface.image_buffer.get_height() as i32),
            );

            assert_eq!(
                surface.image_buffer.get_pixel_format(),
                kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
            );

            let y_texture = self
                .core_video_texture_cache
                .create_texture_from_image(
                    surface.image_buffer.as_concrete_TypeRef(),
                    None,
                    MTLPixelFormat::R8Unorm,
                    surface.image_buffer.get_width_of_plane(0),
                    surface.image_buffer.get_height_of_plane(0),
                    0,
                )
                .unwrap();
            let cb_cr_texture = self
                .core_video_texture_cache
                .create_texture_from_image(
                    surface.image_buffer.as_concrete_TypeRef(),
                    None,
                    MTLPixelFormat::RG8Unorm,
                    surface.image_buffer.get_width_of_plane(1),
                    surface.image_buffer.get_height_of_plane(1),
                    1,
                )
                .unwrap();

            align_offset(instance_offset);
            let next_offset = *instance_offset + mem::size_of::<Surface>();
            if next_offset > instance_buffer.size {
                return false;
            }

            command_encoder.set_vertex_buffer(
                SurfaceInputIndex::Surfaces as u64,
                Some(&instance_buffer.metal_buffer),
                *instance_offset as u64,
            );
            command_encoder.set_vertex_bytes(
                SurfaceInputIndex::TextureSize as u64,
                mem::size_of_val(&texture_size) as u64,
                &texture_size as *const Size<DevicePixels> as *const _,
            );
            // let y_texture = y_texture.get_texture().unwrap().
            command_encoder.set_fragment_texture(SurfaceInputIndex::YTexture as u64, unsafe {
                let texture = CVMetalTextureGetTexture(y_texture.as_concrete_TypeRef());
                Some(metal::TextureRef::from_ptr(texture as *mut _))
            });
            command_encoder.set_fragment_texture(SurfaceInputIndex::CbCrTexture as u64, unsafe {
                let texture = CVMetalTextureGetTexture(cb_cr_texture.as_concrete_TypeRef());
                Some(metal::TextureRef::from_ptr(texture as *mut _))
            });

            unsafe {
                let buffer_contents = (instance_buffer.metal_buffer.contents() as *mut u8)
                    .add(*instance_offset)
                    as *mut SurfaceBounds;
                ptr::write(
                    buffer_contents,
                    SurfaceBounds {
                        bounds: surface.bounds,
                        content_mask: surface.content_mask.clone(),
                    },
                );
            }

            command_encoder.draw_primitives(metal::MTLPrimitiveType::Triangle, 0, 6);
            *instance_offset = next_offset;
        }
        true
    }
}

fn new_command_encoder<'a>(
    command_buffer: &'a metal::CommandBufferRef,
    texture: &metal::TextureRef,
    viewport_size: Size<DevicePixels>,
    configure_color_attachment: impl Fn(&RenderPassColorAttachmentDescriptorRef),
) -> &'a metal::RenderCommandEncoderRef {
    let render_pass_descriptor = metal::RenderPassDescriptor::new();
    let color_attachment = render_pass_descriptor
        .color_attachments()
        .object_at(0)
        .unwrap();
    color_attachment.set_texture(Some(texture));
    color_attachment.set_store_action(metal::MTLStoreAction::Store);
    configure_color_attachment(color_attachment);

    let command_encoder = command_buffer.new_render_command_encoder(render_pass_descriptor);
    command_encoder.set_viewport(metal::MTLViewport {
        originX: 0.0,
        originY: 0.0,
        width: i32::from(viewport_size.width) as f64,
        height: i32::from(viewport_size.height) as f64,
        znear: 0.0,
        zfar: 1.0,
    });
    command_encoder
}

fn build_pipeline_state(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: metal::MTLPixelFormat,
) -> metal::RenderPipelineState {
    try_build_pipeline_state(device, library, label, vertex_fn_name, fragment_fn_name, pixel_format).unwrap_or_else(|error| panic!("Metal pipeline {label} failed: {error}"))
}

fn try_build_pipeline_state(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: metal::MTLPixelFormat,
) -> std::result::Result<metal::RenderPipelineState, String> {
    let vertex_fn = library
        .get_function(vertex_fn_name, None)?;
    let fragment_fn = library
        .get_function(fragment_fn_name, None)?;

    let descriptor = metal::RenderPipelineDescriptor::new();
    descriptor.set_label(label);
    descriptor.set_vertex_function(Some(vertex_fn.as_ref()));
    descriptor.set_fragment_function(Some(fragment_fn.as_ref()));
    let color_attachment = descriptor.color_attachments().object_at(0).unwrap();
    color_attachment.set_pixel_format(pixel_format);
    color_attachment.set_blending_enabled(true);
    color_attachment.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_source_rgb_blend_factor(metal::MTLBlendFactor::SourceAlpha);
    color_attachment.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
    // Kai: preserve source-over coverage so Core Animation can composite
    // the native backdrop through overlapping translucent GPUI primitives.
    color_attachment.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);

    device
        .new_render_pipeline_state(&descriptor)
}

fn build_path_sprite_pipeline_state(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: metal::MTLPixelFormat,
) -> metal::RenderPipelineState {
    try_build_path_sprite_pipeline_state(device, library, label, vertex_fn_name, fragment_fn_name, pixel_format).unwrap_or_else(|error| panic!("Metal pipeline {label} failed: {error}"))
}

fn try_build_path_sprite_pipeline_state(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: metal::MTLPixelFormat,
) -> std::result::Result<metal::RenderPipelineState, String> {
    let vertex_fn = library
        .get_function(vertex_fn_name, None)?;
    let fragment_fn = library
        .get_function(fragment_fn_name, None)?;

    let descriptor = metal::RenderPipelineDescriptor::new();
    descriptor.set_label(label);
    descriptor.set_vertex_function(Some(vertex_fn.as_ref()));
    descriptor.set_fragment_function(Some(fragment_fn.as_ref()));
    let color_attachment = descriptor.color_attachments().object_at(0).unwrap();
    color_attachment.set_pixel_format(pixel_format);
    color_attachment.set_blending_enabled(true);
    color_attachment.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_source_rgb_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
    // Kai: preserve source-over coverage so Core Animation can composite
    // the native backdrop through overlapping translucent GPUI primitives.
    color_attachment.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);

    device
        .new_render_pipeline_state(&descriptor)
}

fn build_path_rasterization_pipeline_state(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: metal::MTLPixelFormat,
    path_sample_count: u32,
) -> metal::RenderPipelineState {
    try_build_path_rasterization_pipeline_state(device, library, label, vertex_fn_name, fragment_fn_name, pixel_format, path_sample_count).unwrap_or_else(|error| panic!("Metal pipeline {label} failed: {error}"))
}

fn try_build_path_rasterization_pipeline_state(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: metal::MTLPixelFormat,
    path_sample_count: u32,
) -> std::result::Result<metal::RenderPipelineState, String> {
    let vertex_fn = library
        .get_function(vertex_fn_name, None)?;
    let fragment_fn = library
        .get_function(fragment_fn_name, None)?;

    let descriptor = metal::RenderPipelineDescriptor::new();
    descriptor.set_label(label);
    descriptor.set_vertex_function(Some(vertex_fn.as_ref()));
    descriptor.set_fragment_function(Some(fragment_fn.as_ref()));
    if path_sample_count > 1 {
        descriptor.set_raster_sample_count(path_sample_count as _);
        descriptor.set_alpha_to_coverage_enabled(false);
    }
    let color_attachment = descriptor.color_attachments().object_at(0).unwrap();
    color_attachment.set_pixel_format(pixel_format);
    color_attachment.set_blending_enabled(true);
    color_attachment.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_source_rgb_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);

    device
        .new_render_pipeline_state(&descriptor)
}

// Align to multiples of 256 make Metal happy.
fn align_offset(offset: &mut usize) {
    *offset = (*offset).div_ceil(256) * 256;
}

#[repr(C)]
enum ShadowInputIndex {
    Vertices = 0,
    Shadows = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum QuadInputIndex {
    Vertices = 0,
    Quads = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum UnderlineInputIndex {
    Vertices = 0,
    Underlines = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum SpriteInputIndex {
    Vertices = 0,
    Sprites = 1,
    ViewportSize = 2,
    AtlasTextureSize = 3,
    AtlasTexture = 4,
    Properties = 5,
}

#[repr(C)]
enum SurfaceInputIndex {
    Vertices = 0,
    Surfaces = 1,
    ViewportSize = 2,
    TextureSize = 3,
    YTexture = 4,
    CbCrTexture = 5,
}

#[repr(C)]
enum PathRasterizationInputIndex {
    Vertices = 0,
    ViewportSize = 1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct PathSprite {
    pub bounds: Bounds<ScaledPixels>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct SurfaceBounds {
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
}

/// Verify the same pipeline builders used by real quads, text, images and paths
/// against a one-pixel GPU render target. Native blur shows through this alpha.
pub fn verify_source_over_blending() -> anyhow::Result<()> {
    let device = metal::Device::system_default().ok_or_else(|| anyhow::anyhow!("No Metal device"))?;
    let library = device.new_library_with_source(r#"
        #include <metal_stdlib>
        using namespace metal;
        vertex float4 glass_probe_vertex(uint id [[vertex_id]]) {
            float2 p[3] = {float2(-1., -1.), float2(3., -1.), float2(-1., 3.)};
            return float4(p[id], 0., 1.);
        }
        fragment float4 glass_probe_fragment(constant float4 &color [[buffer(0)]]) { return color; }
    "#, &metal::CompileOptions::new()).map_err(|error| anyhow::anyhow!(error))?;
    let queue = device.new_command_queue();
    for format in [compositing_format(false), compositing_format(true)] {
    for premultiplied in [false, true] {
        let pipeline = if premultiplied {
            build_path_sprite_pipeline_state(&device, &library, "glass-source-over-path",
                "glass_probe_vertex", "glass_probe_fragment", format)
        } else {
            build_pipeline_state(&device, &library, "glass-source-over-quad",
                "glass_probe_vertex", "glass_probe_fragment", format)
        };
        for (name, colors) in [
            ("overlap", vec![[0.8f32, 0.4, 0.2, 0.25], [0.2, 0.6, 0.9, 0.4]]),
            ("opaque", vec![[0.8, 0.4, 0.2, 1.], [0.2, 0.6, 0.9, 1.]]),
            ("ten-transparent-layers", vec![[0.2, 0.4, 0.6, 0.1]; 10]),
        ] {
            let descriptor = metal::TextureDescriptor::new();
            descriptor.set_texture_type(metal::MTLTextureType::D2);
            descriptor.set_pixel_format(format);
            descriptor.set_width(1); descriptor.set_height(1);
            descriptor.set_usage(metal::MTLTextureUsage::RenderTarget);
            descriptor.set_storage_mode(metal::MTLStorageMode::Shared);
            let texture = device.new_texture(&descriptor);
            let pass = metal::RenderPassDescriptor::new();
            let attachment = pass.color_attachments().object_at(0).unwrap();
            attachment.set_texture(Some(&texture));
            attachment.set_load_action(metal::MTLLoadAction::Clear);
            attachment.set_store_action(metal::MTLStoreAction::Store);
            attachment.set_clear_color(metal::MTLClearColor::new(0., 0., 0., 0.));
            let command = queue.new_command_buffer();
            let encoder = command.new_render_command_encoder(pass);
            encoder.set_render_pipeline_state(&pipeline);
            let mut expected = [0f32; 4];
            for color in colors {
                let mut uploaded = color;
                if premultiplied { for c in &mut uploaded[..3] { *c *= color[3]; } }
                encoder.set_fragment_bytes(0, std::mem::size_of_val(&uploaded) as u64, uploaded.as_ptr().cast());
                encoder.draw_primitives(metal::MTLPrimitiveType::Triangle, 0, 3);
                for c in 0..3 { expected[c] = color[c] * color[3] + expected[c] * (1. - color[3]); }
                expected[3] = color[3] + expected[3] * (1. - color[3]);
            }
            encoder.end_encoding();
            command.commit(); command.wait_until_completed();
            let mut actual = [0u8; 8];
            let wide = format == MTLPixelFormat::RGBA16Float;
            texture.get_bytes(actual.as_mut_ptr().cast(), if wide {8} else {4}, metal::MTLRegion::new_2d(0, 0, 1, 1), 0);
            for channel in 0..4 {
                let (value, reference, tolerance) = if wide {
                    (half_float(u16::from_ne_bytes([actual[channel*2], actual[channel*2+1]])), expected[channel], 0.002)
                } else {
                    (actual[channel] as f32 / 255., expected[[2,1,0,3][channel]], 2. / 255.)
                };
                anyhow::ensure!((value - reference).abs() <= tolerance,
                    "Metal {name} wide={wide} premultiplied={premultiplied} channel={channel}: expected {reference}, got {value}");
            }
        }
    }
    }
    verify_gradient_precision(&device, &queue)?;
    let context = Context::default();
    let mut renderer = MetalRenderer::new(context.clone(), true);
    let resources = context.resources().map_err(anyhow::Error::msg)?;
    menu_backdrop::verify(&resources.device, &resources.library)?;
    let second = MetalRenderer::new(context.clone(), false);
    anyhow::ensure!(renderer.device.as_ptr() == second.device.as_ptr(), "Windows must retain one selected Metal device");
    anyhow::ensure!(renderer.paths_rasterization_pipeline_state.as_ptr() == second.paths_rasterization_pipeline_state.as_ptr(), "Common immutable path pipeline must be reused");
    anyhow::ensure!(renderer.layer.as_ptr() != second.layer.as_ptr(), "Windows must not share mutable Metal layers");
    anyhow::ensure!(renderer.command_queue.as_ptr() != second.command_queue.as_ptr(), "Windows retain independent command queues");
    anyhow::ensure!(!Arc::ptr_eq(&renderer.sprite_atlas, &second.sprite_atlas), "Windows must not share mutable sprite atlases");
    anyhow::ensure!(Arc::ptr_eq(&renderer.instance_buffer_pool, &second.instance_buffer_pool), "Retain the existing completed-buffer reuse pool");
    let transparent_pipeline = renderer.quads_pipeline_state.as_ptr();
    let opaque_pipeline = second.quads_pipeline_state.as_ptr();
    anyhow::ensure!(renderer.layer.pixel_format() == MTLPixelFormat::RGBA16Float && !renderer.layer.is_opaque(), "Glass must use high-precision transparent drawables");
    renderer.update_transparency(false);
    anyhow::ensure!(renderer.quads_pipeline_state.as_ptr() == opaque_pipeline, "Opaque round trip must reuse the existing pipeline");
    anyhow::ensure!(renderer.layer.pixel_format() == MTLPixelFormat::BGRA8Unorm && renderer.layer.is_opaque(), "Opaque mode must restore smaller opaque drawables");
    renderer.update_transparency(true);
    anyhow::ensure!(renderer.quads_pipeline_state.as_ptr() == transparent_pipeline, "Transparent round trip must reuse the existing pipeline");
    anyhow::ensure!(renderer.layer.pixel_format() == MTLPixelFormat::RGBA16Float && !renderer.layer.is_opaque(), "Glass precision must survive an opaque round trip");
    // Exercise real encoded Metal references across explicit hiding, before
    // committing work, and verify the same viewport is recreated on reveal.
    let atlas_before = Arc::clone(&renderer.sprite_atlas);
    let small_viewport = size(DevicePixels(8), DevicePixels(8));
    renderer.prepare_path_intermediate_textures(small_viewport);
    let initial_target = renderer.path_intermediate_texture.as_ref().unwrap().as_ptr();
    let command = renderer.command_queue.new_command_buffer().to_owned();
    let retained: bool = unsafe { msg_send![&*command, retainedReferences] };
    anyhow::ensure!(retained, "Path trim requires standard retaining command buffers");
    {
        let pass = metal::RenderPassDescriptor::new();
        let attachment = pass.color_attachments().object_at(0).unwrap();
        attachment.set_texture(renderer.path_intermediate_msaa_texture.as_deref());
        attachment.set_resolve_texture(renderer.path_intermediate_texture.as_deref());
        attachment.set_load_action(metal::MTLLoadAction::Clear);
        attachment.set_store_action(metal::MTLStoreAction::MultisampleResolve);
        attachment.set_clear_color(metal::MTLClearColor::new(0.25, 0.5, 0.75, 1.));
        let encoder = command.new_render_command_encoder(pass);
        encoder.end_encoding();
    }
    renderer.set_surface_visible(false);
    anyhow::ensure!(renderer.path_texture_size.is_none() && renderer.path_intermediate_texture.is_none()
        && renderer.path_intermediate_msaa_texture.is_none(), "Hidden renderer must release reusable path target references");
    renderer.set_surface_visible(false); // idempotent visibility notification
    renderer.set_surface_visible(true);
    renderer.prepare_path_intermediate_textures(small_viewport);
    anyhow::ensure!(renderer.path_texture_size == Some(small_viewport)
        && renderer.path_intermediate_msaa_texture.is_some(), "Reveal must recreate targets even at unchanged viewport size");
    anyhow::ensure!(renderer.path_intermediate_texture.as_ref().unwrap().as_ptr() != initial_target,
        "Old encoded target must remain retained while recreated targets are distinct");
    anyhow::ensure!(Arc::ptr_eq(&atlas_before, &renderer.sprite_atlas), "Path trim must preserve glyph/SVG/image atlas");
    let recreated_target = renderer.path_intermediate_texture.as_ref().unwrap().as_ptr();
    renderer.prepare_path_intermediate_textures(small_viewport);
    anyhow::ensure!(renderer.path_intermediate_texture.as_ref().unwrap().as_ptr() == recreated_target,
        "Visible equal-size frames must reuse path targets without churn");
    command.commit();
    command.wait_until_completed(); // GPU regression only; never in production trim
    anyhow::ensure!(command.status() == metal::MTLCommandBufferStatus::Completed,
        "Encoded path targets must survive release of renderer references until GPU completion");
    eprintln!("Kai Metal hidden path trim: release before GPU commit, retained encoded references, same-size reveal recreation, atlas preservation and visible reuse passed");
    eprintln!("Kai Metal precision: 256 distinct dark-gradient coverage levels retained; opaque BGRA8/glass RGBA16Float round trips passed");
    anyhow::ensure!(context.0.builds.iter().all(|count| count.load(Ordering::Relaxed) == 1), "Each device/library and drawable-format set must initialize once, including warmup races");
    // Exercise the same retry cell used by GPU warmup without manufacturing an
    // invalid GPU command or depending on an actual driver failure.
    let retry = Mutex::new(None::<Arc<usize>>);
    anyhow::ensure!(cached_resource(&retry, || Err("simulated warmup failure".into())).is_err() && retry.lock().is_none(), "Warmup failure must leave the cache retryable");
    let recovered = cached_resource(&retry, || Ok(7)).map_err(anyhow::Error::msg)?;
    let reused = cached_resource(&retry, || Err("cached values must not rebuild".into())).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(Arc::ptr_eq(&recovered, &reused) && *reused == 7, "Foreground retry must recover and retain its cached result");
    eprintln!("Kai Metal shared context: device/library=1 opaque=1 transparent=1; separate layers/queues/atlases; retry-after-failure passed");
    Ok(())
}

fn half_float(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1. } else { 1. };
    let exponent = ((bits >> 10) & 31) as i32;
    let fraction = (bits & 1023) as f32;
    if exponent == 0 { sign * fraction * 2f32.powi(-24) }
    else { sign * (1. + fraction / 1024.) * 2f32.powi(exponent - 15) }
}
fn verify_gradient_precision(device: &metal::DeviceRef, queue: &metal::CommandQueueRef) -> anyhow::Result<()> {
    let library = device.new_library_with_source(r#"
        #include <metal_stdlib>
        using namespace metal;
        vertex float4 precision_vertex(uint id [[vertex_id]]) {
            float2 p[3]={float2(-1.,-1.),float2(3.,-1.),float2(-1.,3.)};return float4(p[id],0.,1.);
        }
        fragment float4 precision_fragment(float4 position [[position]]) {
            return float4(0.5,0.4,0.3,0.02+position.x*0.06/256.);
        }
    "#,&metal::CompileOptions::new()).map_err(|error| anyhow::anyhow!(error))?;
    for format in [compositing_format(false), compositing_format(true)] {
        let wide=format==MTLPixelFormat::RGBA16Float;
        let pipeline=build_pipeline_state(device,&library,"glass-gradient-precision","precision_vertex","precision_fragment",format);
        let descriptor=metal::TextureDescriptor::new();descriptor.set_texture_type(metal::MTLTextureType::D2);
        descriptor.set_pixel_format(format);descriptor.set_width(256);descriptor.set_height(1);
        descriptor.set_usage(metal::MTLTextureUsage::RenderTarget);descriptor.set_storage_mode(metal::MTLStorageMode::Shared);
        let texture=device.new_texture(&descriptor);let pass=metal::RenderPassDescriptor::new();
        let attachment=pass.color_attachments().object_at(0).unwrap();attachment.set_texture(Some(&texture));
        attachment.set_load_action(metal::MTLLoadAction::Clear);attachment.set_store_action(metal::MTLStoreAction::Store);
        attachment.set_clear_color(metal::MTLClearColor::new(0.,0.,0.,0.));
        let command=queue.new_command_buffer();let encoder=command.new_render_command_encoder(pass);
        encoder.set_render_pipeline_state(&pipeline);encoder.draw_primitives(metal::MTLPrimitiveType::Triangle,0,3);
        encoder.end_encoding();command.commit();command.wait_until_completed();
        let stride=if wide{8}else{4};let mut bytes=vec![0u8;256*stride];
        texture.get_bytes(bytes.as_mut_ptr().cast(),(256*stride) as u64,metal::MTLRegion::new_2d(0,0,256,1),0);
        let distinct:std::collections::HashSet<u16>=bytes.chunks_exact(stride).map(|p|if wide{u16::from_ne_bytes([p[6],p[7]])}else{p[3] as u16}).collect();
        anyhow::ensure!(if wide{distinct.len()>240}else{distinct.len()<20},"Unexpected glass precision wide={wide}: {} gradient levels",distinct.len());
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn hidden_path_scratch_targets_retire_and_recreate() {
    verify_source_over_blending().expect("real GPU hidden path target retirement and recreation");
}

// Apple family2+ supports memoryless render targets; Mac-only/unknown staysPrivate.
// Availability ismacOS11+, below this app'smacOS13 deployment target.
fn path_msaa_storage_mode(supports_apple_family: bool) -> metal::MTLStorageMode {
    if supports_apple_family { metal::MTLStorageMode::Memoryless } else { metal::MTLStorageMode::Private }
}

#[cfg(test)]
#[test]
fn path_msaa_storage_policy_preserves_unknown_and_mac_only_fallback() {
    assert_eq!(path_msaa_storage_mode(false), metal::MTLStorageMode::Private);
    assert_eq!(path_msaa_storage_mode(true), metal::MTLStorageMode::Memoryless);
}

#[cfg(test)]
fn msaa_private_baseline(renderer: &mut MetalRenderer, width: u64, height: u64) {
    if renderer.path_intermediate_msaa_texture.as_ref().is_some_and(|t| t.storage_mode() == metal::MTLStorageMode::Private) { return; }
    let descriptor = metal::TextureDescriptor::new();
    descriptor.set_width(width); descriptor.set_height(height);
    descriptor.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm);
    descriptor.set_texture_type(metal::MTLTextureType::D2Multisample);
    descriptor.set_sample_count(PATH_SAMPLE_COUNT as _);
    descriptor.set_usage(metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead);
    descriptor.set_storage_mode(metal::MTLStorageMode::Private);
    renderer.path_intermediate_msaa_texture = Some(renderer.device.new_texture(&descriptor));
}

#[cfg(test)]
fn msaa_encode_readback(renderer: &MetalRenderer, pipeline: &metal::RenderPipelineStateRef, width: u64, height: u64) -> (metal::CommandBuffer, metal::Buffer, usize) {
    let command = renderer.command_queue.new_command_buffer().to_owned();
    let retained: bool = unsafe { msg_send![&*command, retainedReferences] };
    assert!(retained, "Encoded targets must retain resources across retirement");
    let pass = metal::RenderPassDescriptor::new();
    let color = pass.color_attachments().object_at(0).unwrap();
    color.set_texture(renderer.path_intermediate_msaa_texture.as_deref());
    color.set_resolve_texture(renderer.path_intermediate_texture.as_deref());
    color.set_load_action(metal::MTLLoadAction::Clear);
    color.set_store_action(metal::MTLStoreAction::MultisampleResolve);
    color.set_clear_color(metal::MTLClearColor::new(0., 0., 0., 0.));
    let encoder = command.new_render_command_encoder(pass);
    encoder.set_render_pipeline_state(pipeline);
    encoder.draw_primitives(metal::MTLPrimitiveType::Triangle, 0, 3);
    encoder.end_encoding();
    let row = width * 4; // Workload widths are64/128/256:256-byte aligned.
    let length = (row * height) as usize;
    let buffer = renderer.device.new_buffer(length as _, MTLResourceOptions::StorageModeShared);
    let blit = command.new_blit_command_encoder();
    blit.copy_from_texture_to_buffer(renderer.path_intermediate_texture.as_ref().unwrap(), 0, 0,
        metal::MTLOrigin { x: 0, y: 0, z: 0 }, metal::MTLSize::new(width, height, 1),
        &buffer, 0, row, row * height, metal::MTLBlitOption::None);
    blit.end_encoding();
    (command, buffer, length)
}

#[cfg(test)]
fn msaa_retirement_workload(renderer: &mut MetalRenderer, pipeline: &metal::RenderPipelineStateRef, force_private: bool) -> anyhow::Result<Vec<Vec<u8>>> {
    renderer.set_surface_visible(true); // Allocation reporting retired its targets.
    let mut pending = Vec::new();
    let mut reused = None;
    for (index, (width, height)) in [(64, 64), (64, 64), (128, 96), (256, 128)].into_iter().enumerate() {
        if index >= 2 {
            renderer.set_surface_visible(false);
            anyhow::ensure!(renderer.path_texture_size.is_none() && renderer.path_intermediate_texture.is_none() && renderer.path_intermediate_msaa_texture.is_none(), "Retire both scratch references while encoded commands remain uncommitted");
            renderer.set_surface_visible(true);
        }
        renderer.prepare_path_intermediate_textures(size(DevicePixels(width), DevicePixels(height)));
        if force_private { msaa_private_baseline(renderer, width as _, height as _); }
        let msaa = renderer.path_intermediate_msaa_texture.as_ref().unwrap();
        anyhow::ensure!(msaa.sample_count() == u64::from(PATH_SAMPLE_COUNT), "Keep4x sample quality");
        if index == 1 { anyhow::ensure!(reused == Some(msaa.as_ptr()), "Visible equal-size frames must reuse attachment objects"); }
        reused = Some(msaa.as_ptr());
        let resolve = renderer.path_intermediate_texture.as_ref().unwrap();
        anyhow::ensure!(resolve.sample_count() == 1 && resolve.storage_mode() != metal::MTLStorageMode::Memoryless, "Cross-pass sampled resolve must remain backed and single-sampled");
        if msaa.storage_mode() == metal::MTLStorageMode::Memoryless {
            anyhow::ensure!(msaa.usage() == metal::MTLTextureUsage::RenderTarget, "Memoryless attachment is render-target-only");
        }
        pending.push(msaa_encode_readback(renderer, pipeline, width as _, height as _));
    }
    renderer.set_surface_visible(false); // All queued frames outlive renderer refs.
    for (command, _, _) in &pending { command.commit(); }
    let mut pixels = Vec::new();
    for (command, buffer, length) in pending {
        command.wait_until_completed(); // Test only; no wait added to product path.
        anyhow::ensure!(command.status() == metal::MTLCommandBufferStatus::Completed, "MSAA resolve/readback GPU failure");
        pixels.push(unsafe { std::slice::from_raw_parts(buffer.contents().cast::<u8>(), length) }.to_vec());
    }
    Ok(pixels)
}

#[cfg(test)]
fn msaa_report_allocations(renderer: &mut MetalRenderer, label: &str, force_private: bool) {
    renderer.set_surface_visible(true);
    renderer.prepare_path_intermediate_textures(size(DevicePixels(3420), DevicePixels(1958)));
    if force_private { msaa_private_baseline(renderer, 3420, 1958); }
    let msaa = renderer.path_intermediate_msaa_texture.as_ref().unwrap();
    let resolve = renderer.path_intermediate_texture.as_ref().unwrap();
    eprintln!("KAI_MSAA_ALLOCATION label={} reference_extent=3420x1958 samples={} storage={:?} msaa_api_allocated_bytes={} resolve_api_allocated_bytes={} device_global_current_bytes={} tile_memory_accounted=false process_savings_measured=false",
        label, msaa.sample_count(), msaa.storage_mode(), msaa.allocated_size(), resolve.allocated_size(), renderer.device.current_allocated_size());
    renderer.set_surface_visible(false);
}

#[cfg(test)]
#[test]
fn path_msaa_memoryless_matches_private_resolve_and_retirement() {
    objc::rc::autoreleasepool(|| -> anyhow::Result<()> {
        let context = Context::default();
        let mut baseline = MetalRenderer::new(context.clone(), true);
        let device = baseline.device.clone();
        let apple2 = device.supports_family(metal::MTLGPUFamily::Apple2);
        eprintln!("KAI_MSAA_DEVICE name={:?} apple2={} apple7={} mac2={} supports4x={} policy={:?}", device.name(), apple2,
            device.supports_family(metal::MTLGPUFamily::Apple7), device.supports_family(metal::MTLGPUFamily::Mac2), device.supports_texture_sample_count(4), path_msaa_storage_mode(apple2));
        anyhow::ensure!(device.supports_texture_sample_count(4), "This existing renderer requires4x MSAA");
        let library = device.new_library_with_source(r#"
            #include <metal_stdlib>
            using namespace metal;
            vertex float4 msaa_probe_vertex(uint id [[vertex_id]]) {
                float2 p[3]={float2(-0.73,-0.61),float2(0.79,-0.45),float2(-0.18,0.83)};
                return float4(p[id],0.,1.);
            }
            fragment float4 msaa_probe_fragment(){return float4(0.15,0.30,0.60,0.75);}
        "#, &metal::CompileOptions::new()).map_err(anyhow::Error::msg)?;
        let pipeline = try_build_path_rasterization_pipeline_state(&device, &library, "real-path-builder-msaa-probe", "msaa_probe_vertex", "msaa_probe_fragment", metal::MTLPixelFormat::BGRA8Unorm, PATH_SAMPLE_COUNT).map_err(anyhow::Error::msg)?;
        msaa_report_allocations(&mut baseline, "private-control", true);
        let private = msaa_retirement_workload(&mut baseline, &pipeline, true)?;
        for bytes in &private {
            anyhow::ensure!(bytes.chunks_exact(4).any(|p| p[3] == 0), "Require untouched transparent background");
            anyhow::ensure!(bytes.chunks_exact(4).any(|p| p[3] >= 190), "Require filled triangle interior");
            anyhow::ensure!(bytes.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 180), "Require partially covered antialiased edge pixels");
        }
        let hash = private.iter().flatten().fold(14695981039346656037u64, |h,b| (h ^ u64::from(*b)).wrapping_mul(1099511628211));
        eprintln!("KAI_MSAA_PRIVATE_READBACK frames={} bytes={} fnv64={:016x} queued_hide_reveal_resize=true", private.len(), private.iter().map(Vec::len).sum::<usize>(), hash);
        let mut candidate = MetalRenderer::new(context, true);
        msaa_report_allocations(&mut candidate, "candidate-policy", false);
        candidate.set_surface_visible(true);
        candidate.prepare_path_intermediate_textures(size(DevicePixels(64), DevicePixels(64)));
        anyhow::ensure!(candidate.path_intermediate_msaa_texture.as_ref().unwrap().storage_mode() == path_msaa_storage_mode(apple2), "Actual renderer must select supported memoryless or unchanged Private fallback");
        let proposed = msaa_retirement_workload(&mut candidate, &pipeline, false)?;
        anyhow::ensure!(private == proposed, "Every resolved BGRA pixel must match Private across reuse, queued retirement and resize");
        eprintln!("KAI_MSAA_COMPARE equal_pixels=true frames={} bytes={} memoryless_exercised={} tile_memory_still_used=true process_savings_measured=false", proposed.len(), proposed.iter().map(Vec::len).sum::<usize>(), apple2);
        Ok(())
    }).expect("real Metal4x path attachment policy/pixels/lifetime");
}

#[cfg(test)]
mod retained_image_opacity_gpu_tests {
    use super::*;
    use crate::{
        AtlasKey, ContentMask, Corners, ImageId, ImageOpacityPulse, PlatformAtlas,
        RenderImageParams, point,
    };
    use std::{
        borrow::Cow,
        time::{Duration, Instant},
    };

    fn encode_pixels(
        renderer: &MetalRenderer,
        sprites: &[PolychromeSprite],
        retire: Option<&AtlasKey>,
    ) -> Vec<u8> {
        let mut scene = Scene::default();
        // Callers supply finished instances, including submitted dither steps.
        // Encoding must preserve those steps without treating them as recipe IDs.
        scene.polychrome_sprites.extend_from_slice(sprites);
        encode_scene_pixels(renderer, &scene, retire)
    }

    fn encode_scene_pixels(renderer: &MetalRenderer, scene: &Scene, retire: Option<&AtlasKey>) -> Vec<u8> {
        let width = 64;
        let wide = renderer.layer.pixel_format() == MTLPixelFormat::RGBA16Float;
        let bytes_per_pixel = if wide { 8 } else { 4 };
        let descriptor = metal::TextureDescriptor::new();
        descriptor.set_texture_type(metal::MTLTextureType::D2);
        descriptor.set_pixel_format(renderer.layer.pixel_format());
        descriptor.set_width(width);
        descriptor.set_height(width);
        descriptor.set_storage_mode(metal::MTLStorageMode::Shared);
        descriptor.set_usage(metal::MTLTextureUsage::RenderTarget);
        let texture = renderer.device.new_texture(&descriptor);
        let pass = metal::RenderPassDescriptor::new();
        let color = pass.color_attachments().object_at(0).unwrap();
        color.set_texture(Some(&texture));
        color.set_load_action(metal::MTLLoadAction::Clear);
        color.set_store_action(metal::MTLStoreAction::Store);
        color.set_clear_color(metal::MTLClearColor::new(0., 0., 0., 0.));
        let command = renderer.command_queue.new_command_buffer().to_owned();
        let retained: bool = unsafe { msg_send![&*command, retainedReferences] };
        assert!(
            retained,
            "The actual retaining Metal command must own encoded image resources"
        );
        let encoder = command.new_render_command_encoder(pass);
        let mut buffer = InstanceBuffer {
            metal_buffer: renderer
                .device
                .new_buffer(65536, MTLResourceOptions::StorageModeShared),
            size: 65536,
        };
        let property_bytes = mem::size_of_val(scene.gpu_sprite_properties.as_slice());
        if property_bytes > 0 {
            unsafe { ptr::copy_nonoverlapping(scene.gpu_sprite_properties.as_ptr().cast::<u8>(),
                buffer.metal_buffer.contents().cast::<u8>(), property_bytes); }
        }
        let mut offset = property_bytes;
        align_offset(&mut offset);
        let properties = (property_bytes > 0).then_some(0);
        for batch in scene.batches() {
            let drawn = match batch {
                PrimitiveBatch::PolychromeSprites { texture_id, sprites } => renderer.draw_polychrome_sprites_with_properties(
                    texture_id, sprites, &mut buffer, &mut offset,
                    size(DevicePixels(64), DevicePixels(64)), properties, encoder),
                PrimitiveBatch::MonochromeSprites { texture_id, sprites } => renderer.draw_monochrome_sprites_with_properties(
                    texture_id, sprites, &mut buffer, &mut offset,
                    size(DevicePixels(64), DevicePixels(64)), properties, encoder),
                PrimitiveBatch::Quads(quads) => renderer.draw_quads(quads,
                    &mut buffer, &mut offset, size(DevicePixels(64), DevicePixels(64)), encoder),
                _ => panic!("Presentation GPU proof requires sprite and quad scenes"),
            };
            assert!(drawn);
        }
        encoder.end_encoding();
        if let Some(key) = retire {
            renderer.sprite_atlas.remove(key);
            renderer.sprite_atlas.remove(key);
        }
        command.commit();
        command.wait_until_completed(); // Owned GPU regression only.
        assert_eq!(command.status(), metal::MTLCommandBufferStatus::Completed);
        let row = width * bytes_per_pixel;
        let mut bytes = vec![0; (row * width) as usize];
        texture.get_bytes(
            bytes.as_mut_ptr().cast(),
            row,
            metal::MTLRegion::new_2d(0, 0, width, width),
            0,
        );
        bytes
    }

    fn normalized_components(bytes: &[u8], wide: bool) -> Vec<f32> {
        if !wide { return bytes.iter().map(|byte| *byte as f32 / 255.0).collect(); }
        bytes.chunks_exact(2).map(|pair| {
            let bits = u16::from_ne_bytes([pair[0], pair[1]]);
            let exponent = (bits >> 10) & 31;
            let fraction = (bits & 1023) as f32;
            let value = if exponent == 0 { fraction * 2.0f32.powi(-24) }
                else { (1.0 + fraction / 1024.0) * 2.0f32.powi(exponent as i32 - 15) };
            if bits & 0x8000 != 0 { -value } else { value }
        }).collect()
    }

    #[test]
    fn retained_shared_waves_and_sheet_vertices_match_legacy_metal_pixels() {
        use crate::retained_properties::{ResolvedProperties, SpriteFrames, progress};
        use crate::{OpacityAnimation, OpacityWaveform, px};
        let started = Instant::now();
        let context = Context::default();
        let mut compared_frames = 0usize;
        let mut max_error = [0.0f32; 2];
        for wide in [false, true] {
            let renderer = MetalRenderer::new(context.clone(), wide);
            let key: AtlasKey = RenderImageParams { image_id: ImageId(9161), frame_index: 0 }.into();
            let pixels: Vec<u8> = (0..128*16).flat_map(|index| {
                let column = index % 128 / 16;
                [24 + column as u8 * 26, 230 - column as u8 * 20, 87, 255]
            }).collect();
            let tile = renderer.sprite_atlas.get_or_insert_with(&key, &mut || {
                Ok(Some((size(DevicePixels(128), DevicePixels(16)), Cow::Borrowed(pixels.as_slice()))))
            }).unwrap().unwrap();
            for scale in [1.0f32, 1.5, 2.0] {
                for cell_size in [8.0f32, 8.375] {
                    for waveform in [OpacityWaveform::CosineTau, OpacityWaveform::HalfSinePi] {
                        let animation = OpacityAnimation { period: Duration::from_millis(1300), baseline: 0.72, amplitude: 0.28, waveform };
                        let frames = SpriteFrames::new(vec![0, 7, 3, 2, 6, 1, 4, 5], vec![16, 100, 33, 70, 17, 200, 91, 25], px(cell_size)).unwrap();
                        let origins = std::array::from_fn(|column| [((27.375 - column as f32 * cell_size) * scale).floor(), (7.375 * scale).floor()]);
                        let original = PolychromeSprite {
                            order: 0, opacity_pulse_index: 0, grayscale: false, opacity: 0.63,
                            bounds: Bounds::new(point(ScaledPixels(origins[0][0]), ScaledPixels(origins[0][1])), size(ScaledPixels((cell_size * 8.0 * scale).ceil()), ScaledPixels((cell_size * scale).ceil()))),
                            content_mask: ContentMask { bounds: Bounds::new(point(ScaledPixels(28.0 * scale), ScaledPixels(8.0 * scale)), size(ScaledPixels(cell_size * scale - 1.0), ScaledPixels(cell_size * scale - 1.0))) },
                            corner_radii: Corners::all(ScaledPixels(1.5 * scale)), tile: tile.clone(),
                        };
                        let mut scene = Scene::default();
                        scene.insert_sprite_properties(original.clone().into(), ResolvedProperties { opacity: Some((animation, started)), frames: Some((frames.clone(), started)) }, origins);
                        scene.finish();
                        let mut distinct_frames = std::collections::HashSet::new();
                        for elapsed in [0u64, 15_999, 16_000, 16_001, 115_999, 116_000, 116_001, 199_999, 551_999, 552_000, 1_300_000, 2_600_001] {
                            let now = started + Duration::from_micros(elapsed);
                            let mut legacy = original.clone();
                            legacy.opacity *= animation.value(progress(now, started, animation.period));
                            let column = frames.column(progress(now, started, Duration::from_millis(frames.total)));
                            legacy.bounds.origin = point(ScaledPixels(origins[column as usize][0]), ScaledPixels(origins[column as usize][1]));
                            let expected = normalized_components(&encode_pixels(&renderer, &[legacy], None), wide);
                            assert!(expected.iter().any(|component| *component > 1.0 / 255.0),
                                "Sheet legacy oracle must paint meaningful pixels at scale={scale} cell={cell_size} elapsed={elapsed}");
                            scene.sample_presentation_effects(now);
                            let actual_bytes = encode_scene_pixels(&renderer, &scene, None);
                            distinct_frames.insert(actual_bytes.clone());
                            let actual = normalized_components(&actual_bytes, wide);
                            let error = expected.iter().zip(actual).map(|(left, right)| (left - right).abs()).fold(0.0f32, f32::max);
                            max_error[usize::from(wide)] = max_error[usize::from(wide)].max(error);
                            assert!(error <= 1.0 / 255.0, "wave/sheet error={error} scale={scale} cell={cell_size} elapsed={elapsed} wave={waveform:?}");
                            compared_frames += 1;
                        }
                        assert!(distinct_frames.len() > 1,
                            "Sheet GPU oracle must change output across phases and columns");
                    }
                }
            }
        }
        eprintln!("KAI_RETAINED_SHARED_GPU compared_frames={compared_frames} max_bgra8_error={} max_rgba16_error={} tolerance={} exact_cpu_progress=true precomputed_column_origins=true", max_error[0], max_error[1], 1.0f32 / 255.0);
    }

    #[test]
    fn retained_monochrome_half_sine_matches_legacy_metal_mask_pixels() {
        use crate::{OpacityAnimation, OpacityWaveform, RenderSvgParams, TransformationMatrix, rgb};
        use crate::retained_properties::{ResolvedProperties, progress};
        let started = Instant::now();
        let context = Context::default();
        let mut compared_frames = 0usize;
        let mut max_error = 0.0f32;
        for wide in [false, true] {
            let renderer = MetalRenderer::new(context.clone(), wide);
            let key: AtlasKey = RenderSvgParams { path: "retained-svg-mask".into(), size: size(DevicePixels(8), DevicePixels(8)) }.into();
            let bytes = [192u8; 64];
            let tile = renderer.sprite_atlas.get_or_insert_with(&key, &mut || {
                Ok(Some((size(DevicePixels(8), DevicePixels(8)), Cow::Borrowed(bytes.as_slice()))))
            }).unwrap().unwrap();
            for (baseline, amplitude, millis) in [(0.72, 0.28, 1300), (0.4, 0.6, 900)] {
                for ancestor in [0.0, 0.37, 1.0] {
                    let animation = OpacityAnimation { period: Duration::from_millis(millis), baseline, amplitude, waveform: OpacityWaveform::HalfSinePi };
                    let original = MonochromeSprite {
                        order: 0, pad: 0, bounds: Bounds::new(point(ScaledPixels(8.0), ScaledPixels(8.0)), size(ScaledPixels(48.0), ScaledPixels(48.0))),
                        content_mask: ContentMask { bounds: Bounds::new(point(ScaledPixels(10.0), ScaledPixels(7.0)), size(ScaledPixels(43.0), ScaledPixels(48.0))) },
                        color: crate::Hsla::from(rgb(0x8a9dbc)).opacity(ancestor), tile: tile.clone(), transformation: TransformationMatrix::unit(),
                    };
                    let mut scene = Scene::default();
                    scene.insert_sprite_properties(original.clone().into(), ResolvedProperties { opacity: Some((animation, started)), frames: None }, [[0.0; 2]; 8]);
                    scene.finish();
                    let mut distinct_frames = std::collections::HashSet::new();
                    for micros in [0u64, 100_000, 450_000, 650_000, 899_999, 900_000, 1_300_000, 2_600_001] {
                        let now = started + Duration::from_micros(micros);
                        let mut legacy = original.clone();
                        legacy.color.a *= animation.value(progress(now, started, animation.period));
                        let mut expected_scene = Scene::default();
                        expected_scene.insert_primitive(legacy);
                        expected_scene.finish();
                        let expected = normalized_components(&encode_scene_pixels(&renderer, &expected_scene, None), wide);
                        assert_eq!(expected.iter().any(|component| *component > 1.0 / 255.0), ancestor > 0.0,
                            "Monochrome legacy oracle must paint meaningful pixels precisely when inherited alpha is positive");
                        scene.sample_presentation_effects(now);
                        let actual_bytes = encode_scene_pixels(&renderer, &scene, None);
                        distinct_frames.insert(actual_bytes.clone());
                        let actual = normalized_components(&actual_bytes, wide);
                        let error = expected.iter().zip(actual).map(|(left, right)| (left-right).abs()).fold(0.0f32, f32::max);
                        max_error = max_error.max(error);
                        assert!(error <= 1.0/255.0, "monochrome retained wave error={error}");
                        compared_frames += 1;
                    }
                    assert_eq!(distinct_frames.len() > 1, ancestor > 0.0,
                        "Monochrome GPU output must vary across opacity phases and park at zero inherited alpha");
                }
            }
        }
        eprintln!("KAI_RETAINED_MONO_GPU compared_frames={compared_frames} max_component_error={max_error} tolerance={}", 1.0f32/255.0);
    }

    #[test]
    fn retained_tile_shimmer_preserves_quantized_alpha_within_float_precision() {
        use crate::{QuadShimmer, QuadShimmerTile, QuadShimmerTiming, rgba, solid_background};
        let context = Context::default();
        let mut compared_frames = 0usize;
        let mut max_error = [0.0f32; 2];
        for wide in [false, true] {
            let renderer = MetalRenderer::new(context.clone(), wide);
            // Derive representable output levels from the actual legacy pipeline.
            // This includes blend/storage rounding without assuming one HALF round.
            let mut levels = Scene::default();
            for code in 0..=210u32 {
                levels.insert_primitive(Quad {
                    bounds: Bounds::new(point(ScaledPixels(8.0 + (code % 32) as f32), ScaledPixels(8.0 + (code / 32) as f32)), size(ScaledPixels(1.0), ScaledPixels(1.0))),
                    content_mask: ContentMask { bounds: Bounds::new(point(ScaledPixels(0.0), ScaledPixels(0.0)), size(ScaledPixels(64.0), ScaledPixels(64.0))) },
                    background: solid_background(crate::Hsla::from(rgba(0xffffff00 | code))),
                    ..Default::default()
                });
            }
            levels.finish();
            let level_bytes = encode_scene_pixels(&renderer, &levels, None);
            let bytes_per_pixel = if wide { 8 } else { 4 };
            let alpha_range = if wide { 6..8 } else { 3..4 };
            let quantized_levels: std::collections::HashSet<Vec<u8>> = (0..=210usize).map(|code| {
                let pixel = (8 + code / 32) * 64 + 8 + code % 32;
                level_bytes[pixel * bytes_per_pixel + alpha_range.start..pixel * bytes_per_pixel + alpha_range.end].to_vec()
            }).collect();
            assert_eq!(quantized_levels.len(), 211, "Legacy quantized source levels must remain distinct");
            let started = Instant::now();
            let clock = QuadShimmer { started, timing: QuadShimmerTiming::ElapsedSeconds };
            for feather in [0.2f32, 0.65, 1.0] {
                for filled in [false, true] {
                    let params = QuadShimmerTile { delay: 0.42, filled, feather };
                    let original = Quad {
                        bounds: Bounds::new(point(ScaledPixels(8.0), ScaledPixels(8.0)), size(ScaledPixels(48.0), ScaledPixels(48.0))),
                        content_mask: ContentMask { bounds: Bounds::new(point(ScaledPixels(0.0), ScaledPixels(0.0)), size(ScaledPixels(64.0), ScaledPixels(64.0))) },
                        background: solid_background(crate::Hsla::from(rgba(0xffffffff))).with_shimmer_tile(params),
                        ..Default::default()
                    };
                    let mut candidate = Scene::default();
                    candidate.insert_quad_shimmer(original.clone(), clock);
                    candidate.finish();
                    for micros in [0u64, 420_000, 420_001, 600_000, 783_999, 784_000, 784_001, 1_147_999, 1_148_000, 1_148_001, 1_820_000, 4_700_000] {
                        let elapsed = Duration::from_micros(micros).as_secs_f32();
                        let alpha = params.alpha_at(elapsed);
                        let mut legacy = Scene::default();
                        if alpha > 0 {
                            let mut quad = original.clone();
                            quad.background = solid_background(crate::Hsla::from(rgba(0xffffff00 | alpha)));
                            legacy.insert_primitive(quad);
                        }
                        legacy.finish();
                        candidate.sample_presentation_effects(started + Duration::from_micros(micros));
                        let expected = encode_scene_pixels(&renderer, &legacy, None);
                        let actual = encode_scene_pixels(&renderer, &candidate, None);
                        let bytes_per_pixel = if wide { 8 } else { 4 };
                        let alpha_range = if wide { 6..8 } else { 3..4 };
                        for (actual, expected) in actual.chunks_exact(bytes_per_pixel).zip(expected.chunks_exact(bytes_per_pixel)) {
                            assert!(quantized_levels.contains(&actual[alpha_range.clone()]),
                                "Tile alpha must belong to the legacy byte-quantized output levels");
                            let actual = normalized_components(&actual[alpha_range.clone()], wide)[0];
                            let expected = normalized_components(&expected[alpha_range.clone()], wide)[0];
                            let error = (actual - expected).abs();
                            max_error[usize::from(wide)] = max_error[usize::from(wide)].max(error);
                            assert!(actual.is_finite() && error <= 1.0 / 255.0,
                                "Tile alpha boundary error={error} at t={elapsed}, feather={feather}, filled={filled}, wide={wide}");
                        }
                        compared_frames += 1;
                    }
                }
            }
        }
        eprintln!("KAI_RETAINED_SHIMMER_QUANTIZATION compared_frames={compared_frames} quantized_alpha_grid=true pixel_aligned=true max_bgra8_error={} max_rgba16_error={} tolerance={}", max_error[0], max_error[1], 1.0f32 / 255.0);
    }

    #[test]
    fn retained_tile_shimmer_matches_legacy_quantized_metal_geometry_and_alpha() {
        use crate::{QuadShimmer, QuadShimmerTile, QuadShimmerTiming, rgb, rgba, solid_background};
        let context = Context::default();
        let mut compared_frames = 0usize;
        let mut max_error = [0.0f32; 2];
        for wide in [false, true] {
            let renderer = MetalRenderer::new(context.clone(), wide);
            for scale in [1.0f32, 1.5, 2.0] {
                for ancestor in [0.0f32, 0.37, 1.0] {
                    let started = Instant::now();
                    let clock = QuadShimmer { started, timing: QuadShimmerTiming::ElapsedSeconds };
                    let mut candidate = Scene::default();
                    let mut originals = Vec::new();
                    let width = 46.125f32 / 32.0;
                    let height = 20.375f32 / 5.0;
                    for n in 0..160 {
                        let col = n % 32;
                        let row = n / 32;
                        let distance = (((col as f32 - 15.5) / 15.5).powi(2) + ((row as f32 - 2.0) / 2.0).powi(2)).sqrt();
                        let params = QuadShimmerTile {
                            delay: distance * 0.42,
                            filled: (n * 73 + n * n * 19 + 23) % 101 < 65,
                            feather: ((col as f32 + 0.5) / 4.0).min((31.5-col as f32)/4.0).min(1.0)
                                * if row == 0 || row == 4 { 0.2 } else if row == 1 || row == 3 { 0.65 } else { 1.0 },
                        };
                        let quad = Quad {
                            bounds: Bounds::new(point(ScaledPixels((7.375 + width * col as f32) * scale), ScaledPixels((9.25 + height * row as f32) * scale)), size(ScaledPixels((width-0.7).max(0.0) * scale), ScaledPixels((height-0.7).max(0.0) * scale))),
                            content_mask: ContentMask { bounds: Bounds::new(point(ScaledPixels(8.125 * scale), ScaledPixels(10.0 * scale)), size(ScaledPixels(43.25 * scale), ScaledPixels(18.5 * scale))) },
                            background: solid_background(crate::Hsla::from(rgb(0x9ac3df)).opacity(ancestor)).with_shimmer_tile(params),
                            ..Default::default()
                        };
                        originals.push((quad.clone(), params));
                        candidate.insert_quad_shimmer(quad, clock);
                    }
                    candidate.finish();
                    let mut distinct = std::collections::HashSet::new();
                    for micros in [0u64, 1, 16_667, 200_000, 363_999, 364_000, 364_001, 727_999, 728_000, 728_001, 1_399_999, 1_400_000, 1_400_001, 2_800_000, 4_599_999, 4_600_000, 4_900_000, 4_999_999, 5_000_000, 5_000_001] {
                        let now = started + Duration::from_micros(micros);
                        let elapsed = Duration::from_micros(micros).as_secs_f32();
                        let mut legacy = Scene::default();
                        for (base, params) in &originals {
                            let alpha = params.alpha_at(elapsed);
                            if alpha == 0 { continue; }
                            let mut quad = base.clone();
                            quad.background = solid_background(crate::Hsla::from(rgba(0x9ac3df00 | alpha)).opacity(ancestor));
                            legacy.insert_primitive(quad);
                        }
                        legacy.finish();
                        let expected = normalized_components(&encode_scene_pixels(&renderer, &legacy, None), wide);
                        let expected_nonzero = expected.iter().any(|component| *component > 1.0 / 255.0);
                        if ancestor > 0.0 && elapsed < 4.9 { assert!(expected_nonzero, "Legacy shimmer oracle must paint meaningful nonzero tiles"); }
                        if ancestor == 0.0 || elapsed >= 5.0 { assert!(!expected_nonzero); }
                        candidate.sample_presentation_effects(now);
                        let actual_bytes = encode_scene_pixels(&renderer, &candidate, None);
                        distinct.insert(actual_bytes.clone());
                        let actual = normalized_components(&actual_bytes, wide);
                        let error = expected.iter().zip(actual).map(|(left, right)| (left-right).abs()).fold(0.0f32, f32::max);
                        max_error[usize::from(wide)] = max_error[usize::from(wide)].max(error);
                        assert!(error <= 1.0/255.0, "Tile shimmer error={error} elapsed={elapsed} scale={scale} ancestor={ancestor} wide={wide}");
                        compared_frames += 1;
                    }
                    assert_eq!(distinct.len() > 1, ancestor > 0.0, "Shimmer output must change with phase and finite fade");
                }
            }
        }
        eprintln!("KAI_RETAINED_SHIMMER_GPU compared_frames={compared_frames} tiles_per_group=160 max_bgra8_error={} max_rgba16_error={} tolerance={} fixed_legacy_geometry=true quantized_u8_alpha=true", max_error[0], max_error[1], 1.0f32/255.0);
    }

    #[test]
    fn retained_image_opacity_matches_legacy_metal_pixels_and_encoded_atlas_lifetime() {
        let started = Instant::now();
        let context = Context::default();
        let mut compared_bytes = 0;
        for transparent in [false, true] {
            let renderer = MetalRenderer::new(context.clone(), transparent);
            let key: AtlasKey = RenderImageParams {
                image_id: ImageId(9158),
                frame_index: 0,
            }
            .into();
            let pixels: Vec<u8> = (0..64)
                .flat_map(|index| [40 + index as u8, 90, 180, 64 + (index * 3) as u8])
                .collect();
            let tile = renderer
                .sprite_atlas
                .get_or_insert_with(&key, &mut || {
                    Ok(Some((
                        size(DevicePixels(8), DevicePixels(8)),
                        Cow::Borrowed(pixels.as_slice()),
                    )))
                })
                .unwrap()
                .unwrap();
            let recipe = ImageOpacityPulse {
                started,
                duration: Duration::from_secs(3),
                baseline: 0.88,
                amplitude: 0.12,
            };
            for base in [0.0, 0.37, 1.0] {
                for grayscale in [false, true] {
                    let original = PolychromeSprite {
                        order: 0,
                        opacity_pulse_index: 0,
                        grayscale,
                        opacity: base,
                        bounds: Bounds::new(
                            point(ScaledPixels(8.), ScaledPixels(8.)),
                            size(ScaledPixels(48.), ScaledPixels(48.)),
                        ),
                        content_mask: ContentMask {
                            bounds: Bounds::new(
                                point(ScaledPixels(12.), ScaledPixels(8.)),
                                size(ScaledPixels(40.), ScaledPixels(48.)),
                            ),
                        },
                        corner_radii: Corners::all(ScaledPixels(10.)),
                        tile: tile.clone(),
                    };
                    let mut scene = Scene::default();
                    scene.insert_image_opacity_pulse(original.clone(), recipe);
                    scene.finish();
                    let mut distinct = std::collections::HashSet::new();
                    for micros in [
                        0, 375_000, 750_000, 1_500_000, 2_625_000, 3_000_000, 3_000_001, 6_000_000,
                    ] {
                        let now = started + Duration::from_micros(micros);
                        let mut progress = now.duration_since(started).as_secs_f32() / 3.0;
                        if progress > 1.0 {
                            progress %= 1.0;
                        }
                        let mut legacy = original.clone();
                        legacy.opacity =
                            base * (0.88 + 0.12 * (std::f32::consts::TAU * progress).cos());
                        let expected = encode_pixels(&renderer, &[legacy], None);
                        scene.sample_opacity_pulses(now);
                        let actual = encode_pixels(&renderer, &scene.polychrome_sprites, None);
                        assert_eq!(
                            actual, expected,
                            "transparent={transparent} parent={base} gray={grayscale} elapsed={micros}"
                        );
                        let stride = if transparent { 8 } else { 4 };
                        assert!(
                            actual[(10 * 64 + 8) * stride..(10 * 64 + 9) * stride]
                                .iter()
                                .all(|byte| *byte == 0),
                            "Original clip must remain effective"
                        );
                        assert!(
                            actual[..stride].iter().all(|byte| *byte == 0),
                            "Outside the image stays clear"
                        );
                        assert!(
                            actual[(8 * 64 + 12) * stride..(8 * 64 + 13) * stride]
                                .iter()
                                .all(|byte| *byte == 0),
                            "The rounded corner inside the clip stays clear"
                        );
                        distinct.insert(actual.clone());
                        compared_bytes += actual.len();
                    }
                    assert_eq!(
                        distinct.len() > 1,
                        base > 0.0,
                        "Real GPU output must pulse continuously, including parent opacity zero"
                    );
                }
            }
            let static_sprite = PolychromeSprite {
                order: 0,
                opacity_pulse_index: 0,
                grayscale: false,
                opacity: 1.,
                bounds: Bounds::new(
                    point(ScaledPixels(8.), ScaledPixels(8.)),
                    size(ScaledPixels(48.), ScaledPixels(48.)),
                ),
                content_mask: ContentMask {
                    bounds: Bounds::new(
                        point(ScaledPixels(0.), ScaledPixels(0.)),
                        size(ScaledPixels(64.), ScaledPixels(64.)),
                    ),
                },
                corner_radii: Corners::all(ScaledPixels(10.)),
                tile,
            };
            let expected = encode_pixels(&renderer, &[static_sprite.clone()], None);
            let actual = encode_pixels(&renderer, &[static_sprite], Some(&key));
            assert_eq!(
                actual, expected,
                "Encoded image texture survives atlas key retirement before GPU commit"
            );
        }
        eprintln!(
            "KAI_RETAINED_IMAGE_OPACITY_GPU compared_frames=96 compared_bytes={compared_bytes} exact_pixels=true encoded_atlas_retirement=true"
        );
    }



    fn legacy_dither_pixels(source: &[u8], width: usize, step: usize) -> Vec<u8> {
        let mut pixels = source.to_vec();
        let progress = step as f32 / 14.0;
        for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
            let x = (index % width) / 2;
            let y = (index / width) / 2;
            let mut hash = (x as u32).wrapping_mul(0x9e3779b9)
                ^ (y as u32).wrapping_mul(0x85ebca6b)
                ^ 0x71ea1439;
            hash ^= hash >> 16;
            hash = hash.wrapping_mul(0x7feb352d);
            hash ^= hash >> 15;
            hash = hash.wrapping_mul(0x846ca68b);
            hash ^= hash >> 16;
            if (hash >> 8) as f32 / 16_777_216.0 >= progress {
                pixel[3] = 0;
            }
        }
        pixels
    }

    fn install_dither_test_tile(
        renderer: &MetalRenderer,
        id: u32,
        width: i32,
        height: i32,
        pixels: &[u8],
    ) -> (AtlasKey, crate::AtlasTile) {
        let key: AtlasKey = RenderImageParams {
            image_id: ImageId(id as usize),
            frame_index: 0,
        }
        .into();
        let tile = renderer
            .sprite_atlas
            .get_or_insert_with(&key, &mut || {
                Ok(Some((
                    size(DevicePixels(width), DevicePixels(height)),
                    Cow::Borrowed(pixels),
                )))
            })
            .unwrap()
            .unwrap();
        (key, tile)
    }

    fn dither_half_value(bytes: &[u8]) -> f32 {
        let bits = u16::from_le_bytes([bytes[0], bytes[1]]);
        let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
        let exponent = ((bits >> 10) & 0x1f) as i32;
        let fraction = (bits & 0x03ff) as f32;
        match exponent {
            0 => sign * fraction / 16_777_216.0,
            31 if fraction == 0.0 => sign * f32::INFINITY,
            31 => f32::NAN,
            _ => sign * (1.0 + fraction / 1024.0) * 2.0f32.powi(exponent - 15),
        }
    }

    #[test]
    fn continuous_dither_gpu_advances_every_60hz_frame_with_exact_cell_membership() {
        let mut compared_frames = 0usize;
        let mut compared_bytes = 0usize;
        for wide in [false, true] {
            let control = MetalRenderer::new(Context::default(), wide);
            let candidate = MetalRenderer::new(Context::default(), wide);
            let width = 128usize;
            let pixels: Vec<u8> = (0..width * width).flat_map(|index| {
                let cell = (index % width) / 2 + (index / width) / 2 * 64;
                [32 + (cell * 7 % 200) as u8, 32 + (cell * 11 % 200) as u8, 32 + (cell * 13 % 200) as u8, 255]
            }).collect();
            let neighbor = vec![180u8; 8 * 8 * 4];
            install_dither_test_tile(&control, 9630, 8, 8, &neighbor);
            install_dither_test_tile(&candidate, 9630, 8, 8, &neighbor);
            let (_, control_tile) = install_dither_test_tile(&control, 9631, width as i32, width as i32, &pixels);
            let (_, candidate_tile) = install_dither_test_tile(&candidate, 9631, width as i32, width as i32, &pixels);
            assert_eq!(control_tile.bounds, candidate_tile.bounds);
            let original = PolychromeSprite {
                order: 0, opacity_pulse_index: 0, grayscale: false, opacity: 1.0,
                bounds: Bounds::new(point(ScaledPixels(0.0), ScaledPixels(0.0)), size(ScaledPixels(64.0), ScaledPixels(64.0))),
                content_mask: ContentMask { bounds: Bounds::new(point(ScaledPixels(0.0), ScaledPixels(0.0)), size(ScaledPixels(64.0), ScaledPixels(64.0))) },
                corner_radii: Corners::all(ScaledPixels(0.0)), tile: candidate_tile,
            };
            let started = Instant::now();
            let mut scene = Scene::default();
            scene.insert_image_dither_reveal(original.clone(), crate::ImageDitherReveal::new(started, Duration::from_millis(600)));
            scene.finish();
            scene.sample_presentation_effects(started);
            scene.did_submit_presentation_effects(started);
            let mut outputs = std::collections::HashSet::new();
            for frame in 0..=36u64 {
                let elapsed = Duration::from_nanos(frame * 600_000_000 / 36);
                let progress = (elapsed.as_secs_f32() / Duration::from_millis(600).as_secs_f32()).clamp(0.0, 1.0);
                let cutoff = (progress * 16_777_216.0).ceil() as u32;
                let mut masked = pixels.clone();
                let mut visible = 0usize;
                for (index, pixel) in masked.chunks_exact_mut(4).enumerate() {
                    let x = ((index % width) / 2) as u32;
                    let y = ((index / width) / 2) as u32;
                    let mut hash = x.wrapping_mul(0x9e3779b9) ^ y.wrapping_mul(0x85ebca6b) ^ 0x71ea1439;
                    hash ^= hash >> 16;
                    hash = hash.wrapping_mul(0x7feb352d);
                    hash ^= hash >> 15;
                    hash = hash.wrapping_mul(0x846ca68b);
                    hash ^= hash >> 16;
                    if hash >> 8 >= cutoff { pixel[3] = 0; } else { visible += 1; }
                }
                if frame > 0 { assert!(visible > 0); }
                if frame < 36 { assert!(visible < width * width); }
                let texture = control.sprite_atlas.metal_texture(control_tile.texture_id);
                texture.replace_region(metal::MTLRegion::new_2d(control_tile.bounds.origin.x.0 as u64,
                    control_tile.bounds.origin.y.0 as u64, width as u64, width as u64), 0,
                    masked.as_ptr().cast(), (width * 4) as u64);
                let mut legacy = original.clone();
                legacy.tile = control_tile.clone();
                let expected = encode_pixels(&control, &[legacy], None);
                scene.sample_presentation_effects(started + elapsed);
                assert_eq!(scene.polychrome_sprites[0].opacity_pulse_index & 0x7fffffff, cutoff);
                let actual = encode_scene_pixels(&candidate, &scene, None);
                assert_eq!(actual, expected, "Continuous source-cell membership changed at60Hz frame{frame}, wide={wide}");
                compared_bytes += actual.len();
                outputs.insert(actual);
                compared_frames += 1;
            }
            assert_eq!(outputs.len(), 37, "Every600ms/60Hz presentation must reveal a distinct grain state on the dense source fixture");
        }
        eprintln!("KAI_CONTINUOUS_DITHER_GPU compared_frames={compared_frames} compared_bytes={compared_bytes} distinct_frames_per_format=37 exact_source_cells=true duration_ms=600 refresh_hz=60");
    }

    #[test]
    fn finite_gpu_dither_matches_legacy_within_filter_precision() {
        let started = Instant::now();
        let mut compared_frames = 0;
        let mut compared_bytes = 0;
        let mut max_bgra8_byte_error = 0u8;
        let mut max_rgba16_component_error = 0.0f32;
        let mut differing_components = [0usize; 2];
        // Integer source-texel membership has a separate bit-exact regression.
        // Fractional sampling retains a one-UNorm8-step color error ceiling
        // because software alpha interpolation and hardware filtering round
        // differently, including when the render target is RGBA16Float.
        const FILTER_COMPONENT_TOLERANCE: f32 = 1.0 / 255.0;
        for transparent in [false, true] {
            let control = MetalRenderer::new(Context::default(), transparent);
            let candidate = MetalRenderer::new(Context::default(), transparent);
            // Identical opaque neighbors exercise packed atlas edges. The
            // dither hash must use source texels, including a nonzero origin.
            let neighbor = vec![200u8; 8 * 8 * 4];
            install_dither_test_tile(&control, 9500, 8, 8, &neighbor);
            install_dither_test_tile(&candidate, 9500, 8, 8, &neighbor);
            for (case, (width, height)) in [(8, 8), (9, 7), (31, 19)].into_iter().enumerate() {
                let pixels: Vec<u8> = (0..width * height)
                    .flat_map(|index| {
                        [
                            (40 + index * 7 % 170) as u8,
                            (35 + index * 11 % 180) as u8,
                            (60 + index * 13 % 160) as u8,
                            (64 + index * 3 % 185) as u8,
                        ]
                    })
                    .collect();
                let (control_key, control_tile) =
                    install_dither_test_tile(&control, 9501 + case as u32, width, height, &pixels);
                let (candidate_key, candidate_tile) = install_dither_test_tile(
                    &candidate,
                    9501 + case as u32,
                    width,
                    height,
                    &pixels,
                );
                assert_eq!(control_tile.bounds, candidate_tile.bounds);
                assert!(
                    candidate_tile.bounds.origin.x.0 > 0 || candidate_tile.bounds.origin.y.0 > 0
                );
                for fractional in [false, true] {
                    for base in [0.37, 1.0] {
                        for grayscale in [false, true] {
                            let bounds = Bounds::new(
                                point(
                                    ScaledPixels(if fractional { 7.25 } else { 8.0 }),
                                    ScaledPixels(if fractional { 8.75 } else { 8.0 }),
                                ),
                                size(
                                    ScaledPixels(if fractional { 46.5 } else { 48.0 }),
                                    ScaledPixels(if fractional { 43.25 } else { 48.0 }),
                                ),
                            );
                            let original = PolychromeSprite {
                                order: 0,
                                opacity_pulse_index: 0,
                                grayscale,
                                opacity: base,
                                bounds,
                                content_mask: ContentMask {
                                    bounds: Bounds::new(
                                        point(ScaledPixels(10.0), ScaledPixels(9.0)),
                                        size(ScaledPixels(43.0), ScaledPixels(44.0)),
                                    ),
                                },
                                corner_radii: Corners::all(ScaledPixels(5.0)),
                                tile: candidate_tile.clone(),
                            };
                            let mut scene = Scene::default();
                            scene.insert_image_dither_reveal(
                                original.clone(),
                                crate::ImageDitherReveal::new(started, Duration::from_secs(14)),
                            );
                            scene.finish();
                            // Establish the controlled submission epoch. Actual
                            // drawable deferral is covered by Window tests.
                            scene.sample_presentation_effects(started);
                            scene.did_submit_presentation_effects(started);
                            for step in 0..=14 {
                                let masked = legacy_dither_pixels(&pixels, width as usize, step);
                                let texture =
                                    control.sprite_atlas.metal_texture(control_tile.texture_id);
                                texture.replace_region(
                                    metal::MTLRegion::new_2d(
                                        control_tile.bounds.origin.x.0 as u64,
                                        control_tile.bounds.origin.y.0 as u64,
                                        width as u64,
                                        height as u64,
                                    ),
                                    0,
                                    masked.as_ptr().cast(),
                                    width as u64 * 4,
                                );
                                let mut legacy = original.clone();
                                legacy.tile = control_tile.clone();
                                let expected = encode_pixels(&control, &[legacy], None);
                                scene.sample_presentation_effects(
                                    started + Duration::from_millis(step as u64 * 1000 + 1),
                                );
                                let actual =
                                    encode_pixels(&candidate, &scene.polychrome_sprites, None);
                                assert_eq!(actual.len(), expected.len());
                                if transparent {
                                    for (actual, expected) in
                                        actual.chunks_exact(2).zip(expected.chunks_exact(2))
                                    {
                                        if actual != expected {
                                            differing_components[1] += 1;
                                        }
                                        let error = (dither_half_value(actual)
                                            - dither_half_value(expected))
                                        .abs();
                                        assert!(error.is_finite());
                                        max_rgba16_component_error =
                                            max_rgba16_component_error.max(error);
                                    }
                                } else {
                                    for (&actual, &expected) in actual.iter().zip(&expected) {
                                        let error = actual.abs_diff(expected);
                                        differing_components[0] += usize::from(error != 0);
                                        max_bgra8_byte_error = max_bgra8_byte_error.max(error);
                                    }
                                }
                                compared_frames += 1;
                                compared_bytes += actual.len();
                            }
                        }
                    }
                }
                control.sprite_atlas.remove(&control_key);
                candidate.sprite_atlas.remove(&candidate_key);
            }
        }
        eprintln!("KAI_FINITE_DITHER_GPU compared_frames={compared_frames} compared_bytes={compared_bytes} exact_pixels=false max_bgra8_byte_error={max_bgra8_byte_error} max_rgba16_component_error={max_rgba16_component_error} component_tolerance={FILTER_COMPONENT_TOLERANCE} differing_bgra8_components={} differing_rgba16_components={} legacy_alpha_mask_before_filter=true packed_atlas=true fractional_bounds=true", differing_components[0], differing_components[1]);
        assert!(
            max_bgra8_byte_error <= 1,
            "Fractional BGRA8 dither filtering exceeds one byte"
        );
        assert!(
            max_rgba16_component_error <= FILTER_COMPONENT_TOLERANCE,
            "Fractional RGBA16 dither filtering exceeds one normalized UNorm8 step"
        );
    }
    #[test]
    fn finite_gpu_dither_source_texel_membership_is_exact_at_unit_scale() {
        let started = Instant::now();
        let mut compared_frames = 0;
        let mut compared_bytes = 0;
        for transparent in [false, true] {
            let control = MetalRenderer::new(Context::default(), transparent);
            let candidate = MetalRenderer::new(Context::default(), transparent);
            // Identical opaque neighbors exercise packed atlas edges. The
            // dither hash must use source texels, including a nonzero origin.
            let neighbor = vec![200u8; 8 * 8 * 4];
            install_dither_test_tile(&control, 9500, 8, 8, &neighbor);
            install_dither_test_tile(&candidate, 9500, 8, 8, &neighbor);
            for (case, (width, height)) in [(8, 8), (9, 7), (31, 19)].into_iter().enumerate() {
                let pixels: Vec<u8> = (0..width * height)
                    .flat_map(|index| {
                        [
                            (40 + index * 7 % 170) as u8,
                            (35 + index * 11 % 180) as u8,
                            (60 + index * 13 % 160) as u8,
                            (64 + index * 3 % 185) as u8,
                        ]
                    })
                    .collect();
                let (control_key, control_tile) =
                    install_dither_test_tile(&control, 9501 + case as u32, width, height, &pixels);
                let (candidate_key, candidate_tile) = install_dither_test_tile(
                    &candidate,
                    9501 + case as u32,
                    width,
                    height,
                    &pixels,
                );
                assert_eq!(control_tile.bounds, candidate_tile.bounds);
                assert!(
                    candidate_tile.bounds.origin.x.0 > 0 || candidate_tile.bounds.origin.y.0 > 0
                );
                for fractional in [false] {
                    for base in [1.0] {
                        for grayscale in [false] {
                            let bounds = Bounds::new(
                                point(
                                    ScaledPixels(if fractional { 7.25 } else { 8.0 }),
                                    ScaledPixels(if fractional { 8.75 } else { 8.0 }),
                                ),
                                size(ScaledPixels(width as f32), ScaledPixels(height as f32)),
                            );
                            let original = PolychromeSprite {
                                order: 0,
                                opacity_pulse_index: 0,
                                grayscale,
                                opacity: base,
                                bounds,
                                content_mask: ContentMask {
                                    bounds: Bounds::new(
                                        point(ScaledPixels(10.0), ScaledPixels(9.0)),
                                        size(ScaledPixels(43.0), ScaledPixels(44.0)),
                                    ),
                                },
                                corner_radii: Corners::all(ScaledPixels(0.0)),
                                tile: candidate_tile.clone(),
                            };
                            let mut scene = Scene::default();
                            scene.insert_image_dither_reveal(
                                original.clone(),
                                crate::ImageDitherReveal::new(started, Duration::from_secs(14)),
                            );
                            scene.finish();
                            // Establish the controlled submission epoch. Actual
                            // drawable deferral is covered by Window tests.
                            scene.sample_presentation_effects(started);
                            scene.did_submit_presentation_effects(started);
                            for step in 0..=14 {
                                let masked = legacy_dither_pixels(&pixels, width as usize, step);
                                let texture =
                                    control.sprite_atlas.metal_texture(control_tile.texture_id);
                                texture.replace_region(
                                    metal::MTLRegion::new_2d(
                                        control_tile.bounds.origin.x.0 as u64,
                                        control_tile.bounds.origin.y.0 as u64,
                                        width as u64,
                                        height as u64,
                                    ),
                                    0,
                                    masked.as_ptr().cast(),
                                    width as u64 * 4,
                                );
                                let mut legacy = original.clone();
                                legacy.tile = control_tile.clone();
                                let expected = encode_pixels(&control, &[legacy], None);
                                scene.sample_presentation_effects(
                                    started + Duration::from_millis(step as u64 * 1000 + 1),
                                );
                                let actual =
                                    encode_pixels(&candidate, &scene.polychrome_sprites, None);
                                let first_difference =
                                    actual.iter().zip(&expected).position(|(a, b)| a != b);
                                assert!(
                                    first_difference.is_none(),
                                    "GPU dither differs from pre-masked legacy texture: transparent={transparent} dimensions={width}x{height} fractional={fractional} base={base} grayscale={grayscale} step={step} first_byte={first_difference:?} actual={:?} expected={:?}",
                                    first_difference.map(|i| actual[i]),
                                    first_difference.map(|i| expected[i])
                                );
                                compared_frames += 1;
                                compared_bytes += actual.len();
                            }
                        }
                    }
                }
                control.sprite_atlas.remove(&control_key);
                candidate.sprite_atlas.remove(&candidate_key);
            }
        }
        eprintln!(
            "KAI_DITHER_UNIT_SCALE_MEMBERSHIP compared_frames={compared_frames} compared_bytes={compared_bytes} exact_pixels=true legacy_alpha_mask_before_filter=true packed_atlas=true unit_scale=true pixel_aligned=true"
        );
    }
}

#[cfg(test)]
mod instance_buffer_cache_tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn completed_buffer_cache_is_bounded_without_limiting_in_flight_buffers() {
        let device = metal::Device::system_default().expect("Metal device");
        let mut pool = InstanceBufferPool::default();
        let outstanding = (0..12).map(|_| pool.acquire(&device)).collect::<Vec<_>>();
        assert!(pool.buffers.is_empty());
        assert_eq!(outstanding.len(), 12);
        for buffer in outstanding {
            pool.release(buffer);
        }
        assert_eq!(pool.buffers.len(), 8);
        assert_eq!(pool.buffers.iter().map(|buffer| buffer.length()).sum::<u64>(), 16 * 1024 * 1024);

        let previous_size = pool.acquire(&device);
        pool.reset(32 * 1024 * 1024);
        pool.release(previous_size);
        assert!(pool.buffers.is_empty());
        let first = pool.acquire(&device);
        let second = pool.acquire(&device);
        pool.release(first);
        pool.release(second);
        assert_eq!(pool.buffers.len(), 1);
        let reused = pool.acquire(&device);
        assert_eq!(reused.size, 32 * 1024 * 1024);
        assert_eq!(reused.metal_buffer.length(), 32 * 1024 * 1024);
        pool.release(reused);
        assert_eq!(pool.buffers.len(), 1);
    }
}
