//! `Platform` for iOS: boots `UIApplicationMain`, hands GPUI the launch
//! callback from the app delegate, and implements the subset of platform
//! services that make sense on a phone.

use crate::{dispatcher::IosDispatcher, display::IosDisplay, id, nil, ns_string, window::IosWindow};
use anyhow::{Result, anyhow};
use futures::channel::oneshot;
use gpui::{
    Action, ActivityGuard, AnyWindowHandle, AppLifecyclePhase, BackgroundExecutor, ClipboardItem,
    CursorStyle, DummyKeyboardMapper, ForegroundExecutor, GestureTuning, Keymap, Menu, MenuItem,
    PathPromptOptions, Platform, PlatformDisplay, PlatformGestures, PlatformKeyboardLayout,
    PlatformKeyboardMapper, PlatformTextSystem, PlatformWindow, ScrollPhysics, Task, ThermalState,
    WindowAppearance, WindowParams,
};
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{BOOL, Class, Object, Sel, YES},
    sel, sel_impl,
};
use std::{
    cell::RefCell,
    ffi::{c_char, c_int},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, OnceLock},
};

thread_local! {
    static LAUNCH: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    static LIFECYCLE: RefCell<Option<Box<dyn FnMut(AppLifecyclePhase)>>> = RefCell::new(None);
    static WINDOWS: RefCell<Vec<id>> = RefCell::new(Vec::new());
}

/// Keeps UIWindows alive for the life of the process (UIKit only holds
/// them weakly once they are key).
pub(crate) fn retain_window(window: id) {
    WINDOWS.with(|w| w.borrow_mut().push(window));
}

#[link(name = "UIKit", kind = "framework")]
#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {
    fn UIApplicationMain(argc: c_int, argv: *mut *mut c_char, principal: id, delegate: id) -> c_int;
}

struct IosGestures;

impl PlatformGestures for IosGestures {
    fn tuning(&self) -> GestureTuning {
        GestureTuning {
            scroll_physics: ScrollPhysics::ios(),
            ..GestureTuning::default()
        }
    }
}

struct IosKeyboardLayout;

impl PlatformKeyboardLayout for IosKeyboardLayout {
    fn id(&self) -> &str {
        "ios"
    }
    fn name(&self) -> &str {
        "iOS"
    }
}

pub struct IosPlatform {
    background_executor: BackgroundExecutor,
    foreground_executor: ForegroundExecutor,
    text_system: Arc<dyn PlatformTextSystem>,
    active_window: RefCell<Option<AnyWindowHandle>>,
    open_urls: RefCell<Option<Box<dyn FnMut(Vec<String>)>>>,
}

impl Default for IosPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl IosPlatform {
    pub fn new() -> Self {
        crate::install_logging();
        let dispatcher = Arc::new(IosDispatcher);
        Self {
            background_executor: BackgroundExecutor::new(dispatcher.clone()),
            foreground_executor: ForegroundExecutor::new(dispatcher),
            text_system: Arc::new(gpui_apple::AppleTextSystem::new()),
            active_window: RefCell::new(None),
            open_urls: RefCell::new(None),
        }
    }
}

fn delegate_class() -> &'static Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    let ptr = *CLASS.get_or_init(|| unsafe {
        let mut decl = ClassDecl::new("GPUIAppDelegate", class!(UIResponder)).expect("delegate");
        decl.add_method(
            sel!(application:didFinishLaunchingWithOptions:),
            did_finish as extern "C" fn(&Object, Sel, id, id) -> BOOL,
        );
        let phase = lifecycle as extern "C" fn(&Object, Sel, id);
        decl.add_method(sel!(applicationDidBecomeActive:), phase);
        decl.add_method(sel!(applicationWillResignActive:), phase);
        decl.add_method(sel!(applicationDidEnterBackground:), phase);
        use crate::remote_notifications as rn;
        decl.add_method(
            sel!(application:didRegisterForRemoteNotificationsWithDeviceToken:),
            rn::did_register as extern "C" fn(&Object, Sel, id, id),
        );
        decl.add_method(
            sel!(application:didFailToRegisterForRemoteNotificationsWithError:),
            rn::did_fail as extern "C" fn(&Object, Sel, id, id),
        );
        decl.add_method(
            sel!(application:didReceiveRemoteNotification:fetchCompletionHandler:),
            rn::did_receive as extern "C" fn(&Object, Sel, id, id, id),
        );
        decl.register() as *const Class as usize
    });
    unsafe { &*(ptr as *const Class) }
}

extern "C" fn did_finish(_: &Object, _: Sel, _app: id, _opts: id) -> BOOL {
    if let Some(launch) = LAUNCH.with(|l| l.borrow_mut().take()) {
        launch();
    }
    log::info!("ios did_finish returned");
    #[cfg(debug_assertions)]
    let dev = true;
    #[cfg(not(debug_assertions))]
    let dev = std::env::var_os("GPUI_IOS_NO_IDLE_LOCK").is_some() || option_env!("GPUI_IOS_NO_IDLE_LOCK").is_some();
    if dev {
        // Dev builds: keep the screen awake so USB launches never hit a locked phone.
        unsafe {
            let app: id = msg_send![class!(UIApplication), sharedApplication];
            let _: () = msg_send![app, setIdleTimerDisabled: YES];
        }
    }
    YES
}

extern "C" fn lifecycle(_: &Object, sel: Sel, _app: id) {
    let phase = match sel.name() {
        "applicationDidBecomeActive:" => AppLifecyclePhase::Active,
        "applicationWillResignActive:" => AppLifecyclePhase::Inactive,
        _ => AppLifecyclePhase::Background,
    };
    log::info!("ios lifecycle {}", sel.name());
    LIFECYCLE.with(|cb| {
        if let Some(cb) = cb.borrow_mut().as_mut() {
            cb(phase);
        }
    });
}

impl Platform for IosPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.background_executor.clone()
    }
    fn foreground_executor(&self) -> ForegroundExecutor {
        self.foreground_executor.clone()
    }
    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.text_system.clone()
    }
    fn gestures(&self) -> Option<Rc<dyn PlatformGestures>> {
        Some(Rc::new(IosGestures))
    }

    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        LAUNCH.with(|l| *l.borrow_mut() = Some(on_finish_launching));
        unsafe {
            let name: id = msg_send![delegate_class(), description];
            UIApplicationMain(0, std::ptr::null_mut(), nil, name);
        }
    }

    fn on_app_lifecycle(&self, callback: Box<dyn FnMut(AppLifecyclePhase)>) {
        LIFECYCLE.with(|l| *l.borrow_mut() = Some(callback));
    }

    fn quit(&self) {}
    fn restart(&self, _binary_path: Option<PathBuf>, _arguments: Vec<std::ffi::OsString>) {}
    fn activate(&self, _ignoring_other_apps: bool) {}
    fn hide(&self) {}
    fn hide_other_apps(&self) {}
    fn unhide_other_apps(&self) {}

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        vec![Rc::new(IosDisplay)]
    }
    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(Rc::new(IosDisplay))
    }
    fn active_window(&self) -> Option<AnyWindowHandle> {
        *self.active_window.borrow()
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        params: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        let window = IosWindow::open(handle, params)?;
        *self.active_window.borrow_mut() = Some(handle);
        Ok(Box::new(window))
    }

    fn window_appearance(&self) -> WindowAppearance {
        WindowAppearance::Dark
    }

    fn open_url(&self, url: &str) {
        unsafe {
            let nsurl: id = msg_send![class!(NSURL), URLWithString: ns_string(url)];
            if nsurl == nil {
                log::warn!("open_url: invalid url {url}");
                return;
            }
            let app: id = msg_send![class!(UIApplication), sharedApplication];
            let options: id = msg_send![class!(NSDictionary), dictionary];
            let _: () = msg_send![app, openURL: nsurl options: options completionHandler: nil];
        }
    }
    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        *self.open_urls.borrow_mut() = Some(callback);
    }
    fn register_url_scheme(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    fn prompt_for_paths(
        &self,
        _options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        let (tx, rx) = oneshot::channel();
        tx.send(Err(anyhow!("file pickers are not supported on iOS yet"))).ok();
        rx
    }
    fn prompt_for_new_path(
        &self,
        _directory: &Path,
        _suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        let (tx, rx) = oneshot::channel();
        tx.send(Err(anyhow!("file pickers are not supported on iOS yet"))).ok();
        rx
    }
    fn can_select_mixed_files_and_dirs(&self) -> bool {
        false
    }
    fn reveal_path(&self, _path: &Path) {}
    fn open_with_system(&self, _path: &Path) {}

    fn on_quit(&self, _callback: Box<dyn FnMut() -> bool>) {}
    fn on_reopen(&self, _callback: Box<dyn FnMut()>) {}
    fn on_system_sleep(&self, _callback: Box<dyn FnMut()>) {}
    fn on_system_wake(&self, _callback: Box<dyn FnMut()>) {}

    fn set_menus(&self, _menus: Vec<Menu>, _keymap: &Keymap) {}
    fn set_dock_menu(&self, _menu: Vec<MenuItem>, _keymap: &Keymap) {}
    fn on_app_menu_action(&self, _callback: Box<dyn FnMut(&dyn Action)>) {}
    fn on_will_open_app_menu(&self, _callback: Box<dyn FnMut()>) {}
    fn on_validate_app_menu_command(&self, _callback: Box<dyn FnMut(&dyn Action) -> bool>) {}

    fn thermal_state(&self) -> ThermalState {
        ThermalState::Nominal
    }
    fn on_thermal_state_change(&self, _callback: Box<dyn FnMut()>) {}
    fn prevent_idle_sleep(&self, _reason: &str) -> Task<Result<ActivityGuard>> {
        Task::ready(Ok(ActivityGuard::noop()))
    }

    fn compositor_name(&self) -> &'static str {
        "UIKit"
    }
    fn app_path(&self) -> Result<PathBuf> {
        std::env::current_exe().map_err(Into::into)
    }
    fn path_for_auxiliary_executable(&self, _name: &str) -> Result<PathBuf> {
        Err(anyhow!("auxiliary executables are not available on iOS"))
    }

    fn set_cursor_style(&self, _style: CursorStyle) {}
    fn hide_cursor_until_mouse_moves(&self) {}
    fn is_cursor_visible(&self) -> bool {
        false
    }
    fn should_auto_hide_scrollbars(&self) -> bool {
        true
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        unsafe {
            let board: id = msg_send![class!(UIPasteboard), generalPasteboard];
            let s: id = msg_send![board, string];
            (s != nil).then(|| ClipboardItem::new_string(crate::to_str(s).to_string()))
        }
    }
    fn write_to_clipboard(&self, item: ClipboardItem) {
        if let Some(text) = item.text() {
            unsafe {
                let board: id = msg_send![class!(UIPasteboard), generalPasteboard];
                let _: () = msg_send![board, setString: ns_string(&text)];
            }
        }
    }
    fn write_credentials(&self, _url: &str, _username: &str, _password: &[u8]) -> Task<Result<()>> {
        Task::ready(Err(anyhow!("keychain credentials are not implemented on iOS yet")))
    }
    fn read_credentials(&self, _url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        Task::ready(Ok(None))
    }
    fn delete_credentials(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(IosKeyboardLayout)
    }
    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(DummyKeyboardMapper)
    }
    fn on_keyboard_layout_change(&self, _callback: Box<dyn FnMut()>) {}
}
