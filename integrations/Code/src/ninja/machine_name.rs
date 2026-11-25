//! The friendly hardware name without launching system_profiler on the UI thread.
//! Apple's device-tree value is the same product name (including its optional
//! " xxx" suffix) exposed by the hardware report. Missing values use "This Mac".
#[cfg(target_os = "macos")]
pub(super) fn read() -> Option<String> {
    use std::ffi::{c_char, c_void};
    // Match the opaque CF handle declarations used by the existing native
    // image/PDF bridges; pointer mutability does not change the C ABI.
    type Ref = *mut c_void;
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IORegistryEntryFromPath(port: u32, path: *const c_char) -> u32;
        fn IORegistryEntryCreateCFProperty(entry: u32, key: Ref, allocator: Ref, options: u32) -> Ref;
        fn IOObjectRelease(object: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(allocator: Ref, bytes: *const c_char, encoding: u32) -> Ref;
        fn CFGetTypeID(value: Ref) -> usize;
        fn CFDataGetTypeID() -> usize;
        fn CFDataGetLength(data: Ref) -> isize;
        fn CFDataGetBytePtr(data: Ref) -> *const u8;
        fn CFRelease(value: Ref);
    }
    // All handles are released before returning; no registry object or CF value
    // is retained by the cached String in local_machine(). The value is tiny,
    // bounded and native: no subprocess, pipe, polling or two-second timeout.
    unsafe {
        let entry = IORegistryEntryFromPath(0, c"IODeviceTree:/product".as_ptr());
        if entry == 0 { return None; }
        let key = CFStringCreateWithCString(std::ptr::null_mut(), c"product-name".as_ptr(), 0x08000100);
        if key.is_null() { IOObjectRelease(entry); return None; }
        let value = IORegistryEntryCreateCFProperty(entry, key, std::ptr::null_mut(), 0);
        CFRelease(key);
        IOObjectRelease(entry);
        if value.is_null() { return None; }
        let name = if CFGetTypeID(value) == CFDataGetTypeID() {
            let len = CFDataGetLength(value);
            let bytes = CFDataGetBytePtr(value);
            if (1..=256).contains(&len) && !bytes.is_null() {
                decode(std::slice::from_raw_parts(bytes, len as usize))
            } else { None }
        } else { None };
        CFRelease(value);
        name
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn read() -> Option<String> { None }

#[cfg(any(target_os = "macos", test))]
fn decode(bytes: &[u8]) -> Option<String> {
    let bytes = bytes.split(|byte| *byte == 0).next()?;
    let name = std::str::from_utf8(bytes).ok()?.trim().trim_end_matches(" xxx");
    (!name.is_empty() && !name.chars().any(char::is_control)).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn friendly_name_handles_native_suffix_and_invalid_metadata() {
        assert_eq!(decode(b"MacBook Air xxx\0"), Some("MacBook Air".into()));
        assert_eq!(decode(b"iMac\0padding"), Some("iMac".into()));
        assert_eq!(decode(b"\0"), None);
        assert_eq!(decode(b"Mac\nBook\0"), None);
        assert_eq!(decode(&[0xff, 0]), None);
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_product_name_matches_mac_hardware() {
        let name = read().expect("Mac product name is available in the device tree");
        assert!(!name.is_empty());
        assert!(!name.ends_with(" xxx"));
        let machine = super::super::local_machine();
        assert_eq!(machine.name, format!("This Mac · {name}"));
    }
}
