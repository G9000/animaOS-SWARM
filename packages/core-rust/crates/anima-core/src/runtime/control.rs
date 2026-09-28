//! Cooperative control of one run (spec §4.6–§4.7): a stop signal the
//! runtime checks at its checkpoints, and a steering inbox it drains before
//! each model call. Both are shared with the host by cloning.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::primitives::{Content, LockRecover};

/// The error of a stopped run's result (spec §4.6).
pub const RUN_STOPPED_ERROR: &str = "stopped";
/// The result of a requested tool call that never ran because of a stop.
pub const CANCELLED_TOOL_RESULT: &str = "Cancelled before running (stopped by owner)";
/// Marks the partial text of a model call a stop interrupted.
pub const STOPPED_METADATA_KEY: &str = "stopped";
/// Marks an owner message steered into a running run.
pub const STEER_METADATA_KEY: &str = "steer";

#[derive(Debug, Default)]
struct CancelState {
    cancelled: AtomicBool,
    waiters: Mutex<Vec<Waker>>,
}

/// A one-way stop flag. Cancelling wakes every task waiting on it.
#[derive(Clone, Debug, Default)]
pub struct CancelSignal {
    state: Arc<CancelState>,
}

impl CancelSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        if !self.state.cancelled.swap(true, Ordering::SeqCst) {
            let waiters = std::mem::take(&mut *self.state.waiters.lock_recover());
            for waiter in waiters {
                waiter.wake();
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves once the signal is cancelled (at once if it already is).
    pub fn cancelled(&self) -> CancelWait {
        CancelWait {
            signal: self.clone(),
        }
    }
}

/// The future `CancelSignal::cancelled` returns.
#[derive(Debug)]
pub struct CancelWait {
    signal: CancelSignal,
}

impl Future for CancelWait {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.signal.is_cancelled() {
            return Poll::Ready(());
        }
        {
            let mut waiters = self.signal.state.waiters.lock_recover();
            if !waiters
                .iter()
                .any(|waiter| waiter.will_wake(context.waker()))
            {
                waiters.push(context.waker().clone());
            }
        }
        // `cancel` sets the flag before it takes the waiters, so a cancel
        // racing the registration above is seen here.
        if self.signal.is_cancelled() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

#[derive(Debug, Default)]
struct SteeringState {
    items: VecDeque<Content>,
    closed: bool,
}

/// Owner messages waiting to join a running run (spec §4.7).
#[derive(Clone, Debug, Default)]
pub struct SteeringInbox {
    state: Arc<Mutex<SteeringState>>,
}

impl SteeringInbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues `content` for the next model call; gives it back once the
    /// inbox is closed (the run is finishing), so the host can queue a run.
    pub fn push(&self, content: Content) -> Result<(), Content> {
        let mut state = self.state.lock_recover();
        if state.closed {
            return Err(content);
        }
        state.items.push_back(content);
        Ok(())
    }

    /// Everything waiting, oldest first; the inbox is left empty.
    pub fn drain(&self) -> Vec<Content> {
        self.state.lock_recover().items.drain(..).collect()
    }

    /// A copy of everything waiting, oldest first.
    pub fn pending(&self) -> Vec<Content> {
        self.state.lock_recover().items.iter().cloned().collect()
    }

    /// Refuses further items and returns the ones never drained, oldest first.
    pub fn close(&self) -> Vec<Content> {
        let mut state = self.state.lock_recover();
        state.closed = true;
        state.items.drain(..).collect()
    }

    pub fn is_closed(&self) -> bool {
        self.state.lock_recover().closed
    }
}

/// The controls a host holds for one run.
#[derive(Clone, Debug, Default)]
pub struct RunControl {
    pub cancel: CancelSignal,
    pub steering: SteeringInbox,
}

impl RunControl {
    pub fn new() -> Self {
        Self::default()
    }
}
