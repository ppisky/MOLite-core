# MOLite Core

MOLite Core 已开始实现，基于 MOMO Core 的 MO State v2 与 DMW 设计，面向轻量、快速、配置简单的文本角色对话。

核心目标：**让文本对话持续保有记忆与状态，并自动维护。**

已确定的范围：

- 保留 MO State v2 与 DMW。
- 只使用 MOC 承载原生角色格式，不支持其他角色卡格式兼容或 LSB 载体。
- 没有个人空间、角色级记忆或共享记忆体系；场景与会话的最终命名、映射尚未固定。
- 记忆只按用户明确选择的场景注入范围使用；文件夹、标签仅负责分类。
- 服务商下先配置文本、视觉模型；模型参数组定义模型、请求方式和请求参数，由对话、记忆、视觉回退分别选择。
- 模型接口保留 OpenAI 兼容 Chat Completions、OpenAI Responses、Anthropic Messages。
- 自动提炼默认每 12 个完整问答一批，允许设置轮数和提前提炼；提前成功后按剩余未处理轮次重新计算节奏。编辑、重生、删除后的来源修复仍在计划中。
- 支持图片输入；对话模型具备视觉能力时可直接接收，否则使用视觉回退转为文本。
- TXT、PNG、PDF 等作为数据文件存储；MOC 沿用原生携带能力，文件默认可携带，也可选择排除。
- Rust API、HTTP、标准 C ABI 三种入口都保留。
- 不支持 NSG 或 Turso 向量存储。

图片可以作为输入与数据文件，不支持把 MOC 或角色卡编码进图片载体。

当前已实现独立 Rust 工程和 SQLite 持久化提炼调度器：默认 12 轮、1–200 轮配置、提前批次、原批次恢复、请求幂等及成功确认。它尚未调用记忆模型，也未执行 DMW Patch；完整 MO State、模型适配、MOC、数据文件、来源修复、HTTP 和 C ABI 仍待实现。上述范围列表表示产品目标，不是完成清单。

验证：`cargo test --locked`、`cargo clippy --locked --all-targets -- -D warnings`。
运行调度示例：`cargo run --example early_distillation`（不调用模型）。

具体边界见 [设计草案](docs/design.zh-CN.md)，C ABI 见 [接口设计](docs/c-abi.zh-CN.md)。
