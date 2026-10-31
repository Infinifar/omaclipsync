# rustclipsync-core

RustClipSync 客户端的共享 Rust 核心，从 Omaclipsync 0.2.0 的现有同步实现抽取。
Linux 和 Windows 应复用这里的消息状态机与 HTTPS 协议，分别实现操作系统适配层。
这是客户端库，不是服务端，也不是独立桌面应用。

## 已抽取的内容

| 模块 | 职责 |
| --- | --- |
| `client` / `transport` | HTTPS 注册、状态、内容上传下载、取消、确认、长轮询；传输接口允许离线测试 |
| `state` | UUID、epoch、revision、outbox、接收游标和应用日志；兼容已有 RCS1 AES-256-GCM 状态格式 |
| `session` | 离线仅最新值、固定消息 ID 重试、冲突处理、敏感内容开关、应用前二次检查和回环抑制 |
| `worker` | 元数据长轮询、通知合并和连接退避；不复制剪贴板正文到轮询线程 |
| `Filter` | 上传/下载正则过滤 |
| `adapters` | 系统剪贴板、原子状态存储和可选历史接口 |

没有 Wayland、WinUI、Windows API、Secret Service、托盘或文件路径依赖。构造会话会加载或创建加密状态，但不联网、不读取或写入剪贴板。宿主在专用工作线程串行调用同步方法；不要从 UI 线程调用阻塞网络操作。

当前库版本 `0.1.0` 只表示共享 Rust API 的初始版本，与服务端及客户端的正式产品版本独立。HTTP 路径沿用 `/v1/`，不代表另一个未发布产品。

## 适配边界

1. 用平台凭据库取得随机 32 字节密钥：Linux 使用 Secret Service，Windows 计划用 DPAPI CurrentUser 保护密钥。
2. 实现 `StateStore`：限长读取、仅当前用户可读、原子持久化替换，完整保存核心生成的字节。只有文件不存在才能返回 `None`；解密失败不得偷偷重建身份。
3. 实现 `Clipboard`：返回文本及敏感标记；写入必须携带消息来源标记。`write` 成功表示系统剪贴板已接受内容，单独写入私有收件箱不能伪报 `applied`。
4. 使用 `NoHistory` 或实现 `HistorySink`。核心不决定历史存储格式，也不自行记录剪贴板正文。
5. 使用 `SessionConfig` 和 `Session::open` 构造会话。监视器维护共享的本地事件计数；通过 `record_local` / `record_clear` 记录真实事件，通过 `flush` / `pull` 完成同步。

状态保存错误必须暂停并重建会话，不能继续用可能领先磁盘的内存状态。首次启动不能把原有系统剪贴板误当作新复制事件。通知只是唤醒信号，`state` 返回的 epoch 和 revision 才是同步依据。

## 协议和隐私约束

- 对接 [RustClipSync 服务端](https://github.com/Infinifar/rustclipsync-server) 及其 [接口协议](https://github.com/Infinifar/rustclipsync-server/blob/main/PROTOCOL-V1.md)，没有加入旧 SyncClipboard HTTP/SignalR 传输实现。
- UUID 标识一次复制事件；重试保留 UUID。revision 排序，epoch 隔离服务端重置；客户端时间仅用于重试有效期，不能用来排序或生成唯一性保证。
- 自动上传不静默覆盖冲突；离线只同步最新值。发送结果未知时保留原请求，先确认/取消后才处理新的最后一条。
- 已接收消息通过持久化游标与确认避免重放。崩溃时无法判断是否成功写入的消息恢复为 `skipped`，不能承诺系统剪贴板副作用严格 exactly-once。
- 敏感内容由宿主传入标记，默认应关闭同步；正则和标记都无法识别全部敏感信息。
- TLS 默认校验证书，拒绝明文 HTTP、URL 内嵌凭据和重定向。兼容现有的显式 `insecure` 选项只供受控测试/自签名环境使用。
- 状态加密保护磁盘文件，不是端到端加密；服务端仍可读取同步正文。内存中的文本、密钥和配对密码目前不保证即时清零。
- 单条文本最大 10 MiB；状态最大 32 MiB。长轮询线程退出最多需等待当前请求的 30 秒超时，宿主需避免反复创建重叠 worker。

## 开发和消费

```bash
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
```

已有服务端测试二进制时，可运行真实 HTTPS 协议验证：

```bash
python3 tests/server_e2e.py ../rustclipsync-server/target/release/rustclipsync-server
```

该脚本要求 Python 3 与 OpenSSL，启动临时服务端、数据库和仅监听 loopback 的 HTTPS 代理；剪贴板及密钥存储使用内存替身，不操作用户桌面。单元测试另包含由未修改的 Omaclipsync 0.2.0 代码生成的加密状态迁移样本。验证范围与平台限制见 [VALIDATION.md](VALIDATION.md)。

相邻仓库本地开发可以使用：

```toml
rustclipsync-core = { version = "=0.1.0", path = "../rustclipsync-core" }
```

源码仓库为 [Infinifar/rustclipsync-core](https://github.com/Infinifar/rustclipsync-core)，当前以私有仓库管理，尚未发布 crates.io 包。为了让 Omaclipsync 的独立检出和源码发布包仍可编译，它消费 `core/vendor/rustclipsync-core` 的自动导出快照：

```bash
python3 tools/export_source.py ../omarchy-plugins-dev/io.github.infinifar.omaclipsync/core/vendor/rustclipsync-core
python3 tools/export_source.py ../omarchy-plugins-dev/io.github.infinifar.omaclipsync/core/vendor/rustclipsync-core --check
```

导出清单包含来源版本、原始提取提交及各文件 SHA-256。不要直接编辑消费端快照；修改本仓库、运行测试、重新导出，再更新消费端 Cargo.lock。将来发布共享库后可统一切换到固定版本或 Git 提交，不依赖浮动主分支。

Windows 方案在相邻的 `rustclipsync-destop` 项目中；Android 现阶段继续使用 Kotlin 实现同一协议，不强行加入 JNI。

MIT 许可，原始实现来源：`Infinifar/omaclipsync`，提取基线提交 `a8c330136886ad547dfadf386bb66901b3bbc45c`。新增的跨平台接口、测试和导出工具在本项目维护。
