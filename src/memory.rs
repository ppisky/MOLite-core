//! Transactional DMW and the DMW-only MO State v2 runtime profile.
use crate::{Error, Result, Turn, domain::*, store::Store};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extraction {
    pub scene_id: String,
    pub revision: i64,
    pub batch_id: Option<i64>,
    pub repair: bool,
    pub evidence: Vec<Exchange>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub scene_id: String,
    pub revision: i64,
    pub scope_revisions: BTreeMap<String, i64>,
    pub narrative: Narrative,
    pub dimensions: BTreeMap<String, Vec<String>>,
    pub memories: Vec<Memory>,
    pub context: String,
}

impl Store {
    pub fn commit_exchange(&mut self, e: &Exchange, expected: i64) -> Result<()> {
        let c: Conversation = self.get("conversation", &e.conversation_id)?;
        let s: Scene = self.get("scene", &c.scene_id)?;
        if e.scene_id != c.scene_id
            || e.deleted
            || e.revision != 1
            || e.request_id.is_empty()
            || e.assistant.trim().is_empty()
            || s.ended
        {
            return Err(Error::Invalid("invalid exchange or ended scene"));
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
                [&e.scene_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if revision != expected {
            return Err(Error::Conflict("scene changed during generation; retry"));
        }
        tx.execute(
            "INSERT INTO exchanges(request,conversation,scene,revision,body) VALUES (?,?,?,1,?)",
            params![e.request_id, e.conversation_id, e.scene_id, body],
        )?;
        let t = Turn {
            request_id: e.request_id.clone(),
            user: e.user.clone(),
            assistant: e.assistant.clone(),
        };
        tx.execute(
            "INSERT INTO turns(scene,request,body) VALUES (?,?,?)",
            params![e.scene_id, e.request_id, serde_json::to_string(&t)?],
        )?;
        tx.execute(
            "UPDATE runtime SET activity=activity+1 WHERE scene=?",
            [&e.scene_id],
        )?;
        Self::event_tx(
            &tx,
            &e.scene_id,
            "exchange.committed",
            &json!({"request_id":e.request_id}),
        )?;
        Self::lifecycle_tx(&tx, &e.scene_id)?;
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
        // Frozen work must never commit against revised evidence.
        tx.execute(
            "UPDATE batches SET result=? WHERE scene=? AND result IS NULL",
            params!["{\"invalidated\":true}", e.scene_id],
        )?;
        if delete {
            tx.execute(
                "DELETE FROM turns WHERE scene=? AND request=?",
                params![e.scene_id, id],
            )?;
        } else {
            let turn = Turn {
                request_id: id.into(),
                user: e.user.clone(),
                assistant: e.assistant.clone(),
            };
            tx.execute(
                "UPDATE turns SET body=? WHERE scene=? AND request=?",
                params![serde_json::to_string(&turn)?, e.scene_id, id],
            )?;
        }
        tx.execute("UPDATE runtime SET repair=1 WHERE scene=?", [&e.scene_id])?;
        Self::event_tx(
            &tx,
            &e.scene_id,
            "memory.sources_invalidated",
            &json!({"request_id":id,"revision":e.revision,"repair_pending":true}),
        )?;
        tx.commit()?;
        Ok(e)
    }
    pub fn configure_extraction(&mut self, scene: &str, interval: u32) -> Result<()> {
        let _: Scene = self.get("scene", scene)?;
        self.scheduler.configure(scene, interval)
    }
    pub fn extraction_status(&self, scene: &str) -> Result<Value> {
        let repair: bool = self
            .scheduler
            .db
            .query_row("SELECT repair FROM runtime WHERE scene=?", [scene], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or(false);
        Ok(json!({"schedule":self.scheduler.status(scene)?,"repair_pending":repair}))
    }
    pub fn begin_extraction(&mut self, scene: &str, early: bool) -> Result<Option<Extraction>> {
        let _: Scene = self.get("scene", scene)?;
        let repair: bool = self
            .scheduler
            .db
            .query_row("SELECT repair FROM runtime WHERE scene=?", [scene], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or(false);
        let revision = self.revision(scene)?;
        if repair {
            let mut q=self.scheduler.db.prepare("SELECT e.body FROM exchanges e JOIN turns t ON t.scene=e.scene AND t.request=e.request WHERE e.scene=? AND e.deleted=0 AND t.done=1 ORDER BY e.seq")?;
            let evidence = q
                .query_map([scene], |r| r.get::<_, String>(0))?
                .map(|r| Ok(serde_json::from_str(&r?)?))
                .collect::<Result<Vec<_>>>()?;
            return Ok(Some(Extraction {
                scene_id: scene.into(),
                revision,
                batch_id: None,
                repair: true,
                evidence,
            }));
        }
        let Some(batch) = self.scheduler.begin(scene, early)? else {
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
            scene_id: scene.into(),
            revision,
            batch_id: Some(batch.id),
            repair: false,
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
            let (scene, payload, result): (String, String, Option<String>) = tx.query_row(
                "SELECT scene,payload,result FROM batches WHERE id=?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            if scene != job.scene_id || result.is_some() {
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
            [&job.scene_id],
            |r| r.get(0),
        )?;
        if job.repair && current_revision != job.revision {
            return Err(Error::Conflict("repair evidence changed; retry"));
        }
        for e in &job.evidence {
            let (revision, deleted): (i64, bool) = tx.query_row(
                "SELECT revision,deleted FROM exchanges WHERE request=? AND scene=?",
                params![e.request_id, job.scene_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if deleted || revision != e.revision {
                return Err(Error::Conflict("source changed during extraction"));
            }
        }
        let activity: i64 = tx.query_row(
            "SELECT activity FROM runtime WHERE scene=?",
            [&job.scene_id],
            |r| r.get(0),
        )?;
        if job.repair {
            tx.execute("DELETE FROM sources WHERE scene=?", [&job.scene_id])?;
            tx.execute("DELETE FROM memory WHERE scene=?", [&job.scene_id])?;
        }
        for m in &patch.upsert {
            tx.execute("INSERT INTO memory(scene,id,body,last_hit) VALUES (?,?,?,?) ON CONFLICT(scene,id) DO UPDATE SET body=excluded.body,valid=1,archived=0,last_hit=excluded.last_hit",params![job.scene_id,m.id,serde_json::to_string(m)?,activity])?;
            tx.execute(
                "DELETE FROM sources WHERE scene=? AND memory_id=?",
                params![job.scene_id, m.id],
            )?;
            for s in &m.sources {
                tx.execute(
                    "INSERT INTO sources(scene,memory_id,request,revision) VALUES (?,?,?,?)",
                    params![job.scene_id, m.id, s.request_id, s.revision],
                )?;
            }
        }
        // A late batch may add facts, but cannot roll the current narrative back.
        if let Some(narrative) = &patch.narrative
            && current_revision == job.revision
        {
            let body: String = tx.query_row(
                "SELECT body FROM resources WHERE kind='scene' AND id=?",
                [&job.scene_id],
                |r| r.get(0),
            )?;
            let mut scene: Scene = serde_json::from_str(&body)?;
            scene.narrative = narrative.clone();
            tx.execute(
                "UPDATE resources SET body=?,revision=revision+1 WHERE kind='scene' AND id=?",
                params![serde_json::to_string(&scene)?, job.scene_id],
            )?;
        }
        if let Some(id) = job.batch_id {
            for e in &job.evidence {
                tx.execute(
                    "UPDATE turns SET done=1 WHERE scene=? AND request=?",
                    params![job.scene_id, e.request_id],
                )?;
            }
            tx.execute(
                "UPDATE batches SET result=? WHERE id=?",
                params![serde_json::to_string(patch)?, id],
            )?;
        }
        if job.repair {
            tx.execute("UPDATE runtime SET repair=0 WHERE scene=?", [&job.scene_id])?;
        }
        Self::event_tx(
            &tx,
            &job.scene_id,
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
    fn lifecycle_tx(tx: &rusqlite::Transaction<'_>, scene: &str) -> Result<()> {
        let activity: i64 =
            tx.query_row("SELECT activity FROM runtime WHERE scene=?", [scene], |r| {
                r.get(0)
            })?;
        let rows = {
            let mut q = tx.prepare(
                "SELECT id,body,last_hit,archived FROM memory WHERE scene=? AND valid=1",
            )?;
            q.query_map([scene], |r| {
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
                    params![scene, id],
                )?;
                tx.execute(
                    "DELETE FROM memory WHERE scene=? AND id=?",
                    params![scene, id],
                )?;
                Self::event_tx(tx, scene, "memory.forgotten", &json!({"id":id}))?;
            } else {
                tx.execute(
                    "UPDATE memory SET body=?,archived=? WHERE scene=? AND id=?",
                    params![
                        serde_json::to_string(&m)?,
                        archived || (m.kind == "event" && m.weight < 0.3),
                        scene,
                        id
                    ],
                )?;
            }
        }
        Ok(())
    }
    pub fn snapshot(&mut self, scene_id: &str, query: &str) -> Result<Snapshot> {
        let scene: Scene = self.get("scene", scene_id)?;
        let scope = if scene.injection_scope.is_empty() {
            vec![scene_id.to_string()]
        } else {
            scene.injection_scope.clone()
        };
        let mut candidates = Vec::new();
        let mut scope_revisions = BTreeMap::new();
        let tokens = tokens(query);
        for scene in &scope {
            scope_revisions.insert(scene.clone(), self.revision(scene)?);
            let mut q = self.scheduler.db.prepare(
                "SELECT body FROM memory WHERE scene=? AND valid=1 AND archived=0 ORDER BY id",
            )?;
            for row in q.query_map([scene], |r| r.get::<_, String>(0))? {
                let m: Memory = serde_json::from_str(&row?)?;
                let words = tokens_for(&m);
                let overlap = tokens.intersection(&words).count();
                let score = (overlap as f64) + m.weight + if m.core { 5.0 } else { 0.0 };
                candidates.push((scene.clone(), m, score, overlap > 0));
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
        for (scene, m, _, matches) in &candidates {
            if chosen.len() >= 20 {
                break;
            }
            if (!matches && !m.core) || m.body.len() > budget {
                continue;
            }
            budget -= m.body.len();
            seen.insert((scene.clone(), m.id.clone()));
            chosen.push((scene.clone(), m.clone()));
        }
        let direct = chosen.clone();
        for (scene, m) in direct.iter().take(3) {
            for link in m.links.iter().take(3) {
                if chosen.len() >= 25 {
                    break;
                }
                if let Some((s, linked, _, _)) = candidates
                    .iter()
                    .find(|(s, m, _, _)| s == scene && &m.id == link)
                    && linked.body.len() <= budget
                    && seen.insert((s.clone(), linked.id.clone()))
                {
                    budget -= linked.body.len();
                    chosen.push((s.clone(), linked.clone()));
                }
            }
        }
        let memories: Vec<_> = chosen.iter().map(|(_, m)| m.clone()).collect();
        let dimensions = project(&memories, &scene.narrative);
        let mut snapshot = Snapshot {
            id: id(),
            scene_id: scene_id.into(),
            revision: self.revision(scene_id)?,
            scope_revisions,
            narrative: scene.narrative,
            dimensions,
            memories,
            context: String::new(),
        };
        snapshot.context = format!(
            "[STATE_CONTEXT]\n{}\n[/STATE_CONTEXT]",
            serde_json::to_string(
                &json!({"dimensions":snapshot.dimensions,"scene":snapshot.narrative,"memories":snapshot.memories})
            )?
        );
        self.scheduler.db.execute(
            "INSERT INTO snapshots(id,scene,revision,body) VALUES (?,?,?,?)",
            params![
                snapshot.id,
                scene_id,
                snapshot.revision,
                serde_json::to_string(&snapshot)?
            ],
        )?;
        for (scene, m) in chosen.iter().take(5) {
            self.scheduler.db.execute("UPDATE memory SET last_hit=(SELECT activity FROM runtime WHERE scene=?) WHERE scene=? AND id=?",params![scene,scene,m.id])?;
        }
        Ok(snapshot)
    }
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
