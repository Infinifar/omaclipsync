# Omaclipsync

English · [简体中文](README-ZH.md)

<p align="center">
  <img src="assets/omaclipsync-icon.svg" alt="Omaclipsync icon" width="112">
</p>

Omaclipsync is a lightweight text clipboard client plugin for Omarchy Quattro, using the dedicated [RustClipSync V1 server](https://github.com/Infinifar/rustclipsync-server) protocol. Its synchronization core is written in Rust and its interface lives in the Omarchy bar.

For Android, use the [Fcitx5 Android input method](https://github.com/Infinifar/fcitx5-android-bibi-keyboard) with its built-in V1 synchronization module. All three components are under development; use V1-compatible versions together.

Measurements from the older protocol do not describe V1; V1 desktop resource usage still needs separate measurement.

<p align="center">
  <img src="preview.png" alt="Omaclipsync main panel">
</p>

The screenshot shows the running plugin; the server address and last synchronization time have been removed for privacy.

## Features

- **Real-time receive and event-driven push**: local copies use Wayland data-control events. HTTPS long polling signals remote changes, after which the client retrieves the latest value.
- **Automatic recovery**: when the real-time connection is unavailable, the client falls back to polling every 60 seconds by default, configurable from 30 to 300 seconds. Older values below 30 seconds are treated as 30 seconds. Reconnection uses backoff and checks the latest state immediately after reconnecting.
- **Desktop integration**: bar status and controls for push, receive, and pause, with live English and Chinese translations.
- **Sync direction**: choose push only, receive only, or two-way automatic sync (the default) in the main panel. Manual push and receive remain available in every mode; pause stops automatic sync.
- **Optional local history**: off by default; browse, copy, paste, and favorite up to five entries. Favorites stay at the top and survive clearing ordinary history. Enabled history is stored as plaintext; system clipboard history can be used instead.
- **Sync notifications**: enabled by default and configurable in Settings. Successful pushes and receives show a notification without revealing clipboard text; unchanged and failed operations do not.
- **Sensitive clipboard policy**: synchronization of password-manager-marked selections is off by default and can be enabled in Settings. This applies to automatic and manual pushes.
- **Filtering and deduplication**: regex blacklist or whitelist and optional filtering on receive. Message IDs and a persistent per-device cursor suppress retries and repeated notifications while allowing distinct copies of identical text.

If notifications do not appear, check Omarchy's Do Not Disturb mode with `omarchy shell notifications isDnd`. On Omarchy 4.5.0 RC, notifications received while it is on are kept in notification history without a pop-up. Turn it off in Omarchy or run `omarchy shell notifications setDnd off` to allow pop-ups again.

## Requirements and limitations

- Deploy a [RustClipSync V1 server](https://github.com/Infinifar/rustclipsync-server). This plugin does not include a server. The Android client is the [Fcitx5 Android V1 development build](https://github.com/Infinifar/fcitx5-android-bibi-keyboard).
- **UTF-8 text only**: image and file synchronization are not supported. [LocalSend](https://localsend.org/), included in standard Omarchy installations, can already transfer images and files manually between devices.
- Multiple accounts and WebDAV are not currently supported; configure one server account at a time.
- Requires Linux, Omarchy Quattro / Quickshell, and a Wayland compositor with data-control support. The bundled binaries target **x86_64** only. X11/XWayland, Windows, and macOS are not supported.
- This is a development release; multi-monitor behavior, long-term stability, and all desktop environments have not been fully validated. See [acceptance notes](ACCEPTANCE.md).

## Install

**Dependencies**: an available Secret Service keyring and `secret-tool`; Omarchy's notification service and `omarchy notification send` for notifications; native Wayland data-control watching for V1 synchronization; `wtype` for automatic pasting from history. Git installation also requires Git and a trusted HTTPS server.

For the Omaclipsync icon to remain visible in notification history, install the bundled icon into the user icon theme after installing the plugin:

```sh
install -Dm644 "$HOME/.config/omarchy/plugins/io.github.infinifar.omaclipsync/assets/omaclipsync-icon.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/io.github.infinifar.omaclipsync.svg"
gtk-update-icon-cache -ft "$HOME/.local/share/icons/hicolor"
```

Live notifications also supply the bundled icon directly. Remove the installed icon file when uninstalling if it is no longer needed. `gtk-update-icon-cache` is provided by GTK; a desktop session may need to reload its icon theme after installation.

Plugins run as unsandboxed code with your user's permissions inside Omarchy shell. Review the source and bundled x86_64 binaries before installing and enabling:

If upgrading from the old `omaclipsync` plugin ID, first disable it to prevent two clients from syncing or writing history at the same time:

```sh
omarchy plugin disable omaclipsync
```

Alternatively, build a package from source and back up, deploy, and enable it yourself:

```sh
./scripts/package.sh --use-bundled
```

The packaging command overwrites outputs of the same version in `dist/`; back up any old packages you want to keep first. Git installation is also available:

```sh
omarchy plugin add https://github.com/Infinifar/omaclipsync.git --enable
```

### Manually deploying and rolling back the V1 development package

Run these commands yourself only once the V1 server and other clients are ready. They disable the current plugin, move its directory into a backup under your user data directory, and load the package. The manually installed plugin will not be Git-managed. Configuration, history, and keyring entries are outside the plugin directory and need their own backups. Replace `package` with the absolute path to the verified V1 package:

```sh
(
  set -eu
  plugin_id='io.github.infinifar.omaclipsync'
  load_root="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/plugins"
  backup_root="${XDG_DATA_HOME:-$HOME/.local/share}/omaclipsync-backups"
  package='/absolute/path/to/omaclipsync-plugin-v1-development.tar.gz'
  test -f "$package" && test -d "$load_root/$plugin_id"
  mkdir -p "$backup_root"
  backup=$(mktemp -d "$backup_root/$plugin_id.XXXXXXXX")
  printf 'Previous plugin will be backed up to: %s/previous\n' "$backup"
  omarchy plugin disable "$plugin_id"
  mv "$load_root/$plugin_id" "$backup/previous"
  mkdir -p "$load_root/$plugin_id"
  tar -xzf "$package" -C "$load_root/$plugin_id"
  omarchy plugin validate "$load_root/$plugin_id"
  omarchy-shell shell rescanPlugins
  omarchy plugin enable "$plugin_id"
)
```

Keep the printed backup path. If validation or enabling fails, retain the backup and inspect the error; do not move it again blindly. To roll back, disable V1, move the current plugin directory to a new backup directory, move the saved `previous` directory back to the original load location, then run `omarchy-shell shell rescanPlugins` and `omarchy plugin enable io.github.infinifar.omaclipsync`. The server must also be rolled back to an image and database backup compatible with the old plugin.

The plugin ID is `io.github.infinifar.omaclipsync`. The repository includes `bin/syncclipd` and `bin/syncclipctl`. Enabling it runs the bundled daemon; installation does not download a separate binary or start a second Quickshell.

When upgrading, the configuration, history, logs, and keyring entries keep their existing `omaclipsync` locations and identifiers. Update any shortcut that invokes the old IPC target to use `io.github.infinifar.omaclipsync`.

## Setup and use

1. Open Omaclipsync in the bar, choose **Settings**, and save the server HTTPS URL, username, and password. Include the deployment's URL path prefix if one is configured.
2. The password is stored in the user's Secret Service keyring, not in the configuration file. Leaving the password blank keeps the existing password for that account; unlock the keyring if needed.
3. Initial pairing starts at the current server revision without overwriting the local clipboard; the selection present at startup is not uploaded. Offline synchronization catches up to the latest value. Conflicting pending local text is preserved; explicitly use **Push** to choose the current local clipboard. **Receive** and **Pause** are also available.
4. Enable local history and filters if desired. Selecting a history entry changes the system clipboard and may push it to the server when synchronization is enabled. Paste sends Shift+Insert to the application active before opening history; if that window is unavailable, the item is copied and a notification is shown.

Open the main panel or history from the bar, or run these commands; the plugin does not automatically change keybindings. In the history panel, select **Back** to return to the main panel:

```sh
omarchy shell io.github.infinifar.omaclipsync open
omarchy shell io.github.infinifar.omaclipsync openHistory
```

For example, after checking for shortcut conflicts, add this history shortcut to a compatible Omarchy Hyprland Lua binding configuration:

```lua
o.bind("SUPER + ALT + H", "Omaclipsync history", "omarchy shell io.github.infinifar.omaclipsync openHistory")
```

To cycle the automatic sync direction with `Super + Alt + C`, first check for conflicts and add this binding yourself:

```lua
o.bind("SUPER + ALT + C", "Cycle Omaclipsync direction", "omarchy shell io.github.infinifar.omaclipsync cycleDirection")
```

The shortcut cycles push only → receive only → two-way and shows the saved direction in Omarchy's bottom-center OSD. If automatic sync is paused, the OSD also says so. The plugin does not install or change keybindings.

**Show bar icon** is enabled by default in Settings. Turning it off hides the icon while the service continues to run. Configure a history shortcut before hiding the icon so you can open history and return to Settings to show it again. Keep the Omaclipsync widget in the bar layout: removing it or disabling the plugin is different from hiding its icon and may make the panel unavailable.

You can also open the main panel directly and re-enable **Show bar icon** in Settings:

```sh
omarchy shell io.github.infinifar.omaclipsync open
# Alternatively:
omarchy shell shell summon io.github.infinifar.omaclipsync '{}'
```

To request that the icon be shown or hidden without opening the panel:

```sh
omarchy shell io.github.infinifar.omaclipsync showIcon
omarchy shell io.github.infinifar.omaclipsync hideIcon
```

These commands return `queued` when the request is sent, not when the change has completed. They can be queued even if the daemon is offline, so `showIcon` can be used to attempt recovery; the widget must still be present in the bar layout for either request to take effect.

The hidden-icon mode is implemented but has not yet been verified on a running desktop.

Other commands:

```sh
omarchy shell io.github.infinifar.omaclipsync status
omarchy shell io.github.infinifar.omaclipsync push
omarchy shell io.github.infinifar.omaclipsync pull
omarchy shell io.github.infinifar.omaclipsync toggle
```

`queued` means that a command was queued, not that synchronization succeeded. The bundled `bin/syncclipctl` connects to the existing shell and supports `status`, `push`, `pull`, `toggle`, `watch`, and `config`. V1 does not force replay of processed messages.

## Permissions, network, and data

The plugin runs with the current user's permissions, reads and writes the regular Wayland clipboard, and exchanges text with the **user-configured HTTPS server**. TLS certificate validation is on by default and can only be disabled by explicitly enabling `insecure`. Requests do not follow redirects. HTTPS long polling waits up to 25 seconds and reconnects with backoff.

Text is limited to 10 MiB and metadata to 16 KiB. Automatic and manual pushes and receives skip marked sensitive content by default. If enabled, received text retains its sensitivity marker and can be saved as plaintext if local history is enabled. The protection relies on applications marking their clipboard offers; unmarked secrets cannot be identified. Changing this option discards queued clipboard events and restarts the listener. V1 pauses if native listening is unavailable so its own writes cannot create a loop.

When sensitive synchronization is enabled, received text is written to the system Wayland clipboard with a password-manager sensitivity marker. Clipboard history managers that respect this marker may omit it from their history; the history list is separate from the system clipboard itself. The plugin's own history is off by default and may store sensitive text in plaintext when enabled. Actual display and paste behavior depends on the desktop environment and application.

| Data | Default location | Behavior |
| --- | --- | --- |
| Configuration | `~/.config/omaclipsync/config.json` | Mode `0600`; contains no password |
| Password | User's Secret Service keyring | Stored per server username |
| Device token and pending text | `~/.local/state/omaclipsync/v1-*.state` | AES-GCM encrypted; the key is in Secret Service, without the administrator password |
| Local history | `~/.local/share/omaclipsync/history.json` | Off by default; plaintext when enabled, mode `0600`, default 50 entries, maximum 1000, file limit 16 MiB |
| Logs | `~/.local/state/omaclipsync/` | JSONL, mode `0600`; fixed events only, no password or clipboard text, retained for 7 UTC dates |

The corresponding `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, and `XDG_STATE_HOME` override these paths. Disabling history stops reading and writing it but does not delete existing data. Favorites do not count toward the ordinary entry limit; the 16 MiB file limit still applies to all entries.

## Uninstall and rollback

```sh
omarchy plugin disable io.github.infinifar.omaclipsync
omarchy plugin remove io.github.infinifar.omaclipsync
```

Uninstalling the plugin does not automatically delete configuration, history, logs, or keyring entries; remove those separately if desired. Some Omarchy versions move a manually copied plugin directory to a hidden backup; check the command output. Before re-enabling the old `omaclipsync` plugin, first disable the new plugin to avoid running both clients. Before rolling back to v0.1.1, back up the history file and remove all favorites: that version cannot read history containing favorite fields.

When rolling back, back up the configuration and remove fields unsupported by the older daemon (such as `sync.allowSensitive`, `sync.direction`, or `sync.notifyOnSuccess`) before starting it; older builds reject unknown configuration fields.

## Build from source

Requires Rust, Cargo, Clippy, `jq`, and GNU `tar`; integration tests also require Python 3 and OpenSSL. From the repository root:

For QML static checks on an Omarchy machine with `qmllint` installed, run `./scripts/qmllint.sh`. It resolves the installed shell modules through a temporary directory, fails on warnings, and does not modify the installed shell or the plugin directory. Set `OMARCHY_PATH` for a nonstandard Omarchy installation or `QMLLINT` to select the executable. A clean static check does not replace desktop testing.

```sh
cargo test --manifest-path core/Cargo.toml --locked
cargo clippy --manifest-path core/Cargo.toml --locked --all-targets -- -D warnings
cargo build --manifest-path core/Cargo.toml --locked --bin syncclipd
# With an isolated Wayland compositor and RCS_TEST_DISPOSABLE_WAYLAND=1:
python3 core/tests/v1_e2e.py "$PWD/core/target/debug/syncclipd" /path/to/rustclipsync-server
./scripts/package.sh
omarchy plugin validate "$PWD"
```

Artifacts are written to `dist/`; the integration tests use isolated local services. See [acceptance notes](ACCEPTANCE.md) for further validation and limitations.

## License

[MIT](LICENSE). Rust dependency licenses and notices: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Native clipboard watching uses [wl-clipboard-rs](https://docs.rs/wl-clipboard-rs/0.9.5/wl_clipboard_rs/watch/index.html).
