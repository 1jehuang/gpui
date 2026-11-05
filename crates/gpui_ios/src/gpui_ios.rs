#![cfg(target_os = "ios")]
//! iOS (UIKit) platform implementation for GPUI.
//!
//! Rendering and text come from `gpui_apple` (Metal + CoreText), shared with
//! macOS. This crate adds the UIKit application, a full-screen window/view,
//! raw touch forwarding (GPUI core recognizes gestures), the software
//! keyboard, and safe-area/keyboard insets.

mod accessibility;
mod apple_sign_in;
mod dispatcher;
mod display;
mod platform;
mod remote_notifications;
mod text_input;
mod window;

pub use apple_sign_in::{AppleCredential, AppleSignInError, sign_in_with_apple};
pub use platform::IosPlatform;
pub use remote_notifications::{
    RemoteNotificationEvent, register_for_remote_notifications, set_badge_count,
};

use objc::{class, msg_send, runtime::Object, sel, sel_impl};
use std::ffi::{CStr, c_char};

#[allow(non_camel_case_types)]
pub(crate) type id = *mut Object;
#[allow(non_upper_case_globals)]
pub(crate) const nil: id = std::ptr::null_mut();

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NSRange {
    pub location: usize,
    pub length: usize,
}

unsafe impl objc::Encode for NSRange {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{_NSRange=QQ}") }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct UIEdgeInsets {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
}

unsafe impl objc::Encode for CGPoint {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{CGPoint=dd}") }
    }
}
unsafe impl objc::Encode for CGSize {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{CGSize=dd}") }
    }
}
unsafe impl objc::Encode for CGRect {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{CGRect={CGPoint=dd}{CGSize=dd}}") }
    }
}
unsafe impl objc::Encode for UIEdgeInsets {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{UIEdgeInsets=dddd}") }
    }
}

/// Autoreleased `NSString` from a Rust string.
pub(crate) fn ns_string(s: &str) -> id {
    let bytes = s.as_ptr();
    unsafe {
        let string: id = msg_send![class!(NSString), alloc];
        let string: id = msg_send![string, initWithBytes: bytes length: s.len() encoding: 4usize];
        msg_send![string, autorelease]
    }
}

/// Borrow an `NSString` as UTF-8. Never panics (called from ObjC callbacks).
pub(crate) unsafe fn to_str<'a>(s: id) -> &'a str {
    if s.is_null() {
        return "";
    }
    let cstr: *const c_char = unsafe { msg_send![s, UTF8String] };
    if cstr.is_null() {
        return "";
    }
    unsafe { CStr::from_ptr(cstr) }.to_str().unwrap_or("")
}

/// Routes `log` records and panics to NSLog so they show up in the device
/// syslog (`idevicesyslog`, Console.app). Installed once by `IosPlatform::new`.
pub(crate) fn install_logging() {
    struct NsLogger;
    impl log::Log for NsLogger {
        fn enabled(&self, m: &log::Metadata) -> bool {
            m.level() <= log::Level::Info
        }
        fn log(&self, r: &log::Record) {
            if self.enabled(r.metadata()) {
                ns_log(&format!("[{}] {}: {}", r.level(), r.target(), r.args()));
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: NsLogger = NsLogger;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ns_log(&format!("[PANIC] {info}"));
        prev(info);
    }));
}

pub(crate) fn ns_log(msg: &str) {
    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {
        fn NSLog(fmt: id, ...);
    }
    unsafe { NSLog(ns_string("gpui: %@"), ns_string(msg)) }
    // Also append to <container>/Documents/gpui.log (pullable over USB with
    // house_arrest; NSLog payloads are redacted as <private> in syslog).
    use std::io::Write;
    static LOG_PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let path = LOG_PATH.get_or_init(|| {
        unsafe extern "C" {
            fn NSHomeDirectory() -> id;
        }
        let home = unsafe { to_str(NSHomeDirectory()) }.to_owned();
        std::path::Path::new(&home).join("Documents/gpui.log")
    });
    {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            let _ = writeln!(f, "{t:.3} {msg}");
        }
    }
}
