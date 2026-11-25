//! Explicit Orbit-owned image lifetime. GPU atlas entries outlive CPU Arcs unless
//! App::drop_image is called; model ownership, not Arc counts, decides retirement.
use super::*;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct OwnedImages {
    images: HashMap<gpui::ImageId, (Arc<RenderImage>, bool)>,
    generation: bool,
}
impl OwnedImages {
    fn begin(&mut self) {
        self.generation = !self.generation;
    }
    fn mark(&mut self, image: &Arc<RenderImage>) {
        self.images
            .entry(image.id)
            .and_modify(|(_, seen)| *seen = self.generation)
            .or_insert_with(|| (image.clone(), self.generation));
    }
    fn finish(&mut self, mut window: Option<&mut Window>, cx: &mut App) {
        self.images.retain(|_, (image, seen)| {
            if *seen == self.generation {
                true
            } else {
                cx.drop_image(image.clone(), window.as_deref_mut());
                false
            }
        });
    }
    fn release(&mut self, cx: &mut App) {
        for (_, (image, _)) in self.images.drain() {
            cx.drop_image(image, None);
        }
    }
}
impl Orbit {
    fn visit_owned_render_images(&self, mut add: impl FnMut(&Arc<RenderImage>)) {
        let mut attachment = |a: &attachments::Attachment| {
            if let Some(image) = &a.thumbnail {
                add(image);
            }
        };
        for a in &self.attachments {
            attachment(a);
        }
        for draft in self.attachment_drafts.values() {
            for a in draft {
                attachment(a);
            }
        }
        for run in self.runs.values() {
            for a in &run.submitted_attachments {
                attachment(a);
            }
        }
        if let Some((a, _)) = &self.attachment_preview {
            attachment(a);
        }
        if let Some(submission) = &self.send_after_load {
            for a in &submission.attachments {
                attachment(a);
            }
        }
        if let Some(pending) = &self.handoff_pending {
            for a in pending.image_attachments() {
                attachment(a);
            }
        }
        // Queued prompts and prepared handoff packets own paths/text, not images.
        if let Some(image) = &self.attachment_preview_image {
            add(image);
        }
        for artifact in self
            .ninjas
            .artifacts
            .values()
            .filter_map(|result| result.as_ref().ok())
        {
            if let Some(image) = &artifact.image {
                add(image);
            }
        }
    }
    fn reconcile_render_images(&mut self, window: Option<&mut Window>, cx: &mut App) {
        // Move the existing allocation out to scan all owners without borrowing
        // the registry through self. Unchanged images keep their existing Arc;
        // only newly seen IDs allocate. No mutation-specific dirty flag is needed.
        let mut images = std::mem::take(&mut self.render_images);
        images.begin();
        self.visit_owned_render_images(|image| images.mark(image));
        images.finish(window, cx);
        self.render_images = images;
    }
    pub(super) fn retire_unowned_render_images(&mut self, cx: &mut Context<Self>) {
        self.reconcile_render_images(None, cx);
    }
    pub(super) fn retire_unowned_render_images_in_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reconcile_render_images(Some(window), cx);
    }
    pub(super) fn release_owned_render_images(&mut self, cx: &mut App) {
        let mut images = std::mem::take(&mut self.render_images);
        // Include changes not yet delivered by the notification observer.
        self.visit_owned_render_images(|image| images.mark(image));
        images.release(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use std::prelude::v1::test;
    fn paint(images: &[Arc<RenderImage>], cx: &mut VisualTestContext) {
        let images = images.to_vec();
        cx.draw(Point::default(), size(px(128.), px(128.)), move |_, _| {
            div()
                .flex()
                .children(images.into_iter().map(|image| img(image).size(px(8.))))
                .into_any_element()
        });
    }
    fn uploaded(ids: &HashSet<gpui::ImageId>, cx: &mut VisualTestContext) -> usize {
        cx.update(|window, _| {
            window
                .image_atlas_stats_for_test()
                .0
                .iter()
                .filter(|id| ids.contains(id))
                .count()
        })
    }
    #[gpui::test]
    fn painted_ninja_images_retire_after_eight_entry_owner_eviction(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let mut all = HashSet::new();
        // 72 unique768² buffers, real GPUI paint/upload keys, eight retained.
        for batch in 0..9 {
            let images = (0..8)
                .map(|_| {
                    Arc::new(RenderImage::new(vec![image::Frame::new(
                        image::RgbaImage::new(768, 768),
                    )]))
                })
                .collect::<Vec<_>>();
            all.extend(images.iter().map(|image| image.id));
            view.update(cx, |app, cx| {
                app.ninjas.artifacts = images
                    .iter()
                    .enumerate()
                    .map(|(n, image)| {
                        (
                            format!("batch-{batch}-{n}"),
                            Ok(crate::ninja_chat::Artifact {
                                path: PathBuf::from("/unused-test-preview"),
                                image: Some(image.clone()),
                                native_preview: None,
                            }),
                        )
                    })
                    .collect();
                cx.notify();
            });
            cx.run_until_parked();
            paint(&images, cx);
            assert_eq!(
                uploaded(&all, cx),
                8,
                "previous batches remained in the sprite atlas at batch {batch}"
            );
        }
        view.update(cx, |app, cx| {
            app.ninjas.artifacts.clear();
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(uploaded(&all, cx), 0);
    }
    #[gpui::test]
    fn shared_draft_and_full_preview_keep_their_upload_until_last_owner_retires(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(32, 32))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let item = view.update(cx, |app, _| {
            attachments::import_clipboard(
                bytes.get_ref(),
                &app.attachment_dir,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap()
        });
        let thumbnail = item.thumbnail.clone().unwrap();
        let full = Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::new(768, 768),
        )]));
        let ids = HashSet::from([thumbnail.id, full.id]);
        view.update(cx, |app, cx| {
            app.attachments = vec![item.clone()];
            app.attachment_drafts
                .insert("inactive-draft".into(), vec![item.clone()]);
            app.attachment_preview = Some((item, Instant::now()));
            app.attachment_preview_image = Some(full.clone());
            cx.notify();
        });
        cx.run_until_parked();
        paint(&[thumbnail.clone(), full.clone()], cx);
        assert_eq!(uploaded(&ids, cx), 2);
        let main_window = cx.update(|window, _| window.window_handle());
        let state = view.clone();
        let (quick, cx) = cx.add_window_view(move |window, cx| {
            crate::quick_chat::QuickChat::new(state, window, cx)
        });
        paint(&[thumbnail.clone(), full], cx);
        assert_eq!(uploaded(&ids, cx), 2);
        view.update(cx, |app, cx| {
            app.attachments.clear();
            app.close_attachment_preview(cx);
        });
        cx.run_until_parked();
        assert_eq!(
            uploaded(&ids, cx),
            1,
            "inactive draft lost its shared thumbnail or closed full preview leaked"
        );
        assert_eq!(
            main_window
                .update(cx, |_, window, _| window
                    .image_atlas_stats_for_test()
                    .0
                    .iter()
                    .filter(|id| ids.contains(id))
                    .count())
                .unwrap(),
            1,
            "main atlas must preserve the same inactive-draft owner"
        );
        view.update(cx, |app, cx| {
            app.attachment_drafts.clear();
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(uploaded(&ids, cx), 0);
        assert_eq!(
            main_window
                .update(cx, |_, window, _| window
                    .image_atlas_stats_for_test()
                    .0
                    .iter()
                    .filter(|id| ids.contains(id))
                    .count())
                .unwrap(),
            0,
            "last owner retirement must clear both windows"
        );
        cx.draw(Point::default(), size(px(680.), px(520.)), |_, _| {
            quick.clone().into_any_element()
        });
        main_window
            .update(cx, |_, window, cx| {
                window.draw(cx).clear();
            })
            .unwrap();
    }
    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    #[gpui::test]
    fn entity_release_retires_images_after_the_window_lease_is_restored(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::new(768, 768),
        )]));
        let ids = HashSet::from([image.id]);
        view.update(cx, |app, cx| {
            app.ninjas.artifacts.insert(
                "release-preview".into(),
                Ok(crate::ninja_chat::Artifact {
                    path: PathBuf::from("/unused-test-preview"),
                    image: Some(image.clone()),
                    native_preview: None,
                }),
            );
            cx.notify();
        });
        cx.run_until_parked();
        paint(&[image], cx);
        assert_eq!(uploaded(&ids, cx), 1);
        let weak = view.downgrade();
        cx.update(|window, cx| {
            window.replace_root(cx, |_, _| Empty);
            drop(view);
        });
        cx.run_until_parked();
        assert!(
            weak.upgrade().is_none(),
            "fixture retained the Orbit entity unexpectedly"
        );
        assert_eq!(
            uploaded(&ids, cx),
            0,
            "release skipped the formerly leased window atlas"
        );
    }
}
