use molite_core::{prompt_spaces::*, runtime::Engine, store::Store};
use serde_json::json;
#[test]
fn prompt_overrides_persist_reset_and_do_not_travel_with_moc() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("source");
    let mut s = Store::open(&root).unwrap();
    assert_eq!(s.prompt_spaces().unwrap().len(), 4);
    let original = s.prompt_space(PromptSpaceId::Assistant).unwrap();
    let changed = s
        .replace_prompt_space(
            PromptSpaceId::Assistant,
            "Only the local host prompt.".into(),
            Some(&original.revision),
        )
        .unwrap();
    assert_eq!(changed.source, "override");
    assert_ne!(changed.revision, original.revision);
    assert!(
        s.replace_prompt_space(
            PromptSpaceId::Assistant,
            "stale".into(),
            Some(&original.revision)
        )
        .is_err()
    );
    assert!(
        s.replace_prompt_space(PromptSpaceId::Assistant, " ".into(), None)
            .is_err()
    );
    assert!(
        s.replace_prompt_space(
            PromptSpaceId::Assistant,
            "x".repeat(MAX_PROMPT_SPACE_BYTES + 1),
            None
        )
        .is_err()
    );
    let archive = d.path().join("data.moc");
    s.export_moc(&archive, false, None).unwrap();
    let mut target = Store::open(d.path().join("target")).unwrap();
    target.import_moc(&archive, None).unwrap();
    assert_eq!(
        target
            .prompt_space(PromptSpaceId::Assistant)
            .unwrap()
            .source,
        "builtin"
    );
    drop(s);
    let mut s = Store::open(root).unwrap();
    assert_eq!(
        s.prompt_space(PromptSpaceId::Assistant).unwrap().content,
        changed.content
    );
    assert_eq!(
        s.reset_prompt_space(PromptSpaceId::Assistant, Some(&changed.revision))
            .unwrap()
            .revision,
        original.revision
    );
}
#[tokio::test]
async fn prompt_http_routes_share_auth_and_operation_state() {
    let d = tempfile::tempdir().unwrap();
    let engine = Engine::open(d.path()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            molite_core::http::router(engine.clone(), Some("token".into())),
        )
        .into_future(),
    );
    let client = reqwest::Client::new();
    let base = format!("http://{address}/v1/prompt-spaces");
    assert_eq!(client.get(&base).send().await.unwrap().status(), 401);
    assert_eq!(
        client
            .get(&base)
            .bearer_auth("token")
            .send()
            .await
            .unwrap()
            .json::<Vec<serde_json::Value>>()
            .await
            .unwrap()
            .len(),
        4
    );
    let result = client
        .put(format!("{base}/assistant"))
        .bearer_auth("token")
        .json(&json!({"content":"HTTP override"}))
        .send()
        .await
        .unwrap();
    assert!(result.status().is_success());
    assert_eq!(
        engine
            .execute(json!({"op":"prompt_spaces.get","id":"assistant"}), None)
            .await
            .unwrap()["content"],
        "HTTP override"
    );
    assert!(
        client
            .get(format!("{base}/semantic_graph_governance"))
            .bearer_auth("token")
            .send()
            .await
            .unwrap()
            .status()
            .is_client_error()
    );
    assert!(
        client
            .delete(format!("{base}/assistant"))
            .bearer_auth("token")
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert_eq!(
        engine
            .execute(json!({"op":"prompt_spaces.get","id":"assistant"}), None)
            .await
            .unwrap()["source"],
        "builtin"
    );
    server.abort();
}
