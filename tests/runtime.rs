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
    if v["model"] == "broken" {
        return (
            [("content-type", "text/event-stream")],
            format!(
                "data: {}\n\n",
                json!({"choices":[{"delta":{"content":"partial"},"finish_reason":null}]})
            ),
        )
            .into_response();
    }
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
        json!({"op":"conversation.put","value":{"id":"c","title":"chat"}}),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        e.execute(
            json!({"op":"resource.get","kind":"settings","id":"purposes"}),
            None
        )
        .await
        .unwrap()["conversation"],
        "chat"
    );
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
            json!({"op":"extraction.configure","conversation_id":"c","every_turns":1}),
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
            .execute(
                json!({"op":"extraction.status","conversation_id":"c"}),
                None,
            )
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
async fn regeneration_and_explicit_group_stream_do_not_add_turns() {
    let (_d, e, mock, server) = setup(Protocol::Responses, false).await;
    e.execute(json!({"op":"group.put","value":{"id":"chat","model_id":"chat","protocol":"responses","stream":false,"parameters":{}}}),None).await.unwrap();
    e.execute(
        json!({"op":"chat","request":{"request_id":"q","conversation_id":"c","text":"hello"}}),
        None,
    )
    .await
    .unwrap();
    assert_eq!(mock.calls.lock().unwrap()[0]["stream"], false);
    let regenerated = e
        .execute(
            json!({"op":"exchange.regenerate","request_id":"q","revision":1}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(regenerated["exchange"]["revision"], 2);
    let status = e
        .execute(
            json!({"op":"extraction.status","conversation_id":"c"}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(status["schedule"]["pending_turns"], 1);
    assert_eq!(
        e.execute(json!({"op":"history","conversation_id":"c"}), None)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    server.abort();
}

#[tokio::test]
async fn interrupted_stream_never_commits_and_retries_keep_captured_model() {
    let (_d, e, _mock, server) = setup(Protocol::ChatCompletions, false).await;
    e.execute(json!({"op":"model.put","value":{"id":"broken","provider_id":"p","model":"broken","vision":false}}),None).await.unwrap();
    e.execute(json!({"op":"group.put","value":{"id":"chat","model_id":"broken","protocol":"chat_completions"}}),None).await.unwrap();
    let request =
        json!({"op":"chat","request":{"request_id":"q","conversation_id":"c","text":"hello"}});
    assert!(e.execute(request.clone(), None).await.is_err());
    e.execute(json!({"op":"group.put","value":{"id":"chat","model_id":"chat","protocol":"chat_completions"}}),None).await.unwrap();
    assert!(e.execute(request, None).await.is_err());
    assert!(
        e.execute(json!({"op":"history","conversation_id":"c"}), None)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    e.execute(
        json!({"op":"chat","request":{"request_id":"new","conversation_id":"c","text":"hello"}}),
        None,
    )
    .await
    .unwrap();
    server.abort();
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
    let body = json!({"op":"resource.list","kind":"conversation"});
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
    assert_eq!(result[0]["id"], "c");
    let stream=client.post(format!("http://{address}/v1/stream")).bearer_auth("test-token").json(&json!({"op":"chat","request":{"request_id":"http","conversation_id":"c","text":"hello"}})).send().await.unwrap().text().await.unwrap();
    assert!(stream.contains("event: text.delta"));
    assert!(stream.contains("event: completed"));
    server.abort();
    provider.abort();
}

#[tokio::test]
async fn prompt_overrides_reach_models_and_failed_requests_keep_captured_prompts() {
    let (_d, e, mock, server) = setup(Protocol::ChatCompletions, false).await;
    for (id, content) in [
        ("assistant", "custom assistant"),
        ("roleplay_director", "custom director"),
        ("vision_fallback", "custom vision"),
        ("memory_distillation", "custom extraction"),
    ] {
        e.execute(
            json!({"op":"prompt_spaces.replace","id":id,"content":content}),
            None,
        )
        .await
        .unwrap();
    }
    e.execute(
        json!({"op":"extraction.configure","conversation_id":"c","every_turns":1}),
        None,
    )
    .await
    .unwrap();
    let f = e
        .execute(
            json!({"op":"file.add","name":"x.png","mime":"image/png","base64":"aA=="}),
            None,
        )
        .await
        .unwrap();
    e.execute(json!({"op":"chat","request":{"request_id":"first","conversation_id":"c","text":"look","attachments":[f["id"]]}}),None).await.unwrap();
    {
        let calls = mock.calls.lock().unwrap();
        for (model, prompt) in [
            ("vision", "custom vision"),
            ("memory", "custom extraction"),
            ("chat", "custom assistant"),
        ] {
            assert!(
                calls.iter().any(|c| c["model"] == model
                    && c["messages"][0]["content"][0]["text"]
                        .as_str()
                        .unwrap_or("")
                        .contains(prompt)),
                "{model} missed its prompt"
            );
        }
        assert!(calls.iter().any(|c| {
            c["model"] == "chat"
                && c["messages"][0]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("custom director")
        }));
    }
    e.execute(
        json!({"op":"model.put","value":{"id":"broken","provider_id":"p","model":"broken"}}),
        None,
    )
    .await
    .unwrap();
    e.execute(json!({"op":"group.put","value":{"id":"chat","model_id":"broken","protocol":"chat_completions"}}),None).await.unwrap();
    let request =
        json!({"op":"chat","request":{"request_id":"retry","conversation_id":"c","text":"hi"}});
    assert!(e.execute(request.clone(), None).await.is_err());
    e.execute(
        json!({"op":"prompt_spaces.replace","id":"assistant","content":"new assistant"}),
        None,
    )
    .await
    .unwrap();
    assert!(e.execute(request, None).await.is_err());
    {
        let calls = mock.calls.lock().unwrap();
        let retry = calls.iter().rev().find(|c| c["model"] == "broken").unwrap();
        let system = retry["messages"][0]["content"][0]["text"].as_str().unwrap();
        assert!(system.contains("custom assistant"));
        assert!(!system.contains("new assistant"));
    }
    server.abort();
}

#[tokio::test]
async fn memory_retry_retains_the_captured_prompt_and_model_group() {
    let (_d, e, mock, server) = setup(Protocol::ChatCompletions, false).await;
    e.execute(
        json!({"op":"model.put","value":{"id":"broken","provider_id":"p","model":"broken"}}),
        None,
    )
    .await
    .unwrap();
    e.execute(json!({"op":"group.put","value":{"id":"memory","model_id":"broken","protocol":"chat_completions"}}),None).await.unwrap();
    e.execute(json!({"op":"prompt_spaces.replace","id":"memory_distillation","content":"captured memory prompt"}),None).await.unwrap();
    e.execute(
        json!({"op":"extraction.configure","conversation_id":"c","every_turns":1}),
        None,
    )
    .await
    .unwrap();
    e.execute(
        json!({"op":"chat","request":{"request_id":"q","conversation_id":"c","text":"hello"}}),
        None,
    )
    .await
    .unwrap();
    e.execute(json!({"op":"prompt_spaces.replace","id":"memory_distillation","content":"new memory prompt"}),None).await.unwrap();
    e.execute(json!({"op":"group.put","value":{"id":"memory","model_id":"memory","protocol":"chat_completions"}}),None).await.unwrap();
    assert!(
        e.execute(json!({"op":"distill","conversation_id":"c"}), None)
            .await
            .is_err()
    );
    {
        let calls = mock.calls.lock().unwrap();
        let last = calls.last().unwrap();
        assert_eq!(last["model"], "broken");
        assert_eq!(
            last["messages"][0]["content"][0]["text"],
            "captured memory prompt"
        );
    }
    server.abort();
}
