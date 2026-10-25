// SPDX-License-Identifier: Apache-2.0
// Input handling adapted from Zed Industries' GPUI 0.2.2 examples/input.rs.
// Copyright Zed Industries, Inc. https://github.com/zed-industries/zed
// Orbit adds multiline layout, scoped bindings, submission events, and IME fixes.

use std::ops::Range;

use crate::theme::{rgb, rgba};
use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, ScrollHandle, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle,
    Window, WrappedLine, actions, div, fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::*;

/// Bound text shaping and replacement work, including large clipboard pastes.
pub const MAX_DRAFT_BYTES: usize = 64 * 1024;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        Up,
        Down,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Submit,
        Newline,
        CompleteCommand,
        SaveFile,
        Undo,
        Redo,
    ]
);

pub struct TextInput {
    pub focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Vec<WrappedLine>,
    last_line_height: Pixels,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    dictation: Option<DictationTail>,
    mirrored_dictation: bool,
    command_navigation: bool,
    scroll: ScrollHandle,
    secret: Option<SecretLayout>,
    image_paste: bool,
    editor_mode: bool,
    history: EditHistory,
}

#[derive(Clone)]
struct EditState {
    text: SharedString,
    selection: Range<usize>,
}
#[derive(Default)]
struct EditHistory {
    undo: std::collections::VecDeque<EditState>,
    redo: std::collections::VecDeque<EditState>,
    last_edit: Option<std::time::Instant>,
}
impl EditHistory {
    fn trim(&mut self) {
        const BUDGET: usize = 2 * 1024 * 1024;
        let mut bytes = self
            .undo
            .iter()
            .chain(&self.redo)
            .map(|s| s.text.len())
            .sum::<usize>();
        while self.undo.len() + self.redo.len() > 128 || bytes > BUDGET {
            let removed = if self.undo.len() > 1 {
                self.undo.pop_front()
            } else {
                self.redo.pop_front()
            };
            let Some(removed) = removed else {
                break;
            };
            bytes = bytes.saturating_sub(removed.text.len());
        }
    }
}

/// One visible bullet per extended grapheme; source byte offsets remain the
/// canonical editable/IME representation. The normal composer allocates no map.
#[derive(Default)]
struct SecretLayout {
    text: SharedString,
    boundaries: Vec<usize>,
}
impl SecretLayout {
    fn new(source: &str) -> Self {
        let mut boundaries: Vec<_> = source
            .grapheme_indices(true)
            .map(|(offset, _)| offset)
            .collect();
        let text = "•".repeat(boundaries.len()).into();
        boundaries.push(source.len());
        Self { text, boundaries }
    }
    fn to_display(&self, source: usize) -> usize {
        self.boundaries
            .partition_point(|offset| *offset <= source)
            .saturating_sub(1)
            * "•".len()
    }
    fn to_source(&self, display: usize) -> usize {
        self.boundaries[(display / "•".len()).min(self.boundaries.len() - 1)]
    }
}

/// Spoken snapshots have their own cursor, independent of editable content.
/// Manual deletion never moves this cursor backwards or resurrects old words.
#[derive(Default)]
struct DictationTail {
    consumed: String,
}
impl DictationTail {
    fn appended(&mut self, content: &str, text: &str) -> Option<String> {
        let Some(delta) = text.strip_prefix(&self.consumed) else {
            // Shorter/stale/revised snapshots cannot delete or replace text.
            return Some(content.to_owned());
        };
        if delta.is_empty() {
            return Some(content.to_owned());
        }
        let separator = !content.is_empty()
            && !content.ends_with(char::is_whitespace)
            && !delta.starts_with(char::is_whitespace);
        let len = content
            .len()
            .saturating_add(delta.len())
            .saturating_add(usize::from(separator));
        if len > MAX_DRAFT_BYTES {
            return None;
        }
        let mut appended = String::with_capacity(len);
        appended.push_str(content);
        if separator {
            appended.push(' ');
        }
        appended.push_str(delta);
        self.consumed.clear();
        self.consumed.push_str(text);
        Some(appended)
    }
}

#[derive(Clone, Debug)]
pub enum InputEvent {
    Submit,
    Save,
    Changed,
    LimitReached,
    DictationCancelled,
    CommandPrevious,
    CommandNext,
    CommandAccept,
    CommandComplete,
    CursorChanged,
    ImagePasted(std::sync::Arc<[u8]>),
    ClipboardError(String),
}
impl gpui::EventEmitter<InputEvent> for TextInput {}

impl TextInput {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: "Ask anything, or give your agent a task…".into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: vec![],
            last_line_height: px(24.),
            last_bounds: None,
            is_selecting: false,
            dictation: None,
            mirrored_dictation: false,
            command_navigation: false,
            scroll: ScrollHandle::new(),
            secret: None,
            image_paste: false,
            editor_mode: false,
            history: EditHistory::default(),
        }
    }

    /// Password-style request fields never expose their text to clipboard,
    /// shaping, command discovery or macOS text retrieval services. They must
    /// remain ephemeral request inputs; callers must not save them as drafts.
    pub fn set_secret(&mut self, secret: bool, cx: &mut Context<Self>) {
        self.cancel_dictation(cx);
        self.history = EditHistory::default();
        self.secret = secret.then(|| SecretLayout::new(&self.content));
        self.command_navigation = false;
        self.last_layout.clear();
        cx.notify();
    }
    fn refresh_secret(&mut self) {
        if self.secret.is_some() {
            self.secret = Some(SecretLayout::new(&self.content));
        }
    }
    fn display_offset(&self, source: usize) -> usize {
        self.secret
            .as_ref()
            .map_or(source, |secret| secret.to_display(source))
    }
    fn source_offset(&self, display: usize) -> usize {
        self.secret
            .as_ref()
            .map_or(display.min(self.content.len()), |secret| {
                secret.to_source(display)
            })
    }

    pub fn text(&self) -> String {
        self.content.to_string()
    }
    pub fn set_editor_mode(&mut self) {
        self.editor_mode = true;
    }
    fn remember_edit(&mut self) {
        if self.secret.is_some() {
            return;
        }
        let now = std::time::Instant::now();
        if self
            .history
            .last_edit
            .is_none_or(|last| now.duration_since(last).as_millis() > 350)
        {
            self.history.undo.push_back(EditState {
                text: self.content.clone(),
                selection: self.selected_range.clone(),
            });
        }
        self.history.redo.clear();
        self.history.last_edit = Some(now);
        self.history.trim();
    }
    pub fn undo_edit(&mut self, cx: &mut Context<Self>) {
        self.replay_edit(false, cx);
    }
    pub fn redo_edit(&mut self, cx: &mut Context<Self>) {
        self.replay_edit(true, cx);
    }
    fn replay_edit(&mut self, redo: bool, cx: &mut Context<Self>) {
        if self.secret.is_some() {
            return;
        }
        let next = if redo {
            self.history.redo.pop_back()
        } else {
            self.history.undo.pop_back()
        };
        let Some(next) = next else {
            return;
        };
        self.cancel_dictation(cx);
        let current = EditState {
            text: self.content.clone(),
            selection: self.selected_range.clone(),
        };
        if redo {
            self.history.undo.push_back(current);
        } else {
            self.history.redo.push_back(current);
        }
        self.history.trim();
        self.history.last_edit = None;
        self.content = next.text;
        self.selected_range = next.selection;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout.clear();
        cx.emit(InputEvent::Changed);
        cx.notify();
    }
    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.undo_edit(cx);
    }
    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.redo_edit(cx);
    }
    /// Only the prompt composer accepts raster clipboard data. Search, names
    /// and secret/request inputs keep their existing text-only paste behavior.
    pub fn set_image_paste(&mut self, enabled: bool) {
        self.image_paste = enabled && self.secret.is_none();
    }
    pub fn command_query(&self) -> Option<&str> {
        if self.secret.is_some()
            || self.marked_range.is_some()
            || !self.selected_range.is_empty()
            || self.cursor_offset() != self.content.len()
        {
            return None;
        }
        crate::commands::query(&self.content)
    }
    pub fn set_command_navigation(&mut self, active: bool) {
        self.command_navigation = active;
    }
    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.last_bounds
    }
    #[cfg(test)]
    pub(crate) fn rendered_line_height(&self) -> Pixels {
        self.last_line_height
    }

    #[cfg(test)]
    pub(crate) fn scroll_position(&self) -> Point<Pixels> {
        self.scroll.offset()
    }
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.cancel_dictation(cx);
        if text.len() > MAX_DRAFT_BYTES {
            cx.emit(InputEvent::LimitReached);
            return;
        }
        self.content = text.to_string().into();
        self.history = EditHistory::default();
        self.refresh_secret();
        self.selected_range = text.len()..text.len();
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout.clear();
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    pub fn is_dictating(&self) -> bool {
        self.dictation.is_some()
    }
    /// Mirror the main draft's dictation follow behavior with independent geometry.
    pub fn follow_mirrored_dictation(&mut self, follow: bool, cx: &mut Context<Self>) {
        if self.mirrored_dictation != follow {
            self.mirrored_dictation = follow;
            cx.notify();
        }
    }
    pub fn begin_dictation(&mut self, cx: &mut Context<Self>) {
        if self.secret.is_some() {
            return;
        }
        self.dictation = Some(DictationTail::default());
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        self.is_selecting = false;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Returns false if the session was cancelled or the complete revision cannot
    /// fit. Never selects/marks dictated text or slices a Unicode character.
    pub fn apply_dictation(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let Some(tail) = &mut self.dictation else {
            return false;
        };
        if text.len() <= tail.consumed.len() || !text.starts_with(&tail.consumed) {
            // Nothing new: avoid copying/shaping a potentially large draft.
            return true;
        }
        let Some(revised) = tail.appended(&self.content, text) else {
            cx.emit(InputEvent::LimitReached);
            self.cancel_dictation(cx);
            return false;
        };
        if revised == self.content.as_ref() {
            return true;
        }
        self.remember_edit();
        self.content = revised.into();
        self.refresh_secret();
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        self.is_selecting = false;
        // GPUI resolves this after measuring the new text, so even a large
        // partial revision follows the new final line in the same frame.
        self.scroll.scroll_to_bottom();
        cx.emit(InputEvent::Changed);
        cx.notify();
        true
    }

    pub fn end_dictation(&mut self, cx: &mut Context<Self>) {
        if self.dictation.take().is_some() {
            cx.notify();
        }
    }

    fn cancel_dictation(&mut self, cx: &mut Context<Self>) {
        if self.dictation.take().is_some() {
            cx.emit(InputEvent::DictationCancelled);
        }
    }

    pub fn set_placeholder(&mut self, text: &str, cx: &mut Context<Self>) {
        self.placeholder = text.to_owned().into();
        cx.notify();
    }

    fn save_file(&mut self, _: &SaveFile, _: &mut Window, cx: &mut Context<Self>) {
        if self.editor_mode {
            cx.emit(InputEvent::Save);
        } else {
            cx.propagate();
        }
    }
    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_mode && self.marked_range.is_none() {
            self.newline(&Newline, window, cx);
            return;
        }
        if self.marked_range.is_none() {
            cx.emit(
                if self.command_navigation && self.command_query().is_some() {
                    InputEvent::CommandAccept
                } else {
                    InputEvent::Submit
                },
            );
        }
    }
    fn complete_command(
        &mut self,
        _: &CompleteCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor_mode {
            self.replace_text_in_range(None, "\t", window, cx);
            return;
        }
        if self.command_navigation && self.command_query().is_some() {
            cx.emit(InputEvent::CommandComplete);
        } else {
            cx.propagate();
        }
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn vertical(&mut self, direction: f32, select: bool, cx: &mut Context<Self>) {
        if let Some(mut position) = self.position_for_offset(self.cursor_offset()) {
            position.y += self.last_line_height * direction + px(1.);
            let offset = self.index_for_mouse_position(position);
            if select {
                self.select_to(offset, cx);
            } else {
                self.move_to(offset, cx);
            }
        }
    }
    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.command_navigation && self.command_query().is_some() {
            cx.emit(InputEvent::CommandPrevious);
            return;
        }
        self.vertical(-1., false, cx);
    }
    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.command_navigation && self.command_query().is_some() {
            cx.emit(InputEvent::CommandNext);
            return;
        }
        self.vertical(1., false, cx);
    }
    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1., true, cx);
    }
    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1., true, cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        self.is_selecting = true;

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.image_paste && self.secret.is_none() {
            match crate::attachments::clipboard_image(cx) {
                Ok(Some(bytes)) => {
                    cx.emit(InputEvent::ImagePasted(bytes));
                    return;
                }
                Err(error) => {
                    cx.emit(InputEvent::ClipboardError(error));
                    return;
                }
                Ok(None) => {}
            }
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // Refuse oversized raw pastes before allocating a normalized copy.
            if !replacement_fits(
                self.content.len(),
                self.marked_range
                    .clone()
                    .unwrap_or_else(|| self.selected_range.clone()),
                text.len(),
            ) {
                cx.emit(InputEvent::LimitReached);
                return;
            }
            if text.contains("\r\n") {
                self.replace_text_in_range(None, &text.replace("\r\n", "\n"), window, cx);
            } else {
                self.replace_text_in_range(None, &text, window, cx);
            }
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.secret.is_some() {
            cx.stop_propagation();
            return;
        }
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.secret.is_some() {
            cx.stop_propagation();
            return;
        }
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        cx.emit(InputEvent::CursorChanged);
        self.selection_reversed = false;
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let Some(bounds) = self.last_bounds else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        let mut origin = bounds.origin;
        let mut offset = 0;
        for line in &self.last_layout {
            let height = line.size(self.last_line_height).height;
            if position.y < origin.y + height {
                let ix = line
                    .closest_index_for_position(position - origin, self.last_line_height)
                    .unwrap_or_else(|ix| ix);
                return self.source_offset(offset + ix);
            }
            origin.y += height;
            offset += line.len() + 1;
        }
        self.content.len()
    }

    fn position_for_offset(&self, index: usize) -> Option<Point<Pixels>> {
        let index = self.display_offset(index);
        let mut origin = self.last_bounds?.origin;
        let mut offset = 0;
        for line in &self.last_layout {
            if index <= offset + line.len() {
                return Some(
                    origin
                        + line.position_for_index(
                            index.saturating_sub(offset),
                            self.last_line_height,
                        )?,
                );
            }
            offset += line.len() + 1;
            origin.y += line.size(self.last_line_height).height;
        }
        Some(origin)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        cx.emit(InputEvent::CursorChanged);
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        if self.secret.is_some() {
            *actual_range = None;
            return None;
        }
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
        _cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        if !replacement_fits(self.content.len(), range.clone(), new_text.len()) {
            cx.emit(InputEvent::LimitReached);
            return;
        }

        self.remember_edit();
        self.content = replace_content(&self.content, range.clone(), new_text);
        self.refresh_secret();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        self.selection_reversed = false;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        if !replacement_fits(self.content.len(), range.clone(), new_text.len()) {
            cx.emit(InputEvent::LimitReached);
            return;
        }

        self.remember_edit();
        self.content = replace_content(&self.content, range.clone(), new_text);
        self.refresh_secret();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .map(|selection| {
                let start = utf16_to_utf8(new_text, selection.start);
                let end = utf16_to_utf8(new_text, selection.end);
                range.start + start..range.start + end
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let start = self.position_for_offset(range.start)?;
        let end = self.position_for_offset(range.end)?;
        let _ = bounds;
        Some(Bounds::from_corners(
            start,
            point(end.x.max(start.x + px(1.)), end.y + self.last_line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_for_mouse_position(point)))
    }
}

fn replace_content(content: &str, replaced: Range<usize>, inserted: &str) -> SharedString {
    let mut result = String::with_capacity(content.len() - replaced.len() + inserted.len());
    result.push_str(&content[..replaced.start]);
    result.push_str(inserted);
    result.push_str(&content[replaced.end..]);
    result.into()
}

fn replacement_fits(current: usize, replaced: Range<usize>, inserted: usize) -> bool {
    current
        .saturating_sub(replaced.len())
        .saturating_add(inserted)
        <= MAX_DRAFT_BYTES
}

fn utf16_to_utf8(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        if units >= offset {
            return index;
        }
        units += ch.len_utf16();
    }
    text.len()
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    lines: Vec<WrappedLine>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

fn shape_input(
    input: &TextInput,
    width: Pixels,
    window: &Window,
    style: &gpui::TextStyle,
) -> Vec<WrappedLine> {
    let (text, color) = if input.content.is_empty() {
        (input.placeholder.clone(), rgb(0x929292).into())
    } else {
        (
            input
                .secret
                .as_ref()
                .map_or_else(|| input.content.clone(), |secret| secret.text.clone()),
            style.color,
        )
    };
    let run = TextRun {
        len: text.len(),
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let runs = if let Some(marked) = &input.marked_range {
        let marked = input.display_offset(marked.start)..input.display_offset(marked.end);
        vec![
            TextRun {
                len: marked.start,
                ..run.clone()
            },
            TextRun {
                len: marked.len(),
                underline: Some(UnderlineStyle {
                    color: Some(color),
                    thickness: px(1.),
                    wavy: false,
                }),
                ..run.clone()
            },
            TextRun {
                len: text.len().saturating_sub(marked.end),
                ..run
            },
        ]
    } else {
        vec![run]
    };
    window
        .text_system()
        .shape_text(
            text,
            style.font_size.to_pixels(window.rem_size()),
            &runs,
            Some(width.max(px(1.))),
            None,
        )
        .map(|lines| lines.into_iter().collect())
        .unwrap_or_default()
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let entity = self.input.clone();
        let text_style = _window.text_style();
        let line_height = _window.line_height();
        let layout = _window.request_measured_layout(style, move |known, available, window, cx| {
            let width = known.width.unwrap_or_else(|| match available.width {
                AvailableSpace::Definite(width) => width,
                _ => px(500.),
            });
            let lines = shape_input(entity.read(cx), width, window, &text_style);
            let height = lines
                .iter()
                .map(|line| line.size(line_height).height)
                .fold(px(0.), |sum, value| sum + value)
                .max(line_height);
            size(width, height)
        });
        (layout, ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PrepaintState {
        let input = self.input.read(cx);
        let lines = shape_input(input, bounds.size.width, window, &window.text_style());
        let height = window.line_height();
        let mut origin = bounds.origin;
        let mut offset = 0;
        let selected_range = input.display_offset(input.selected_range.start)
            ..input.display_offset(input.selected_range.end);
        let cursor_offset = input.display_offset(input.cursor_offset());
        let mut cursor = None;
        let mut selections = vec![];
        for line in &lines {
            let end = offset + line.len();
            if selected_range.is_empty() && cursor_offset >= offset && cursor_offset <= end {
                if let Some(position) = line.position_for_index(cursor_offset - offset, height) {
                    cursor = Some(fill(
                        Bounds::new(origin + position, size(px(1.5), height)),
                        rgb(0x459bf7),
                    ));
                }
            } else if selected_range.start <= end && selected_range.end > offset {
                let start = selected_range.start.saturating_sub(offset).min(line.len());
                let stop = selected_range.end.saturating_sub(offset).min(line.len());
                if let (Some(a), Some(b)) = (
                    line.position_for_index(start, height),
                    line.position_for_index(stop, height),
                ) {
                    let mut row_y = a.y;
                    while row_y <= b.y {
                        let x1 = if row_y == a.y { a.x } else { px(0.) };
                        let x2 = if row_y == b.y { b.x } else { bounds.size.width };
                        if x2 > x1 {
                            selections.push(fill(
                                Bounds::new(origin + point(x1, row_y), size(x2 - x1, height)),
                                rgba(0x459bf73a),
                            ));
                        }
                        row_y += height;
                    }
                }
            }
            offset = end + 1;
            origin.y += line.size(height).height;
        }
        PrepaintState {
            lines,
            cursor,
            selections,
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for selection in state.selections.drain(..) {
            window.paint_quad(selection);
        }
        let mut origin = bounds.origin;
        let height = window.line_height();
        for line in &state.lines {
            let _ = line.paint(origin, height, gpui::TextAlign::Left, None, window, cx);
            origin.y += line.size(height).height;
        }
        if focus.is_focused(window)
            && let Some(cursor) = state.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _| {
            input.last_layout = std::mem::take(&mut state.lines);
            input.last_bounds = Some(bounds);
            input.last_line_height = height;
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if (self.dictation.is_some() || self.mirrored_dictation)
            && self.selected_range.is_empty()
            && self.selected_range.end == self.content.len()
            && self.marked_range.is_none()
            && !self.is_selecting
        {
            // Follow after wrapping/layout changes too, not just when Apple
            // publishes a partial. The last caret stays visible during pauses
            // and when the composer/window changes width. Preserve an explicit
            // earlier caret/selection while the user edits during a pause.
            self.scroll.scroll_to_bottom();
        }
        div()
            .id("orbit-composer-input")
            .w_full()
            .max_h(px(180.))
            .when(self.editor_mode, |s| s.h_full().max_h(relative(1.)))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::save_file))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::complete_command))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .font_family(if self.editor_mode {
                crate::theme::mono_font()
            } else {
                crate::theme::ui_font()
            })
            .line_height(if self.editor_mode {
                crate::theme::mono_line_height(22.)
            } else {
                crate::theme::ui_line_height(22.)
            })
            .text_size(if self.editor_mode {
                crate::theme::mono_px(14.)
            } else {
                crate::theme::ui_px(14.)
            })
            .text_color(rgb(0xebebeb))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
        KeyBinding::new("up", Up, Some("TextInput")),
        KeyBinding::new("down", Down, Some("TextInput")),
        KeyBinding::new("shift-up", SelectUp, Some("TextInput")),
        KeyBinding::new("shift-down", SelectDown, Some("TextInput")),
        KeyBinding::new("cmd-a", SelectAll, Some("TextInput")),
        KeyBinding::new("cmd-shift-a", SelectAll, Some("TextInput")),
        KeyBinding::new("tab", CompleteCommand, Some("TextInput")),
        KeyBinding::new("cmd-v", Paste, Some("TextInput")),
        KeyBinding::new("cmd-c", Copy, Some("TextInput")),
        KeyBinding::new("cmd-x", Cut, Some("TextInput")),
        KeyBinding::new("cmd-s", SaveFile, Some("TextInput")),
        KeyBinding::new("cmd-z", Undo, Some("TextInput")),
        KeyBinding::new("cmd-shift-z", Redo, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("cmd-left", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new("cmd-right", End, Some("TextInput")),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("TextInput")),
        KeyBinding::new("enter", Submit, Some("TextInput")),
        KeyBinding::new("shift-enter", Newline, Some("TextInput")),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn native_editor_enter_tab_and_unicode_undo_redo_preserve_text(cx: &mut gpui::TestAppContext) {
        cx.update(bind_keys);
        let (input, cx) = cx.add_window_view(TextInput::new);
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_editor_mode();
                input.set_text("let café = 1;", cx);
                input.submit(&Submit, window, cx);
                input.complete_command(&CompleteCommand, window, cx);
                input.replace_text_in_range(None, "日本語", window, cx);
                assert_eq!(input.text(), "let café = 1;\n\t日本語");
                input.undo_edit(cx);
                assert_eq!(input.text(), "let café = 1;");
                input.redo_edit(cx);
                assert_eq!(input.text(), "let café = 1;\n\t日本語");
                input.set_secret(true, cx);
                assert!(input.history.undo.is_empty() && input.history.redo.is_empty());
                input.replace_text_in_range(None, "hidden", window, cx);
                assert!(input.history.undo.is_empty());
            })
        });
    }
    #[gpui::test]
    fn input_history_is_bounded_and_new_edit_discards_only_redo(cx: &mut gpui::TestAppContext) {
        let (input, cx) = cx.add_window_view(TextInput::new);
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_text(&"a".repeat(60 * 1024), cx);
                for i in 0..160 {
                    input.history.last_edit = None;
                    input.replace_text_in_range(
                        Some(0..1),
                        if i % 2 == 0 { "b" } else { "a" },
                        window,
                        cx,
                    );
                }
                assert!(input.history.undo.len() <= 128);
                assert!(
                    input
                        .history
                        .undo
                        .iter()
                        .chain(&input.history.redo)
                        .map(|s| s.text.len())
                        .sum::<usize>()
                        <= 2 * 1024 * 1024
                );
                input.undo_edit(cx);
                assert!(!input.history.redo.is_empty());
                input.replace_text_in_range(Some(0..1), "c", window, cx);
                assert!(input.history.redo.is_empty());
                assert!(input.text().starts_with('c'));
            })
        });
    }
    #[gpui::test]
    fn text_only_inputs_keep_plain_paste_and_secret_fields_cannot_enable_images(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(bind_keys);
        let (input, cx) = cx.add_window_view(TextInput::new);
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                assert!(!input.image_paste);
                input.focus_handle.focus(window);
                cx.write_to_clipboard(ClipboardItem::new_string("First\r\nSecond 日本語".into()));
                input.paste(&Paste, window, cx);
                assert_eq!(input.text(), "First\nSecond 日本語");
                input.set_secret(true, cx);
                input.set_image_paste(true);
                assert!(!input.image_paste);
            });
        });
    }
    #[test]
    fn secret_layout_maps_extended_graphemes_without_leaking_or_splitting() {
        let text = "A👨‍👩‍👧‍👦e\u{301}日本";
        let secret = SecretLayout::new(text);
        assert_eq!(secret.text.as_ref(), "•••••");
        for (i, (offset, grapheme)) in text.grapheme_indices(true).enumerate() {
            assert_eq!(secret.to_display(offset), i * 3);
            assert_eq!(secret.to_source(i * 3), offset);
            for byte in offset..offset + grapheme.len() {
                assert_eq!(secret.to_display(byte), i * 3);
            }
        }
        assert_eq!(secret.to_source(usize::MAX), text.len());
        assert_eq!(secret.to_display(usize::MAX), 15);
        let empty = SecretLayout::new("");
        assert_eq!(empty.to_source(100), 0);
        assert_eq!(empty.to_display(100), 0);
    }

    #[gpui::test]
    fn secret_input_masks_ime_bounds_mouse_and_disables_clipboard_commands(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(bind_keys);
        let (input, cx) = cx.add_window_view(TextInput::new);
        let secret = "é👨‍👩‍👧‍👦日本";
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_secret(true, cx);
                input.set_text(secret, cx);
                assert_eq!(input.secret.as_ref().unwrap().text.as_ref(), "••••");
                assert!(input.command_query().is_none());
                input.select_all(&SelectAll, window, cx);
                cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
                input.copy(&Copy, window, cx);
                input.cut(&Cut, window, cx);
                assert_eq!(input.text(), secret);
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some("unchanged")
                );
                let mut actual = Some(0..1);
                assert!(
                    input
                        .text_for_range(0..8, &mut actual, window, cx)
                        .is_none()
                );
                assert!(actual.is_none());
                input.begin_dictation(cx);
                assert!(input.dictation.is_none());
                input.selected_range = secret.len()..secret.len();
            })
        });
        cx.draw(point(px(0.), px(0.)), size(px(450.), px(300.)), |_, _| {
            input.clone().into_any_element()
        });
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                let mut utf16 = 0;
                for (offset, grapheme) in secret.grapheme_indices(true) {
                    let position = input.position_for_offset(offset).unwrap();
                    assert_eq!(
                        input.index_for_mouse_position(position + point(px(0.), px(1.))),
                        offset
                    );
                    let bounds = input
                        .bounds_for_range(utf16..utf16, input.last_bounds.unwrap(), window, cx)
                        .unwrap();
                    assert_eq!(bounds.origin, position);
                    utf16 += grapheme.encode_utf16().count();
                }
                input.replace_and_mark_text_in_range(None, "🦀", Some(2..2), window, cx);
                assert_eq!(input.secret.as_ref().unwrap().text.as_ref(), "•••••");
                assert_eq!(input.selected_range.start, input.content.len());
                assert_eq!(input.display_offset(input.content.len()), 15);
                assert!(
                    input
                        .bounds_for_range(utf16..utf16 + 2, input.last_bounds.unwrap(), window, cx)
                        .is_some()
                );
            })
        });
    }

    #[test]
    fn oversized_paste_is_rejected_as_a_whole() {
        assert!(replacement_fits(MAX_DRAFT_BYTES, 0..4, "🦀".len()));
        assert!(!replacement_fits(MAX_DRAFT_BYTES, 0..0, "é".len()));
        assert!(!replacement_fits(0, 0..0, usize::MAX));
    }
    #[test]
    fn ime_offsets_count_surrogate_pairs() {
        assert_eq!(utf16_to_utf8("A🦀é", 3), 5);
        assert_eq!(utf16_to_utf8("A🦀é", 4), 7);
        assert_eq!(utf16_to_utf8("A🦀é", 100), 7);
    }

    #[test]
    fn dictated_updates_are_append_only_and_manual_deletion_is_not_reinserted() {
        let mut tail = DictationTail::default();
        let mut content = "Typed 🦀".to_owned();
        for snapshot in [
            "Hello",
            "Hello 日本語",
            "Hello",
            "",
            "different correction",
            "Hello 日本語 more words",
        ] {
            let next = tail.appended(&content, snapshot).unwrap();
            assert!(next.starts_with(&content));
            content = next;
        }
        assert_eq!(content, "Typed 🦀 Hello 日本語 more words");
        content = "Manual replacement".into();
        assert_eq!(
            tail.appended(&content, "Hello 日本語 more words").unwrap(),
            content
        );
        assert_eq!(
            tail.appended(&content, "Hello 日本語 more words and new speech")
                .unwrap(),
            "Manual replacement and new speech"
        );
        assert!(
            DictationTail::default()
                .appended(&"x".repeat(MAX_DRAFT_BYTES), "é")
                .is_none()
        );
    }

    #[gpui::test]
    fn manual_edits_keep_dictation_active_and_append_at_tail(cx: &mut gpui::TestAppContext) {
        cx.update(bind_keys);
        let (input, cx) = cx.add_window_view(TextInput::new);
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.focus_handle.focus(window);
                input.set_text("Existing 🦀", cx);
                input.begin_dictation(cx);
                assert!(input.apply_dictation("Hello 日本語", cx));
                input.selected_range = 0..input.content.len();
            })
        });
        cx.simulate_input("Manual edits");
        cx.update(|_, cx| {
            input.update(cx, |input, cx| {
                assert!(input.dictation.is_some());
                assert_eq!(input.text(), "Manual edits");
                input.move_to(0, cx);
                assert!(input.dictation.is_some());
                assert!(input.apply_dictation("Hello 日本語 more", cx));
                assert_eq!(input.text(), "Manual edits more");
                let before = input.text();
                assert!(input.apply_dictation("Hello", cx));
                assert_eq!(input.text(), before);
                assert!(input.selected_range.is_empty());
                assert!(input.marked_range.is_none());
                assert_eq!(input.selected_range.end, input.content.len());
                input.end_dictation(cx);
                assert!(!input.apply_dictation("late", cx));
            })
        });
    }

    #[gpui::test]
    fn native_dictation_follows_new_last_line(cx: &mut gpui::TestAppContext) {
        let (input, cx) = cx.add_window_view(TextInput::new);
        cx.simulate_resize(size(px(450.), px(300.)));
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(point(px(0.), px(0.)), size(px(450.), px(300.)), |_, _| {
                input.clone().into_any_element()
            });
        };
        cx.update(|_, cx| {
            input.update(cx, |input, cx| {
                input.begin_dictation(cx);
                assert!(input.apply_dictation(&"Line 日本語\n".repeat(60), cx));
            })
        });
        draw(cx);
        cx.update(|_, cx| {
            let input = input.read(cx);
            assert!(input.scroll.offset().y < px(-500.));
            assert_eq!(input.scroll.offset().y, -input.scroll.max_offset().height);
        });
        cx.update(|_, cx| {
            input.update(cx, |input, cx| {
                input.scroll.set_offset(point(px(0.), px(0.)));
                assert!(input.apply_dictation(
                    &format!(
                        "{}{}",
                        "Line 日本語\n".repeat(60),
                        "Revision 🦀\n".repeat(90)
                    ),
                    cx
                ));
            })
        });
        draw(cx);
        cx.update(|_, cx| {
            let input = input.read(cx);
            assert!(input.scroll.offset().y < px(-1000.));
            assert_eq!(input.scroll.offset().y, -input.scroll.max_offset().height);
            assert!(input.selected_range.is_empty());
        });
    }

    #[gpui::test]
    fn dictation_does_not_pull_manual_edits_offscreen_during_a_pause(
        cx: &mut gpui::TestAppContext,
    ) {
        let (input, cx) = cx.add_window_view(TextInput::new);
        cx.simulate_resize(size(px(360.), px(260.)));
        let text = "Line 日本語\n".repeat(50);
        cx.update(|_, cx| {
            input.update(cx, |input, cx| {
                input.begin_dictation(cx);
                input.apply_dictation(&text, cx);
            })
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(point(px(0.), px(0.)), size(px(360.), px(260.)), |_, _| {
                input.clone().into_any_element()
            });
        };
        draw(cx);
        cx.update(|_, cx| {
            input.update(cx, |input, cx| {
                input.move_to(0, cx);
                input.scroll.set_offset(point(px(0.), px(0.)));
            })
        });
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(input.read(cx).scroll.offset().y, px(0.));
            input.update(cx, |input, cx| {
                input.apply_dictation(&format!("{text}Next line"), cx);
            });
        });
        draw(cx);
        cx.update(|_, cx| {
            let input = input.read(cx);
            assert_eq!(input.scroll.offset().y, -input.scroll.max_offset().height);
            assert!(input.selected_range.is_empty());
            assert!(input.marked_range.is_none());
        });
    }

    #[gpui::test]
    fn dictation_follows_wrapped_caret_after_pause_and_resize(cx: &mut gpui::TestAppContext) {
        let (input, cx) = cx.add_window_view(TextInput::new);
        let text = "Continue 日本語 🦀 with the next sentence. ".repeat(80);
        cx.update(|_, cx| {
            input.update(cx, |input, cx| {
                input.is_selecting = true;
                input.begin_dictation(cx);
                assert!(!input.is_selecting);
                assert!(input.apply_dictation(&text, cx));
            })
        });
        for width in [450., 220., 600.] {
            cx.simulate_resize(size(px(width), px(300.)));
            // No transcript arrives during this resize/pause. A render must
            // still follow the final wrapped line instead of the old offset.
            cx.draw(point(px(0.), px(0.)), size(px(width), px(300.)), |_, _| {
                input.clone().into_any_element()
            });
            cx.update(|_, cx| {
                let input = input.read(cx);
                assert!(input.selected_range.is_empty());
                assert_eq!(input.selected_range.end, text.len());
                assert!(input.marked_range.is_none());
                assert_eq!(input.scroll.offset().y, -input.scroll.max_offset().height);
                let caret = input.position_for_offset(text.len()).unwrap();
                let viewport = input.scroll.bounds();
                assert!(
                    caret.y >= viewport.top(),
                    "caret above viewport after resize"
                );
                assert!(
                    caret.y + input.last_line_height <= viewport.bottom() + px(0.1),
                    "caret below viewport after resize"
                );
            });
        }
    }
}
