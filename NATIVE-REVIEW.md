# Native change review guide

Plugin: `io.github.infinifar.omaclipsync`. Baseline: marketplace-listed commit `371294deb092657a67f02a8fcfed1621e9340595` (0.1.6). Previous pending target: `72a0b6e651748b912055ad79a1ac5653caa74dd8` (0.2.3). The 0.2.4 remediation changes build provenance and the package version; it does not add synchronization behavior. This guide is an inventory for the maintainer's security review, **not** independent approval.

## Scope and source boundaries

From the listed baseline, the native delta adds RustClipSync V1 and the sensitive inbox, then extracts the portable engine into a committed source snapshot. It is not just a screenshot/QML update. Review all of the following groups:

| Files | Behavior and review boundary |
| --- | --- |
| `core/src/bin/syncclipd.rs` | Selects V1/legacy engine, pairing/credential state, receive/retry scheduling, event handling, configuration transitions, inbox IPC, and shutdown. Fixed-size main event queue; stale watcher/notification generations are ignored. |
| `core/src/config.rs` | Explicit `v1` / `legacy` protocol, V1 default; sensitive synchronization defaults off. New configs default to inbox mode; existing serialized sync configs missing `autoApplySensitive` retain the prior automatic clipboard behavior. |
| `core/src/v1_session.rs`, `v1_state.rs`, `v1_client.rs`, `v1_worker.rs` | Linux adapters/re-exports: Wayland clipboard, per-server/user Secret Service state key, bounded private encrypted state storage. Failed key lookup for existing state does not replace its key. Explicit inbox copy uses the device origin marker and cancels queued upload. |
| `core/vendor/rustclipsync-core/src/client.rs`, `transport.rs`, `worker.rs` | V1 HTTPS transport: Basic auth for registration, device bearer identity afterward; metadata long polling. HTTPS-only base URL without credentials/query/fragment; redirects disabled. Normal request timeout 10 s, changes timeout 30 s, metadata bounded to 16 KiB, text bounded to 10 MiB. Reconnection backs off up to 30 s. Worker cancellation waits for an in-flight bounded request to finish. |
| `core/vendor/rustclipsync-core/src/session.rs`, `state.rs` | Durable outbox/cursor/ack state, epoch and revision checks, stale-response/clipboard generation checks, re-pair and restart recovery, latest sensitive inbox. State is AES-256-GCM via ring with random 96-bit nonce and authenticated format marker, limited to 64 MiB; text fields have bounded Base64 encoding. Encryption key is held separately in Secret Service. This is local encryption, not end-to-end server encryption. |
| `core/vendor/rustclipsync-core/src/adapters.rs`, `filter.rs`, `lib.rs`, `error.rs` | Shared adapter contracts, outcome/filter definitions and error classification. The snapshot is version 0.2.0; `UPSTREAM.json` records origin and file hashes. Builds never fetch or execute the sibling project or a remote shared-core checkout. |
| `core/src/inbox.rs`, `history.rs` | General status includes inbox ID/time only. Explicit ID-checked reads return at most 16 KiB UTF-8 chunks; copy and delete also check current ID. Sensitive V1 uploads/downloads do not enter normal history. Ordinary history is still plaintext if the user enables it. |
| `core/src/wayland.rs` | Regular selection only; password-manager hint and origin MIME markers, V1 clear events, input size/read timeout, sensitive write marker. A MIME hint is an application convention, not reliable detection of every secret. |
| `core/src/session.rs`, `signalr.rs`, `lib.rs` | Legacy SyncClipboard path remains explicit: sensitivity policy for local reading; shared filter/outcome re-exports and existing SignalR support. Legacy servers do not provide V1 sensitive-receipt metadata or the inbox protocol. |
| `core/src/storage.rs` | Private atomic binary state writes without appending an extra newline; file sync plus containing-directory sync in the V1 adapter. |
| `core/Cargo.toml`, `Cargo.lock` | New local path dependency and its crypto dependencies are locked; review the lockfile delta, crate checksums, and image toolchain as supply-chain inputs. |
| `core/tests/`, shared snapshot `VALIDATION.md` | Mocked protocol/recovery tests and isolated V1 integration harness. These provide regression evidence and do not replace review of production paths. |

`core/src/bin/syncclipctl.rs` is unchanged from the listed baseline. Its binary still changes because of package/dependency/toolchain inputs. It delegates fixed commands to the existing `omarchy-shell` process and reads configuration; it does not start another daemon. Both committed binaries therefore require a fresh comparison.

## Execution, persistence and networking

- `Service.qml` starts the plugin-relative `bin/syncclipd` when the service is instantiated/enabled (`Component.onCompleted: launch()` and `Process.command`). It sends JSON requests over stdin/stdout, bounds restart attempts/backoff, and requests shutdown on destruction. A cloned/enabled plugin therefore executes native code immediately. There is no sandbox or deferred installer approval.
- Native subprocess capabilities include Secret Service through `secret-tool` and clipboard fallback through `wl-paste`/the current executable; the QML layer also delegates explicit history paste to existing desktop tools. They use fixed command/argument arrays, not shell interpolation of clipboard text. These pre-existing capabilities remain relevant to review.
- V1 accesses the user-configured HTTPS server's `/v1/devices`, `/v1/state`, `/v1/changes`, message content/upload/cancel, and ack routes under the configured base. It is an opt-in destination via account configuration, not a fixed third-party telemetry endpoint. TLS verification is on by default; the pre-existing explicit insecure setting weakens it and must be reviewed separately.
- Configuration, encrypted pending/inbox state, optional plaintext normal history and redacted logs persist under the user's XDG directories. Per-account state keys/passwords are stored via Secret Service. Deleting an inbox item removes that local item; it is not secure erasure of backups or server-side deletion.
- Remote sensitive text is delivered to QML only after an explicit reveal request; preview is capped at 64 KiB and rendered as plain text. It may remain in process memory. Explicit copy writes the full item to the system clipboard with a sensitivity hint, where other permitted desktop clients can access it.

## Build evidence and remaining approval

The former release workflow only packaged committed bytes. Version 0.2.4 introduces an immutable-image clean build, compares both outputs byte for byte, and packages those compared outputs; see [REPRODUCIBLE-BUILD.md](REPRODUCIBLE-BUILD.md). No existing release tag is rewritten, and there is no claim that the 0.2.3 bytes were independently reproduced.

To inspect the full native delta at an exact candidate commit:

```sh
git diff --stat 371294deb092657a67f02a8fcfed1621e9340595 <candidate> -- core Service.qml bin scripts .github .gitlab-ci.yml
git diff 371294deb092657a67f02a8fcfed1621e9340595 <candidate> -- core Service.qml
```

Marketplace approval remains with its maintainer. A successful workflow, hashes, upstream export metadata, or this author's inventory must not be described as an independent security approval. Desktop behavior was validated on earlier versions; a reproducible build alone does not repeat the desktop or Android test matrix.
