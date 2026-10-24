use crate::{Error, Result, Scheduler, domain::*};
use rusqlite::{OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub struct Store {
    pub(crate) scheduler: Scheduler,
    pub root: PathBuf,
}
impl Store {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        std::fs::create_dir_all(root.as_ref())?;
        let root = std::fs::canonicalize(root)?;
        let scheduler = Scheduler::open(root.join("molite.sqlite3"))?;
        scheduler.db.execute_batch("CREATE TABLE IF NOT EXISTS resources(kind TEXT NOT NULL,id TEXT NOT NULL,body TEXT NOT NULL,revision INTEGER NOT NULL DEFAULT 1,PRIMARY KEY(kind,id));
        CREATE TABLE IF NOT EXISTS exchanges(seq INTEGER PRIMARY KEY AUTOINCREMENT,request TEXT UNIQUE NOT NULL,conversation TEXT NOT NULL,scene TEXT NOT NULL,revision INTEGER NOT NULL,body TEXT NOT NULL,deleted INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX IF NOT EXISTS scene_history ON exchanges(scene,seq);
        CREATE TABLE IF NOT EXISTS memory(scene TEXT NOT NULL,id TEXT NOT NULL,body TEXT NOT NULL,valid INTEGER NOT NULL DEFAULT 1,archived INTEGER NOT NULL DEFAULT 0,last_hit INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(scene,id));
        CREATE TABLE IF NOT EXISTS sources(scene TEXT NOT NULL,memory_id TEXT NOT NULL,request TEXT NOT NULL,revision INTEGER NOT NULL,PRIMARY KEY(scene,memory_id,request));
        CREATE INDEX IF NOT EXISTS source_reverse ON sources(request,revision);
        CREATE TABLE IF NOT EXISTS runtime(scene TEXT PRIMARY KEY,revision INTEGER NOT NULL DEFAULT 0,activity INTEGER NOT NULL DEFAULT 0,repair INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY AUTOINCREMENT,scene TEXT NOT NULL,kind TEXT NOT NULL,body TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS snapshots(id TEXT PRIMARY KEY,scene TEXT NOT NULL,revision INTEGER NOT NULL,body TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS requests(id TEXT PRIMARY KEY,input TEXT NOT NULL,result TEXT);
        CREATE TABLE IF NOT EXISTS maintenance_requests(id TEXT PRIMARY KEY,body TEXT NOT NULL);")?;
        std::fs::create_dir_all(root.join("files"))?;
        Ok(Self { scheduler, root })
    }
    pub fn get<T: DeserializeOwned>(&self, kind: &str, id: &str) -> Result<T> {
        let body: Option<String> = self
            .scheduler
            .db
            .query_row(
                "SELECT body FROM resources WHERE kind=? AND id=?",
                params![kind, id],
                |r| r.get(0),
            )
            .optional()?;
        serde_json::from_str(&body.ok_or(Error::Invalid("resource not found"))?).map_err(Into::into)
    }
    pub fn list(&self, kind: &str) -> Result<Vec<Value>> {
        let mut q = self
            .scheduler
            .db
            .prepare("SELECT body FROM resources WHERE kind=? ORDER BY id")?;
        q.query_map([kind], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    pub(crate) fn put<T: Serialize>(&self, kind: &str, id: &str, value: &T) -> Result<()> {
        if id.trim().is_empty() || id.len() > 256 {
            return Err(Error::Invalid("invalid resource ID"));
        }
        self.scheduler.db.execute("INSERT INTO resources(kind,id,body) VALUES (?,?,?) ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body,revision=resources.revision+1",params![kind,id,serde_json::to_string(value)?])?;
        Ok(())
    }
    pub fn put_provider(&self, p: Provider) -> Result<()> {
        let url =
            reqwest::Url::parse(&p.base_url).map_err(|_| Error::Invalid("invalid provider URL"))?;
        if !["http", "https"].contains(&url.scheme())
            || !url.username().is_empty()
            || url.password().is_some()
            || p.protocols.is_empty()
        {
            return Err(Error::Invalid("invalid provider URL or protocols"));
        }
        self.put("provider", &p.id, &p)
    }
    pub fn put_model(&self, m: Model) -> Result<()> {
        let _: Provider = self.get("provider", &m.provider_id)?;
        if m.model.is_empty() {
            return Err(Error::Invalid("empty model"));
        }
        self.put("model", &m.id, &m)
    }
    pub fn put_group(&self, g: ModelGroup) -> Result<()> {
        let m: Model = self.get("model", &g.model_id)?;
        let p: Provider = self.get("provider", &m.provider_id)?;
        if !p.protocols.contains(&g.protocol) || !(1..=600).contains(&g.timeout_seconds) {
            return Err(Error::Invalid("unsupported protocol or timeout"));
        }
        let Some(params) = g.parameters.as_object() else {
            return Err(Error::Invalid("parameters must be an object"));
        };
        if params.keys().any(|k| {
            [
                "model",
                "messages",
                "input",
                "system",
                "stream",
                "tools",
                "tool_choice",
                "store",
                "previous_response_id",
            ]
            .contains(&k.as_str())
        }) {
            return Err(Error::Invalid("reserved request parameter"));
        }
        self.put("group", &g.id, &g)
    }
    pub fn set_purposes(&self, p: Purposes) -> Result<()> {
        let _: ModelGroup = self.get("group", &p.conversation)?;
        let _: ModelGroup = self.get("group", &p.memory)?;
        if let Some(v) = &p.vision_fallback {
            let g: ModelGroup = self.get("group", v)?;
            let m: Model = self.get("model", &g.model_id)?;
            if !m.vision {
                return Err(Error::Invalid("vision fallback requires image capability"));
            }
        }
        self.put("settings", "purposes", &p)
    }
    pub fn put_character(&self, c: Character) -> Result<()> {
        if uuid::Uuid::parse_str(&c.id).is_err()
            || c.name.trim().is_empty()
            || c.author_name.trim().is_empty()
            || c.character_markdown.len() > 200_000
            || semver::Version::parse(&c.version).is_err()
        {
            return Err(Error::Invalid("invalid native character"));
        }
        self.put("character", &c.id, &c)
    }

    /// Classification never changes scene selection or memory provenance.
    pub fn classify(&self, kind: &str, id: &str, classification: Classification) -> Result<()> {
        if !["scene", "conversation", "character", "file"].contains(&kind) {
            return Err(Error::Invalid("resource cannot be classified"));
        }
        if classification.tags.len() > 100
            || classification.tags.iter().any(|t| t.len() > 200)
            || classification
                .folder
                .as_ref()
                .is_some_and(|s| s.len() > 1000)
        {
            return Err(Error::Invalid("classification exceeds limits"));
        }
        let mut value: Value = self.get(kind, id)?;
        value["classification"] = serde_json::to_value(classification)?;
        self.put(kind, id, &value)
    }
    pub fn delete_scene(&mut self, id: &str, expected: i64) -> Result<()> {
        let _: Scene = self.get("scene", id)?;
        for value in self.list("scene")? {
            let scene: Scene = serde_json::from_value(value)?;
            if scene.id != id && scene.injection_scope.iter().any(|scope| scope == id) {
                return Err(Error::Conflict(
                    "scene is referenced by an injection scope; switch that scope first",
                ));
            }
        }
        let tx = self
            .scheduler
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let revision: i64 =
            tx.query_row("SELECT revision FROM runtime WHERE scene=?", [id], |r| {
                r.get(0)
            })?;
        if revision != expected {
            return Err(Error::Conflict("scene revision changed"));
        }
        tx.execute(
            "DELETE FROM requests WHERE id IN (SELECT request FROM exchanges WHERE scene=?)",
            [id],
        )?;
        tx.execute(
            "DELETE FROM resources WHERE kind='conversation' AND json_extract(body,'$.scene_id')=?",
            [id],
        )?;
        for table in [
            "sources",
            "memory",
            "snapshots",
            "turns",
            "batches",
            "exchanges",
            "runtime",
        ] {
            tx.execute(&format!("DELETE FROM {table} WHERE scene=?"), [id])?;
        }
        tx.execute("DELETE FROM scenes WHERE id=?", [id])?;
        tx.execute("DELETE FROM resources WHERE kind='scene' AND id=?", [id])?;
        tx.execute(
            "INSERT INTO events(scene,kind,body) VALUES (?,'scene.deleted','{}')",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn delete_file(&self, id: &str) -> Result<()> {
        let _: DataFile = self.get("file", id)?;
        let used:bool=self.scheduler.db.query_row("SELECT EXISTS(SELECT 1 FROM exchanges,json_each(exchanges.body,'$.attachments') WHERE json_each.value=?)",[id],|r|r.get(0))?;
        if used {
            return Err(Error::Conflict("data file is retained as message evidence"));
        }
        self.scheduler
            .db
            .execute("DELETE FROM resources WHERE kind='file' AND id=?", [id])?;
        Ok(())
    }
    pub fn put_scene(&mut self, s: Scene, expected: Option<i64>) -> Result<()> {
        for scope in &s.injection_scope {
            if scope != &s.id {
                let _: Scene = self.get("scene", scope)?;
            }
        }
        let tx = self
            .scheduler
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let revision: i64 = tx
            .query_row("SELECT revision FROM runtime WHERE scene=?", [&s.id], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or(0);
        if expected.is_some_and(|e| e != revision) {
            return Err(Error::Conflict("scene revision changed"));
        }
        tx.execute("INSERT OR IGNORE INTO scenes(id) VALUES (?)", [&s.id])?;
        tx.execute("INSERT INTO resources(kind,id,body) VALUES ('scene',?,?) ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body,revision=resources.revision+1",params![s.id,serde_json::to_string(&s)?])?;
        tx.execute(
            "DELETE FROM resources WHERE kind='narrative_sources' AND id=?",
            [&s.id],
        )?;
        Self::event_tx(&tx, &s.id, "scene.updated", &json!({}))?;
        tx.commit()?;
        Ok(())
    }
    pub fn put_conversation(&self, c: Conversation) -> Result<()> {
        let _: Scene = self.get("scene", &c.scene_id)?;
        for id in &c.character_ids {
            let _: Character = self.get("character", id)?;
        }
        if let Ok(old) = self.get::<Conversation>("conversation", &c.id)
            && old.scene_id != c.scene_id
            && !self.history(&c.id)?.is_empty()
        {
            return Err(Error::Conflict(
                "cannot move nonempty conversation; change injection scope instead",
            ));
        }
        self.put("conversation", &c.id, &c)
    }
    pub fn history(&self, conversation: &str) -> Result<Vec<Exchange>> {
        let mut q = self.scheduler.db.prepare(
            "SELECT body FROM exchanges WHERE conversation=? AND deleted=0 ORDER BY seq",
        )?;
        q.query_map([conversation], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    pub fn revision(&self, scene: &str) -> Result<i64> {
        Ok(self
            .scheduler
            .db
            .query_row("SELECT revision FROM runtime WHERE scene=?", [scene], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or(0))
    }
    pub(crate) fn event_tx(
        tx: &rusqlite::Transaction<'_>,
        scene: &str,
        kind: &str,
        data: &Value,
    ) -> Result<()> {
        tx.execute("INSERT INTO runtime(scene,revision) VALUES (?,1) ON CONFLICT(scene) DO UPDATE SET revision=revision+1",[scene])?;
        tx.execute(
            "INSERT INTO events(scene,kind,body) VALUES (?,?,?)",
            params![scene, kind, serde_json::to_string(data)?],
        )?;
        Ok(())
    }
    pub fn events(&self, after: i64) -> Result<Vec<Event>> {
        let mut q = self.scheduler.db.prepare(
            "SELECT seq,scene,kind,body FROM events WHERE seq>? ORDER BY seq LIMIT 1000",
        )?;
        let rows = q.query_map([after], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, String>(3)?))
        })?;
        rows.map(|r| {
            let (sequence, scene_id, kind, body) = r?;
            Ok(Event {
                sequence,
                scene_id,
                kind,
                data: serde_json::from_str(&body)?,
            })
        })
        .collect()
    }
    pub fn add_file(&self, name: String, mime: String, bytes: &[u8]) -> Result<DataFile> {
        use sha2::{Digest, Sha256};
        if bytes.len() > 64 * 1024 * 1024 || name.is_empty() {
            return Err(Error::Invalid("file exceeds 64 MiB or empty name"));
        }
        let sha256 = hex::encode(Sha256::digest(bytes));
        let path = self.root.join("files").join(&sha256);
        if !path.exists() {
            let mut tmp = tempfile::NamedTempFile::new_in(self.root.join("files"))?;
            use std::io::Write;
            tmp.write_all(bytes)?;
            tmp.as_file().sync_all()?;
            tmp.persist(&path).map_err(|e| Error::Io(e.error))?;
        }
        let f = DataFile {
            id: id(),
            name,
            mime,
            sha256,
            bytes: bytes.len() as u64,
            classification: Default::default(),
        };
        self.put("file", &f.id, &f)?;
        Ok(f)
    }
    pub fn read_file(&self, id: &str) -> Result<(DataFile, Vec<u8>)> {
        use sha2::{Digest, Sha256};
        let f: DataFile = self.get("file", id)?;
        if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid("invalid file digest"));
        }
        let b = std::fs::read(self.root.join("files").join(&f.sha256))?;
        if hex::encode(Sha256::digest(&b)) != f.sha256 {
            return Err(Error::Conflict("file integrity check failed"));
        }
        Ok((f, b))
    }
}
