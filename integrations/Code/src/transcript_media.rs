use super::*;
use crate::{citations::Citation, interaction_ui::WorkMotion, remote_workspace::RemoteFiles};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const SLOT: &str = "transcript-media";
const MAX_NATIVE_BYTES: u64 = 128 * 1024 * 1024;

struct ToolImageAsset;

impl gpui::Asset for ToolImageAsset {
    type Source = Arc<str>;
    type Output = Result<Arc<RenderImage>, gpui::ImageCacheError>;

    fn load(source: Self::Source, _: &mut App) -> impl Future<Output = Self::Output> + std::marker::Send + 'static {
        async move {
            crate::tool_images::decode(&source).map_err(|error| gpui::ImageCacheError::Asset(error.into()))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Photo,
    Image,
    Pdf,
    Video,
    Model,
}

impl Kind {
    pub(super) fn for_target(target: &str) -> Option<Self> {
        let Ok(Citation::File(file)) = citations::classify(target) else {
            return None;
        };
        if file.line.is_some() {
            return None;
        }
        Self::for_path(&file.path)
    }

    fn for_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "png" | "jpg" | "jpeg" | "webp" | "bmp" | "heic" | "heif" | "tif" | "tiff" => {
                Some(Self::Photo)
            }
            "gif" | "svg" | "avif" => Some(Self::Image),
            "pdf" => Some(Self::Pdf),
            "mov" | "mp4" | "m4v" => Some(Self::Video),
            "usdz" => Some(Self::Model),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Photo | Self::Image => "Image",
            Self::Pdf => "PDF",
            Self::Video => "Video",
            Self::Model => "3D model",
        }
    }

    fn height(self) -> f32 {
        match self {
            Self::Pdf => 360.,
            Self::Video | Self::Model => 300.,
            Self::Image | Self::Photo => 260.,
        }
    }
}

enum Source {
    Local(PathBuf),
    Remote(RemoteFiles),
    Unavailable,
}

#[derive(serde::Deserialize)]
struct FileSize {
    size: u64,
}

struct Download(PathBuf);

impl Drop for Download {
    fn drop(&mut self) {
        let path = std::mem::take(&mut self.0);
        // A dismissed preview can own a large remote file. Cleanup stays off the UI thread.
        let _ = std::thread::Builder::new()
            .name("kai-media-cleanup".into())
            .spawn(move || {
                let _ = fs::remove_dir_all(path);
            });
    }
}

struct Media {
    path: PathBuf,
    photo: Option<Arc<RenderImage>>,
    _download: Option<Download>,
}

fn load(source: Source, target: &str, kind: Kind, cancelled: &AtomicBool) -> Result<Media, String> {
    let Ok(Citation::File(citation)) = citations::classify(target) else {
        return Err("This file link is unavailable.".into());
    };
    if cancelled.load(Ordering::Acquire) {
        return Err("Preview closed.".into());
    }
    let (path, download) = match source {
        Source::Local(root) => {
            let path = root.join(citation.path);
            // Canonicalize before asking either decoder to open the file.
            let path = path
                .canonicalize()
                .map_err(|_| "This file is unavailable.")?;
            (path, None)
        }
        Source::Remote(files) => {
            let remote_path = files.path(&citation.path)?;
            let name = remote_path
                .file_name()
                .ok_or("This file link is unavailable.")?;
            let metadata: FileSize =
                files.call("file-preview", &remote_path, serde_json::Value::Null)?;
            if metadata.size > MAX_NATIVE_BYTES {
                return Err("Inline previews support files up to 128 MB.".into());
            }
            if cancelled.load(Ordering::Acquire) {
                return Err("Preview closed.".into());
            }
            let directory =
                std::env::temp_dir().join(format!("kai-chat-media-{}", uuid::Uuid::new_v4()));
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .map_err(|_| "Kai couldn’t prepare the temporary preview.")?;
            let download = Download(directory);
            let path = download.0.join(name);
            files.call::<serde_json::Value>(
                "file-download",
                &remote_path,
                serde_json::json!({"path":path}),
            )?;
            (path, Some(download))
        }
        Source::Unavailable => {
            return Err("Reconnect to the host and check this project’s file access.".into());
        }
    };
    if cancelled.load(Ordering::Acquire) {
        return Err("Preview closed.".into());
    }
    let info = fs::metadata(&path).map_err(|_| "This file is unavailable.")?;
    if !info.is_file() || Kind::for_path(&path) != Some(kind) {
        return Err("This file isn’t a supported preview type.".into());
    }
    if info.len() > MAX_NATIVE_BYTES {
        return Err("Inline previews support files up to 128 MB.".into());
    }
    let photo = if kind == Kind::Photo {
        Some(attachments::preview(&path, cancelled).map_err(|e| e.to_string())?)
    } else {
        None
    };
    Ok(Media {
        path,
        photo,
        _download: download,
    })
}

#[derive(PartialEq, Eq)]
struct Key {
    scope: String,
    row: usize,
    target: String,
}

pub(super) struct Preview {
    key: Key,
    window: WindowId,
    cancel: Arc<AtomicBool>,
    opened: bool,
    motion: Option<WorkMotion>,
    result: Option<Result<Media, String>>,
    painted: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl Preview {
    fn reveal(&self) -> f32 {
        self.motion
            .map_or(if self.opened { 1. } else { 0. }, WorkMotion::progress)
    }
}

impl Orbit {
    pub(super) fn tick_transcript_media(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preview) = self
            .transcript_media
            .as_mut()
            .filter(|p| p.window == window.window_handle().window_id())
        else {
            return;
        };
        if preview.motion.is_some_and(WorkMotion::active) {
            request_motion_frame(window, cx);
        } else {
            preview.motion = None;
            if !preview.opened {
                self.transcript_media = None;
                return;
            }
        }
        if unified::active()
            && preview
                .result
                .as_ref()
                .is_some_and(|result| result.as_ref().is_ok_and(|media| media.photo.is_none()))
        {
            let painted = preview.painted.clone();
            painted.set(false);
            window.on_frame_submitted(move |_, _| {
                if !painted.get() {
                    unified::hide_inline_preview(SLOT);
                }
            });
        }
    }

    pub(super) fn transcript_media_card(
        &self,
        row: usize,
        name: &str,
        target: &str,
        peer: Option<&SessionSummary>,
        remote: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let scope = if remote {
            self.remote_workspace.media_scope()?
        } else {
            format!(
                "local:{}",
                peer.map(|session| session.id.as_str())
                    .or(self.active.as_deref())?
            )
        };
        if crate::tool_images::is_target(target) {
            use gpui::StyledImage;
            use std::hash::{DefaultHasher, Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            (scope, row, target).hash(&mut hasher);
            let id = hasher.finish();
            let collapsed = self.collapsed_tool_images.contains(&id);
            let main = peer.is_none();
            let source: Arc<str> = target.into();
            let media = div().max_w(px(480.)).child(div().id(("tool-image-toggle", row))
                .debug_selector(move || format!("tool-image-toggle-{row}"))
                .cursor_pointer().flex().items_center().gap_2().px_1().py_1()
                .child(icon(if collapsed { "chevron-right" } else { "chevron-down" }, 12., MUTED))
                .child(div().text_size(theme::ui_px(12.)).text_color(rgb(MUTED)).child(name.to_owned()))
                .on_click(cx.listener(move |app, _, _, cx| {
                    if !app.collapsed_tool_images.remove(&id) {
                        app.collapsed_tool_images.insert(id);
                    }
                    let rows = vec![row..row + 1];
                    if remote { app.remote_workspace.remeasure_media_rows(rows); }
                    else if main { interaction_ui::remeasure_work_rows(&app.list, rows); }
                    cx.notify();
                })));
            return Some(media.when(!collapsed, |card| card.child(
                img(move |window: &mut Window, cx: &mut App| window.use_asset::<ToolImageAsset>(&source, cx))
                    .with_fallback(|| media_caption("The tool image could not be decoded.").into_any_element())
                    .w_full().h(px(255.)).rounded_md().object_fit(ObjectFit::Contain)
            )).into_any_element());
        }
        let kind = Kind::for_target(target)?;
        let key = Key {
            scope,
            row,
            target: target.to_owned(),
        };
        let preview = self
            .transcript_media
            .as_ref()
            .filter(|entry| entry.key == key);
        let opened = preview.is_some_and(|entry| entry.opened);
        let name = name.to_owned();
        let open_target = key.target.clone();
        let open_peer = peer.cloned();
        let click_peer = open_peer.clone();
        let header = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id(("media-toggle", row))
                    .debug_selector(move || format!("media-toggle-{row}"))
                    .max_w_full()
                    .min_w(px(0.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .rounded_md()
                    .px_2()
                    .py_1()
                    .hover(|style| style.bg(rgba(GLASS_HOVER)))
                    .child(
                        icon("chevron-right", 12., MUTED)
                            .flex_none()
                            .with_transformation(Transformation::rotate(gpui::radians(
                                preview.map_or(0., Preview::reveal) * std::f32::consts::FRAC_PI_2,
                            ))),
                    )
                    .child(div().min_w(px(0.)).text_ellipsis().child(name))
                    .child(div().flex_none().text_color(rgb(MUTED)).child(kind.label()))
                    .on_click(cx.listener(move |app, _, window, cx| {
                        let key = Key {
                            scope: key.scope.clone(),
                            row,
                            target: key.target.clone(),
                        };
                        app.toggle_transcript_media(
                            key,
                            kind,
                            click_peer.as_ref(),
                            remote,
                            window,
                            cx,
                        );
                    })),
            )
            .child(
                div()
                    .id(("media-file", row))
                    .flex_none()
                    .px_1()
                    .py_1()
                    .cursor_pointer()
                    .rounded_md()
                    .hover(|style| style.bg(rgba(GLASS_HOVER)))
                    .child(icon("external", 12., MUTED))
                    .on_click(cx.listener(move |app, _, window, cx| {
                        if remote {
                            app.open_remote_citation(&open_target, window, cx);
                        } else {
                            if let Some(session) = &open_peer
                                && app.active.as_deref() != Some(&session.id)
                            {
                                app.open_session(session.clone(), window, cx);
                            }
                            app.open_citation(&open_target, window, cx);
                        }
                    })),
            );
        let mut card = div().text_size(theme::ui_px(12.)).child(header);
        if let Some(preview) = preview {
            let (height, body) = match &preview.result {
                Some(Ok(Media {
                    photo: Some(photo), ..
                })) => {
                    let photo = photo.clone();
                    let size = photo.size(0);
                    let aspect = size.height.0 as f32 / size.width.0.max(1) as f32;
                    let height = (460. * aspect).clamp(48., kind.height());
                    let image = img(photo)
                        .h(px(height))
                        .w_full()
                        .object_fit(ObjectFit::Contain);
                    (height, div().max_w(px(460.)).child(image))
                }
                Some(Ok(media)) if opened && preview.motion.is_none() && unified::active() => (
                    kind.height(),
                    div().max_w(px(540.)).child(
                        unified::transcript_inline_preview(
                            SLOT,
                            &media.path,
                            preview.window,
                            preview.painted.clone(),
                        )
                        .w_full()
                        .h(px(kind.height())),
                    ),
                ),
                Some(Ok(_)) if !unified::active() => {
                    (35., media_caption("Open the file for a full preview."))
                }
                Some(Ok(_)) => (kind.height(), div()),
                Some(Err(error)) => (35., media_caption(error.clone())),
                None => (35., media_caption("Loading preview…")),
            };
            card = card.child(
                div()
                    .id(("media-content", row))
                    .debug_selector(move || format!("media-content-{row}"))
                    .mt_1()
                    .ml_2()
                    .overflow_hidden()
                    .h(px(height * preview.reveal()))
                    .child(body),
            );
        }
        Some(card.into_any_element())
    }

    fn toggle_transcript_media(
        &mut self,
        key: Key,
        kind: Kind,
        peer: Option<&SessionSummary>,
        remote: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let window_id = window.window_handle().window_id();
        let motion = self.motion && !native::reduced_motion();
        let mut rows = vec![key.row..key.row.saturating_add(1)];
        if let Some(previous) = &self.transcript_media {
            if previous.key.scope == key.scope && previous.key.row != key.row {
                rows.push(previous.key.row..previous.key.row.saturating_add(1));
            }
        }
        if let Some(preview) = &mut self.transcript_media
            && preview.key == key
            && preview.opened
            && preview.window == window_id
        {
            let from = preview.reveal();
            preview.opened = false;
            preview.motion = motion.then(|| WorkMotion::new(from, false));
            preview.cancel.store(true, Ordering::Release);
            unified::hide_inline_preview(SLOT);
        } else {
            unified::hide_inline_preview(SLOT);
            let source = if remote {
                self.remote_workspace
                    .media_source()
                    .map(Source::Remote)
                    .unwrap_or(Source::Unavailable)
            } else {
                let root = peer
                    .map(|session| {
                        session
                            .execution_path
                            .clone()
                            .unwrap_or_else(|| session.project_path.clone())
                    })
                    .or_else(|| self.execution_root())
                    .unwrap_or_else(|| self.project.clone());
                Source::Local(root)
            };
            let target = key.target.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            self.transcript_media = Some(Preview {
                key,
                window: window_id,
                cancel: cancel.clone(),
                opened: true,
                motion: motion.then(|| WorkMotion::new(0., true)),
                result: None,
                painted: Default::default(),
            });
            let worker_cancel = cancel.clone();
            let job = cx
                .background_executor()
                .spawn(async move { load(source, &target, kind, &worker_cancel) });
            cx.spawn(async move |app, cx| {
                let result = job.await;
                let _ = app.update(cx, |app, cx| {
                    if let Some(preview) = &mut app.transcript_media
                        && Arc::ptr_eq(&preview.cancel, &cancel)
                        && preview.opened
                    {
                        preview.result = Some(result);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        if remote {
            self.remote_workspace.remeasure_media_rows(rows);
        } else if peer.is_none() {
            interaction_ui::remeasure_work_rows(&self.list, rows);
        }
        cx.notify();
    }
}

fn media_caption(text: impl Into<SharedString>) -> Div {
    div()
        .px_2()
        .py_1()
        .text_color(rgb(MUTED))
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::prelude::v1::test;

    #[test]
    fn agent_file_links_keep_their_media_and_literal_text() {
        let fixtures = [
            ("![Test photo](/tmp/shot.jpeg)", Kind::Photo),
            ("[Test book](<file:///tmp/Test%20Book.pdf>)", Kind::Pdf),
            ("Video: [Test clip](clip.mp4)", Kind::Video),
            ("[Test model](/tmp/model.usdz)", Kind::Model),
        ];
        for (markdown, kind) in fixtures {
            let blocks = presentation::message_blocks("assistant", markdown);
            let attachments: Vec<_> = blocks
                .iter()
                .filter_map(|block| match block {
                    MessageBlock::Attachment { target, .. } => Kind::for_target(target),
                    _ => None,
                })
                .collect();
            assert_eq!(attachments, [kind], "{markdown}");
        }
        for text in [
            "`[example](/tmp/shot.png)`",
            "[Website](https://example.com/photo.png)",
            "[Line](/tmp/example.pdf#L2)",
            "[Source](/tmp/example.rs)",
        ] {
            assert!(
                !presentation::message_blocks("assistant", text)
                    .iter()
                    .any(|block| matches!(block, MessageBlock::Attachment { .. })),
                "{text}"
            );
        }
    }

    #[gpui::test]
    fn transcript_photo_disclosure_is_lazy_and_collapsible(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            app.motion = false;
            let session = &app.sessions[0];
            let path = session.project_path.join("fixture.png");
            image::RgbImage::from_pixel(60, 40, image::Rgb([218, 122, 96]))
                .save(&path)
                .unwrap();
            app.active = Some(session.id.clone());
            app.loading = false;
            app.rows = vec![Row {
                role: Some("assistant".into()),
                block: MessageBlock::Attachment {
                    name: "Fixture photo".into(),
                    target: "fixture.png".into(),
                },
            }];
            app.list.reset(1);
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear();
        });
        let toggle = cx
            .debug_bounds("media-toggle-0")
            .expect("collapsed media header");
        assert!(cx.debug_bounds("media-content-0").is_none());
        let collapsed_height = cx.debug_bounds("transcript-row-0").unwrap().size.height;
        cx.update(|_, cx| assert!(view.read(cx).transcript_media.is_none()));

        cx.simulate_click(toggle.center(), Modifiers::none());
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear();
            let preview = view
                .read(cx)
                .transcript_media
                .as_ref()
                .expect("clicked preview");
            assert!(
                preview
                    .result
                    .as_ref()
                    .is_some_and(|r| r.as_ref().is_ok_and(|m| m.photo.is_some()))
            );
        });
        assert!(
            cx.debug_bounds("media-content-0")
                .expect("expanded photo")
                .size
                .height
                > px(48.)
        );

        let toggle = cx
            .debug_bounds("media-toggle-0")
            .expect("expanded media header");
        cx.simulate_click(toggle.center(), Modifiers::none());
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear();
            assert!(
                view.read(cx).transcript_media.is_none(),
                "The second click must release the preview"
            );
        });
        assert_eq!(
            cx.debug_bounds("transcript-row-0").unwrap().size.height,
            collapsed_height
        );
    }
}
