//! Native MOC v3 packaging. Space paths are wire namespaces only.
use crate::{Error, Result, crypto, domain::*, moc, store::Store};
use rusqlite::{params, types::Value as SqlValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};
const AAD: &[u8] = b"momo-private-moc-v1";
const TABLES: &[(&str, &str)] = &[
    ("resources", "kind,id,body,revision"),
    ("scenes", "id,every_turns"),
    ("turns", "seq,scene,request,body,done"),
    ("batches", "id,scene,early,payload,result"),
    (
        "exchanges",
        "seq,request,conversation,scene,revision,body,deleted",
    ),
    ("memory", "scene,id,body,valid,archived,last_hit"),
    ("sources", "scene,memory_id,request,revision"),
    ("runtime", "scene,revision,activity,repair"),
];
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    version: u32,
    files_included: bool,
    tables: BTreeMap<String, Vec<Vec<Value>>>,
}
fn err(e: impl std::fmt::Display) -> Error {
    Error::Message(e.to_string())
}
impl Store {
    pub fn export_moc(
        &self,
        output: &Path,
        include_files: bool,
        password: Option<&str>,
    ) -> Result<Value> {
        let _read_snapshot = self.scheduler.db.unchecked_transaction()?;
        if output.exists() {
            return Err(Error::Conflict("export path already exists"));
        }
        let staging = tempfile::tempdir()?;
        let root = staging.path();
        fs::create_dir_all(root.join("molite"))?;
        let mut tables = BTreeMap::new();
        for (table, columns) in TABLES {
            let filter = if *table == "resources" {
                " WHERE kind NOT IN ('provider','model','group','settings','prompt_space')"
            } else {
                ""
            };
            let mut q = self
                .scheduler
                .db
                .prepare(&format!("SELECT {columns} FROM {table}{filter}"))?;
            let count = q.column_count();
            let rows = q
                .query_map([], |r| {
                    (0..count)
                        .map(|i| {
                            Ok(match r.get::<_, SqlValue>(i)? {
                                SqlValue::Null => Value::Null,
                                SqlValue::Integer(v) => json!(v),
                                SqlValue::Real(v) => json!(v),
                                SqlValue::Text(v) => json!(v),
                                SqlValue::Blob(_) => Value::Null,
                            })
                        })
                        .collect::<std::result::Result<Vec<_>, rusqlite::Error>>()
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            tables.insert(table.to_string(), rows);
        }
        fs::write(
            root.join("molite/state.json"),
            serde_json::to_vec(&Bundle {
                version: 2,
                files_included: include_files,
                tables,
            })?,
        )?;
        if include_files {
            fs::create_dir_all(root.join("molite/files"))?;
            for f in self.list("file")? {
                let f: DataFile = serde_json::from_value(f)?;
                let (_, bytes) = self.read_file(&f.id)?;
                fs::write(root.join("molite/files").join(f.sha256), bytes)?;
            }
        }
        let namespace = "00000000-0000-0000-0000-000000000001";
        let roles = root.join("characters/spaces").join(namespace);
        fs::create_dir_all(&roles)?;
        let mut ids = vec![];
        for value in self.list("character")? {
            let c: Character = serde_json::from_value(value)?;
            let uuid = uuid::Uuid::parse_str(&c.id)
                .map_err(|_| Error::Invalid("native character ID must be UUID"))?;
            ids.push(uuid.to_string());
            let directory = roles.join(uuid.to_string());
            fs::create_dir_all(&directory)?;
            let metadata = json!({"id":format!("urn:uuid:{uuid}"),"name":c.name,"version":c.version,"author":{"name":c.author_name},"character_file":"character.md","user_file":"user.md"});
            let mut metadata: toml::Value = serde_json::from_value(metadata).map_err(err)?;
            if c.opening_markdown.is_some() {
                metadata
                    .as_table_mut()
                    .ok_or(Error::Invalid("native metadata must be a table"))?
                    .insert(
                        "opening_file".into(),
                        toml::Value::String("opening.md".into()),
                    );
            }
            fs::write(
                directory.join("character.toml"),
                toml::to_string(&metadata).map_err(err)?,
            )?;
            fs::write(directory.join("character.md"), c.character_markdown)?;
            fs::write(directory.join("user.md"), c.user_markdown)?;
            if let Some(opening) = c.opening_markdown {
                fs::write(directory.join("opening.md"), opening)?;
            }
        }
        fs::write(roles.join("index.json"), serde_json::to_vec(&ids)?)?;
        let definitions = vec![
            moc::ModuleDefinition {
                id: "characters".into(),
                path: "characters".into(),
                dependencies: vec![],
                import_order: 20,
            },
            moc::ModuleDefinition {
                id: "molite-runtime".into(),
                path: "molite".into(),
                dependencies: vec!["characters".into()],
                import_order: 60,
            },
        ];
        let spaces = vec![moc::SpaceModuleDefinition {
            space_id: namespace.into(),
            module: "characters".into(),
            path: format!("characters/spaces/{namespace}"),
        }];
        let inner = root.join("inner.moc");
        moc::create_from_definitions_and_spaces(&inner, root, &definitions, &spaces)
            .map_err(err)?;
        let manifest = if let Some(password) = password {
            if password.is_empty() {
                return Err(Error::Invalid("empty passphrase"));
            }
            let bytes = fs::read(&inner)?;
            if bytes.len() > 512 * 1024 * 1024 {
                return Err(Error::Invalid("encrypted MOC exceeds 512 MiB"));
            }
            let envelope = crypto::encrypt(&bytes, password, AAD, crypto::KdfParameters::STANDARD)
                .map_err(err)?;
            fs::create_dir(root.join("private"))?;
            fs::write(
                root.join("private/payload.enc"),
                crypto::encode(&envelope).map_err(err)?,
            )?;
            moc::create_with_encryption(
                output,
                root,
                &[("encrypted-container".into(), PathBuf::from("private"))],
                Some(moc::EncryptionMetadata {
                    profile: "momo-envelope-v1".into(),
                    payload_path: "private/payload.enc".into(),
                    associated_data: String::from_utf8_lossy(AAD).into(),
                }),
            )
            .map_err(err)?
        } else {
            let manifest = moc::inspect(&inner).map_err(err)?;
            let mut input = fs::File::open(&inner)?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(output)?;
            std::io::copy(&mut input, &mut file)?;
            file.sync_all()?;
            manifest
        };
        Ok(json!(manifest))
    }
    pub fn import_moc(&mut self, input: &Path, password: Option<&str>) -> Result<Value> {
        let outer = tempfile::tempdir()?;
        let limits = moc::ExtractionLimits {
            max_entries: 10_000,
            max_unpacked_bytes: 512 * 1024 * 1024,
        };
        let manifest = moc::extract(input, outer.path(), limits).map_err(err)?;
        let inner = tempfile::tempdir()?;
        let (root, manifest) = if let Some(encryption) = &manifest.encryption {
            if encryption.profile != "momo-envelope-v1"
                || encryption.payload_path != "private/payload.enc"
                || encryption.associated_data.as_bytes() != AAD
            {
                return Err(Error::Invalid("unsupported encrypted MOC profile"));
            }
            let envelope =
                crypto::decode_envelope(&fs::read(outer.path().join("private/payload.enc"))?)
                    .map_err(err)?;
            let payload = crypto::decrypt(
                &envelope,
                password.ok_or(Error::Invalid("passphrase required"))?,
                AAD,
            )
            .map_err(err)?;
            let path = inner.path().join("inner.moc");
            fs::write(&path, payload)?;
            let extracted = inner.path().join("payload");
            fs::create_dir(&extracted)?;
            let m = moc::extract(path, &extracted, limits).map_err(err)?;
            (extracted, m)
        } else {
            (outer.path().to_path_buf(), manifest)
        };
        let bundle = root.join("molite/state.json");
        if bundle.exists() {
            let bundle: Bundle = serde_json::from_slice(&fs::read(bundle)?)?;
            if bundle.version != 2 {
                return Err(Error::Invalid("unsupported MOLite bundle version"));
            }
            let temp = tempfile::tempdir()?;
            let mut validated = Store::open(temp.path())?;
            {
                let tx = validated.scheduler.db.transaction()?;
                for (table, columns) in TABLES {
                    let rows = bundle
                        .tables
                        .get(*table)
                        .ok_or(Error::Invalid("incomplete MOLite bundle"))?;
                    let placeholders = columns
                        .split(',')
                        .map(|_| "?")
                        .collect::<Vec<_>>()
                        .join(",");
                    for row in rows {
                        if row.len() != columns.split(',').count() {
                            return Err(Error::Invalid("invalid bundle row"));
                        }
                        let values = row.iter().map(sql_value).collect::<Result<Vec<_>>>()?;
                        tx.execute(
                            &format!("INSERT INTO {table}({columns}) VALUES ({placeholders})"),
                            rusqlite::params_from_iter(values),
                        )?;
                    }
                }
                tx.commit()?;
            }
            validate_bundle(&validated)?;
            for f in validated.list("file")? {
                let f: DataFile = serde_json::from_value(f)?;
                if bundle.files_included {
                    let path = root.join("molite/files").join(&f.sha256);
                    let bytes = fs::read(path)?;
                    use sha2::{Digest, Sha256};
                    if bytes.len() as u64 != f.bytes
                        || hex::encode(Sha256::digest(&bytes)) != f.sha256
                    {
                        return Err(Error::Invalid("bundle data file integrity failure"));
                    }
                    let target = self.root.join("files").join(&f.sha256);
                    if !target.exists() {
                        fs::write(target, bytes)?;
                    }
                }
            }
            let tx = self
                .scheduler
                .db
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            for (table, columns) in TABLES {
                let rows = &bundle.tables[*table];
                // Sequence numbers are local; request, scene and document identities are preserved.
                let drop_sequence = ["turns", "batches", "exchanges"].contains(table);
                let columns = if drop_sequence {
                    columns.split_once(',').unwrap().1
                } else {
                    columns
                };
                let placeholders = columns
                    .split(',')
                    .map(|_| "?")
                    .collect::<Vec<_>>()
                    .join(",");
                for row in rows {
                    let values = row[usize::from(drop_sequence)..]
                        .iter()
                        .map(sql_value)
                        .collect::<Result<Vec<_>>>()?;
                    tx.execute(&format!("INSERT INTO {table}({columns}) VALUES ({placeholders})"),rusqlite::params_from_iter(values)).map_err(|_|Error::Conflict("MOC identities conflict; import into an empty store or remove conflicts"))?;
                }
            }
            Store::event_tx(
                &tx,
                "",
                "moc.imported",
                &json!({"profile":"molite-runtime-v1"}),
            )?;
            tx.commit()?;
            Ok(
                json!({"imported":true,"profile":"molite-runtime-v1","files_included":bundle.files_included}),
            )
        } else {
            // Native MOMO role assets retain their native identity and markdown format.
            let mut characters = vec![];
            for entry in &manifest.modules {
                if entry.module != "characters" || !entry.path.ends_with("/character.toml") {
                    continue;
                }
                let path = root.join(&entry.path);
                let directory = path.parent().ok_or(Error::Invalid("invalid role path"))?;
                let metadata: toml::Value =
                    toml::from_str(&fs::read_to_string(&path)?).map_err(err)?;
                let id = metadata
                    .get("id")
                    .and_then(toml::Value::as_str)
                    .and_then(|s| s.strip_prefix("urn:uuid:"))
                    .ok_or(Error::Invalid("invalid native character URN"))?;
                let uuid = uuid::Uuid::parse_str(id).map_err(err)?;
                let c = Character {
                    id: uuid.to_string(),
                    name: meta(&metadata, "name")?,
                    version: meta(&metadata, "version")?,
                    author_name: metadata
                        .get("author")
                        .and_then(|v| v.get("name"))
                        .and_then(toml::Value::as_str)
                        .ok_or(Error::Invalid("native author required"))?
                        .into(),
                    character_markdown: asset(
                        directory,
                        metadata
                            .get("character_file")
                            .and_then(toml::Value::as_str)
                            .unwrap_or("character.md"),
                    )?,
                    user_markdown: optional_asset(directory, &metadata, "user_file", "user.md")?
                        .unwrap_or_default(),
                    opening_markdown: optional_asset(
                        directory,
                        &metadata,
                        "opening_file",
                        "opening.md",
                    )?,
                    classification: Default::default(),
                };
                if self.get::<Character>("character", &c.id).is_ok() {
                    return Err(Error::Conflict("native character already exists"));
                }
                characters.push(c);
            }
            if characters.is_empty() {
                return Err(Error::Invalid(
                    "MOC contains no supported native characters or MOLite runtime module",
                ));
            }
            let tx = self.scheduler.db.transaction()?;
            for c in &characters {
                if semver::Version::parse(&c.version).is_err() || c.name.is_empty() {
                    return Err(Error::Invalid("invalid native metadata"));
                }
                tx.execute(
                    "INSERT INTO resources(kind,id,body) VALUES ('character',?,?)",
                    params![c.id, serde_json::to_string(c)?],
                )?;
            }
            tx.commit()?;
            Ok(
                json!({"imported":true,"characters":characters.len(),"skipped_modules":manifest.module_definitions.iter().filter(|m|m.id!="characters").map(|m|&m.id).collect::<Vec<_>>()}),
            )
        }
    }
}
fn meta(v: &toml::Value, key: &str) -> Result<String> {
    v.get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or(Error::Invalid("missing native metadata"))
}
fn asset(directory: &Path, path: &str) -> Result<String> {
    let p = Path::new(path);
    if p.components().any(|c| !matches!(c, Component::Normal(_)))
        || path.contains('\\')
        || path.contains(':')
    {
        return Err(Error::Invalid("unsafe native asset path"));
    }
    let b = fs::read(directory.join(p))?;
    if b.len() > 200_000 {
        return Err(Error::Invalid("native markdown too large"));
    }
    String::from_utf8(b).map_err(err)
}
fn optional_asset(
    directory: &Path,
    v: &toml::Value,
    key: &str,
    default: &str,
) -> Result<Option<String>> {
    if let Some(path) = v.get(key).and_then(toml::Value::as_str) {
        return asset(directory, path).map(Some);
    }
    if directory.join(default).exists() {
        asset(directory, default).map(Some)
    } else {
        Ok(None)
    }
}
fn sql_value(v: &Value) -> Result<SqlValue> {
    match v {
        Value::Null => Ok(SqlValue::Null),
        Value::String(v) => Ok(SqlValue::Text(v.clone())),
        Value::Number(v) => v
            .as_i64()
            .map(SqlValue::Integer)
            .ok_or(Error::Invalid("bundle integer required")),
        _ => Err(Error::Invalid("unsupported bundle value")),
    }
}
fn validate_bundle(s: &Store) -> Result<()> {
    let mut expected_sources = 0i64;
    for kind in ["provider", "model", "group", "settings", "prompt_space"] {
        if !s.list(kind)?.is_empty() {
            return Err(Error::Invalid(
                "MOC must not install provider configuration",
            ));
        }
    }
    let mut kinds = s
        .scheduler
        .db
        .prepare("SELECT DISTINCT kind FROM resources")?;
    for kind in kinds.query_map([], |r| r.get::<_, String>(0))? {
        if !["conversation", "character", "file", "narrative_sources"].contains(&kind?.as_str()) {
            return Err(Error::Invalid("unsupported MOC resource kind"));
        }
    }
    for v in s.list("conversation")? {
        let scene: Conversation = serde_json::from_value(v)?;
        for id in &scene.injection_scope {
            let _: Conversation = s.get("conversation", id)?;
        }
    }
    for v in s.list("conversation")? {
        let c: Conversation = serde_json::from_value(v)?;
        for id in c.character_ids {
            let _: Character = s.get("character", &id)?;
        }
    }
    let mut exchanges = s
        .scheduler
        .db
        .prepare("SELECT request,conversation,scene,revision,body,deleted FROM exchanges")?;
    for row in exchanges.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, bool>(5)?,
        ))
    })? {
        let (request, conversation, owner, revision, body, deleted) = row?;
        let e: Exchange = serde_json::from_str(&body)?;
        let _: Conversation = s.get("conversation", &conversation)?;
        if owner != conversation
            || e.conversation_id != conversation
            || e.request_id != request
            || e.revision != revision
            || e.deleted != deleted
        {
            return Err(Error::Invalid("MOC exchange ownership mismatch"));
        }
    }
    for v in s.list("file")? {
        let f: DataFile = serde_json::from_value(v)?;
        if f.sha256.len() != 64
            || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || f.bytes > 64 * 1024 * 1024
        {
            return Err(Error::Invalid("invalid data file metadata"));
        }
    }
    let mut q = s
        .scheduler
        .db
        .prepare("SELECT scene,id,body,valid FROM memory")?;
    for row in q.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, bool>(3)?,
        ))
    })? {
        let (scene, id, body, valid) = row?;
        let _: Conversation = s.get("conversation", &scene)?;
        let m: Memory = serde_json::from_str(&body)?;
        m.validate()?;
        if m.id != id {
            return Err(Error::Invalid("memory identity mismatch"));
        }
        expected_sources += m.sources.len() as i64;
        for source in m.sources {
            let indexed:i64=s.scheduler.db.query_row("SELECT COUNT(*) FROM sources WHERE scene=? AND memory_id=? AND request=? AND revision=?",params![scene,id,source.request_id,source.revision],|r|r.get(0))?;
            if indexed != 1 {
                return Err(Error::Invalid(
                    "imported source index does not match document",
                ));
            }
            let body: String = s.scheduler.db.query_row(
                "SELECT body FROM exchanges WHERE request=? AND scene=?",
                params![source.request_id, scene],
                |r| r.get(0),
            )?;
            let e: Exchange = serde_json::from_str(&body)?;
            if valid && (e.deleted || e.revision != source.revision) {
                return Err(Error::Invalid("stale imported memory provenance"));
            }
        }
    }
    let extra:i64=s.scheduler.db.query_row("SELECT COUNT(*) FROM sources s LEFT JOIN memory m ON m.scene=s.scene AND m.id=s.memory_id WHERE m.id IS NULL",[],|r|r.get(0))?;
    let count: i64 = s
        .scheduler
        .db
        .query_row("SELECT COUNT(*) FROM sources", [], |r| r.get(0))?;
    if extra != 0 || count != expected_sources {
        return Err(Error::Invalid("orphan imported source index"));
    }
    Ok(())
}
