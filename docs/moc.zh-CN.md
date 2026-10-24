# MOC 原生适配

[English](moc.md)

直接复用原版 MOC v3 与密码封装：tar+zstd、manifest.toml、模块路径/摘要校验、安全解压、AES-256-GCM/Argon2id。没有图片载体或其他角色卡解码。

角色使用 characters/spaces/00000000-0000-0000-0000-000000000001/<uuid>/character.toml 和 Markdown。归档 UUID 仅为线格式命名空间，不建立个人空间或记忆池。可同时携带多个角色。

完整 MOLite 运行数据使用显式扩展模块：id=molite-runtime，path=molite，dependencies=[characters]，import_order=60。molite/state.json 的 version=2，包含 files_included 和白名单业务表：资源、对话、交换、DMW、来源、计数、批次。附件在 molite/files/<sha256>。提供商、模型配置、提示词覆盖、凭据及本地请求缓存不携带。

导入先写暂存库并校验，再以目标库事务提交；身份冲突整笔拒绝，本地序号重新分配。include_files=false 保留元数据和引用，返回 files_included:false；未恢复的附件读取会明确失败。内容文件先写入，数据库回滚可能留下无引用文件，不声称文件系统与SQLite属于同一物理事务。

密码版沿用 momo-envelope-v1、private/payload.enc、momo-private-moc-v1 associated data。导入限制10,000条目、512MiB解压数据，密码版导出内层最多512MiB，口令不保存。

兼容边界：原版 MOMO MOC 角色可导入；旧 Space 对话/DMW 树尚未自动迁移为 MOLite 对话与来源，返回 skipped_modules。MOLite 全量业务数据在本版之间往返。NSG始终不激活。原版宿主是否读取 molite-runtime 取决于其扩展能力，不宣称完整双向业务兼容。
