//! NativeActivity backend for the same GPUI scene graph used on desktop.
#[cfg(any(target_os = "android", test))]
mod accessibility;
#[cfg(any(target_os = "android", test))]
mod keyboard_configuration;
#[cfg(target_os = "android")]
pub use keyboard_configuration::AndroidTextInputConfiguration;
#[cfg(target_os = "android")]
mod dispatcher;
#[cfg(target_os = "android")]
mod native_services;
#[cfg(target_os = "android")]
mod platform;
#[cfg(target_os = "android")]
mod services;
#[cfg(target_os = "android")]
pub use services::{AndroidPlatformServices, set_platform_services};
#[cfg(target_os = "android")]
mod text_input;
#[cfg(target_os = "android")]
pub use text_input::{AndroidTextInput, dispatch_text_input};
#[cfg(target_os = "android")]
mod window;
#[cfg(any(target_os = "android", test))]
mod window_insets;
#[cfg(target_os = "android")]
pub use android_activity;
#[cfg(target_os = "android")]
pub use platform::{AndroidPlatform, android_app, application, current_platform};
#[cfg(target_os = "android")]
pub use window_insets::dispatch_window_insets;
