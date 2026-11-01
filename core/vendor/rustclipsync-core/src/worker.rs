//! Long-poll notifications carry metadata only. State always remains authoritative.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use crate::{state::Persisted, transport::Transport, Error};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Connected,
    Changed,
    Disconnected,
}

pub struct Worker {
    state: Arc<Mutex<Persisted>>,
    stop: Arc<AtomicBool>,
    // Only consuming join can take this private handle. Unwinding can at most
    // detach it; no partially joined state is exposed for callers to reuse.
    // Limit the assertion to the handle so Worker retains its unwind traits.
    thread: Option<std::panic::AssertUnwindSafe<thread::JoinHandle<()>>>,
}

impl Worker {
    pub fn start<R: Transport + 'static>(
        client: Arc<R>,
        state: Persisted,
        generation: u64,
        events: SyncSender<(u64, Event)>,
    ) -> Self {
        let state = Arc::new(Mutex::new(state.notification_identity()));
        let stop = Arc::new(AtomicBool::new(false));
        let current = Arc::clone(&state);
        let stopped = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let mut online = false;
            let mut delay = 1;
            let mut pending_connection = None;
            'poll: while !stopped.load(Ordering::SeqCst) {
                if !deliver(&events, generation, &mut pending_connection) {
                    break;
                }
                let snapshot = match current.lock() {
                    Ok(value) => value.clone(),
                    Err(_) => break,
                };
                match client.changes(&snapshot) {
                    Ok(result) => {
                        if stopped.load(Ordering::SeqCst) {
                            break;
                        }
                        if !online {
                            pending_connection = Some(Event::Connected);
                        }
                        if !deliver(&events, generation, &mut pending_connection) {
                            break;
                        }
                        online = true;
                        delay = 1;
                        if result.is_some() {
                            let _ = events.try_send((generation, Event::Changed));
                            // Coalesce repeated notices while the main loop handles the revision.
                            for _ in 0..10 {
                                if stopped.load(Ordering::SeqCst)
                                    || !deliver(&events, generation, &mut pending_connection)
                                {
                                    break 'poll;
                                }
                                thread::sleep(Duration::from_millis(100));
                                if current
                                    .lock()
                                    .is_ok_and(|value| value.state.cursor != snapshot.state.cursor)
                                {
                                    break;
                                }
                            }
                        }
                    }
                    Err(Error::Status(400 | 401 | 403 | 409)) => {
                        // Wake the main loop to classify revocation or epoch mismatch.
                        // These are the last events: unlike recurring change notices,
                        // they must survive a full queue before the worker exits.
                        pending_connection = Some(Event::Disconnected);
                        let mut changed = Some(Event::Changed);
                        while !stopped.load(Ordering::SeqCst) {
                            if !deliver(&events, generation, &mut pending_connection)
                                || (pending_connection.is_none()
                                    && !deliver(&events, generation, &mut changed))
                                || (pending_connection.is_none() && changed.is_none())
                            {
                                break;
                            }
                            thread::sleep(Duration::from_millis(100));
                        }
                        break;
                    }
                    Err(_) => {
                        if online {
                            pending_connection = Some(Event::Disconnected);
                        }
                        online = false;
                        for _ in 0..delay * 10 {
                            if stopped.load(Ordering::SeqCst)
                                || !deliver(&events, generation, &mut pending_connection)
                            {
                                break 'poll;
                            }
                            thread::sleep(Duration::from_millis(100));
                        }
                        delay = (delay * 2).min(30);
                    }
                }
            }
        });
        Self {
            state,
            stop,
            thread: Some(std::panic::AssertUnwindSafe(thread)),
        }
    }

    pub fn update(&self, state: &Persisted) {
        if let Ok(mut value) = self.state.lock() {
            *value = state.notification_identity();
        }
    }

    /// Request shutdown without waiting for an in-flight transport request.
    /// Repeated calls are harmless. [`Self::join`] waits for the thread to exit.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Request shutdown and wait until the polling thread has exited.
    ///
    /// This blocks until an in-flight [`Transport::changes`] call returns, so
    /// call it away from the UI thread. The built-in client bounds that request
    /// to 30 seconds; custom transports must also enforce a finite timeout.
    /// Dropping the worker requests shutdown without waiting.
    ///
    /// # Errors
    /// Returns the panic payload if the polling thread panicked.
    pub fn join(mut self) -> thread::Result<()> {
        self.shutdown();
        match self.thread.take() {
            Some(thread) => thread.0.join(),
            None => Ok(()),
        }
    }
}

/// Retain the latest connection state until delivered; a closed receiver stops polling.
fn deliver(
    events: &SyncSender<(u64, Event)>,
    generation: u64,
    pending: &mut Option<Event>,
) -> bool {
    let Some(event) = *pending else {
        return true;
    };
    match events.try_send((generation, event)) {
        Ok(()) => {
            *pending = None;
            true
        }
        Err(TrySendError::Full(_)) => true,
        Err(TrySendError::Disconnected(_)) => false,
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}
