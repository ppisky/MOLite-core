# MOLite Core

[English](README.md) | 简体中文

Rust 实现的场景记忆角色对话内核。DMW-only MO State 包含五维投影、快照、来源修复及轮次驱动维护。只有原生 MOC；没有个人 Space、角色记忆池、NSG、向量数据库、外部角色卡或图片载体。

## 运行

```powershell
cargo run --release --bin molite-server
```

默认 `127.0.0.1:3791`，数据目录 `data/`。可设置 `MOLITE_DATA`、`MOLITE_LISTEN`、`MOLITE_HTTP_TOKEN`；非回环监听必须配置 token。首次按服务商→模型→模型参数组→用途选择→场景→会话的顺序配置，见 [API 使用说明](docs/api.zh-CN.md)。

## 已接入

- 服务商、文本/视觉模型、可复用模型参数组；三个用途分别选组，组指定协议、流式偏好、超时和参数。
- Chat Completions、Responses、Anthropic Messages；流式增量、完成校验、取消、冻结请求重试。
- 场景、会话、多角色、文件夹/标签、明确选择的场景注入范围。
- 默认12轮提炼，支持1–200轮和提前执行；提前成功后重新累计未处理轮次，失败保留原批次。
- 编辑、删除、模型重生立即禁用关联记忆并修复；无关记忆保留，修复不增加轮次。启动重建索引，在 Tokio 环境中每30秒恢复维护。
- DMW 词元/中文二元词、别名、标签、有界一跳检索；五维快照、叙事状态、保护核心事实和承诺的轮次生命周期。
- 图片直传或视觉回退，UTF-8文本附件注入，任意类型数据文件存储及完整性校验。
- 原生 MOC v3、多角色、MOLite 场景/会话/DMW/来源/计数往返、可选文件、原版密码封装。
- Rust API、HTTP/SSE、标准 C ABI；Windows x64 DLL 已通过真实 C 程序调用验证。

## 兼容与运行边界

- PDF 可以存储、读取及随 MOC 携带；模型输入目前处理文本和PNG/JPEG/WebP/GIF，不包含 PDF 内容解析。
- 完整运行数据使用明确声明的 `molite-runtime` MOC 扩展。原版 MOMO MOC 的角色可导入；旧 Space 会话/记忆树自动迁移尚未实现，返回 `skipped_modules` 明确列出。详见 [MOC 说明](docs/moc.zh-CN.md)。
- 使用固定默认投影规则，不含可编辑 YAML 状态契约、DDM 个性档案或通用工具执行器。历史最多48轮，未接供应商专用 token 计数器。
- 一个 Engine 串行协调，网络调用不持有 SQLite 写事务；每个目录推荐只由一个 Engine 所有者使用。
- API v1 为本地初版契约。Linux/macOS 未实机验证；供应商联调使用本地模拟服务，没有调用付费模型。

## 构建与验证

```powershell
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
cargo build --locked --lib
cmd /c scripts\test-c-abi.cmd
```

Windows 输出位于 `target/release/`：`molite-server.exe`、`molite_core.dll`、`molite_core.dll.lib`、`molite_core.lib`。头文件见 [include/molite.h](include/molite.h)，用法见 [C ABI](docs/c-abi.zh-CN.md)。

发布统一使用 `opt-level="z"`、完整 LTO，保留 rustls / AWS-LC、HTTP/2 和系统代理支持。只启用所需依赖功能：zstd 不含历史格式解码和字典训练，tar 不处理文件系统扩展属性；网络 JSON/SSE 使用 UTF-8，不启用通用字符集转换。MOC 仍使用当前原生格式和标准 zstd 数据，保留压缩等级 9、快速解码及密码封装。

仓库的 [.cargo/config.toml](.cargo/config.toml) 裁掉 SQLite 的 FTS3/5、RTree、DBSTAT、Soundex、列来源元数据和动态扩展加载；保留 JSON、WAL、事务、约束、线程安全及查询优化。作为 Rust 依赖在其他工作区编译时，Cargo 不会继承依赖仓库的配置，需要将该 `LIBSQLITE3_FLAGS` 配置放入宿主工作区；已有同名环境变量也会覆盖仓库默认值。`tests/sqlite_profile.rs` 验证实际编译选项与关键数据库功能。

运行 `powershell -File scripts/measure-size.ps1` 可按当前唯一发布配置构建并记录体积；历史实验数据保存在 `docs/size-audit-*.json`，不是受支持的发布变体。

Windows x64 可运行 `powershell -File scripts/pack-upx.ps1`，下载固定版本 UPX 5.2.1（校验下载包 SHA-256），以 `--ultra-brute` 搜索最高压缩率。原始 `target/release/` 文件保留，压缩副本位于独立的 `target/upx-时间戳/`。脚本验证压缩完整性、PE 加载属性、DLL C ABI、加密 MOC/文件往返和 HTTP/SSE，并运行真实 C 宿主；需要 Python 3 和 MSVC Build Tools。验证失败则报错，不把副本视为已验证产物。此步骤适用于未签名产物，代码签名应在压缩后完成。

设计与实现差异以本 README、API 及 [实施记录](docs/implementation-plan.zh-CN.md) 为准；[设计草案](docs/design.zh-CN.md) 保留目标范围。复用来源和许可证见 [NOTICE](NOTICE)、[LICENSE](LICENSE)。公开仓库为 https://github.com/ppisky/MOLite-core 。英文入口、贡献与发布流程见 README.md 和 CONTRIBUTING.md。
