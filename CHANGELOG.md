# Changelog

## 0.2.1 (2026-10-31)

- Use the shared `rustclipsync-core` engine for HTTPS transport, durable message state, retry, filtering and long polling.
- Keep Wayland access, Secret Service and history storage in the Linux adapter; preserve existing state filenames, keyring accounts and encrypted state compatibility.
- Include a verified shared-core source snapshot so independent checkouts and source packages remain buildable.
- Sync the containing directory after private state replacement, and bundle rebuilt 0.2.1 daemon/client binaries.

Validated locally on Omarchy and confirmed working by the user. Release assets include the plugin package, Linux x86_64 daemon and SHA-256 checksums. Current checks and remaining test limits are recorded in [ACCEPTANCE.md](ACCEPTANCE.md).
