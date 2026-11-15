#[path = "test/dispatcher.rs"]
mod dispatcher;
#[path = "test/display.rs"]
mod display;
#[path = "test/platform.rs"]
mod platform;
#[path = "test/window.rs"]
mod window;

pub use dispatcher::*;
pub(crate) use display::*;
pub(crate) use platform::*;
pub(crate) use window::*;

#[cfg(any(test, feature = "test-api"))]
pub use platform::{TestScreenCaptureSource, TestScreenCaptureStream};
