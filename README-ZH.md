# Omaclipsync

[English](README.md) · 简体中文

<p align="center">
  <img src="assets/omaclipsync-icon.svg" alt="Omaclipsync 图标" width="112">
</p>

Omaclipsync 是 [SyncClipboard](https://github.com/Jeric-X/SyncClipboard) 的 Omarchy Quattro 轻量级文本剪贴板客户端插件。同步核心由 Rust 编写，界面融入 Omarchy 顶栏，帮助使用 SyncClipboard 的多台设备同步文本。

在实际测试中,Omaclipsync 的 Rust 守护进程峰值约 **7.25 MiB**，9 分钟平均 CPU 约 **0.022%**。其中不包含共享 Omarchy shell 的内存。

<p align="center">
  <img src="preview.png" alt="Omaclipsync 主面板">
</p>

截图来自实际运行的插件；服务器地址和最近同步时间已为保护隐私而遮盖。

## 特点

- **实时接收，事件驱动推送**：本机复制由 Wayland data-control 事件触发；远端变化通过 SignalR / WebSocket 通知，随后通过 HTTPS 获取并校验内容。
- **自动恢复**：实时连接中断后回退到低频轮询；默认 60 秒，可设置 30–300 秒。旧配置低于 30 秒时按 30 秒执行。后台按退避策略重连，恢复后立即检查最新内容。
- **融入桌面**：顶栏显示连接状态，面板提供推送、接收和暂停；支持中文和英文随系统语言切换。
- **可选的本地历史**：默认关闭，可浏览、复制、粘贴和收藏最多 5 条；收藏置顶，清空普通历史时保留。启用后内容以明文保存在本机，也可以使用系统剪贴板历史代替。
- **过滤与去重**：支持正则黑名单、白名单和可选的接收过滤；默认 5 分钟内不重复写入相同的远端内容，可手动强制接收。

## 使用前须知与限制

- 需要自行部署 [SyncClipboard 服务端](https://github.com/Jeric-X/SyncClipboard)，或已有可访问的兼容服务。插件不包含服务端。
- **仅同步 UTF-8 文本**，不支持同步图片或文件。Omarchy 默认提供的 [LocalSend](https://localsend.org/) 已经可用于手动在设备间传输图片和文件。
- 暂不支持多账户和 WebDAV；一次只配置一个 SyncClipboard 服务器账户。
- 需要 Linux、Omarchy Quattro / Quickshell 和支持 data-control 协议的 Wayland 合成器。当前预编译包仅支持 **x86_64**；不支持 X11/XWayland、Windows 或 macOS。
- 仍是开发版；多屏、长期稳定性及所有桌面环境尚未完整验收。请参阅 [验收记录](ACCEPTANCE.md)。

## 安装

**依赖**：可用的 Secret Service 密钥环及 `secret-tool`；原生剪贴板监听不可用时需要 `wl-paste`；历史条目的自动粘贴需要 `wtype`。使用 Git 安装还需要 Git 和受信任的 HTTPS 服务器。

插件运行在未沙箱化的 Omarchy shell 中，具有当前用户权限；请先审阅源码及仓库内的 x86_64 预编译二进制。安装并启用：

如果从旧 ID `omaclipsync` 升级，请先禁用旧插件，避免两个客户端同时同步或写入历史：

```sh
omarchy plugin disable omaclipsync
```

然后安装并启用新插件：

```sh
omarchy plugin add https://github.com/Infinifar/omaclipsync.git --enable
```

插件 ID 为 `io.github.infinifar.omaclipsync`。安装源包含 `bin/syncclipd` 和 `bin/syncclipctl`；启用后会运行守护进程，安装时不另行下载二进制或启动第二个 Quickshell。

升级时配置、历史、日志和密钥环项仍使用原有的 `omaclipsync` 位置和标识。若快捷键使用了旧 IPC target，请手动改为 `io.github.infinifar.omaclipsync`。

## 配置和使用

1. 在顶栏打开 Omaclipsync，进入 **设置**，填写服务器的 HTTPS 地址、用户名和密码并保存。URL 必须指向 SyncClipboard 服务根地址，包括部署时配置的路径前缀（如有）。
2. 密码保存到系统 Secret Service 密钥环，不写入配置文件；留空会保留当前账户已有密码。密钥环锁定时请先解锁。
3. 同步启用时，本机复制会自动推送，远端变更会自动接收。启动时已有的剪贴板内容不会自动推送；如有需要，可使用面板上的 **推送** 按钮。面板也提供 **接收** 和 **暂停**。
4. 在设置中按需启用历史和过滤。选择历史条目会改变系统剪贴板，开启同步时可能被推送到服务器。粘贴会向打开历史前的应用发送 Shift+Insert；如果原窗口不可用，则仅复制并提示。

历史面板可以从顶栏打开，也可绑定以下命令；快捷键由用户自行配置，插件不会自动修改键位：

```sh
omarchy shell io.github.infinifar.omaclipsync openHistory
```

例如，在适用的 Omarchy Hyprland Lua 绑定配置中，可在确认快捷键未冲突后添加：

```lua
o.bind("SUPER + ALT + V", "Omaclipsync history", "omarchy shell io.github.infinifar.omaclipsync openHistory")
```

设置中的 **显示顶栏图标** 默认开启。关闭后只隐藏图标，服务继续运行。建议隐藏前先配置历史快捷键，以便打开历史面板、返回设置并重新显示图标。请保留 Omaclipsync 在顶栏布局中的组件：移除组件或禁用插件不同于隐藏图标，可能导致面板无法打开。

也可以直接打开主面板，在设置中重新开启 **显示顶栏图标**：

```sh
omarchy shell io.github.infinifar.omaclipsync open
# 或者：
omarchy shell shell summon io.github.infinifar.omaclipsync '{}'
```

也可以不打开面板，直接请求显示或隐藏图标：

```sh
omarchy shell io.github.infinifar.omaclipsync showIcon
omarchy shell io.github.infinifar.omaclipsync hideIcon
```

这些命令返回 `queued` 表示请求已发出，不代表更改已经完成。即使守护进程离线，也可以排队请求，尝试通过 `showIcon` 恢复图标；这两个请求都要求组件仍保留在顶栏布局中才会生效。

隐藏图标模式已实现，但尚未在实际桌面验证。

其他命令：

```sh
omarchy shell io.github.infinifar.omaclipsync status
omarchy shell io.github.infinifar.omaclipsync push
omarchy shell io.github.infinifar.omaclipsync pull
omarchy shell io.github.infinifar.omaclipsync pullForce
omarchy shell io.github.infinifar.omaclipsync toggle
```

`queued` 表示命令已排队，不代表同步已经成功。`bin/syncclipctl` 也可连接现有 shell，支持 `status`、`push`、`pull [--force]`、`toggle`、`watch` 和 `config`。

## 权限、网络与数据

插件以当前用户权限读取和写入普通 Wayland 剪贴板，向**用户配置的 HTTPS 服务**发送或接收文本。默认校验证书，只有在设置中显式开启 `insecure` 才会关闭校验；HTTP 和 SignalR 协商不跟随重定向。SignalR 协商遵循系统代理环境变量，WebSocket 目前直连；依赖代理才能访问服务器的网络可能回退到轮询。

同步内容上限为 10 MiB，元数据与 SignalR 消息上限为 256 KiB；长文本通过服务端的文本数据接口传输。自动监听会跳过带密码管理器敏感标记的内容；手动推送不应用此保护，请自行确认内容是否适合共享。降级使用 `wl-paste --watch` 且无法读取初始快照时，为避免覆盖服务器，首个数据事件可能被忽略。

| 数据 | 默认位置 | 说明 |
| --- | --- | --- |
| 配置 | `~/.config/omaclipsync/config.json` | 文件权限 `0600`，不包含密码 |
| 密码 | 当前用户的 Secret Service 密钥环 | 按服务器用户名保存 |
| 本地历史 | `~/.local/share/omaclipsync/history.json` | 默认关闭；启用后明文保存，权限 `0600`，默认 50 条、最多 1000 条，文件上限 16 MiB |
| 日志 | `~/.local/state/omaclipsync/` | JSONL，权限 `0600`；记录固定事件，不记录密码和正文，按 UTC 日期保留 7 天 |

设置了 `XDG_CONFIG_HOME`、`XDG_DATA_HOME` 或 `XDG_STATE_HOME` 时，相应路径以这些目录为准。关闭历史后不会读取或写入历史文件，但已有文件不会自动删除。历史收藏不计入普通历史条数上限；所有记录仍受 16 MiB 文件上限约束。

## 卸载与回退

```sh
omarchy plugin disable io.github.infinifar.omaclipsync
omarchy plugin remove io.github.infinifar.omaclipsync
```

移除插件不会自动删除配置、历史、日志或密钥环项，请按需自行清理。手动复制安装的目录在某些 Omarchy 版本中可能被移至隐藏备份；请留意命令输出。重新启用旧 `omaclipsync` 插件前，请先禁用新插件，避免两个客户端同时运行。回退到 v0.1.1 前应备份历史文件并取消全部收藏：该版本无法读取包含收藏字段的历史。

## 从源码构建

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

## 许可证

[MIT](LICENSE)。Rust 依赖许可证与声明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。协议实现参考 [SyncClipboard 上游](https://github.com/Jeric-X/SyncClipboard)（核对版本 `fd25fd62409967a69cec842cabaec2c00adf901d`）；原生剪贴板监听使用 [wl-clipboard-rs](https://docs.rs/wl-clipboard-rs/0.9.5/wl_clipboard_rs/watch/index.html)。
