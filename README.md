# Omaclipsync

[中文](#中文) · [English](#english)

## 中文

Omaclipsync 是 [SyncClipboard](https://github.com/Jeric-X/SyncClipboard) 的 Omarchy Quattro 轻量级文本剪贴板客户端插件。同步核心由 Rust 编写，界面融入 Omarchy 顶栏，帮助使用 SyncClipboard 的多台设备同步文本。

在一次本机测试中，原 SyncClipboard Desktop 进程占用 **400 多 MiB** 内存，而 Omaclipsync 的 Rust 守护进程峰值约 **7.25 MiB**，9 分钟平均 CPU 约 **0.022%**。这是单机、不同客户端的观察值，后者不包含共享 Omarchy shell 的内存；实际占用会随环境、配置和剪贴板内容变化，并非性能保证。

### 特点

- **实时接收，事件驱动推送**：本机复制由 Wayland data-control 事件触发；远端变化通过 SignalR / WebSocket 通知，随后通过 HTTPS 获取并校验内容。
- **自动恢复**：实时连接中断后回退到低频轮询；默认 60 秒，可设置 30–300 秒。旧配置低于 30 秒时按 30 秒执行。后台按退避策略重连，恢复后立即检查最新内容。
- **融入桌面**：顶栏显示连接状态，面板提供推送、接收和暂停；支持中文和英文随系统语言切换。
- **可选的本地历史**：默认关闭，可浏览、复制、粘贴和收藏最多 5 条；收藏置顶，清空普通历史时保留。启用后内容以明文保存在本机，也可以使用系统剪贴板历史代替。
- **过滤与去重**：支持正则黑名单、白名单和可选的接收过滤；默认 5 分钟内不重复写入相同的远端内容，可手动强制接收。

### 使用前须知与限制

- 需要自行部署 [SyncClipboard 服务端](https://github.com/Jeric-X/SyncClipboard)，或已有可访问的兼容服务。插件不包含服务端。
- **仅同步 UTF-8 文本**，不支持同步图片或文件。Omarchy 默认提供的 [LocalSend](https://localsend.org/) 可用于手动在设备间传输图片和文件；它不是本插件的自动剪贴板同步功能。
- 暂不支持多账户和 WebDAV；一次只配置一个 SyncClipboard 服务器账户。
- 需要 Linux、Omarchy Quattro / Quickshell 和支持 data-control 协议的 Wayland 合成器。当前预编译包仅支持 **x86_64**；不支持 X11/XWayland、Windows 或 macOS。
- 仍是开发版；多屏、长期稳定性及所有桌面环境尚未完整验收。请参阅 [验收记录](ACCEPTANCE.md)。

### 安装

**依赖**：可用的 Secret Service 密钥环及 `secret-tool`；原生剪贴板监听不可用时需要 `wl-paste`；历史条目的自动粘贴需要 `wtype`。使用 Git 安装还需要 Git 和受信任的 HTTPS 服务器。

插件运行在未沙箱化的 Omarchy shell 中，具有当前用户权限；请先审阅源码及仓库内的 x86_64 预编译二进制。安装并启用：

```sh
omarchy plugin add https://github.com/Infinifar/omaclipsync.git --enable
```

插件 ID 为 `omaclipsync`。安装源包含 `bin/syncclipd` 和 `bin/syncclipctl`；启用后会运行守护进程，安装时不另行下载二进制或启动第二个 Quickshell。

### 配置和使用

1. 在顶栏打开 Omaclipsync，进入 **设置**，填写服务器的 HTTPS 地址、用户名和密码并保存。URL 必须指向 SyncClipboard 服务根地址，包括部署时配置的路径前缀（如有）。
2. 密码保存到系统 Secret Service 密钥环，不写入配置文件；留空会保留当前账户已有密码。密钥环锁定时请先解锁。
3. 同步启用时，本机复制会自动推送，远端变更会自动接收。启动时已有的剪贴板内容不会自动推送；如有需要，可使用面板上的 **推送** 按钮。面板也提供 **接收** 和 **暂停**。
4. 在设置中按需启用历史和过滤。选择历史条目会改变系统剪贴板，开启同步时可能被推送到服务器。粘贴会向打开历史前的应用发送 Shift+Insert；如果原窗口不可用，则仅复制并提示。

历史面板可以从顶栏打开，也可绑定以下命令；快捷键由用户自行配置，插件不会自动修改键位：

```sh
omarchy shell omaclipsync openHistory
```

例如，在适用的 Omarchy Hyprland Lua 绑定配置中，可在确认快捷键未冲突后添加：

```lua
o.bind("SUPER + ALT + V", "Omaclipsync history", "omarchy shell omaclipsync openHistory")
```

其他命令：

```sh
omarchy shell omaclipsync status
omarchy shell omaclipsync push
omarchy shell omaclipsync pull
omarchy shell omaclipsync pullForce
omarchy shell omaclipsync toggle
```

`queued` 表示命令已排队，不代表同步已经成功。`bin/syncclipctl` 也可连接现有 shell，支持 `status`、`push`、`pull [--force]`、`toggle`、`watch` 和 `config`。

### 权限、网络与数据

插件以当前用户权限读取和写入普通 Wayland 剪贴板，向**用户配置的 HTTPS 服务**发送或接收文本。默认校验证书，只有在设置中显式开启 `insecure` 才会关闭校验；HTTP 和 SignalR 协商不跟随重定向。SignalR 协商遵循系统代理环境变量，WebSocket 目前直连；依赖代理才能访问服务器的网络可能回退到轮询。

同步内容上限为 10 MiB，元数据与 SignalR 消息上限为 256 KiB；长文本通过服务端的文本数据接口传输。自动监听会跳过带密码管理器敏感标记的内容；手动推送不应用此保护，请自行确认内容是否适合共享。降级使用 `wl-paste --watch` 且无法读取初始快照时，为避免覆盖服务器，首个数据事件可能被忽略。

| 数据 | 默认位置 | 说明 |
| --- | --- | --- |
| 配置 | `~/.config/omaclipsync/config.json` | 文件权限 `0600`，不包含密码 |
| 密码 | 当前用户的 Secret Service 密钥环 | 按服务器用户名保存 |
| 本地历史 | `~/.local/share/omaclipsync/history.json` | 默认关闭；启用后明文保存，权限 `0600`，默认 50 条、最多 1000 条，文件上限 16 MiB |
| 日志 | `~/.local/state/omaclipsync/` | JSONL，权限 `0600`；记录固定事件，不记录密码和正文，按 UTC 日期保留 7 天 |

设置了 `XDG_CONFIG_HOME`、`XDG_DATA_HOME` 或 `XDG_STATE_HOME` 时，相应路径以这些目录为准。关闭历史后不会读取或写入历史文件，但已有文件不会自动删除。历史收藏不计入普通历史条数上限；所有记录仍受 16 MiB 文件上限约束。

### 卸载与回退

```sh
omarchy plugin disable omaclipsync
omarchy plugin remove omaclipsync
```

移除插件不会自动删除配置、历史、日志或密钥环项，请按需自行清理。手动复制安装的目录在某些 Omarchy 版本中可能被移至隐藏备份；请留意命令输出。回退到 v0.1.1 前应备份历史文件并取消全部收藏：该版本无法读取包含收藏字段的历史。

### 从源码构建

需要 Rust、Cargo、Clippy、`jq` 和 GNU `tar`；集成测试还需要 Python 3 和 OpenSSL。在源码目录运行：

```sh
cargo test --manifest-path core/Cargo.toml --locked
cargo clippy --manifest-path core/Cargo.toml --locked --all-targets -- -D warnings
cargo build --manifest-path core/Cargo.toml --locked --bin syncclipd
python3 core/tests/signalr_e2e.py "$PWD/core/target/debug/syncclipd"
python3 core/tests/offline_recovery.py "$PWD/core/target/debug/syncclipd"
./scripts/package.sh
omarchy plugin validate "$PWD"
```

打包输出位于 `dist/`；测试使用隔离的本地服务。更多验证和已知限制见 [验收记录](ACCEPTANCE.md)。

## English

Omaclipsync is a lightweight text clipboard client plugin for Omarchy Quattro, compatible with [SyncClipboard](https://github.com/Jeric-X/SyncClipboard). Its synchronization core is written in Rust and its interface lives in the Omarchy bar.

In one local observation, the original SyncClipboard Desktop process used **over 400 MiB** of memory, while the Omaclipsync Rust daemon peaked at about **7.25 MiB** and averaged **0.022% CPU** over nine minutes. This is a single-machine observation of different clients; the daemon figure excludes the shared Omarchy shell. Usage varies with the environment, settings, and clipboard contents and is not a performance guarantee.

### Features

- **Real-time receive and event-driven push**: local copies use Wayland data-control events. SignalR over WebSocket notifies the client of remote changes, which it then retrieves and validates over HTTPS.
- **Automatic recovery**: when the real-time connection is unavailable, the client falls back to polling every 60 seconds by default, configurable from 30 to 300 seconds. Older values below 30 seconds are treated as 30 seconds. Reconnection uses backoff and checks the latest state immediately after reconnecting.
- **Desktop integration**: bar status and controls for push, receive, and pause, with live English and Chinese translations.
- **Optional local history**: off by default; browse, copy, paste, and favorite up to five entries. Favorites stay at the top and survive clearing ordinary history. Enabled history is stored as plaintext; system clipboard history can be used instead.
- **Filtering and deduplication**: regex blacklist or whitelist, optional filtering on receive, and a default five-minute deduplication window. Manual forced receive is available.

### Requirements and limitations

- Deploy your own [SyncClipboard server](https://github.com/Jeric-X/SyncClipboard), or use an existing compatible server. This plugin does not include a server.
- **UTF-8 text only**: image and file synchronization are not supported. [LocalSend](https://localsend.org/), included in standard Omarchy installations, can transfer images and files manually between devices; it is not automatic clipboard synchronization.
- Multiple accounts and WebDAV are not currently supported; configure one SyncClipboard server account at a time.
- Requires Linux, Omarchy Quattro / Quickshell, and a Wayland compositor with data-control support. The bundled binaries target **x86_64** only. X11/XWayland, Windows, and macOS are not supported.
- This is a development release; multi-monitor behavior, long-term stability, and all desktop environments have not been fully validated. See [acceptance notes](ACCEPTANCE.md).

### Install

**Dependencies**: an available Secret Service keyring and `secret-tool`; `wl-paste` is needed if native clipboard watching is unavailable; `wtype` is needed for automatic pasting from history. Git installation also requires Git and a trusted HTTPS server.

Plugins run as unsandboxed code with your user's permissions inside Omarchy shell. Review the source and bundled x86_64 binaries before installing and enabling:

```sh
omarchy plugin add https://github.com/Infinifar/omaclipsync.git --enable
```

The plugin ID is `omaclipsync`. The repository includes `bin/syncclipd` and `bin/syncclipctl`. Enabling it runs the bundled daemon; installation does not download a separate binary or start a second Quickshell.

### Setup and use

1. Open Omaclipsync in the bar, choose **Settings**, and save the server HTTPS URL, username, and password. Include the deployment's URL path prefix if one is configured.
2. The password is stored in the user's Secret Service keyring, not in the configuration file. Leaving the password blank keeps the existing password for that account; unlock the keyring if needed.
3. Local copies are pushed and remote changes received automatically while synchronization is enabled. The clipboard present at startup is not pushed automatically; use **Push** when needed. **Receive** and **Pause** are also available.
4. Enable local history and filters if desired. Selecting a history entry changes the system clipboard and may push it to the server when synchronization is enabled. Paste sends Shift+Insert to the application active before opening history; if that window is unavailable, the item is copied and a notification is shown.

Open history from the bar or bind this command yourself; the plugin does not automatically change keybindings:

```sh
omarchy shell omaclipsync openHistory
```

For example, after checking for shortcut conflicts, add this to a compatible Omarchy Hyprland Lua binding configuration:

```lua
o.bind("SUPER + ALT + V", "Omaclipsync history", "omarchy shell omaclipsync openHistory")
```

Other commands:

```sh
omarchy shell omaclipsync status
omarchy shell omaclipsync push
omarchy shell omaclipsync pull
omarchy shell omaclipsync pullForce
omarchy shell omaclipsync toggle
```

`queued` means that a command was queued, not that synchronization succeeded. The bundled `bin/syncclipctl` connects to the existing shell and supports `status`, `push`, `pull [--force]`, `toggle`, `watch`, and `config`.

### Permissions, network, and data

The plugin runs with the current user's permissions, reads and writes the regular Wayland clipboard, and exchanges text with the **user-configured HTTPS server**. TLS certificate validation is on by default and can only be disabled by explicitly enabling `insecure`. HTTP and SignalR negotiation do not follow redirects. SignalR negotiation honors system proxy environment variables, while WebSocket currently connects directly; networks requiring a proxy may fall back to polling.

Text is limited to 10 MiB; metadata and SignalR messages are limited to 256 KiB. Long text uses the server's text data endpoint. Automatic monitoring skips selections marked sensitive by password managers; manual push does not apply that protection, so check what you send. When falling back to `wl-paste --watch`, the first data event may be ignored if the initial clipboard snapshot cannot be read, to avoid overwriting the server.

| Data | Default location | Behavior |
| --- | --- | --- |
| Configuration | `~/.config/omaclipsync/config.json` | Mode `0600`; contains no password |
| Password | User's Secret Service keyring | Stored per server username |
| Local history | `~/.local/share/omaclipsync/history.json` | Off by default; plaintext when enabled, mode `0600`, default 50 entries, maximum 1000, file limit 16 MiB |
| Logs | `~/.local/state/omaclipsync/` | JSONL, mode `0600`; fixed events only, no password or clipboard text, retained for 7 UTC dates |

The corresponding `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, and `XDG_STATE_HOME` override these paths. Disabling history stops reading and writing it but does not delete existing data. Favorites do not count toward the ordinary entry limit; the 16 MiB file limit still applies to all entries.

### Uninstall and rollback

```sh
omarchy plugin disable omaclipsync
omarchy plugin remove omaclipsync
```

Uninstalling the plugin does not automatically delete configuration, history, logs, or keyring entries; remove those separately if desired. Some Omarchy versions move a manually copied plugin directory to a hidden backup; check the command output. Before rolling back to v0.1.1, back up the history file and remove all favorites: that version cannot read history containing favorite fields.

### Build from source

Requires Rust, Cargo, Clippy, `jq`, and GNU `tar`; integration tests also require Python 3 and OpenSSL. From the repository root:

```sh
cargo test --manifest-path core/Cargo.toml --locked
cargo clippy --manifest-path core/Cargo.toml --locked --all-targets -- -D warnings
cargo build --manifest-path core/Cargo.toml --locked --bin syncclipd
python3 core/tests/signalr_e2e.py "$PWD/core/target/debug/syncclipd"
python3 core/tests/offline_recovery.py "$PWD/core/target/debug/syncclipd"
./scripts/package.sh
omarchy plugin validate "$PWD"
```

Artifacts are written to `dist/`; the integration tests use isolated local services. See [acceptance notes](ACCEPTANCE.md) for further validation and limitations.

## License / 许可证

[MIT](LICENSE). Rust dependency licenses and notices: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). The protocol implementation references [SyncClipboard upstream](https://github.com/Jeric-X/SyncClipboard) (reviewed at `fd25fd62409967a69cec842cabaec2c00adf901d`); native clipboard watching uses [wl-clipboard-rs](https://docs.rs/wl-clipboard-rs/0.9.5/wl_clipboard_rs/watch/index.html).
