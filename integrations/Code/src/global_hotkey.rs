//! A user-recorded, persistable global shortcut. Carbon delivers one bounded
//! wake per press; no event tap, Accessibility permission, or polling loop.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shortcut {
    pub key: String,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub command: bool,
    /// Preserve the actual key pressed on non-ANSI layouts. Older/default
    /// configurations resolve their key label through the ANSI fallback table.
    pub key_code: Option<u16>,
}
impl Default for Shortcut {
    fn default() -> Self {
        Self {
            key: "j".into(),
            control: false,
            alt: false,
            shift: false,
            command: true,
            key_code: None,
        }
    }
}
impl Shortcut {
    pub fn from_keystroke(stroke: &gpui::Keystroke) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !stroke.modifiers.function,
            "Choose a shortcut without the Fn key."
        );
        let candidate = Self {
            key: stroke.key.clone(),
            control: stroke.modifiers.control,
            alt: stroke.modifiers.alt,
            shift: stroke.modifiers.shift,
            command: stroke.modifiers.platform,
            key_code: None,
        }
        .canonical();
        candidate.validate()?;
        Ok(candidate)
    }
    fn canonical(mut self) -> Self {
        self.key = match self.key.to_ascii_lowercase().as_str() {
            " " | "spacebar" => "space".into(),
            "return" => "enter".into(),
            "esc" => "escape".into(),
            "arrowup" => "up".into(),
            "arrowdown" => "down".into(),
            "arrowleft" => "left".into(),
            "arrowright" => "right".into(),
            key => key.to_owned(),
        };
        self
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.control || self.alt || self.command,
            "Include Command, Control, or Option in the shortcut."
        );
        anyhow::ensure!(self.key.len() <= 24, "This shortcut key is not supported.");
        anyhow::ensure!(
            ansi_key_code(&self.clone().canonical().key).is_some(),
            "Choose a letter, number, function key, arrow, or Space."
        );
        if let Some(code) = self.key_code {
            anyhow::ensure!(
                known_key_code(code),
                "This physical shortcut key is not supported."
            );
        }
        // Standard escape/close/app-switch bindings should remain available.
        anyhow::ensure!(
            !self.command
                || self.control
                || self.alt
                || !matches!(self.key.as_str(), "q" | "w" | "tab" | "space"),
            "Choose a shortcut other than a standard macOS app shortcut."
        );
        Ok(())
    }
    /// Invalid or edited restart files fall back to the safe default binding.
    pub fn normalized(self) -> Self {
        let candidate = self.canonical();
        if candidate.validate().is_ok() {
            candidate
        } else {
            Self::default()
        }
    }
    pub fn label(&self) -> String {
        let mut label = String::new();
        if self.control {
            label.push('⌃');
        }
        if self.alt {
            label.push('⌥');
        }
        if self.shift {
            label.push('⇧');
        }
        if self.command {
            label.push('⌘');
        }
        label.push_str(match self.key.as_str() {
            "space" => "Space",
            "enter" => "↩",
            "escape" => "Esc",
            "tab" => "⇥",
            "backspace" => "⌫",
            "delete" => "⌦",
            "up" => "↑",
            "down" => "↓",
            "left" => "←",
            "right" => "→",
            "home" => "↖",
            "end" => "↘",
            "pageup" => "⇞",
            "pagedown" => "⇟",
            key => return format!("{label}{}", key.to_ascii_uppercase()),
        });
        label
    }
    fn code(&self) -> anyhow::Result<u16> {
        self.validate()?;
        self.key_code
            .or_else(|| ansi_key_code(&self.clone().canonical().key))
            .ok_or_else(|| anyhow::anyhow!("This shortcut key is not supported."))
    }
    #[cfg(target_os = "macos")]
    fn modifiers(&self) -> u32 {
        (u32::from(self.command) << 8)
            | (u32::from(self.shift) << 9)
            | (u32::from(self.alt) << 11)
            | (u32::from(self.control) << 12)
    }
    /// Called only while handling the actual native key-down event.
    pub fn capture_physical_key(&mut self) {
        #[cfg(all(target_os = "macos", not(test)))]
        unsafe {
            use objc2::{
                msg_send,
                runtime::{AnyClass, AnyObject},
            };
            if objc2::MainThreadMarker::new().is_none() {
                return;
            }
            let Some(class) = AnyClass::get(c"NSApplication") else {
                return;
            };
            let app: *mut AnyObject = msg_send![class, sharedApplication];
            let event: *mut AnyObject = msg_send![app, currentEvent];
            if event.is_null() {
                return;
            }
            let kind: usize = msg_send![event, type];
            if kind != 10 {
                return;
            } // NSEventTypeKeyDown
            let code: u16 = msg_send![event, keyCode];
            if known_key_code(code) {
                self.key_code = Some(code);
            }
        }
    }
}

fn known_key_code(code: u16) -> bool {
    KEY_CODES.iter().any(|(_, candidate)| *candidate == code)
}
fn ansi_key_code(key: &str) -> Option<u16> {
    KEY_CODES
        .iter()
        .find_map(|(name, code)| (*name == key).then_some(*code))
}
const KEY_CODES: &[(&str, u16)] = &[
    ("a", 0),
    ("s", 1),
    ("d", 2),
    ("f", 3),
    ("h", 4),
    ("g", 5),
    ("z", 6),
    ("x", 7),
    ("c", 8),
    ("v", 9),
    ("b", 11),
    ("q", 12),
    ("w", 13),
    ("e", 14),
    ("r", 15),
    ("y", 16),
    ("t", 17),
    ("1", 18),
    ("2", 19),
    ("3", 20),
    ("4", 21),
    ("6", 22),
    ("5", 23),
    ("=", 24),
    ("9", 25),
    ("7", 26),
    ("-", 27),
    ("8", 28),
    ("0", 29),
    ("]", 30),
    ("o", 31),
    ("u", 32),
    ("[", 33),
    ("i", 34),
    ("p", 35),
    ("enter", 36),
    ("l", 37),
    ("j", 38),
    ("'", 39),
    ("k", 40),
    (";", 41),
    ("\\", 42),
    (",", 43),
    ("/", 44),
    ("n", 45),
    ("m", 46),
    (".", 47),
    ("tab", 48),
    ("space", 49),
    ("`", 50),
    ("backspace", 51),
    ("escape", 53),
    ("f17", 64),
    ("f18", 79),
    ("f19", 80),
    ("f20", 90),
    ("f5", 96),
    ("f6", 97),
    ("f7", 98),
    ("f3", 99),
    ("f8", 100),
    ("f9", 101),
    ("f11", 103),
    ("f13", 105),
    ("f16", 106),
    ("f14", 107),
    ("f10", 109),
    ("f12", 111),
    ("f15", 113),
    ("home", 115),
    ("pageup", 116),
    ("delete", 117),
    ("f4", 118),
    ("end", 119),
    ("f2", 120),
    ("pagedown", 121),
    ("f1", 122),
    ("left", 123),
    ("right", 124),
    ("down", 125),
    ("up", 126),
];

pub struct GlobalHotkey {
    pub events: async_channel::Receiver<()>,
    sender: async_channel::Sender<()>,
    shortcut: Shortcut,
    #[cfg(target_os = "macos")]
    registration: Option<mac::Registration>,
    _main_thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl GlobalHotkey {
    /// Main-thread only. Registration failure is an ordinary settings error.
    pub fn new(shortcut: Shortcut) -> anyhow::Result<Self> {
        let (sender, events) = async_channel::bounded(1);
        let shortcut = shortcut.canonical();
        shortcut.validate()?;
        #[cfg(target_os = "macos")]
        {
            let registration = mac::Registration::new(&shortcut, sender.clone())?;
            Ok(Self {
                events,
                sender,
                shortcut,
                registration: Some(registration),
                _main_thread: std::marker::PhantomData,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (sender, events, shortcut);
            anyhow::bail!("Global Quick Chat shortcuts currently require macOS.");
        }
    }
    /// Register the candidate first. Conflicts leave the previous shortcut live.
    pub fn rebind(&mut self, candidate: Shortcut) -> anyhow::Result<()> {
        let candidate = candidate.canonical();
        candidate.validate()?;
        if self.shortcut.code()? == candidate.code()?
            && self.shortcut.control == candidate.control
            && self.shortcut.alt == candidate.alt
            && self.shortcut.command == candidate.command
            && self.shortcut.shift == candidate.shift
        {
            self.shortcut = candidate;
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        {
            let replacement = mac::Registration::new(&candidate, self.sender.clone())?;
            self.registration = Some(replacement);
            self.shortcut = candidate;
            while self.events.try_recv().is_ok() {}
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            anyhow::bail!("Global Quick Chat shortcuts currently require macOS.")
        }
    }
    pub fn stop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            self.registration = None;
        }
        self.sender.close();
    }
}
impl Drop for GlobalHotkey {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::{
        ffi::c_void,
        ptr,
        sync::atomic::{AtomicU32, Ordering},
    };
    type Ref = *mut c_void;
    #[repr(C)]
    struct EventType {
        class: u32,
        kind: u32,
    }
    #[repr(C)]
    struct HotkeyId {
        signature: u32,
        id: u32,
    }
    struct Callback {
        sender: async_channel::Sender<()>,
        id: u32,
    }
    const SIGNATURE: u32 = u32::from_be_bytes(*b"CoQC");
    const KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
    const NOT_HANDLED: i32 = -9874;
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn GetApplicationEventTarget() -> Ref;
        fn InstallEventHandler(
            target: Ref,
            handler: unsafe extern "C" fn(Ref, Ref, Ref) -> i32,
            count: usize,
            types: *const EventType,
            data: Ref,
            result: *mut Ref,
        ) -> i32;
        fn RemoveEventHandler(handler: Ref) -> i32;
        fn RegisterEventHotKey(
            code: u32,
            modifiers: u32,
            id: HotkeyId,
            target: Ref,
            options: u32,
            result: *mut Ref,
        ) -> i32;
        fn UnregisterEventHotKey(hotkey: Ref) -> i32;
        fn GetEventParameter(
            event: Ref,
            name: u32,
            kind: u32,
            actual_kind: *mut u32,
            size: usize,
            actual_size: *mut usize,
            result: Ref,
        ) -> i32;
    }
    pub(super) struct Registration {
        hotkey: Ref,
        handler: Ref,
        callback: Box<Callback>,
    }
    impl Registration {
        pub(super) fn new(
            shortcut: &Shortcut,
            sender: async_channel::Sender<()>,
        ) -> anyhow::Result<Self> {
            anyhow::ensure!(
                objc2::MainThreadMarker::new().is_some(),
                "Configure Quick Chat shortcuts on the app's main thread."
            );
            static NEXT_ID: AtomicU32 = AtomicU32::new(1);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let mut state = Self {
                hotkey: ptr::null_mut(),
                handler: ptr::null_mut(),
                callback: Box::new(Callback { sender, id }),
            };
            // Carbon owns neither this Box nor the function pointer. Handler
            // removal is synchronous on this same thread before Box destruction.
            unsafe {
                let target = GetApplicationEventTarget();
                let status = InstallEventHandler(
                    target,
                    callback,
                    1,
                    &EventType {
                        class: KEYBOARD,
                        kind: 5,
                    },
                    (&mut *state.callback as *mut Callback).cast(),
                    &mut state.handler,
                );
                anyhow::ensure!(
                    status == 0,
                    "Could not listen for the Quick Chat shortcut (macOS {status})."
                );
                let status = RegisterEventHotKey(
                    shortcut.code()? as u32,
                    shortcut.modifiers(),
                    HotkeyId {
                        signature: SIGNATURE,
                        id,
                    },
                    target,
                    0,
                    &mut state.hotkey,
                );
                if status == -9878 {
                    anyhow::bail!(
                        "{} is already used by another app or macOS. Choose another shortcut.",
                        shortcut.label()
                    );
                }
                anyhow::ensure!(
                    status == 0,
                    "macOS could not register {} (error {status}). Choose another shortcut.",
                    shortcut.label()
                );
            }
            Ok(state)
        }
    }
    impl Drop for Registration {
        fn drop(&mut self) {
            unsafe {
                if !self.hotkey.is_null() {
                    let _ = UnregisterEventHotKey(self.hotkey);
                }
                if !self.handler.is_null() {
                    let _ = RemoveEventHandler(self.handler);
                }
            }
        }
    }
    unsafe extern "C" fn callback(_: Ref, event: Ref, data: Ref) -> i32 {
        if data.is_null() {
            return NOT_HANDLED;
        }
        let mut id = HotkeyId {
            signature: 0,
            id: 0,
        };
        let status = unsafe {
            GetEventParameter(
                event,
                u32::from_be_bytes(*b"----"),
                u32::from_be_bytes(*b"hkid"),
                ptr::null_mut(),
                std::mem::size_of::<HotkeyId>(),
                ptr::null_mut(),
                (&mut id as *mut HotkeyId).cast(),
            )
        };
        let state = unsafe { &*(data as *const Callback) };
        if status != 0 || id.signature != SIGNATURE || id.id != state.id {
            return NOT_HANDLED;
        }
        let _ = state.sender.try_send(());
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcut_roundtrip_and_validation() {
        let shortcut = Shortcut::default();
        assert_eq!(shortcut.label(), "⌘J");
        assert_eq!(shortcut.code().unwrap(), 38);
        assert_eq!(
            serde_json::from_str::<Shortcut>(&serde_json::to_string(&shortcut).unwrap()).unwrap(),
            shortcut
        );
        assert!(
            Shortcut {
                command: false,
                ..shortcut.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            Shortcut {
                key: "made-up".into(),
                ..shortcut.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            Shortcut {
                key_code: Some(255),
                ..shortcut.clone()
            }
            .validate()
            .is_err()
        );
        assert_eq!(
            Shortcut {
                key: "invalid".into(),
                ..shortcut
            }
            .normalized(),
            Shortcut::default()
        );
    }
    #[test]
    fn recorder_chord_keeps_all_modifiers_and_rejects_fn() {
        let mut stroke = gpui::Keystroke::parse("ctrl-alt-shift-cmd-k").unwrap();
        let recorded = Shortcut::from_keystroke(&stroke).unwrap();
        assert_eq!(recorded.label(), "⌃⌥⇧⌘K");
        stroke.modifiers.function = true;
        assert!(Shortcut::from_keystroke(&stroke).is_err());
    }
    #[test]
    fn standard_app_commands_are_not_stolen() {
        for key in ["q", "w", "tab", "space"] {
            assert!(
                Shortcut {
                    key: key.into(),
                    control: false,
                    alt: false,
                    command: true,
                    ..Shortcut::default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            Shortcut::from_keystroke(&gpui::Keystroke::parse("ctrl-alt-space").unwrap()).is_ok()
        );
    }
}
