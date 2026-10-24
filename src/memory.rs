//! Transactional DMW and the DMW-only MO State v2 runtime profile.
use crate::{Error, Result, Turn, domain::*, store::Store};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extraction {
    pub conversation_id: String,
    pub revision: i64,
    pub batch_id: Option<i64>,
    pub repair: bool,
    #[serde(default)]
    pub affected_ids: Vec<String>,
    pub evidence: Vec<Exchange>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub conversation_id: String,
    pub revision: i64,
    pub scope_revisions: BTreeMap<String, i64>,
    pub narrative: Narrative,
    pub dimensions: BTreeMap<String, Vec<String>>,
    pub memories: Vec<Memory>,
    pub context: String,
}

impl Store {
    /// Rebuild the derived reverse index and quarantine stale facts on startup.
    pub fn reconcile(&mut self) -> Result<()> {
        let tx = self
            .scheduler
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let rows = {
            let mut q = tx.prepare("SELECT scene,id,body,valid FROM memory")?;
            q.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        tx.execute("DELETE FROM sources", [])?;
        for (conversation, id, body, valid) in rows {
            let m: Memory = serde_json::from_str(&body)?;
            m.validate()?;
            let mut stale = false;
            for source in &m.sources {
                let current: Option<(i64, bool)> = tx
                    .query_row(
                        "SELECT revision,deleted FROM exchanges WHERE request=? AND scene=?",
                        params![source.request_id, conversation],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                stale |= current != Some((source.revision, false));
                tx.execute("INSERT OR IGNORE INTO sources(scene,memory_id,request,revision) VALUES (?,?,?,?)",params![conversation,id,source.request_id,source.revision])?;
            }
            if stale && valid {
                tx.execute(
                    "UPDATE memory SET valid=0 WHERE scene=? AND id=?",
                    params![conversation, id],
                )?;
                tx.execute("UPDATE runtime SET repair=1 WHERE scene=?", [&conversation])?;
                Self::event_tx(
                    &tx,
                    &conversation,
                    "memory.reconcile_invalidated",
                    &json!({"id":id}),
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub fn commit_exchange(&mut self, e: &Exchange, expected: i64) -> Result<()> {
        let c: Conversation = self.get("conversation", &e.conversation_id)?;
        if e.conversation_id != c.id
            || e.deleted
            || e.revision != 1
            || e.request_id.is_empty()
            || e.assistant.trim().is_empty()
            || c.ended
        {
            return Err(Error::Invalid("invalid exchange or ended conversation"));
        }
        let body = serde_json::to_string(e)?;
        let tx = self
            .scheduler
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let old: Option<String> = tx
            .query_row(
                "SELECT body FROM exchanges WHERE request=?",
                [&e.request_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old) = old {
            return if old == body {
                Ok(())
            } else {
                Err(Error::Conflict("request ID already used"))
            };
        }
        let revision: i64 = tx
            .query_row(
                "SELECT revision FROM runtime WHERE scene=?",
                [&e.conversation_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if revision != expected {
            return Err(Error::Conflict(
                "conversation changed during generation; retry",
            ));
        }
        tx.execute(
            "INSERT INTO exchanges(request,conversation,scene,revision,body) VALUES (?,?,?,1,?)",
            params![e.request_id, e.conversation_id, e.conversation_id, body],
        )?;
        let t = Turn {
            request_id: e.request_id.clone(),
            user: e.user.clone(),
            assistant: e.assistant.clone(),
        };
        tx.execute(
            "INSERT INTO turns(scene,request,body) VALUES (?,?,?)",
            params![e.conversation_id, e.request_id, serde_json::to_string(&t)?],
        )?;
        tx.execute(
            "UPDATE runtime SET activity=activity+1 WHERE scene=?",
            [&e.conversation_id],
        )?;
        Self::event_tx(
            &tx,
            &e.conversation_id,
            "exchange.committed",
            &json!({"request_id":e.request_id}),
        )?;
        Self::lifecycle_tx(&tx, &e.conversation_id)?;
        tx.commit()?;
        Ok(())
    }
    /// Edits and regeneration replace one revision, never count as another turn.
    pub fn revise_exchange(
        &mut self,
        id: &str,
        expected: i64,
        user: Option<String>,
        assistant: Option<String>,
        delete: bool,
    ) -> Result<Exchange> {
        let tx = self
            .scheduler
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let body: String =
            tx.query_row("SELECT body FROM exchanges WHERE request=?", [id], |r| {
                r.get(0)
            })?;
        let mut e: Exchange = serde_json::from_str(&body)?;
        if e.revision != expected || e.deleted {
            return Err(Error::Conflict("message revision changed"));
        }
        if let Some(v) = user {
            e.user = v;
        }
        if let Some(v) = assistant {
            if v.trim().is_empty() {
                return Err(Error::Invalid("empty assistant"));
            }
            e.assistant = v;
        }
        e.deleted = delete;
        e.revision += 1;
        tx.execute(
            "UPDATE exchanges SET revision=?,body=?,deleted=? WHERE request=?",
            params![e.revision, serde_json::to_string(&e)?, delete, id],
        )?;
        tx.execute("UPDATE memory SET valid=0 WHERE (scene,id) IN (SELECT scene,memory_id FROM sources WHERE request=?)",[id])?;
        let narrative_sources: Option<String> = tx
            .query_row(
                "SELECT body FROM resources WHERE kind='narrative_sources' AND id=?",
                [&e.conversation_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(body) = narrative_sources {
            let sources: Vec<Source> = serde_json::from_str(&body)?;
            if sources.iter().any(|s| s.request_id == id) {
                let body: String = tx.query_row(
                    "SELECT body FROM resources WHERE kind='conversation' AND id=?",
                    [&e.conversation_id],
                    |r| r.get(0),
                )?;
                let mut conversation: Conversation = serde_json::from_str(&body)?;
                conversation.narrative = Narrative::default();
                tx.execute(
                    "UPDATE resources SET body=?,revision=revision+1 WHERE kind='conversation' AND id=?",
                    params![serde_json::to_string(&conversation)?, e.conversation_id],
                )?;
            }
        }
        // Frozen work must never commit against revised evidence.
        tx.execute(
            "UPDATE batches SET result=? WHERE scene=? AND result IS NULL",
            params!["{\"invalidated\":true}", e.conversation_id],
        )?;
        if delete {
            tx.execute(
                "DELETE FROM turns WHERE scene=? AND request=?",
                params![e.conversation_id, id],
            )?;
        } else {
            let turn = Turn {
                request_id: id.into(),
                user: e.user.clone(),
                assistant: e.assistant.clone(),
            };
            tx.execute(
                "UPDATE turns SET body=? WHERE scene=? AND request=?",
                params![serde_json::to_string(&turn)?, e.conversation_id, id],
            )?;
        }
        tx.execute(
            "UPDATE runtime SET repair=1 WHERE scene=?",
            [&e.conversation_id],
        )?;
        Self::event_tx(
            &tx,
            &e.conversation_id,
            "memory.sources_invalidated",
            &json!({"request_id":id,"revision":e.revision,"repair_pending":true}),
        )?;
        tx.commit()?;
        Ok(e)
    }
    pub fn configure_extraction(&mut self, conversation: &str, interval: u32) -> Result<()> {
        let _: Conversation = self.get("conversation", conversation)?;
        self.scheduler.configure(conversation, interval)
    }
    pub fn extraction_status(&self, conversation: &str) -> Result<Value> {
        let repair: bool = self
            .scheduler
            .db
            .query_row(
                "SELECT repair FROM runtime WHERE scene=?",
                [conversation],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        Ok(json!({"schedule":self.scheduler.status(conversation)?,"repair_pending":repair}))
    }
    pub fn begin_extraction(
        &mut self,
        conversation: &str,
        early: bool,
    ) -> Result<Option<Extraction>> {
        let _: Conversation = self.get("conversation", conversation)?;
        let repair: bool = self
            .scheduler
            .db
            .query_row(
                "SELECT repair FROM runtime WHERE scene=?",
                [conversation],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        let revision = self.revision(conversation)?;
        if repair {
            let (affected_ids, evidence) = repair_evidence(&self.scheduler.db, conversation)?;
            return Ok(Some(Extraction {
                conversation_id: conversation.into(),
                revision,
                batch_id: None,
                repair: true,
                affected_ids,
                evidence,
            }));
        }
        let Some(batch) = self.scheduler.begin(conversation, early)? else {
            return Ok(None);
        };
        let evidence = batch
            .turns
            .iter()
            .map(|t| {
                let body: String = self.scheduler.db.query_row(
                    "SELECT body FROM exchanges WHERE request=? AND deleted=0",
                    [&t.request_id],
                    |r| r.get(0),
                )?;
                Ok(serde_json::from_str(&body)?)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(Extraction {
            conversation_id: conversation.into(),
            revision,
            batch_id: Some(batch.id),
            repair: false,
            affected_ids: vec![],
            evidence,
        }))
    }
    pub fn apply_extraction(&mut self, job: &Extraction, patch: &MemoryPatch) -> Result<()> {
        let evidence: BTreeMap<_, _> = job
            .evidence
            .iter()
            .map(|e| (e.request_id.as_str(), e.revision))
            .collect();
        let mut ids = BTreeSet::new();
        if patch.upsert.len() > 256 {
            return Err(Error::Invalid("too many patch documents"));
        }
        for m in &patch.upsert {
            m.validate()?;
            if !ids.insert(&m.id)
                || m.sources
                    .iter()
                    .any(|s| evidence.get(s.request_id.as_str()) != Some(&s.revision))
            {
                return Err(Error::Invalid("duplicate document or unsupported evidence"));
            }
        }
        let tx = self
            .scheduler
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(id) = job.batch_id {
            let (conversation, payload, result): (String, String, Option<String>) = tx.query_row(
                "SELECT scene,payload,result FROM batches WHERE id=?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            if conversation != job.conversation_id || result.is_some() {
                return Err(Error::Conflict("batch closed or scene differs"));
            }
            let frozen: Vec<Turn> = serde_json::from_str(&payload)?;
            if frozen.len() != job.evidence.len()
                || frozen.iter().zip(&job.evidence).any(|(t, e)| {
                    t.request_id != e.request_id || t.user != e.user || t.assistant != e.assistant
                })
            {
                return Err(Error::Conflict("extraction evidence changed"));
            }
        } else if !job.repair {
            return Err(Error::Invalid("missing extraction batch"));
        }
        let current_revision: i64 = tx.query_row(
            "SELECT revision FROM runtime WHERE scene=?",
            [&job.conversation_id],
            |r| r.get(0),
        )?;
        if job.repair && current_revision != job.revision {
            return Err(Error::Conflict("repair evidence changed; retry"));
        }
        if job.repair {
            let (ids, expected) = repair_evidence(&tx, &job.conversation_id)?;
            if ids != job.affected_ids
                || serde_json::to_value(expected)? != serde_json::to_value(&job.evidence)?
            {
                return Err(Error::Conflict("repair evidence is incomplete or changed"));
            }
        }
        for e in &job.evidence {
            let (revision, deleted): (i64, bool) = tx.query_row(
                "SELECT revision,deleted FROM exchanges WHERE request=? AND scene=?",
                params![e.request_id, job.conversation_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if deleted || revision != e.revision {
                return Err(Error::Conflict("source changed during extraction"));
            }
        }
        let activity: i64 = tx.query_row(
            "SELECT activity FROM runtime WHERE scene=?",
            [&job.conversation_id],
            |r| r.get(0),
        )?;
        if job.repair {
            for m in &patch.upsert {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM memory WHERE scene=? AND id=? AND valid=1)",
                    params![job.conversation_id, m.id],
                    |r| r.get(0),
                )?;
                if exists {
                    return Err(Error::Conflict(
                        "repair cannot replace an unaffected memory",
                    ));
                }
            }
            for id in &job.affected_ids {
                tx.execute(
                    "DELETE FROM sources WHERE scene=? AND memory_id=?",
                    params![job.conversation_id, id],
                )?;
                tx.execute(
                    "DELETE FROM memory WHERE scene=? AND id=?",
                    params![job.conversation_id, id],
                )?;
            }
        }
        for m in &patch.upsert {
            tx.execute("INSERT INTO memory(scene,id,body,last_hit) VALUES (?,?,?,?) ON CONFLICT(scene,id) DO UPDATE SET body=excluded.body,valid=1,archived=0,last_hit=excluded.last_hit",params![job.conversation_id,m.id,serde_json::to_string(m)?,activity])?;
            tx.execute(
                "DELETE FROM sources WHERE scene=? AND memory_id=?",
                params![job.conversation_id, m.id],
            )?;
            for s in &m.sources {
                tx.execute(
                    "INSERT INTO sources(scene,memory_id,request,revision) VALUES (?,?,?,?)",
                    params![job.conversation_id, m.id, s.request_id, s.revision],
                )?;
            }
        }
        // A late batch may add facts, but cannot roll the current narrative back.
        if let Some(narrative) = &patch.narrative
            && current_revision == job.revision
        {
            let body: String = tx.query_row(
                "SELECT body FROM resources WHERE kind='conversation' AND id=?",
                [&job.conversation_id],
                |r| r.get(0),
            )?;
            let mut conversation: Conversation = serde_json::from_str(&body)?;
            conversation.narrative = narrative.clone();
            let sources: Vec<_> = job
                .evidence
                .iter()
                .map(|e| Source {
                    request_id: e.request_id.clone(),
                    revision: e.revision,
                })
                .collect();
            tx.execute("INSERT INTO resources(kind,id,body) VALUES ('narrative_sources',?,?) ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body",params![job.conversation_id,serde_json::to_string(&sources)?])?;
            tx.execute(
                "UPDATE resources SET body=?,revision=revision+1 WHERE kind='conversation' AND id=?",
                params![serde_json::to_string(&conversation)?, job.conversation_id],
            )?;
        }
        if let Some(id) = job.batch_id {
            for e in &job.evidence {
                tx.execute(
                    "UPDATE turns SET done=1 WHERE scene=? AND request=?",
                    params![job.conversation_id, e.request_id],
                )?;
            }
            tx.execute(
                "UPDATE batches SET result=? WHERE id=?",
                params![serde_json::to_string(patch)?, id],
            )?;
        }
        if job.repair {
            tx.execute(
                "UPDATE runtime SET repair=0 WHERE scene=?",
                [&job.conversation_id],
            )?;
        }
        Self::event_tx(
            &tx,
            &job.conversation_id,
            if job.repair {
                "memory.repaired"
            } else {
                "memory.distilled"
            },
            &json!({"batch":job.batch_id,"documents":patch.upsert.len()}),
        )?;
        tx.commit()?;
        Ok(())
    }
    fn lifecycle_tx(tx: &rusqlite::Transaction<'_>, conversation: &str) -> Result<()> {
        let activity: i64 = tx.query_row(
            "SELECT activity FROM runtime WHERE scene=?",
            [conversation],
            |r| r.get(0),
        )?;
        let rows = {
            let mut q = tx.prepare(
                "SELECT id,body,last_hit,archived FROM memory WHERE scene=? AND valid=1",
            )?;
            q.query_map([conversation], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, bool>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let linked: BTreeSet<String> = rows
            .iter()
            .filter_map(|(_, b, _, _)| serde_json::from_str::<Memory>(b).ok())
            .flat_map(|m| m.links)
            .collect();
        for (id, body, last_hit, archived) in rows {
            let mut m: Memory = serde_json::from_str(&body)?;
            if m.core || m.tags.iter().any(|t| t == "promise") || linked.contains(&id) {
                continue;
            }
            let age = activity - last_hit;
            if age >= 48 && age % 48 == 0 {
                m.weight *= 0.9;
            }
            if archived && age >= 240 && m.weight < 0.2 {
                tx.execute(
                    "DELETE FROM sources WHERE scene=? AND memory_id=?",
                    params![conversation, id],
                )?;
                tx.execute(
                    "DELETE FROM memory WHERE scene=? AND id=?",
                    params![conversation, id],
                )?;
                Self::event_tx(tx, conversation, "memory.forgotten", &json!({"id":id}))?;
            } else {
                tx.execute(
                    "UPDATE memory SET body=?,archived=? WHERE scene=? AND id=?",
                    params![
                        serde_json::to_string(&m)?,
                        archived || (m.kind == "event" && m.weight < 0.3),
                        conversation,
                        id
                    ],
                )?;
            }
        }
        Ok(())
    }
    pub fn snapshot(&mut self, conversation_id: &str, query: &str) -> Result<Snapshot> {
        let conversation: Conversation = self.get("conversation", conversation_id)?;
        let scope = if conversation.injection_scope.is_empty() {
            vec![conversation_id.to_string()]
        } else {
            conversation.injection_scope.clone()
        };
        let mut candidates = Vec::new();
        let mut scope_revisions = BTreeMap::new();
        let tokens = tokens(query);
        for conversation in &scope {
            scope_revisions.insert(conversation.clone(), self.revision(conversation)?);
            let mut q = self.scheduler.db.prepare(
                "SELECT body FROM memory WHERE scene=? AND valid=1 AND archived=0 ORDER BY id",
            )?;
            for row in q.query_map([conversation], |r| r.get::<_, String>(0))? {
                let m: Memory = serde_json::from_str(&row?)?;
                let words = tokens_for(&m);
                let overlap = tokens.intersection(&words).count();
                let score = (overlap as f64) + m.weight + if m.core { 5.0 } else { 0.0 };
                candidates.push((conversation.clone(), m, score, overlap > 0));
            }
        }
        candidates.sort_by(|a, b| {
            b.2.total_cmp(&a.2)
                .then(a.0.cmp(&b.0))
                .then(a.1.id.cmp(&b.1.id))
        });
        let mut chosen = Vec::new();
        let mut seen = BTreeSet::new();
        let mut budget = 16_000usize;
        for (conversation, m, _, matches) in &candidates {
            if chosen.len() >= 20 {
                break;
            }
            if (!matches && !m.core) || m.body.len() > budget {
                continue;
            }
            budget -= m.body.len();
            seen.insert((conversation.clone(), m.id.clone()));
            chosen.push((conversation.clone(), m.clone()));
        }
        let direct = chosen.clone();
        for (conversation, m) in direct.iter().take(3) {
            for link in m.links.iter().take(3) {
                if chosen.len() >= 25 {
                    break;
                }
                if let Some((s, linked, _, _)) = candidates
                    .iter()
                    .find(|(s, m, _, _)| s == conversation && &m.id == link)
                    && linked.body.len() <= budget
                    && seen.insert((s.clone(), linked.id.clone()))
                {
                    budget -= linked.body.len();
                    chosen.push((s.clone(), linked.clone()));
                }
            }
        }
        let memories: Vec<_> = chosen.iter().map(|(_, m)| m.clone()).collect();
        let dimensions = project(&memories, &conversation.narrative);
        let mut snapshot = Snapshot {
            id: id(),
            conversation_id: conversation_id.into(),
            revision: self.revision(conversation_id)?,
            scope_revisions,
            narrative: conversation.narrative,
            dimensions,
            memories,
            context: String::new(),
        };
        snapshot.context = format!(
            "[STATE_CONTEXT]\n{}\n[/STATE_CONTEXT]",
            serde_json::to_string(
                &json!({"dimensions":snapshot.dimensions,"conversation":snapshot.narrative,"memories":snapshot.memories})
            )?
        );
        self.scheduler.db.execute(
            "INSERT INTO snapshots(id,scene,revision,body) VALUES (?,?,?,?)",
            params![
                snapshot.id,
                conversation_id,
                snapshot.revision,
                serde_json::to_string(&snapshot)?
            ],
        )?;
        for (conversation, m) in chosen.iter().take(5) {
            self.scheduler.db.execute("UPDATE memory SET last_hit=(SELECT activity FROM runtime WHERE scene=?) WHERE scene=? AND id=?",params![conversation,conversation,m.id])?;
        }
        Ok(snapshot)
    }
}
fn repair_evidence(
    db: &rusqlite::Connection,
    conversation: &str,
) -> Result<(Vec<String>, Vec<Exchange>)> {
    let mut q = db.prepare("SELECT id FROM memory WHERE scene=? AND valid=0 ORDER BY id")?;
    let ids = q
        .query_map([conversation], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut q=db.prepare("SELECT e.body FROM exchanges e WHERE e.scene=? AND e.deleted=0 AND (e.request IN (SELECT s.request FROM sources s JOIN memory m ON m.scene=s.scene AND m.id=s.memory_id WHERE m.scene=? AND m.valid=0) OR e.request IN (SELECT json_extract(j.value,'$.request_id') FROM resources r,json_each(r.body) j WHERE r.kind='narrative_sources' AND r.id=e.scene)) ORDER BY e.seq")?;
    let evidence = q
        .query_map(params![conversation, conversation], |r| {
            r.get::<_, String>(0)
        })?
        .map(|r| Ok(serde_json::from_str(&r?)?))
        .collect::<Result<Vec<_>>>()?;
    Ok((ids, evidence))
}
fn tokens(s: &str) -> BTreeSet<String> {
    let normalized = s.nfkc().collect::<String>().to_lowercase();
    let mut out: BTreeSet<_> = normalized
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let chars: Vec<_> = normalized.chars().collect();
    for w in chars.windows(2) {
        if w.iter().all(|c| *c >= '\u{3400}' && *c <= '\u{9fff}') {
            out.insert(w.iter().collect());
        }
    }
    out
}
fn tokens_for(m: &Memory) -> BTreeSet<String> {
    tokens(&format!(
        "{} {} {} {} {}",
        m.id,
        m.title,
        m.body,
        m.tags.join(" "),
        m.aliases.join(" ")
    ))
}
fn project(memories: &[Memory], n: &Narrative) -> BTreeMap<String, Vec<String>> {
    let tags: BTreeSet<_> = memories
        .iter()
        .flat_map(|m| m.tags.iter().map(String::as_str))
        .collect();
    let mut d: BTreeMap<String, Vec<String>> = [
        "relational_stance",
        "emotional_tone",
        "scene_constraint",
        "physiological_state",
        "epistemic_state",
    ]
    .into_iter()
    .map(|k| (k.into(), vec![]))
    .collect();
    if tags.contains("conflict") || tags.contains("tension") {
        d.get_mut("relational_stance")
            .unwrap()
            .push("Preserve established tension; do not invent reconciliation.".into());
    } else if tags.contains("trust") || tags.contains("attachment") {
        d.get_mut("relational_stance")
            .unwrap()
            .push("Preserve established closeness.".into());
    }
    if tags.contains("loss") || tags.contains("regret") {
        d.get_mut("emotional_tone")
            .unwrap()
            .push("Keep a measured tone appropriate to loss.".into());
    } else if tags.contains("danger") || tags.contains("fear") {
        d.get_mut("emotional_tone")
            .unwrap()
            .push("Attend to immediate danger.".into());
    }
    d.insert("scene_constraint".into(), n.constraints.clone());
    if tags.contains("wounded") {
        d.get_mut("physiological_state")
            .unwrap()
            .push("Respect established injury and physical limitations.".into());
    }
    if memories
        .iter()
        .any(|m| m.tags.iter().any(|t| t == "secret") && !m.tags.iter().any(|t| t == "witness"))
    {
        d.get_mut("epistemic_state")
            .unwrap()
            .push("Do not treat an unwitnessed secret as character knowledge.".into());
    }
    d
}
