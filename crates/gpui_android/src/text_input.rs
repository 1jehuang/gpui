//! Thread-safe bridge from Android's InputConnection into the GPUI UI thread.
//!
//! The Java input connection is an OS service adapter only. All text state,
//! layout, selection, and painting continue to live in GPUI's InputHandler.
use android_activity::AndroidAppWaker;
use parking_lot::Mutex;
use std::collections::VecDeque;

/// An edit delivered by the activity's input connection.
#[derive(Debug)]
pub enum AndroidTextInput {
    Commit(String),
    Compose(String),
    FinishComposition,
    DeleteSurrounding { before: usize, after: usize },
    Enter,
}

static INPUT: Mutex<(VecDeque<AndroidTextInput>, Option<AndroidAppWaker>)> =
    Mutex::new((VecDeque::new(), None));

pub(crate) fn initialize(waker: AndroidAppWaker) {
    INPUT.lock().1 = Some(waker);
}

/// Queues an IME edit from any thread and wakes the native event loop.
/// JNI callbacks must not directly update GPUI entities on Android's Java UI thread.
pub fn dispatch_text_input(input: AndroidTextInput) {
    let mut state = INPUT.lock();
    state.0.push_back(input);
    if let Some(waker) = &state.1 {
        waker.wake();
    }
}

pub(crate) fn drain() -> Vec<AndroidTextInput> {
    INPUT.lock().0.drain(..).collect()
}
