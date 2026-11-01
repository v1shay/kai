//! Bounded off-thread image imports. Submitted copies outlive moved originals.
use anyhow::{Context, Result, bail, ensure};
use gpui::RenderImage;
use image::{ImageEncoder, RgbaImage};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

pub const MAX_ATTACHMENTS: usize = 4;
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
// Screenshot shortcuts commonly publish uncompressed Retina TIFF data. This
// bounds only the source clipboard bytes; normalized storage keeps file quotas.
const MAX_CLIPBOARD_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_SIDE: u32 = 2048;
const THUMBNAIL_SIDE: u32 = 160;
const CACHE_BYTES: u64 = 512 * 1024 * 1024;
const CACHE_ENTRIES: usize = 4096;

/// Explicit composer paste only. The native reader checks encoded length before
/// copying bytes, and prefers raster representations over a companion URL/text.
/// It never decodes an image or reads files on the UI thread.
pub fn clipboard_image(_cx: &mut gpui::App) -> Result<Option<Arc<[u8]>>, String> {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        native_clipboard_image()
    }
    #[cfg(any(not(target_os = "macos"), test))]
    {
        let Some(item) = _cx.read_from_clipboard() else {
            return Ok(None);
        };
        for entry in item.into_entries() {
            if let gpui::ClipboardEntry::Image(image) = entry {
                validate_clipboard_image(&image.bytes)?;
                return Ok(Some(image.bytes.into()));
            }
        }
        Ok(None)
    }
}

fn validate_clipboard_image(bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_CLIPBOARD_BYTES {
        return Err("Clipboard captures must be 64 MB or smaller before compression.".into());
    }
    if !supported_content(bytes) {
        return Err("Paste a PNG, JPEG, WebP, GIF, HEIC, TIFF or BMP image.".into());
    }
    Ok(())
}

#[cfg(all(target_os = "macos", not(test)))]
fn native_clipboard_image() -> Result<Option<Arc<[u8]>>, String> {
    use objc2::{
        msg_send,
        rc::autoreleasepool,
        runtime::{AnyClass, AnyObject},
    };
    autoreleasepool(|_| unsafe {
        let class = AnyClass::get(c"NSPasteboard").ok_or("macOS clipboard is unavailable.")?;
        let pasteboard: *mut AnyObject = msg_send![class, generalPasteboard];
        if pasteboard.is_null() {
            return Ok(None);
        }
        native_clipboard_image_from(&*pasteboard)
    })
}

#[cfg(target_os = "macos")]
fn native_clipboard_image_from(
    pasteboard: &objc2::runtime::AnyObject,
) -> Result<Option<Arc<[u8]>>, String> {
    use objc2::{
        msg_send,
        runtime::{AnyObject, Bool},
    };
    use objc2_foundation::NSString;
    unsafe {
        let types: *mut AnyObject = msg_send![pasteboard, types];
        if types.is_null() {
            return Ok(None);
        }
        for name in [
            "public.png",
            "public.jpeg",
            "org.webmproject.webp",
            "com.compuserve.gif",
            "public.heic",
            "public.tiff",
            "com.microsoft.bmp",
            "com.microsoft.windows-bitmap",
        ] {
            let name = NSString::from_str(name);
            let present: Bool = msg_send![types, containsObject: &*name];
            if !present.as_bool() {
                continue;
            }
            let data: *mut AnyObject = msg_send![pasteboard, dataForType: &*name];
            if data.is_null() {
                continue;
            }
            let length: usize = msg_send![data, length];
            if length == 0 || length as u64 > MAX_CLIPBOARD_BYTES {
                return Err(
                    "Clipboard captures must be 64 MB or smaller before compression.".into(),
                );
            }
            let bytes: *const std::ffi::c_void = msg_send![data, bytes];
            if bytes.is_null() {
                return Err("The clipboard image is unavailable.".into());
            }
            let bytes = std::slice::from_raw_parts(bytes.cast::<u8>(), length);
            validate_clipboard_image(bytes)?;
            return Ok(Some(Arc::from(bytes)));
        }
        Ok(None)
    }
}

#[derive(Clone)]
pub struct Attachment {
    pub id: uuid::Uuid,
    pub path: PathBuf,
    pub thumbnail: Arc<RenderImage>,
    pub name: String,
    pub size: u64,
    file: Arc<ManagedFile>,
}
impl Attachment {
    /// Call only after successful native launch. Preserve references in history.
    pub fn mark_submitted(&self) {
        self.file.submitted.store(true, Ordering::Release);
        if let Some(permit) = &self.file.permit {
            permit.release();
        }
    }
}
struct ManagedFile {
    path: PathBuf,
    submitted: AtomicBool,
    permit: Option<CleanupPermit>,
}
impl Drop for ManagedFile {
    fn drop(&mut self) {
        if !self.submitted.load(Ordering::Acquire)
            && let Some(permit) = self.permit.take()
        {
            let cleanup = permit.cleanup.clone();
            // Every imported transient owns one reservation. Thus this
            // queue always has room, without waiting or touching disk here.
            let result = cleanup.tx.try_send(CleanupJob {
                path: self.path.clone(),
                permit,
            });
            debug_assert!(result.is_ok(), "Reserved cleanup queue must have capacity");
        }
    }
}
struct CleanupJob {
    path: PathBuf,
    permit: CleanupPermit,
}
struct Cleanup {
    tx: mpsc::SyncSender<CleanupJob>,
    slots: Mutex<usize>,
    ready: Condvar,
}
struct CleanupPermit {
    cleanup: Arc<Cleanup>,
    held: AtomicBool,
}
impl CleanupPermit {
    fn release(&self) {
        if self.held.swap(false, Ordering::AcqRel) {
            *self.cleanup.slots.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            self.cleanup.ready.notify_one();
        }
    }
}
impl Drop for CleanupPermit {
    fn drop(&mut self) {
        self.release();
    }
}
impl Cleanup {
    fn reserve(cancelled: &AtomicBool) -> Result<CleanupPermit> {
        static CLEANUP: OnceLock<Arc<Cleanup>> = OnceLock::new();
        let cleanup = CLEANUP
            .get_or_init(|| {
                let (tx, rx) = mpsc::sync_channel::<CleanupJob>(128);
                let state = Arc::new(Cleanup {
                    tx,
                    slots: Mutex::new(128),
                    ready: Condvar::new(),
                });
                std::thread::spawn(move || {
                    while let Ok(job) = rx.recv() {
                        let _ = fs::remove_file(&job.path);
                        drop(job.permit);
                    }
                });
                state
            })
            .clone();
        let mut slots = cleanup.slots.lock().unwrap_or_else(|e| e.into_inner());
        while *slots == 0 {
            check_cancelled(cancelled)?;
            slots = cleanup
                .ready
                .wait_timeout(slots, std::time::Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *slots -= 1;
        drop(slots);
        Ok(CleanupPermit {
            cleanup,
            held: AtomicBool::new(true),
        })
    }
}
static DECODE: OnceLock<Mutex<()>> = OnceLock::new();
struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Blocking worker operation. Serializes decodes so queued drags cannot multiply
/// raster memory. ImageIO requests a bounded thumbnail directly (including HEIC).
pub fn import(source: &Path, managed_dir: &Path, cancelled: &AtomicBool) -> Result<Attachment> {
    let _guard = DECODE
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    check_cancelled(cancelled)?;
    let file = File::open(source).context("The image is unavailable")?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "Drop an image file");
    ensure!(
        metadata.len() > 0 && metadata.len() <= MAX_FILE_BYTES,
        "Images must be 16 MB or smaller"
    );
    let bytes = read_bounded_image(file, metadata.len(), cancelled)?;
    let name = source.file_name().unwrap_or_default().to_string_lossy();
    import_encoded(&bytes, &name, MAX_FILE_BYTES, managed_dir, cancelled)
}

/// Clipboard bytes use the exact same normalization, private storage, quotas
/// and managed cleanup as dragged files. No temporary source file is created.
pub fn import_clipboard(
    bytes: &[u8],
    managed_dir: &Path,
    cancelled: &AtomicBool,
) -> Result<Attachment> {
    let _guard = DECODE
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    import_encoded(
        bytes,
        "Pasted image",
        MAX_CLIPBOARD_BYTES,
        managed_dir,
        cancelled,
    )
}

fn import_encoded(
    bytes: &[u8],
    name: &str,
    source_limit: u64,
    managed_dir: &Path,
    cancelled: &AtomicBool,
) -> Result<Attachment> {
    check_cancelled(cancelled)?;
    ensure!(
        !bytes.is_empty() && bytes.len() as u64 <= source_limit,
        "Image exceeds its source byte budget"
    );
    let permit = Cleanup::reserve(cancelled)?;
    ensure!(
        supported_content(bytes),
        "Use a PNG, JPEG, WebP, GIF, HEIC, TIFF or BMP image"
    );
    fs::create_dir_all(managed_dir)?;
    let mut total = 0_u64;
    for (index, entry) in fs::read_dir(managed_dir)?.enumerate() {
        ensure!(
            index < CACHE_ENTRIES,
            "Image storage is full. Remove older saved attachments to add more."
        );
        check_cancelled(cancelled)?;
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    ensure!(
        total <= CACHE_BYTES.saturating_sub(MAX_FILE_BYTES),
        "Image storage is full. Remove older saved attachments to add more."
    );
    let pixels = crate::background::attachment_thumbnail(bytes, MAX_SIDE)
        .or_else(|native_error| {
            // ImageIO cannot thumbnail every valid TIFF/BMP encoding. Prefer
            // its direct bounded thumbnail; a strict small-raster fallback
            // handles compatible clipboard encodings without an unbounded
            // full-resolution decode or a new subprocess.
            if !matches!(
                image::guess_format(bytes),
                Ok(image::ImageFormat::Tiff | image::ImageFormat::Bmp)
            ) {
                return Err(native_error);
            }
            small_clipboard_raster(bytes)
        })
        .context("This image could not be decoded")?;
    check_cancelled(cancelled)?;
    ensure!(
        pixels.width() > 0
            && pixels.height() > 0
            && pixels.width() <= MAX_SIDE
            && pixels.height() <= MAX_SIDE,
        "Image exceeds the raster budget"
    );
    let id = uuid::Uuid::new_v4();
    let staging = Staging(managed_dir.join(format!(".{id}.partial")));
    let path = managed_dir.join(format!("{id}.png"));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging.0)?;
    let mut writer = BoundedWriter {
        inner: file,
        bytes: 0,
        cancelled,
    };
    image::codecs::png::PngEncoder::new(&mut writer).write_image(
        pixels.as_raw(),
        pixels.width(),
        pixels.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    writer.flush()?;
    let size = writer.bytes;
    drop(writer);
    check_cancelled(cancelled)?;
    let thumbnail = thumbnail(pixels);
    fs::rename(&staging.0, &path)?;
    let managed = Arc::new(ManagedFile {
        path: path.clone(),
        submitted: AtomicBool::new(false),
        permit: Some(permit),
    });
    check_cancelled(cancelled)?;
    let name: String = name.chars().filter(|c| !c.is_control()).take(120).collect();
    Ok(Attachment {
        id,
        path,
        thumbnail,
        name,
        size,
        file: managed,
    })
}

fn small_clipboard_raster(bytes: &[u8]) -> std::io::Result<RgbaImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_TOTAL_BYTES);
    reader.limits(limits);
    reader
        .decode()
        .map(image::DynamicImage::into_rgba8)
        .map_err(std::io::Error::other)
}

/// One higher-resolution preview, prepared off-thread on explicit click.
pub fn preview(path: &Path, cancelled: &AtomicBool) -> Result<Arc<RenderImage>> {
    let _guard = DECODE
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    check_cancelled(cancelled)?;
    let file = File::open(path).context("This attachment is unavailable")?;
    let size = file.metadata()?.len();
    let bytes = read_bounded_image(file, size, cancelled)?;
    ensure!(
        supported_content(&bytes),
        "This attachment is not a supported image"
    );
    let mut pixels = crate::background::attachment_thumbnail(&bytes, 768)?;
    check_cancelled(cancelled)?;
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}

/// Reopen a retained history image without another copy. All I/O is worker-only.
pub fn load_saved(
    path: &Path,
    name: &str,
    managed_dir: &Path,
    cancelled: &AtomicBool,
) -> Result<Attachment> {
    let _guard = DECODE
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    check_cancelled(cancelled)?;
    let root = managed_dir
        .canonicalize()
        .context("Saved image storage is unavailable")?;
    let path = path
        .canonicalize()
        .context("The saved image is unavailable")?;
    ensure!(
        path.starts_with(&root),
        "Saved image is outside managed storage"
    );
    let file = File::open(&path)?;
    let size = file.metadata()?.len();
    ensure!(
        size > 0 && size <= MAX_FILE_BYTES,
        "Saved image exceeds 16 MB"
    );
    let bytes = read_bounded_image(file, size, cancelled)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE_BYTES && bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "Saved image is not a normalized PNG"
    );
    check_cancelled(cancelled)?;
    let mut pixels = crate::background::attachment_thumbnail(&bytes, THUMBNAIL_SIDE)?;
    check_cancelled(cancelled)?;
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    let thumbnail = Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .unwrap_or_else(uuid::Uuid::new_v4);
    let name = name.chars().filter(|c| !c.is_control()).take(120).collect();
    let file = Arc::new(ManagedFile {
        path: path.clone(),
        submitted: AtomicBool::new(true),
        permit: None,
    });
    Ok(Attachment {
        id,
        path,
        thumbnail,
        name,
        size,
        file,
    })
}

fn thumbnail(pixels: RgbaImage) -> Arc<RenderImage> {
    let mut thumbnail = image::imageops::thumbnail(&pixels, THUMBNAIL_SIDE, THUMBNAIL_SIDE);
    for pixel in thumbnail.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(thumbnail)]))
}

pub fn supported_content(bytes: &[u8]) -> bool {
    match image::guess_format(bytes) {
        Ok(
            image::ImageFormat::Png
            | image::ImageFormat::Jpeg
            | image::ImageFormat::WebP
            | image::ImageFormat::Gif
            | image::ImageFormat::Tiff
            | image::ImageFormat::Bmp,
        ) => true,
        _ => {
            // ISO-BMFF ftyp major/compatible HEIC brands; decoder validates data.
            if bytes.len() < 16 || &bytes[4..8] != b"ftyp" {
                return false;
            }
            let length = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
            if length < 16 || length > bytes.len() || length > 4096 {
                return false;
            }
            let brands = [&bytes[8..12], &bytes[16..length]];
            brands
                .into_iter()
                .flat_map(|b| b.chunks_exact(4))
                .any(|b| matches!(b, b"heic" | b"heix" | b"hevc" | b"hevx"))
        }
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        bail!("Image import cancelled");
    }
    Ok(())
}

fn read_bounded_image(file: File, size: u64, cancelled: &AtomicBool) -> Result<Vec<u8>> {
    ensure!(
        size > 0 && size <= MAX_FILE_BYTES,
        "Images must be 16 MB or smaller"
    );
    let mut bytes = Vec::with_capacity(size as usize);
    let mut reader = file.take(MAX_FILE_BYTES + 1);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        check_cancelled(cancelled)?;
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(bytes);
        }
        ensure!(
            bytes.len() as u64 + count as u64 <= MAX_FILE_BYTES,
            "Images must be 16 MB or smaller"
        );
        // Keep even a concurrently growing input's allocation within the cap.
        if bytes.capacity() - bytes.len() < count {
            bytes.reserve_exact(count);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}
struct BoundedWriter<'a> {
    inner: File,
    bytes: u64,
    cancelled: &'a AtomicBool,
}
impl Write for BoundedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Image import cancelled",
            ));
        }
        if self.bytes + bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(std::io::Error::other("Normalized image exceeds 16 MB"));
        }
        let count = self.inner.write(bytes)?;
        self.bytes += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipboard_rasters_use_private_normalized_managed_imports_and_reject_limits() {
        use std::io::Cursor;
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let managed = fixture.0.join("managed");
        let image = image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            12,
            6,
            image::Rgba([10, 80, 120, 255]),
        ));
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::Tiff,
            image::ImageFormat::Bmp,
        ] {
            let mut encoded = Cursor::new(Vec::new());
            image.write_to(&mut encoded, format).unwrap();
            let bytes = encoded.into_inner();
            validate_clipboard_image(&bytes).unwrap();
            if format == image::ImageFormat::Bmp || format == image::ImageFormat::Tiff {
                let fallback = small_clipboard_raster(&bytes).unwrap();
                assert_eq!((fallback.width(), fallback.height()), (12, 6));
            }
            let item = import_clipboard(&bytes, &managed, &AtomicBool::new(false))
                .unwrap_or_else(|error| panic!("{format:?} clipboard import: {error:#}"));
            assert_eq!(item.name, "Pasted image");
            assert!(item.path.starts_with(&managed));
            assert!(
                fs::read(&item.path)
                    .unwrap()
                    .starts_with(b"\x89PNG\r\n\x1a\n")
            );
            assert_eq!(
                fs::metadata(&item.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert!(item.thumbnail.size(0).width.0 <= THUMBNAIL_SIDE as i32);
        }
        assert!(validate_clipboard_image(&[]).is_err());
        assert!(validate_clipboard_image(b"<svg><script>no</script></svg>").is_err());
        assert!(validate_clipboard_image(&vec![0; MAX_FILE_BYTES as usize + 1]).is_err());
        assert!(import_clipboard(b"\x89PNG\r\n\x1a\n", &managed, &AtomicBool::new(true)).is_err());
        let mut oversized_bmp = Cursor::new(Vec::new());
        image
            .write_to(&mut oversized_bmp, image::ImageFormat::Bmp)
            .unwrap();
        let mut oversized_bmp = oversized_bmp.into_inner();
        oversized_bmp[18..22].copy_from_slice(&(MAX_SIDE + 1).to_le_bytes());
        assert!(
            small_clipboard_raster(&oversized_bmp).is_err(),
            "Fallback must reject oversized dimensions before raster allocation"
        );
    }
    #[cfg(target_os = "macos")]
    #[gpui::test]
    fn retina_screenshot_native_clipboard_pastes_as_a_bounded_composer_attachment(
        cx: &mut gpui::TestAppContext,
    ) {
        use objc2::{
            msg_send,
            rc::autoreleasepool,
            runtime::{AnyClass, AnyObject, Bool},
        };
        use objc2_foundation::{NSArray, NSData, NSString};
        use std::io::Cursor;
        let screenshot = image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            2600,
            1700,
            image::Rgba([90, 120, 160, 255]),
        ));
        let mut encoded = Cursor::new(Vec::new());
        screenshot
            .write_to(&mut encoded, image::ImageFormat::Tiff)
            .unwrap();
        drop(screenshot);
        let bytes = encoded.into_inner();
        assert!(bytes.len() as u64 > MAX_FILE_BYTES);
        assert!(bytes.len() as u64 <= MAX_CLIPBOARD_BYTES);
        validate_clipboard_image(&bytes).unwrap();
        // A unique native board exercises the production UTI/NSData reader
        // without reading or replacing the user's general clipboard.
        let copied = autoreleasepool(|_| unsafe {
            let class = AnyClass::get(c"NSPasteboard").unwrap();
            let board: *mut AnyObject = msg_send![class, pasteboardWithUniqueName];
            assert!(!board.is_null());
            let data = NSData::with_bytes(&bytes);
            let uti = NSString::from_str("public.tiff");
            let types = NSArray::from_slice(&[&*uti]);
            let _: isize =
                msg_send![board, declareTypes: &*types, owner: std::ptr::null_mut::<AnyObject>()];
            let written: Bool = msg_send![board, setData: &*data, forType: &*uti];
            assert!(written.as_bool());
            let captured = native_clipboard_image_from(&*board);
            let _: () = msg_send![board, releaseGlobally];
            captured.unwrap().unwrap()
        });
        assert_eq!(copied.as_ref(), bytes.as_slice());
        drop(bytes);
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.simulate_input("Keep my draft");
        cx.write_to_clipboard(gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(
            gpui::ImageFormat::Tiff,
            copied.to_vec(),
        )));
        drop(copied);
        cx.simulate_keystrokes("cmd-v");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert_eq!(app.composer.read(cx).text(), "Keep my draft");
            assert_eq!(
                app.attachments.len(),
                1,
                "Screenshot paste failed: {:?}",
                app.notice
            );
            assert_eq!(app.attachment_pending, 0);
            assert!(app.runs.is_empty());
            let attachment = &app.attachments[0];
            assert!(attachment.size <= MAX_FILE_BYTES);
            assert_eq!(attachment.name, "Pasted image");
            let (width, height) = image::image_dimensions(&attachment.path).unwrap();
            assert_eq!(width, MAX_SIDE);
            assert!(height > 0 && height <= MAX_SIDE);
        });
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("orbit-attachment-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn imported_image_is_private_bounded_source_independent_and_owned() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let source = fixture.0.join("actually-a-png.txt");
        RgbaImage::from_pixel(2500, 200, image::Rgba([250, 40, 80, 255]))
            .save_with_format(&source, image::ImageFormat::Png)
            .unwrap();
        let attachment =
            import(&source, &fixture.0.join("managed"), &AtomicBool::new(false)).unwrap();
        fs::remove_file(source).unwrap();
        assert!(attachment.path.is_file());
        assert_eq!(
            fs::metadata(&attachment.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let (width, height) = image::image_dimensions(&attachment.path).unwrap();
        assert!(width <= MAX_SIDE && height <= MAX_SIDE);
        let thumbnail_size = attachment.thumbnail.size(0);
        assert!(
            thumbnail_size.width.0 <= THUMBNAIL_SIDE as i32
                && thumbnail_size.height.0 <= THUMBNAIL_SIDE as i32
        );
        let path = attachment.path.clone();
        let retained = attachment.clone();
        drop(attachment);
        assert!(path.exists());
        drop(retained);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!path.exists());
    }
    #[test]
    fn submitted_images_survive_and_cancellation_or_invalid_content_leaves_no_files() {
        let fixture = Fixture::new();
        let source = fixture.0.join("photo.png");
        RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]))
            .save(&source)
            .unwrap();
        let managed = fixture.0.join("managed");
        assert!(import(&source, &managed, &AtomicBool::new(true)).is_err());
        assert!(!managed.exists());
        fs::write(&source, b"fake PNG text").unwrap();
        assert!(import(&source, &managed, &AtomicBool::new(false)).is_err());
        RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]))
            .save(&source)
            .unwrap();
        let attachment = import(&source, &managed, &AtomicBool::new(false)).unwrap();
        let path = attachment.path.clone();
        attachment.mark_submitted();
        drop(attachment);
        assert!(path.exists());
        assert_eq!(fs::read_dir(managed).unwrap().count(), 1);
    }
    #[test]
    fn encoded_file_and_cache_quota_are_enforced_before_decode_or_commit() {
        let fixture = Fixture::new();
        let source = fixture.0.join("oversized.png");
        File::create(&source)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        assert!(import(&source, &fixture.0.join("managed"), &AtomicBool::new(false)).is_err());
        RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]))
            .save(&source)
            .unwrap();
        let managed = fixture.0.join("managed");
        fs::create_dir_all(&managed).unwrap();
        File::create(managed.join("retained.png"))
            .unwrap()
            .set_len(CACHE_BYTES)
            .unwrap();
        assert!(import(&source, &managed, &AtomicBool::new(false)).is_err());
        assert_eq!(fs::read_dir(managed).unwrap().count(), 1);
    }

    #[test]
    fn saved_images_are_confined_and_previews_are_lazily_bounded() {
        let fixture = Fixture::new();
        let source = fixture.0.join("photo.png");
        RgbaImage::from_pixel(1800, 900, image::Rgba([20, 50, 80, 255]))
            .save(&source)
            .unwrap();
        let managed = fixture.0.join("managed");
        let attachment = import(&source, &managed, &AtomicBool::new(false)).unwrap();
        attachment.mark_submitted();
        let path = attachment.path.clone();
        drop(attachment);
        let saved =
            load_saved(&path, "Historical photo", &managed, &AtomicBool::new(false)).unwrap();
        assert_eq!(saved.name, "Historical photo");
        assert!(load_saved(&source, "outside", &managed, &AtomicBool::new(false)).is_err());
        assert!(load_saved(&path, "cancelled", &managed, &AtomicBool::new(true)).is_err());
        let preview = preview(&path, &AtomicBool::new(false)).unwrap();
        assert!(preview.size(0).width.0 <= 768 && preview.size(0).height.0 <= 768);
        assert!(preview.as_bytes(0).unwrap().len() <= 768 * 768 * 4);
        drop(saved);
        assert!(path.exists());
    }
}

/// Finder file copies are references, never inserted as raw composer text.
pub fn clipboard_paths()->Vec<std::path::PathBuf>{
    #[cfg(all(target_os="macos",not(test)))]
    {use objc2::{msg_send,runtime::{AnyClass,AnyObject}};use objc2_foundation::NSString;
    unsafe{let Some(class)=AnyClass::get(c"NSPasteboard") else{return vec![]};let board:*mut AnyObject=msg_send![class,generalPasteboard];if board.is_null(){return vec![]}let kind=NSString::from_str("NSFilenamesPboardType");let list:*mut AnyObject=msg_send![board,propertyListForType:&*kind];if list.is_null(){return vec![]}let Some(array)=AnyClass::get(c"NSArray") else{return vec![]};let valid:bool=msg_send![list,isKindOfClass:array];if !valid{return vec![]}let count:usize=msg_send![list,count];(0..count.min(9)).filter_map(|i|{let item:*mut AnyObject=msg_send![list,objectAtIndex:i];let raw:*const std::ffi::c_char=msg_send![item,UTF8String];if raw.is_null(){None}else{Some(std::path::PathBuf::from(std::ffi::CStr::from_ptr(raw).to_string_lossy().into_owned()))}}).collect()}}
    #[cfg(any(not(target_os="macos"),test))]{vec![]}
}
