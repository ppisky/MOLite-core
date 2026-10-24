use crate::{
    Error, Result,
    domain::*,
    model::{Connection, Message},
    prompt_spaces::PromptSpaceId,
    store::Store,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatRequest {
    pub request_id: String,
    pub conversation_id: String,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Prepared {
    request: ChatRequest,
    connection: Connection,
    messages: Vec<Message>,
    revision: i64,
    #[serde(default)]
    scope_revisions: std::collections::BTreeMap<String, i64>,
    conversation_id: String,
    #[serde(default)]
    prompt_revisions: std::collections::BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct Engine {
    pub(crate) store: Arc<Mutex<Store>>,
    gate: Arc<tokio::sync::Mutex<()>>,
    client: reqwest::Client,
}
impl Engine {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let engine = Self {
            store: Arc::new(Mutex::new(Store::open(root)?)),
            gate: Arc::new(tokio::sync::Mutex::new(())),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| Error::Message(e.to_string()))?,
        };
        engine.lock()?.reconcile()?;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let weak = Arc::downgrade(&engine.store);
            let gate = Arc::downgrade(&engine.gate);
            let client = engine.client.clone();
            handle.spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    let (Some(store), Some(gate)) = (weak.upgrade(), gate.upgrade()) else {
                        break;
                    };
                    let engine = Engine {
                        store,
                        gate,
                        client: client.clone(),
                    };
                    let _ = engine.recover().await;
                }
            });
        }
        Ok(engine)
    }
    pub(crate) fn lock(&self) -> Result<MutexGuard<'_, Store>> {
        self.store
            .lock()
            .map_err(|_| Error::Message("store lock poisoned".into()))
    }
    fn conversation_prompt(s: &Store, c: &Conversation) -> Result<String> {
        let mut prompt = s.prompt_space(PromptSpaceId::Assistant)?.content;
        prompt.push('\n');
        prompt.push_str(&s.prompt_space(PromptSpaceId::RoleplayDirector)?.content);
        for id in &c.character_ids {
            let role: Character = s.get("character", id)?;
            prompt.push('\n');
            prompt.push_str(&role.character_markdown);
            prompt.push('\n');
            prompt.push_str(&role.user_markdown);
        }
        Ok(prompt)
    }
    fn connection(s: &Store, id: &str) -> Result<Connection> {
        let group: ModelGroup = s.get("group", id)?;
        let model: Model = s.get("model", &group.model_id)?;
        let provider: Provider = s.get("provider", &model.provider_id)?;
        Ok(Connection {
            provider,
            model,
            group,
        })
    }

    /// Resume persisted maintenance without generating conversation turns.
    /// Called by the automatic worker when opened inside a Tokio runtime.
    pub async fn recover(&self) -> Result<Value> {
        let _gate = self.gate.lock().await;
        let scenes = { self.lock()?.list("conversation")? };
        let mut results = vec![];
        for conversation in scenes {
            let id = conversation["id"]
                .as_str()
                .ok_or(Error::Invalid("invalid stored conversation"))?;
            match self.distill_inner(id, false, None).await {
                Ok(v) => results.push(json!({"conversation_id":id,"result":v})),
                Err(e) => {
                    results.push(json!({"conversation_id":id,"pending":true,"error":e.to_string()}))
                }
            }
        }
        Ok(json!(results))
    }

    pub async fn regenerate(
        &self,
        request_id: &str,
        revision: i64,
        events: Option<&UnboundedSender<Value>>,
    ) -> Result<Value> {
        let _gate = self.gate.lock().await;
        let (old, connection, mut messages, vision, vision_prompt) = {
            let s = self.lock()?;
            let body: String = s.scheduler.db.query_row(
                "SELECT body FROM exchanges WHERE request=?",
                [request_id],
                |r| r.get(0),
            )?;
            let old: Exchange = serde_json::from_str(&body)?;
            if old.deleted || old.revision != revision {
                return Err(Error::Conflict("message revision changed"));
            }
            let c: Conversation = s.get("conversation", &old.conversation_id)?;
            let p: Purposes = s.get("settings", "purposes")?;
            if c.ended {
                return Err(Error::Conflict("conversation ended"));
            }
            let prompt = Self::conversation_prompt(&s, &c)?;
            let vision_prompt = s.prompt_space(PromptSpaceId::VisionFallback)?.content;
            // Do not feed the response being regenerated, or any future turns,
            // back to the model as if they had already happened.
            let mut messages = vec![Message::text("system", prompt)];
            for e in s
                .history(&c.id)?
                .into_iter()
                .take_while(|e| e.request_id != request_id)
            {
                messages.push(Message::text("user", e.user));
                messages.push(Message::text("assistant", e.assistant));
            }
            let mut current = Message::text("user", &old.user);
            for id in &old.attachments {
                let (f, b) = s.read_file(id)?;
                if f.mime.starts_with("image/") {
                    current.images.push((f.mime, STANDARD.encode(b)));
                } else if f.mime.starts_with("text/") {
                    current.text.push_str(
                        &String::from_utf8(b)
                            .map_err(|_| Error::Invalid("invalid text attachment"))?,
                    );
                }
            }
            messages.push(current);
            let connection = Self::connection(&s, &p.conversation)?;
            let vision = p
                .vision_fallback
                .as_deref()
                .map(|id| Self::connection(&s, id))
                .transpose()?;
            (old, connection, messages, vision, vision_prompt)
        };
        if !connection.model.vision && messages.last().is_some_and(|m| !m.images.is_empty()) {
            let vision = vision.ok_or(Error::Invalid("vision fallback required"))?;
            let current = messages.last_mut().unwrap();
            let description = vision
                .complete(
                    &self.client,
                    &[Message::text("system", &vision_prompt), current.clone()],
                    false,
                    None,
                )
                .await?;
            current.images.clear();
            current.text.push_str(&format!(
                "\n[IMAGE_DESCRIPTION]{description}[/IMAGE_DESCRIPTION]"
            ));
        }
        let text = connection
            .complete(&self.client, &messages, true, events)
            .await?;
        let exchange =
            self.lock()?
                .revise_exchange(request_id, revision, None, Some(text), false)?;
        let repair = match self
            .distill_inner(&old.conversation_id, false, events)
            .await
        {
            Ok(v) => v,
            Err(e) => json!({"pending":true,"error":e.to_string()}),
        };
        Ok(json!({"exchange":exchange,"repair":repair}))
    }
    pub async fn chat(
        &self,
        request: ChatRequest,
        events: Option<&UnboundedSender<Value>>,
    ) -> Result<Exchange> {
        let _gate = self.gate.lock().await;
        self.chat_inner(request, events).await
    }
    async fn chat_inner(
        &self,
        request: ChatRequest,
        events: Option<&UnboundedSender<Value>>,
    ) -> Result<Exchange> {
        if request.request_id.is_empty()
            || request.text.len() > 200_000
            || request.attachments.len() > 16
        {
            return Err(Error::Invalid("invalid chat request"));
        }
        let previous: Option<String> = {
            self.lock()?
                .scheduler
                .db
                .query_row(
                    "SELECT input FROM requests WHERE id=?",
                    [&request.request_id],
                    |r| r.get(0),
                )
                .optional()?
        };
        let prepared = if let Some(previous) = previous {
            let p: Prepared = serde_json::from_str(&previous)?;
            if serde_json::to_value(&p.request)? != serde_json::to_value(&request)? {
                return Err(Error::Conflict("request ID reused with different input"));
            }
            let committed: Option<String> = {
                self.lock()?
                    .scheduler
                    .db
                    .query_row(
                        "SELECT body FROM exchanges WHERE request=?",
                        [&request.request_id],
                        |r| r.get(0),
                    )
                    .optional()?
            };
            if let Some(body) = committed {
                return Ok(serde_json::from_str(&body)?);
            }
            p
        } else {
            let (
                mut messages,
                connection,
                vision,
                revision,
                scope_revisions,
                conversation,
                vision_prompt,
                prompt_revisions,
            ) = {
                let mut s = self.lock()?;
                let c: Conversation = s.get("conversation", &request.conversation_id)?;
                if c.ended {
                    return Err(Error::Conflict("conversation ended"));
                }
                let purposes: Purposes = s.get("settings", "purposes")?;
                let connection = Self::connection(&s, &purposes.conversation)?;
                let vision = purposes
                    .vision_fallback
                    .as_deref()
                    .map(|id| Self::connection(&s, id))
                    .transpose()?;
                let snapshot = s.snapshot(&c.id, &request.text)?;
                let mut instructions = Self::conversation_prompt(&s, &c)?;
                let vision_prompt = s.prompt_space(PromptSpaceId::VisionFallback)?.content;
                let prompt_revisions = s
                    .prompt_spaces()?
                    .into_iter()
                    .map(|p| (p.id.as_str().to_owned(), p.revision))
                    .collect();
                instructions.push_str(&snapshot.context);
                let mut messages = vec![Message::text("system", instructions)];
                let history = s.history(&c.id)?;
                for exchange in history
                    .iter()
                    .rev()
                    .take(48)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    let saved: Option<String> = s
                        .scheduler
                        .db
                        .query_row(
                            "SELECT input FROM requests WHERE id=?",
                            [&exchange.request_id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    let previous = if exchange.revision == 1 {
                        saved
                            .and_then(|body| serde_json::from_str::<Prepared>(&body).ok())
                            .and_then(|p| p.messages.last().cloned())
                    } else {
                        None
                    };
                    messages
                        .push(previous.unwrap_or_else(|| Message::text("user", &exchange.user)));
                    messages.push(Message::text("assistant", &exchange.assistant));
                }
                let mut current = Message::text("user", &request.text);
                let mut attachment_bytes = 0u64;
                for id in &request.attachments {
                    let metadata: DataFile = s.get("file", id)?;
                    attachment_bytes = attachment_bytes.saturating_add(metadata.bytes);
                    if attachment_bytes > 16 * 1024 * 1024 {
                        return Err(Error::Invalid(
                            "model attachments exceed 16 MiB; files remain stored",
                        ));
                    }
                    let (f, b) = s.read_file(id)?;
                    if f.mime.starts_with("image/") {
                        current.images.push((f.mime, STANDARD.encode(b)));
                    } else if f.mime.starts_with("text/") {
                        let text = String::from_utf8(b)
                            .map_err(|_| Error::Invalid("text attachment is not UTF-8"))?;
                        if text.len() > 200_000 {
                            return Err(Error::Invalid("text attachment too large for injection"));
                        }
                        current
                            .text
                            .push_str(&format!("\n[DATA_FILE {}]\n{}\n[/DATA_FILE]", f.name, text));
                    } else {
                        return Err(Error::Invalid(
                            "file is stored, but this model adapter cannot parse its MIME type",
                        ));
                    }
                }
                messages.push(current);
                while messages.len() > 2
                    && messages
                        .iter()
                        .map(|m| {
                            m.text.len()
                                + m.images.iter().map(|(_, data)| data.len()).sum::<usize>()
                        })
                        .sum::<usize>()
                        > 24 * 1024 * 1024
                {
                    messages.drain(1..3);
                }
                (
                    messages,
                    connection,
                    vision,
                    snapshot.revision,
                    snapshot.scope_revisions,
                    c.id,
                    vision_prompt,
                    prompt_revisions,
                )
            };
            if !connection.model.vision && messages.iter().any(|m| !m.images.is_empty()) {
                let vision = vision.as_ref().ok_or(Error::Invalid(
                    "image input requires a vision fallback group",
                ))?;
                for current in messages.iter_mut().filter(|m| !m.images.is_empty()) {
                    let result = vision
                        .complete(
                            &self.client,
                            &[Message::text("system", &vision_prompt), current.clone()],
                            false,
                            None,
                        )
                        .await?;
                    current.images.clear();
                    current.text.push_str(&format!(
                        "\n[IMAGE_DESCRIPTION]\n{result}\n[/IMAGE_DESCRIPTION]"
                    ));
                    if let Some(e) = events {
                        let _ = e.send(json!({"type":"vision.completed"}));
                    }
                }
            }
            let p = Prepared {
                request: request.clone(),
                connection,
                messages,
                revision,
                scope_revisions,
                conversation_id: conversation,
                prompt_revisions,
            };
            self.lock()?.scheduler.db.execute(
                "INSERT INTO requests(id,input) VALUES (?,?)",
                params![request.request_id, serde_json::to_string(&p)?],
            )?;
            p
        };
        if self.lock()?.revision(&prepared.conversation_id)? != prepared.revision {
            return Err(Error::Conflict(
                "prepared conversation changed; use a new request ID",
            ));
        }
        for (conversation, revision) in &prepared.scope_revisions {
            if self.lock()?.revision(conversation)? != *revision {
                return Err(Error::Conflict(
                    "injection scope changed; use a new request ID",
                ));
            }
        }
        let assistant = prepared
            .connection
            .complete(&self.client, &prepared.messages, true, events)
            .await?;
        let exchange = Exchange {
            request_id: request.request_id,
            conversation_id: request.conversation_id,
            revision: 1,
            user: request.text,
            assistant,
            attachments: request.attachments,
            deleted: false,
        };
        {
            let mut s = self.lock()?;
            for (conversation, revision) in &prepared.scope_revisions {
                if s.revision(conversation)? != *revision {
                    return Err(Error::Conflict("injection scope changed during generation"));
                }
            }
            s.commit_exchange(&exchange, prepared.revision)?;
        }
        if let Some(e) = events {
            let _ = e.send(json!({"type":"exchange.committed","exchange":exchange}));
        }
        if let Err(error) = self
            .distill_inner(&exchange.conversation_id, false, events)
            .await
        {
            let mut s = self.lock()?;
            let tx = s.scheduler.db.transaction()?;
            Store::event_tx(
                &tx,
                &exchange.conversation_id,
                "memory.retry_pending",
                &json!({"error":error.to_string()}),
            )?;
            tx.commit()?;
            if let Some(e) = events {
                let _ = e.send(json!({"type":"memory.retry_pending","error":error.to_string()}));
            }
        }
        Ok(exchange)
    }
    pub async fn distill(
        &self,
        conversation: &str,
        early: bool,
        events: Option<&UnboundedSender<Value>>,
    ) -> Result<Value> {
        let _gate = self.gate.lock().await;
        self.distill_inner(conversation, early, events).await
    }
    async fn distill_inner(
        &self,
        conversation: &str,
        early: bool,
        events: Option<&UnboundedSender<Value>>,
    ) -> Result<Value> {
        let job = { self.lock()?.begin_extraction(conversation, early)? };
        let Some(job) = job else {
            return Ok(json!({"processed":false}));
        };
        let patch = if job.evidence.is_empty() {
            MemoryPatch::default()
        } else {
            let (connection, existing, prompt) = {
                let s = self.lock()?;
                let p: Purposes = s.get("settings", "purposes")?;
                (
                    Self::connection(&s, &p.memory)?,
                    s.list_memories(conversation)?,
                    s.prompt_space(PromptSpaceId::MemoryDistillation)?.content,
                )
            };
            let key = job
                .batch_id
                .map(|id| format!("batch:{id}"))
                .unwrap_or_else(|| format!("repair:{}:{}", job.conversation_id, job.revision));
            let messages = vec![
                Message::text("system", prompt),
                Message::text(
                    "user",
                    serde_json::to_string(
                        &json!({"repair":job.repair,"affected_ids":job.affected_ids,"evidence":job.evidence,"existing":existing}),
                    )?,
                ),
            ];
            let (connection, messages): (Connection, Vec<Message>) = {
                let s = self.lock()?;
                let previous: Option<String> = s
                    .scheduler
                    .db
                    .query_row(
                        "SELECT body FROM maintenance_requests WHERE id=?",
                        [&key],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(body) = previous {
                    serde_json::from_str(&body)?
                } else {
                    s.scheduler.db.execute(
                        "INSERT INTO maintenance_requests(id,body) VALUES (?,?)",
                        params![key, serde_json::to_string(&(&connection, &messages))?],
                    )?;
                    (connection, messages)
                }
            };
            let result = connection
                .complete(&self.client, &messages, false, None)
                .await?;
            serde_json::from_str::<MemoryPatch>(&result)?
        };
        self.lock()?.apply_extraction(&job, &patch)?;
        if let Some(e) = events {
            let _=e.send(json!({"type":if job.repair{"memory.repaired"}else{"memory.distilled"},"count":patch.upsert.len()}));
        }
        Ok(json!({"processed":true,"repair":job.repair,"documents":patch.upsert.len()}))
    }
    /// The same versioned operation surface serves Rust, HTTP and C ABI callers.
    pub async fn execute(
        &self,
        v: Value,
        events: Option<&UnboundedSender<Value>>,
    ) -> Result<Value> {
        if v.get("version").is_some_and(|v| v != 1) {
            return Err(Error::Invalid("unsupported operation version"));
        }
        let op = field(&v, "op")?;
        match op {
            "recover" => return self.recover().await,
            "exchange.regenerate" => {
                return self
                    .regenerate(
                        field(&v, "request_id")?,
                        v["revision"]
                            .as_i64()
                            .ok_or(Error::Invalid("revision required"))?,
                        events,
                    )
                    .await;
            }
            "chat" => {
                return Ok(serde_json::to_value(
                    self.chat(serde_json::from_value(v["request"].clone())?, events)
                        .await?,
                )?);
            }
            "distill" => {
                return self
                    .distill(
                        field(&v, "conversation_id")?,
                        v["early"].as_bool().unwrap_or(false),
                        events,
                    )
                    .await;
            }
            "exchange.revise" => {
                let _gate = self.gate.lock().await;
                let e = {
                    self.lock()?.revise_exchange(
                        field(&v, "request_id")?,
                        v["revision"]
                            .as_i64()
                            .ok_or(Error::Invalid("revision required"))?,
                        v["user"].as_str().map(str::to_owned),
                        v["assistant"].as_str().map(str::to_owned),
                        v["delete"].as_bool().unwrap_or(false),
                    )?
                };
                let repair = self.distill_inner(&e.conversation_id, false, events).await;
                return Ok(
                    json!({"exchange":e,"repair":match repair{Ok(v)=>v,Err(e)=>json!({"pending":true,"error":e.to_string()})}}),
                );
            }
            _ => {}
        }
        let _gate = self.gate.lock().await;
        let mut s = self.lock()?;
        match op {
            "prompt_spaces.list" => return Ok(json!(s.prompt_spaces()?)),
            "prompt_spaces.get" => {
                return Ok(json!(
                    s.prompt_space(serde_json::from_value(v["id"].clone())?)?
                ));
            }
            "prompt_spaces.replace" => {
                return Ok(json!(s.replace_prompt_space(
                    serde_json::from_value(v["id"].clone())?,
                    field(&v, "content")?.into(),
                    v["revision"].as_str()
                )?));
            }
            "prompt_spaces.reset" => {
                return Ok(json!(s.reset_prompt_space(
                    serde_json::from_value(v["id"].clone())?,
                    v["revision"].as_str()
                )?));
            }
            "conversation.delete" => s.delete_conversation(
                field(&v, "id")?,
                v["revision"]
                    .as_i64()
                    .ok_or(Error::Invalid("revision required"))?,
            )?,
            "file.delete" => s.delete_file(field(&v, "id")?)?,
            "resource.classify" => s.classify(
                field(&v, "kind")?,
                field(&v, "id")?,
                serde_json::from_value(v["value"].clone())?,
            )?,
            "moc.export" => {
                return s.export_moc(
                    Path::new(field(&v, "path")?),
                    v["include_files"].as_bool().unwrap_or(true),
                    v["passphrase"].as_str(),
                );
            }
            "moc.import" => {
                return s.import_moc(Path::new(field(&v, "path")?), v["passphrase"].as_str());
            }
            "provider.put" => s.put_provider(serde_json::from_value(v["value"].clone())?)?,
            "model.put" => s.put_model(serde_json::from_value(v["value"].clone())?)?,
            "group.put" => s.put_group(serde_json::from_value(v["value"].clone())?)?,
            "purposes.set" => s.set_purposes(serde_json::from_value(v["value"].clone())?)?,
            "character.put" => s.put_character(serde_json::from_value(v["value"].clone())?)?,
            "conversation.put" => s.put_conversation(
                serde_json::from_value(v["value"].clone())?,
                v["revision"].as_i64(),
            )?,
            "extraction.configure" => s.configure_extraction(
                field(&v, "conversation_id")?,
                v["every_turns"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or(Error::Invalid("every_turns required"))?,
            )?,
            "extraction.status" => return s.extraction_status(field(&v, "conversation_id")?),
            "resource.list" => return Ok(json!(s.list(resource_kind(&v)?)?)),
            "resource.get" => return s.get(resource_kind(&v)?, field(&v, "id")?),
            "history" => return Ok(json!(s.history(field(&v, "conversation_id")?)?)),
            "snapshot" => {
                return Ok(json!(s.snapshot(
                    field(&v, "conversation_id")?,
                    v["query"].as_str().unwrap_or("")
                )?));
            }
            "memory.list" => return Ok(json!(s.list_memories(field(&v, "conversation_id")?)?)),
            "events" => return Ok(json!(s.events(v["after"].as_i64().unwrap_or(0))?)),
            "file.add" => {
                let bytes = STANDARD
                    .decode(field(&v, "base64")?)
                    .map_err(|_| Error::Invalid("invalid base64"))?;
                return Ok(json!(s.add_file(
                    field(&v, "name")?.into(),
                    field(&v, "mime")?.into(),
                    &bytes
                )?));
            }
            "file.read" => {
                let (f, b) = s.read_file(field(&v, "id")?)?;
                return Ok(json!({"file":f,"base64":STANDARD.encode(b)}));
            }
            _ => return Err(Error::Invalid("unknown operation")),
        }
        Ok(json!({"ok":true}))
    }
}
fn resource_kind(v: &Value) -> Result<&str> {
    let kind = field(v, "kind")?;
    if kind == "settings" && v["op"] == "resource.get" && v["id"] == "purposes" {
        return Ok(kind);
    }
    if ![
        "conversation",
        "character",
        "file",
        "provider",
        "model",
        "group",
    ]
    .contains(&kind)
    {
        return Err(Error::Invalid("unknown resource kind"));
    }
    Ok(kind)
}
pub(crate) fn field<'a>(v: &'a Value, name: &str) -> Result<&'a str> {
    v[name]
        .as_str()
        .ok_or_else(|| Error::Message(format!("missing string field: {name}")))
}
impl Store {
    pub fn list_memories(&self, conversation: &str) -> Result<Vec<Value>> {
        let mut q = self
            .scheduler
            .db
            .prepare("SELECT body,valid,archived FROM memory WHERE scene=? ORDER BY id")?;
        q.query_map([conversation],|r|Ok((r.get::<_,String>(0)?,r.get::<_,bool>(1)?,r.get::<_,bool>(2)?)))?.map(|r|{let(b,valid,archived)=r?;Ok(json!({"document":serde_json::from_str::<Value>(&b)?,"valid":valid,"archived":archived}))}).collect()
    }
}
