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

## 尚未验证的部分

- Windows 原生编译和 OS 剪贴板行为：本机未安装 Windows 编译工具链，未宣称通过。已加入 Linux/Windows CI 工作流，需发布仓库后在 runner 上执行。
- 本轮真实 Wayland 剪贴板端到端测试：尝试启动隔离 Hyprland，但当前环境无法独立启动 headless backend；未改用用户的真实剪贴板。协议集成测试不能代替这项平台验证。
- Windows UI、DPAPI、Named Pipe、安装签名、内存/延迟/24 小时测试：属于桌面项目后续阶段，本轮没有实现或测量。

这次抽取保持现有协议和敏感行为，不包含新私有收件箱、账户密码短期注入、历史加密策略或端到端加密。这些能力的安排见相邻 `rustclipsync-destop` 的设计与实施计划。
