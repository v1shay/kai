//! Demand-created native PTY with Alacritty's Rust terminal state machine.
//! The worker owns parsing/process IO; the UI receives only one coalesced,
//! viewport-sized frame. Clipboard OSC52 is disabled and never executes links.
use alacritty_terminal::{
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{Config, Osc52, Term, TermMode, cell::Flags, color::Colors, point_to_viewport},
    vte::ansi::{Color, CursorShape, NamedColor, Processor},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::VecDeque,
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};
const HISTORY_LINES: usize = 2000;
const INPUT_LIMIT: usize = 64 * 1024;
const WRITE_BUDGET: usize = 256 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub columns: usize,
    pub lines: usize,
    pub cell_width: u16,
    pub cell_height: u16,
}
impl Size {
    pub fn new(columns: usize, lines: usize) -> Self {
        Self {
            columns: columns.clamp(2, 512),
            lines: lines.clamp(1, 256),
            cell_width: 7,
            cell_height: 14,
        }
    }
    pub fn with_cell(mut self, width: f32, height: f32) -> Self {
        self.cell_width = width.round().clamp(1., 128.) as u16;
        self.cell_height = height.round().clamp(1., 128.) as u16;
        self
    }
}
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}
#[derive(Clone, Debug)]
pub struct Cell {
    pub column: usize,
    pub columns: usize,
    pub text: String,
    pub fg: u32,
    pub bg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}
#[derive(Clone, Debug)]
pub struct Frame {
    pub rows: Vec<Vec<Cell>>,
    pub size: Size,
    pub cursor: Option<(usize, usize)>,
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    pub alternate: bool,
    pub display_offset: usize,
    pub title: String,
    pub status: String,
    pub running: bool,
}
impl Frame {
    fn empty(size: Size) -> Self {
        Self {
            rows: vec![vec![]; size.lines],
            size,
            cursor: None,
            app_cursor: false,
            bracketed_paste: false,
            alternate: false,
            display_offset: 0,
            title: "Terminal".into(),
            status: "Starting…".into(),
            running: true,
        }
    }
}
pub fn blank_frame() -> Frame {
    Frame::empty(Size::new(100, 16))
}
enum Control {
    Input(Vec<u8>),
    Resize(Size),
    Scroll(i32),
    Select {
        row: usize,
        column: usize,
        start: bool,
    },
    Copy(async_channel::Sender<Option<String>>),
    SelectAll,
}
#[derive(Clone)]
struct Proxy {
    writes: Arc<Mutex<VecDeque<Vec<u8>>>>,
    metadata: Arc<Mutex<(String, Size)>>,
}
impl EventListener for Proxy {
    fn send_event(&self, event: Event) {
        // Only protocol answers return to the shell. Never honor terminal
        // clipboard loads/stores, arbitrary title commands, or URL actions.
        let value = match event {
            Event::PtyWrite(text) => Some(text),
            Event::Title(title) => {
                self.metadata.lock().unwrap_or_else(|e| e.into_inner()).0 = title
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(240)
                    .collect();
                None
            }
            Event::ResetTitle => {
                self.metadata.lock().unwrap_or_else(|e| e.into_inner()).0 = "Terminal".into();
                None
            }
            Event::TextAreaSizeRequest(format) => {
                let size = self.metadata.lock().unwrap_or_else(|e| e.into_inner()).1;
                Some(format(alacritty_terminal::event::WindowSize {
                    num_lines: size.lines as u16,
                    num_cols: size.columns as u16,
                    cell_width: size.cell_width,
                    cell_height: size.cell_height,
                }))
            }
            Event::ColorRequest(index, format) => {
                let color = if index < 256 {
                    Color::Indexed(index as u8)
                } else if index == 257 {
                    Color::Named(NamedColor::Background)
                } else {
                    Color::Named(NamedColor::Foreground)
                };
                let value = color_value(color);
                Some(format(alacritty_terminal::vte::ansi::Rgb {
                    r: (value >> 16) as u8,
                    g: (value >> 8) as u8,
                    b: value as u8,
                }))
            }
            _ => None,
        };
        if let Some(value) = value.filter(|v| v.len() <= 4096) {
            let mut writes = self.writes.lock().unwrap_or_else(|e| e.into_inner());
            if writes.len() < 32 {
                writes.push_back(value.into_bytes());
            }
        }
    }
}
pub struct Terminal {
    control: SyncSender<Control>,
    signal: UnixStream,
    stopped: Arc<AtomicBool>,
    visible: Arc<AtomicBool>,
    queued: Arc<AtomicUsize>,
    frame: Arc<Mutex<Arc<Frame>>>,
    pub wake: async_channel::Receiver<()>,
    pub settled: async_channel::Receiver<()>,
}
impl Terminal {
    pub fn spawn(project: &Path, stopped: Arc<AtomicBool>) -> Result<Self> {
        ensure!(
            !stopped.load(Ordering::Acquire),
            "Terminal startup canceled"
        );
        let shell = std::env::var_os("SHELL")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && path.is_file())
            .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
        Self::spawn_command_cancellable(project, &shell, &["-l", "-i"], Size::new(100, 16), stopped)
    }
    #[cfg(test)]
    fn spawn_command(project: &Path, shell: &Path, args: &[&str], size: Size) -> Result<Self> {
        Self::spawn_command_cancellable(
            project,
            shell,
            args,
            size,
            Arc::new(AtomicBool::new(false)),
        )
    }
    fn spawn_command_cancellable(
        project: &Path,
        shell: &Path,
        args: &[&str],
        size: Size,
        stopped: Arc<AtomicBool>,
    ) -> Result<Self> {
        ensure!(
            !stopped.load(Ordering::Acquire),
            "Terminal startup canceled"
        );
        ensure!(project.is_dir(), "Terminal project folder is unavailable");
        let mut master = -1;
        let mut slave = -1;
        let mut winsize = window_size(size);
        // openpty runs in the parent. Only async-signal-safe calls are used by
        // pre_exec in the multithreaded child, before exec installs the shell.
        ensure!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut winsize,
                )
            } == 0,
            "Could not create native PTY: {}",
            std::io::Error::last_os_error()
        );
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            ensure!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } >= 0,
                "Could not protect PTY descriptors"
            );
        }
        nonblocking(master.as_raw_fd())?;
        let mut command = Command::new(shell);
        crate::process_env::apply(&mut command);
        command
            .args(args)
            .current_dir(project)
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        let child_stop = stopped.clone();
        unsafe {
            command.pre_exec(move || {
                // AtomicBool is lock-free on the supported native platforms;
                // no allocation or mutex access is introduced after fork.
                if child_stop.load(Ordering::Acquire) {
                    return Err(std::io::Error::from_raw_os_error(libc::ECANCELED));
                }
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let (signal, worker_signal) = UnixStream::pair()?;
        signal.set_nonblocking(true)?;
        worker_signal.set_nonblocking(true)?;
        ensure!(
            !stopped.load(Ordering::Acquire),
            "Terminal startup canceled"
        );
        let child = command.spawn().context("Could not start terminal shell")?;
        let owner = Arc::new(Mutex::new(Some(child.id() as i32)));
        let child = OwnedChild {
            child,
            owner: owner.clone(),
        };
        // A concurrent Stop after fork keeps the same process owner and reaps
        // it before returning, rather than handing a live shell to a dead view.
        ensure!(
            !stopped.load(Ordering::Acquire),
            "Terminal startup canceled"
        );
        let (control, controls) = mpsc::sync_channel(32);
        let (wake_tx, wake) = async_channel::bounded(1);
        let (settled_tx, settled) = async_channel::bounded(1);
        let visible = Arc::new(AtomicBool::new(true));
        let queued = Arc::new(AtomicUsize::new(0));
        let frame = Arc::new(Mutex::new(Arc::new(Frame::empty(size))));
        let shared = Shared {
            stopped: stopped.clone(),
            visible: visible.clone(),
            queued: queued.clone(),
            frame: frame.clone(),
            wake: wake_tx,
            settled: settled_tx,
            metadata: Arc::new(Mutex::new(("Terminal".into(), size))),
        };
        std::thread::Builder::new()
            .name("code-terminal".into())
            .spawn(move || worker(master, worker_signal, child, size, controls, shared))?;
        Ok(Self {
            control,
            signal,
            stopped,
            visible,
            queued,
            frame,
            wake,
            settled,
        })
    }
    fn send(&self, control: Control) -> Result<()> {
        ensure!(
            !self.stopped.load(Ordering::Acquire),
            "Terminal has stopped"
        );
        self.control
            .try_send(control)
            .map_err(|_| anyhow::anyhow!("Terminal input is busy; try again"))?;
        self.signal();
        Ok(())
    }
    fn signal(&self) {
        let mut stream = &self.signal;
        let _ = stream.write(&[1]);
    }
    pub fn input(&self, bytes: Vec<u8>) -> Result<()> {
        ensure!(bytes.len() <= INPUT_LIMIT, "Terminal paste exceeds 64 KiB");
        let len = bytes.len();
        ensure!(
            self.queued
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| (current
                    .saturating_add(len)
                    <= WRITE_BUDGET)
                    .then_some(current + len))
                .is_ok(),
            "Terminal input queue is full; try again"
        );
        if let Err(error) = self.send(Control::Input(bytes)) {
            self.queued.fetch_sub(len, Ordering::AcqRel);
            return Err(error);
        }
        Ok(())
    }
    pub fn resize(&self, size: Size) -> Result<()> {
        self.send(Control::Resize(Size::new(size.columns, size.lines)))
    }
    pub fn scroll(&self, lines: i32) -> Result<()> {
        self.send(Control::Scroll(lines.clamp(-2000, 2000)))
    }
    pub fn select(&self, row: usize, column: usize, start: bool) -> Result<()> {
        self.send(Control::Select { row, column, start })
    }
    pub fn select_all(&self) -> Result<()> { self.send(Control::SelectAll) }
    pub fn copy(&self) -> Result<async_channel::Receiver<Option<String>>> {
        let (tx, rx) = async_channel::bounded(1);
        self.send(Control::Copy(tx))?;
        Ok(rx)
    }
    pub fn snapshot(&self) -> Arc<Frame> {
        self.frame.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn set_visible(&self, visible: bool) {
        self.visible.store(visible, Ordering::Release);
        self.signal();
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.signal();
    }
    pub fn is_settled(&self) -> bool {
        self.settled.is_closed()
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Shared {
    stopped: Arc<AtomicBool>,
    visible: Arc<AtomicBool>,
    queued: Arc<AtomicUsize>,
    frame: Arc<Mutex<Arc<Frame>>>,
    wake: async_channel::Sender<()>,
    settled: async_channel::Sender<()>,
    metadata: Arc<Mutex<(String, Size)>>,
}
struct OwnedChild {
    child: Child,
    owner: Arc<Mutex<Option<i32>>>,
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let mut owner = self.owner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pid) = *owner {
            kill_session(pid);
            let _ = self.child.wait();
            *owner = None;
        }
    }
}
/// The root child is unreaped while checking session membership, preventing PID
/// reuse from turning cleanup into a signal to an unrelated process.
fn kill_session(session: i32) {
    #[cfg(target_os = "macos")]
    {
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) }.max(0) as usize;
        let mut pids = vec![0i32; (count + 128).min(131072)];
        let length = unsafe {
            libc::proc_listallpids(
                pids.as_mut_ptr().cast(),
                (pids.len() * std::mem::size_of::<i32>()) as i32,
            )
        }
        .max(0) as usize;
        for pid in pids.into_iter().take(length) {
            if pid > 0 && pid != session && unsafe { libc::getsid(pid) } == session {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            if let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>()
                && pid != session
                && unsafe { libc::getsid(pid) } == session
            {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
    }
    unsafe {
        libc::kill(-session, libc::SIGKILL);
        libc::kill(session, libc::SIGKILL);
    }
}
fn nonblocking(fd: i32) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    ensure!(
        flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
        "Could not configure PTY IO"
    );
    Ok(())
}
fn window_size(size: Size) -> libc::winsize {
    libc::winsize {
        ws_row: size.lines as u16,
        ws_col: size.columns as u16,
        ws_xpixel: (size.columns * usize::from(size.cell_width)).min(u16::MAX as usize) as u16,
        ws_ypixel: (size.lines * usize::from(size.cell_height)).min(u16::MAX as usize) as u16,
    }
}
fn worker(
    mut master: File,
    mut signal: UnixStream,
    child: OwnedChild,
    mut size: Size,
    controls: mpsc::Receiver<Control>,
    shared: Shared,
) {
    let writes = Arc::new(Mutex::new(VecDeque::new()));
    let mut term = Term::new(
        Config {
            scrolling_history: HISTORY_LINES,
            osc52: Osc52::Disabled,
            ..Default::default()
        },
        &size,
        Proxy {
            writes: writes.clone(),
            metadata: shared.metadata.clone(),
        },
    );
    let mut parser: Processor = Processor::new();
    let mut buffer = [0u8; 8192];
    let mut dirty = true;
    let mut published = Instant::now() - Duration::from_secs(1);
    let mut input = VecDeque::<(Vec<u8>, bool)>::new();
    let mut input_offset = 0usize;
    let mut pending_bytes = 0usize;
    let mut done = false;
    let mut status = String::new();
    while !shared.stopped.load(Ordering::Acquire) && !done {
        for _ in 0..32 {
            let Ok(control) = controls.try_recv() else {
                break;
            };
            match control {
                Control::Input(bytes) => {
                    pending_bytes += bytes.len();
                    if !bytes.is_empty() {
                        input.push_back((bytes, true));
                    }
                    term.scroll_display(Scroll::Bottom);
                    term.selection = None;
                    dirty = true;
                }
                Control::Resize(next) => {
                    if next != size {
                        size = next;
                        shared.metadata.lock().unwrap_or_else(|e| e.into_inner()).1 = size;
                        term.resize(size);
                        let next = window_size(size);
                        unsafe {
                            libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as _, &next);
                        }
                        dirty = true;
                    }
                }
                Control::Scroll(lines) => {
                    term.scroll_display(Scroll::Delta(lines));
                    dirty = true;
                }
                Control::Select { row, column, start } => {
                    let point = Point::new(
                        Line(row.min(size.lines - 1) as i32 - term.grid().display_offset() as i32),
                        Column(column.min(size.columns - 1)),
                    );
                    if start {
                        term.selection =
                            Some(Selection::new(SelectionType::Simple, point, Side::Left));
                    } else if let Some(selection) = &mut term.selection {
                        selection.update(point, Side::Right);
                    }
                    dirty = true;
                }
                Control::SelectAll => {
                    let mut selection = Selection::new(SelectionType::Simple, Point::new(term.topmost_line(), Column(0)), Side::Left);
                    selection.update(Point::new(term.bottommost_line(), term.last_column()), Side::Right);
                    term.selection = Some(selection);
                    dirty = true;
                }
                Control::Copy(tx) => {
                    let value = term
                        .selection_to_string()
                        .filter(|text| text.len() <= 256 * 1024);
                    let _ = tx.try_send(value);
                }
            }
        }
        for _ in 0..8 {
            match master.read(&mut buffer) {
                Ok(0) => {
                    done = true;
                    break;
                }
                Ok(n) => {
                    parser.advance(&mut term, &buffer[..n]);
                    dirty = true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    if e.raw_os_error() != Some(libc::EIO) {
                        status = format!("PTY: {e}");
                    }
                    done = true;
                    break;
                }
            }
        }
        {
            let mut responses = writes.lock().unwrap_or_else(|e| e.into_inner());
            while let Some(bytes) = responses.pop_front() {
                if pending_bytes + bytes.len() <= WRITE_BUDGET {
                    pending_bytes += bytes.len();
                    input.push_back((bytes, false));
                }
            }
        }
        while let Some((bytes, user)) = input.front() {
            match master.write(&bytes[input_offset..]) {
                Ok(0) => break,
                Ok(n) => {
                    input_offset += n;
                    pending_bytes -= n;
                    if *user {
                        shared.queued.fetch_sub(n, Ordering::AcqRel);
                    }
                    if input_offset == bytes.len() {
                        input.pop_front();
                        input_offset = 0;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        if dirty
            && shared.visible.load(Ordering::Acquire)
            && (done || published.elapsed() >= Duration::from_millis(16))
        {
            publish(
                &term,
                size,
                &shared,
                if status.is_empty() {
                    "Running"
                } else {
                    &status
                },
                true,
            );
            dirty = false;
            published = Instant::now();
        }
        // Hidden tabs still publish title changes, without rendering their grid.
        if dirty && !shared.visible.load(Ordering::Acquire) {
            let title = shared.metadata.lock().unwrap_or_else(|e| e.into_inner()).0.clone();
            let mut frame = shared.frame.lock().unwrap_or_else(|e| e.into_inner());
            if frame.title != title {
                Arc::make_mut(&mut frame).title = title;
                let _ = shared.wake.try_send(());
            }
        }
        if done {
            break;
        }
        let mut poll = [
            libc::pollfd {
                fd: master.as_raw_fd(),
                events: libc::POLLIN | if input.is_empty() { 0 } else { libc::POLLOUT },
                revents: 0,
            },
            libc::pollfd {
                fd: signal.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let timeout = if dirty && shared.visible.load(Ordering::Acquire) {
            16
        } else {
            -1
        };
        unsafe {
            libc::poll(poll.as_mut_ptr(), 2, timeout);
        }
        while signal.read(&mut buffer).is_ok_and(|n| n > 0) {}
    }
    let stopped = shared.stopped.load(Ordering::Acquire);
    // Retire PTY IO before waiting for shell teardown. The unreaped session
    // leader still anchors process ownership while its background jobs stop.
    drop(master);
    // Cleanup happens before publishing settled, and only targets this session.
    drop(child);
    publish(
        &term,
        size,
        &shared,
        if stopped {
            "Stopped"
        } else if status.is_empty() {
            "Exited"
        } else {
            &status
        },
        false,
    );
    drop(shared.settled);
}
fn publish(term: &Term<Proxy>, size: Size, shared: &Shared, status: &str, running: bool) {
    let mut frame = Frame::empty(size);
    frame.status = status.into();
    frame.title = shared
        .metadata
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .0
        .clone();
    frame.running = running;
    let content = term.renderable_content();
    frame.display_offset = content.display_offset;
    frame.app_cursor = content.mode.contains(TermMode::APP_CURSOR);
    frame.bracketed_paste = content.mode.contains(TermMode::BRACKETED_PASTE);
    frame.alternate = content.mode.contains(TermMode::ALT_SCREEN);
    if content.cursor.shape != CursorShape::Hidden
        && let Some(point) = point_to_viewport(content.display_offset, content.cursor.point)
    {
        frame.cursor = Some((point.line, point.column.0));
    }
    for indexed in content.display_iter {
        let Some(point) = point_to_viewport(content.display_offset, indexed.point) else {
            continue;
        };
        if point.line >= size.lines
            || indexed
                .cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let cell = indexed.cell;
        let mut fg = color(cell.fg, content.colors);
        let mut bg = if cell.bg == Color::Named(NamedColor::Background) {
            None
        } else {
            Some(color(cell.bg, content.colors))
        };
        if cell.flags.contains(Flags::INVERSE) {
            let old = fg;
            fg = bg.unwrap_or(0x202326);
            bg = Some(old);
        }
        if content
            .selection
            .as_ref()
            .is_some_and(|selection| selection.contains(indexed.point))
        {
            bg = Some(0x365a78);
        }
        let bold = cell.flags.contains(Flags::BOLD);
        let italic = cell.flags.contains(Flags::ITALIC);
        let underline = cell.flags.intersects(Flags::ALL_UNDERLINES);
        let strike = cell.flags.contains(Flags::STRIKEOUT);
        let columns = if cell.flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        };
        let row = &mut frame.rows[point.line];
        if row.last().is_none_or(|last| {
            last.column + last.columns != point.column.0
                || last.fg != fg
                || last.bg != bg
                || last.bold != bold
                || last.italic != italic
                || last.underline != underline
                || last.strike != strike
        }) {
            row.push(Cell {
                column: point.column.0,
                columns: 0,
                text: String::new(),
                fg,
                bg,
                bold,
                italic,
                underline,
                strike,
            });
        }
        if let Some(run) = row.last_mut() {
            run.columns += columns;
            run.text.push(if cell.flags.contains(Flags::HIDDEN) {
                ' '
            } else {
                cell.c
            });
            if let Some(chars) = cell.zerowidth() {
                run.text.extend(chars.iter().take(16));
            }
        }
    }
    *shared.frame.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(frame);
    let _ = shared.wake.try_send(());
}
fn color(color: Color, colors: &Colors) -> u32 {
    let index = match color {
        Color::Spec(rgb) => return ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32,
        Color::Indexed(i) => i as usize,
        Color::Named(i) => i as usize,
    };
    if let Some(rgb) = colors[index] {
        return ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32;
    }
    const PALETTE: [u32; 16] = [
        0x202326, 0xe58c8c, 0xa4cfa1, 0xe1c788, 0x91b8e8, 0xc8a4df, 0x91ced4, 0xd6d9df, 0x666c76,
        0xf5a0a0, 0xb6e1b3, 0xf2d8a0, 0xa4cafd, 0xddb9ef, 0xa8e3e7, 0xf5f6f8,
    ];
    match index {
        0..=15 => PALETTE[index],
        16..=231 => {
            let n = index - 16;
            let level = |v| if v == 0 { 0 } else { 55 + 40 * v };
            ((level(n / 36) as u32) << 16) | ((level(n / 6 % 6) as u32) << 8) | level(n % 6) as u32
        }
        232..=255 => {
            let n = (8 + (index - 232) * 10) as u32;
            n << 16 | n << 8 | n
        }
        257 => 0x202326,
        _ => 0xd6d9df,
    }
}
fn color_value(value: Color) -> u32 {
    color(value, &Colors::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("code-pty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    fn text(frame: &Frame) -> String {
        frame
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.text.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn wait(terminal: &Terminal, needle: &str) -> Arc<Frame> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let frame = terminal.snapshot();
            if text(&frame).contains(needle) {
                return frame;
            }
            assert!(
                Instant::now() < deadline,
                "Terminal did not show {needle}: {}",
                text(&frame)
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn settled(terminal: &Terminal) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !terminal.is_settled() {
            assert!(Instant::now() < deadline, "Terminal worker did not settle");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn canceled_terminal_startup_never_executes_the_shell() {
        let root = root();
        let result = Terminal::spawn_command_cancellable(
            &root,
            Path::new("/bin/sh"),
            &["-c", "touch must-not-run"],
            Size::new(80, 8),
            Arc::new(AtomicBool::new(true)),
        );
        assert!(result.is_err());
        assert!(!root.join("must-not-run").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn actual_pty_accepts_unicode_input_and_reports_real_resized_window() {
        let root = root();
        let terminal=Terminal::spawn_command(&root,Path::new("/bin/sh"),&["-c","test -t 0 && test -t 1 || exit 9; printf 'PTY_READY\\r\\n'; read line; printf 'RECEIVED:%s\\r\\n' \"$line\"; stty size; read finish"],Size::new(80,8)).unwrap();
        wait(&terminal, "PTY_READY");
        terminal.resize(Size::new(120, 20)).unwrap();
        terminal.input("native🦀\r".as_bytes().to_vec()).unwrap();
        let frame = wait(&terminal, "20 120");
        assert!(text(&frame).contains("RECEIVED:native🦀"));
        assert_eq!(frame.size, Size::new(120, 20));
        terminal.stop();
        settled(&terminal);
        assert!(!terminal.snapshot().running);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stop_reaps_owned_pty_shell_and_its_background_process() {
        let root = root();
        let terminal=Terminal::spawn_command(&root,Path::new("/bin/sh"),&["-c","set -m; sleep 30 & child=$!; printf '%s' \"$child\" > child.next; mv child.next child.pid; printf 'BACKGROUND_READY\\r\\n'; wait"],Size::new(80,8)).unwrap();
        wait(&terminal, "BACKGROUND_READY");
        let pid: i32 = std::fs::read_to_string(root.join("child.pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
        terminal.stop();
        settled(&terminal);
        let deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "Owned background job survived terminal Stop"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn emulator_preserves_split_ansi_unicode_alt_screen_and_bounded_scrollback() {
        let size = Size::new(80, 8);
        let writes = Arc::new(Mutex::new(VecDeque::new()));
        let mut term = Term::new(
            Config {
                scrolling_history: HISTORY_LINES,
                osc52: Osc52::Disabled,
                ..Default::default()
            },
            &size,
            Proxy {
                writes,
                metadata: Arc::new(Mutex::new(("Terminal".into(), size))),
            },
        );
        let mut parser: Processor = Processor::new();
        for byte in "\x1b[2J\x1b[H日本語🦀\x1b[31mRED\x1b[0m".as_bytes() {
            parser.advance(&mut term, &[*byte]);
        }
        let event = |term: &Term<Proxy>| {
            let (tx, _) = async_channel::bounded(1);
            let (settled, _) = async_channel::bounded(1);
            let shared = Shared {
                stopped: Arc::new(AtomicBool::new(false)),
                visible: Arc::new(AtomicBool::new(true)),
                queued: Arc::new(AtomicUsize::new(0)),
                frame: Arc::new(Mutex::new(Arc::new(Frame::empty(size)))),
                wake: tx,
                settled,
                metadata: Arc::new(Mutex::new(("Terminal".into(), size))),
            };
            publish(term, size, &shared, "Running", true);
            shared.frame.lock().unwrap().clone()
        };
        let frame = event(&term);
        assert!(text(&frame).contains("日本語🦀RED"));
        assert!(
            frame
                .rows
                .iter()
                .flatten()
                .any(|cell| cell.text.contains("RED") && cell.fg == 0xe58c8c)
        );
        parser.advance(&mut term, b"\x1b[?1049hALTERNATE");
        assert!(event(&term).alternate);
        assert!(text(&event(&term)).contains("ALTERNATE"));
        parser.advance(&mut term, b"\x1b[?1049l");
        assert!(!event(&term).alternate);
        assert!(text(&event(&term)).contains("日本語🦀RED"));
        for _ in 0..10_000 {
            parser.advance(&mut term, b"bounded history\r\n");
        }
        assert!(term.grid().total_lines() <= HISTORY_LINES + size.lines);
        let frame = event(&term);
        assert_eq!(frame.rows.len(), size.lines);
        assert!(frame.rows.iter().map(Vec::len).sum::<usize>() <= size.lines * size.columns);
        assert!(Size::new(usize::MAX, usize::MAX) == Size::new(512, 256));
    }
    #[test]
    fn output_wakes_are_coalesced_and_large_input_is_rejected() {
        let root = root();
        let terminal=Terminal::spawn_command(&root,Path::new("/bin/sh"),&["-c","printf 'HIDE_READY\\r\\n'; read first; i=0; while [ $i -lt 5000 ]; do printf 'frame %s\\r\\n' \"$i\"; i=$((i+1)); done; printf 'FLOOD_DONE\\r\\n'; touch flood.done; read line"],Size::new(80,8)).unwrap();
        wait(&terminal, "HIDE_READY");
        terminal.set_visible(false);
        while terminal.wake.try_recv().is_ok() {}
        let hidden = terminal.snapshot();
        terminal.input(b"go\r".to_vec()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("flood.done").is_file() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            Arc::ptr_eq(&hidden, &terminal.snapshot()),
            "Hidden output must not allocate new UI frames"
        );
        assert!(terminal.wake.is_empty());
        terminal.set_visible(true);
        wait(&terminal, "FLOOD_DONE");
        assert!(terminal.wake.len() <= 1);
        assert!(terminal.input(vec![b'x'; INPUT_LIMIT + 1]).is_err());
        terminal.stop();
        settled(&terminal);
        std::fs::remove_dir_all(root).unwrap();
    }
}
