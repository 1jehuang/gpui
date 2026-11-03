//! UIKit window: one full-screen `GPUIView` hosting the Metal layer.
//!
//! The view forwards raw touches (GPUI core recognizes taps, pans, flings and
//! long presses), drives frames with `CADisplayLink`, acts as a `UIKeyInput`
//! responder for the software keyboard, and reports safe-area and keyboard
//! insets.

use crate::display::IosDisplay;
use crate::{CGPoint, CGRect, UIEdgeInsets, id, nil, ns_string, to_str};
use gpui::{
    AnyWindowHandle, Bounds, Capslock, DevicePixels, DispatchEventResult, Edges, GpuSpecs,
    KeyDownEvent, Keystroke, Modifiers, Pixels, PlatformAtlas, PlatformDisplay, PlatformInput,
    PlatformInputHandler, PlatformWindow, Point, PromptButton, PromptLevel, RequestFrameOptions,
    Scene, Size, TextInputStateChange, TouchEvent, TouchId, TouchPhase, WindowAppearance,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowInsets, WindowParams,
    WindowVisibility, point, px, size,
};
use gpui_apple::metal_renderer::{InstanceBufferPool, MetalRenderer};
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{BOOL, Class, NO, Object, Protocol, Sel, YES},
    sel, sel_impl,
};
use parking_lot::Mutex;
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    rc::Rc,
    sync::{Arc, OnceLock},
};

const STATE_IVAR: &str = "gpuiWindowState";

#[derive(Default)]
struct Callbacks {
    request_frame: Option<Box<dyn FnMut(RequestFrameOptions)>>,
    input: Option<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>>,
    active_status_change: Option<Box<dyn FnMut(bool)>>,
    resize: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    appearance_changed: Option<Box<dyn FnMut()>>,
    insets_changed: Option<Box<dyn FnMut(WindowInsets)>>,
}

pub(crate) struct WindowState {
    view: id,
    renderer: RefCell<MetalRenderer>,
    size: Cell<Size<Pixels>>,
    scale: Cell<f32>,
    safe_area: Cell<Edges<Pixels>>,
    keyboard: Cell<Edges<Pixels>>,
    mouse_position: Cell<Point<Pixels>>,
    input_handler: RefCell<Option<PlatformInputHandler>>,
    callbacks: RefCell<Callbacks>,
    keyboard_wanted: Cell<bool>,
}

impl WindowState {
    /// Runs `f` with a callback taken out of its slot so the callback may
    /// re-enter the window (GPUI does this constantly) without a borrow panic.
    fn with_cb<T, R>(
        &self,
        slot: impl Fn(&mut Callbacks) -> &mut Option<T>,
        f: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        let mut cb = slot(&mut self.callbacks.borrow_mut()).take()?;
        let result = f(&mut cb);
        let mut callbacks = self.callbacks.borrow_mut();
        let entry = slot(&mut callbacks);
        if entry.is_none() {
            *entry = Some(cb);
        }
        Some(result)
    }

    fn dispatch(&self, input: PlatformInput) -> bool {
        self.with_cb(|c| &mut c.input, |cb| cb(input))
            .map(|r| !r.propagate)
            .unwrap_or(false)
    }

    fn insets(&self) -> WindowInsets {
        WindowInsets {
            safe_area: self.safe_area.get(),
            ime: self.keyboard.get(),
        }
    }

    fn notify_insets(&self) {
        let insets = self.insets();
        self.with_cb(|c| &mut c.insets_changed, |cb| cb(insets));
    }

    fn layout(&self) {
        unsafe {
            let bounds: CGRect = msg_send![self.view, bounds];
            let screen: id = msg_send![class!(UIScreen), mainScreen];
            let scale: f64 = msg_send![screen, nativeScale];
            let insets: UIEdgeInsets = msg_send![self.view, safeAreaInsets];
            let new_size = size(px(bounds.size.width as f32), px(bounds.size.height as f32));
            let safe = Edges {
                top: px(insets.top as f32),
                right: px(insets.right as f32),
                bottom: px(insets.bottom as f32),
                left: px(insets.left as f32),
            };
            let changed = new_size != self.size.get() || scale as f32 != self.scale.get();
            self.size.set(new_size);
            self.scale.set(scale as f32);
            {
                let renderer = self.renderer.borrow();
                if let Some(layer) = renderer.layer() {
                    let layer: id = layer as *const _ as id;
                    let _: () = msg_send![layer, setFrame: bounds];
                    let _: () = msg_send![layer, setContentsScale: scale];
                }
            }
            self.renderer.borrow_mut().update_drawable_size(size(
                DevicePixels((bounds.size.width * scale) as i32),
                DevicePixels((bounds.size.height * scale) as i32),
            ));
            if changed {
                let s = scale as f32;
                self.with_cb(|c| &mut c.resize, |cb| cb(new_size, s));
            }
            if safe != self.safe_area.get() {
                self.safe_area.set(safe);
                self.notify_insets();
            }
        }
    }

    fn set_keyboard(&self, visible: bool) {
        self.keyboard_wanted.set(visible);
        unsafe {
            let is_first: BOOL = msg_send![self.view, isFirstResponder];
            if visible && is_first == NO {
                let ok: BOOL = msg_send![self.view, becomeFirstResponder];
                log::info!("ios keyboard: becomeFirstResponder -> {ok}");
            } else if !visible && is_first == YES {
                let _: BOOL = msg_send![self.view, resignFirstResponder];
            }
        }
    }
}

pub(crate) struct IosWindow {
    state: Rc<WindowState>,
    display: Rc<dyn PlatformDisplay>,
    #[allow(dead_code)]
    handle: AnyWindowHandle,
}

impl IosWindow {
    pub(crate) fn open(handle: AnyWindowHandle, _params: WindowParams) -> anyhow::Result<Self> {
        unsafe {
            let screen: id = msg_send![class!(UIScreen), mainScreen];
            let frame: CGRect = msg_send![screen, bounds];
            let scale: f64 = msg_send![screen, nativeScale];

            let view: id = msg_send![view_class(), alloc];
            let view: id = msg_send![view, initWithFrame: frame];
            let _: () = msg_send![view, setMultipleTouchEnabled: YES];
            let black: id = msg_send![class!(UIColor), blackColor];
            let _: () = msg_send![view, setBackgroundColor: black];

            let renderer = MetalRenderer::new(Arc::new(Mutex::new(InstanceBufferPool::default())), false);
            if let Some(layer) = renderer.layer() {
                let view_layer: id = msg_send![view, layer];
                let _: () = msg_send![view_layer, addSublayer: layer as *const _ as id];
            }

            let state = Rc::new(WindowState {
                view,
                renderer: RefCell::new(renderer),
                size: Cell::new(size(px(frame.size.width as f32), px(frame.size.height as f32))),
                scale: Cell::new(scale as f32),
                safe_area: Cell::new(Edges::default()),
                keyboard: Cell::new(Edges::default()),
                mouse_position: Cell::new(Point::default()),
                input_handler: RefCell::new(None),
                callbacks: RefCell::new(Callbacks::default()),
                keyboard_wanted: Cell::new(false),
            });
            (*view).set_ivar::<*const c_void>(STATE_IVAR, Rc::into_raw(state.clone()) as *const c_void);

            let controller: id = msg_send![controller_class(), new];
            let _: () = msg_send![controller, setView: view];
            let window: id = msg_send![class!(UIWindow), alloc];
            let window: id = msg_send![window, initWithFrame: frame];
            let _: () = msg_send![window, setRootViewController: controller];
            let _: () = msg_send![window, makeKeyAndVisible];
            crate::platform::retain_window(window);

            // Keyboard frame tracking.
            let center: id = msg_send![class!(NSNotificationCenter), defaultCenter];
            for name in [
                "UIKeyboardWillChangeFrameNotification",
                "UIKeyboardWillHideNotification",
            ] {
                let _: () = msg_send![center, addObserver: view selector: sel!(gpuiKeyboardChanged:) name: ns_string(name) object: nil];
            }

            // Frame loop.
            let link: id = msg_send![class!(CADisplayLink), displayLinkWithTarget: view selector: sel!(gpuiStep:)];
            let run_loop: id = msg_send![class!(NSRunLoop), mainRunLoop];
            unsafe extern "C" {
                static NSRunLoopCommonModes: id;
            }
            let _: () = msg_send![link, setPaused: NO];
            let _: () = msg_send![link, addToRunLoop: run_loop forMode: NSRunLoopCommonModes];
            let responds: BOOL = msg_send![view, respondsToSelector: sel!(gpuiStep:)];
            log::info!("ios display link {link:p} responds {responds}");

            state.layout();
            log::info!(
                "ios window open: frame {:?} scale {scale} layer {}",
                frame.size,
                state.renderer.borrow().layer().is_some()
            );
            Ok(Self {
                state,
                display: Rc::new(IosDisplay),
                handle,
            })
        }
    }
}

fn state_of(this: &Object) -> Option<&'static WindowState> {
    unsafe {
        let ptr: *const c_void = *this.get_ivar(STATE_IVAR);
        (ptr as *const WindowState).as_ref()
    }
}

fn view_class() -> &'static Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    let ptr = *CLASS.get_or_init(|| unsafe {
        let mut decl = ClassDecl::new("GPUIView", class!(UIView)).expect("GPUIView");
        decl.add_ivar::<*const c_void>(STATE_IVAR);
        if let Some(p) = Protocol::get("UIKeyInput") {
            decl.add_protocol(p);
        }
        decl.add_method(sel!(layoutSubviews), layout_subviews as extern "C" fn(&Object, Sel));
        decl.add_method(sel!(safeAreaInsetsDidChange), layout_subviews as extern "C" fn(&Object, Sel));
        decl.add_method(sel!(gpuiStep:), step as extern "C" fn(&Object, Sel, id));
        decl.add_method(
            sel!(gpuiKeyboardChanged:),
            keyboard_changed as extern "C" fn(&Object, Sel, id),
        );
        let touch = touches as extern "C" fn(&Object, Sel, id, id);
        decl.add_method(sel!(touchesBegan:withEvent:), touch);
        decl.add_method(sel!(touchesMoved:withEvent:), touch);
        decl.add_method(sel!(touchesEnded:withEvent:), touch);
        decl.add_method(sel!(touchesCancelled:withEvent:), touch);
        decl.add_method(sel!(canBecomeFirstResponder), yes as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(hasText), yes as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(insertText:), insert_text as extern "C" fn(&Object, Sel, id));
        decl.add_method(sel!(deleteBackward), delete_backward as extern "C" fn(&Object, Sel));
        decl.add_method(sel!(autocorrectionType), zero as extern "C" fn(&Object, Sel) -> isize);
        decl.add_method(sel!(keyboardAppearance), one as extern "C" fn(&Object, Sel) -> isize);
        decl.register() as *const Class as usize
    });
    unsafe { &*(ptr as *const Class) }
}

fn controller_class() -> &'static Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    let ptr = *CLASS.get_or_init(|| unsafe {
        let mut decl =
            ClassDecl::new("GPUIViewController", class!(UIViewController)).expect("GPUIViewController");
        // UIStatusBarStyleLightContent: GPUI apps here draw dark UIs.
        decl.add_method(sel!(preferredStatusBarStyle), one as extern "C" fn(&Object, Sel) -> isize);
        decl.register() as *const Class as usize
    });
    unsafe { &*(ptr as *const Class) }
}

extern "C" fn yes(_: &Object, _: Sel) -> BOOL {
    YES
}
extern "C" fn zero(_: &Object, _: Sel) -> isize {
    0
}
extern "C" fn one(_: &Object, _: Sel) -> isize {
    1
}

extern "C" fn layout_subviews(this: &Object, _: Sel) {
    if let Some(state) = state_of(this) {
        state.layout();
    }
}

extern "C" fn step(this: &Object, _: Sel, _link: id) {
    static STEPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = STEPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if n < 3 || n == 120 {
        let has = state_of(this).map(|s| s.callbacks.borrow().request_frame.is_some());
        log::info!("ios step #{n}: state/request_frame {has:?}");
    }
    if let Some(state) = state_of(this) {
        state.with_cb(
            |c| &mut c.request_frame,
            |cb| {
                cb(RequestFrameOptions {
                    require_presentation: false,
                    force_render: false,
                })
            },
        );
    }
}

extern "C" fn keyboard_changed(this: &Object, _: Sel, note: id) {
    let Some(state) = state_of(this) else { return };
    unsafe {
        let name: id = msg_send![note, name];
        let hiding = to_str(name) == "UIKeyboardWillHideNotification";
        let mut bottom = 0.0f32;
        if !hiding {
            let info: id = msg_send![note, userInfo];
            let value: id = msg_send![info, objectForKey: ns_string("UIKeyboardFrameEndUserInfoKey")];
            if value != nil {
                let frame: CGRect = msg_send![value, CGRectValue];
                let height = f32::from(state.size.get().height) as f64;
                bottom = (height - frame.origin.y).max(0.0) as f32;
            }
        }
        let edges = Edges {
            bottom: px(bottom),
            ..Edges::default()
        };
        if edges != state.keyboard.get() {
            state.keyboard.set(edges);
            state.notify_insets();
        }
    }
}

extern "C" fn touches(this: &Object, sel: Sel, touches: id, _event: id) {
    let Some(state) = state_of(this) else { return };
    let phase = match sel.name() {
        "touchesBegan:withEvent:" => TouchPhase::Started,
        "touchesMoved:withEvent:" => TouchPhase::Moved,
        "touchesEnded:withEvent:" => TouchPhase::Ended,
        _ => TouchPhase::Cancelled,
    };
    unsafe {
        let all: id = msg_send![touches, allObjects];
        let count: usize = msg_send![all, count];
        for i in 0..count {
            let touch: id = msg_send![all, objectAtIndex: i];
            let p: CGPoint = msg_send![touch, locationInView: this as *const Object as id];
            let position = point(px(p.x as f32), px(p.y as f32));
            let max_force: f64 = msg_send![touch, maximumPossibleForce];
            let force = if max_force > 0.0 {
                let f: f64 = msg_send![touch, force];
                Some((f / max_force) as f32)
            } else {
                None
            };
            state.mouse_position.set(position);
            if !matches!(phase, TouchPhase::Moved) {
                log::info!("ios touch {phase:?} at {position:?}");
            }
            state.dispatch(PlatformInput::Touch(TouchEvent {
                id: TouchId(touch as usize as u64),
                phase,
                position,
                predicted_position: None,
                force,
            }));
        }
    }
}

fn key(state: &WindowState, key: &str, key_char: Option<String>) {
    let keystroke = Keystroke {
        modifiers: Modifiers::default(),
        key: key.into(),
        key_char,
    };
    state.dispatch(PlatformInput::KeyDown(KeyDownEvent {
        keystroke,
        is_held: false,
        prefer_character_input: true,
    }));
}

extern "C" fn insert_text(this: &Object, _: Sel, text: id) {
    let Some(state) = state_of(this) else { return };
    let text = unsafe { to_str(text) }.to_string();
    if text == "\n" {
        key(state, "enter", None);
        return;
    }
    // Multi-character insertions (autocomplete, paste, dictation) go straight
    // to the focused input; single characters travel as keystrokes so key
    // bindings still see them.
    if text.chars().count() == 1 {
        let k = text.to_lowercase();
        let k = if k == " " { "space".into() } else { k };
        key(state, &k, Some(text));
    } else if let Some(mut handler) = state.input_handler.borrow_mut().take() {
        handler.replace_text_in_range(None, &text);
        state.input_handler.borrow_mut().get_or_insert(handler);
    }
}

extern "C" fn delete_backward(this: &Object, _: Sel) {
    if let Some(state) = state_of(this) {
        key(state, "backspace", None);
    }
}

impl raw_window_handle::HasWindowHandle for IosWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let view = std::ptr::NonNull::new(self.state.view as *mut c_void)
            .ok_or(raw_window_handle::HandleError::Unavailable)?;
        let handle = raw_window_handle::UiKitWindowHandle::new(view);
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(handle.into()) })
    }
}

impl raw_window_handle::HasDisplayHandle for IosWindow {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(raw_window_handle::DisplayHandle::uikit())
    }
}

impl PlatformWindow for IosWindow {
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
    fn resize(&mut self, _size: Size<Pixels>) {}
    fn scale_factor(&self) -> f32 {
        self.state.scale.get()
    }
    fn appearance(&self) -> WindowAppearance {
        WindowAppearance::Dark
    }
    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(self.display.clone())
    }
    fn mouse_position(&self) -> Point<Pixels> {
        self.state.mouse_position.get()
    }
    fn modifiers(&self) -> Modifiers {
        Modifiers::default()
    }
    fn capslock(&self) -> Capslock {
        Capslock::default()
    }
    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        *self.state.input_handler.borrow_mut() = Some(input_handler);
    }
    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.state.input_handler.borrow_mut().take()
    }
    fn show_soft_keyboard(&self) {
        self.state.set_keyboard(true);
    }
    fn hide_soft_keyboard(&self) {
        self.state.set_keyboard(false);
    }
    fn text_input_state_changed(&self, change: TextInputStateChange) {
        match change {
            TextInputStateChange::FocusGained => self.state.set_keyboard(true),
            TextInputStateChange::FocusLost => self.state.set_keyboard(false),
            _ => {}
        }
    }
    fn insets(&self) -> WindowInsets {
        self.state.insets()
    }
    fn on_insets_changed(&self, callback: Box<dyn FnMut(WindowInsets)>) {
        self.state.callbacks.borrow_mut().insets_changed = Some(callback);
    }
    fn prompt(
        &self,
        _level: PromptLevel,
        _msg: &str,
        _detail: Option<&str>,
        _answers: &[PromptButton],
    ) -> Option<futures::channel::oneshot::Receiver<usize>> {
        None
    }
    fn activate(&self) {}
    fn is_active(&self) -> bool {
        true
    }
    fn visibility(&self) -> WindowVisibility {
        WindowVisibility::Visible
    }
    fn is_hovered(&self) -> bool {
        false
    }
    fn background_appearance(&self) -> WindowBackgroundAppearance {
        WindowBackgroundAppearance::Opaque
    }
    fn set_title(&mut self, _title: &str) {}
    fn set_background_appearance(&self, _background: WindowBackgroundAppearance) {}
    fn minimize(&self) {}
    fn zoom(&self) {}
    fn toggle_fullscreen(&self) {}
    fn is_fullscreen(&self) -> bool {
        true
    }
    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.state.callbacks.borrow_mut().request_frame = Some(callback);
    }
    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.state.callbacks.borrow_mut().input = Some(callback);
    }
    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.state.callbacks.borrow_mut().active_status_change = Some(callback);
    }
    fn on_hover_status_change(&self, _callback: Box<dyn FnMut(bool)>) {}
    fn on_visibility_change(&self, _callback: Box<dyn FnMut(WindowVisibility)>) {}
    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.state.callbacks.borrow_mut().resize = Some(callback);
    }
    fn on_moved(&self, _callback: Box<dyn FnMut()>) {}
    fn on_should_close(&self, _callback: Box<dyn FnMut() -> bool>) {}
    fn on_hit_test_window_control(&self, _callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {}
    fn on_close(&self, _callback: Box<dyn FnOnce()>) {}
    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.state.callbacks.borrow_mut().appearance_changed = Some(callback);
    }
    fn draw(&self, scene: &Scene) {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 3 || n % 600 == 0 {
            log::info!("ios draw #{n}: size {:?} scale {}", self.state.size.get(), self.state.scale.get());
        }
        self.state.renderer.borrow_mut().draw(scene);
    }
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.state.renderer.borrow().sprite_atlas().clone()
    }
    fn is_subpixel_rendering_supported(&self) -> bool {
        false
    }
    fn gpu_specs(&self) -> Option<GpuSpecs> {
        None
    }
    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {}
}
