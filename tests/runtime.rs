use axum::{
    Json, Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::post,
};
use molite_core::{domain::*, runtime::Engine};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Clone)]
struct Mock {
    calls: Arc<Mutex<Vec<Value>>>,
}
async fn provider(State(s): State<Mock>, Json(v): Json<Value>) -> Response {
    s.calls.lock().unwrap().push(v.clone());
    let output = if v["model"] == "memory" {
        r#"{"upsert":[],"narrative":null}"#
    } else if v["model"] == "vision" {
        "A red apple."
    } else {
        "你好"
    };
    if v["stream"] == true {
        let wire = if v.get("input").is_some() {
            format!(
                "data: {}\n\ndata: {}\n\n",
                json!({"type":"response.output_text.delta","delta":output}),
                json!({"type":"response.completed"})
            )
        } else if v.get("system").is_some() {
            format!(
                "data: {}\n\ndata: {}\n\n",
                json!({"type":"content_block_delta","delta":{"type":"text_delta","text":output}}),
                json!({"type":"message_stop"})
            )
        } else {
            format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[{"delta":{"content":output},"finish_reason":null}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
            )
        };
        ([("content-type", "text/event-stream")], wire).into_response()
    } else if v.get("input").is_some() {
        Json(json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":output}]}]})).into_response()
    } else if v.get("system").is_some() {
        Json(json!({"stop_reason":"end_turn","content":[{"type":"text","text":output}]}))
            .into_response()
    } else {
        Json(json!({"choices":[{"finish_reason":"stop","message":{"content":output}}]}))
            .into_response()
    }
}
async fn setup(
    protocol: Protocol,
    vision: bool,
) -> (tempfile::TempDir, Engine, Mock, tokio::task::JoinHandle<()>) {
    let mock = Mock {
        calls: Default::default(),
    };
    let app = Router::new()
        .route("/v1/chat/completions", post(provider))
        .route("/v1/responses", post(provider))
        .route("/v1/messages", post(provider))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let d = tempfile::tempdir().unwrap();
    let e = Engine::open(d.path()).unwrap();
    e.execute(json!({"op":"provider.put","value":{"id":"p","base_url":format!("http://{address}/v1"),"api_key_env":null,"protocols":[protocol]}}),None).await.unwrap();
    for (id, vision) in [("chat", vision), ("memory", false), ("vision", true)] {
        e.execute(json!({"op":"model.put","value":{"id":id,"provider_id":"p","model":id,"vision":vision}}),None).await.unwrap();
        e.execute(json!({"op":"group.put","value":{"id":id,"model_id":id,"protocol":protocol,"parameters":{"temperature":0.2}}}),None).await.unwrap();
    }
    e.execute(json!({"op":"purposes.set","value":{"conversation":"chat","memory":"memory","vision_fallback":"vision"}}),None).await.unwrap();
    e.execute(
        json!({"op":"scene.put","value":{"id":"s","title":"room"}}),
        None,
    )
    .await
    .unwrap();
    e.execute(
        json!({"op":"conversation.put","value":{"id":"c","scene_id":"s","title":"chat"}}),
        None,
    )
    .await
    .unwrap();
    (d, e, mock, server)
}
#[tokio::test]
async fn all_protocols_stream_commit_and_extract_with_replay() {
    for p in [
        Protocol::ChatCompletions,
        Protocol::Responses,
        Protocol::Anthropic,
    ] {
        let (_d, e, mock, server) = setup(p, false).await;
        e.execute(
            json!({"op":"extraction.configure","scene_id":"s","every_turns":1}),
            None,
        )
        .await
        .unwrap();
        let request =
            json!({"op":"chat","request":{"request_id":"q","conversation_id":"c","text":"你好"}});
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let result = e.execute(request.clone(), Some(&tx)).await.unwrap();
        assert_eq!(result["assistant"], "你好");
        assert_eq!(e.execute(request, None).await.unwrap()["request_id"], "q");
        assert_eq!(mock.calls.lock().unwrap().len(), 2);
        let mut emitted = vec![];
        while let Ok(v) = rx.try_recv() {
            emitted.push(v);
        }
        assert!(emitted.iter().any(|v| v["type"] == "text.delta"));
        let status = e
            .execute(json!({"op":"extraction.status","scene_id":"s"}), None)
            .await
            .unwrap();
        assert_eq!(status["schedule"]["pending_turns"], 0);
        server.abort();
    }
}
#[tokio::test]
async fn image_route_uses_fallback_only_for_text_models() {
    for native in [true, false] {
        let (_d, e, mock, server) = setup(Protocol::ChatCompletions, native).await;
        let f = e
            .execute(
                json!({"op":"file.add","name":"a.png","mime":"image/png","base64":"aW1hZ2U="}),
                None,
            )
            .await
            .unwrap();
        e.execute(json!({"op":"chat","request":{"request_id":"q","conversation_id":"c","text":"看图片","attachments":[f["id"]]}}),None).await.unwrap();
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), if native { 1 } else { 2 });
        let chat = calls.last().unwrap().to_string();
        if native {
            assert!(chat.contains("image_url"));
        } else {
            assert!(chat.contains("A red apple"));
            assert!(!chat.contains("image_url"));
        }
        server.abort();
    }
}
#[tokio::test]
async fn http_uses_shared_operations_and_checks_token() {
    let (_d, e, _mock, provider) = setup(Protocol::Responses, false).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = molite_core::http::router(e, Some("test-token".into()));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    let url = format!("http://{address}/v1/operations");
    let body = json!({"op":"resource.list","kind":"scene"});
    assert_eq!(
        client.post(&url).json(&body).send().await.unwrap().status(),
        401
    );
    let result: Value = client
        .post(url)
        .bearer_auth("test-token")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result[0]["id"], "s");
    let stream=client.post(format!("http://{address}/v1/stream")).bearer_auth("test-token").json(&json!({"op":"chat","request":{"request_id":"http","conversation_id":"c","text":"hello"}})).send().await.unwrap().text().await.unwrap();
    assert!(stream.contains("event: text.delta"));
    assert!(stream.contains("event: completed"));
    server.abort();
    provider.abort();
}
