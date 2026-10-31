# Changelog

## 0.2.1 — local release candidate (2026-10-31)

- Use the shared `rustclipsync-core` engine for HTTPS transport, durable message state, retry, filtering and long polling.
- Keep Wayland access, Secret Service and history storage in the Linux adapter; preserve existing state filenames, keyring accounts and encrypted state compatibility.
- Include a verified shared-core source snapshot so independent checkouts and source packages remain buildable.
- Sync the containing directory after private state replacement, and bundle rebuilt 0.2.1 daemon/client binaries.

This version is prepared for local testing. No GitHub release or release tag has been published. Current checks and desktop-test limits are recorded in [ACCEPTANCE.md](ACCEPTANCE.md).
