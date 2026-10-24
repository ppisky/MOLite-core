# API v1

Send JSON with `Content-Type: application/json` to `POST /v1/operations`. For SSE, send the same JSON to `POST /v1/stream`. If a token is configured, add `Authorization: Bearer <token>`. Rust uses `Engine::execute` / `Engine::chat`; C uses `molite_submit`. An omitted `version` means 1; an explicit version must be 1.

## Configure and start a conversation

Send these requests in order. Replace the provider URL, model name and credential environment variable. `base_url` is the API prefix, not the complete completion endpoint.

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
{"op":"conversation.put","value":{"id":"c","title":"First conversation","character_ids":[]}}
```
```json
{"op":"chat","request":{"request_id":"unique-1","conversation_id":"c","text":"Hello","attachments":[]}}
```

For characters, `character.put` requires `value` with a UUID `id`, `name`, SemVer `version`, `author_name` and `character_markdown`. Optional fields are `user_markdown`, `opening_markdown` and `classification`. Add the ID to the conversation's `character_ids`. Characters do not implicitly share memory across conversations.

For vision fallback, configure a model with `vision:true`, create its parameter group, and select that group in `vision_fallback`. A conversation model marked `vision:true` receives original images directly.

When `stream` is omitted, conversation generation streams by default and memory/vision requests do not. Explicit values apply to every purpose. `parameters` contains native provider fields, such as Responses `max_output_tokens` or Anthropic `max_tokens`; incompatible fields are not guessed or converted. Control fields such as model/messages/input/system/stream/tools cannot be overridden. `timeout_seconds` defaults to 120 and accepts 1–600.

## Operations

| Operation | Main input | Behavior |
|---|---|---|
| provider.put / model.put / group.put | value | Validate references and save configuration |
| purposes.set | value | Select groups for purposes |
| character.put | value | Save resources |
| conversation.put | value, optional revision | Save narrative/scope with revision checks |
| conversation.delete | id, revision | Delete; reject if another scope references it |
| resource.list / resource.get | kind; get also needs id | Read resources |
| resource.classify | kind, id, value:{folder,tags} | Classify without changing memory scope |
| chat | request:{request_id,conversation_id,text,attachments} | Generate, commit and maintain memory |
| history | conversation_id | Effective exchanges and revisions |
| exchange.revise | request_id, revision; optional user/assistant/delete | Edit/delete and repair memory |
| exchange.regenerate | request_id, revision | Regenerate without adding a round |
| extraction.configure | conversation_id, every_turns | Set subsequent batch interval, 1–200 |
| extraction.status | conversation_id | Queue, batches and repair status |
| distill | conversation_id, early | Run early with early=true |
| recover | none | Resume maintenance without generation |
| memory.list | conversation_id | Documents and validity/archive states |
| snapshot | conversation_id, query | Five-dimensional state and context |
| events | after (default 0) | Up to 1,000 events; continue using last sequence |
| file.add | name, mime, base64 | Save a data file |
| file.read / file.delete | id | Read or delete an unreferenced file |
| moc.export | path, include_files; optional passphrase | Export without overwriting an existing path |
| moc.import | path; optional passphrase | Preflight then atomic database import; reject conflicts |

Resource kinds: conversation, character, file, provider, model, group. An empty `injection_scope` selects the conversation itself; a nonempty scope selects only listed conversations. `narrative` contains location/time/participants/focus/open_threads/constraints. `ended:true` stops new generation. A conversation directly owns messages, narrative state and memory. Create it with one request; use explicit injection scope for continuity.

## Streaming, retries and maintenance

Events include `text.delta`, `vision.completed`, `exchange.committed`, `memory.distilled`, `memory.repaired` and `memory.retry_pending`. Terminal events are `completed` / `failed`, plus `cancelled` in C. Disconnecting SSE cancels unfinished work. A committed exchange is retained if later memory extraction fails.

`request_id` is unique across the database. Retrying identical content returns the committed result; prepared input and model configuration are frozen. Conversation revision changes require a new request ID. A crash after upstream acceptance but before local commit can cause duplicate upstream billing on retry.

Extraction defaults to 12 rounds. Successful early extraction restarts counting unprocessed rounds. Failures retain the batch; interval changes do not rewrite an active batch. Repairs do not add rounds. Opening Engine inside Tokio starts maintenance recovery every 30 seconds; other hosts can call `recover` explicitly.

## Limits and files

A stored file is limited to 64 MiB; new-chat attachments total at most 16 MiB, with at most 16 attachments. Text input and each text attachment are limited to 200,000 bytes. Responses are limited to 8 MiB. History has at most 48 rounds; image history is further reduced to a 24 MiB input budget. DMW body budget is 16,000 bytes and at most 25 records. There is no provider-specific token counter.

Input parsing supports UTF-8 `text/*` and PNG/JPEG/WebP/GIF. PDFs can be stored, read and carried in MOC; they are not parsed into model input. Credentials store only environment-variable names; values never enter the business database or MOC. File IDs reference immutable content. Files referenced by messages cannot be deleted directly.

## Direct conversations and prompt spaces

Create a conversation with one `conversation.put` request containing `id` and `title`. Optional fields are `character_ids`, `injection_scope` (conversation IDs), `narrative`, `ended`, and `classification`. Updates replace the complete resource and can supply the current runtime `revision` for concurrency control. No separate scene resource or `scene_id` is accepted.

The four fixed prompt spaces are `assistant`, `roleplay_director`, `memory_distillation`, and `vision_fallback`. Assistant and director content enter the same conversation request; the director does not add another model purpose or call. Each resource reports `content`, `source` (builtin/override), and a SHA-256 `revision`. Overrides persist locally and are excluded from MOC; they are unrelated to personal Spaces or memory scope.

```json
{"op":"prompt_spaces.replace","id":"assistant","content":"Continue the conversation according to the character."}
```

Use `prompt_spaces.list`, `prompt_spaces.get` (id), `prompt_spaces.replace` (id, content, optional revision), and `prompt_spaces.reset` (id, optional revision). Replacement content must be nonblank and at most 256 KiB of UTF-8. Reset removes the override. Unsupported prompt IDs, including NSG/DDM prompts, are rejected. Newly prepared requests use the new content; captured generation and maintenance retries retain their original messages.

HTTP also exposes `GET /v1/prompt-spaces` and `GET/PUT/DELETE /v1/prompt-spaces/{id}` with the same token checks. PUT accepts `{ "content": "...", "revision": "optional" }`. Rust callers use `Store::prompt_spaces/prompt_space/replace_prompt_space/reset_prompt_space`; C uses the same operation JSON.
