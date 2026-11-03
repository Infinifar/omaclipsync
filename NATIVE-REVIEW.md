# Native review guide

Plugin: `io.github.infinifar.omaclipsync`. This candidate supports only the current RustClipSync protocol, using `/api/...`. The prior approved marketplace snapshot is `d59ea2974a414316efdaa5323475970bc051b643` (0.3.0); the pause/storage finding concerns the subsequent 0.4.0 candidate `9d6f3e36ad58e30095b6d87569171369460d488f`. This inventory supports maintainer review; it is not independent security approval.

## Removed capabilities and the retention finding

The old SyncClipboard transport is removed, including profile/file APIs, SignalR, the fallback `wl-paste` subprocess/helper, the old local upload/history session, its engine and obsolete tests/examples. The daemon cannot select that transport. This removes the sensitive plaintext history path identified by the reviewer instead of retaining another compatibility implementation.

The panel no longer saves a protocol selector. A previously saved `server.protocol` field is consumed and discarded; it never selects a transport and is absent from serialized settings. Other unknown server fields still fail validation. The current protocol's marked uploads/receives continue to use `HistorySink::record_marked`; marked text is excluded from ordinary plaintext history. Previously stored history is not automatically erased.

## Saved-policy / storage-failure correction (0.4.1)

`App::apply` fences watchers/notifications and removes the live session before fallible cleanup. It adopts the saved selection immediately. A failed cleanup is retained in a boxed, quarantined session; it can only be cleared/retried, never used by tick, watcher restart or manual sync. Storage failure during session open/direction activation enters the same blocked state. `configApplyPending` distinguishes durable configuration from unfinished application. Requests report `config_apply_pending` and the shell refreshes saved settings/status rather than claiming success.

Blocked application retries re-read the configuration at most every 30 seconds, independently of its observed file stamp. A resume/settings change must first finish the pending cleanup; it cannot overwrite the saved restriction while storage is unavailable. Successful cleanup durably preserves cancellation of an attempted outbox before any new live session. Restart uses the saved pause/direction and the shared core applies its local guard before saving/reporting. No server/protocol/shared-core source changes are needed.

Regression fixtures use a paired encrypted local session and a directory at the atomic replacement target to fail state writes while configuration writes succeed. They cover pause, receive-only, cycling, settings, file reload, delayed callbacks, manual actions, recovery without a file change and restart-time activation failure. Fixtures make no network/Wayland calls. Shell reply tests execute the actual `Service.qml` receive handler under Node/GJS.

## Current source boundaries

| Files | Review boundary |
| --- | --- |
| `core/src/bin/syncclipd.rs` | One shared-core session; pairing, configuration, bounded event queues, notification generation checks, recovery, direction controls and ID-checked inbox requests. Enabling the plugin immediately starts this bundled native daemon through `Service.qml`. |
| `core/src/session.rs`, `state.rs`, `client.rs`, `worker.rs` | Linux adapters/re-exports for the shared engine: native Wayland, per-account Secret Service key, bounded encrypted state and durable directory sync. Explicit inbox copy carries an origin marker and cancels pending upload. |
| `core/vendor/rustclipsync-core/` | Automatically exported core 0.4.0 source snapshot, verified by `UPSTREAM.json` file hashes. The authoritative transport/state implementation is maintained in the separate shared-core repository; consumer copies are not edited by hand. |
| `core/src/config.rs`, `history.rs`, `inbox.rs` | Strict config validation, bounded optional history and five favorites, metadata-only inbox status, bounded explicit preview chunks and stale-ID rejection. Ordinary history remains plaintext when enabled. |
| `core/src/wayland.rs` | Regular clipboard only, native data-control events and same-offer sensitivity/origin hints, bounded reads and timeout. Failure to establish the native watcher pauses synchronization. A sensitivity hint cannot identify every secret. |
| `Panel.qml`, `Service.qml` | Real-time status is named `realtime`; no protocol/version selector. Explicit reveal uses plain text, 64 KiB maximum preview, full-text copy stays native, and reveal is cleared on close/replacement/disconnection. |

The shared transport performs HTTPS Basic registration at `/api/devices`, then device bearer authentication for state, changes, message and ack routes. Redirects are disabled; ordinary requests time out after 10 s, long polls after 30 s; metadata is limited to 16 KiB and text to 10 MiB. TLS verification is enabled unless the user explicitly chooses the insecure setting. There is no telemetry destination or downloaded executable.

`/api/...` replaces the former endpoint prefix across the service and clients. Device identities, cursor format, AES-256-GCM encrypted state, keyring accounts, state filenames and clipboard-origin marker strings are retained. Those opaque local identifiers are not transport selectors and changing them would unnecessarily lose pairing/echo state. The shared core's immutable source snapshot contains the complete network logic.

Secret Service still uses a bounded `secret-tool` subprocess with password on stdin. The thin `syncclipctl` delegates fixed commands to the existing shell and does not start another daemon. History paste delegates explicit user actions to existing desktop tools. Plugin code runs with the user's unsandboxed privileges; plaintext may exist in process memory or the system clipboard. Local encryption is not server end-to-end encryption. Inbox deletion does not erase backups or server data.

## Verification and approval boundary

The fixed-image release build must compare both committed executables byte for byte before verified packaging. See [REPRODUCIBLE-BUILD.md](REPRODUCIBLE-BUILD.md). Current tests cover the removed selector, current HTTPS transport, durable recovery and sensitive-history/inbox behavior; see [ACCEPTANCE.md](ACCEPTANCE.md) for actual results and desktop limits. Approval remains with the marketplace maintainer for the exact candidate commit. The author does not apply an approval label.
