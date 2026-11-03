#![cfg(any(target_os = "macos", target_os = "ios"))]
#![cfg_attr(not(target_os = "macos"), allow(unused))]
//! Shared Apple platform support for GPUI.
//!
//! This crate contains the Metal renderer, GPU resource management and the
//! CoreText text system shared by GPUI's Apple platform backends (macOS and
//! iOS).

mod metal_atlas;
pub mod metal_renderer;

#[cfg(feature = "font-kit")]
mod open_type;
#[cfg(feature = "font-kit")]
mod sized_fonts;
#[cfg(feature = "font-kit")]
mod text_system;

#[cfg(feature = "font-kit")]
pub use text_system::MacTextSystem;
/// Alias used by non-macOS Apple backends.
#[cfg(feature = "font-kit")]
pub type AppleTextSystem = MacTextSystem;
