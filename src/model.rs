//! Typed provider wire conversion and bounded SSE decoding.
use crate::{Error, Result, domain::*};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub role: String,
    pub text: String,
    pub images: Vec<(String, String)>,
}
impl Message {
    pub fn text(role: &str, text: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            text: text.into(),
            images: vec![],
        }
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Connection {
    pub provider: Provider,
    pub model: Model,
    pub group: ModelGroup,
}
impl Connection {
    pub fn body(&self, messages: &[Message], stream: bool) -> Result<Value> {
        let mut body = self.group.parameters.clone();
        let mut items = vec![];
        let mut system = vec![];
        for m in messages {
            if !["user", "assistant", "system"].contains(&m.role.as_str()) {
                return Err(Error::Invalid("invalid role"));
            }
            if !m.images.is_empty() && !self.model.vision {
                return Err(Error::Invalid("model cannot receive images"));
            }
            let mut parts = vec![match self.group.protocol {
                Protocol::Responses => {
                    json!({"type":if m.role=="assistant"{"output_text"}else{"input_text"},"text":m.text})
                }
                _ => json!({"type":"text","text":m.text}),
            }];
            for (mime, data) in &m.images {
                if !["image/png", "image/jpeg", "image/webp", "image/gif"].contains(&mime.as_str())
                {
                    return Err(Error::Invalid("unsupported image MIME"));
                }
                parts.push(match self.group.protocol {
     Protocol::ChatCompletions=>json!({"type":"image_url","image_url":{"url":format!("data:{mime};base64,{data}")}}),
     Protocol::Responses=>json!({"type":"input_image","image_url":format!("data:{mime};base64,{data}")}),
     Protocol::Anthropic=>json!({"type":"image","source":{"type":"base64","media_type":mime,"data":data}}),
    });
            }
            if m.role == "system" && self.group.protocol == Protocol::Anthropic {
                system.extend(parts);
            } else {
                items.push(json!({"role":m.role,"content":parts}));
            }
        }
        body["model"] = json!(self.model.model);
        body["stream"] = json!(stream);
        match self.group.protocol {
            Protocol::Responses => {
                body["input"] = json!(items);
                body["store"] = json!(false);
            }
            Protocol::Anthropic => {
                body["messages"] = json!(items);
                body["system"] = json!(system);
                if body.get("max_tokens").is_none() {
                    body["max_tokens"] = json!(4096);
                }
            }
            Protocol::ChatCompletions => body["messages"] = json!(items),
        }
        Ok(body)
    }
    pub async fn complete(
        &self,
        client: &reqwest::Client,
        messages: &[Message],
        stream: bool,
        events: Option<&UnboundedSender<Value>>,
    ) -> Result<String> {
        let stream = self.group.stream.unwrap_or(stream);
        let suffix = match self.group.protocol {
            Protocol::ChatCompletions => "chat/completions",
            Protocol::Responses => "responses",
            Protocol::Anthropic => "messages",
        };
        let url = format!(
            "{}/{}",
            self.provider.base_url.trim_end_matches('/'),
            suffix
        );
        let mut req = client
            .post(url)
            .timeout(std::time::Duration::from_secs(self.group.timeout_seconds))
            .json(&self.body(messages, stream)?);
        if let Some(env) = &self.provider.api_key_env {
            let secret = std::env::var(env)
                .map_err(|_| Error::Invalid("provider API key environment variable missing"))?;
            req = if self.group.protocol == Protocol::Anthropic {
                req.header("x-api-key", secret)
            } else {
                req.bearer_auth(secret)
            };
        }
        if self.group.protocol == Protocol::Anthropic {
            req = req.header("anthropic-version", "2023-06-01");
        }
        let response = req.send().await.map_err(|e| {
            Error::Message(format!(
                "provider transport: {}",
                if e.is_timeout() {
                    "timeout"
                } else {
                    "connection failed"
                }
            ))
        })?;
        if !response.status().is_success() {
            return Err(Error::Message(format!(
                "provider HTTP {}",
                response.status().as_u16()
            )));
        }
        let is_sse = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.starts_with("text/event-stream"));
        let mut bytes = response.bytes_stream();
        let mut pending = vec![];
        let mut text = String::new();
        let mut done = false;
        let mut total = 0;
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk.map_err(|_| Error::Message("provider stream interrupted".into()))?;
            total += chunk.len();
            if total > 8 * 1024 * 1024 {
                return Err(Error::Invalid("provider response exceeds 8 MiB"));
            }
            pending.extend_from_slice(&chunk);
            if !is_sse {
                continue;
            }
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = pending.drain(..=end).collect();
                let line = std::str::from_utf8(&line)
                    .map_err(|_| Error::Invalid("invalid provider UTF-8"))?
                    .trim();
                let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                    continue;
                };
                if data == "[DONE]" {
                    done = true;
                    continue;
                }
                let value: Value = serde_json::from_str(data)?;
                let delta = delta(&self.group.protocol, &value, &mut done)?;
                if !delta.is_empty() {
                    text.push_str(&delta);
                    if let Some(e) = events {
                        let _ = e.send(json!({"type":"text.delta","text":delta}));
                    }
                }
            }
        }
        if !is_sse {
            text = full_text(&self.group.protocol, &serde_json::from_slice(&pending)?)?;
            if let Some(e) = events {
                let _ = e.send(json!({"type":"text.delta","text":text}));
            }
        } else if !done {
            return Err(Error::Message(
                "provider stream ended before completion".into(),
            ));
        }
        if text.trim().is_empty() {
            return Err(Error::Invalid("provider returned empty text"));
        }
        Ok(text)
    }
}
fn delta(protocol: &Protocol, v: &Value, done: &mut bool) -> Result<String> {
    if v.get("error").is_some()
        || v["type"] == "error"
        || v["type"] == "response.failed"
        || v["type"] == "response.incomplete"
    {
        return Err(Error::Message("provider generation failed".into()));
    }
    let text = match protocol {
        Protocol::ChatCompletions => {
            let c = &v["choices"][0];
            if c["delta"].get("tool_calls").is_some() {
                return Err(Error::Invalid("tool calls unsupported"));
            }
            if let Some(f) = c["finish_reason"].as_str() {
                if f != "stop" {
                    return Err(Error::Invalid("provider output incomplete or unsupported"));
                }
                *done = true;
            }
            c["delta"]["content"].as_str().unwrap_or("")
        }
        Protocol::Responses => {
            if v["type"] == "response.completed" {
                *done = true;
            }
            if v["type"] == "response.output_item.added" && v["item"]["type"] == "function_call" {
                return Err(Error::Invalid("tool calls unsupported"));
            }
            if v["type"] == "response.output_text.delta" {
                v["delta"].as_str().unwrap_or("")
            } else {
                ""
            }
        }
        Protocol::Anthropic => {
            if v["type"] == "message_stop" {
                *done = true;
            }
            if v["content_block"]["type"] == "tool_use" {
                return Err(Error::Invalid("tool calls unsupported"));
            }
            if let Some(r) = v["delta"]["stop_reason"].as_str()
                && !["end_turn", "stop_sequence"].contains(&r)
            {
                return Err(Error::Invalid("provider output incomplete"));
            }
            if v["delta"]["type"] == "text_delta" {
                v["delta"]["text"].as_str().unwrap_or("")
            } else {
                ""
            }
        }
    };
    Ok(text.into())
}
fn full_text(protocol: &Protocol, v: &Value) -> Result<String> {
    if v.get("error").is_some() {
        return Err(Error::Invalid("provider error"));
    }
    match protocol {
        Protocol::ChatCompletions => {
            let c = &v["choices"][0];
            if c["finish_reason"] != "stop" || c["message"].get("tool_calls").is_some() {
                return Err(Error::Invalid("provider output incomplete or unsupported"));
            }
            Ok(c["message"]["content"].as_str().unwrap_or("").into())
        }
        Protocol::Responses => {
            if v["status"] != "completed" {
                return Err(Error::Invalid("response not completed"));
            }
            let mut text = String::new();
            for item in v["output"]
                .as_array()
                .ok_or(Error::Invalid("missing output"))?
            {
                if item["type"] == "function_call" {
                    return Err(Error::Invalid("tool calls unsupported"));
                }
                if let Some(parts) = item["content"].as_array() {
                    for p in parts {
                        if p["type"] == "output_text" {
                            text.push_str(p["text"].as_str().unwrap_or(""));
                        }
                    }
                }
            }
            Ok(text)
        }
        Protocol::Anthropic => {
            if ![Some("end_turn"), Some("stop_sequence")].contains(&v["stop_reason"].as_str()) {
                return Err(Error::Invalid("message not completed"));
            }
            let mut text = String::new();
            for p in v["content"]
                .as_array()
                .ok_or(Error::Invalid("missing content"))?
            {
                if p["type"] == "tool_use" {
                    return Err(Error::Invalid("tool calls unsupported"));
                }
                if p["type"] == "text" {
                    text.push_str(p["text"].as_str().unwrap_or(""));
                }
            }
            Ok(text)
        }
    }
}
