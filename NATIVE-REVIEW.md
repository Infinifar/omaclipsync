# Native review guide

Plugin: `io.github.infinifar.omaclipsync`. This candidate supports only the current RustClipSync protocol, using `/api/...`. The prior marketplace target was `0015474841a5819afd6ce897312fa193cd471b80` (0.2.5). This inventory supports maintainer review; it is not independent security approval.

## Removed capabilities and the retention finding

The old SyncClipboard transport is removed, including profile/file APIs, SignalR, the fallback `wl-paste` subprocess/helper, the old local upload/history session, its engine and obsolete tests/examples. The daemon cannot select that transport. This removes the sensitive plaintext history path identified by the reviewer instead of retaining another compatibility implementation.

The panel no longer saves a protocol selector. A previously saved `server.protocol` field is consumed and discarded; it never selects a transport and is absent from serialized settings. Other unknown server fields still fail validation. The current protocol's marked uploads/receives continue to use `HistorySink::record_marked`; marked text is excluded from ordinary plaintext history. Previously stored history is not automatically erased.

## Current source boundaries

| Files | Review boundary |
| --- | --- |
| `core/src/bin/syncclipd.rs` | One shared-core session; pairing, configuration, bounded event queues, notification generation checks, recovery, direction controls and ID-checked inbox requests. Enabling the plugin immediately starts this bundled native daemon through `Service.qml`. |
| `core/src/session.rs`, `state.rs`, `client.rs`, `worker.rs` | Linux adapters/re-exports for the shared engine: native Wayland, per-account Secret Service key, bounded encrypted state and durable directory sync. Explicit inbox copy carries an origin marker and cancels pending upload. |
| `core/vendor/rustclipsync-core/` | Automatically exported core 0.3.0 source snapshot, verified by `UPSTREAM.json` file hashes. The authoritative transport/state implementation is maintained in the separate shared-core repository; consumer copies are not edited by hand. |
| `core/src/config.rs`, `history.rs`, `inbox.rs` | Strict config validation, bounded optional history and five favorites, metadata-only inbox status, bounded explicit preview chunks and stale-ID rejection. Ordinary history remains plaintext when enabled. |
| `core/src/wayland.rs` | Regular clipboard only, native data-control events and same-offer sensitivity/origin hints, bounded reads and timeout. Failure to establish the native watcher pauses synchronization. A sensitivity hint cannot identify every secret. |
| `Panel.qml`, `Service.qml` | Real-time status is named `realtime`; no protocol/version selector. Explicit reveal uses plain text, 64 KiB maximum preview, full-text copy stays native, and reveal is cleared on close/replacement/disconnection. |

The shared transport performs HTTPS Basic registration at `/api/devices`, then device bearer authentication for state, changes, message and ack routes. Redirects are disabled; ordinary requests time out after 10 s, long polls after 30 s; metadata is limited to 16 KiB and text to 10 MiB. TLS verification is enabled unless the user explicitly chooses the insecure setting. There is no telemetry destination or downloaded executable.

`/api/...` replaces the former endpoint prefix across the service and clients. Device identities, cursor format, AES-256-GCM encrypted state, keyring accounts, state filenames and clipboard-origin marker strings are retained. Those opaque local identifiers are not transport selectors and changing them would unnecessarily lose pairing/echo state. The shared core's immutable source snapshot contains the complete network logic.

Secret Service still uses a bounded `secret-tool` subprocess with password on stdin. The thin `syncclipctl` delegates fixed commands to the existing shell and does not start another daemon. History paste delegates explicit user actions to existing desktop tools. Plugin code runs with the user's unsandboxed privileges; plaintext may exist in process memory or the system clipboard. Local encryption is not server end-to-end encryption. Inbox deletion does not erase backups or server data.

## Verification and approval boundary

The fixed-image release build must compare both committed executables byte for byte before verified packaging. See [REPRODUCIBLE-BUILD.md](REPRODUCIBLE-BUILD.md). Current tests cover the removed selector, current HTTPS transport, durable recovery and sensitive-history/inbox behavior; see [ACCEPTANCE.md](ACCEPTANCE.md) for actual results and desktop limits. Approval remains with the marketplace maintainer for the exact candidate commit. The author does not apply an approval label.
