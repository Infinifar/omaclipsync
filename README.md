# Omaclipsync

Omarchy Quattro 的文本剪贴板同步插件。插件 ID 为 `omaclipsync`。**当前为开发版，已通过一次真实桌面和双向同步冒烟测试；长期稳定性仍待验收。**

A text-only SyncClipboard client and Omarchy Quattro bar plugin. This development build has passed a desktop and end-to-end synchronization smoke test; long-term acceptance is still outstanding.

## 功能与权限

插件在 Omarchy shell 中运行，并托管一个 `syncclipd` 进程；不会启动第二个 Quickshell。同步启用时，该进程使用 Wayland data-control 事件读取普通剪贴板中的 `text/plain;charset=utf-8`，并向配置的 HTTPS SyncClipboard 服务器发送或接收文本。原生监听不可用时尝试 `wl-paste --watch`；它同样需要合成器支持 wlroots data-control；若降级时无法读取初始快照，为避免覆盖服务器，首个数据事件可能被忽略。带密码管理器敏感标记的自动监听内容会被跳过。启动时的现有剪贴板不会自动上传；需要时可手动上传。

服务器使用 HTTP Basic 认证；密码通过 `secret-tool` 存入当前用户的 Secret Service 密钥环，不写入配置或日志。默认校验 TLS 证书；仅在设置中显式启用 `insecure` 时关闭校验。请求不跟随重定向，单次 HTTP 超时 10 秒，5xx 最多重试两次；断网重试间隔由 1 秒递增至 30 秒。默认 3 秒读取服务器元数据，可改为 1–30 秒，也可暂停同步。发送和接收的正文上限 10 MiB，元数据上限 256 KiB。长文本先传文件再更新元数据；已存在历史中的同一哈希会被复用。只处理 `Text`，不处理图片或文件。

正则使用 Rust `regex` 语法，默认关闭；可选黑名单、白名单和下载过滤。栏设置 schema 提供间隔、过滤开关和历史开关；通过该开关启用过滤时，若当前模式为关闭则使用黑名单，模式及规则可在面板中调整。规则无法编译时拒绝启用同步。远端成功写入的同一哈希默认在 300 秒内不会重复写入本地，`pullForce` 可绕过。为限制内存，窗口内最多保留 10000 个不同哈希；达到上限时新的下载会等待旧记录过期。历史默认关闭；开启后保存明文到本地 JSON，默认 50 条、最多 1000 条，总文件大小上限 16 MiB，超过上限时保留较新的记录。关闭历史后不读写历史文件；已存在的记录不会自动删除。日志仅包含固定事件和时间，不包含正文或密码，按 UTC 日期保留 7 天；可设置 `logging.level`，`syncclipd --verbose` 可启用调试级事件。

配置：`$XDG_CONFIG_HOME/omaclipsync/config.json`（未设置时 `~/.config/omaclipsync/config.json`，0600）。历史：`$XDG_DATA_HOME/omaclipsync/history.json`（默认 `~/.local/share/...`，0600）。日志：`$XDG_STATE_HOME/omaclipsync/`（默认 `~/.local/state/...`，0600）。配置变更会重新加载。日志、历史、配置和密钥环项不会随插件禁用而自动删除。

依赖：Linux、Wayland data-control、目标版本的 Omarchy Quattro / Quickshell、Secret Service 与 `secret-tool`；降级监听还需要 `wl-paste`；历史条目的自动粘贴需要 `wtype`。构建另需 Rust、Cargo、`jq` 和 GNU `tar`；CI 的界面设置模型测试使用 GJS。发布包目前只面向 x86_64。网络仅访问用户配置的 HTTPS 地址；插件无服务端、systemd 单元或 X11 依赖。

## 构建与验证

在插件源码目录执行：

```sh
cargo test --manifest-path core/Cargo.toml --locked
cargo clippy --manifest-path core/Cargo.toml --locked --all-targets -- -D warnings
gjs tests/settings.gjs Settings.js
cargo build --manifest-path core/Cargo.toml --locked --bin syncclipd
python3 core/tests/mocked_e2e.py "$PWD/core/target/debug/syncclipd"
python3 core/tests/mocked_e2e.py "$PWD/core/target/debug/syncclipd" --blacklist
python3 core/tests/auth_recovery.py "$PWD/core/target/debug/syncclipd"
python3 core/tests/offline_recovery.py "$PWD/core/target/debug/syncclipd"
./scripts/package.sh
omarchy plugin validate "$PWD"
# qs 是 Quickshell 的运行时别名；在临时目录提供同一导入映射供 qmllint 使用。
qml_imports=$(mktemp -d)
ln -s /usr/share/omarchy/shell "$qml_imports/qs"
/usr/lib/qt6/bin/qmllint -I "$qml_imports" --max-warnings 0 Service.qml BarWidget.qml Panel.qml
rm "$qml_imports/qs" && rmdir "$qml_imports"
```

打包脚本生成 `dist/syncclipd-linux-x86_64` 和 `dist/omaclipsync-plugin-0.1.1.tar.gz`，不修改桌面加载目录。`.gitlab-ci.yml` 定义格式、Clippy、测试和打包；版本标签触发发布脚本，使用 CI job token 上传制品并创建 GitLab Release。该仓库的实际流水线和发布尚未验证。Git 安装源包含 x86_64 Linux 的预编译 `bin/syncclipd` 和 `bin/syncclipctl`，源码位于 `core/`；可以按上述步骤自行重建并验证。插件加载时运行仓库内的守护进程，不会在安装时下载或执行其他代码。

协议参照 [SyncClipboard 上游源码](https://github.com/Jeric-X/SyncClipboard)，核对版本 `fd25fd62409967a69cec842cabaec2c00adf901d`；原生监听基于 [wl-clipboard-rs 0.9.5](https://docs.rs/wl-clipboard-rs/0.9.5/wl_clipboard_rs/watch/index.html)。

## 用户部署及回退

通过 GitHub 安装（请先审阅源码与预编译二进制；目标仅支持 x86_64 Linux）：

```sh
omarchy plugin add https://github.com/Infinifar/omaclipsync.git --enable
```

也可使用已构建的归档离线部署；复制前检查目标，已有目录应先备份，不要覆盖。归档路径按实际构建结果调整：

```sh
plugin_dir="$HOME/.config/omarchy/plugins/omaclipsync"
archive="$PWD/dist/omaclipsync-plugin-0.1.1.tar.gz"
if [ -e "$plugin_dir" ] || [ -L "$plugin_dir" ]; then
  printf '%s\n' '目标已存在，请先备份并选择其他处理方式。' >&2
else
  mkdir -p "$plugin_dir" &&
    tar -xzf "$archive" -C "$plugin_dir" --no-same-owner &&
    omarchy plugin validate "$plugin_dir" &&
    omarchy shell shell rescanPlugins &&
    omarchy plugin enable omaclipsync --section right
fi
```

启用后，在面板设置服务器地址、用户名和密码，保存配置。密码留空会保留该账号已有的密钥环密码。密钥环锁定时需要先解锁。需要快捷键时，可使用以下已核对的本机命令；调用返回 `queued` 表示已排队，不代表同步成功。历史面板可通过 `omarchy shell omaclipsync openHistory` 打开；本机验收时配置了 `Super + Alt + V`，其他机器可按需绑定此命令；每条可复制或粘贴，粘贴使用 `wtype` 向打开面板前的应用发送 Shift+Insert，原窗口不可用时只复制并提示。选择历史会改变系统剪贴板；同步开启时可能上传到服务器。每页显示 20 条，本地历史默认关闭，开启后以明文保存。归档还提供 `bin/syncclipctl`，支持 `status`、`push`、`pull [--force]`、`toggle`、`watch` 和 `config`；它连接现有 shell，不另启守护进程。也可直接使用：

```sh
omarchy shell omaclipsync openHistory
omarchy shell omaclipsync push
omarchy shell omaclipsync pull
omarchy shell omaclipsync pullForce
omarchy shell omaclipsync toggle
omarchy shell omaclipsync status
```

暂停可通过面板或 `toggle` 完成。用户可运行 `omarchy plugin disable omaclipsync` 回退，再按需运行 `omarchy plugin remove omaclipsync`；本机对非 Git 安装目录会将其移至插件目录下的隐藏备份。配置、历史、日志和密钥环项独立保存，按需由用户自行清理。

[逐项需求核对](ACCEPTANCE.md)记录了已完成与待验收项目。

## 未完成的验收

已通过本地模拟测试、真实服务器认证及 10 万字符服务端往返测试，并在现有 Omarchy shell 中验证了面板显示、真实 Wayland 剪贴板上传和下载。测试后恢复了原服务端内容；合成测试文本可能保留在服务器历史中。模拟测试中，隔离的本地 HTTPS 端点断线 60 秒后恢复，排队的复制事件成功上传；未断开本机网络。在真实桌面上，打开的面板可实时切换中英文及日期格式。实机守护进程 60 秒空闲采样的平均 CPU 为 0.033%，RSS 约 7.02 MiB；隔离测试读取 500 条、每条约 1 KB 的合成历史时，RSS 峰值为 5.92 MiB。历史视图功能更新前的版本在 10 分钟实机采样中，RSS 从 7.02 MiB 到 7.02 MiB，平均 CPU 为 0.03%；本次没有进行 24 小时采样。多屏、主题切换、RTL、完整的同步处理耗时和长期稳定性仍待系统验收。

## License

MIT. See [LICENSE](LICENSE). Bundled Rust dependency licenses and notices are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## English

The daemon syncs only UTF-8 text with a user-configured HTTPS SyncClipboard server. It uses Secret Service for the password, optional local history, regex filtering, and a configurable deduplication window. Automatic monitoring skips password-manager-sensitive selections. Install with `omarchy plugin add https://github.com/Infinifar/omaclipsync.git --enable`; see the build and rollback commands above. The repository includes x86_64 Linux binaries and their Rust source. Desktop display and real clipboard upload/download have passed a smoke test; extended reliability and resource acceptance are pending.
