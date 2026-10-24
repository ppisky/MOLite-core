//! MOLite domain: scene-scoped state, independent reusable model groups.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub fn id() -> String {
    uuid::Uuid::now_v7().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    ChatCompletions,
    Responses,
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub id: String,
    pub base_url: String,
    /// Environment variable containing the secret. Secrets never enter exports.
    pub api_key_env: Option<String>,
    pub protocols: Vec<Protocol>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub id: String,
    pub provider_id: String,
    pub model: String,
    #[serde(default)]
    pub vision: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelGroup {
    pub id: String,
    pub model_id: String,
    pub protocol: Protocol,
    /// None uses the purpose default; explicit values are honored for every use.
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default = "empty_object")]
    pub parameters: Value,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
}
fn timeout() -> u64 {
    120
}
fn empty_object() -> Value {
    json!({})
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Purposes {
    pub conversation: String,
    pub memory: String,
    pub vision_fallback: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Classification {
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Character {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author_name: String,
    pub character_markdown: String,
    #[serde(default)]
    pub user_markdown: String,
    #[serde(default)]
    pub opening_markdown: Option<String>,
    #[serde(default)]
    pub classification: Classification,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Narrative {
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub participants: Vec<String>,
    #[serde(default)]
    pub focus: String,
    #[serde(default)]
    pub open_threads: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub character_ids: Vec<String>,
    /// Explicit conversation IDs; empty selects this conversation only.
    #[serde(default)]
    pub injection_scope: Vec<String>,
    #[serde(default)]
    pub narrative: Narrative,
    #[serde(default)]
    pub ended: bool,
    #[serde(default)]
    pub classification: Classification,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataFile {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub sha256: String,
    pub bytes: u64,
    #[serde(default)]
    pub classification: Classification,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub request_id: String,
    pub revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Memory {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub core: bool,
    #[serde(default = "weight")]
    pub weight: f64,
    pub sources: Vec<Source>,
}
fn weight() -> f64 {
    1.0
}
impl Memory {
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || !["character", "relationship", "event", "world", "current"]
                .contains(&self.kind.as_str())
            || self.body.is_empty()
            || self.body.len() > 100_000
            || !self.weight.is_finite()
            || !(0.0..=1.0).contains(&self.weight)
            || self.sources.is_empty()
        {
            return Err(Error::Invalid("invalid DMW document or missing provenance"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPatch {
    #[serde(default)]
    pub upsert: Vec<Memory>,
    #[serde(default)]
    pub narrative: Option<Narrative>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exchange {
    pub request_id: String,
    pub conversation_id: String,
    pub revision: i64,
    pub user: String,
    pub assistant: String,
    pub attachments: Vec<String>,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub sequence: i64,
    pub conversation_id: String,
    pub kind: String,
    pub data: Value,
}
