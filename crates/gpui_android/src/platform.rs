use crate::{
    dispatcher::AndroidDispatcher,
    window::{AndroidDisplay, AndroidWindow, WindowState},
};
use android_activity::{AndroidApp, MainEvent, PollEvent};
use anyhow::{Result, anyhow};
use futures::channel::oneshot;
use gpui::*;
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
thread_local! { static APP: RefCell<Option<AndroidApp>> = const { RefCell::new(None) }; }
pub fn android_app() -> Option<AndroidApp> {
    APP.with(|a| a.borrow().clone())
}
pub fn current_platform(_headless: bool) -> Rc<dyn Platform> {
    Rc::new(AndroidPlatform::new(
        android_app().expect("initialize AndroidApp before current_platform"),
    ))
}
pub fn application(app: AndroidApp) -> Application {
    Application::with_platform(Rc::new(AndroidPlatform::new(app)))
}
pub struct AndroidPlatform {
    app: AndroidApp,
    dispatcher: Arc<AndroidDispatcher>,
    background: BackgroundExecutor,
    foreground: ForegroundExecutor,
    text: Arc<dyn PlatformTextSystem>,
    windows: RefCell<Vec<Rc<WindowState>>>,
    active: Cell<Option<AnyWindowHandle>>,
    quit: Cell<bool>,
    lifecycle: RefCell<Option<Box<dyn FnMut(AppLifecyclePhase)>>>,
    memory_warning: RefCell<Option<Box<dyn FnMut()>>>,
}
impl AndroidPlatform {
    pub fn new(app: AndroidApp) -> Self {
        APP.with(|a| *a.borrow_mut() = Some(app.clone()));
        crate::text_input::initialize(app.create_waker());
        crate::window_insets::initialize(app.create_waker());
        crate::accessibility::initialize(app.create_waker());
        let dispatcher = AndroidDispatcher::new(app.create_waker());
        Self {
            app,
            background: BackgroundExecutor::new(dispatcher.clone()),
            foreground: ForegroundExecutor::new(dispatcher.clone()),
            dispatcher,
            text: Arc::new(gpui_wgpu::CosmicTextSystem::new("Roboto")),
            windows: RefCell::new(Vec::new()),
            active: Cell::new(None),
            quit: Cell::new(false),
            lifecycle: RefCell::new(None),
            memory_warning: RefCell::new(None),
        }
    }
    fn windows(&self) -> Vec<Rc<WindowState>> {
        self.windows.borrow().clone()
    }
    fn phase(&self, phase: AppLifecyclePhase) {
        let cb = self.lifecycle.borrow_mut().take();
        if let Some(mut cb) = cb {
            cb(phase);
            self.lifecycle.borrow_mut().get_or_insert(cb);
        }
    }
}
struct KeyboardLayout;
impl PlatformKeyboardLayout for KeyboardLayout {
    fn id(&self) -> &str {
        "android"
    }
    fn name(&self) -> &str {
        "Android"
    }
}
impl Platform for AndroidPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.background.clone()
    }
    fn foreground_executor(&self) -> ForegroundExecutor {
        self.foreground.clone()
    }
    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.text.clone()
    }
    fn run(&self, launch: Box<dyn FnOnce()>) {
        let mut launch = Some(launch);
        let mut frame_started = Instant::now();
        let frame_interval = Duration::from_nanos(16_666_667);
        while !self.quit.get() {
            let timeout = frame_interval.saturating_sub(frame_started.elapsed());
            self.app.poll_events(Some(timeout), |event| {
                if let PollEvent::Main(event) = event {
                    match event {
                        MainEvent::InitWindow { .. } => {
                            for window in self.windows() {
                                if let Err(error) = window.resume() {
                                    log::error!("Android surface resume: {error:#}");
                                }
                            }
                            if let Some(launch) = launch.take() {
                                launch();
                            }
                        }
                        MainEvent::TerminateWindow { .. } => {
                            for window in self.windows() {
                                window.suspend();
                            }
                        }
                        MainEvent::WindowResized { .. }
                        | MainEvent::ConfigChanged { .. }
                        | MainEvent::ContentRectChanged { .. }
                        | MainEvent::InsetsChanged { .. } => {
                            for window in self.windows() {
                                window.resize();
                            }
                        }
                        MainEvent::RedrawNeeded { .. } => {
                            for window in self.windows() {
                                window.redraw();
                            }
                        }
                        MainEvent::InputAvailable => {
                            for window in self.windows() {
                                window.input();
                            }
                        }
                        MainEvent::Resume { .. } | MainEvent::GainedFocus => {
                            for window in self.windows() {
                                window.set_active(true);
                            }
                            self.phase(AppLifecyclePhase::Active)
                        }
                        MainEvent::Pause | MainEvent::LostFocus => {
                            for window in self.windows() {
                                window.set_active(false);
                            }
                            self.phase(AppLifecyclePhase::Inactive)
                        }
                        MainEvent::Start => self.phase(AppLifecyclePhase::Foreground),
                        MainEvent::Stop => self.phase(AppLifecyclePhase::Background),
                        MainEvent::LowMemory => {
                            let callback = self.memory_warning.borrow_mut().take();
                            if let Some(mut callback) = callback {
                                callback();
                                self.memory_warning.borrow_mut().get_or_insert(callback);
                            }
                        }
                        MainEvent::Destroy => self.quit.set(true),
                        _ => {}
                    }
                }
            });
            self.dispatcher.drain();
            let windows = self.windows();
            if !windows.is_empty()
                && let Some((safe, ime)) = crate::window_insets::take_native()
            {
                for window in &windows {
                    window.native_insets(safe, ime);
                }
            }
            if let Some(window) = windows.first() {
                for edit in crate::text_input::drain() {
                    window.text_input(edit);
                }
            }
            frame_started = Instant::now();
            for window in windows {
                window.frame();
            }
        }
        for window in self.windows() {
            window.suspend();
        }
    }
    fn quit(&self) {
        self.quit.set(true);
        self.app.create_waker().wake();
    }
    fn restart(&self, _: Option<PathBuf>, _: Vec<std::ffi::OsString>) {}
    fn activate(&self, _: bool) {}
    fn hide(&self) {}
    fn hide_other_apps(&self) {}
    fn unhide_other_apps(&self) {}
    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        vec![Rc::new(AndroidDisplay(self.app.clone()))]
    }
    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.displays().pop()
    }
    fn active_window(&self) -> Option<AnyWindowHandle> {
        self.active.get()
    }
    fn open_window(
        &self,
        handle: AnyWindowHandle,
        _: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        let window = AndroidWindow::new(self.app.clone())?;
        self.windows.borrow_mut().push(window.state.clone());
        self.active.set(Some(handle));
        Ok(Box::new(window))
    }
    fn window_appearance(&self) -> WindowAppearance {
        WindowAppearance::Dark
    }
    fn open_url(&self, url: &str) {
        if let Some(services) = crate::services::services() {
            services.open_url(url);
        } else {
            crate::native_services::open_url(&self.app, url);
        }
    }
    fn on_open_urls(&self, _: Box<dyn FnMut(Vec<String>)>) {}
    fn register_url_scheme(&self, _: &str) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }
    fn prompt_for_paths(
        &self,
        _: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Err(anyhow!("Android file picker unavailable")));
        rx
    }
    fn prompt_for_new_path(
        &self,
        _: &Path,
        _: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Err(anyhow!("Android file picker unavailable")));
        rx
    }
    fn can_select_mixed_files_and_dirs(&self) -> bool {
        false
    }
    fn reveal_path(&self, _: &Path) {}
    fn open_with_system(&self, _: &Path) {}
    fn on_quit(&self, _: Box<dyn FnMut() -> bool>) {}
    fn on_reopen(&self, _: Box<dyn FnMut()>) {}
    fn on_system_sleep(&self, _: Box<dyn FnMut()>) {}
    fn on_system_wake(&self, _: Box<dyn FnMut()>) {}
    fn on_memory_warning(&self, callback: Box<dyn FnMut()>) {
        *self.memory_warning.borrow_mut() = Some(callback);
    }
    fn on_app_lifecycle(&self, cb: Box<dyn FnMut(AppLifecyclePhase)>) {
        *self.lifecycle.borrow_mut() = Some(cb);
    }
    fn set_menus(&self, _: Vec<Menu>, _: &Keymap) {}
    fn set_dock_menu(&self, _: Vec<MenuItem>, _: &Keymap) {}
    fn on_app_menu_action(&self, _: Box<dyn FnMut(&dyn Action)>) {}
    fn on_will_open_app_menu(&self, _: Box<dyn FnMut()>) {}
    fn on_validate_app_menu_command(&self, _: Box<dyn FnMut(&dyn Action) -> bool>) {}
    fn thermal_state(&self) -> ThermalState {
        ThermalState::Nominal
    }
    fn on_thermal_state_change(&self, _: Box<dyn FnMut()>) {}
    fn prevent_idle_sleep(&self, _: &str) -> Task<Result<ActivityGuard>> {
        Task::ready(Ok(ActivityGuard::noop()))
    }
    fn compositor_name(&self) -> &'static str {
        "Android NativeWindow"
    }
    fn app_path(&self) -> Result<PathBuf> {
        std::env::current_exe().map_err(Into::into)
    }
    fn path_for_auxiliary_executable(&self, _: &str) -> Result<PathBuf> {
        Err(anyhow!("Android has no auxiliary executables"))
    }
    fn set_cursor_style(&self, _: CursorStyle) {}
    fn hide_cursor_until_mouse_moves(&self) {}
    fn is_cursor_visible(&self) -> bool {
        false
    }
    fn should_auto_hide_scrollbars(&self) -> bool {
        true
    }
    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        if let Some(services) = crate::services::services() {
            services.read_clipboard().map(ClipboardItem::new_string)
        } else {
            crate::native_services::read_clipboard(&self.app)
        }
    }
    fn write_to_clipboard(&self, item: ClipboardItem) {
        if let Some(services) = crate::services::services() {
            if let Some(text) = item.text() {
                services.write_clipboard(&text);
            }
        } else {
            crate::native_services::write_clipboard(&self.app, item);
        }
    }
    fn write_credentials(&self, _: &str, _: &str, _: &[u8]) -> Task<Result<()>> {
        Task::ready(Err(anyhow!("use Android credential services")))
    }
    fn read_credentials(&self, _: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        Task::ready(Ok(None))
    }
    fn delete_credentials(&self, _: &str) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }
    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(KeyboardLayout)
    }
    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(DummyKeyboardMapper)
    }
    fn on_keyboard_layout_change(&self, _: Box<dyn FnMut()>) {}
}
