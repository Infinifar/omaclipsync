# 本地验证记录

日期：2026-10-31。环境：Linux x86_64，系统 Rust/Cargo 1.99.1。

| 检查 | 结果 |
| --- | --- |
| 共享库 `cargo test --locked` | 20 项通过：4 项模块测试、15 项会话行为测试、1 项旧状态迁移测试 |
| 共享库 `cargo clippy --locked --all-targets -- -D warnings` | 通过，包含协议探针示例 |
| Rust 格式检查 | 通过 |
| 共享库 `cargo package --locked --allow-dirty --offline` | 打包与解包编译验证通过；未发布至 registry |
| Omaclipsync 测试 | 53 项通过：库 40、daemon 8、CLI 3、daemon 协议 2 |
| Omaclipsync Clippy | `--locked --all-targets -- -D warnings` 通过 |
| Omaclipsync release 构建及插件打包 | 通过；仅生成本地构建产物，没有替换已安装插件 |
| 插件源码包独立性 | 在临时目录解包，离线 `cargo check --locked --all-targets` 通过，无需相邻共享项目 |
| 导出源码一致性 | `export_source.py --check` 比较全部文件及 SHA-256 |
| 真实 HTTPS 协议集成 | 对临时 rustclipsync-server 1.0.0 二进制运行，注册、最新值、幂等重试、清空/重启、回环、敏感策略和冲突均通过 |

迁移样本由原 Omaclipsync 0.2.0 的未修改加密代码生成，验证已尝试上传和后续离线最后一条、UTF-8、敏感标记、token 与 cursor 的保留。样本只包含合成数据，未读取已安装插件的密钥或状态。

真实 HTTPS 测试使用临时 SQLCipher 数据库、固定测试账户、临时自签名证书和 loopback TLS 代理；自签名验证例外只用于这个隔离测试。测试中的两个客户端使用内存剪贴板和内存密文存储，不访问真实桌面或线上服务。

## GitHub 原生验证

共享核心在 [GitHub CI 41190778425](https://github.com/Infinifar/rustclipsync-core/actions/runs/41190778425) 的 Ubuntu 与 Windows x64 MSVC 作业均通过，验证提交 `d1a7cb341baa6d0fe9065805d5802261aa0258fd`，工具链 Rust 1.99.1。每个平台执行 20 项测试、格式检查和 Clippy。Windows 测试包含旧加密状态迁移与完整会话行为测试。

## 尚未验证的部分

- 共享核心的 Windows 原生编译与测试已经通过；OS 剪贴板、DPAPI、UI 等适配行为由 Windows 客户端独立验证，不能用共享库测试代替。
- 本轮真实 Wayland 剪贴板端到端测试：尝试启动隔离 Hyprland，但当前环境无法独立启动 headless backend；未改用用户的真实剪贴板。协议集成测试不能代替这项平台验证。
- Windows UI、DPAPI、Named Pipe、安装签名、内存/延迟/24 小时测试：属于桌面项目后续阶段，本轮没有实现或测量。

这次抽取保持现有协议和敏感行为，不包含新私有收件箱、账户密码短期注入、历史加密策略或端到端加密。这些能力的安排见相邻 `rustclipsync-destop` 的设计与实施计划。

## 0.2.0 本地验证

新增敏感收件项的持久化、重启不重放、skipped 确认、通知无正文、条件删除与保存失败不确认测试。`cargo test --locked` 共 22 项通过，旧 Omaclipsync 状态迁移测试继续通过。Windows 原生矩阵由本次提交的 CI 另行验证。

## 2026-11-01 同步边界修复验证

- 修复前，新增的 6 项核心回归测试均复现失败；修复后，共享库 `cargo test --locked --all-features` 共 39 项通过，包含原有 22 项测试及旧状态迁移样本。
- 新增覆盖策略变更后过滤待发内容、未决上传取消、慢请求回环抑制、写入结果不明时的持久化恢复、恢复保存失败、最终剪贴板读取失败、超大新复制淘汰旧内容、重启后直接同步、连接事件队列满时重试及停止。
- `cargo fmt --check`、`cargo clippy --locked --all-targets --all-features -- -D warnings`、`cargo doc --locked --no-deps` 和 `git diff --check` 通过。
- 用 `export_source.py` 将本次核心导出到客户端的临时副本：桌面 agent 的 `cargo test --locked` 为 24 项通过、5 项按原设置忽略；Omaclipsync 为 56 项通过。两者无需修改客户端 Rust 代码或依赖版本。
- 桌面 agent 的 `cargo check --locked --all-targets --features check-windows` 通过。这是在 Linux 上检查 Windows 适配代码，不代替 Windows 原生运行验证。
- 对现有服务器测试二进制运行 `tests/server_e2e.py` 通过：真实 HTTPS 注册、latest-only 同步、幂等重试、清空与重启、回环、敏感策略及冲突行为均通过。

公开方法签名、错误枚举、HTTP 协议及 RCS1 持久化格式保持兼容。客户端验证使用临时源码副本，未替换实际客户端的 vendor 快照；本轮未运行真实桌面剪贴板、密钥环或 Windows 原生集成测试。
