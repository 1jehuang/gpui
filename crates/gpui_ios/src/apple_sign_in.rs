//! Sign in with Apple via `ASAuthorizationController`.
//!
//! The app generates a random nonce, passes `sha256_hex(nonce)` here, and
//! sends the returned identity token plus the raw nonce to its server, which
//! verifies the token against Apple's JWKS and checks that the token's `nonce`
//! claim equals the hash. Requires the `com.apple.developer.applesignin`
//! entitlement (`["Default"]`).

use crate::{id, nil, ns_string, to_str};
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{Class, Object, Sel},
    sel, sel_impl,
};
use std::cell::RefCell;

/// Successful Sign in with Apple result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleCredential {
    /// Stable Apple user id (`sub`).
    pub user: String,
    /// JWT signed by Apple. Send to the server with the raw nonce.
    pub identity_token: String,
    /// Short-lived authorization code (for server-side token exchange, optional).
    pub authorization_code: Option<String>,
    /// Only present on the first authorization. May be a privaterelay address.
    pub email: Option<String>,
    /// Only present on the first authorization.
    pub given_name: Option<String>,
    pub family_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppleSignInError {
    /// User dismissed the sheet (`ASAuthorizationErrorCanceled`).
    Canceled,
    Failed(String),
}

impl std::fmt::Display for AppleSignInError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Canceled => write!(f, "Sign in with Apple was canceled"),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for AppleSignInError {}

type Callback = Box<dyn FnOnce(Result<AppleCredential, AppleSignInError>)>;

thread_local! {
    static PENDING: RefCell<Option<(Callback, id, id)>> = const { RefCell::new(None) };
}

#[link(name = "AuthenticationServices", kind = "framework")]
unsafe extern "C" {
    static ASAuthorizationScopeEmail: id;
    static ASAuthorizationScopeFullName: id;
}

/// Present the system Sign in with Apple sheet. `hashed_nonce` is the hex
/// SHA-256 of a fresh random nonce. `callback` runs once on the main thread.
/// Starting a new request cancels the callback of any in-flight one.
pub fn sign_in_with_apple(
    hashed_nonce: &str,
    callback: impl FnOnce(Result<AppleCredential, AppleSignInError>) + 'static,
) {
    finish(Err(AppleSignInError::Canceled));
    unsafe {
        let provider: id = msg_send![class!(ASAuthorizationAppleIDProvider), new];
        let request: id = msg_send![provider, createRequest];
        let scopes: id = msg_send![
            class!(NSArray),
            arrayWithObjects: [ASAuthorizationScopeEmail, ASAuthorizationScopeFullName].as_ptr()
            count: 2usize
        ];
        let _: () = msg_send![request, setRequestedScopes: scopes];
        let _: () = msg_send![request, setNonce: ns_string(hashed_nonce)];
        let requests: id = msg_send![class!(NSArray), arrayWithObject: request];
        let controller: id = msg_send![class!(ASAuthorizationController), alloc];
        let controller: id = msg_send![controller, initWithAuthorizationRequests: requests];
        let delegate: id = msg_send![delegate_class(), new];
        let _: () = msg_send![controller, setDelegate: delegate];
        let _: () = msg_send![controller, setPresentationContextProvider: delegate];
        let _: () = msg_send![provider, release];
        // The controller holds its delegate weakly; keep both alive until done.
        PENDING.with(|p| *p.borrow_mut() = Some((Box::new(callback), controller, delegate)));
        let _: () = msg_send![controller, performRequests];
    }
}

fn finish(result: Result<AppleCredential, AppleSignInError>) {
    let Some((callback, controller, delegate)) = PENDING.with(|p| p.borrow_mut().take()) else {
        return;
    };
    callback(result);
    unsafe {
        let _: () = msg_send![controller, setDelegate: nil];
        let _: () = msg_send![controller, release];
        let _: () = msg_send![delegate, release];
    }
}

fn delegate_class() -> &'static Class {
    static CLASS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let ptr = *CLASS.get_or_init(|| {
        let mut decl = ClassDecl::new("GPUIAppleSignInDelegate", class!(NSObject)).expect("apple delegate");
        unsafe {
            decl.add_method(
                sel!(authorizationController:didCompleteWithAuthorization:),
                did_complete as extern "C" fn(&Object, Sel, id, id),
            );
            decl.add_method(
                sel!(authorizationController:didCompleteWithError:),
                did_fail as extern "C" fn(&Object, Sel, id, id),
            );
            decl.add_method(
                sel!(presentationAnchorForAuthorizationController:),
                anchor as extern "C" fn(&Object, Sel, id) -> id,
            );
        }
        decl.register() as *const Class as usize
    });
    unsafe { &*(ptr as *const Class) }
}

unsafe fn optional_string(s: id) -> Option<String> {
    let s = unsafe { to_str(s) };
    (!s.is_empty()).then(|| s.to_string())
}

unsafe fn data_string(data: id) -> Option<String> {
    if data.is_null() {
        return None;
    }
    unsafe {
        let len: usize = msg_send![data, length];
        let ptr: *const u8 = msg_send![data, bytes];
        if ptr.is_null() {
            return None;
        }
        Some(String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned())
    }
}

extern "C" fn did_complete(_: &Object, _: Sel, _controller: id, authorization: id) {
    let result = unsafe {
        let credential: id = msg_send![authorization, credential];
        let is_apple_id: objc::runtime::BOOL =
            msg_send![credential, isKindOfClass: class!(ASAuthorizationAppleIDCredential)];
        if credential.is_null() || is_apple_id == objc::runtime::NO {
            Err(AppleSignInError::Failed("unexpected credential type".into()))
        } else {
            let token: id = msg_send![credential, identityToken];
            let code: id = msg_send![credential, authorizationCode];
            let name: id = msg_send![credential, fullName];
            let (given, family) = if name.is_null() {
                (None, None)
            } else {
                let given: id = msg_send![name, givenName];
                let family: id = msg_send![name, familyName];
                (optional_string(given), optional_string(family))
            };
            let user: id = msg_send![credential, user];
            let email: id = msg_send![credential, email];
            match data_string(token) {
                Some(identity_token) => Ok(AppleCredential {
                    user: to_str(user).to_string(),
                    identity_token,
                    authorization_code: data_string(code),
                    email: optional_string(email),
                    given_name: given,
                    family_name: family,
                }),
                None => Err(AppleSignInError::Failed("Apple returned no identity token".into())),
            }
        }
    };
    finish(result);
}

extern "C" fn did_fail(_: &Object, _: Sel, _controller: id, error: id) {
    let result = unsafe {
        let code: isize = msg_send![error, code];
        // ASAuthorizationErrorCanceled = 1001
        if code == 1001 {
            AppleSignInError::Canceled
        } else {
            let desc: id = msg_send![error, localizedDescription];
            log::warn!("sign in with apple failed ({code}): {}", to_str(desc));
            AppleSignInError::Failed(format!("Sign in with Apple failed: {}", to_str(desc)))
        }
    };
    finish(Err(result));
}

extern "C" fn anchor(_: &Object, _: Sel, _controller: id) -> id {
    unsafe {
        let app: id = msg_send![class!(UIApplication), sharedApplication];
        let windows: id = msg_send![app, windows];
        let count: usize = msg_send![windows, count];
        let mut fallback = nil;
        for i in 0..count {
            let window: id = msg_send![windows, objectAtIndex: i];
            let key: objc::runtime::BOOL = msg_send![window, isKeyWindow];
            if key != objc::runtime::NO {
                return window;
            }
            if fallback.is_null() {
                fallback = window;
            }
        }
        fallback
    }
}
