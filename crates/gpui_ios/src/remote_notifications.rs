//! APNs plumbing that has to live on the `UIApplicationDelegate`: device token
//! registration and background (`content-available`) pushes. Presentation,
//! taps, and actions go through `UNUserNotificationCenter`'s own delegate,
//! which the app can install itself.

use crate::{id, nil};
use objc::{class, msg_send, runtime::Object, sel, sel_impl};
use std::cell::RefCell;

/// Something APNs told the app delegate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteNotificationEvent {
    /// Raw APNs device token. Hex-encode it before sending it to a server.
    Token(Vec<u8>),
    /// `registerForRemoteNotifications` failed (simulator, missing
    /// `aps-environment` entitlement, no network).
    RegistrationFailed(String),
    /// A push delivered to the app delegate (background `content-available`
    /// pushes, or any push while the app runs). `payload` is the JSON
    /// `userInfo` dictionary.
    Received { payload: String },
}

type Handler = Box<dyn FnMut(RemoteNotificationEvent)>;

thread_local! {
    static HANDLER: RefCell<Option<Handler>> = RefCell::new(None);
    static PENDING: RefCell<Vec<RemoteNotificationEvent>> = RefCell::new(Vec::new());
}

/// Install `handler` and ask iOS for an APNs device token. Events that arrived
/// before the handler (a background push that launched the app) are replayed.
/// Call on the main thread. Safe to call again to refresh the token.
pub fn register_for_remote_notifications(handler: impl FnMut(RemoteNotificationEvent) + 'static) {
    HANDLER.with(|h| *h.borrow_mut() = Some(Box::new(handler)));
    for event in PENDING.with(|p| std::mem::take(&mut *p.borrow_mut())) {
        dispatch(event);
    }
    unsafe {
        let app: id = msg_send![class!(UIApplication), sharedApplication];
        let _: () = msg_send![app, registerForRemoteNotifications];
    }
}

/// Set the app icon badge count (0 clears it).
pub fn set_badge_count(count: usize) {
    unsafe {
        let center: id = msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
        let _: () = msg_send![center, setBadgeCount: count withCompletionHandler: nil];
    }
}

fn dispatch(event: RemoteNotificationEvent) {
    let delivered = HANDLER.with(|h| match h.borrow_mut().as_mut() {
        Some(handler) => {
            handler(event.clone());
            true
        }
        None => false,
    });
    if !delivered {
        PENDING.with(|p| p.borrow_mut().push(event));
    }
}

pub(crate) extern "C" fn did_register(_: &Object, _: objc::runtime::Sel, _app: id, token: id) {
    let bytes = unsafe {
        let len: usize = msg_send![token, length];
        let ptr: *const u8 = msg_send![token, bytes];
        if ptr.is_null() { Vec::new() } else { std::slice::from_raw_parts(ptr, len).to_vec() }
    };
    log::info!("apns: registered ({} byte token)", bytes.len());
    dispatch(RemoteNotificationEvent::Token(bytes));
}

pub(crate) extern "C" fn did_fail(_: &Object, _: objc::runtime::Sel, _app: id, error: id) {
    let message = unsafe {
        let desc: id = msg_send![error, localizedDescription];
        crate::to_str(desc).to_string()
    };
    log::warn!("apns: registration failed: {message}");
    dispatch(RemoteNotificationEvent::RegistrationFailed(message));
}

pub(crate) extern "C" fn did_receive(
    _: &Object,
    _: objc::runtime::Sel,
    _app: id,
    user_info: id,
    completion: id,
) {
    let payload = unsafe { json_string(user_info) };
    dispatch(RemoteNotificationEvent::Received { payload });
    if !completion.is_null() {
        // UIBackgroundFetchResultNewData = 0
        let block = completion as *mut block::Block<(usize,), ()>;
        unsafe { (*block).call((0,)) };
    }
}

unsafe fn json_string(object: id) -> String {
    unsafe {
        if object.is_null() {
            return "{}".into();
        }
        let valid: objc::runtime::BOOL = msg_send![class!(NSJSONSerialization), isValidJSONObject: object];
        if valid == objc::runtime::NO {
            return "{}".into();
        }
        let data: id = msg_send![class!(NSJSONSerialization), dataWithJSONObject: object options: 0usize error: nil];
        if data.is_null() {
            return "{}".into();
        }
        let len: usize = msg_send![data, length];
        let ptr: *const u8 = msg_send![data, bytes];
        String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned()
    }
}
