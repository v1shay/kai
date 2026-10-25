//! Native chrome adapted from Monocode's MIT-licensed src-tauri/src/macos.rs.
//! Reuses GPUI's backing view; never replaces its renderer or event host.
#[cfg(target_os = "macos")]
mod mac {
    use crate::appearance::Appearance;
    use gpui::Window;
    use objc2::{
        msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject, Bool},
    };
    use objc2_foundation::NSString;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{cell::RefCell, collections::HashMap, sync::OnceLock};

    thread_local! {
        // GPUI calls native chrome on the UI thread. WindowId is checked before
        // any Objective-C messages, so opacity-only updates do no native work.
        static APPLIED_BLUR: RefCell<HashMap<gpui::WindowId, u8>> = RefCell::new(HashMap::new());
        static APPLIED_THEME: RefCell<HashMap<gpui::WindowId, (bool,u32)>> = RefCell::new(HashMap::new());
    }
    struct BlurFunctions {
        set: unsafe extern "C" fn(usize, i32, i32) -> i32,
        connection: unsafe extern "C" fn() -> usize,
    }
    fn blur_functions() -> Option<&'static BlurFunctions> {
        static FUNCTIONS: OnceLock<Option<BlurFunctions>> = OnceLock::new();
        FUNCTIONS
            .get_or_init(|| unsafe {
                let blur = libc::dlsym(
                    libc::RTLD_DEFAULT,
                    c"CGSSetWindowBackgroundBlurRadius".as_ptr(),
                );
                let connection = libc::dlsym(libc::RTLD_DEFAULT, c"CGSMainConnectionID".as_ptr());
                if blur.is_null() || connection.is_null() {
                    return None;
                }
                // Optional symbols use the same signatures as Monocode's macOS
                // adaptation. AppKit's semantic material remains the fallback.
                Some(BlurFunctions {
                    set: std::mem::transmute::<
                        *mut libc::c_void,
                        unsafe extern "C" fn(usize, i32, i32) -> i32,
                    >(blur),
                    connection: std::mem::transmute::<
                        *mut libc::c_void,
                        unsafe extern "C" fn() -> usize,
                    >(connection),
                })
            })
            .as_ref()
    }

    unsafe fn ns_window(window: &Window) -> Option<*mut AnyObject> {
        let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
        else {
            return None;
        };
        // SAFETY: GPUI owns this NSView for the live window; callers run on its UI thread.
        let view = handle.ns_view.as_ptr().cast::<AnyObject>();
        let native: *mut AnyObject = unsafe { msg_send![view, window] };
        (!native.is_null()).then_some(native)
    }
    #[cfg(not(test))]
    pub fn reduced_motion() -> bool {
        // UI-thread read of the system accessibility preference; no polling task.
        unsafe {
            let Some(class) = AnyClass::get(c"NSWorkspace") else {
                return false;
            };
            let workspace: *mut AnyObject = msg_send![class, sharedWorkspace];
            if workspace.is_null() {
                return false;
            }
            let reduced: Bool = msg_send![workspace, accessibilityDisplayShouldReduceMotion];
            reduced.as_bool()
        }
    }
    pub fn configure(window: &Window) {
        set_application_icon();
        // SAFETY: called once on the UI thread after GPUI creates its native window.
        unsafe {
            let Some(native) = ns_window(window) else {
                return;
            };
            for kind in [0usize, 1, 2] {
                let button: *mut AnyObject = msg_send![native, standardWindowButton: kind];
                if !button.is_null() {
                    let _: () = msg_send![button, setHidden: Bool::YES];
                }
            }
            let content: *mut AnyObject = msg_send![native, contentView];
            if !content.is_null() {
                let views: *mut AnyObject = msg_send![content, subviews];
                let count: usize = msg_send![views, count];
                if let Some(effect_class) = AnyClass::get(c"NSVisualEffectView") {
                    for ix in 0..count {
                        let view: *mut AnyObject = msg_send![views, objectAtIndex: ix];
                        let is_effect: Bool = msg_send![view, isKindOfClass: effect_class];
                        if is_effect.as_bool() {
                            let _: () = msg_send![view, setMaterial: 21isize]; // UnderWindowBackground
                            let _: () = msg_send![view, setBlendingMode: 0isize]; // BehindWindow
                            let _: () = msg_send![view, setState: 1isize]; // Active
                        }
                    }
                }
            }
        }
        apply_appearance(window, &Appearance::default());
    }
    /// The caller may use this after load/user changes. An unchanged normalized
    /// radius returns before touching AppKit or resolving private symbols.
    pub fn apply_appearance(window: &Window, appearance: &Appearance) {
        let radius = appearance.normalized().blur_radius;
        let id = gpui::Window::window_handle(window).window_id();
        if APPLIED_BLUR.with(|cache| cache.borrow().get(&id) == Some(&radius)) {
            return;
        }
        // SAFETY: GPUI owns the NSWindow and its effect views. These UI-thread
        // messages adjust only its native backdrop, never application/text alpha.
        unsafe {
            let Some(native) = ns_window(window) else {
                return;
            };
            let number: isize = msg_send![native, windowNumber];
            if number <= 0 {
                return; // A not-yet-attached native window remains retryable.
            }
            APPLIED_BLUR.with(|cache| {
                let mut cache = cache.borrow_mut();
                if cache.len() >= 32 {
                    cache.clear();
                }
                cache.insert(id, radius);
            });
            let custom_blur = blur_functions().is_some_and(|functions| {
                (functions.set)((functions.connection)(), number as i32, radius as i32) == 0
            });
            let content: *mut AnyObject = msg_send![native, contentView];
            let Some(effect_class) = AnyClass::get(c"NSVisualEffectView") else {
                return;
            };
            if content.is_null() {
                return;
            }
            let views: *mut AnyObject = msg_send![content, subviews];
            let count: usize = msg_send![views, count];
            for ix in 0..count {
                let view: *mut AnyObject = msg_send![views, objectAtIndex: ix];
                let is_effect: Bool = msg_send![view, isKindOfClass: effect_class];
                if is_effect.as_bool() {
                    // Keep semantic material fully intact if custom blur is
                    // unavailable. Restore it if a later native call fails.
                    let _: () =
                        msg_send![view, setAlphaValue: if custom_blur { 0.01f64 } else { 1.0f64 }];
                }
            }
        }
    }
    #[cfg(not(test))]
    pub fn apply_theme(window: &mut Window, theme: &crate::theme::ResolvedTheme) {
        let id = gpui::Window::window_handle(window).window_id();
        let signature = (theme.native_transparency, theme.background);
        if APPLIED_THEME.with(|cache| cache.borrow().get(&id) == Some(&signature)) {
            return;
        }
        APPLIED_THEME.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.len() >= 32 {
                cache.clear();
            }
            cache.insert(id, signature);
        });
        window.set_background_appearance(if theme.native_transparency {
            gpui::WindowBackgroundAppearance::Blurred
        } else {
            gpui::WindowBackgroundAppearance::Opaque
        });
        unsafe {
            let Some(native) = ns_window(window) else {
                return;
            };
            let _: () = msg_send![native, setOpaque: Bool::new(!theme.native_transparency)];
            let Some(color_class) = AnyClass::get(c"NSColor") else {
                return;
            };
            let color: *mut AnyObject = if theme.native_transparency {
                msg_send![color_class, clearColor]
            } else {
                msg_send![color_class, colorWithSRGBRed: ((theme.background >> 16) & 255) as f64 / 255.,
                    green: ((theme.background >> 8) & 255) as f64 / 255.,
                    blue: (theme.background & 255) as f64 / 255., alpha: 1f64]
            };
            let _: () = msg_send![native, setBackgroundColor: color];
            let content: *mut AnyObject = msg_send![native, contentView];
            if content.is_null() {
                return;
            }
            let views: *mut AnyObject = msg_send![content, subviews];
            let count: usize = msg_send![views, count];
            if let Some(effect_class) = AnyClass::get(c"NSVisualEffectView") {
                for ix in 0..count {
                    let view: *mut AnyObject = msg_send![views, objectAtIndex: ix];
                    let effect: Bool = msg_send![view, isKindOfClass: effect_class];
                    if effect.as_bool() {
                        let _: () =
                            msg_send![view, setHidden: Bool::new(!theme.native_transparency)];
                    }
                }
            }
        }
    }
    pub fn show_window(window: &gpui::Window) {
        unsafe {
            if let Some(native) = ns_window(window) {
                let _: () = msg_send![native,makeKeyAndOrderFront:std::ptr::null::<AnyObject>()];
            }
        }
    }
    pub fn hide_window(window: &gpui::Window) {
        unsafe {
            if let Some(native) = ns_window(window) {
                let _: () = msg_send![native,orderOut:std::ptr::null::<AnyObject>()];
            }
        }
    }
    fn set_application_icon() {
        // Set the running app's Dock icon explicitly: replacing the ICNS on
        // disk alone can leave Launch Services displaying the previous mark.
        // SAFETY: configure runs on the AppKit UI thread; AppKit retains the
        // icon, while Retained releases our temporary ownership after the call.
        unsafe {
            let (Some(bundle_class), Some(image_class), Some(app_class)) = (
                AnyClass::get(c"NSBundle"),
                AnyClass::get(c"NSImage"),
                AnyClass::get(c"NSApplication"),
            ) else {
                return;
            };
            let bundle: *mut AnyObject = msg_send![bundle_class, mainBundle];
            let url: *mut AnyObject = msg_send![bundle,
                URLForResource: &*NSString::from_str("Code-Bonsai"),
                withExtension: &*NSString::from_str("icns")
            ];
            if url.is_null() {
                return; // Unbundled development/test runs keep the system icon.
            }
            let image: *mut AnyObject = msg_send![image_class, alloc];
            let image: *mut AnyObject = msg_send![image, initWithContentsOfURL: url];
            if let Some(image) = Retained::from_raw(image) {
                let app: *mut AnyObject = msg_send![app_class, sharedApplication];
                let _: () = msg_send![app, setApplicationIconImage: &*image];
            }
        }
    }
    /// Open a validated web citation using macOS's chosen default browser.
    pub fn open_web_url(url: &str) -> Result<(), String> {
        if !matches!(
            crate::citations::classify(url),
            Ok(crate::citations::Citation::Web(_))
        ) {
            return Err("Only HTTP and HTTPS links can open in the browser".into());
        }
        objc2::rc::autoreleasepool(|_| unsafe {
            let workspace_class = AnyClass::get(c"NSWorkspace")
                .ok_or("macOS application services are unavailable")?;
            let string_class =
                AnyClass::get(c"NSString").ok_or("macOS string services are unavailable")?;
            let url_class = AnyClass::get(c"NSURL").ok_or("macOS URL services are unavailable")?;
            let workspace: *mut AnyObject = msg_send![workspace_class, sharedWorkspace];
            let encoded = std::ffi::CString::new(url).map_err(|_| "Invalid web citation")?;
            let string: *mut AnyObject =
                msg_send![string_class,stringWithUTF8String:encoded.as_ptr()];
            let target: *mut AnyObject = msg_send![url_class,URLWithString:string];
            if target.is_null() {
                return Err("This citation has an invalid web address".into());
            }
            let accepted: Bool = msg_send![workspace,openURL:target];
            if accepted.as_bool() {
                Ok(())
            } else {
                Err("The default browser could not open this citation".into())
            }
        })
    }
    #[cfg(not(test))]
    pub fn drag(window: &Window) {
        // SAFETY: invoked synchronously from a native mouse-down on chrome.
        unsafe {
            let Some(native) = ns_window(window) else {
                return;
            };
            let Some(class) = AnyClass::get(c"NSApplication") else {
                return;
            };
            let app: *mut AnyObject = msg_send![class, sharedApplication];
            let event: *mut AnyObject = msg_send![app, currentEvent];
            if !event.is_null() {
                let _: () = msg_send![native, performWindowDragWithEvent: event];
            }
        }
    }
}
#[cfg(all(target_os = "macos", not(test)))]
pub use mac::apply_appearance;
#[cfg(target_os = "macos")]
pub use mac::configure;
#[cfg(target_os = "macos")]
pub use mac::open_web_url;
#[cfg(not(target_os = "macos"))]
pub fn open_web_url(_: &str) -> Result<(), String> {
    Err("Native browser citations require macOS".into())
}

/// Run only after the user presses the account login button. Arguments are
/// passed directly to macOS; no shell parses the generated script's path.
pub fn open_login_script(path: &std::path::Path) -> Result<(), String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if path
        .extension()
        .is_none_or(|extension| extension != "command")
        || !path.is_file()
    {
        return Err("The generated login command is unavailable".into());
    }
    let status = std::process::Command::new("/usr/bin/open")
        .arg("-a")
        .arg("Terminal")
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("Terminal could not open: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("Terminal could not open the login command".into())
    }
}
#[cfg(all(target_os = "macos", not(test)))]
pub use mac::drag;
#[cfg(test)]
pub fn drag(_: &gpui::Window) {}
#[cfg(not(target_os = "macos"))]
pub fn configure(_: &gpui::Window) {}
#[cfg(any(not(target_os = "macos"), test))]
pub fn apply_appearance(_: &gpui::Window, _: &crate::appearance::Appearance) {}
#[cfg(all(not(target_os = "macos"), not(test)))]
pub fn drag(window: &gpui::Window) {
    window.start_window_move();
}

#[cfg(all(target_os = "macos", not(test)))]
pub use mac::reduced_motion;
#[cfg(any(not(target_os = "macos"), test))]
pub fn reduced_motion() -> bool {
    false
}

#[cfg(all(target_os = "macos", not(test)))]
pub use mac::apply_theme;
#[cfg(any(not(target_os = "macos"), test))]
pub fn apply_theme(_: &mut gpui::Window, _: &crate::theme::ResolvedTheme) {}

#[cfg(target_os = "macos")]
pub use mac::{hide_window, show_window};
#[cfg(not(target_os = "macos"))]
pub fn show_window(window: &gpui::Window) {
    window.activate_window();
}
#[cfg(not(target_os = "macos"))]
pub fn hide_window(_: &gpui::Window) {}

#[cfg(test)]
mod citation_tests {
    #[test]
    fn unsupported_browser_links_and_missing_login_scripts_do_not_launch() {
        assert!(super::open_web_url("javascript:alert(1)").is_err());
        assert!(super::open_web_url("file:///tmp/source.rs").is_err());
        assert!(
            super::open_login_script(
                &std::env::temp_dir().join("orbit-missing-login-script.command")
            )
            .is_err()
        );
    }
}
