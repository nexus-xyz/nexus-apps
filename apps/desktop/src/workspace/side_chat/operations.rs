//! An operation's answer belongs to the side chat, independently of any native window.
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui_kit::{App, Task};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    future::Future,
    rc::{Rc, Weak},
};

pub(super) struct Pending<T>(Rc<Inner<T>>);
struct Inner<T> {
    valid: Rc<Cell<bool>>,
    guard: RefCell<Rc<dyn Fn() -> bool>>,
    busy: Cell<bool>,
    result: RefCell<Option<T>>,
    wake: RefCell<Option<UnboundedSender<()>>>,
    task: RefCell<Option<Task<()>>>,
}
impl<T> Clone for Pending<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> Default for Pending<T> {
    fn default() -> Self {
        Self(Rc::new(Inner {
            valid: Rc::new(Cell::new(true)),
            guard: RefCell::new(Rc::new(|| true)),
            busy: Cell::new(false),
            result: RefCell::new(None),
            wake: RefCell::new(None),
            task: RefCell::new(None),
        }))
    }
}
#[derive(Clone)]
pub(super) struct Permit(Weak<Cell<bool>>, Rc<dyn Fn() -> bool>);
impl Permit {
    pub(super) fn check(&self) -> Result<(), String> {
        if self.0.upgrade().is_some_and(|valid| valid.get()) && (self.1)() {
            Ok(())
        } else {
            Err("The side chat's account or source changed.".into())
        }
    }
}
impl<T: 'static> Pending<T> {
    pub(super) fn valid(&self) -> bool {
        self.0.valid.get() && (self.0.guard.borrow())()
    }
    pub(super) fn guard(&self, guard: Rc<dyn Fn() -> bool>) {
        *self.0.guard.borrow_mut() = guard;
    }
    pub(super) fn busy(&self) -> bool {
        self.0.busy.get()
    }
    pub(super) fn occupied(&self) -> bool {
        self.busy() || self.0.result.borrow().is_some()
    }

    pub(super) fn permit(&self) -> Permit {
        Permit(Rc::downgrade(&self.0.valid), self.0.guard.borrow().clone())
    }
    pub(super) fn subscribe(&self) -> UnboundedReceiver<()> {
        let (send, read) = unbounded();
        // One bounded result slot; subscribing also picks up completion while parked.
        if self.0.result.borrow().is_some() {
            let _ = send.unbounded_send(());
        }
        *self.0.wake.borrow_mut() = Some(send);
        read
    }
    pub(super) fn detach(&self) {
        self.0.wake.borrow_mut().take();
    }
    pub(super) fn revoke(&self) {
        self.0.valid.set(false);
        self.detach();
        self.0.task.borrow_mut().take();
        self.0.result.borrow_mut().take();
        self.0.busy.set(false);
    }
    pub(super) fn inspect<R>(&self, read: impl FnOnce(Option<&T>) -> R) -> R {
        read(self.0.result.borrow().as_ref())
    }
    pub(super) fn take(&self) -> Option<T> {
        let result = self.0.result.borrow_mut().take();
        if result.is_some() {
            self.0.task.borrow_mut().take();
        }
        result
    }
    pub(super) fn start(&self, work: impl Future<Output = T> + 'static, cx: &mut App) -> bool {
        if !self.valid() || self.busy() || self.0.result.borrow().is_some() {
            return false;
        }
        self.0.busy.set(true);
        let weak = Rc::downgrade(&self.0);
        *self.0.task.borrow_mut() = Some(cx.spawn(async move |_| {
            let result = work.await;
            let Some(inner) = weak
                .upgrade()
                .filter(|inner| inner.valid.get() && (inner.guard.borrow())())
            else {
                return;
            };
            *inner.result.borrow_mut() = Some(result);
            inner.busy.set(false);
            if let Some(wake) = inner.wake.borrow().as_ref() {
                let _ = wake.unbounded_send(());
            }
        }));
        true
    }
}

/// A message the person sent, waiting in the side chat's line (`commands.rs`): it left the composer at once.
pub(super) struct Outgoing {
    pub text: String,
    pub draft: crate::attachments::Draft,
    /// Wait for the running turn to end instead of joining it (Ctrl+Enter, Queue), as the session's `queued`.
    pub queued: bool,
}

pub(super) enum Effect {
    /// The message being sent: on a failure, its text and files go back to the composer.
    Send {
        text: String,
        draft: crate::attachments::Draft,
    },
    Command {
        close: bool,
        note: Option<String>,
        handing_back: bool,
    },
    Rewrite {
        seq: i64,
        text: String,
    },
    Secret,
}
pub(super) struct Completion {
    pub effect: Effect,
    pub result: Result<Value, String>,
}
