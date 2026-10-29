//! GPUI terminal viewport. Alacritty handles terminal state; this view paints
//! only visible cells and forwards native keyboard/IME input to its owned PTY.
use crate::terminal::{self, Frame, Size, Terminal};
use crate::theme::{rgb, rgba};
use gpui::{prelude::*, *};
use std::{
    ops::Range,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
pub enum TerminalEvent {
    Close,
    Dock(crate::terminal_dock::Side),
    DockStarted,
}
pub struct TerminalPanel {
    runtime: Option<Terminal>,
    frame: Arc<Frame>,
    focus: FocusHandle,
    bounds: Option<Bounds<Pixels>>,
    size: Size,
    selecting: bool,
    marked: String,
    starting: bool,
    stopping: bool,
    visible: bool,
    error: Option<String>,
    project: PathBuf,
    pending_input: Vec<u8>,
    startup_cancel: Arc<AtomicBool>,
}
impl EventEmitter<TerminalEvent> for TerminalPanel {}
impl Drop for TerminalPanel {
    fn drop(&mut self) {
        self.stop();
    }
}
impl Focusable for TerminalPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl TerminalPanel {
    pub fn new(project: PathBuf, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root = project.clone();
        let startup_cancel = Arc::new(AtomicBool::new(false));
        let cancel = startup_cancel.clone();
        cx.on_release(|this, _| this.stop()).detach();
        cx.spawn(async move |this, cx| {
            let task = cx
                .background_executor()
                .spawn(async move { Terminal::spawn(&root, cancel) });
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.starting = false;
                match result {
                    Ok(runtime) => {
                        if this.stopping {
                            runtime.stop();
                        } else {
                            runtime.set_visible(this.visible);
                            let _ = runtime.resize(this.size);
                            if !this.pending_input.is_empty() {
                                let _ = runtime.input(std::mem::take(&mut this.pending_input));
                            }
                        }
                        let wake = runtime.wake.clone();
                        this.frame = runtime.snapshot();
                        this.runtime = Some(runtime);
                        cx.spawn(async move |this, cx| {
                            while wake.recv().await.is_ok() {
                                if this
                                    .update(cx, |this, cx| {
                                        if let Some(runtime) = &this.runtime {
                                            this.frame = runtime.snapshot();
                                        }
                                        cx.notify();
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            let _ = this.update(cx, |this, cx| {
                                if let Some(runtime) = &this.runtime {
                                    this.frame = runtime.snapshot();
                                }
                                cx.notify();
                            });
                        })
                        .detach();
                    }
                    Err(error) => this.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        Self {
            runtime: None,
            frame: Arc::new(terminal::blank_frame()),
            focus: cx.focus_handle(),
            bounds: None,
            size: Size::new(100, 16),
            selecting: false,
            marked: String::new(),
            starting: true,
            stopping: false,
            visible: true,
            error: None,
            project,
            pending_input: Vec::new(),
            startup_cancel,
        }
    }
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if let Some(runtime) = &self.runtime {
            runtime.set_visible(visible);
        }
    }
    pub fn stop(&mut self) {
        self.stopping = true;
        self.startup_cancel.store(true, Ordering::Release);
        if let Some(runtime) = &self.runtime {
            runtime.stop();
        }
    }
    fn close(&mut self, cx: &mut Context<Self>) {
        self.stop();
        // Keep the entity's active-session guard until cleanup finishes. A
        // checkpoint restore immediately after Close must not race shell IO.
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(done) = this.update(cx, |this, cx| {
                    if this.is_settled() {
                        cx.emit(TerminalEvent::Close);
                        true
                    } else {
                        false
                    }
                }) else {
                    return;
                };
                if done {
                    return;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(10))
                    .await;
            }
        })
        .detach();
    }
    pub fn is_settled(&self) -> bool {
        !self.starting && self.runtime.as_ref().is_none_or(Terminal::is_settled)
    }
    fn input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if self.stopping {
            return;
        }
        if self.starting {
            if self.pending_input.len() + bytes.len() <= 64 * 1024 {
                self.pending_input.extend(bytes);
            } else {
                self.error = Some("Terminal input is limited to 64 KiB while starting".into());
                cx.notify();
            }
            return;
        }
        if let Some(runtime) = &self.runtime
            && let Err(error) = runtime.input(bytes)
        {
            self.error = Some(error.to_string());
            cx.notify();
        }
    }
    fn copy(&mut self, cx: &mut Context<Self>) {
        let Some(rx) = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.copy().ok())
        else {
            return;
        };
        cx.spawn(async move |_, cx| {
            if let Ok(Some(text)) = rx.recv().await {
                let _ = cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text)));
            }
        })
        .detach();
    }
    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            match paste_bytes(&text, self.frame.bracketed_paste) {
                Ok(bytes) => self.input(bytes, cx),
                Err(error) => {
                    self.error = Some(error.into());
                    cx.notify();
                }
            }
        }
    }
    fn key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let stroke = &event.keystroke;
        if stroke.modifiers.platform {
            match stroke.key.as_str() {
                "c" => {
                    self.copy(cx);
                    cx.stop_propagation();
                }
                "v" => {
                    self.paste(cx);
                    cx.stop_propagation();
                }
                _ => {}
            }
            return;
        }
        if self.marked.is_empty()
            && let Some(bytes) = key_bytes(
                &stroke.key,
                stroke.modifiers.control,
                stroke.modifiers.alt,
                stroke.modifiers.shift,
                self.frame.app_cursor,
            )
        {
            self.input(bytes, cx);
            cx.stop_propagation();
        }
    }
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize) {
        self.bounds
            .map(|bounds| {
                let row = (f32::from(position.y - bounds.origin.y)
                    / crate::theme::mono_cell_height())
                .max(0.) as usize;
                let column = (f32::from(position.x - bounds.origin.x)
                    / crate::theme::mono_cell_width())
                .max(0.) as usize;
                (
                    row.min(self.size.lines - 1),
                    column.min(self.size.columns - 1),
                )
            })
            .unwrap_or((0, 0))
    }
    fn select(&mut self, position: Point<Pixels>, start: bool) {
        let (row, column) = self.cell_at(position);
        if let Some(runtime) = &self.runtime {
            let _ = runtime.select(row, column, start);
        }
    }
}
fn paste_bytes(text: &str, bracketed: bool) -> Result<Vec<u8>, &'static str> {
    if text.len() > 64 * 1024 - 12 {
        return Err("Paste is limited to 64 KiB; use a file for larger input");
    }
    // Embedded bracket terminators must not escape the application's bracketed
    // paste guard. NUL and terminal escapes never become injected key commands.
    let text = text.replace(['\0', '\u{1b}'], "");
    Ok(if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\n").replace('\n', "\r").into_bytes()
    })
}
fn key_bytes(
    key: &str,
    control: bool,
    alt: bool,
    shift: bool,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    let modifier = 1 + usize::from(shift) + 2 * usize::from(alt) + 4 * usize::from(control);
    let navigation = match key {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(letter) = navigation {
        return Some(if modifier > 1 {
            format!("\x1b[1;{modifier}{letter}").into_bytes()
        } else {
            format!("\x1b{}{letter}", if app_cursor { 'O' } else { '[' }).into_bytes()
        });
    }
    if key == "tab" && shift {
        return Some(b"\x1b[Z".to_vec());
    }
    let csi = match key {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(code) = csi {
        return Some(if modifier > 1 {
            format!("\x1b[{code};{modifier}~").into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        });
    }
    let function = match key {
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    };
    if let Some(letter) = function {
        return Some(if modifier > 1 {
            format!("\x1b[1;{modifier}{letter}").into_bytes()
        } else {
            format!("\x1bO{letter}").into_bytes()
        });
    }
    let mut bytes = if control {
        let byte = match key {
            "space" | "@" => 0,
            "[" => 27,
            "\\" => 28,
            "]" => 29,
            "^" => 30,
            "_" => 31,
            "?" => 127,
            _ => {
                let byte = key.as_bytes().first().copied()?;
                if key.len() != 1 || !byte.is_ascii_alphabetic() {
                    return None;
                }
                byte.to_ascii_lowercase() - b'a' + 1
            }
        };
        vec![byte]
    } else {
        match key {
            "enter" => vec![b'\r'],
            "backspace" => vec![127],
            "tab" => vec![b'\t'],
            "escape" => vec![27],
            _ if alt && key.len() == 1 && key.is_ascii() => key.as_bytes().to_vec(),
            _ => return None,
        }
    };
    if alt {
        bytes.insert(0, 27);
    }
    Some(bytes)
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::theme::ensure_font_metrics(window);
        let focus = self.focus.clone();
        let entity = cx.entity();
        let painted = entity.clone();
        let frame = self.frame.clone();
        let cursor_focus = self.focus.clone();
        div()
            .id("native-terminal-panel").debug_selector(||"native-terminal-panel".into())
            .flex()
            .flex_col()
            .h_full()
            .w_full()
            .min_w(px(0.))
            .flex_none().overflow_hidden()
            .border_t_1()
            .border_color(rgba(0xebebeb18))
            .bg(rgba(0xebebeb08))
            .key_context("CodeTerminal")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .child(
                div().id("terminal-drag-handle").debug_selector(||"terminal-drag-handle".into()).cursor_move()
                    .on_drag(crate::terminal_dock::MoveTerminal,{let weak=cx.weak_entity();move|_,_,_,cx|{let _=weak.update(cx,|_,cx|cx.emit(TerminalEvent::DockStarted));cx.new(|_|crate::terminal_dock::MoveTerminal)}})
                    .h(px(32.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(crate::theme::ui_px(11.))
                    .text_color(rgb(0x929292))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(self.frame.title.clone()))
                    .when(self.frame.alternate, |s| s.child("Full screen"))
                    .when(self.frame.display_offset > 0, |s| {
                        s.child(format!("↑ {}", self.frame.display_offset))
                    })
                    .child(
                        self.project
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                    )
                    .child(div().flex_1())
                    .children(crate::terminal_dock::Side::ALL.into_iter().map(|side|div().id(("terminal-dock-side",side as usize)).px_1().cursor_pointer().hover(|s|s.bg(rgba(0xffffff14))).child(side.glyph()).on_click(cx.listener(move|_,_,_,cx|{cx.stop_propagation();cx.emit(TerminalEvent::Dock(side));}))))
                    .child(self.error.clone().unwrap_or_else(|| {
                        if self.starting {
                            "Starting…".into()
                        } else {
                            self.frame.status.clone()
                        }
                    }))
                    .child(
                        div()
                            .id("terminal-stop")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0xffffff10)))
                            .child(if self.frame.running {
                                "Stop"
                            } else {
                                "Stopped"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stop();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("terminal-close")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0xffffff10)))
                            .child("×")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close(cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("native-terminal-viewport")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .mx_2()
                    .relative()
                    .cursor(CursorStyle::IBeam)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, _| {
                            focus.focus(window);
                            this.selecting = true;
                            this.select(event.position, true);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                        if this.selecting {
                            this.select(event.position, false);
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.selecting = false),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.selecting = false),
                    )
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                        let delta = f32::from(
                            event
                                .delta
                                .pixel_delta(px(crate::theme::mono_cell_height()))
                                .y,
                        );
                        if let Some(runtime) = &this.runtime {
                            let _ = runtime
                                .scroll((delta / crate::theme::mono_cell_height()).round() as i32);
                        }
                        cx.stop_propagation();
                    }))
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                entity.update(cx, |this, _| {
                                    this.bounds = Some(bounds);
                                    let size = Size::new(
                                        (f32::from(bounds.size.width)
                                            / crate::theme::mono_cell_width())
                                            as usize,
                                        (f32::from(bounds.size.height)
                                            / crate::theme::mono_cell_height())
                                            as usize,
                                    );
                                    if size != this.size {
                                        this.size = size;
                                        if let Some(runtime) = &this.runtime {
                                            let _ = runtime.resize(size);
                                        }
                                    }
                                });
                            },
                            move |bounds, _, window, cx| {
                                window.handle_input(
                                    &cursor_focus,
                                    ElementInputHandler::new(bounds, painted.clone()),
                                    cx,
                                );
                                paint_frame(
                                    &frame,
                                    bounds,
                                    cursor_focus.is_focused(window),
                                    window,
                                    cx,
                                );
                            },
                        )
                        .size_full(),
                    ),
            )
            .when(!self.marked.is_empty(), |s| {
                s.child(
                    div()
                        .h(px(20.))
                        .px_3()
                        .text_size(crate::theme::ui_px(12.))
                        .text_color(rgb(0xd6d9df))
                        .child(self.marked.clone()),
                )
            })
    }
}
fn paint_frame(
    frame: &Frame,
    bounds: Bounds<Pixels>,
    focused: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let cell_width = crate::theme::mono_cell_width();
    let cell_height = crate::theme::mono_cell_height();
    for (row, cells) in frame.rows.iter().enumerate() {
        let y = bounds.origin.y + px(row as f32 * cell_height);
        for cell in cells {
            let x = bounds.origin.x + px(cell.column as f32 * cell_width);
            if let Some(bg) = cell.bg {
                window.paint_quad(fill(
                    Bounds::new(
                        point(x, y),
                        size(px(cell_width * cell.columns as f32), px(cell_height)),
                    ),
                    if bg == 0x202326 {
                        rgb(bg)
                    } else {
                        gpui::rgb(bg)
                    },
                ));
            }
            if cell.text.chars().all(|ch| ch == ' ') {
                continue;
            }
            let mut face = font(crate::theme::mono_font());
            if cell.bold {
                face.weight = FontWeight::BOLD;
            }
            if cell.italic {
                face.style = FontStyle::Italic;
            }
            let run = TextRun {
                len: cell.text.len(),
                font: face,
                color: if cell.fg == 0xd6d9df {
                    rgb(cell.fg)
                } else {
                    gpui::rgb(cell.fg)
                }
                .into(),
                background_color: None,
                underline: cell.underline.then_some(UnderlineStyle {
                    thickness: px(1.),
                    color: None,
                    wavy: false,
                }),
                strikethrough: cell.strike.then_some(StrikethroughStyle {
                    thickness: px(1.),
                    color: None,
                }),
            };
            let line = window.text_system().shape_line(
                cell.text.clone().into(),
                crate::theme::mono_px(12.),
                &[run],
                Some(px(cell_width)),
            );
            let _ = line.paint(point(x, y), px(cell_height), window, cx);
        }
    }
    if focused
        && let Some((row, column)) = frame
            .cursor
            .filter(|(row, column)| *row < frame.size.lines && *column < frame.size.columns)
    {
        window.paint_quad(fill(
            Bounds::new(
                bounds.origin + point(px(column as f32 * cell_width), px(row as f32 * cell_height)),
                size(px(1.5), px(cell_height)),
            ),
            rgb(0xa4cafa),
        ));
    }
}
impl EntityInputHandler for TerminalPanel {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        if self.marked.is_empty() {
            *actual = None;
            None
        } else {
            *actual = Some(0..self.marked.encode_utf16().count());
            Some(self.marked.clone())
        }
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.marked.encode_utf16().count()..self.marked.encode_utf16().count(),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.marked.is_empty()).then_some(0..self.marked.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let text = std::mem::take(&mut self.marked);
        if !text.is_empty() {
            self.input(text.into_bytes(), cx);
        }
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked.clear();
        self.input(text.as_bytes().to_vec(), cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = text.chars().take(2048).collect();
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (row, column) = self.frame.cursor.unwrap_or((0, 0));
        Some(Bounds::new(
            bounds.origin
                + point(
                    px(column as f32 * crate::theme::mono_cell_width()),
                    px(row as f32 * crate::theme::mono_cell_height()),
                ),
            size(
                px(crate::theme::mono_cell_width()),
                px(crate::theme::mono_cell_height()),
            ),
        ))
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn terminal_keys_and_paste_preserve_protocol_boundaries() {
        assert_eq!(
            super::key_bytes("up", false, false, false, true).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(
            super::key_bytes("c", true, false, false, false).unwrap(),
            [3]
        );
        assert_eq!(
            super::paste_bytes("日本語\x1b[201~", true).unwrap(),
            "\x1b[200~日本語[201~\x1b[201~".as_bytes()
        );
        assert!(super::paste_bytes(&"x".repeat(65536), false).is_err());
    }
}
