use crate::{Error, Result, domain::Conversation, store::Store};
use rusqlite::params;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
impl Store {
    pub(crate) fn upgrade_conversations(&mut self) -> Result<()> {
        let legacy = self.list("scene")?;
        if legacy.is_empty() {
            return Ok(());
        }
        let records = self.list("conversation")?;
        let mut mapping = BTreeMap::new();
        let mut merged = Vec::new();
        let mut used: BTreeSet<String> = records
            .iter()
            .filter(|r| r.get("scene_id").is_none())
            .filter_map(|r| r["id"].as_str().map(str::to_owned))
            .collect();
        for state in legacy {
            let old = state["id"]
                .as_str()
                .ok_or(Error::Invalid("invalid legacy conversation state"))?;
            let owners: Vec<_> = records
                .iter()
                .filter(|c| c["scene_id"].as_str() == Some(old))
                .collect();
            if owners.len() > 1 {
                return Err(Error::Conflict(
                    "legacy state is shared by multiple conversations; automatic merge would mix provenance",
                ));
            }
            let mut unified = state.clone();
            if let Some(record) = owners.first() {
                for key in ["id", "title", "character_ids", "classification"] {
                    if let Some(value) = record.get(key) {
                        unified[key] = value.clone();
                    }
                }
            }
            let id = unified["id"]
                .as_str()
                .ok_or(Error::Invalid("invalid legacy conversation"))?
                .to_string();
            if !used.insert(id.clone()) {
                return Err(Error::Conflict("legacy conversation IDs collide"));
            }
            mapping.insert(old.to_string(), id);
            merged.push(unified);
        }
        for unified in &mut merged {
            if let Some(scope) = unified
                .get_mut("injection_scope")
                .and_then(Value::as_array_mut)
            {
                for id in scope {
                    if let Some(new) = id.as_str().and_then(|s| mapping.get(s)) {
                        *id = Value::String(new.clone());
                    }
                }
            }
            let _: Conversation = serde_json::from_value(unified.clone())?;
        }
        let tx = self.scheduler.db.transaction()?;
        tx.execute_batch("PRAGMA defer_foreign_keys=ON")?;
        let staging: Vec<_> = mapping
            .iter()
            .map(|(old, new)| {
                (
                    old.clone(),
                    new.clone(),
                    format!("upgrade:{}", uuid::Uuid::now_v7()),
                )
            })
            .collect();
        for (old, _, temporary) in &staging {
            for table in [
                "turns",
                "batches",
                "exchanges",
                "memory",
                "sources",
                "runtime",
                "events",
                "snapshots",
            ] {
                tx.execute(
                    &format!("UPDATE {table} SET scene=? WHERE scene=?"),
                    params![temporary, old],
                )?;
            }
            tx.execute("UPDATE scenes SET id=? WHERE id=?", params![temporary, old])?;
            move_repairs(&tx, old, temporary)?;

            tx.execute(
                "UPDATE resources SET id=? WHERE kind='narrative_sources' AND id=?",
                params![temporary, old],
            )?;
        }
        for (_, new, temporary) in &staging {
            for table in [
                "turns",
                "batches",
                "exchanges",
                "memory",
                "sources",
                "runtime",
                "events",
                "snapshots",
            ] {
                tx.execute(
                    &format!("UPDATE {table} SET scene=? WHERE scene=?"),
                    params![new, temporary],
                )?;
            }
            tx.execute("UPDATE scenes SET id=? WHERE id=?", params![new, temporary])?;
            move_repairs(&tx, temporary, new)?;

            tx.execute(
                "UPDATE resources SET id=? WHERE kind='narrative_sources' AND id=?",
                params![new, temporary],
            )?;
        }
        for unified in merged {
            tx.execute("INSERT INTO resources(kind,id,body) VALUES ('conversation',?,?) ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body",params![unified["id"].as_str().unwrap(),serde_json::to_string(&unified)?])?;
        }
        // Prepared requests retain their complete messages and model configuration.
        let rows = {
            let mut q = tx.prepare("SELECT id,input FROM requests")?;
            q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (id, body) in rows {
            let mut value: Value = serde_json::from_str(&body)?;
            if let Some(new) = value["scene"].as_str().and_then(|s| mapping.get(s)) {
                value["conversation_id"] = Value::String(new.clone());
            }
            if let Some(object) = value.as_object_mut() {
                object.remove("scene");
            }
            if let Some(scope) = value
                .get_mut("scope_revisions")
                .and_then(Value::as_object_mut)
            {
                let old = std::mem::take(scope);
                for (key, revision) in old {
                    scope.insert(mapping.get(&key).cloned().unwrap_or(key), revision);
                }
            }
            tx.execute(
                "UPDATE requests SET input=? WHERE id=?",
                params![serde_json::to_string(&value)?, id],
            )?;
        }
        tx.execute(
            "UPDATE exchanges SET body=json_remove(body,'$.scene_id')",
            [],
        )?;
        tx.execute(
            "UPDATE events SET kind='conversation.updated' WHERE kind='scene.updated'",
            [],
        )?;
        tx.execute(
            "UPDATE events SET kind='conversation.deleted' WHERE kind='scene.deleted'",
            [],
        )?;
        tx.execute("DELETE FROM resources WHERE kind='scene'", [])?;
        tx.commit()?;
        Ok(())
    }
}

fn move_repairs(tx: &rusqlite::Transaction<'_>, old: &str, new: &str) -> Result<()> {
    let keys = {
        let mut q = tx.prepare("SELECT id FROM maintenance_requests")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for key in keys {
        if let Some((owner, revision)) = Store::repair_key_parts(&key)
            && owner == old
        {
            tx.execute(
                "UPDATE maintenance_requests SET id=? WHERE id=?",
                params![format!("repair:{new}:{revision}"), key],
            )?;
        }
    }
    Ok(())
}
