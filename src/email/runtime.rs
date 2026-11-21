// SPDX-License-Identifier: GPL-3.0-only

use super::document::BlockBackgroundNetwork;
use cosmic::iced::{Subscription, futures};
use servo::{EventLoopWaker, Servo, ServoBuilder};
use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use tokio::sync::Notify;
use url::Url;

#[derive(Clone, Default)]
struct WakeSignal(Arc<Notify>);

impl Hash for WakeSignal {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

impl EventLoopWaker for WakeSignal {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        self.0.notify_one();
    }
}

pub(super) struct Runtime {
    pub(super) servo: Servo,
    wake: WakeSignal,
    next_message_id: Cell<u64>,
}

impl Runtime {
    fn new() -> Self {
        let wake = WakeSignal::default();
        let servo = ServoBuilder::default()
            .event_loop_waker(Box::new(wake.clone()))
            .build();
        servo.set_delegate(Rc::new(BlockBackgroundNetwork));
        Self {
            servo,
            wake,
            next_message_id: Cell::new(0),
        }
    }

    pub(super) fn shared() -> Rc<Self> {
        RUNTIME.with(Rc::clone)
    }

    pub(super) fn wake(&self) {
        self.wake.wake();
    }

    pub(super) fn subscription(&self) -> Subscription<()> {
        Subscription::run_with(self.wake.clone(), |wake| {
            futures::stream::unfold(wake.clone(), |wake| async move {
                wake.0.notified().await;
                Some(((), wake))
            })
        })
    }

    pub(super) fn message_url(&self) -> Url {
        let id = self.next_message_id.get();
        self.next_message_id.set(id + 1);
        Url::parse(&format!("https://cosmic-mail.invalid/message/{id}")).expect("valid message URL")
    }
}

thread_local! {
    static RUNTIME: Rc<Runtime> = Rc::new(Runtime::new());
}
