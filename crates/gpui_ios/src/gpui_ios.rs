#![cfg(target_os = "ios")]
//! iOS (UIKit) platform implementation for GPUI.
//!
//! Rendering and text come from `gpui_apple` (Metal + CoreText), shared with
//! macOS. This crate adds the UIKit application, a full-screen window/view,
//! raw touch forwarding (GPUI core recognizes gestures), the software
//! keyboard, and safe-area/keyboard insets.

mod dispatcher;
mod display;
mod platform;
mod window;

pub use platform::IosPlatform;

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
