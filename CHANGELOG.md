# Changelog

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
