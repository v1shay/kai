//! Composer controls: native dictation and bounded, demand-driven image previews.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A modifier-only chord has no key-down binding. Fire once on its leading
/// edge, in either press order, and rearm only after Fn or Control is released.
pub(super) fn dictation_hotkey(down: &mut bool, modifiers: Modifiers) -> bool {
    let chord = modifiers.function && modifiers.control;
    let trigger = chord && !*down && !modifiers.platform && !modifiers.alt && !modifiers.shift;
    *down = chord;
    trigger
}

fn fade(start: Instant, duration: f32) -> f32 {
    (start.elapsed().as_secs_f32() / duration).min(1.)
}
enum AttachmentSource {
    File(PathBuf),
    Saved(PathBuf),
    Clipboard(Arc<[u8]>),
}
impl Orbit {
    pub(super) fn finish_attachment_removals(&mut self) {
        if self.attachment_removals.is_empty() {
            return;
        }
        self.attachments
            .retain(|a| !self.attachment_removals.contains_key(&a.id));
        self.attachment_hover
            .retain(|id, _| !self.attachment_removals.contains_key(id));
        self.attachment_inserts
            .retain(|id, _| !self.attachment_removals.contains_key(id));
        self.attachment_removals.clear();
    }
    pub(super) fn stop_dictation(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.speech.take() {
            handle.stop();
            let (text, error) = handle.take_pending();
            if let Some(text) = text {
                // Drain the last cumulative callback before releasing the
                // immutable dictation prefix. A stop click must retain words
                // that are queued but have not yet reached the composer.
                self.composer.update(cx, |input, cx| {
                    input.apply_dictation(&text, cx);
                });
            }
            if let Some(error) = error {
                self.notice = Some(error);
            }
            self.speech_generation += 1;
            self.dictation_transition = Some(Instant::now());
        }
        self.dictation_status = None;
        self.composer
            .update(cx, |input, cx| input.end_dictation(cx));
    }
    pub(super) fn toggle_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_voice(cx);
        if self.speech.is_some() {
            self.stop_dictation(cx);
            cx.notify();
            return;
        }
        if self.starting_current() {
            return;
        }
        match speech::SpeechHandle::start() {
            Ok(handle) => {
                self.composer.read(cx).focus_handle.focus(window);
                self.composer
                    .update(cx, |input, cx| input.begin_dictation(cx));
                self.speech_generation += 1;
                let generation = self.speech_generation;
                let events = handle.events.clone();
                self.speech = Some(handle);
                self.dictation_status = Some("Starting dictation…");
                self.dictation_transition = Some(Instant::now());
                cx.spawn(async move |this, cx| {
                    while let Ok(event) = events.recv().await {
                        let keep = this.update(cx, |this, cx| {
                            if this.speech_generation != generation || this.speech.is_none() {
                                return false;
                            }
                            match event {
                                speech::SpeechEvent::Authorizing => {
                                    this.dictation_status =
                                        Some("Allow microphone and speech access…")
                                }
                                speech::SpeechEvent::Listening { on_device } => {
                                    this.dictation_status = Some(if on_device {
                                        "Listening on device"
                                    } else {
                                        "Listening with Apple Speech"
                                    })
                                }
                                speech::SpeechEvent::Transcript { text, .. } => {
                                    let applied = this
                                        .composer
                                        .update(cx, |input, cx| input.apply_dictation(&text, cx));
                                    // Apple finalizes phrases during pauses. The
                                    // same microphone session keeps listening;
                                    // cumulative revisions retain earlier text.
                                    if !applied {
                                        this.stop_dictation(cx);
                                        cx.notify();
                                        return false;
                                    }
                                }
                                speech::SpeechEvent::Error(error) => {
                                    this.notice = Some(error);
                                    this.stop_dictation(cx);
                                    cx.notify();
                                    return false;
                                }
                                speech::SpeechEvent::Stopped => {
                                    this.stop_dictation(cx);
                                    cx.notify();
                                    return false;
                                }
                            }
                            cx.notify();
                            true
                        });
                        if !matches!(keep, Ok(true)) {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(error) => self.notice = Some(error.to_string()),
        }
        cx.notify();
    }
    pub(super) fn reset_attachment_imports(&mut self) {
        if self.attachment_pending > 0
            && let Some(edit) = &mut self.queue_editing
        {
            edit.error = Some("Image restoration was interrupted".into());
        }
        self.attachment_cancel.store(true, Ordering::Release);
        self.attachment_cancel = Arc::new(AtomicBool::new(false));
        self.attachment_generation += 1;
        self.attachment_pending = 0;
        self.attachment_preview_cancel
            .store(true, Ordering::Release);
        unified::hide_inline_preview("attachment");
        self.attachment_preview = None;
        self.attachment_preview_image = None;
        self.attachment_inserts.clear();
        self.attachment_removals.clear();
        self.attachment_hover.clear();
    }
    pub(super) fn add_attachments(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.ninja_surface_owner().is_some(){self.ninja_reference_paths(paths,cx);return;}
        self.import_attachment_sources(paths.into_iter().map(AttachmentSource::File).collect(), cx);
    }
    pub(super) fn add_saved_attachments(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.import_attachment_sources(
            paths.into_iter().map(AttachmentSource::Saved).collect(),
            cx,
        );
    }
    pub(super) fn add_clipboard_image(&mut self, bytes: Arc<[u8]>, cx: &mut Context<Self>) {
        if self.ninja_surface_owner().is_some(){self.ninja_reference_image(bytes,cx);return;}
        self.import_attachment_sources(vec![AttachmentSource::Clipboard(bytes)], cx);
    }
    fn import_attachment_sources(&mut self, paths: Vec<AttachmentSource>, cx: &mut Context<Self>) {
        if self.starting_current() {
            return;
        }
        self.finish_attachment_removals();
        let slots = attachments::MAX_ATTACHMENTS
            .saturating_sub(self.attachments.len() + self.attachment_pending);
        let count = paths.len();
        let mut known: HashSet<_> = self
            .attachment_drafts
            .values()
            .flatten()
            .map(|a| a.id)
            .collect();
        known.extend(self.attachments.iter().map(|a| a.id));
        if count == 0 {
            return;
        }
        if count > slots || known.len() + count > 32 {
            if let Some(edit) = &mut self.queue_editing {
                edit.error = Some("Attachment slots are full".into());
            }
            self.notice = Some(format!(
                "Add up to {} files per prompt. Send or remove existing files to add more.",
                attachments::MAX_ATTACHMENTS
            ));
            cx.notify();
            return;
        }
        self.attachment_pending += count;
        let generation = self.attachment_generation;
        let cancelled = self.attachment_cancel.clone();
        let directory = self.attachment_dir.clone();
        cx.spawn(async move |this, cx| {
            for source in paths {
                if cancelled.load(Ordering::Acquire) {
                    break;
                }
                let directory = directory.clone();
                let cancel = cancelled.clone();
                let source_path = match &source {
                    AttachmentSource::File(path) => Some(path.clone()),
                    _ => None,
                };
                let job = cx.background_executor().spawn(async move {
                    match source {
                        AttachmentSource::File(path) => {
                            attachments::import(&path, &directory, &cancel)
                        }
                        AttachmentSource::Saved(path) => {
                            attachments::load_saved(&path, "Queued image", &directory, &cancel)
                        }
                        AttachmentSource::Clipboard(bytes) => {
                            attachments::import_clipboard(&bytes, &directory, &cancel)
                        }
                    }
                });
                let result = job.await;
                if this
                    .update(cx, |this, cx| {
                        if this.attachment_generation != generation {
                            return;
                        }
                        this.attachment_pending = this.attachment_pending.saturating_sub(1);
                        match result {
                            Ok(attachment) => {
                                if let Some(path) = source_path {
                                    let live: HashSet<_> =
                                        this.attachments.iter().map(|a| a.id).collect();
                                    this.unified_attachment_sources
                                        .retain(|_, id| live.contains(id));
                                    this.unified_attachment_sources.insert(path, attachment.id);
                                }
                                let total = this.attachments.iter().map(|a| a.size).sum::<u64>();
                                let image_bytes = this.attachments.iter().filter(|a| a.is_image()).map(|a| a.size).sum::<u64>();
                                if total.saturating_add(attachment.size) > attachments::MAX_ATTACHMENT_BYTES
                                    || (attachment.is_image() && image_bytes.saturating_add(attachment.size) > attachments::MAX_TOTAL_BYTES) {
                                    if let Some(edit) = &mut this.queue_editing {
                                        edit.error = Some("Attachment budget exceeded".into());
                                    }
                                    this.notice =
                                        Some("Keep files under 256 MB total and images under 32 MB total.".into());
                                } else {
                                    this.attachment_inserts
                                        .insert(attachment.id, Instant::now());
                                    this.attachments.push(attachment);
                                }
                            }
                            Err(error) => {
                                if let Some(edit) = &mut this.queue_editing {
                                    edit.error = Some(error.to_string());
                                }
                                this.notice = Some(error.to_string());
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    cancelled.store(true, Ordering::Release);
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }
    pub(super) fn choose_images(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach files".into()),
        });
        let generation = self.attachment_generation;
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update(cx, |this, cx| {
                    if this.attachment_generation == generation {
                        this.add_attachments(
                            paths
                                .into_iter()
                                .take(attachments::MAX_ATTACHMENTS + 1)
                                .collect(),
                            cx,
                        );
                    }
                });
            }
        })
        .detach();
    }
    fn remove_attachment(&mut self, id: uuid::Uuid, cx: &mut Context<Self>) {
        if self.starting_current() {
            return;
        }
        if self.motion {
            self.attachment_removals
                .entry(id)
                .or_insert_with(Instant::now);
        } else {
            self.attachments.retain(|a| a.id != id);
            self.attachment_hover.remove(&id);
        }
        if self
            .attachment_preview
            .as_ref()
            .is_some_and(|(a, _)| a.id == id)
        {
            self.close_attachment_preview(cx);
        }
        cx.notify();
    }
    fn hover_attachment(&mut self, id: uuid::Uuid, hover: bool, cx: &mut Context<Self>) {
        let from = self
            .attachment_hover
            .get(&id)
            .map(|(t, entered, from)| {
                let p = fade(*t, 0.12);
                if *entered {
                    *from + (1. - *from) * p
                } else {
                    *from * (1. - p)
                }
            })
            .unwrap_or(0.);
        self.attachment_hover
            .insert(id, (Instant::now(), hover, from));
        cx.notify();
    }
    pub(super) fn open_attachment_preview(
        &mut self,
        attachment: attachments::Attachment,
        cx: &mut Context<Self>,
    ) {
        if !attachment.is_image() {
            self.close_attachment_preview(cx);
            if unified::active() {
                self.attachment_preview = Some((attachment, Instant::now()));
                cx.notify();
            }
            return;
        }
        unified::hide_inline_preview("attachment");
        self.attachment_preview_cancel
            .store(true, Ordering::Release);
        self.attachment_preview_cancel = Arc::new(AtomicBool::new(false));
        self.attachment_preview_image = None;
        let id = attachment.id;
        let path = attachment.path.clone();
        let cancel = self.attachment_preview_cancel.clone();
        let completion_cancel = cancel.clone();
        self.attachment_preview = Some((attachment, Instant::now()));
        let job = cx
            .background_executor()
            .spawn(async move { attachments::preview(&path, &cancel) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if completion_cancel.load(Ordering::Acquire)
                    || !Arc::ptr_eq(&completion_cancel, &this.attachment_preview_cancel)
                {
                    return;
                }
                if this
                    .attachment_preview
                    .as_ref()
                    .is_some_and(|(a, _)| a.id == id)
                {
                    match result {
                        Ok(image) => this.attachment_preview_image = Some(image),
                        Err(error) => this.notice = Some(error.to_string()),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn close_attachment_preview(&mut self, cx: &mut Context<Self>) {
        self.attachment_preview_cancel
            .store(true, Ordering::Release);
        unified::hide_inline_preview("attachment");
        self.attachment_preview = None;
        self.attachment_preview_image = None;
        cx.notify();
    }
    pub(super) fn prompt_with_attachments(&self, prompt: &str) -> String {
        let mut text = prompt.to_owned();
        for attachment in &self.attachments {
            // Managed paths are absolute and generated; retain image names as alt text.
            let name = attachment.name.replace(['[', ']', '\n', '\r'], " ");
            text.push_str(&format!(
                "\n\n{}[{name}](<{}>)",
                if attachment.is_image() { "!" } else { "" }, attachment.path.to_string_lossy()
            ));
        }
        text
    }
    pub(super) fn tick_composer_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let enabled=unified::decode_enabled(self.motion&&self.unified_text_morph);
        self.composer.update(cx,|input,_|input.set_reveal_enabled(enabled));
        let mut animate = false;
        let remeasure=!self.work_animations.is_empty()||!self.interaction_animations.is_empty();
        self.interaction_animations.retain(|_, (t, _, _)| {
            let live = self.motion && t.elapsed() < Duration::from_millis(320);
            animate |= live;
            live
        });
        self.work_animations.retain(|_,(t,_,_)|{let live=self.motion&&t.elapsed()<Duration::from_millis(320);animate|=live;live});
        if remeasure{let anchor=self.list.logical_scroll_top();self.list.reset(self.rows.len());let ix=self.flow_index.anchor(anchor.item_ix,&self.expanded_work);self.list.scroll_to(ListOffset{item_ix:ix,offset_in_item:if ix==anchor.item_ix{anchor.offset_in_item}else{px(0.)}});}
        if self.motion
            && self
                .question_choices
                .values()
                .any(|(_, t)| t.elapsed() < Duration::from_millis(280))
        {
            animate = true;
        }

        if let Some(start) = self.dictation_transition {
            if !self.motion || fade(start, 0.32) >= 1. {
                self.dictation_transition = None;
            } else {
                animate = true;
            }
        }
        self.attachment_inserts.retain(|_, start| {
            let live = self.motion && fade(*start, 0.32) < 1.;
            animate |= live;
            live
        });
        let removed: Vec<_> = self
            .attachment_removals
            .iter()
            .filter_map(|(id, start)| {
                if !self.motion || fade(*start, 0.12) >= 1. {
                    Some(*id)
                } else {
                    animate = true;
                    None
                }
            })
            .collect();
        for id in removed {
            self.attachments.retain(|a| a.id != id);
            self.attachment_removals.remove(&id);
            self.attachment_hover.remove(&id);
        }
        if self.motion
            && self
                .attachment_hover
                .values()
                .any(|(start, _, _)| fade(*start, 0.12) < 1.)
        {
            animate = true;
        }
        if self.motion
            && self
                .attachment_preview
                .as_ref()
                .is_some_and(|(_, start)| fade(*start, 0.32) < 1.)
        {
            animate = true;
        }
        if animate {
            request_motion_frame(window, cx);
        }
    }
    pub(super) fn attachment_button(&self, cx: &Context<Self>) -> Stateful<Div> {
        div()
            .id("attach-image")
            .debug_selector(|| "attach-image".into())
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(rgba(GLASS_HOVER)))
            .child(icon("paperclip", 15., MUTED))
            .on_click(cx.listener(|this, _, _, cx| this.choose_images(cx)))
    }
    pub(super) fn dictation_button(&self, peer: Option<&SessionSummary>, cx: &Context<Self>) -> Stateful<Div> {
        let active = peer.is_none() && self.speech.is_some();
        let listening = active
            && self
                .dictation_status
                .is_some_and(|s| s.starts_with("Listening"));
        let progress = self
            .dictation_transition
            .map(|t| presentation::ease_out(fade(t, 0.32)))
            .unwrap_or(1.);
        div()
            .id("dictate")
            .debug_selector(|| "dictate".into())
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .bg(rgba(if listening && !self.theme.borderless_composer { 0x8fc89a20 } else { 0xffffff00 }))
            .hover(|s| s.bg(rgba(GLASS_HOVER)))
            .child(
                div()
                    .relative()
                    .top(px(if self.motion {
                        2. * (1. - progress)
                    } else {
                        0.
                    }))
                    .child(icon(
                        if active { "stop" } else { "mic" },
                        14.,
                        if listening { 0x8fc89a } else { MUTED },
                    )),
            )
            .on_click(Self::chat_control(peer, cx, |this, _, window, cx| this.toggle_dictation(window, cx)))
    }
    pub(super) fn attachment_strip(&self, cx: &Context<Self>) -> AnyElement {
        self.conversation_attachments(None, cx)
    }
    pub(super) fn conversation_attachments(&self, peer: Option<&SessionSummary>, cx: &Context<Self>) -> AnyElement {
        let (attachments, pending) = if let Some(session) = peer {
            (self.attachment_drafts.get(&session.id).map(Vec::as_slice).unwrap_or_default(), 0)
        } else {
            (self.attachments.as_slice(), self.attachment_pending)
        };
        if attachments.is_empty() && pending == 0 {
            return div().into_any_element();
        }
        div()
            .px_3()
            .pb_2()
            .flex()
            .items_center()
            .gap_2()
            .overflow_hidden()
            .children(attachments.iter().map(|attachment| {
                let id = attachment.id;
                let preview = attachment.clone();
                let reveal = if let Some(start) = self.attachment_removals.get(&id) {
                    1. - fade(*start, 0.12)
                } else if let Some(start) = self.attachment_inserts.get(&id) {
                    presentation::ease_out(fade(*start, 0.32))
                } else {
                    1.
                };
                let hover = self
                    .attachment_hover
                    .get(&id)
                    .map(|(t, entered, from)| {
                        let p = if self.motion { fade(*t, 0.12) } else { 1. };
                        if *entered {
                            *from + (1. - *from) * p
                        } else {
                            *from * (1. - p)
                        }
                    })
                    .unwrap_or(0.);
                div()
                    .id(SharedString::from(format!("attachment-{id}")))
                    .debug_selector(move || format!("attachment-{id}"))
                    .w(px(72. * reveal.max(0.)))
                    .h(px(62.))
                    .flex_none()
                    .relative()
                    .rounded_md()
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgba(0xebebeb1a))
                    .bg(rgba(GLASS_SURFACE))
                    .opacity(reveal.clamp(0., 1.))
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, hover: &bool, _, cx| {
                        this.hover_attachment(id, *hover, cx)
                    }))
                    .when_some(attachment.thumbnail.clone(), |s, image| s.child(
                        img(image).w(px(72.)).h(px(62.)).object_fit(ObjectFit::Cover),
                    ))
                    .when(!attachment.is_image(), |s| s.child(div().size_full().flex().flex_col()
                        .items_center().justify_center().gap_1().px_1()
                        .child(icon(files::icon_for_path(&attachment.path), 18., MUTED))
                        .child(div().max_w_full().truncate().text_size(theme::ui_px(9.)).child(attachment.name.clone()))))
                    .child(
                        div()
                            .id(SharedString::from(format!("remove-attachment-{id}")))
                            .debug_selector(move || format!("remove-attachment-{id}"))
                            .absolute()
                            .right(px(3.))
                            .top(px(3.))
                            .size(px(20.))
                            .rounded_full()
                            .bg(rgba(0x242424bb))
                            .flex()
                            .items_center()
                            .justify_center()
                            .opacity(hover)
                            .child(icon("close", 11., TEXT))
                            .on_click(Self::chat_control(peer, cx, move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.remove_attachment(id, cx);
                            })),
                    )
                    .on_click(Self::chat_control(peer, cx, move |this, _, _, cx| {
                        this.open_attachment_preview(preview.clone(), cx)
                    }))
            }))
            .when(pending > 0, |s| {
                s.child(
                    div()
                        .text_size(crate::theme::ui_px(11.))
                        .text_color(rgb(MUTED))
                        .child("Preparing images…"),
                )
            })
            .into_any_element()
    }
    pub(super) fn attachment_preview_view(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some((attachment, start)) = &self.attachment_preview else {
            return div().into_any_element();
        };
        let reveal = if self.motion {
            presentation::ease_out(fade(*start, 0.32)).clamp(0., 1.)
        } else {
            1.
        };
        anchored()
            .anchor(Corner::BottomLeft)
            .position(point(
                px(self.sidebar_width + 268.),
                window.viewport_size().height - px(136. + 8. * (1. - reveal)),
            ))
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .id("attachment-preview")
                    .debug_selector(|| "attachment-preview".into())
                    .max_h(window.viewport_size().height - px(16.))
                    .w(px(400.))
                    .p_2()
                    .rounded_lg()
                    .bg(rgba(0x242424bb))
                    .border_1()
                    .border_color(rgba(0xebebeb20))
                    .opacity(reveal)
                    .occlude()
                    .on_mouse_down_out(
                        cx.listener(|this, _, _, cx| this.close_attachment_preview(cx)),
                    )
                    .child(
                        div()
                            .h(px(30.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon("file-image", 14., MUTED))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(crate::theme::ui_px(12.))
                                    .text_ellipsis()
                                    .child(attachment.name.clone()),
                            )
                            .child({
                                let path = attachment.path.clone();
                                div().id("copy-preview-attachment").p_1().rounded_md().cursor_pointer()
                                    .child("Copy file").on_click(move |_, _, _| unified::copy_file(path.clone()))
                            })
                            .child({let path=attachment.path.clone();div().id("native-quick-look").p_1().rounded_md().cursor_pointer().child("Quick Look").on_click(move|_,_,_|unified::quick_look(path.clone()))})
                            .child(
                                div()
                                    .id("close-image-preview")
                                    .debug_selector(|| "close-image-preview".into())
                                    .size(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .child(icon("close", 12., MUTED))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_attachment_preview(cx)
                                    })),
                            ),
                    )
                    .when(!attachment.is_image(), |s| s.child(
                        unified::inline_preview("attachment", &attachment.path)
                            .w_full().h(px(280.).min(window.viewport_size().height - px(68.))),
                    ))
                    .when_some(self.attachment_preview_image.clone().or_else(|| attachment.thumbnail.clone()), |s, image| s.child(
                        img(image).w_full().h(px(300.).min(window.viewport_size().height - px(68.)))
                            .object_fit(ObjectFit::Contain),
                    )),
            )
            .into_any_element()
    }
}
impl Drop for Orbit {
    fn drop(&mut self) {
        self.attachment_cancel.store(true, Ordering::Release);
        self.attachment_preview_cancel
            .store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn fn_control_dictation_fires_once_in_either_order_and_ignores_other_chords() {
        let chord = Modifiers {
            function: true,
            control: true,
            ..Modifiers::none()
        };
        for first in [
            Modifiers {
                function: true,
                ..Modifiers::none()
            },
            Modifiers {
                control: true,
                ..Modifiers::none()
            },
        ] {
            let mut down = false;
            assert!(!dictation_hotkey(&mut down, first));
            assert!(dictation_hotkey(&mut down, chord));
            assert!(!dictation_hotkey(&mut down, chord));
            // Adding/removing another modifier while held must not toggle again.
            assert!(!dictation_hotkey(
                &mut down,
                Modifiers {
                    shift: true,
                    ..chord
                }
            ));
            assert!(!dictation_hotkey(&mut down, chord));
            assert!(!dictation_hotkey(&mut down, first));
            assert!(dictation_hotkey(&mut down, chord));
        }
        for extra in [
            Modifiers {
                shift: true,
                ..chord
            },
            Modifiers {
                platform: true,
                ..chord
            },
            Modifiers { alt: true, ..chord },
        ] {
            let mut down = false;
            assert!(!dictation_hotkey(&mut down, extra));
            assert!(!dictation_hotkey(&mut down, chord));
            assert!(!dictation_hotkey(&mut down, Modifiers::none()));
            assert!(dictation_hotkey(&mut down, chord));
        }
    }

    fn clipboard_png() -> Vec<u8> {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &[120, 40, 80, 255].repeat(4),
                2,
                2,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        bytes
    }

    #[gpui::test]
    fn composer_image_paste_reuses_previews_and_removal_without_touching_text(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.simulate_input("Keep typed words 🦀");
        cx.write_to_clipboard(ClipboardItem::new_image(&gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            clipboard_png(),
        )));
        cx.simulate_keystrokes("cmd-v");
        cx.run_until_parked();
        let item = cx.update(|_, cx| {
            let app = view.read(cx);
            assert_eq!(app.composer.read(cx).text(), "Keep typed words 🦀");
            assert_eq!(app.attachments.len(), 1);
            assert_eq!(app.attachment_pending, 0);
            assert!(app.runs.is_empty());
            app.attachments[0].clone()
        });
        view.update(cx, |app, cx| app.open_attachment_preview(item.clone(), cx));
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("attachment-preview").is_some());
        view.update(cx, |app, cx| app.remove_attachment(item.id, cx));
        cx.update(|_, cx| assert!(view.read(cx).attachments.is_empty()));
        assert!(
            item.path.exists(),
            "An open preview/temporary owner retains the imported file until released"
        );
    }

    #[gpui::test]
    fn image_paste_failure_preserves_composer_and_text_paste_stays_native(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.simulate_input("Preserve this");
        cx.write_to_clipboard(ClipboardItem::new_image(&gpui::Image::from_bytes(
            gpui::ImageFormat::Svg,
            b"<svg></svg>".to_vec(),
        )));
        cx.simulate_keystrokes("cmd-v");
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert_eq!(app.composer.read(cx).text(), "Preserve this");
            assert!(app.attachments.is_empty());
            assert!(
                app.notice
                    .as_deref()
                    .is_some_and(|notice| notice.contains("Paste a PNG"))
            );
        });
        cx.write_to_clipboard(ClipboardItem::new_string("\nPlain 日本語".into()));
        cx.simulate_keystrokes("cmd-v");
        cx.update(|_, cx| {
            assert_eq!(
                view.read(cx).composer.read(cx).text(),
                "Preserve this\nPlain 日本語"
            )
        });
    }
}
