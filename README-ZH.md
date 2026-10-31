# Omaclipsync

[English](README.md) · 简体中文

<p align="center">
  <img src="assets/omaclipsync-icon.svg" alt="Omaclipsync 图标" width="112">
</p>

Omaclipsync 是 Omarchy Quattro 的轻量级文本剪贴板客户端插件，使用 [RustClipSync 服务端](https://github.com/Infinifar/rustclipsync-server)的专用协议。同步核心由 Rust 编写，界面融入 Omarchy 顶栏。

手机端可使用内置 RustClipSync 同步模块的 [Fcitx5 Android 输入法](https://github.com/Infinifar/fcitx5-android-bibi-keyboard)，配合该服务端与桌面插件同步。

旧协议版本的历史性能数据不适用于当前实现；实际桌面资源占用尚需单独测量。

<p align="center">
  <img src="preview.png" alt="Omaclipsync 主面板">
</p>

截图来自实际运行的插件；服务器地址和最近同步时间已为保护隐私而遮盖。

## 特点

- **实时接收，事件驱动推送**：本机复制由 Wayland data-control 事件触发；远端变化通过 HTTPS 长轮询通知，随后读取最新内容。
- **自动恢复**：实时连接中断后回退到低频轮询；默认 60 秒，可设置 30–300 秒。旧配置低于 30 秒时按 30 秒执行。后台按退避策略重连，恢复后立即检查最新内容。
- **融入桌面**：顶栏显示连接状态，面板提供推送、接收和暂停；支持中文和英文随系统语言切换。
- **同步方向**：主面板可选单向推送、单向接收或双向自动同步（默认）。任何模式下都可手动推送和接收；暂停会停止自动同步。
- **可选的本地历史**：默认关闭，可浏览、复制、粘贴和收藏最多 5 条；收藏置顶，清空普通历史时保留。启用后内容以明文保存在本机，也可以使用系统剪贴板历史代替。
- **同步通知**：默认开启，可在设置中关闭。成功推送或接收新文本时显示通知，不包含剪贴板正文；内容未变化或同步失败时不提示成功。
- **敏感内容策略**：默认不推送密码管理器标记的内容，可在设置中开启；自动同步和手动推送均遵循该设置。
- **过滤与去重**：支持正则黑名单、白名单和可选的接收过滤；按消息 ID 与每台设备的持久游标避免重试和通知造成重复写入。相同文本的不同复制事件仍可同步。

如果通知没有弹出，可以运行 `omarchy shell notifications isDnd` 检查 Omarchy 的勿扰模式。在 Omarchy 4.5.0 RC 中，开启勿扰时通知会保留在通知历史中，但不会弹窗。可在 Omarchy 中关闭勿扰，或运行 `omarchy shell notifications setDnd off` 恢复弹窗。

## 使用前须知与限制

- 需要自行部署 [RustClipSync 服务端](https://github.com/Infinifar/rustclipsync-server)。插件不包含服务端；手机端使用 [Fcitx5 Android 输入法](https://github.com/Infinifar/fcitx5-android-bibi-keyboard)。
- **仅同步 UTF-8 文本**，不支持同步图片或文件。Omarchy 默认提供的 [LocalSend](https://localsend.org/) 已经可用于手动在设备间传输图片和文件。
- 暂不支持多账户和 WebDAV；一次只配置一个服务端账户。
- 需要 Linux、Omarchy Quattro / Quickshell 和支持 data-control 协议的 Wayland 合成器。当前预编译包仅支持 **x86_64**；不支持 X11/XWayland、Windows 或 macOS。
- 仍是开发版；多屏、长期稳定性及所有桌面环境尚未完整验收。请参阅 [验收记录](ACCEPTANCE.md)。

## 安装

**依赖**：可用的 Secret Service 密钥环及 `secret-tool`；通知需要 Omarchy 通知服务和 `omarchy notification send`；自动同步需要原生 data-control 监听；历史条目的自动粘贴需要 `wtype`。使用 Git 安装还需要 Git 和受信任的 HTTPS 服务器。

为了让通知历史也能显示 Omaclipsync 图标，请在安装插件后将附带的图标安装到用户图标主题：

```sh
install -Dm644 "$HOME/.config/omarchy/plugins/io.github.infinifar.omaclipsync/assets/omaclipsync-icon.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/io.github.infinifar.omaclipsync.svg"
gtk-update-icon-cache -ft "$HOME/.local/share/icons/hicolor"
```

实时通知还会直接提供插件自带的图标。卸载后如果不再需要，可自行删除安装的图标文件。`gtk-update-icon-cache` 由 GTK 提供；安装后桌面会话可能需要重新加载图标主题。

插件运行在未沙箱化的 Omarchy shell 中，具有当前用户权限；请先审阅源码及仓库内的 x86_64 预编译二进制。安装并启用：

如果从旧 ID `omaclipsync` 升级，请先禁用旧插件，避免两个客户端同时同步或写入历史：

```sh
omarchy plugin disable omaclipsync
```

也可以从源码打包，由用户自行备份、部署和启用：

```sh
./scripts/package.sh --use-bundled
```

打包命令会覆盖 `dist/` 内同版本的输出；请先备份需要保留的旧包。也可使用 Git 安装：

```sh
omarchy plugin add https://github.com/Infinifar/omaclipsync.git --enable
```

### 手动部署与回退插件安装包

仅在 服务端和其他客户端已准备好后执行。下面的命令由用户执行，会停止当前插件，将其目录移到用户数据目录中的备份位置，再加载新包；手动部署后的插件不再是 Git 管理的安装。配置、历史和密钥环位于插件目录之外，需按自己的备份策略另行保护。把 `PACKAGE` 改为已验证的插件安装包绝对路径：

```sh
(
  set -eu
  plugin_id='io.github.infinifar.omaclipsync'
  load_root="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/plugins"
  backup_root="${XDG_DATA_HOME:-$HOME/.local/share}/omaclipsync-backups"
  package='/absolute/path/to/omaclipsync-plugin.tar.gz'
  test -f "$package" && test -d "$load_root/$plugin_id"
  mkdir -p "$backup_root"
  backup=$(mktemp -d "$backup_root/$plugin_id.XXXXXXXX")
  printf '旧插件将备份到：%s/previous\n' "$backup"
  omarchy plugin disable "$plugin_id"
  mv "$load_root/$plugin_id" "$backup/previous"
  mkdir -p "$load_root/$plugin_id"
  tar -xzf "$package" -C "$load_root/$plugin_id"
  omarchy plugin validate "$load_root/$plugin_id"
  omarchy-shell shell rescanPlugins
  omarchy plugin enable "$plugin_id"
)
```

请记下输出的备份路径。如果验证或启用失败，保留该备份并检查错误；不要重复移动备份。回退时先禁用插件，将当前插件目录移到一个新的备份目录，然后把记下的 `previous` 目录移回原加载位置，重新运行 `omarchy-shell shell rescanPlugins` 和 `omarchy plugin enable io.github.infinifar.omaclipsync`。服务端也必须回退为与旧插件兼容的镜像及数据库备份。

插件 ID 为 `io.github.infinifar.omaclipsync`。安装源包含 `bin/syncclipd` 和 `bin/syncclipctl`；启用后会运行守护进程，安装时不另行下载二进制或启动第二个 Quickshell。

升级时配置、历史、日志和密钥环项仍使用原有的 `omaclipsync` 位置和标识。若快捷键使用了旧 IPC target，请手动改为 `io.github.infinifar.omaclipsync`。

## 配置和使用

1. 在顶栏打开 Omaclipsync，进入 **设置**，填写 服务端的 HTTPS 地址、用户名和密码并保存。URL 必须指向服务根地址，包括部署时配置的路径前缀（如有）。
2. 密码保存到系统 Secret Service 密钥环，不写入配置文件；留空会保留当前账户已有密码。密钥环锁定时请先解锁。
3. 首次配对从服务端当前版本开始，不覆盖旧的本地内容；启动时已有的本机剪贴板也不会自动推送。离线期间只追上最新内容，本地待发送内容与远端冲突时不会静默覆盖；确认后点击 **推送** 可选择当前本地剪贴板。面板也提供 **接收** 和 **暂停**。
4. 在设置中按需启用历史和过滤。选择历史条目会改变系统剪贴板，开启同步时可能被推送到服务器。粘贴会向打开历史前的应用发送 Shift+Insert；如果原窗口不可用，则仅复制并提示。

主面板和历史面板可以从顶栏打开，也可运行以下命令；快捷键由用户自行配置，插件不会自动修改键位。在历史面板中点击 **返回** 可进入主面板：

```sh
omarchy shell io.github.infinifar.omaclipsync open
omarchy shell io.github.infinifar.omaclipsync openHistory
```

例如，在适用的 Omarchy Hyprland Lua 绑定配置中，可先检查快捷键冲突，再添加以下历史面板快捷键：

```lua
o.bind("SUPER + ALT + H", "Omaclipsync history", "omarchy shell io.github.infinifar.omaclipsync openHistory")
```

如果希望使用 `Super + Alt + C` 循环切换自动同步方向，请先检查快捷键冲突，再自行添加：

```lua
o.bind("SUPER + ALT + C", "Cycle Omaclipsync direction", "omarchy shell io.github.infinifar.omaclipsync cycleDirection")
```

快捷键按单向推送 → 单向接收 → 双向同步循环，并在屏幕下方的 Omarchy OSD 中显示保存后的方向。自动同步暂停时，OSD 也会显示暂停状态。插件不会自动添加或修改快捷键。

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
omarchy shell io.github.infinifar.omaclipsync toggle
```

`queued` 表示命令已排队，不代表同步已经成功。`bin/syncclipctl` 也可连接现有 shell，支持 `status`、`push`、`pull`、`toggle`、`watch` 和 `config`。插件不提供重复写入已处理消息的强制接收。

## 权限、网络与数据

插件以当前用户权限读取和写入普通 Wayland 剪贴板，向**用户配置的 HTTPS 服务**发送或接收文本。默认校验证书，只有在设置中显式开启 `insecure` 才会关闭校验；请求不跟随重定向。通知通过最多 25 秒的 HTTPS 长轮询实现，断线后按退避间隔重试。

同步内容上限为 10 MiB，元数据上限为 16 KiB。自动和手动推送以及接收默认跳过密码管理器标记的敏感内容；开启后接收时保留敏感标记，并可能在启用本地历史时以明文保存。检测依赖复制来源提供敏感标记，无法识别未标记的秘密内容。更改开关时会丢弃等待处理的剪贴板事件并重启监听。插件无法使用原生监听时会暂停同步，避免无法识别自身写入造成循环。

开启敏感内容同步后，接收内容会写入系统 Wayland 剪贴板，并附带密码管理器敏感标记。支持该标记的系统剪贴板历史管理器可能不显示或保存它；这与系统剪贴板本身是不同的功能。插件自带历史默认关闭，开启后可能明文保存敏感内容。实际显示和粘贴行为取决于桌面环境和应用。

| 数据 | 默认位置 | 说明 |
| --- | --- | --- |
| 配置 | `~/.config/omaclipsync/config.json` | 文件权限 `0600`，不包含密码 |
| 密码 | 当前用户的 Secret Service 密钥环 | 按服务器用户名保存 |
| 设备令牌和待发送内容 | `~/.local/state/omaclipsync/v1-*.state` | AES-GCM 加密，密钥存于 Secret Service；不包含管理员密码 |
| 本地历史 | `~/.local/share/omaclipsync/history.json` | 默认关闭；启用后明文保存，权限 `0600`，默认 50 条、最多 1000 条，文件上限 16 MiB |
| 日志 | `~/.local/state/omaclipsync/` | JSONL，权限 `0600`；记录固定事件，不记录密码和正文，按 UTC 日期保留 7 天 |

设置了 `XDG_CONFIG_HOME`、`XDG_DATA_HOME` 或 `XDG_STATE_HOME` 时，相应路径以这些目录为准。关闭历史后不会读取或写入历史文件，但已有文件不会自动删除。历史收藏不计入普通历史条数上限；所有记录仍受 16 MiB 文件上限约束。

## 卸载与回退

```sh
omarchy plugin disable io.github.infinifar.omaclipsync
omarchy plugin remove io.github.infinifar.omaclipsync
```

移除插件不会自动删除配置、历史、日志或密钥环项，请按需自行清理。手动复制安装的目录在某些 Omarchy 版本中可能被移至隐藏备份；请留意命令输出。重新启用旧 `omaclipsync` 插件前，请先禁用新插件，避免两个客户端同时运行。回退到 v0.1.1 前应备份历史文件并取消全部收藏：该版本无法读取包含收藏字段的历史。

回退到旧版本时，请先备份配置，并在启动旧版服务前从 JSON 配置中移除旧版不支持的字段（例如 `sync.allowSensitive`、`sync.direction` 或 `sync.notifyOnSuccess`）；旧版会拒绝未知配置字段。

## 从源码构建

需要 Rust、Cargo、Clippy、`jq` 和 GNU `tar`；集成测试还需要 Python 3 和 OpenSSL。在源码目录运行：

在已安装 `qmllint` 的 Omarchy 机器上，可运行 `./scripts/qmllint.sh` 做 QML 静态检查。脚本通过临时目录解析已安装的 shell 模块，遇到警告时返回失败，不修改已安装的 shell 或插件目录。非标准安装可设置 `OMARCHY_PATH`；可通过 `QMLLINT` 指定可执行文件。静态检查通过不能代替桌面测试。

```sh
cargo test --manifest-path core/Cargo.toml --locked
cargo clippy --manifest-path core/Cargo.toml --locked --all-targets -- -D warnings
cargo build --manifest-path core/Cargo.toml --locked --bin syncclipd
# 在隔离 Wayland 合成器中设置 RCS_TEST_DISPOSABLE_WAYLAND=1 后，可执行：
python3 core/tests/v1_e2e.py "$PWD/core/target/debug/syncclipd" /path/to/rustclipsync-server
./scripts/package.sh
omarchy plugin validate "$PWD"
```

打包输出位于 `dist/`；测试使用隔离的本地服务。更多验证和已知限制见 [验收记录](ACCEPTANCE.md)。

## 许可证

[MIT](LICENSE)。Rust 依赖许可证与声明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。原生剪贴板监听使用 [wl-clipboard-rs](https://docs.rs/wl-clipboard-rs/0.9.5/wl_clipboard_rs/watch/index.html)。
