//! HTTP adapter for the shared operation surface; streamed disconnects cancel work.
use crate::runtime::Engine;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc};
#[derive(Clone)]
struct HttpState {
    engine: Engine,
    token: Option<Arc<String>>,
}
pub fn router(engine: Engine, token: Option<String>) -> Router {
    Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"status":"ok","api_version":1})) }),
        )
        .route("/v1/prompt-spaces", get(prompt_list))
        .route(
            "/v1/prompt-spaces/{id}",
            get(prompt_get).put(prompt_replace).delete(prompt_reset),
        )
        .route("/v1/operations", post(operation))
        .route("/v1/stream", post(stream))
        .layer(DefaultBodyLimit::max(90 * 1024 * 1024))
        .with_state(HttpState {
            engine,
            token: token.map(Arc::new),
        })
}
fn authorized(s: &HttpState, h: &HeaderMap) -> bool {
    s.token.as_ref().is_none_or(|t| {
        h.get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|v| v == t.as_str())
    })
}
async fn operation(
    State(s): State<HttpState>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Response {
    if !authorized(&s, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match s.engine.execute(input, None).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":e.to_string()})),
        )
            .into_response(),
    }
}
struct CancelOnDrop(tokio::task::JoinHandle<()>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn stream(
    State(s): State<HttpState>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Response {
    if !authorized(&s, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        let result = s.engine.execute(input, Some(&tx)).await;
        let event = match result {
            Ok(v) => json!({"type":"completed","result":v}),
            Err(e) => json!({"type":"failed","error":e.to_string()}),
        };
        let _ = tx.send(event);
    });
    let events =
        futures_util::stream::unfold((rx, CancelOnDrop(handle)), |(mut rx, guard)| async move {
            rx.recv().await.map(|v| {
                (
                    Ok::<_, Infallible>(
                        Event::default()
                            .event(v["type"].as_str().unwrap_or("event"))
                            .data(v.to_string()),
                    ),
                    (rx, guard),
                )
            })
        });
    Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn prompt_list(State(s): State<HttpState>, h: HeaderMap) -> Response {
    operation(State(s), h, Json(json!({"op":"prompt_spaces.list"}))).await
}
async fn prompt_get(State(s): State<HttpState>, Path(id): Path<String>, h: HeaderMap) -> Response {
    operation(State(s), h, Json(json!({"op":"prompt_spaces.get","id":id}))).await
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptReplacement {
    content: String,
    #[serde(default)]
    revision: Option<String>,
}
async fn prompt_replace(
    State(s): State<HttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(body): Json<PromptReplacement>,
) -> Response {
    operation(State(s), h, Json(json!({"op":"prompt_spaces.replace","id":id,"content":body.content,"revision":body.revision}))).await
}
async fn prompt_reset(
    State(s): State<HttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Response {
    operation(
        State(s),
        h,
        Json(json!({"op":"prompt_spaces.reset","id":id})),
    )
    .await
}
