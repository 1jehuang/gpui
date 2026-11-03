use dispatch2::{DispatchQueue, DispatchQueueGlobalPriority, DispatchTime, GlobalQueueIdentifier};
use gpui::{PlatformDispatcher, Priority, RunnableMeta, RunnableVariant};

use async_task::Runnable;
use objc::{class, msg_send, runtime::BOOL, runtime::YES, sel, sel_impl};
use std::{ffi::c_void, ptr::NonNull, time::Duration};

/// Grand Central Dispatch based dispatcher, shared design with macOS.
pub(crate) struct IosDispatcher;

impl PlatformDispatcher for IosDispatcher {
    fn is_main_thread(&self) -> bool {
        let is_main: BOOL = unsafe { msg_send![class!(NSThread), isMainThread] };
        is_main == YES
    }

    fn dispatch(&self, runnable: RunnableVariant, priority: Priority) {
        let context = runnable.into_raw().as_ptr() as *mut c_void;
        let queue_priority = match priority {
            Priority::RealtimeAudio | Priority::High => DispatchQueueGlobalPriority::High,
            Priority::Medium => DispatchQueueGlobalPriority::Default,
            Priority::Low => DispatchQueueGlobalPriority::Low,
        };
        unsafe {
            DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(queue_priority))
                .exec_async_f(context, trampoline);
        }
    }

    fn dispatch_on_main_thread(&self, runnable: RunnableVariant, _priority: Priority) {
        let context = runnable.into_raw().as_ptr() as *mut c_void;
        unsafe { DispatchQueue::main().exec_async_f(context, trampoline) };
    }

    fn dispatch_after(&self, duration: Duration, runnable: RunnableVariant) {
        let context = runnable.into_raw().as_ptr() as *mut c_void;
        let queue = DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(
            DispatchQueueGlobalPriority::High,
        ));
        let when = DispatchTime::NOW.time(duration.as_nanos() as i64);
        unsafe { DispatchQueue::exec_after_f(when, &queue, context, trampoline) };
    }

    fn spawn_realtime(&self, f: Box<dyn FnOnce() + Send>) {
        std::thread::spawn(f);
    }
}

extern "C" fn trampoline(context: *mut c_void) {
    let runnable =
        unsafe { Runnable::<RunnableMeta>::from_raw(NonNull::new_unchecked(context as *mut ())) };
    runnable.run();
}
