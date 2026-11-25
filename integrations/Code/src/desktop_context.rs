//! The provider runs in the host user's graphical session. Read that session's
//! current state directly instead of interpreting blank captures as a signal.
#[cfg(target_os = "macos")]
pub(crate) fn unavailable() -> Option<&'static str> {
    use objc2_foundation::NSString;
    use std::ffi::c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGSessionCopyCurrentDictionary() -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDictionaryGetValue(dictionary: *const c_void, key: *const c_void) -> *const c_void;
        fn CFGetTypeID(value: *const c_void) -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFBooleanGetValue(value: *const c_void) -> u8;
        fn CFRelease(value: *mut c_void);
    }

    // The copied dictionary owns every value until the single CFRelease.
    unsafe {
        let dictionary = CGSessionCopyCurrentDictionary();
        if dictionary.is_null() {
            return Some("The host has no graphical session for Kai.");
        }
        let lock_key = NSString::from_str("CGSSessionScreenIsLocked");
        let lock = CFDictionaryGetValue(dictionary, (&*lock_key as *const NSString).cast());
        let locked = !lock.is_null() && CFGetTypeID(lock) == CFBooleanGetTypeID() && CFBooleanGetValue(lock) != 0;
        CFRelease(dictionary.cast_mut());
        locked.then_some("The host Mac’s desktop is locked. Unlock it to use its apps or capture its desktop.")
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn unavailable() -> Option<&'static str> { None }
