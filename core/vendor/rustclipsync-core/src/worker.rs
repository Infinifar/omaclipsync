//! Long-poll notifications carry metadata only. State always remains authoritative.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
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
        thread::spawn(move || {
            let mut online = false;
            let mut delay = 1;
            while !stopped.load(Ordering::SeqCst) {
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
                            let _ = events.try_send((generation, Event::Connected));
                        }
                        online = true;
                        delay = 1;
                        if result.is_some() {
                            let _ = events.try_send((generation, Event::Changed));
                            // Coalesce repeated notices while the main loop handles the revision.
                            for _ in 0..10 {
                                if stopped.load(Ordering::SeqCst) {
                                    break;
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
                        let _ = events.try_send((generation, Event::Changed));
                        if online {
                            let _ = events.try_send((generation, Event::Disconnected));
                        }
                        break;
                    }
                    Err(_) => {
                        if online {
                            let _ = events.try_send((generation, Event::Disconnected));
                        }
                        online = false;
                        for _ in 0..delay * 10 {
                            if stopped.load(Ordering::SeqCst) {
                                break;
                            }
                            thread::sleep(Duration::from_millis(100));
                        }
                        delay = (delay * 2).min(30);
                    }
                }
            }
        });
        Self { state, stop }
    }

    pub fn update(&self, state: &Persisted) {
        if let Ok(mut value) = self.state.lock() {
            *value = state.notification_identity();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}
