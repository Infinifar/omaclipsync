//! Shell-owned daemon. Stdout is exclusively the versioned JSON-lines protocol.
use chrono::Utc;
use omaclipsync_core::{
    config::{Config, ConfigError, SyncDirection},
    history::{Entry, History, HistoryError},
    inbox, keyring,
    logging::{Event, Level, Logger},
    protocol::{self, Request},
    session::{self, SyncError},
    wayland,
    worker::{self, Event as NotificationEvent},
    Error, Outcome,
};
use serde_json::{json, Value};
use std::{
    env, fs,
    io::{self, BufReader, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use wl_clipboard_rs::watch::CancelHandle;

const ID: &str = "omaclipsync";
enum WatchCancel {
    Native(CancelHandle),
}
impl WatchCancel {
    fn cancel(self) {
        match self {
            Self::Native(handle) => handle.cancel(),
        }
    }
}
enum Message {
    Request(Request),
    InvalidRequest,
    Clipboard,
    WatcherReady(u64, WatchCancel),
    WatcherEnded(u64),
    ClipboardError(u64),
    End,
}
struct Paths {
    config: PathBuf,
    history: PathBuf,
    logs: PathBuf,
}
fn xdg(key: &str, fallback: &str) -> Option<PathBuf> {
    match env::var_os(key) {
        Some(path) if PathBuf::from(&path).is_absolute() => Some(PathBuf::from(path)),
        _ => Some(PathBuf::from(env::var_os("HOME")?).join(fallback)),
    }
}
impl Paths {
    fn from_env() -> Option<Self> {
        Some(Self {
            config: xdg("XDG_CONFIG_HOME", ".config")?
                .join(ID)
                .join("config.json"),
            history: xdg("XDG_DATA_HOME", ".local/share")?
                .join(ID)
                .join("history.json"),
            logs: xdg("XDG_STATE_HOME", ".local/state")?.join(ID),
        })
    }
}
struct App {
    paths: Paths,
    config: Config,
    session: Option<session::Session<session::WaylandClipboard>>,
    worker: Option<worker::Worker>,
    conflict: bool,
    clipboard_latest: Arc<Mutex<Option<(u64, u64, wayland::ClipboardChange)>>>,
    clipboard_serial: Arc<AtomicU64>,
    processed_clipboard_serial: u64,
    active_watcher: Arc<Mutex<u64>>,
    state: &'static str,
    last_sync: Option<u64>,
    log_error: bool,
    logger: Logger,
    next_poll: Instant,
    next_fallback: Instant,
    next_notification_pull: Instant,
    pull_pending: bool,
    notification_tx: SyncSender<(u64, NotificationEvent)>,
    notification_rx: Receiver<(u64, NotificationEvent)>,
    notification_generation: u64,
    notification_connected: bool,
    next_config_check: Instant,
    backoff: u64,
    config_stamp: Option<(SystemTime, u64)>,
    sender: SyncSender<Message>,
    watcher_active: bool,
    watcher_cancel: Option<WatchCancel>,
    watcher_generation: u64,
    watcher_retry: Instant,
    clipboard_error: bool,
    needs_check: bool,
    verbose: bool,
    dedupe_full: bool,
    config_revision: u64,
}
impl App {
    fn new(paths: Paths, sender: SyncSender<Message>, verbose: bool) -> Self {
        let logger = Logger::new(&paths.logs);
        let now = Instant::now();
        let (notification_tx, notification_rx) = mpsc::sync_channel(32);
        let mut app = Self {
            paths,
            config: Config::default(),
            session: None,
            worker: None,
            conflict: false,
            clipboard_latest: Arc::new(Mutex::new(None)),
            clipboard_serial: Arc::new(AtomicU64::new(0)),
            processed_clipboard_serial: 0,
            active_watcher: Arc::new(Mutex::new(0)),
            state: "needs_config",
            last_sync: None,
            log_error: false,
            logger,
            next_poll: now,
            next_fallback: now,
            next_notification_pull: now,
            pull_pending: false,
            notification_tx,
            notification_rx,
            notification_generation: 0,
            notification_connected: false,
            next_config_check: now,
            backoff: 1,
            config_stamp: None,
            sender,
            watcher_active: false,
            watcher_cancel: None,
            watcher_generation: 0,
            watcher_retry: now,
            clipboard_error: false,
            needs_check: true,
            verbose,
            dedupe_full: false,
            config_revision: 0,
        };
        app.log(Event::Started);
        app.reload();
        app
    }
    fn log(&mut self, event: Event) {
        self.log_error = self.logger.write(event, Utc::now()).is_err();
    }
    fn stamp(&self) -> Option<(SystemTime, u64)> {
        let m = fs::metadata(&self.paths.config).ok()?;
        Some((m.modified().ok()?, m.len()))
    }
    fn reload(&mut self) {
        self.config_stamp = self.stamp();
        let config = match Config::read(&self.paths.config) {
            Ok(c) => c,
            Err(ConfigError::Io(e)) if e.kind() == io::ErrorKind::NotFound => Config::default(),
            Err(_) => {
                self.stop_notifications();
                self.session = None;
                self.state = "config_error";
                self.log(Event::ConfigError);
                self.stop_watcher();
                return;
            }
        };
        self.apply(config);
    }
    fn apply(&mut self, config: Config) {
        self.stop_notifications();
        self.stop_watcher();
        self.session = None;
        self.conflict = false;
        // A watcher captures this policy at startup. Cancel it and discard
        // queued clipboard data before adopting a changed sensitive policy.
        if self.config.sync.allow_sensitive != config.sync.allow_sensitive {
            self.stop_watcher();
        }
        let filter = match config.validate() {
            Ok(f) => f,
            Err(_) => {
                self.state = "config_error";
                self.stop_watcher();
                return;
            }
        };
        let history = match History::new(
            &self.paths.history,
            config.history.enabled,
            config.history.limit,
        ) {
            Ok(h) => h,
            Err(_) => {
                self.state = "config_error";
                self.stop_watcher();
                return;
            }
        };
        let ready = !config.server.url.is_empty() && !config.server.user.is_empty();
        self.config = config;
        self.config_revision = self.config_revision.wrapping_add(1);
        self.logger.set_level(if self.verbose {
            Level::Debug
        } else {
            Level::parse(&self.config.logging.level).unwrap_or(Level::Info)
        });
        if !ready {
            self.state = "needs_config";
            self.stop_watcher();
            return;
        }
        let password = match keyring::lookup(&self.config.server.user) {
            Ok(v) => v,
            Err(_) => {
                self.state = "credentials";
                self.stop_watcher();
                return;
            }
        };
        match session::Session::open(
            &self.config.server.url,
            &self.config.server.user,
            password,
            self.config.server.insecure,
            &self.paths.logs,
            session::WaylandClipboard,
            filter,
            self.config.sync.allow_sensitive,
            history,
            Arc::clone(&self.clipboard_serial),
        ) {
            Ok(mut session) => {
                session.use_sensitive_inbox(!self.config.sync.auto_apply_sensitive);
                self.session = Some(session);
            }
            Err(SyncError::Credentials) => {
                self.state = "credentials";
                return;
            }
            Err(_) => {
                self.state = "config_error";
                return;
            }
        }
        self.state = if self.config.sync.enabled {
            "connecting"
        } else {
            "paused"
        };
        self.backoff = 1;
        self.needs_check = true;
        self.pull_pending = true;
        self.next_fallback = Instant::now();
        self.next_notification_pull = Instant::now();
        // Let the host complete the protocol handshake before any network request.
        self.next_poll = Instant::now() + Duration::from_secs(1);
        if !self.config.sync.enabled {
            self.stop_watcher();
        }
    }
    fn stop_notifications(&mut self) {
        self.notification_generation = self.notification_generation.wrapping_add(1);
        self.notification_connected = false;
        self.worker = None;
        while self.notification_rx.try_recv().is_ok() {}
    }
    fn fallback_interval(&self) -> Duration {
        Duration::from_secs(self.config.sync.interval_sec.clamp(30, 300))
    }
    fn notification_events(&mut self, now: Instant) {
        // Bound work per tick even if an untrusted server floods notifications.
        for _ in 0..64 {
            let Ok((generation, event)) = self.notification_rx.try_recv() else {
                break;
            };
            if generation != self.notification_generation || !self.wants_watcher() {
                continue;
            }
            match event {
                NotificationEvent::Connected => {
                    self.notification_connected = true;
                    self.pull_pending = true;
                    self.next_notification_pull = now;
                    self.next_poll = now;
                }
                NotificationEvent::Changed => {
                    self.pull_pending = true;
                    if self.state != "offline" && self.state != "credentials" {
                        self.next_poll = now;
                    }
                }
                NotificationEvent::Disconnected => {
                    if self.notification_connected {
                        self.next_fallback = now + self.fallback_interval();
                    }
                    self.notification_connected = false;
                }
            }
        }
    }
    fn stop_watcher(&mut self) {
        self.watcher_generation += 1;
        if let Ok(mut active) = self.active_watcher.lock() {
            *active = self.watcher_generation;
            self.processed_clipboard_serial =
                self.clipboard_serial.fetch_add(1, Ordering::SeqCst) + 1;
        }
        self.watcher_active = false;
        if let Some(cancel) = self.watcher_cancel.take() {
            cancel.cancel();
        }
        if let Ok(mut latest) = self.clipboard_latest.lock() {
            *latest = None;
        }
    }
    fn wants_watcher(&self) -> bool {
        self.config.sync.enabled && self.session.is_some()
    }
    fn watcher_ready(&mut self, now: Instant) {
        self.clipboard_error = false;
        self.pull_pending = true;
        // Re-establishing the local listener must not defeat network or
        // authentication backoff; otherwise a flapping watcher can trigger
        // repeated HTTP requests while the server remains unavailable.
        if self.state != "offline" && self.state != "credentials" {
            self.next_poll = now;
        }
    }
    fn ensure_watcher(&mut self, now: Instant) {
        if !self.wants_watcher() || self.watcher_active || now < self.watcher_retry {
            return;
        }
        self.watcher_active = true;
        self.watcher_generation += 1;
        let generation = self.watcher_generation;
        if let Ok(mut active) = self.active_watcher.lock() {
            *active = generation;
            self.processed_clipboard_serial = self.clipboard_serial.load(Ordering::SeqCst);
        }
        let allow_sensitive = self.config.sync.allow_sensitive;
        let tx = self.sender.clone();
        let clipboard_latest = Arc::clone(&self.clipboard_latest);
        let serial = Arc::clone(&self.clipboard_serial);
        let active = Arc::clone(&self.active_watcher);
        let changed_active = Arc::clone(&active);
        thread::spawn(move || {
            if let Ok(watcher) = wayland::create_watcher() {
                if tx
                    .send(Message::WatcherReady(
                        generation,
                        WatchCancel::Native(watcher.cancel_handle()),
                    ))
                    .is_err()
                {
                    return;
                }
                let result = wayland::watch_loop(
                    watcher,
                    allow_sensitive,
                    || {
                        if let Ok(active) = changed_active.lock() {
                            if *active == generation {
                                serial.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    },
                    |item| match item {
                        Ok(change) => {
                            if let Ok(active) = active.lock() {
                                if *active == generation {
                                    if let Ok(mut value) = clipboard_latest.lock() {
                                        *value = Some((
                                            generation,
                                            serial.load(Ordering::SeqCst),
                                            change,
                                        ));
                                    }
                                    let _ = tx.try_send(Message::Clipboard);
                                }
                            }
                        }
                        Err(_) => {
                            if let Ok(active) = active.lock() {
                                if *active == generation {
                                    if let Ok(mut value) = clipboard_latest.lock() {
                                        *value = Some((
                                            generation,
                                            serial.load(Ordering::SeqCst),
                                            wayland::ClipboardChange::Cleared,
                                        ));
                                    }
                                    let _ = tx.try_send(Message::ClipboardError(generation));
                                }
                            }
                        }
                    },
                );
                if result.is_err() {
                    let _ = tx.try_send(Message::ClipboardError(generation));
                }
            } else {
                // The old wl-paste fallback cannot reliably identify our
                // own selection or every clear; fail closed.
                let _ = tx.try_send(Message::ClipboardError(generation));
            }
            let _ = tx.send(Message::WatcherEnded(generation));
        });
    }
    fn status(&self) -> Value {
        let transport = if !self.config.sync.enabled || self.session.is_none() {
            "off"
        } else if !self.config.sync.direction.can_receive() {
            "push"
        } else if self.notification_connected {
            "realtime"
        } else {
            "polling"
        };
        json!({"state":self.state,"transport":transport,"enabled":self.config.sync.enabled,"direction":self.config.sync.direction,"url":self.config.server.url,"deviceId":self.session.as_ref().map(|s| &s.persisted.state.device_id),"inbox":inbox::metadata(self.session.as_ref().and_then(|s| s.persisted.inbox.as_ref())),"lastSync":self.last_sync,"historyError":self.session.as_ref().is_some_and(|s|s.history_error),"logError":self.log_error,"watching":self.watcher_cancel.is_some(),"clipboardError":self.clipboard_error,"dedupeFull":self.dedupe_full,"configRevision":self.config_revision})
    }
    fn stamp_sync(&mut self) {
        self.last_sync = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );
    }
    fn note_outcome(&mut self, outcome: &Outcome) {
        self.dedupe_full = *outcome == Outcome::Capacity;
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.history_error)
        {
            self.log(Event::HistoryError);
        }
        match outcome {
            Outcome::Uploaded => {
                self.stamp_sync();
                self.log(Event::Uploaded)
            }
            Outcome::Downloaded | Outcome::Stored => {
                self.stamp_sync();
                self.log(Event::Downloaded)
            }
            Outcome::Filtered => self.log(Event::Filtered),
            Outcome::Unsupported => self.log(Event::Unsupported),
            _ => {}
        }
    }
    fn tick(&mut self, out: &mut impl Write) {
        let now = Instant::now();
        if now >= self.next_config_check {
            self.next_config_check = now + Duration::from_secs(1);
            if self.stamp() != self.config_stamp {
                self.reload();
            }
        }
        self.notification_events(now);
        self.ensure_watcher(now);
        if self.session.is_some() {
            self.tick_sync(out, now);
        }
    }
    fn sync_failure(&mut self, error: SyncError) {
        self.state = match error {
            SyncError::Remote(Error::Status(401 | 403)) | SyncError::Credentials => "credentials",
            SyncError::Remote(Error::Status(409)) | SyncError::Conflict => {
                self.conflict = true;
                "conflict"
            }
            SyncError::Expired => {
                self.conflict = true;
                "expired"
            }
            SyncError::Epoch => {
                self.conflict = true;
                "re_pair"
            }
            SyncError::Storage(_) => {
                self.conflict = true;
                "storage_error"
            }
            SyncError::Clipboard(_) => "clipboard_error",
            _ => "offline",
        };
        self.next_poll = Instant::now() + Duration::from_secs(self.backoff);
        self.backoff = (self.backoff * 2).min(30);
    }

    fn tick_sync(&mut self, out: &mut impl Write, now: Instant) {
        let change = self
            .clipboard_latest
            .lock()
            .ok()
            .and_then(|mut latest| latest.take());
        if let Some((generation, serial, change)) = change {
            if generation == self.watcher_generation && self.wants_watcher() {
                self.processed_clipboard_serial = serial;
                if !self.config.sync.direction.can_push() {
                    return;
                }
                let result = match change {
                    wayland::ClipboardChange::Text {
                        text,
                        origin,
                        sensitive,
                    } => self.session.as_mut().unwrap().record_local(
                        &text,
                        origin.as_deref(),
                        sensitive,
                        now,
                    ),
                    wayland::ClipboardChange::Cleared => {
                        self.session.as_mut().unwrap().record_clear()
                    }
                };
                match result {
                    Ok(outcome) => {
                        self.note_outcome(&outcome);
                        if self.state != "offline" && self.state != "credentials" {
                            self.next_poll = now;
                        }
                    }
                    Err(error) => {
                        self.sync_failure(error);
                        return;
                    }
                }
            }
        }
        if !self.config.sync.enabled
            || self.session.is_none()
            || now < self.next_poll
            || self.conflict
        {
            return;
        }
        if self.clipboard_serial.load(Ordering::SeqCst) != self.processed_clipboard_serial {
            return;
        }
        if self.watcher_cancel.is_none() {
            self.state = "clipboard_error";
            self.next_poll = now + Duration::from_secs(1);
            return;
        }
        if self.needs_check {
            if let Err(error) = self.session.as_mut().unwrap().check() {
                self.sync_failure(error);
                return;
            }
            self.needs_check = false;
            self.pull_pending = true;
            if self.config.sync.direction.can_receive() {
                let session = self.session.as_ref().unwrap();
                self.worker = Some(worker::Worker::start(
                    Arc::clone(&session.remote),
                    session.persisted.notification_identity(),
                    self.notification_generation,
                    self.notification_tx.clone(),
                ));
            }
            self.state = "connected";
        }
        let pending = self.session.as_ref().unwrap().has_pending();
        let may_flush = self.config.sync.direction.can_push()
            || self
                .session
                .as_ref()
                .unwrap()
                .persisted
                .state
                .pending
                .is_none();
        let receive_due = self.config.sync.direction.can_receive()
            && ((self.pull_pending && now >= self.next_notification_pull)
                || (!self.notification_connected && now >= self.next_fallback));
        if !(receive_due || pending && may_flush) {
            return;
        }
        let _ = protocol::event(out, "activity", &json!({"active":true}));
        if pending && may_flush {
            if self.clipboard_serial.load(Ordering::SeqCst) != self.processed_clipboard_serial {
                let _ = protocol::event(out, "activity", &json!({"active":false}));
                return;
            }
            let observed = self.clipboard_serial.load(Ordering::SeqCst);
            match self.session.as_mut().unwrap().flush(observed) {
                Ok(outcome) => {
                    self.note_outcome(&outcome);
                    emit_sync_success(out, &outcome, self.config.sync.notify_on_success);
                }
                Err(error) => {
                    self.sync_failure(error);
                    let _ = protocol::event(out, "activity", &json!({"active":false}));
                    return;
                }
            }
        }
        if receive_due {
            if self.clipboard_serial.load(Ordering::SeqCst) != self.processed_clipboard_serial {
                let _ = protocol::event(out, "activity", &json!({"active":false}));
                return;
            }
            let observed = self.clipboard_serial.load(Ordering::SeqCst);
            match self.session.as_mut().unwrap().pull(observed, now) {
                Ok(outcome) => {
                    self.note_outcome(&outcome);
                    emit_sync_success(out, &outcome, self.config.sync.notify_on_success);
                    self.pull_pending = false;
                    self.next_fallback = Instant::now() + self.fallback_interval();
                    self.next_notification_pull = Instant::now() + Duration::from_millis(500);
                }
                Err(error) => {
                    self.sync_failure(error);
                    let _ = protocol::event(out, "activity", &json!({"active":false}));
                    return;
                }
            }
        }
        if let (Some(worker), Some(session)) = (&self.worker, &self.session) {
            worker.update(&session.persisted);
        }
        self.state = "connected";
        self.backoff = 1;
        self.next_poll = Instant::now();
        let _ = protocol::event(out, "activity", &json!({"active":false}));
    }
    fn handle(&mut self, request: Request) -> (u64, Result<Value, &'static str>, bool) {
        let id = request.id;
        let result = match request.method.as_str() {
            "hello" => Ok(protocol::hello()),
            "status" => Ok(self.status()),
            "get_config" => serde_json::to_value(&self.config).map_err(|_| "internal_error"),
            "set_config" => {
                match serde_json::from_value::<Config>(
                    request.params.get("config").cloned().unwrap_or(Value::Null),
                ) {
                    Ok(config) if config.validate().is_ok() => {
                        match config.write(&self.paths.config) {
                            Ok(()) => {
                                self.config_stamp = self.stamp();
                                self.apply(config);
                                Ok(self.status())
                            }
                            Err(_) => Err("config_write_failed"),
                        }
                    }
                    _ => Err("invalid_config"),
                }
            }
            "test_filter" => {
                let mut config = self.config.clone();
                let text = request
                    .params
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                match serde_json::from_value(
                    request.params.get("filter").cloned().unwrap_or(Value::Null),
                ) {
                    Ok(filter) if text.len() <= 65536 => {
                        config.filter = filter;
                        match config.validate() {
                            Ok(compiled) => Ok(
                                json!({"matched": compiled.matches(text), "allowed": compiled.allows(text, false)}),
                            ),
                            Err(_) => Err("invalid_filter"),
                        }
                    }
                    _ => Err("invalid_filter"),
                }
            }
            "re_pair" if self.session.is_some() => {
                if request.params.get("confirm").and_then(Value::as_bool) != Some(true) {
                    Err("confirmation_required")
                } else {
                    self.stop_notifications();
                    match self.session.as_mut().unwrap().re_pair() {
                        Ok(()) => {
                            self.conflict = false;
                            self.needs_check = true;
                            self.pull_pending = true;
                            self.state = "connecting";
                            self.next_poll = Instant::now();
                            Ok(self.status())
                        }
                        Err(error) => {
                            self.sync_failure(error);
                            Err("sync_failed")
                        }
                    }
                }
            }
            "set_password" => {
                let account = request
                    .params
                    .get("account")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let password = request
                    .params
                    .get("password")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                match keyring::store(account, password) {
                    Ok(()) => {
                        self.reload();
                        Ok(self.status())
                    }
                    Err(_) => Err("keyring_failed"),
                }
            }
            "toggle" => {
                let mut config = self.config.clone();
                config.sync.enabled = !config.sync.enabled;
                match config.write(&self.paths.config) {
                    Ok(()) => {
                        self.config_stamp = self.stamp();
                        self.apply(config);
                        Ok(self.status())
                    }
                    Err(_) => Err("config_write_failed"),
                }
            }
            "set_direction" | "cycle_direction" => {
                let direction = if request.method == "cycle_direction" {
                    Ok(match self.config.sync.direction {
                        SyncDirection::Push => SyncDirection::Receive,
                        SyncDirection::Receive => SyncDirection::Both,
                        SyncDirection::Both => SyncDirection::Push,
                    })
                } else {
                    serde_json::from_value(
                        request
                            .params
                            .get("direction")
                            .cloned()
                            .unwrap_or(Value::Null),
                    )
                };
                match direction {
                    Ok(direction) => {
                        let mut config = self.config.clone();
                        config.sync.direction = direction;
                        match config.write(&self.paths.config) {
                            Ok(()) => {
                                self.config_stamp = self.stamp();
                                self.apply(config);
                                Ok(self.status())
                            }
                            Err(_) => Err("config_write_failed"),
                        }
                    }
                    Err(_) => Err("invalid_config"),
                }
            }
            "push" if self.session.is_some() => {
                if self.watcher_cancel.is_none()
                    || self.clipboard_serial.load(Ordering::SeqCst)
                        != self.processed_clipboard_serial
                {
                    Err("clipboard_unavailable")
                } else {
                    if let Ok(mut latest) = self.clipboard_latest.lock() {
                        *latest = None;
                    }
                    let observed = self.clipboard_serial.load(Ordering::SeqCst);
                    match self.session.as_mut().unwrap().push_now(observed) {
                        Ok(outcome) => {
                            self.conflict = false;
                            self.state = "connected";
                            self.next_poll = Instant::now();
                            if let (Some(worker), Some(session)) = (&self.worker, &self.session) {
                                worker.update(&session.persisted);
                            }
                            self.note_outcome(&outcome);
                            Ok(json!({"outcome":outcome_name(&outcome)}))
                        }
                        Err(SyncError::Clipboard(session::ClipboardError::Sensitive)) => {
                            Err("sensitive_blocked")
                        }
                        Err(error) => {
                            self.sync_failure(error);
                            Err("sync_failed")
                        }
                    }
                }
            }
            "pull" if self.session.is_some() => {
                if request.params.get("force").and_then(Value::as_bool) == Some(true) {
                    Err("force_not_supported")
                } else if self.watcher_cancel.is_none()
                    || self.clipboard_serial.load(Ordering::SeqCst)
                        != self.processed_clipboard_serial
                {
                    Err("clipboard_unavailable")
                } else if self.conflict {
                    Err("sync_failed")
                } else {
                    let observed = self.clipboard_serial.load(Ordering::SeqCst);
                    match self
                        .session
                        .as_mut()
                        .unwrap()
                        .pull(observed, Instant::now())
                    {
                        Ok(outcome) => {
                            if let (Some(worker), Some(session)) = (&self.worker, &self.session) {
                                worker.update(&session.persisted);
                            }
                            self.note_outcome(&outcome);
                            Ok(json!({"outcome":outcome_name(&outcome)}))
                        }
                        Err(error) => {
                            self.sync_failure(error);
                            Err("sync_failed")
                        }
                    }
                }
            }
            "push" | "pull" => Err("not_configured"),
            "read_inbox" => match self
                .session
                .as_ref()
                .and_then(|s| s.persisted.inbox.as_ref())
            {
                Some(item) => {
                    match (
                        request.params.get("id").and_then(Value::as_str),
                        request.params.get("offset").and_then(Value::as_u64),
                    ) {
                        (Some(id), Some(offset)) => inbox::read_chunk(item, id, offset),
                        _ => Err("invalid_request"),
                    }
                }
                None => Err("inbox_stale"),
            },
            "copy_inbox" => match request.params.get("id").and_then(Value::as_str) {
                Some(id) => match self.session.as_mut() {
                    Some(session) => match session.copy_inbox(id) {
                        Ok(true) => Ok(json!({})),
                        Ok(false) => Err("inbox_stale"),
                        Err(SyncError::Clipboard(_)) => Err("clipboard_unavailable"),
                        Err(error) => {
                            self.sync_failure(error);
                            Err("inbox_failed")
                        }
                    },
                    None => Err("inbox_stale"),
                },
                None => Err("invalid_request"),
            },
            "clear_inbox" => match request.params.get("id").and_then(Value::as_str) {
                Some(id) => match self.session.as_mut().map(|s| s.clear_inbox(id)) {
                    Some(Ok(true)) => Ok(json!({"cleared": true})),
                    Some(Ok(false)) | None => Err("inbox_stale"),
                    Some(Err(error)) => {
                        self.sync_failure(error);
                        Err("inbox_failed")
                    }
                },
                None => Err("invalid_request"),
            },
            "history" => match self.session.as_ref() {
                Some(s) => s
                    .history
                    .load()
                    .map(|h| {
                        history_page(
                            h,
                            request
                                .params
                                .get("offset")
                                .and_then(Value::as_u64)
                                .unwrap_or(0),
                        )
                    })
                    .map_err(|_| "history_failed"),
                None => Ok(json!({"entries":[]})),
            },
            "copy_history" => match self.session.as_ref() {
                Some(s) => {
                    let digest = request
                        .params
                        .get("hash")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    match s
                        .history
                        .load()
                        .ok()
                        .and_then(|entries| history_text(entries, digest))
                    {
                        Some(text) => wayland::write_text(&text)
                            .map(|_| json!({}))
                            .map_err(|_| "clipboard_unavailable"),
                        None => Err("history_failed"),
                    }
                }
                None => Err("not_configured"),
            },
            "set_favorite" => match self.session.as_ref() {
                Some(s) => {
                    let digest = request
                        .params
                        .get("hash")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let favorite = request.params.get("favorite").and_then(Value::as_bool);
                    match favorite {
                        Some(value) => s
                            .history
                            .set_favorite(digest, value)
                            .map(|_| json!({}))
                            .map_err(|e| match e {
                                HistoryError::FavoriteLimit => "favorite_limit",
                                HistoryError::NotFound => "history_not_found",
                                HistoryError::Disabled => "history_disabled",
                                _ => "history_failed",
                            }),
                        None => Err("invalid_request"),
                    }
                }
                None => Err("not_configured"),
            },
            "clear_history" => match self.session.as_ref() {
                Some(s) => s
                    .history
                    .clear()
                    .map(|_| json!({}))
                    .map_err(|_| "history_failed"),
                None => Err("not_configured"),
            },
            "shutdown" => return (id, Ok(json!({})), true),
            _ => Err("unknown_method"),
        };
        (id, result, false)
    }
}
fn history_page(mut entries: Vec<Entry>, requested_offset: u64) -> Value {
    // Stable sorting keeps chronological order inside each group before reversing.
    entries.sort_by_key(|entry| entry.favorite);
    let total = entries.len();
    let offset = requested_offset.min(total as u64) as usize;
    let rows: Vec<_> = entries.into_iter().rev().skip(offset).take(20).map(|entry| {
        let preview: String = entry.text.chars().take(160).collect();
        json!({"text": preview, "hash": entry.hash, "time": entry.time, "direction": entry.direction, "favorite": entry.favorite})
    }).collect();
    json!({"entries": rows, "offset": offset, "total": total})
}

fn history_text(entries: Vec<Entry>, digest: &str) -> Option<String> {
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    entries
        .into_iter()
        .rev()
        .find(|entry| entry.hash.eq_ignore_ascii_case(digest))
        .map(|entry| entry.text)
}
fn outcome_name(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Stored => "stored",
        Outcome::Uploaded => "uploaded",
        Outcome::Current => "current",
        Outcome::Downloaded => "downloaded",
        Outcome::Skipped => "skipped",
        Outcome::Filtered => "filtered",
        Outcome::Paused => "paused",
        Outcome::Unsupported => "unsupported",
        Outcome::Invalid => "invalid",
        Outcome::Capacity => "capacity",
    }
}

fn emit_sync_success(out: &mut impl Write, outcome: &Outcome, enabled: bool) {
    if enabled
        && matches!(
            outcome,
            Outcome::Uploaded | Outcome::Downloaded | Outcome::Stored
        )
    {
        let _ = protocol::event(
            out,
            "sync_success",
            &json!({"outcome": outcome_name(outcome)}),
        );
    }
}
fn main() {
    let Some(paths) = Paths::from_env() else {
        eprintln!("missing home directory");
        return;
    };
    let (tx, rx) = mpsc::sync_channel(16);
    let stdin_tx = tx.clone();
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        loop {
            match protocol::read_line(&mut reader) {
                Ok(Some(line)) => {
                    let msg = match Request::parse(&line) {
                        Ok(req) => Message::Request(req),
                        Err(_) => Message::InvalidRequest,
                    };
                    if stdin_tx.send(msg).is_err() {
                        return;
                    }
                }
                Ok(None) | Err(_) => {
                    let _ = stdin_tx.send(Message::End);
                    return;
                }
            }
        }
    });
    let mut app = App::new(paths, tx, env::args().any(|arg| arg == "--verbose"));
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut published = Value::Null;
    loop {
        app.tick(&mut out);
        let status = app.status();
        if status != published {
            if protocol::event(&mut out, "status", &status).is_err() {
                break;
            }
            published = status;
        }
        let msg = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(msg) => msg,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        let mut shutdown = false;
        match msg {
            Message::Request(req) => {
                let manual_sync = matches!(req.method.as_str(), "push" | "pull");
                let (id, result, stop) = app.handle(req);
                if manual_sync && app.config.sync.notify_on_success {
                    if let Ok(value) = &result {
                        let outcome = match value.get("outcome").and_then(Value::as_str) {
                            Some("uploaded") => Some(Outcome::Uploaded),
                            Some("downloaded") => Some(Outcome::Downloaded),
                            Some("stored") => Some(Outcome::Stored),
                            _ => None,
                        };
                        if let Some(outcome) = outcome {
                            emit_sync_success(&mut out, &outcome, true);
                        }
                    }
                }
                let result = result.as_ref().map_err(|e| *e);
                if protocol::reply(&mut out, id, result).is_err() {
                    break;
                }
                shutdown = stop;
            }
            Message::InvalidRequest => {
                let _ = protocol::event(
                    &mut out,
                    "protocol_error",
                    &json!({"code":"invalid_request"}),
                );
            }
            Message::Clipboard => {}
            Message::ClipboardError(generation) => {
                if generation == app.watcher_generation {
                    app.clipboard_error = true;
                    app.log(Event::ClipboardError);
                }
            }
            Message::WatcherReady(generation, cancel) => {
                if generation == app.watcher_generation && app.wants_watcher() {
                    app.watcher_cancel = Some(cancel);
                    app.watcher_ready(Instant::now());
                } else {
                    cancel.cancel();
                }
            }
            Message::WatcherEnded(generation) => {
                if generation == app.watcher_generation {
                    app.watcher_active = false;
                    app.watcher_cancel = None;
                    app.watcher_retry = Instant::now() + Duration::from_secs(3);
                }
            }
            Message::End => shutdown = true,
        }
        if shutdown {
            break;
        }
    }
    app.stop_watcher();
    app.stop_notifications();
    app.log(Event::Shutdown);
}

#[cfg(test)]
mod tests {
    use super::*;
    use omaclipsync_core::{Filter, FilterMode};
    fn fixture_session(
        path: &std::path::Path,
        history: History,
    ) -> session::Session<session::WaylandClipboard> {
        use rustclipsync_core::{
            client::Client,
            session::{Session as CoreSession, SessionConfig},
        };
        session::Session::from_core(
            CoreSession::open(
                SessionConfig {
                    server: "https://example.invalid".into(),
                    encryption_key: [7; 32],
                    filter: Filter::new(FilterMode::Off, &[], false).unwrap(),
                    allow_sensitive: false,
                },
                Arc::new(
                    Client::new(
                        "https://example.invalid",
                        "fixture".into(),
                        String::new(),
                        false,
                    )
                    .unwrap(),
                ),
                omaclipsync_core::state::FileStateStore(path.join("state")),
                session::WaylandClipboard,
                history,
                Arc::new(AtomicU64::new(0)),
            )
            .unwrap(),
        )
    }

    #[test]
    fn changing_sensitive_policy_discards_pending_events_and_invalidates_old_watcher() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config: dir.path().join("config.json"),
            history: dir.path().join("history.json"),
            logs: dir.path().join("logs"),
        };
        let (tx, _rx) = mpsc::sync_channel(4);
        let mut app = App::new(paths, tx, false);
        app.config.sync.allow_sensitive = true;
        *app.clipboard_latest.lock().unwrap() =
            Some((app.watcher_generation, 1, wayland::ClipboardChange::Cleared));
        let old_generation = app.watcher_generation;
        let mut config = app.config.clone();
        config.sync.allow_sensitive = false;
        app.apply(config);
        assert!(app.clipboard_latest.lock().unwrap().is_none());
        assert_ne!(app.watcher_generation, old_generation);
        assert!(!app.config.sync.allow_sensitive);
    }

    #[test]
    fn direction_command_validates_and_persists_without_changing_pause() {
        use omaclipsync_core::config::SyncDirection;
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config: dir.path().join("config.json"),
            history: dir.path().join("history.json"),
            logs: dir.path().join("logs"),
        };
        let (tx, _rx) = mpsc::sync_channel(4);
        let mut app = App::new(paths, tx, false);
        app.config.sync.enabled = false;
        let request = Request::parse(
            br#"{"id":1,"method":"set_direction","params":{"direction":"receive"}}"#,
        )
        .unwrap();
        assert!(app.handle(request).1.is_ok());
        let saved = Config::read(&app.paths.config).unwrap();
        assert_eq!(saved.sync.direction, SyncDirection::Receive);
        assert!(!saved.sync.enabled);
        let bad = Request::parse(
            br#"{"id":2,"method":"set_direction","params":{"direction":"invalid"}}"#,
        )
        .unwrap();
        assert!(app.handle(bad).1.is_err());
        assert_eq!(Config::read(&app.paths.config).unwrap(), saved);
        for expected in ["both", "push", "receive"] {
            let request = Request::parse(br#"{"id":3,"method":"cycle_direction"}"#).unwrap();
            let result = app.handle(request).1.unwrap();
            assert_eq!(result["direction"], expected);
            assert_eq!(
                Config::read(&app.paths.config).unwrap().sync.direction,
                serde_json::from_value::<SyncDirection>(json!(expected)).unwrap()
            );
            assert!(!result["enabled"].as_bool().unwrap());
        }
    }

    #[test]
    fn success_events_report_transfers_and_durable_receipts_without_bodies() {
        let mut output = Vec::new();
        emit_sync_success(&mut output, &Outcome::Uploaded, false);
        emit_sync_success(&mut output, &Outcome::Current, true);
        emit_sync_success(&mut output, &Outcome::Skipped, true);
        assert!(output.is_empty());
        emit_sync_success(&mut output, &Outcome::Uploaded, true);
        emit_sync_success(&mut output, &Outcome::Downloaded, true);
        emit_sync_success(&mut output, &Outcome::Stored, true);
        let events: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            events,
            vec![
                json!({"event":"sync_success", "data":{"outcome":"uploaded"}}),
                json!({"event":"sync_success", "data":{"outcome":"downloaded"}}),
                json!({"event":"sync_success", "data":{"outcome":"stored"}})
            ]
        );
    }
    #[test]
    fn reconnecting_local_watcher_preserves_network_and_auth_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config: dir.path().join("config.json"),
            history: dir.path().join("history.json"),
            logs: dir.path().join("logs"),
        };
        let (tx, _rx) = mpsc::sync_channel(4);
        let mut app = App::new(paths, tx, false);
        let now = Instant::now();
        let retry_at = now + Duration::from_secs(60);
        for state in ["offline", "credentials"] {
            app.state = state;
            app.next_poll = retry_at;
            app.pull_pending = false;
            app.clipboard_error = true;
            app.watcher_ready(now);
            assert_eq!(app.next_poll, retry_at);
            assert!(app.pull_pending);
            assert!(!app.clipboard_error);
        }
        app.state = "connected";
        app.next_poll = retry_at;
        app.watcher_ready(now);
        assert_eq!(app.next_poll, now);
    }
    #[test]
    fn notification_transitions_schedule_catchup_and_bounded_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config: dir.path().join("config.json"),
            history: dir.path().join("history.json"),
            logs: dir.path().join("logs"),
        };
        let history = History::new(&paths.history, false, 10).unwrap();
        let (tx, _rx) = mpsc::sync_channel(4);
        let mut app = App::new(paths, tx, false);
        app.session = Some(fixture_session(dir.path(), history));
        app.config.sync.interval_sec = 3; // Legacy value, clamped to 30 seconds.
        app.notification_generation = 7;
        app.state = "connected";
        let now = Instant::now();
        app.notification_tx
            .send((7, NotificationEvent::Connected))
            .unwrap();
        app.notification_events(now);
        assert!(app.notification_connected && app.pull_pending);
        assert_eq!(app.next_poll, now);
        assert_eq!(app.status()["transport"], "realtime");

        app.pull_pending = false;
        app.notification_tx
            .send((7, NotificationEvent::Disconnected))
            .unwrap();
        app.notification_events(now);
        assert!(!app.notification_connected);
        assert_eq!(app.next_fallback, now + Duration::from_secs(30));
        assert_eq!(app.status()["transport"], "polling");

        // A late notification from a canceled connection cannot revive it.
        app.notification_tx
            .send((6, NotificationEvent::Connected))
            .unwrap();
        app.notification_events(now);
        assert!(!app.notification_connected && !app.pull_pending);

        // Real-time events must not defeat network-error backoff.
        app.state = "offline";
        app.next_poll = now + Duration::from_secs(8);
        app.notification_tx
            .send((7, NotificationEvent::Changed))
            .unwrap();
        app.notification_events(now);
        assert!(app.pull_pending);
        assert_eq!(app.next_poll, now + Duration::from_secs(8));

        app.notification_tx
            .send((7, NotificationEvent::Connected))
            .unwrap();
        app.notification_events(now);
        assert_eq!(app.next_poll, now); // Reconnect triggers immediate catch-up.
        app.config.sync.enabled = false;
        assert_eq!(app.status()["transport"], "off");
    }
    #[test]
    fn history_pagination_limits_previews_and_copy_uses_full_validated_entry() {
        let entries: Vec<_> = (0..23)
            .map(|n| {
                let text = format!("{n}-{}", "文".repeat(200));
                Entry {
                    hash: omaclipsync_core::hash(&text),
                    text,
                    time: n,
                    direction: omaclipsync_core::history::Direction::Upload,
                    favorite: n == 1,
                }
            })
            .collect();
        let page = history_page(entries.clone(), 0);
        assert_eq!(page["total"], 23);
        assert_eq!(page["entries"][0]["hash"], entries[1].hash);
        assert_eq!(page["entries"][0]["favorite"], true);
        assert_eq!(page["entries"].as_array().unwrap().len(), 20);
        assert_eq!(
            page["entries"][0]["text"].as_str().unwrap().chars().count(),
            160
        );
        assert_eq!(
            history_page(entries.clone(), 20)["entries"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(history_page(entries.clone(), u64::MAX)["offset"], 23);
        assert_eq!(
            history_text(entries.clone(), &entries[22].hash.to_lowercase()).unwrap(),
            entries[22].text
        );
        assert!(history_text(entries, "../../invalid").is_none());
    }
    #[test]
    fn active_manual_pull_waits_for_the_clipboard_listener() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config: dir.path().join("config.json"),
            history: dir.path().join("history.json"),
            logs: dir.path().join("logs"),
        };
        let (tx, _rx) = mpsc::sync_channel(4);
        let mut app = App::new(paths, tx, false);
        app.config.sync.enabled = true;
        app.session = Some(fixture_session(
            dir.path(),
            History::new(dir.path().join("history.json"), false, 50).unwrap(),
        ));
        let req = Request::parse(br#"{"id":1,"method":"pull"}"#).unwrap();
        let (_, result, _) = app.handle(req);
        assert_eq!(result.unwrap_err(), "clipboard_unavailable");
    }

    #[test]
    fn favorite_requests_expose_errors_and_preserve_favorites_on_clear() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config: dir.path().join("config.json"),
            history: dir.path().join("history.json"),
            logs: dir.path().join("logs"),
        };
        let history = History::new(&paths.history, true, 10).unwrap();
        for i in 0..6 {
            history
                .record(
                    &format!("item-{i}"),
                    omaclipsync_core::history::Direction::Upload,
                    i,
                )
                .unwrap();
        }
        let (tx, _rx) = mpsc::sync_channel(4);
        let mut app = App::new(paths, tx, false);
        app.session = Some(fixture_session(dir.path(), history));
        for i in 0..6 {
            let req = Request::parse(
                serde_json::to_vec(&json!({"id":1,"method":"set_favorite",
                "params":{"hash":omaclipsync_core::hash(&format!("item-{i}")),"favorite":true}}))
                .unwrap()
                .as_slice(),
            )
            .unwrap();
            let (_, result, _) = app.handle(req);
            if i < 5 {
                assert!(result.is_ok());
            } else {
                assert_eq!(result.unwrap_err(), "favorite_limit");
            }
        }
        let (_, result, _) =
            app.handle(Request::parse(br#"{"id":2,"method":"clear_history"}"#).unwrap());
        assert!(result.is_ok());
        let (_, result, _) = app.handle(Request::parse(br#"{"id":3,"method":"history"}"#).unwrap());
        let page = result.unwrap();
        assert_eq!(page["total"], 5);
        assert!(page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["favorite"] == true));
    }
}
