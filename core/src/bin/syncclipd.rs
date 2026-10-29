//! Shell-owned daemon. Stdout is exclusively the versioned JSON-lines protocol.
use chrono::Utc;
use omaclipsync_core::{
    config::{Config, ConfigError, SyncDirection},
    history::{Entry, History, HistoryError},
    keyring,
    logging::{Event, Level, Logger},
    protocol::{self, Request},
    session::{Outcome, Session, SyncError, WaylandClipboard},
    signalr::{self, Event as SignalEvent},
    wayland, Error, Server,
};
use serde_json::{json, Value};
use std::{
    env, fs,
    io::{self, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use wl_clipboard_rs::watch::CancelHandle;

const ID: &str = "omaclipsync";
type LiveSession = Session<Server, WaylandClipboard>;
struct SignalOptions {
    url: String,
    user: String,
    password: String,
    insecure: bool,
}
enum WatchCancel {
    Native(CancelHandle),
    Fallback(Arc<Mutex<Child>>),
}
impl WatchCancel {
    fn cancel(self) {
        match self {
            Self::Native(handle) => handle.cancel(),
            Self::Fallback(child) => {
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                }
            }
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
    session: Option<LiveSession>,
    state: &'static str,
    last_sync: Option<u64>,
    log_error: bool,
    logger: Logger,
    next_poll: Instant,
    next_fallback: Instant,
    next_signal_pull: Instant,
    pull_pending: bool,
    signal_tx: SyncSender<(u64, SignalEvent)>,
    signal_rx: Receiver<(u64, SignalEvent)>,
    signal_generation: u64,
    signal_connected: bool,
    signal_options: Option<SignalOptions>,
    signal_worker: Option<signalr::Worker>,
    next_config_check: Instant,
    backoff: u64,
    config_stamp: Option<(SystemTime, u64)>,
    pending: Option<String>,
    latest: Arc<Mutex<Option<(u64, String)>>>,
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
    fn new(
        paths: Paths,
        sender: SyncSender<Message>,
        latest: Arc<Mutex<Option<(u64, String)>>>,
        verbose: bool,
    ) -> Self {
        let logger = Logger::new(&paths.logs);
        let now = Instant::now();
        let (signal_tx, signal_rx) = mpsc::sync_channel(32);
        let mut app = Self {
            paths,
            config: Config::default(),
            session: None,
            state: "needs_config",
            last_sync: None,
            log_error: false,
            logger,
            next_poll: now,
            next_fallback: now,
            next_signal_pull: now,
            pull_pending: false,
            signal_tx,
            signal_rx,
            signal_generation: 0,
            signal_connected: false,
            signal_options: None,
            signal_worker: None,
            next_config_check: now,
            backoff: 1,
            config_stamp: None,
            pending: None,
            latest,
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
                self.stop_signalr();
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
        self.stop_signalr();
        if self.config.sync.direction != config.sync.direction {
            self.pending = None;
            if let Ok(mut latest) = self.latest.lock() {
                *latest = None;
            }
        }
        let filter = match config.validate() {
            Ok(f) => f,
            Err(_) => {
                self.session = None;
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
                self.session = None;
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
            self.session = None;
            self.state = "needs_config";
            self.stop_watcher();
            return;
        }
        let password = match keyring::lookup(&self.config.server.user) {
            Ok(v) => v,
            Err(_) => {
                self.session = None;
                self.state = "credentials";
                self.stop_watcher();
                return;
            }
        };
        if self.config.sync.enabled && self.config.sync.direction.can_receive() {
            self.signal_options = Some(SignalOptions {
                url: self.config.server.url.clone(),
                user: self.config.server.user.clone(),
                password: password.clone(),
                insecure: self.config.server.insecure,
            });
        }
        let server = match Server::with_tls(
            &self.config.server.url,
            self.config.server.user.clone(),
            password,
            self.config.server.insecure,
        ) {
            Ok(s) => s,
            Err(_) => {
                self.stop_signalr();
                self.session = None;
                self.state = "config_error";
                self.stop_watcher();
                return;
            }
        };
        let window = Duration::from_secs(self.config.dedupe.window_sec);
        if let Some(session) = self.session.as_mut() {
            session.remote = server;
            session.history = history;
            session.max_inline = self.config.sync.max_inline_bytes;
            session
                .engine
                .reconfigure(filter, window, self.config.sync.enabled);
        } else {
            self.session = Some(Session::new(
                server,
                WaylandClipboard,
                filter,
                window,
                self.config.sync.enabled,
                self.config.sync.max_inline_bytes,
                history,
            ));
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
        self.next_signal_pull = Instant::now();
        // Let the host complete the protocol handshake before any network request.
        self.next_poll = Instant::now() + Duration::from_secs(1);
        if !self.config.sync.enabled {
            self.stop_watcher();
        }
    }
    fn stop_signalr(&mut self) {
        self.signal_generation = self.signal_generation.wrapping_add(1);
        self.signal_connected = false;
        self.signal_options = None;
        self.signal_worker = None;
        while self.signal_rx.try_recv().is_ok() {}
    }
    fn fallback_interval(&self) -> Duration {
        Duration::from_secs(self.config.sync.interval_sec.clamp(30, 300))
    }
    fn signal_events(&mut self, now: Instant) {
        // Bound work per tick even if an untrusted server floods notifications.
        for _ in 0..64 {
            let Ok((generation, event)) = self.signal_rx.try_recv() else {
                break;
            };
            if generation != self.signal_generation || !self.wants_watcher() {
                continue;
            }
            match event {
                SignalEvent::Connected => {
                    self.signal_connected = true;
                    self.pull_pending = true;
                    self.next_signal_pull = now;
                    self.next_poll = now;
                }
                SignalEvent::Changed => {
                    self.pull_pending = true;
                    if self.state != "offline" && self.state != "credentials" {
                        self.next_poll = now;
                    }
                }
                SignalEvent::Disconnected => {
                    if self.signal_connected {
                        self.next_fallback = now + self.fallback_interval();
                    }
                    self.signal_connected = false;
                }
            }
        }
    }
    fn stop_watcher(&mut self) {
        self.watcher_generation += 1;
        self.watcher_active = false;
        if let Some(cancel) = self.watcher_cancel.take() {
            cancel.cancel();
        }
        self.pending = None;
        if let Ok(mut latest) = self.latest.lock() {
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
        let tx = self.sender.clone();
        let latest = self.latest.clone();
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
                let result = wayland::watch_loop(watcher, |item| match item {
                    Ok(text) => {
                        if let Ok(mut value) = latest.lock() {
                            *value = Some((generation, text));
                        }
                        let _ = tx.try_send(Message::Clipboard);
                    }
                    Err(_) => {
                        let _ = tx.try_send(Message::ClipboardError(generation));
                    }
                });
                if result.is_err() {
                    let _ = tx.try_send(Message::ClipboardError(generation));
                }
            } else if fallback_watch(generation, &tx, &latest).is_err() {
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
        } else if self.signal_connected {
            "signalr"
        } else {
            "polling"
        };
        json!({"state":self.state,"transport":transport,"enabled":self.config.sync.enabled,"direction":self.config.sync.direction,"url":self.config.server.url,"lastSync":self.last_sync,"historyError":self.session.as_ref().is_some_and(|s|s.history_error),"logError":self.log_error,"watching":self.watcher_cancel.is_some(),"clipboardError":self.clipboard_error,"dedupeFull":self.dedupe_full,"configRevision":self.config_revision})
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
            Outcome::Downloaded => {
                self.stamp_sync();
                self.log(Event::Downloaded)
            }
            Outcome::Filtered => self.log(Event::Filtered),
            Outcome::Unsupported => self.log(Event::Unsupported),
            _ => {}
        }
    }
    fn failure(&mut self, err: SyncError) {
        let remote_error = matches!(&err, SyncError::Remote(_));
        self.state = match err {
            SyncError::Remote(Error::Status(401 | 403)) => {
                self.log(Event::AuthFailed);
                "credentials"
            }
            SyncError::Clipboard(_) => {
                self.log(Event::ClipboardError);
                "clipboard_error"
            }
            _ => {
                self.log(Event::Offline);
                "offline"
            }
        };
        self.needs_check = true;
        self.next_poll = Instant::now() + Duration::from_secs(self.backoff);
        // In fallback mode, repeated HTTP failures must not turn the slow
        // polling cadence back into a fast retry loop. A pending local upload
        // still uses backoff and a restored SignalR connection wakes us early.
        if remote_error && !self.signal_connected && self.pending.is_none() {
            self.next_poll = self
                .next_poll
                .max(Instant::now() + self.fallback_interval());
        }
        self.backoff = (self.backoff * 2).min(30);
    }
    fn tick(&mut self, out: &mut impl Write) {
        let now = Instant::now();
        if now >= self.next_config_check {
            self.next_config_check = now + Duration::from_secs(1);
            if self.stamp() != self.config_stamp {
                self.reload();
            }
        }
        self.signal_events(now);
        self.ensure_watcher(now);
        if let Ok(mut latest) = self.latest.lock() {
            if let Some((generation, text)) = latest.take() {
                if generation == self.watcher_generation && self.wants_watcher() {
                    if self.config.sync.direction.can_push() {
                        self.pending = Some(text);
                        if self.state != "offline" && self.state != "credentials" {
                            self.next_poll = now;
                        }
                    } else if let Some(session) = self.session.as_mut() {
                        session.engine.clear_clipboard_suppression();
                    }
                }
            }
        }
        if !self.config.sync.enabled || self.session.is_none() || now < self.next_poll {
            return;
        }
        // Start the long connection only after the host's initial handshake delay.
        if let Some(options) = self.signal_options.take() {
            self.signal_worker = Some(signalr::Worker::start(
                options.url,
                options.user,
                options.password,
                options.insecure,
                self.signal_generation,
                self.signal_tx.clone(),
            ));
        }
        let fallback_due = self.config.sync.direction.can_receive()
            && !self.signal_connected
            && now >= self.next_fallback;
        let signal_pull_due = self.config.sync.direction.can_receive()
            && self.pull_pending
            && now >= self.next_signal_pull;
        if !self.needs_check && self.pending.is_none() && !signal_pull_due && !fallback_due {
            return;
        }
        if self.watcher_cancel.is_none() && !self.needs_check {
            self.state = "clipboard_error";
            self.next_poll = Instant::now() + Duration::from_secs(1);
            return;
        }
        let _ = protocol::event(out, "activity", &json!({"active":true}));
        if self.needs_check {
            let check = self.session.as_ref().unwrap().remote.check();
            match check {
                Ok(()) => self.needs_check = false,
                Err(err) => {
                    self.failure(SyncError::Remote(err));
                    let _ = protocol::event(out, "activity", &json!({"active":false}));
                    return;
                }
            }
        }
        // Never write the remote selection without an active listener: its
        // corresponding clipboard event could otherwise arrive only after a
        // later user copy, leaving a stale loop-suppression marker behind.
        if self.watcher_cancel.is_none() {
            self.state = "clipboard_error";
            self.next_poll = Instant::now() + Duration::from_secs(1);
            let _ = protocol::event(out, "activity", &json!({"active":false}));
            return;
        }
        if let Some(text) = self.pending.take() {
            let result = self.session.as_mut().unwrap().local_changed(&text);
            match result {
                Ok(outcome) => {
                    self.note_outcome(&outcome);
                    emit_sync_success(out, &outcome, self.config.sync.notify_on_success);
                }
                Err(err) => {
                    self.pending = Some(text);
                    self.failure(err);
                    let _ = protocol::event(out, "activity", &json!({"active":false}));
                    return;
                }
            }
        }
        if signal_pull_due || fallback_due {
            let result = self.session.as_mut().unwrap().pull(false, now);
            match result {
                Ok(outcome) => {
                    self.note_outcome(&outcome);
                    emit_sync_success(out, &outcome, self.config.sync.notify_on_success);
                    self.log(Event::Polled);
                    self.pull_pending = false;
                    self.next_fallback = Instant::now() + self.fallback_interval();
                    // Coalesce bursts while preserving the last notification.
                    self.next_signal_pull = Instant::now() + Duration::from_millis(500);
                }
                Err(err) => {
                    self.failure(err);
                    let _ = protocol::event(out, "activity", &json!({"active":false}));
                    return;
                }
            }
        }
        if self.state != "connected" {
            self.log(Event::Connected);
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
            "push" => match self.session.as_mut().map(|s| s.push_now()) {
                Some(Ok(outcome)) => {
                    self.note_outcome(&outcome);
                    Ok(json!({"outcome":outcome_name(&outcome)}))
                }
                Some(Err(err)) => {
                    self.failure(err);
                    Err("sync_failed")
                }
                None => Err("not_configured"),
            },
            "pull" if self.config.sync.enabled && self.watcher_cancel.is_none() => {
                Err("clipboard_unavailable")
            }
            "pull" => {
                let force = request
                    .params
                    .get("force")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let enabled = self.config.sync.enabled;
                if let Some(session) = self.session.as_mut() {
                    session.engine.set_enabled(true);
                }
                let result = self.session.as_mut().map(|s| s.pull(force, Instant::now()));
                if let Some(session) = self.session.as_mut() {
                    session.engine.set_enabled(enabled);
                    if !enabled || self.watcher_cancel.is_none() {
                        session.engine.clear_clipboard_suppression();
                    }
                }
                match result {
                    Some(Ok(outcome)) => {
                        self.note_outcome(&outcome);
                        Ok(json!({"outcome":outcome_name(&outcome)}))
                    }
                    Some(Err(err)) => {
                        self.failure(err);
                        Err("sync_failed")
                    }
                    None => Err("not_configured"),
                }
            }
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
// wl-paste executes one helper per selection and waits for it to finish. Frames
// are therefore serialized; a u32::MAX frame denotes an empty/sensitive offer.
fn watch_event() {
    let state = env::var("CLIPBOARD_STATE").unwrap_or_default();
    let mut output = io::stdout().lock();
    if state != "data" {
        let _ = output.write_all(&u32::MAX.to_be_bytes());
        return;
    }
    let input = io::stdin();
    let text = wayland::read_bounded(
        input.lock(),
        omaclipsync_core::MAX_TEXT_BYTES,
        wayland::READ_TIMEOUT,
    );
    match text {
        Ok(text) => {
            let _ = output
                .write_all(&(text.len() as u32).to_be_bytes())
                .and_then(|_| output.write_all(text.as_bytes()));
        }
        Err(_) => {
            let _ = output.write_all(&u32::MAX.to_be_bytes());
        }
    }
}
fn fallback_snapshot() -> Option<String> {
    let mut child = Command::new("wl-paste")
        .args(["--no-newline", "--type", wayland::MIME])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let text = wayland::read_bounded(
        stdout,
        omaclipsync_core::MAX_TEXT_BYTES,
        wayland::READ_TIMEOUT,
    )
    .ok();
    let deadline = Instant::now() + Duration::from_secs(1);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    if success {
        text.map(|value| omaclipsync_core::hash(&value))
    } else {
        None
    }
}
fn fallback_watch(
    generation: u64,
    tx: &SyncSender<Message>,
    latest: &Arc<Mutex<Option<(u64, String)>>>,
) -> io::Result<()> {
    let initial_hash = fallback_snapshot();
    let exe = env::current_exe()?;
    let mut child = Command::new("wl-paste")
        .args(["--type", wayland::MIME, "--watch"])
        .arg(exe)
        .arg("--watch-event")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing watcher output"))?;
    let child = Arc::new(Mutex::new(child));
    if tx
        .send(Message::WatcherReady(
            generation,
            WatchCancel::Fallback(child.clone()),
        ))
        .is_err()
    {
        if let Ok(mut child) = child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        return Ok(());
    }
    let mut first = true;
    let result = loop {
        let mut header = [0u8; 4];
        if let Err(error) = stdout.read_exact(&mut header) {
            break Err(error);
        }
        let length = u32::from_be_bytes(header);
        if length == u32::MAX {
            first = false;
            continue;
        }
        if length as usize > omaclipsync_core::MAX_TEXT_BYTES {
            break Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "oversized offer",
            ));
        }
        let mut bytes = vec![0u8; length as usize];
        if let Err(error) = stdout.read_exact(&mut bytes) {
            break Err(error);
        }
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => continue,
        };
        // If the initial snapshot is unavailable, discard the first data frame
        // rather than risk overwriting the server with a stale selection.
        let is_initial = first
            && initial_hash
                .as_ref()
                .is_none_or(|digest| *digest == omaclipsync_core::hash(&text));
        first = false;
        if is_initial {
            continue;
        }
        if let Ok(mut value) = latest.lock() {
            *value = Some((generation, text));
        }
        let _ = tx.try_send(Message::Clipboard);
    };
    if let Ok(mut child) = child.lock() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}
fn outcome_name(outcome: &Outcome) -> &'static str {
    match outcome {
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
    if enabled && matches!(outcome, Outcome::Uploaded | Outcome::Downloaded) {
        let _ = protocol::event(
            out,
            "sync_success",
            &json!({"outcome": outcome_name(outcome)}),
        );
    }
}
fn main() {
    if env::args().nth(1).as_deref() == Some("--watch-event") {
        watch_event();
        return;
    }
    let Some(paths) = Paths::from_env() else {
        eprintln!("missing home directory");
        return;
    };
    let (tx, rx) = mpsc::sync_channel(16);
    let latest = Arc::new(Mutex::new(None));
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
    let mut app = App::new(paths, tx, latest, env::args().any(|arg| arg == "--verbose"));
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
    app.stop_signalr();
    app.log(Event::Shutdown);
}

#[cfg(test)]
mod tests {
    use super::*;
    use omaclipsync_core::{Filter, FilterMode};

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
        let mut app = App::new(paths, tx, Arc::new(Mutex::new(None)), false);
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
    fn success_events_only_report_actual_transfers_when_enabled() {
        let mut output = Vec::new();
        emit_sync_success(&mut output, &Outcome::Uploaded, false);
        emit_sync_success(&mut output, &Outcome::Current, true);
        emit_sync_success(&mut output, &Outcome::Skipped, true);
        assert!(output.is_empty());
        emit_sync_success(&mut output, &Outcome::Uploaded, true);
        emit_sync_success(&mut output, &Outcome::Downloaded, true);
        let events: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            events,
            vec![
                json!({"event":"sync_success", "data":{"outcome":"uploaded"}}),
                json!({"event":"sync_success", "data":{"outcome":"downloaded"}})
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
        let mut app = App::new(paths, tx, Arc::new(Mutex::new(None)), false);
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
    fn signal_transitions_schedule_catchup_and_bounded_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config: dir.path().join("config.json"),
            history: dir.path().join("history.json"),
            logs: dir.path().join("logs"),
        };
        let history = History::new(&paths.history, false, 10).unwrap();
        let (tx, _rx) = mpsc::sync_channel(4);
        let mut app = App::new(paths, tx, Arc::new(Mutex::new(None)), false);
        app.session = Some(Session::new(
            Server::new(
                "https://example.invalid",
                "fixture".into(),
                "fixture".into(),
            )
            .unwrap(),
            WaylandClipboard,
            Filter::new(FilterMode::Off, &[], false).unwrap(),
            Duration::from_secs(300),
            true,
            32768,
            history,
        ));
        app.config.sync.interval_sec = 3; // Legacy value, clamped to 30 seconds.
        app.signal_generation = 7;
        app.state = "connected";
        let now = Instant::now();
        app.signal_tx.send((7, SignalEvent::Connected)).unwrap();
        app.signal_events(now);
        assert!(app.signal_connected && app.pull_pending);
        assert_eq!(app.next_poll, now);
        assert_eq!(app.status()["transport"], "signalr");

        app.pull_pending = false;
        app.signal_tx.send((7, SignalEvent::Disconnected)).unwrap();
        app.signal_events(now);
        assert!(!app.signal_connected);
        assert_eq!(app.next_fallback, now + Duration::from_secs(30));
        assert_eq!(app.status()["transport"], "polling");

        // A late notification from a canceled connection cannot revive it.
        app.signal_tx.send((6, SignalEvent::Connected)).unwrap();
        app.signal_events(now);
        assert!(!app.signal_connected && !app.pull_pending);

        // Real-time events must not defeat network-error backoff.
        app.state = "offline";
        app.next_poll = now + Duration::from_secs(8);
        app.signal_tx.send((7, SignalEvent::Changed)).unwrap();
        app.signal_events(now);
        assert!(app.pull_pending);
        assert_eq!(app.next_poll, now + Duration::from_secs(8));

        app.signal_tx.send((7, SignalEvent::Connected)).unwrap();
        app.signal_events(now);
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
        let mut app = App::new(paths, tx, Arc::new(Mutex::new(None)), false);
        app.config.sync.enabled = true;
        app.session = Some(Session::new(
            Server::new(
                "https://example.invalid",
                "fixture".into(),
                "fixture".into(),
            )
            .unwrap(),
            WaylandClipboard,
            Filter::new(FilterMode::Off, &[], false).unwrap(),
            Duration::from_secs(300),
            true,
            32768,
            History::new(dir.path().join("history.json"), false, 50).unwrap(),
        ));
        let req = Request::parse(br#"{"id":1,"method":"pull","params":{"force":true}}"#).unwrap();
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
        let mut app = App::new(paths, tx, Arc::new(Mutex::new(None)), false);
        app.session = Some(Session::new(
            Server::new(
                "https://example.invalid",
                "fixture".into(),
                "fixture".into(),
            )
            .unwrap(),
            WaylandClipboard,
            Filter::new(FilterMode::Off, &[], false).unwrap(),
            Duration::from_secs(300),
            true,
            32768,
            history,
        ));
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
