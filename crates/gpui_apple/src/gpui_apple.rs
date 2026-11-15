#![cfg(any(target_os = "macos", target_os = "ios"))]
#![cfg_attr(not(target_os = "macos"), allow(unused))]
//! Shared Apple platform support for GPUI.
//!
//! This crate contains the Metal renderer and GPU resource management shared
//! by GPUI's Apple platform backends.

mod fast;
mod metal_atlas;
pub use crate::fast::metal_renderer;

#[cfg(feature = "font-kit")]
pub use crate::fast::text_system::{AppleTextSystem, MacTextSystem};
