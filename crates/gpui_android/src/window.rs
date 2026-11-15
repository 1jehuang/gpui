use android_activity::{
    AndroidApp, InputStatus,
    input::{InputEvent, KeyAction, Keycode, MotionAction},
};
use gpui::*;
use gpui_wgpu::{WgpuRenderer, WgpuSurfaceConfig};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

#[derive(Debug)]
pub(crate) struct AndroidDisplay(pub AndroidApp);
impl PlatformDisplay for AndroidDisplay {
    fn id(&self) -> DisplayId {
        DisplayId::new(1)
    }
    fn uuid(&self) -> anyhow::Result<uuid::Uuid> {
        Ok(uuid::Uuid::nil())
    }
    fn bounds(&self) -> Bounds<Pixels> {
        let scale = scale(&self.0);
        let s = self
            .0
            .native_window()
            .map(|w| size(px(w.width() as f32 / scale), px(w.height() as f32 / scale)))
            .unwrap_or_default();
        Bounds::new(Point::default(), s)
    }
}
fn scale(app: &AndroidApp) -> f32 {
    app.config().density().unwrap_or(160).max(1) as f32 / 160.
}
#[derive(Default)]
struct Callbacks {
    frame: Option<Box<dyn FnMut(RequestFrameOptions)>>,
    input: Option<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>>,
    resize: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    active: Option<Box<dyn FnMut(bool)>>,
    appearance: Option<Box<dyn FnMut()>>,
    visibility: Option<Box<dyn FnMut(WindowVisibility)>>,
    insets: Option<Box<dyn FnMut(WindowInsets)>>,
}
pub(crate) struct WindowState {
    app: AndroidApp,
    renderer: RefCell<WgpuRenderer>,
    // ANativeWindow stays retained until after its wgpu surface is released.
    native: RefCell<Option<ndk::native_window::NativeWindow>>,
    callbacks: RefCell<Callbacks>,
    handler: RefCell<Option<PlatformInputHandler>>,
    keyboard_configuration: crate::keyboard_configuration::ConfigurationDelivery,
    size: Cell<Size<Pixels>>,
    scale: Cell<f32>,
    visible: Cell<bool>,
    active: Cell<bool>,
    modifiers: Cell<Modifiers>,
    capslock: Cell<Capslock>,
    position: Cell<Point<Pixels>>,
    insets: RefCell<WindowInsets>,
    base_insets: RefCell<Option<Edges<Pixels>>>,
    authoritative_insets: Cell<Option<([i32; 4], [i32; 4])>>,
    a11y: crate::accessibility::A11yBridge,
    needs_full_redraw: Cell<bool>,
}
impl WindowState {
    fn apply_keyboard_configuration(&self) {
        self.keyboard_configuration.apply(|configuration| {
            crate::services::services()
                .is_some_and(|services| services.set_text_input_configuration(configuration))
        });
    }
    fn callback<T, R>(
        &self,
        slot: impl Fn(&mut Callbacks) -> &mut Option<T>,
        f: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        let mut cb = slot(&mut self.callbacks.borrow_mut()).take()?;
        let result = f(&mut cb);
        slot(&mut self.callbacks.borrow_mut()).get_or_insert(cb);
        Some(result)
    }
    pub fn set_active(&self, active: bool) {
        self.active.set(active);
        self.callback(|c| &mut c.active, |cb| cb(active));
    }
    pub fn suspend(&self) {
        self.visible.set(false);
        self.renderer.borrow_mut().unconfigure_surface();
        self.renderer.borrow_mut().destroy();
        self.native.borrow_mut().take();
        self.callback(|c| &mut c.visibility, |cb| cb(WindowVisibility::Hidden));
    }
    pub fn resume(&self) -> anyhow::Result<()> {
        let native = self
            .app
            .native_window()
            .ok_or_else(|| anyhow::anyhow!("Android has no native window"))?;
        self.renderer
            .borrow_mut()
            .recover(&NativeSurface(native.clone()))?;
        self.needs_full_redraw.set(true);
        *self.native.borrow_mut() = Some(native);
        self.visible.set(true);
        self.resize();
        self.callback(|c| &mut c.visibility, |cb| cb(WindowVisibility::Visible));
        Ok(())
    }
    pub fn resize(&self) {
        let Some(native) = self.app.native_window() else {
            return;
        };
        let s = scale(&self.app);
        let dims = size(
            px(native.width() as f32 / s),
            px(native.height() as f32 / s),
        );
        self.scale.set(s);
        self.a11y.set_scale(1.0);
        self.size.set(dims);
        self.renderer.borrow_mut().update_drawable_size(size(
            DevicePixels(native.width()),
            DevicePixels(native.height()),
        ));
        self.callback(|c| &mut c.resize, |cb| cb(dims, s));
        self.update_insets(native.width(), native.height(), s);
    }
    fn update_insets(&self, width: i32, height: i32, scale: f32) {
        if let Some((safe, ime)) = self.authoritative_insets.get() {
            self.native_insets(safe, ime);
            return;
        }
        let rect = self.app.content_rect();
        let insets = {
            let mut baseline = self.base_insets.borrow_mut();
            crate::window_insets::content_insets(
                size(DevicePixels(width), DevicePixels(height)),
                scale,
                [rect.left, rect.top, rect.right, rect.bottom],
                &mut baseline,
            )
        };
        let Some(insets) = insets else { return };
        if *self.insets.borrow() != insets {
            log::debug!("Android window insets updated: {insets:?}");
            *self.insets.borrow_mut() = insets.clone();
            self.callback(|c| &mut c.insets, |cb| cb(insets));
        }
    }
    pub fn native_insets(&self, safe: [i32; 4], ime: [i32; 4]) {
        let Some(insets) = crate::window_insets::physical_insets(self.scale.get(), safe, ime)
        else {
            return;
        };
        self.authoritative_insets.set(Some((safe, ime)));
        if *self.insets.borrow() != insets {
            *self.insets.borrow_mut() = insets.clone();
            self.callback(|c| &mut c.insets, |cb| cb(insets));
        }
    }
    pub fn redraw(&self) {
        if self.visible.get() {
            self.callback(
                |c| &mut c.frame,
                |cb| {
                    cb(RequestFrameOptions {
                        require_presentation: true,
                        force_render: self.needs_full_redraw.replace(false),
                    })
                },
            );
        }
    }
    pub fn frame(&self) {
        self.a11y.process_events();
        if self.visible.get() {
            let force_render = self.needs_full_redraw.replace(false);
            self.callback(
                |c| &mut c.frame,
                |cb| {
                    cb(RequestFrameOptions {
                        force_render,
                        require_presentation: force_render,
                    })
                },
            );
        }
    }
    fn dispatch(&self, input: PlatformInput) -> bool {
        self.callback(|c| &mut c.input, |cb| !cb(input).propagate)
            .unwrap_or(false)
    }
    fn edit(&self, f: impl FnOnce(&mut PlatformInputHandler)) {
        let Some(mut handler) = self.handler.borrow_mut().take() else {
            return;
        };
        f(&mut handler);
        self.handler.borrow_mut().get_or_insert(handler);
    }
    pub fn text_input(&self, input: crate::AndroidTextInput) {
        use crate::AndroidTextInput;
        match input {
            AndroidTextInput::Commit(text) => self.edit(|handler| {
                let marked = handler.marked_text_range();
                handler.replace_text_in_range(marked, &text);
            }),
            AndroidTextInput::Compose(text) => self.edit(|handler| {
                let marked = handler.marked_text_range();
                let end = text.encode_utf16().count();
                handler.replace_and_mark_text_in_range(marked, &text, Some(end..end));
            }),
            AndroidTextInput::FinishComposition => self.edit(|handler| handler.unmark_text()),
            AndroidTextInput::DeleteSurrounding { before, after } => self.edit(|handler| {
                if let Some(selection) = handler.selected_text_range(false) {
                    let range = selection.range;
                    handler.replace_text_in_range(
                        Some(range.start.saturating_sub(before)..range.end.saturating_add(after)),
                        "",
                    );
                }
            }),
            AndroidTextInput::Enter => {
                self.dispatch(PlatformInput::KeyDown(KeyDownEvent {
                    keystroke: Keystroke {
                        key: "enter".into(),
                        key_char: None,
                        modifiers: Modifiers::default(),
                    },
                    is_held: false,
                    prefer_character_input: false,
                }));
            }
        }
    }
    pub fn input(&self) {
        let Ok(mut iter) = self.app.input_events_iter() else {
            return;
        };
        while iter.next(|event| match event {
            InputEvent::MotionEvent(event) => {
                let action = event.action();
                for pointer in event.pointers() {
                    let phase = match action {
                        MotionAction::Down | MotionAction::PointerDown
                            if pointer.pointer_index() == event.pointer_index() =>
                        {
                            TouchPhase::Started
                        }
                        MotionAction::Up | MotionAction::PointerUp
                            if pointer.pointer_index() == event.pointer_index() =>
                        {
                            TouchPhase::Ended
                        }
                        MotionAction::Move => TouchPhase::Moved,
                        MotionAction::Cancel => TouchPhase::Cancelled,
                        _ => continue,
                    };
                    let position = point(
                        px(pointer.x() / self.scale.get()),
                        px(pointer.y() / self.scale.get()),
                    );
                    self.position.set(position);
                    self.dispatch(PlatformInput::Touch(TouchEvent {
                        id: TouchId(pointer.pointer_id() as u64),
                        phase,
                        position,
                        predicted_position: None,
                        force: None,
                    }));
                }
                InputStatus::Handled
            }
            InputEvent::KeyEvent(event) => {
                let meta = event.meta_state();
                let modifiers = Modifiers {
                    control: meta.ctrl_on(),
                    alt: meta.alt_on(),
                    shift: meta.shift_on(),
                    platform: meta.meta_on(),
                    function: meta.function_on(),
                };
                self.modifiers.set(modifiers);
                self.capslock.set(Capslock {
                    on: meta.caps_lock_on(),
                });
                let key_char = self
                    .app
                    .device_key_character_map(event.device_id())
                    .ok()
                    .and_then(
                        |map| match map.get(event.key_code(), event.meta_state()).ok()? {
                            android_activity::input::KeyMapChar::Unicode(c) if !c.is_control() => {
                                Some(c.to_string())
                            }
                            _ => None,
                        },
                    );
                let base_character = self
                    .app
                    .device_key_character_map(event.device_id())
                    .ok()
                    .and_then(|map| {
                        match map
                            .get(event.key_code(), android_activity::input::MetaState(0))
                            .ok()?
                        {
                            android_activity::input::KeyMapChar::Unicode(c) if !c.is_control() => {
                                Some(c.to_string())
                            }
                            _ => None,
                        }
                    });
                let key = match event.key_code() {
                    Keycode::Del => "backspace",
                    Keycode::Enter => "enter",
                    Keycode::Tab => "tab",
                    Keycode::Escape | Keycode::Back => "escape",
                    Keycode::DpadLeft => "left",
                    Keycode::DpadRight => "right",
                    Keycode::DpadUp => "up",
                    Keycode::DpadDown => "down",
                    Keycode::ForwardDel => "delete",
                    Keycode::MoveHome => "home",
                    Keycode::MoveEnd => "end",
                    Keycode::PageUp => "pageup",
                    Keycode::PageDown => "pagedown",
                    Keycode::Space => "space",
                    _ => base_character
                        .as_deref()
                        .or(key_char.as_deref())
                        .unwrap_or(""),
                };
                if key.is_empty() {
                    return InputStatus::Unhandled;
                }
                let keystroke = Keystroke {
                    key: key.into(),
                    key_char: key_char.clone(),
                    modifiers,
                };
                let input = match event.action() {
                    KeyAction::Down => PlatformInput::KeyDown(KeyDownEvent {
                        keystroke,
                        is_held: event.repeat_count() > 0,
                        prefer_character_input: false,
                    }),
                    KeyAction::Up => PlatformInput::KeyUp(KeyUpEvent { keystroke }),
                    _ => return InputStatus::Unhandled,
                };
                if self.dispatch(input) {
                    InputStatus::Handled
                } else if event.action() == KeyAction::Down
                    && key_char.is_some()
                    && !modifiers.control
                    && !modifiers.platform
                {
                    self.edit(|handler| {
                        handler.replace_text_in_range(None, key_char.as_deref().unwrap_or_default())
                    });
                    InputStatus::Handled
                } else {
                    InputStatus::Unhandled
                }
            }
            InputEvent::TextEvent(state) => {
                self.edit(|handler| {
                    if let Some(compose) = state.compose_region {
                        handler.replace_and_mark_text_in_range(
                            None,
                            &state.text,
                            Some(compose.start..compose.end),
                        );
                    } else {
                        handler.replace_text_in_range(None, &state.text);
                    }
                });
                InputStatus::Handled
            }
            _ => InputStatus::Unhandled,
        }) {}
    }
}
#[derive(Clone, Debug)]
struct NativeSurface(ndk::native_window::NativeWindow);
impl HasWindowHandle for NativeSurface {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        self.0.window_handle()
    }
}
impl HasDisplayHandle for NativeSurface {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(raw_window_handle::DisplayHandle::android())
    }
}
pub(crate) struct AndroidWindow {
    pub state: Rc<WindowState>,
}
impl AndroidWindow {
    pub fn new(app: AndroidApp) -> anyhow::Result<Self> {
        let native = app
            .native_window()
            .ok_or_else(|| anyhow::anyhow!("open_window called before InitWindow"))?;
        log::info!(
            "GPUI Android native window: {}x{} physical pixels, density {}",
            native.width(),
            native.height(),
            scale(&app)
        );
        let gpu = Rc::new(RefCell::new(None));
        let renderer = WgpuRenderer::new(
            gpu.clone(),
            &NativeSurface(native.clone()),
            WgpuSurfaceConfig {
                size: size(DevicePixels(native.width()), DevicePixels(native.height())),
                transparent: false,
                preferred_present_mode: None,
            },
            None,
        )?;
        let state = Rc::new(WindowState {
            app,
            renderer: RefCell::new(renderer),
            native: RefCell::new(Some(native)),
            callbacks: RefCell::default(),
            handler: RefCell::default(),
            keyboard_configuration: Default::default(),
            size: Cell::default(),
            scale: Cell::new(1.),
            visible: Cell::new(true),
            active: Cell::new(true),
            modifiers: Cell::default(),
            capslock: Cell::default(),
            position: Cell::default(),
            insets: RefCell::default(),
            base_insets: RefCell::default(),
            authoritative_insets: Cell::new(None),
            a11y: crate::accessibility::A11yBridge::new(),
            needs_full_redraw: Cell::new(false),
        });
        state.resize();
        Ok(Self { state })
    }
}
impl HasWindowHandle for AndroidWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let native = self.state.native.borrow();
        let raw = native
            .as_ref()
            .ok_or(raw_window_handle::HandleError::Unavailable)?
            .window_handle()?
            .as_raw();
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}
impl HasDisplayHandle for AndroidWindow {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(raw_window_handle::DisplayHandle::android())
    }
}
impl PlatformWindow for AndroidWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(Point::default(), self.state.size.get())
    }
    fn is_maximized(&self) -> bool {
        true
    }
    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Fullscreen(self.bounds())
    }
    fn content_size(&self) -> Size<Pixels> {
        self.state.size.get()
    }
    fn resize(&mut self, _: Size<Pixels>) {}
    fn scale_factor(&self) -> f32 {
        self.state.scale.get()
    }
    fn appearance(&self) -> WindowAppearance {
        WindowAppearance::Dark
    }
    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(Rc::new(AndroidDisplay(self.state.app.clone())))
    }
    fn mouse_position(&self) -> Point<Pixels> {
        self.state.position.get()
    }
    fn modifiers(&self) -> Modifiers {
        self.state.modifiers.get()
    }
    fn capslock(&self) -> Capslock {
        self.state.capslock.get()
    }
    fn set_input_handler(&mut self, handler: PlatformInputHandler) {
        *self.state.handler.borrow_mut() = Some(handler);
        self.state.apply_keyboard_configuration();
    }
    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.state.handler.borrow_mut().take()
    }
    fn set_text_input_configuration(&mut self, configuration: TextInputConfiguration) {
        let configuration = crate::AndroidTextInputConfiguration::from(&configuration);
        self.state.keyboard_configuration.request(configuration);
        self.state.apply_keyboard_configuration();
    }
    fn insets(&self) -> WindowInsets {
        self.state.insets.borrow().clone()
    }
    fn on_insets_changed(&self, callback: Box<dyn FnMut(WindowInsets)>) {
        self.state.callbacks.borrow_mut().insets = Some(callback);
    }
    fn show_soft_keyboard(&self) {
        self.state.apply_keyboard_configuration();
        if !crate::services::services().is_some_and(|s| s.set_keyboard_visible(true)) {
            self.state.app.show_soft_input(true);
        }
    }
    fn hide_soft_keyboard(&self) {
        if !crate::services::services().is_some_and(|s| s.set_keyboard_visible(false)) {
            self.state.app.hide_soft_input(false);
        }
    }
    fn text_input_state_changed(&self, change: TextInputStateChange) {
        match change {
            TextInputStateChange::FocusGained => self.show_soft_keyboard(),
            TextInputStateChange::FocusLost => self.hide_soft_keyboard(),
            _ => {}
        }
    }
    fn prompt(
        &self,
        _: PromptLevel,
        _: &str,
        _: Option<&str>,
        _: &[PromptButton],
    ) -> Option<futures::channel::oneshot::Receiver<usize>> {
        None
    }
    fn activate(&self) {}
    fn is_active(&self) -> bool {
        self.state.active.get()
    }
    fn visibility(&self) -> WindowVisibility {
        if self.state.visible.get() {
            WindowVisibility::Visible
        } else {
            WindowVisibility::Hidden
        }
    }
    fn is_hovered(&self) -> bool {
        false
    }
    fn background_appearance(&self) -> WindowBackgroundAppearance {
        WindowBackgroundAppearance::Opaque
    }
    fn set_title(&mut self, _: &str) {}
    fn set_background_appearance(&self, _: WindowBackgroundAppearance) {}
    fn minimize(&self) {}
    fn zoom(&self) {}
    fn toggle_fullscreen(&self) {}
    fn is_fullscreen(&self) -> bool {
        true
    }
    fn on_request_frame(&self, cb: Box<dyn FnMut(RequestFrameOptions)>) {
        self.state.callbacks.borrow_mut().frame = Some(cb);
    }
    fn on_input(&self, cb: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.state.callbacks.borrow_mut().input = Some(cb);
    }
    fn on_active_status_change(&self, cb: Box<dyn FnMut(bool)>) {
        self.state.callbacks.borrow_mut().active = Some(cb);
    }
    fn on_hover_status_change(&self, _: Box<dyn FnMut(bool)>) {}
    fn on_visibility_change(&self, cb: Box<dyn FnMut(WindowVisibility)>) {
        self.state.callbacks.borrow_mut().visibility = Some(cb);
    }
    fn on_resize(&self, cb: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.state.callbacks.borrow_mut().resize = Some(cb);
    }
    fn on_moved(&self, _: Box<dyn FnMut()>) {}
    fn on_should_close(&self, _: Box<dyn FnMut() -> bool>) {}
    fn on_hit_test_window_control(&self, _: Box<dyn FnMut() -> Option<WindowControlArea>>) {}
    fn on_close(&self, _: Box<dyn FnOnce()>) {}
    fn on_appearance_changed(&self, cb: Box<dyn FnMut()>) {
        self.state.callbacks.borrow_mut().appearance = Some(cb);
    }
    fn draw(&self, scene: &Scene) {
        if self.state.visible.get() {
            self.state.renderer.borrow_mut().draw(scene);
        }
    }
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.state.renderer.borrow().sprite_atlas().clone()
    }
    fn is_subpixel_rendering_supported(&self) -> bool {
        false
    }
    fn gpu_specs(&self) -> Option<GpuSpecs> {
        Some(self.state.renderer.borrow().gpu_specs())
    }
    fn a11y_init(&self, callbacks: A11yCallbacks) {
        self.state.a11y.init(callbacks);
    }
    fn a11y_tree_update(&self, update: accesskit::TreeUpdate) {
        self.state.a11y.apply(update);
    }
    fn update_ime_position(&self, _: Bounds<Pixels>) {}
}
