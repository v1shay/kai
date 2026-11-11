//! GPUI terminal viewport. Both terminal engines submit glyphs and quads to
//! the workspace Metal renderer and receive native keyboard/IME input.
use crate::terminal::{self, Frame, Size, Terminal};
use crate::theme::{rgb, rgba};
use gpui::{prelude::*, *};
use serde_json::json;
use std::{
    ops::Range,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
actions!(terminal, [Copy, Paste, SelectAll, NewTerminal, CloseTerminal, SendHome, SendEnd, DeleteToStart]);
pub fn bind_keys(cx: &mut App) {
    let context=Some("CodeTerminal");
    cx.bind_keys([
        KeyBinding::new("cmd-c",Copy,context), KeyBinding::new("cmd-v",Paste,context),
        KeyBinding::new("cmd-a",SelectAll,context),
        KeyBinding::new("cmd-w",CloseTerminal,context),
        KeyBinding::new("cmd-left",SendHome,context), KeyBinding::new("cmd-right",SendEnd,context),
        KeyBinding::new("cmd-backspace",DeleteToStart,context),
    ]);
    cx.bind_keys([KeyBinding::new(
        "cmd-w", crate::keyboard::Execute { id: "close_tab".into() },
        Some("Orbit && ChatCli && CodeTerminal"),
    )]);
}

pub enum TerminalEvent {
    Focused,
    Close,
    Dock(crate::terminal_dock::Side),
    DockStarted,
    TitleChanged,
    PersistenceChanged,
    New,
}
pub struct Session {
    pub number: u64,
    pub panel: Entity<TerminalPanel>,
    pub _subscription: Subscription,
}

impl Session {
    pub fn title(&self, cx: &App) -> String {
        let title = self.panel.read(cx).title();
        let title: String = title.chars().filter(|c| !c.is_control()).take(160).collect();
        if title.trim().is_empty() || title == "Terminal" { format!("Terminal {}", self.number) } else { title }
    }
}

pub struct TerminalPanel {
    runtime: Option<Terminal>,
    metal: Option<MetalDock>,
    resume: Option<terminal::Resume>,
    profile: PathBuf,
    ending: bool,
    frame: Arc<Frame>,
    focus: FocusHandle,
    bounds: Option<Bounds<Pixels>>,
    size: Size,
    selecting: bool,
    scroll_remainder: f32,
    marked: String,
    starting: bool,
    stopping: bool,
    visible: bool,
    error: Option<String>,
    project: PathBuf,
    pending_input: Vec<u8>,
    startup_cancel: Arc<AtomicBool>,
}
struct MetalDock {
    id: uuid::Uuid,
    open: bool,
    directory_ready: bool,
    ready: bool,
    stopped: bool,
    close_on_stop: bool,
    geometry: Option<(Bounds<Pixels>, Bounds<Pixels>, bool)>,
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
    pub(super) fn error(&self) -> Option<&str> { self.error.as_deref() }
    pub fn title(&self) -> &str { self.frame.title.trim() }
    fn refresh_frame(&mut self, cx: &mut Context<Self>) {
        if let Some(runtime) = &self.runtime {
            let next = runtime.snapshot();
            let title_changed = next.title != self.frame.title;
            self.frame = next;
            if title_changed { cx.emit(TerminalEvent::TitleChanged); }
        }
    }
    pub fn resume(&self) -> Option<&terminal::Resume> { self.resume.as_ref() }

    pub fn new(project: PathBuf, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_workspace(project, false, terminal::Backend::default(), PathBuf::new(), None, cx)
    }
    pub fn new_workspace(project: PathBuf, create_workspace: bool, backend: terminal::Backend, profile: PathBuf, resume: Option<terminal::Resume>, cx: &mut Context<Self>) -> Self {
        Self::start(project, create_workspace, backend, profile, resume, None, cx)
    }
    pub(super) fn agent_cli(project: PathBuf, args: Vec<String>, environment: Vec<(String, String)>, cx: &mut Context<Self>) -> Self {
        Self::start(project, false, terminal::Backend::Ghostty, PathBuf::new(), None, Some((args, environment)), cx)
    }
    fn start(project: PathBuf, create_workspace: bool, backend: terminal::Backend, profile: PathBuf,
             resume: Option<terminal::Resume>, command: Option<(Vec<String>, Vec<(String, String)>)>, cx: &mut Context<Self>) -> Self {
        let use_metal = backend == terminal::Backend::Metal && resume.is_none() && command.is_none()
            && crate::unified::active() && !std::env::args().any(|arg| arg == "--unified-smoke-test");
        let reconnect = resume.is_some();
        let direct = command.is_some();
        let session = resume.unwrap_or_else(|| terminal::Resume { id: uuid::Uuid::new_v4(), project: project.clone(), backend });
        let startup_cancel = Arc::new(AtomicBool::new(false));
        let cancel = startup_cancel.clone();
        let worker_profile = profile.clone();
        let worker_session = session.clone();
        if use_metal && create_workspace {
            let directory = project.clone();
            cx.spawn(async move |this, cx| {
                let result = cx.background_executor().spawn(async move { std::fs::create_dir_all(directory) }).await;
                let _ = this.update(cx, |panel, cx| {
                    match result {
                        Ok(()) => {
                            if let Some(metal) = &mut panel.metal { metal.directory_ready = true; }
                            cx.notify();
                        }
                        Err(error) => panel.finish_metal(Some(format!("Could not create the terminal workspace: {error}")), cx),
                    }
                });
            }).detach();
        }
        if !use_metal { cx.spawn(async move |this, cx| {
            let task = cx.background_executor().spawn(async move {
                let root = &worker_session.project;
                if let Some((command, environment)) = command {
                    let args: Vec<_> = command.iter().map(String::as_str).collect();
                    return Terminal::spawn_command_with_env_cancellable(root, std::path::Path::new("/bin/sh"), &args,
                        &environment, Size::new(100, 16), cancel, backend).map(|runtime| (runtime, false));
                }
                if create_workspace && !cancel.load(Ordering::Acquire) {
                    std::fs::create_dir_all(root).map_err(|error|anyhow::anyhow!("Could not create this chat's workspace: {error}"))?;
                }
                let server = (!worker_profile.as_os_str().is_empty())
                    .then(|| terminal::SessionServer::find(&worker_profile)).flatten();
                if let Some(server) = server {
                    Terminal::attach(&server, &worker_session, reconnect, cancel).map(|runtime| (runtime, true))
                } else if reconnect {
                    anyhow::bail!("Install tmux to reconnect to this terminal")
                } else {
                    Terminal::spawn(root, cancel, backend).map(|runtime| (runtime, false))
                }
            });
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.starting = false;
                let (result, durable) = match result {
                    Ok((runtime, durable)) => (Ok(runtime), durable),
                    Err(error) => {
                        let durable = this.stopping || (reconnect && !error.is::<terminal::SessionEnded>());
                        (Err(error), durable)
                    }
                };
                if !durable && this.resume.take().is_some() {
                    cx.emit(TerminalEvent::PersistenceChanged);
                }
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
                        this.runtime = Some(runtime);
                        this.refresh_frame(cx);
                        cx.spawn(async move |this, cx| {
                            while wake.recv().await.is_ok() {
                                if this
                                    .update(cx, |this, cx| {
                                        this.refresh_frame(cx);
                                        cx.notify();
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            let _ = this.update(cx, |this, cx| {
                                this.refresh_frame(cx);
                                if direct { cx.emit(TerminalEvent::Close); }
                                cx.notify();
                            });
                        })
                        .detach();
                    }
                    Err(error) => {
                        this.error = Some(error.to_string());
                        if direct { cx.emit(TerminalEvent::Close); }
                    },
                }
                cx.notify();
            });
        })
        .detach(); }
        Self {
            runtime: None,
            metal: use_metal.then(|| MetalDock {
                id: session.id, open: false, directory_ready: !create_workspace, ready: false,
                stopped: false, close_on_stop: false, geometry: None,
            }),
            resume: (!use_metal && !profile.as_os_str().is_empty()).then_some(session),
            profile,
            ending: false,
            frame: Arc::new(terminal::blank_frame()),
            focus: cx.focus_handle(),
            bounds: None,
            size: Size::new(100, 16),
            selecting: false,
            scroll_remainder: 0.,
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
    pub(super) fn is_visible(&self) -> bool { self.visible }

    pub fn set_visible(&mut self, visible: bool) {
        if self.visible == visible { return; }
        self.visible = visible;
        if let Some(metal) = &mut self.metal {
            metal.geometry = None;
            if !visible && metal.open && !metal.stopped {
                crate::unified::emit(&json!({"type":"dockTerminalHide","id":metal.id}));
            }
        }
        if let Some(runtime) = &self.runtime {
            runtime.set_visible(visible);
        }
    }
    pub(super) fn probe_command(&self, command: &str) -> bool {
        self.runtime.as_ref().is_some_and(|runtime| runtime.input(command.as_bytes().to_vec()).is_ok())
    }
    pub(super) fn probe_has_text(&self, text: &str) -> bool {
        self.frame.rows.iter().any(|row| {
            row.iter().map(|cell| cell.text.as_str()).collect::<String>().contains(text)
        })
    }
    pub fn stop(&mut self) {
        self.stopping = true;
        if let Some(metal) = &mut self.metal {
            if metal.open && !metal.stopped {
                crate::unified::emit(&json!({"type":"dockTerminalClose","id":metal.id}));
            } else {
                metal.stopped = true;
            }
        }
        self.startup_cancel.store(true, Ordering::Release);
        if let Some(runtime) = &self.runtime {
            runtime.stop();
        }
    }
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.end(true, cx);
    }

    fn end(&mut self, close: bool, cx: &mut Context<Self>) {
        if self.ending { return; }
        self.ending = true;
        if let Some(metal) = &mut self.metal {
            metal.close_on_stop |= close;
            self.stop();
            if self.metal.as_ref().is_some_and(|metal| metal.stopped) {
                self.finish_metal(None, cx);
            }
            return;
        }
        self.stop();
        // A closing terminal remains a checkpoint blocker until its shell
        // has actually stopped, including a session still being created.
        cx.spawn(async move |this, cx| {
            let session = loop {
                let Ok(state) = this.update(cx, |panel, _| panel.client_settled()
                    .then(|| panel.resume.clone().map(|session| (panel.profile.clone(), session)))) else { return; };
                if let Some(session) = state { break session; }
                cx.background_executor().timer(std::time::Duration::from_millis(10)).await;
            };
            let result = if let Some((profile, session)) = session {
                cx.background_executor().spawn(async move {
                    terminal::SessionServer::find(&profile)
                        .ok_or_else(|| anyhow::anyhow!("Install tmux to stop this terminal"))?
                        .end(session.id)
                }).await
            } else {
                Ok(())
            };
            let _ = this.update(cx, |panel, cx| {
                panel.ending = false;
                match result {
                    Ok(()) => {
                        if panel.resume.take().is_some() { cx.emit(TerminalEvent::PersistenceChanged); }
                        if close { cx.emit(TerminalEvent::Close); }
                    }
                    Err(error) => panel.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }).detach();
    }
    fn client_settled(&self) -> bool {
        if let Some(metal) = &self.metal { return metal.stopped; }
        !self.starting && self.runtime.as_ref().is_none_or(Terminal::is_settled)
    }
    pub fn is_settled(&self) -> bool {
        !self.ending && self.client_settled()
    }
    pub(super) fn metal_id(&self) -> Option<uuid::Uuid> { self.metal.as_ref().map(|metal| metal.id) }

    pub(super) fn metal_ready(&mut self, cx: &mut Context<Self>) {
        if let Some(metal) = &mut self.metal {
            if self.stopping || metal.stopped { return; }
            metal.ready = true;
            self.starting = false;
            cx.notify();
        }
    }

    pub(super) fn finish_metal(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        let Some(metal) = &mut self.metal else { return; };
        metal.stopped = true;
        metal.ready = false;
        self.starting = false;
        self.ending = false;
        self.error = error;
        if metal.close_on_stop { cx.emit(TerminalEvent::Close); }
        cx.notify();
    }

    fn sync_metal_geometry(&mut self, bounds: Bounds<Pixels>, clip: Bounds<Pixels>, focused: bool) {
        let Some(metal) = &mut self.metal else { return; };
        if !self.visible || self.stopping || metal.stopped || !metal.directory_ready { return; }
        if metal.geometry == Some((bounds, clip, focused)) { return; }
        metal.geometry = Some((bounds, clip, focused));
        if clip.size.width < px(2.) || clip.size.height < px(2.) {
            if metal.open { crate::unified::emit(&json!({"type":"dockTerminalHide","id":metal.id})); }
            return;
        }
        let rect = |bounds: Bounds<Pixels>| [f32::from(bounds.origin.x), f32::from(bounds.origin.y),
            f32::from(bounds.size.width), f32::from(bounds.size.height)];
        let directory = (!metal.open).then_some(&self.project);
        crate::unified::emit(&json!({"type":"dockTerminalSlot","id":metal.id,
            "directory":directory,"rect":rect(bounds),"clip":rect(clip),"focused":focused}));
        metal.open = true;
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
        if stroke.modifiers.platform { return; }
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
        let native = self.metal.is_some();
        let running = self.metal.as_ref().map_or(self.frame.running, |metal| metal.ready && !metal.stopped);
        let stop_label = if running { "Stop" } else { "Stopped" };
        let text_width = |text: &str| crate::composer_toolbar::text_width(window, text);
        // Account for every fixed control and gap before adding optional labels.
        // Omit whole labels instead of squeezing the title or project to an ellipsis.
        let fixed_width = 16. + 24. + 6. * 4. + 16. + text_width(stop_label)
            + crate::terminal_dock::Side::ALL.iter().map(|side| 8. + text_width(side.glyph())).sum::<f32>();
        let available = self.bounds.map_or(0., |bounds| f32::from(bounds.size.width) + 16.) - fixed_width;
        let status = self.error.clone().unwrap_or_else(|| if self.starting { "Starting…".into() } else if native { if running { "Running".into() } else { "Stopped".into() } } else { self.frame.status.clone() });
        let mut metadata = String::new();
        let project = self.project.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let project = if project == self.title() { String::new() } else { project };
        for label in [project, status,
            if self.starting || self.error.is_some() || !running { String::new() }
            else if self.resume.is_some() { "Persistent".into() } else { "Temporary".into() },
            if self.frame.alternate && self.resume.is_none() { "Full screen".into() } else { String::new() },
            if self.frame.display_offset > 0 { format!("↑ {}",self.frame.display_offset) } else { String::new() }] {
            if label.is_empty() { continue; }
            let candidate = if metadata.is_empty() { label } else { format!("{metadata} · {label}") };
            if text_width(&candidate) <= available { metadata = candidate; }
        }
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
            .on_action(cx.listener(|this,_: &Copy,_,cx|{this.copy(cx);cx.stop_propagation();}))
            .on_action(cx.listener(|this,_: &Paste,_,cx|{this.paste(cx);cx.stop_propagation();}))
            .on_action(cx.listener(|this,_: &SelectAll,_,cx|{if let Some(runtime)=&this.runtime{let _=runtime.select_all();}cx.stop_propagation();}))
            .on_action(cx.listener(|_,_: &NewTerminal,_,cx|{cx.emit(TerminalEvent::New);cx.stop_propagation();}))
            .on_action(cx.listener(|this,_: &CloseTerminal,_,cx|{this.close(cx);cx.stop_propagation();}))
            .on_action(cx.listener(|this,_: &SendHome,_,cx|{this.input(vec![1],cx);cx.stop_propagation();}))
            .on_action(cx.listener(|this,_: &SendEnd,_,cx|{this.input(vec![5],cx);cx.stop_propagation();}))
            .on_action(cx.listener(|this,_: &DeleteToStart,_,cx|{this.input(vec![21],cx);cx.stop_propagation();}))
            .child(
                div().id("terminal-header")
                    .h(px(32.)).flex_none().min_w(px(0.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(crate::theme::ui_px(11.))
                    .text_color(rgb(0x929292))
                    .child(div().id("terminal-drag-handle").debug_selector(||"terminal-drag-handle".into())
                        .w(px(24.)).h_full().flex_none().flex().items_center().justify_center()
                        .cursor_move().hover(|s|s.text_color(rgb(0xebebeb)).bg(rgba(0xffffff10)))
                        .child("⠿")
                        .on_mouse_down(MouseButton::Left,|_,window,cx|{crate::native::panel_drag_guard(window,true);cx.stop_propagation();})
                        .on_mouse_up(MouseButton::Left,|_,window,_|crate::native::panel_drag_guard(window,false))
                        .on_drag(crate::terminal_dock::MoveTerminal,{let weak=cx.weak_entity();move|_,_,_,cx|{let _=weak.update(cx,|_,cx|cx.emit(TerminalEvent::DockStarted));cx.new(|_|crate::terminal_dock::MoveTerminal)}}))
                    .child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().child(metadata))
                    .children(crate::terminal_dock::Side::ALL.into_iter().map(|side|div().id(("terminal-dock-side",side as usize)).flex_none().px_1().cursor_pointer().hover(|s|s.bg(rgba(0xffffff14))).child(side.glyph()).on_click(cx.listener(move|_,_,_,cx|{cx.stop_propagation();cx.emit(TerminalEvent::Dock(side));}))))
                    .child(
                        div()
                            .id("terminal-stop").flex_none()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0xffffff10)))
                            .child(if running {
                                "Stop"
                            } else {
                                "Stopped"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.end(false, cx);
                            })),
                    )
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
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            focus.focus(window);
                            cx.emit(TerminalEvent::Focused);
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
                            // Trackpads often deliver less than one row per event.
                            let total = this.scroll_remainder + delta / crate::theme::mono_cell_height();
                            let lines = total.trunc();
                            this.scroll_remainder = total - lines;
                            if lines != 0. {
                                let _ = runtime.scroll(lines as i32);
                            }
                        }
                        cx.stop_propagation();
                    }))
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                entity.update(cx, |this, cx| {
                                    let width_changed = this.bounds.is_none_or(|previous| previous.size.width != bounds.size.width);
                                    this.bounds = Some(bounds);
                                    if width_changed { cx.notify(); }
                                    let size = Size::new(
                                        (f32::from(bounds.size.width)
                                            / crate::theme::mono_cell_width())
                                            as usize,
                                        (f32::from(bounds.size.height)
                                            / crate::theme::mono_cell_height())
                                            as usize,
                                    ).with_cell(crate::theme::mono_cell_width(),crate::theme::mono_cell_height());
                                    if size != this.size {
                                        this.size = size;
                                        if let Some(runtime) = &this.runtime {
                                            let _ = runtime.resize(size);
                                        }
                                    }
                                });
                            },
                            move |bounds, _, window, cx| {
                                if native {
                                    let clip = bounds.intersect(&window.content_mask().bounds);
                                    let focused = cursor_focus.is_focused(window);
                                    painted.update(cx, |panel, _| panel.sync_metal_geometry(bounds, clip, focused));
                                    return;
                                }
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
        // Foreground styling can split a continuous background into many
        // cell runs. Emit one GPU quad for each contiguous same-color span.
        let mut remaining = cells.iter().peekable();
        while let Some(cell) = remaining.next() {
            let Some(bg) = cell.bg else { continue; };
            let mut end = cell.column + cell.columns;
            while let Some(next) = remaining.next_if(|next| next.column == end && next.bg == Some(bg)) {
                end += next.columns;
            }
            let x = bounds.origin.x + px(cell.column as f32 * cell_width);
            window.paint_quad(fill(
                Bounds::new(point(x, y), size(px(cell_width * (end - cell.column) as f32), px(cell_height))),
                if bg == 0x202326 { rgb(bg) } else { gpui::rgb(bg) },
            ));
        }
        for cell in cells {
            let x = bounds.origin.x + px(cell.column as f32 * cell_width);
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
