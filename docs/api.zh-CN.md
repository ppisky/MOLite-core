# API v1

[English](api.md)

HTTP `POST /v1/operations`，Content-Type 为 application/json；SSE 用相同请求发送到 `POST /v1/stream`。配置了 HTTP token 时带 `Authorization: Bearer ...`。Rust 使用 `Engine::execute` 或 `Engine::chat`，C 使用 `molite_submit`。`version` 可省略，显式提供必须为1。

## 初始配置示例

依次发送以下 JSON；替换地址、模型名及凭据环境变量。base_url 是 API 前缀而不是完整 completion URL。

```json
{"op":"provider.put","value":{"id":"p","base_url":"https://example.com/v1","api_key_env":"MODEL_API_KEY","protocols":["chat_completions","responses","anthropic"]}}
```
```json
{"op":"model.put","value":{"id":"text","provider_id":"p","model":"your-model","vision":false}}
```
```json
{"op":"group.put","value":{"id":"chat","model_id":"text","protocol":"chat_completions","stream":true,"parameters":{"temperature":0.7}}}
```
```json
{"op":"group.put","value":{"id":"memory","model_id":"text","protocol":"chat_completions","stream":false,"parameters":{"temperature":0.1}}}
```
```json
{"op":"purposes.set","value":{"conversation":"chat","memory":"memory","vision_fallback":null}}
```
```json
{"op":"scene.put","value":{"id":"s","title":"初始场景"}}
```
```json
{"op":"conversation.put","value":{"id":"c","scene_id":"s","title":"初始对话","character_ids":[]}}
```
```json
{"op":"chat","request":{"request_id":"unique-1","conversation_id":"c","text":"你好","attachments":[]}}
```

角色用 character.put，value 必含 UUID id、name、SemVer version、author_name、character_markdown，可附 user_markdown、opening_markdown、classification。随后把 ID 加入会话 character_ids。同一角色不自动共享场景记忆。

视觉回退配置一个 vision:true 的模型及参数组，再填写 vision_fallback。对话模型自身 vision:true 时直接接收原图。

参数组的 stream 省略时对话默认流式、记忆/视觉默认非流式；显式值在所有用途保留。parameters 是接口原生参数，例如 Responses 的 max_output_tokens、Anthropic 的 max_tokens，不自动猜测或转换不兼容字段。不能覆盖 model/messages/input/system/stream/tools 等控制字段。timeout_seconds 默认120，范围1–600。

## 操作表

| op | 主要输入 | 行为 |
|---|---|---|
| provider.put / model.put / group.put | value | 校验引用并保存配置 |
| purposes.set | value | 用途选择 |
| character.put / conversation.put | value | 保存资源 |
| scene.put | value、可选revision | 叙事、范围及版本检查 |
| scene.delete | id、revision | 删除场景，其他范围仍引用时拒绝 |
| resource.list / resource.get | kind、get另需id | 读取资源 |
| resource.classify | kind、id、value:{folder,tags} | 分类，不修改范围 |
| chat | request:{request_id,conversation_id,text,attachments} | 一轮生成、提交、到期维护 |
| history | conversation_id | 当前有效交换及revision |
| exchange.revise | request_id、revision，可选user/assistant/delete | 修改或删除，自动修复 |
| exchange.regenerate | request_id、revision | 模型重生，不增加轮次 |
| extraction.configure | scene_id、every_turns | 后续批次间隔1–200 |
| extraction.status | scene_id | 队列、批次及修复状态 |
| distill | scene_id、early | early=true提前执行 |
| recover | 无 | 恢复维护，不生成对话 |
| memory.list | scene_id | 文档及有效/归档状态 |
| snapshot | scene_id、query | 五维状态及上下文 |
| events | after（默认0） | 最多1000条事件，用末尾sequence继续 |
| file.add | name、mime、base64 | 保存文件 |
| file.read / file.delete | id | 读取，或删除未被引用资源 |
| moc.export | path、include_files、可选passphrase | 不覆盖已有路径 |
| moc.import | path、可选passphrase | 预检后导入，冲突整笔拒绝 |

resource kind 为 scene、conversation、character、file、provider、model、group。injection_scope 为空时使用自身场景，非空时只使用所列场景。narrative 包含 location/time/participants/focus/open_threads/constraints；ended:true 关闭新生成。已有消息的会话不能改挂其他场景，延续通过显式范围表达。

## 提交、重试与限制

流式事件含 text.delta、vision.completed、exchange.committed、memory.distilled、memory.repaired、memory.retry_pending；终止为 completed/failed，C另有cancelled。SSE断开会取消未完成操作；已提交交换不会因后续记忆模型失败丢失。

request_id 全库唯一。相同内容重试返回已提交结果；准备后的输入与模型配置冻结。场景版本变化时需新request_id。上游已接受而本地未落盘的故障窗口不能保证不重复计费。

默认12轮，提前成功后重新累计未处理轮次。失败保留批次，改间隔不改活动批次；修复不消耗新轮次。Tokio环境打开Engine时自动启动30秒维护恢复；其他宿主可显式调用recover。

单文件存储64MiB；一次新对话输入的附件合计最多16MiB；文本输入/文本附件每项200,000字节；每次最多16附件；响应8MiB；历史最多48轮，含图片的历史再按24MiB输入大小缩减；DMW正文预算16,000字节、最多25条。不含供应商专用token计数器。附件解析支持UTF-8 text/*及PNG/JPEG/WebP/GIF；PDF仅存储/读取/携带。

凭据只保存环境变量名称，值不写入业务库或MOC。文件ID指向不可变内容；被消息引用的文件不能直接删除，避免破坏证据。
