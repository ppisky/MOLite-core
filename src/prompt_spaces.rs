//! Named prompt resources with compiled defaults and persistent local overrides.
use crate::{Error, Result, store::Store};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_PROMPT_SPACE_BYTES: usize = 256 * 1024;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromptSpaceId {
    Assistant,
    RoleplayDirector,
    MemoryDistillation,
    VisionFallback,
}
impl PromptSpaceId {
    pub const ALL: [Self; 4] = [
        Self::Assistant,
        Self::RoleplayDirector,
        Self::MemoryDistillation,
        Self::VisionFallback,
    ];
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::RoleplayDirector => "roleplay_director",
            Self::MemoryDistillation => "memory_distillation",
            Self::VisionFallback => "vision_fallback",
        }
    }
    pub const fn default_content(self) -> &'static str {
        match self {
            Self::Assistant => include_str!("prompts/assistant.md"),
            Self::RoleplayDirector => include_str!("prompts/roleplay_director.md"),
            Self::MemoryDistillation => include_str!("prompts/memory_distillation.md"),
            Self::VisionFallback => include_str!("prompts/vision_fallback.md"),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSpace {
    pub id: PromptSpaceId,
    pub content: String,
    pub source: String,
    pub revision: String,
}
impl Store {
    pub fn prompt_space(&self, id: PromptSpaceId) -> Result<PromptSpace> {
        let stored: Option<String> = self
            .scheduler
            .db
            .query_row(
                "SELECT body FROM resources WHERE kind='prompt_space' AND id=?",
                [id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        let content = stored
            .as_deref()
            .map(serde_json::from_str::<String>)
            .transpose()?
            .unwrap_or_else(|| id.default_content().into());
        let revision = hex::encode(Sha256::digest(content.as_bytes()));
        Ok(PromptSpace {
            id,
            content,
            source: if stored.is_some() {
                "override"
            } else {
                "builtin"
            }
            .into(),
            revision,
        })
    }
    pub fn prompt_spaces(&self) -> Result<Vec<PromptSpace>> {
        PromptSpaceId::ALL
            .into_iter()
            .map(|id| self.prompt_space(id))
            .collect()
    }
    pub fn replace_prompt_space(
        &mut self,
        id: PromptSpaceId,
        content: String,
        expected: Option<&str>,
    ) -> Result<PromptSpace> {
        if content.trim().is_empty() || content.len() > MAX_PROMPT_SPACE_BYTES {
            return Err(Error::Invalid("prompt must contain 1..=262144 UTF-8 bytes"));
        }
        let tx = self
            .scheduler
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let old: Option<String> = tx
            .query_row(
                "SELECT body FROM resources WHERE kind='prompt_space' AND id=?",
                [id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        let old = old
            .as_deref()
            .map(serde_json::from_str::<String>)
            .transpose()?
            .unwrap_or_else(|| id.default_content().into());
        if expected.is_some_and(|r| r != hex::encode(Sha256::digest(old.as_bytes()))) {
            return Err(Error::Conflict("prompt revision changed"));
        }
        if content == id.default_content() {
            tx.execute(
                "DELETE FROM resources WHERE kind='prompt_space' AND id=?",
                [id.as_str()],
            )?;
        } else {
            tx.execute("INSERT INTO resources(kind,id,body) VALUES ('prompt_space',?,?) ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body,revision=resources.revision+1",params![id.as_str(),serde_json::to_string(&content)?])?;
        }
        tx.commit()?;
        self.prompt_space(id)
    }
    pub fn reset_prompt_space(
        &mut self,
        id: PromptSpaceId,
        expected: Option<&str>,
    ) -> Result<PromptSpace> {
        self.replace_prompt_space(id, id.default_content().into(), expected)
    }
}
