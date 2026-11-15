//! Optional activity services supplied by the embedding Android application.
use parking_lot::RwLock;
use std::sync::Arc;

/// JNI adapters for Android OS services. Implementations may be called from the
/// native GPUI thread and must marshal Java UI operations onto the activity thread.
pub trait AndroidPlatformServices: Send + Sync + 'static {
    fn open_url(&self, url: &str);
    fn read_clipboard(&self) -> Option<String>;
    fn write_clipboard(&self, text: &str);
    /// Return true when the activity's custom InputConnection handles the request.
    fn set_keyboard_visible(&self, _visible: bool) -> bool {
        false
    }
    /// Apply EditorInfo hints without changing GPUI text or interrupting composition.
    /// Return false to retry when the activity's IME endpoint is not ready yet.
    fn set_text_input_configuration(
        &self,
        _configuration: crate::AndroidTextInputConfiguration,
    ) -> bool {
        false
    }
}

static SERVICES: RwLock<Option<Arc<dyn AndroidPlatformServices>>> = RwLock::new(None);

/// Registers OS service adapters without coupling GPUI to an application's JNI package.
pub fn set_platform_services(services: Arc<dyn AndroidPlatformServices>) {
    *SERVICES.write() = Some(services);
}

pub(crate) fn services() -> Option<Arc<dyn AndroidPlatformServices>> {
    SERVICES.read().clone()
}
