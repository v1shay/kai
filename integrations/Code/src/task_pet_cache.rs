//! Scoped, bounded inline sprite-sheet assets. Bundled and imported sheets share
//! this cache across Orbit views, without entering the generic path-image cache.
use gpui::{
    App, AppContext, Asset, Entity, ImageCache, ImageCacheError, RenderImage, Resource, Window,
};
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

const MAX_ENCODED_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SOURCE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_SHEET_WIDTH: u32 = 512;
const MAX_SHEET_HEIGHT: u32 = 4096;
const MAX_OUTPUT_PIXELS: u64 = 512 * 1024;
const MAX_ENTRIES: usize = 128;

pub(super) struct Sheets {
    owner: uuid::Uuid,
    library_revision: Option<u64>,
    images: HashMap<PathBuf, Option<Arc<RenderImage>>>,
}
impl Sheets {
    pub(super) fn new(cx: &mut App) -> Entity<Self> {
        let cache = cx.new(|_| Self {
            owner: uuid::Uuid::new_v4(),
            library_revision: None,
            images: HashMap::new(),
        });
        cx.observe_release(&cache, |cache, cx| {
            for (path, image) in cache.images.drain() {
                cx.remove_asset::<Sheet>(&(cache.owner, path));
                if let Some(image) = image {
                    cx.drop_image(image, None);
                }
            }
        })
        .detach();
        cache
    }
    pub(super) fn set_library_revision(
        &mut self,
        revision: Option<u64>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(revision) = revision else {
            return;
        };
        if self.library_revision == Some(revision) {
            return;
        }
        self.library_revision = Some(revision);
        // A cache generation has a distinct asset-task namespace. Old decode
        // completions cannot be fetched by a newly requested same-path sheet.
        let previous_owner = std::mem::replace(&mut self.owner, uuid::Uuid::new_v4());
        for (path, image) in self.images.drain() {
            cx.remove_asset::<Sheet>(&(previous_owner, path));
            if let Some(image) = image {
                cx.drop_image(image, Some(&mut *window));
            }
        }
    }
    pub(super) fn remove(&mut self, path: &Path, window: &mut Window, cx: &mut App) {
        cx.remove_asset::<Sheet>(&(self.owner, path.to_path_buf()));
        if let Some(Some(image)) = self.images.remove(path) {
            cx.drop_image(image, Some(window));
        }
    }
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.images.len()
    }
}
impl ImageCache for Sheets {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let Resource::Path(path) = resource else {
            return Some(Err(ImageCacheError::Asset(
                "Inline pets require a local sprite sheet".into(),
            )));
        };
        let path = path.to_path_buf();
        if !self.images.contains_key(&path) && self.images.len() >= MAX_ENTRIES {
            return Some(Err(ImageCacheError::Asset(
                "Inline pet cache reached its 256 MiB decoded budget".into(),
            )));
        }
        let image = self.images.entry(path.clone()).or_default();
        if let Some(image) = image {
            return Some(Ok(image.clone()));
        }
        let result = window.use_asset::<Sheet>(&(self.owner, path), cx);
        if let Some(Ok(decoded)) = &result {
            *image = Some(decoded.clone());
        }
        result
    }
}

struct Sheet;
struct DecodePermit(async_channel::Sender<()>);
impl Drop for DecodePermit {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}
impl Asset for Sheet {
    type Source = (uuid::Uuid, PathBuf);
    type Output = Result<Arc<RenderImage>, ImageCacheError>;
    fn load(
        (_, path): Self::Source,
        _: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        async move {
            // Waiting loads do not hold encoded/decoded pixels or block executor
            // threads. At most one full-resolution source is decoded at a time.
            static DECODE: OnceLock<(async_channel::Sender<()>, async_channel::Receiver<()>)> =
                OnceLock::new();
            let (sender, receiver) = DECODE.get_or_init(|| {
                let (sender, receiver) = async_channel::bounded(1);
                sender.try_send(()).unwrap();
                (sender, receiver)
            });
            receiver
                .recv()
                .await
                .map_err(|e| ImageCacheError::Asset(e.to_string().into()))?;
            let _permit = DecodePermit(sender.clone());
            load_sheet(&path).map_err(|e| ImageCacheError::Asset(e.to_string().into()))
        }
    }
}
fn load_sheet(path: &Path) -> anyhow::Result<Arc<RenderImage>> {
    let file = std::fs::File::open(path)?;
    anyhow::ensure!(
        file.metadata()?.len() <= MAX_ENCODED_BYTES,
        "Pet sprite sheet exceeds 32 MiB"
    );
    let mut bytes = Vec::new();
    file.take(MAX_ENCODED_BYTES + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_ENCODED_BYTES,
        "Pet sprite sheet exceeds 32 MiB"
    );
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()?
        .into_dimensions()?;
    anyhow::ensure!(
        width > 0
            && height > 0
            && width <= 8192
            && height <= 8192
            && u64::from(width) * u64::from(height) <= MAX_SOURCE_PIXELS,
        "Pet sprite sheet exceeds its pixel budget"
    );
    let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    // Native import converts animated GIF packs to one static sprite sheet.
    // Decode one sheet image; timeline rows/columns supply the animation.
    let scale = (f64::from(MAX_SHEET_WIDTH) / f64::from(width))
        .min(f64::from(MAX_SHEET_HEIGHT) / f64::from(height))
        .min((MAX_OUTPUT_PIXELS as f64 / (u64::from(width) * u64::from(height)) as f64).sqrt())
        .min(1.0);
    let target_width = (f64::from(width) * scale).floor().max(1.0) as u32;
    let target_height = (f64::from(height) * scale).floor().max(1.0) as u32;
    let mut image = reader
        .decode()?
        .resize_exact(
            target_width,
            target_height,
            image::imageops::FilterType::Triangle,
        )
        .into_rgba8();
    for pixel in image.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(image)])))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_sheet_preserves_geometry_and_bgra_colors_within_two_mib() {
        let root = std::env::temp_dir().join(format!("kai-sheet-budget-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("large.png");
        image::RgbaImage::from_pixel(1536, 2288, image::Rgba([17, 39, 83, 255]))
            .save(&path)
            .unwrap();
        let sheet = load_sheet(&path).unwrap();
        assert_eq!(sheet.frame_count(), 1);
        assert_eq!(
            sheet.size(0).width.0,
            512,
            "supported sheets retain64pixels per each of8columns"
        );
        let size = sheet.size(0);
        let ratio = size.width.0 as f64 / size.height.0 as f64;
        assert!((ratio - 1536.0 / 2288.0).abs() < 0.002);
        assert!(sheet.as_bytes(0).unwrap().len() <= 2 * 1024 * 1024);
        assert!(
            sheet
                .as_bytes(0)
                .unwrap()
                .chunks_exact(4)
                .all(|pixel| pixel == [83, 39, 17, 255])
        );
        image::RgbaImage::new(256, 8192).save(&path).unwrap();
        let tall = load_sheet(&path).unwrap();
        assert!(tall.as_bytes(0).unwrap().len() <= 2 * 1024 * 1024);
        let path = root.join("too-large.png");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_ENCODED_BYTES + 1)
            .unwrap();
        assert!(load_sheet(&path).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;
    use gpui::{prelude::*, *};
    use std::prelude::v1::test;
    struct SheetView {
        cache: Entity<Sheets>,
        path: PathBuf,
        visible: bool,
    }
    impl Render for SheetView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().children(self.visible.then(|| {
                img(self.path.clone())
                    .image_cache(&self.cache)
                    .size(px(32.))
            }))
        }
    }
    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    fn path(cx: &mut TestAppContext) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("kai-cache-lifetime-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("sheet.png");
        image::RgbaImage::from_pixel(128, 144, image::Rgba([17, 39, 83, 255]))
            .save(&path)
            .unwrap();
        cx.on_quit(move || {
            let _ = std::fs::remove_dir_all(root);
        });
        path
    }
    #[gpui::test]
    fn independent_cache_retirement_does_not_evict_another_owner_of_the_same_path(
        cx: &mut TestAppContext,
    ) {
        let path = path(cx);
        let (first, cx) = cx.add_window_view({
            let path = path.clone();
            move |_, cx| SheetView {
                cache: Sheets::new(cx),
                path,
                visible: true,
            }
        });
        cx.run_until_parked();
        for _ in 0..2 {
            cx.draw(Point::default(), size(px(64.), px(64.)), {
                let first = first.clone();
                move |_, _| first.into_any_element()
            });
            cx.run_until_parked();
        }
        let first_window = cx.update(|window, _| window.window_handle());
        let a = cx.update(|_, cx| first.read(cx).cache.clone());
        let a_id = cx.update(|_, cx| a.read(cx).images.values().flatten().next().unwrap().id);
        let (second, cx) = cx.add_window_view({
            let path = path.clone();
            move |_, cx| SheetView {
                cache: Sheets::new(cx),
                path,
                visible: true,
            }
        });
        cx.run_until_parked();
        for _ in 0..2 {
            cx.draw(Point::default(), size(px(64.), px(64.)), {
                let second = second.clone();
                move |_, _| second.into_any_element()
            });
            cx.run_until_parked();
        }
        let b = cx.update(|_, cx| second.read(cx).cache.clone());
        let b_id = cx.update(|_, cx| b.read(cx).images.values().flatten().next().unwrap().id);
        assert_ne!(
            a_id, b_id,
            "independent model caches must not share a global task/image lifetime"
        );
        first_window
            .update(cx, |_, window, cx| {
                first.update(cx, |view, cx| {
                    view.visible = false;
                    view.cache
                        .update(cx, |cache, cx| cache.remove(&path, window, cx));
                    cx.notify();
                })
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| a.read(cx).len()), 0);
        assert_eq!(cx.update(|_, cx| b.read(cx).len()), 1);
        assert!(
            cx.update(|window, _| window.image_atlas_stats_for_test().0.contains(&b_id)),
            "first cache removal evicted another cache's live upload"
        );
    }
    #[gpui::test]
    fn pending_decode_cannot_restore_retired_or_released_cache_ownership(cx: &mut TestAppContext) {
        let path = path(cx);
        let (view, cx) = cx.add_window_view({
            let path = path.clone();
            move |_, cx| SheetView {
                cache: Sheets::new(cx),
                path,
                visible: false,
            }
        });
        let cache = cx.update(|_, cx| view.read(cx).cache.clone());
        cx.update(|window, cx| {
            view.update(cx, |view, _| view.visible = true);
            window.draw(cx).clear(); // Establish a real asynchronous loading task.
            view.update(cx, |view, cx| {
                view.visible = false;
                view.cache
                    .update(cx, |cache, cx| cache.remove(&path, window, cx));
                cx.notify();
            });
        });
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| cache.read(cx).len()), 0);
        assert!(cx.update(|window, _| window.image_atlas_stats_for_test().0.is_empty()));
        cx.update(|window, cx| {
            view.update(cx, |view, _| view.visible = true);
            window.draw(cx).clear();
            window.replace_root(cx, |_, _| Empty);
            drop(view);
        });
        let weak = cache.downgrade();
        drop(cache);
        cx.run_until_parked();
        assert!(
            weak.upgrade().is_none(),
            "pending loader retained or recreated its released cache entity"
        );
        assert!(cx.update(|window, _| window.image_atlas_stats_for_test().0.is_empty()));
    }
    #[gpui::test]
    fn explicit_revision_reloads_same_path_for_shared_views_and_preserves_legacy_updates(
        cx: &mut TestAppContext,
    ) {
        let path = path(cx);
        let (first, cx) = cx.add_window_view({
            let path = path.clone();
            move |window, cx| {
                let cache = Sheets::new(cx);
                cache.update(cx, |cache, cx| {
                    cache.set_library_revision(Some(1), window, cx)
                });
                SheetView {
                    cache,
                    path,
                    visible: true,
                }
            }
        });
        for _ in 0..2 {
            cx.draw(Point::default(), size(px(64.), px(64.)), {
                let view = first.clone();
                move |_, _| view.into_any_element()
            });
            cx.run_until_parked();
        }
        let first_window = cx.update(|window, _| window.window_handle());
        let cache = cx.update(|_, cx| first.read(cx).cache.clone());
        let old = cx.update(|_, cx| {
            cache
                .read(cx)
                .images
                .values()
                .flatten()
                .next()
                .unwrap()
                .clone()
        });
        let (second, cx) = cx.add_window_view({
            let cache = cache.clone();
            let path = path.clone();
            move |_, _| SheetView {
                cache,
                path,
                visible: true,
            }
        });
        for _ in 0..2 {
            cx.draw(Point::default(), size(px(64.), px(64.)), {
                let view = second.clone();
                move |_, _| view.into_any_element()
            });
            cx.run_until_parked();
        }
        assert!(cx.update(|window, _| window.image_atlas_stats_for_test().0.contains(&old.id)));
        image::RgbaImage::from_pixel(128, 144, image::Rgba([31, 47, 211, 255]))
            .save(&path)
            .unwrap();
        cx.update(|window, cx| {
            cache.update(cx, |cache, cx| {
                let owner = cache.owner;
                cache.set_library_revision(None, window, cx); // Old clients and status changes retain current image.
                cache.set_library_revision(Some(1), window, cx);
                assert_eq!(cache.owner, owner);
                assert_eq!(cache.images.values().flatten().next().unwrap().id, old.id);
            })
        });
        cx.update(|window, cx| {
            cache.update(cx, |cache, cx| {
                cache.set_library_revision(Some(2), window, cx)
            })
        });
        for _ in 0..2 {
            first_window
                .update(cx, |_, window, cx| window.draw(cx).clear())
                .unwrap();
            cx.draw(Point::default(), size(px(64.), px(64.)), {
                let view = second.clone();
                move |_, _| view.into_any_element()
            });
            cx.run_until_parked();
        }
        let current = cx.update(|_, cx| {
            cache
                .read(cx)
                .images
                .values()
                .flatten()
                .next()
                .unwrap()
                .clone()
        });
        assert_eq!(
            &current.as_bytes(0).unwrap()[..4],
            &[211, 47, 31, 255],
            "same-path replacement still uses old decoded pixels"
        );
        assert_ne!(current.id, old.id);
        assert!(!cx.update(|window, _| window.image_atlas_stats_for_test().0.contains(&old.id)));
        assert!(cx.update(|window, _| window.image_atlas_stats_for_test().0.contains(&current.id)));
        first_window
            .update(cx, |_, window, _| {
                let ids = window.image_atlas_stats_for_test().0;
                assert!(!ids.contains(&old.id));
                assert!(ids.contains(&current.id));
            })
            .unwrap();
        cx.update(|window, cx| {
            cache.update(cx, |cache, cx| {
                cache.set_library_revision(None, window, cx);
                assert_eq!(
                    cache.images.values().flatten().next().unwrap().id,
                    current.id
                );
            })
        });
    }
    #[gpui::test]
    fn old_decode_completion_cannot_repopulate_a_new_library_revision(cx: &mut TestAppContext) {
        let path = path(cx);
        let (view, cx) = cx.add_window_view({
            let path = path.clone();
            move |window, cx| {
                let cache = Sheets::new(cx);
                cache.update(cx, |cache, cx| {
                    cache.set_library_revision(Some(1), window, cx)
                });
                SheetView {
                    cache,
                    path,
                    visible: false,
                }
            }
        });
        let cache = cx.update(|_, cx| view.read(cx).cache.clone());
        let (old_task, old_owner) = cx.update(|window, cx| {
            view.update(cx, |view, _| view.visible = true);
            window.draw(cx).clear();
            let old_owner = cache.read(cx).owner;
            let old_task = cx.fetch_asset::<Sheet>(&(old_owner, path.clone())).0;
            cache.update(cx, |cache, cx| {
                cache.set_library_revision(Some(2), window, cx)
            });
            image::RgbaImage::from_pixel(128, 144, image::Rgba([31, 47, 211, 255]))
                .save(&path)
                .unwrap();
            view.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear();
            (old_task, old_owner)
        });
        cx.run_until_parked();
        for _ in 0..2 {
            cx.draw(Point::default(), size(px(64.), px(64.)), {
                let view = view.clone();
                move |_, _| view.into_any_element()
            });
            cx.run_until_parked();
        }
        let old_result = futures_lite::future::block_on(old_task).unwrap();
        let current = cx.update(|_, cx| {
            let cache = cache.read(cx);
            assert_ne!(cache.owner, old_owner);
            assert_eq!(cache.library_revision, Some(2));
            assert_eq!(cache.images.len(), 1);
            cache.images.values().flatten().next().unwrap().clone()
        });
        assert_ne!(
            current.id, old_result.id,
            "old task result crossed the revision namespace"
        );
        assert_eq!(&current.as_bytes(0).unwrap()[..4], &[211, 47, 31, 255]);
    }
}
