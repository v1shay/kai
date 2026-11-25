//! Worker-local binding for the bundled libghostty-vt adapter.
use super::terminal::{Cell, Frame, Proxy, Size};
use anyhow::{Context, Result};
use std::ffi::{CStr, CString, c_void};
use std::os::unix::ffi::OsStrExt;
use std::ptr::NonNull;
use std::sync::OnceLock;

type Bytes = unsafe extern "C" fn(*mut c_void, *const u8, usize);
type Emit = unsafe extern "C" fn(*mut c_void, usize, usize, usize, *const u8, usize, u32, u32, u8);

#[repr(C)]
struct Bridge {
    create:
        unsafe extern "C" fn(u16, u16, u16, u16, usize, *mut c_void, Bytes, Bytes) -> *mut c_void,
    destroy: unsafe extern "C" fn(*mut c_void),
    resize: unsafe extern "C" fn(*mut c_void, u16, u16, u16, u16) -> bool,
    write: unsafe extern "C" fn(*mut c_void, *const u8, usize),
    scroll: unsafe extern "C" fn(*mut c_void, i32),
    select_cells: unsafe extern "C" fn(*mut c_void, u16, u16, bool),
    select_all: unsafe extern "C" fn(*mut c_void),
    copy: unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> usize,
    snapshot: unsafe extern "C" fn(*mut c_void, *mut Viewport, *mut c_void, Emit),
}

#[repr(C)]
#[derive(Default)]
struct Viewport {
    display_offset: usize,
    cursor_x: u16,
    cursor_y: u16,
    cursor: bool,
    app_cursor: bool,
    bracketed_paste: bool,
    alternate: bool,
}

fn library() -> Result<&'static Bridge> {
    static BRIDGE: OnceLock<Result<&'static Bridge, String>> = OnceLock::new();
    BRIDGE
        .get_or_init(|| {
            let path = std::env::current_exe()
                .map_err(|error| error.to_string())?
                .with_file_name("../Resources/libKaiTerminalGhostty.dylib");
            let path =
                CString::new(path.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
            // Keep the signed library mapped for the process lifetime. The bridge
            // table and its function pointers are borrowed from that mapping.
            unsafe {
                let handle = libc::dlopen(path.as_ptr(), libc::RTLD_LOCAL | libc::RTLD_NOW);
                if handle.is_null() {
                    let error = libc::dlerror();
                    return Err(if error.is_null() {
                        "Library unavailable".into()
                    } else {
                        CStr::from_ptr(error).to_string_lossy().into_owned()
                    });
                }
                let symbol = libc::dlsym(handle, c"kai_terminal_ghostty_v1".as_ptr());
                if !symbol.is_null() {
                    let bridge: unsafe extern "C" fn() -> *const Bridge =
                        std::mem::transmute(symbol);
                    if let Some(table) = bridge().as_ref() {
                        return Ok(table);
                    }
                }
                libc::dlclose(handle);
                Err("Incompatible terminal library".into())
            }
        })
        .as_ref()
        .copied()
        .map_err(|error| anyhow::anyhow!("Ghostty VT is unavailable: {error}"))
}

pub(super) struct Ghostty {
    bridge: &'static Bridge,
    engine: NonNull<c_void>,
    _proxy: Box<Proxy>,
}

// The engine and its callback context have a single owner. Moving them from
// the launch task to the PTY worker never permits concurrent calls.
unsafe impl Send for Ghostty {}

impl Ghostty {
    pub(super) fn new(size: Size, history: usize, proxy: Proxy) -> Result<Self> {
        let bridge = library()?;
        let mut proxy = Box::new(proxy);
        let engine = unsafe {
            (bridge.create)(
                size.columns as u16,
                size.lines as u16,
                size.cell_width,
                size.cell_height,
                history,
                (&mut *proxy as *mut Proxy).cast(),
                send_reply,
                set_title,
            )
        };
        Ok(Self {
            bridge,
            engine: NonNull::new(engine).context("Could not initialize Ghostty VT")?,
            _proxy: proxy,
        })
    }

    pub(super) fn resize(&mut self, size: Size) -> bool {
        unsafe {
            (self.bridge.resize)(
                self.engine.as_ptr(),
                size.columns as u16,
                size.lines as u16,
                size.cell_width,
                size.cell_height,
            )
        }
    }

    pub(super) fn write(&mut self, data: &[u8]) {
        unsafe {
            (self.bridge.write)(self.engine.as_ptr(), data.as_ptr(), data.len());
        }
    }

    pub(super) fn scroll(&mut self, delta: i32) {
        unsafe {
            (self.bridge.scroll)(self.engine.as_ptr(), delta);
        }
    }

    pub(super) fn select(&mut self, row: usize, col: usize, start: bool) {
        unsafe {
            (self.bridge.select_cells)(self.engine.as_ptr(), row as u16, col as u16, start);
        }
    }

    pub(super) fn select_all(&mut self) {
        unsafe {
            (self.bridge.select_all)(self.engine.as_ptr());
        }
    }

    pub(super) fn copy(&self) -> Option<String> {
        let mut bytes = vec![0; 256 * 1024];
        let len =
            unsafe { (self.bridge.copy)(self.engine.as_ptr(), bytes.as_mut_ptr(), bytes.len()) };
        if len == 0 || len > bytes.len() {
            return None;
        }
        bytes.truncate(len);
        String::from_utf8(bytes).ok()
    }

    pub(super) fn snapshot(&mut self, frame: &mut Frame) {
        let mut viewport = Viewport::default();
        unsafe {
            (self.bridge.snapshot)(
                self.engine.as_ptr(),
                &mut viewport,
                (frame as *mut Frame).cast(),
                emit_cell,
            );
        }
        frame.cursor = viewport
            .cursor
            .then_some((viewport.cursor_y.into(), viewport.cursor_x.into()));
        frame.display_offset = viewport.display_offset;
        frame.app_cursor = viewport.app_cursor;
        frame.bracketed_paste = viewport.bracketed_paste;
        frame.alternate = viewport.alternate;
    }
}

impl Drop for Ghostty {
    fn drop(&mut self) {
        unsafe {
            (self.bridge.destroy)(self.engine.as_ptr());
        }
    }
}

unsafe extern "C" fn send_reply(proxy: *mut c_void, data: *const u8, len: usize) {
    if data.is_null() || len > 8192 {
        return;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    unsafe { &*proxy.cast::<Proxy>() }.reply(bytes);
}

unsafe extern "C" fn set_title(proxy: *mut c_void, data: *const u8, len: usize) {
    let bytes = if data.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len.min(1024)) }
    };
    unsafe { &*proxy.cast::<Proxy>() }.set_title(&String::from_utf8_lossy(bytes));
}

unsafe extern "C" fn emit_cell(
    out: *mut c_void,
    y: usize,
    column: usize,
    columns: usize,
    data: *const u8,
    len: usize,
    fg: u32,
    bg: u32,
    flags: u8,
) {
    let frame = unsafe { &mut *out.cast::<Frame>() };
    let Some(row) = frame.rows.get_mut(y) else {
        return;
    };
    let text = unsafe { std::slice::from_raw_parts(data, len) };
    let text = std::str::from_utf8(text).unwrap_or("�");
    let bg = (bg != u32::MAX).then_some(bg);
    let (bold, italic, underline, strike) = (
        flags & 1 != 0,
        flags & 2 != 0,
        flags & 4 != 0,
        flags & 8 != 0,
    );
    if row.last().is_none_or(|last| {
        last.column + last.columns != column
            || (
                last.fg,
                last.bg,
                last.bold,
                last.italic,
                last.underline,
                last.strike,
            ) != (fg, bg, bold, italic, underline, strike)
    }) {
        row.push(Cell {
            column,
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
        run.text.push_str(text);
    }
}
