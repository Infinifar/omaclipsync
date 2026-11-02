# Changelog

## 0.3.0 (2026-11-01)

- Fix sensitive-policy hot reload through shared core `b19057b`: disabling sensitive synchronization while marked text remains on the clipboard no longer blocks ordinary receives. Real clipboard failures and sensitive receive filtering remain enforced.
- Support only the current RustClipSync protocol. Remove the selectable SyncClipboard transport, its profile/file APIs, SignalR connection, fallback watcher and obsolete tests/examples.
- Remove the protocol selector from saved settings and the panel. An existing selector is consumed without enabling any alternate transport; HTTP endpoints now use `/api/...`; encrypted state filenames and device identities remain unchanged.
- Rename current Linux adapter modules and the real-time transport label. Retain current synchronization, sensitive inbox, history, direction and recovery behavior.
- Address the marketplace sensitive plaintext retention finding by removing the affected transport entirely. Existing history is not automatically erased.

## 0.2.5 (2026-11-01)

- Update the shared core to fix pending-upload filtering, restart validation, oversized-copy replacement, slow-download echo suppression, ambiguous clipboard write recovery and connection-event delivery under backpressure.
- Preserve pairing and encrypted state; include standard error chains and explicit worker shutdown/join APIs.
- Rebuild both bundled executables with the fixed toolchain and verify their bytes before packaging.

## 0.2.4 (2026-10-31)

- Pin the native build image, compiler/linker and paths; rebuild and byte-compare both committed executables in GitHub/GitLab CI before packaging.
- Publish the exact-commit build evidence and both executable checksums alongside the verified archive. Preserve existing release tags.
- Document the full native delta from the listed marketplace snapshot and the limits of build provenance versus security approval. No synchronization behavior changes.

## 0.2.3 (2026-10-31)

- Replace documentation screenshots with fresh English UI captures, using harmless sample text and redacted account details.
- Simplify an equivalent boolean condition for the newer GitLab Clippy version.

## 0.2.2 (2026-10-31)

- Add a masked sensitive inbox to the main panel, with receipt time, explicit reveal, full-text copy and local deletion. Keep only the latest item in encrypted state, independently of ordinary history.
- Preserve automatic sensitive clipboard writing as a setting. New configurations use the inbox; older saved configurations retain automatic writing until changed.
- Upgrade the generated shared-core snapshot to 0.2.0. Keep sensitive bodies out of status, notifications and new ordinary history entries; reject stale item actions and suppress re-upload of explicit inbox copies.
- Bound previews to 64 KiB and clear them when hidden, replaced, changing views or losing the background connection. No additional server polling.


## 0.2.1 (2026-10-31)

- Use the shared `rustclipsync-core` engine for HTTPS transport, durable message state, retry, filtering and long polling.
- Keep Wayland access, Secret Service and history storage in the Linux adapter; preserve existing state filenames, keyring accounts and encrypted state compatibility.
- Include a verified shared-core source snapshot so independent checkouts and source packages remain buildable.
- Sync the containing directory after private state replacement, and bundle rebuilt 0.2.1 daemon/client binaries.

Validated locally on Omarchy and confirmed working by the user. Release assets include the plugin package, Linux x86_64 daemon and SHA-256 checksums. Current checks and remaining test limits are recorded in [ACCEPTANCE.md](ACCEPTANCE.md).
