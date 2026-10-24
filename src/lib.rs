//! MOLite's scene-scoped durable distillation scheduler.
//!
//! A model worker receives a frozen batch from `begin` and commits its validated
//! result with `complete`. A failed worker leaves the batch available for retry.
//! This crate currently implements scheduling, not the complete MO State runtime.
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub mod domain;
pub mod memory;
pub mod store;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(String),
    #[error("storage: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("encoding: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("invalid input: {0}")]
    Invalid(&'static str),
    #[error("conflict: {0}")]
    Conflict(&'static str),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    pub request_id: String,
    pub user: String,
    pub assistant: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    pub id: i64,
    pub scene_id: String,
    pub early: bool,
    pub turns: Vec<Turn>,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub every_turns: u32,
    pub pending_turns: u32,
    pub turns_until_due: u32,
    pub active_batch_id: Option<i64>,
}

pub struct Scheduler {
    pub(crate) db: Connection,
}

impl Scheduler {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch(
            "PRAGMA foreign_keys=ON;
             PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS scenes(id TEXT PRIMARY KEY, every_turns INTEGER NOT NULL DEFAULT 12 CHECK(every_turns BETWEEN 1 AND 200));
             CREATE TABLE IF NOT EXISTS turns(seq INTEGER PRIMARY KEY AUTOINCREMENT, scene TEXT NOT NULL REFERENCES scenes(id), request TEXT NOT NULL, body TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0, UNIQUE(scene,request));
             CREATE TABLE IF NOT EXISTS batches(id INTEGER PRIMARY KEY AUTOINCREMENT, scene TEXT NOT NULL REFERENCES scenes(id), early INTEGER NOT NULL, payload TEXT NOT NULL, result TEXT);
             CREATE UNIQUE INDEX IF NOT EXISTS one_active_batch ON batches(scene) WHERE result IS NULL;
             CREATE INDEX IF NOT EXISTS pending_turns ON turns(scene,done,seq);",
        )?;
        Ok(Self { db })
    }

    /// Changes the threshold for future batches, without rewriting staged work.
    pub fn configure(&mut self, scene: &str, every_turns: u32) -> Result<()> {
        if scene.trim().is_empty() || !(1..=200).contains(&every_turns) {
            return Err(Error::Invalid(
                "scene must be nonempty and interval within 1..=200",
            ));
        }
        self.db.execute(
            "INSERT INTO scenes(id,every_turns) VALUES (?,?) ON CONFLICT(id) DO UPDATE SET every_turns=excluded.every_turns",
            params![scene, every_turns],
        )?;
        Ok(())
    }

    /// Records one successfully committed complete exchange. Replays do not count.
    pub fn record(&mut self, scene: &str, turn: &Turn) -> Result<()> {
        if scene.trim().is_empty()
            || turn.request_id.trim().is_empty()
            || turn.assistant.trim().is_empty()
        {
            return Err(Error::Invalid(
                "scene, request ID and assistant response are required",
            ));
        }
        let body = serde_json::to_string(turn)?;
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("INSERT OR IGNORE INTO scenes(id) VALUES (?)", [scene])?;
        let old: Option<String> = tx
            .query_row(
                "SELECT body FROM turns WHERE scene=? AND request=?",
                params![scene, turn.request_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old) = old {
            if old != body {
                return Err(Error::Conflict("request ID reused with different content"));
            }
        } else {
            tx.execute(
                "INSERT INTO turns(scene,request,body) VALUES (?,?,?)",
                params![scene, turn.request_id, body],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// `early=true` captures the current pending tail (up to the configured batch
    /// size). Turns arriving during a model call remain pending for the next cycle.
    /// Existing failed/interrupted batches always resume with the original inputs.
    pub fn begin(&mut self, scene: &str, early: bool) -> Result<Option<Batch>> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let active: Option<(i64, bool, String)> = tx
            .query_row(
                "SELECT id,early,payload FROM batches WHERE scene=? AND result IS NULL",
                [scene],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((id, early, payload)) = active {
            return Ok(Some(Batch {
                id,
                scene_id: scene.into(),
                early,
                turns: serde_json::from_str(&payload)?,
            }));
        }
        let interval: Option<u32> = tx
            .query_row("SELECT every_turns FROM scenes WHERE id=?", [scene], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(interval) = interval else {
            return Ok(None);
        };
        let bodies = {
            let mut query =
                tx.prepare("SELECT body FROM turns WHERE scene=? AND done=0 ORDER BY seq LIMIT ?")?;
            query
                .query_map(params![scene, interval], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        if bodies.is_empty() || (!early && bodies.len() < interval as usize) {
            return Ok(None);
        }
        let turns: Vec<Turn> = bodies
            .iter()
            .map(|s| serde_json::from_str(s))
            .collect::<std::result::Result<_, _>>()?;
        tx.execute(
            "INSERT INTO batches(scene,early,payload) VALUES (?,?,?)",
            params![scene, early, serde_json::to_string(&turns)?],
        )?;
        let batch = Batch {
            id: tx.last_insert_rowid(),
            scene_id: scene.into(),
            early,
            turns,
        };
        tx.commit()?;
        Ok(Some(batch))
    }

    /// Stores the worker's validated result and acknowledges exactly the frozen
    /// evidence in one transaction. This is not itself a DMW patch validator.
    pub fn complete(&mut self, batch: &Batch, validated_result: &serde_json::Value) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let stored: Option<(String, bool, String, Option<String>)> = tx
            .query_row(
                "SELECT scene,early,payload,result FROM batches WHERE id=?",
                [batch.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((scene, early, payload, result)) = stored else {
            return Err(Error::Conflict("unknown batch"));
        };
        if scene != batch.scene_id
            || early != batch.early
            || serde_json::from_str::<Vec<Turn>>(&payload)? != batch.turns
        {
            return Err(Error::Conflict("batch evidence changed"));
        }
        let encoded = serde_json::to_string(validated_result)?;
        if let Some(result) = result {
            return if result == encoded {
                Ok(())
            } else {
                Err(Error::Conflict("completed batch result changed"))
            };
        }
        for turn in &batch.turns {
            tx.execute(
                "UPDATE turns SET done=1 WHERE scene=? AND request=?",
                params![scene, turn.request_id],
            )?;
        }
        tx.execute(
            "UPDATE batches SET result=? WHERE id=?",
            params![encoded, batch.id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn status(&self, scene: &str) -> Result<Status> {
        let every_turns = self
            .db
            .query_row("SELECT every_turns FROM scenes WHERE id=?", [scene], |r| {
                r.get::<_, u32>(0)
            })
            .optional()?
            .unwrap_or(12);
        let pending_turns = self.db.query_row(
            "SELECT COUNT(*) FROM turns WHERE scene=? AND done=0",
            [scene],
            |r| r.get::<_, u32>(0),
        )?;
        let active_batch_id = self
            .db
            .query_row(
                "SELECT id FROM batches WHERE scene=? AND result IS NULL",
                [scene],
                |r| r.get(0),
            )
            .optional()?;
        Ok(Status {
            every_turns,
            pending_turns,
            turns_until_due: every_turns.saturating_sub(pending_turns),
            active_batch_id,
        })
    }
}
