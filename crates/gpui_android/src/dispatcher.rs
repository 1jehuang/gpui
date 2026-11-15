use android_activity::AndroidAppWaker;
use gpui::{PlatformDispatcher, Priority, RunnableVariant};
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::{Arc, mpsc},
    thread::ThreadId,
    time::Duration,
};

pub(crate) struct AndroidDispatcher {
    main: ThreadId,
    queue: Mutex<VecDeque<RunnableVariant>>,
    background: mpsc::Sender<RunnableVariant>,
    waker: AndroidAppWaker,
}
impl AndroidDispatcher {
    pub fn new(waker: AndroidAppWaker) -> Arc<Self> {
        let (background, receiver) = mpsc::channel::<RunnableVariant>();
        let receiver = Arc::new(Mutex::new(receiver));
        let count = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4));
        for index in 0..count {
            let receiver = receiver.clone();
            std::thread::Builder::new()
                .name(format!("gpui-android-worker-{index}"))
                .spawn(move || {
                    loop {
                        let task = receiver.lock().recv();
                        match task {
                            Ok(task) => {
                                task.run();
                            }
                            Err(_) => break,
                        }
                    }
                })
                .expect("failed to start Android executor worker");
        }
        Arc::new(Self {
            background,
            main: std::thread::current().id(),
            queue: Mutex::new(VecDeque::new()),
            waker,
        })
    }
    pub fn drain(&self) {
        loop {
            let task = self.queue.lock().pop_front();
            match task {
                Some(task) => {
                    task.run();
                }
                None => break,
            }
        }
    }
}
impl PlatformDispatcher for AndroidDispatcher {
    fn is_main_thread(&self) -> bool {
        std::thread::current().id() == self.main
    }
    fn dispatch(&self, runnable: RunnableVariant, _: Priority) {
        let _ = self.background.send(runnable);
    }
    fn dispatch_on_main_thread(&self, runnable: RunnableVariant, _: Priority) {
        self.queue.lock().push_back(runnable);
        self.waker.wake();
    }
    fn dispatch_after(&self, duration: Duration, runnable: RunnableVariant) {
        std::thread::spawn(move || {
            std::thread::sleep(duration);
            runnable.run();
        });
    }
    fn spawn_realtime(&self, f: Box<dyn FnOnce() + Send>) {
        std::thread::spawn(f);
    }
}
