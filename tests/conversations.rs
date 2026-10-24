use molite_core::{domain::*, runtime::Engine, store::Store};
use serde_json::json;
#[tokio::test]
async fn conversations_own_state_without_a_separate_creation_step() {
    let d = tempfile::tempdir().unwrap();
    let engine = Engine::open(d.path()).unwrap();
    engine
        .execute(
            json!({"op":"conversation.put","value":{"id":"first","title":"First"}}),
            None,
        )
        .await
        .unwrap();
    let value = engine
        .execute(
            json!({"op":"resource.get","kind":"conversation","id":"first"}),
            None,
        )
        .await
        .unwrap();
    assert!(value.get("scene_id").is_none());
    assert_eq!(
        value["narrative"],
        json!({"location":"","time":"","participants":[],"focus":"","open_threads":[],"constraints":[]})
    );
    assert!(
        engine
            .execute(
                json!({"op":"scene.put","value":{"id":"old","title":"old"}}),
                None
            )
            .await
            .is_err()
    );
    assert!(
        engine
            .execute(json!({"op":"resource.list","kind":"scene"}), None)
            .await
            .is_err()
    );
    let status = engine
        .execute(
            json!({"op":"extraction.status","conversation_id":"first"}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(status["schedule"]["every_turns"], 12);
    engine.execute(json!({"op":"conversation.put","value":{"id":"second","title":"Second","injection_scope":["first"]}}),None).await.unwrap();
    let snap = engine
        .execute(json!({"op":"snapshot","conversation_id":"first"}), None)
        .await
        .unwrap();
    assert_eq!(snap["conversation_id"], "first");
    assert!(snap.get("scene_id").is_none());
    assert!(
        engine
            .execute(
                json!({"op":"conversation.delete","id":"first","revision":snap["revision"]}),
                None
            )
            .await
            .is_err()
    );
    let second = engine
        .execute(json!({"op":"snapshot","conversation_id":"second"}), None)
        .await
        .unwrap();
    engine
        .execute(
            json!({"op":"conversation.delete","id":"second","revision":second["revision"]}),
            None,
        )
        .await
        .unwrap();
    engine
        .execute(
            json!({"op":"conversation.delete","id":"first","revision":snap["revision"]}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        engine
            .execute(json!({"op":"resource.list","kind":"conversation"}), None)
            .await
            .unwrap(),
        json!([])
    );
}
#[test]
fn legacy_one_to_one_state_is_upgraded_without_losing_evidence() {
    let d = tempfile::tempdir().unwrap();
    drop(Store::open(d.path()).unwrap());
    let db = rusqlite::Connection::open(d.path().join("molite.sqlite3")).unwrap();
    let scene =
        json!({"id":"s","title":"room","injection_scope":[],"narrative":{"location":"station"}});
    let conversation = json!({"id":"c","scene_id":"s","title":"chat","character_ids":[]});
    db.execute(
        "INSERT INTO resources(kind,id,body) VALUES ('scene','s',?)",
        [scene.to_string()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO resources(kind,id,body) VALUES ('conversation','c',?)",
        [conversation.to_string()],
    )
    .unwrap();
    db.execute_batch(
        "INSERT INTO scenes VALUES ('s',2); INSERT INTO runtime(scene,revision) VALUES ('s',7);",
    )
    .unwrap();
    let exchange = json!({"request_id":"q","conversation_id":"c","scene_id":"s","revision":1,"user":"hello","assistant":"world","attachments":[],"deleted":false});
    db.execute(
        "INSERT INTO exchanges(request,conversation,scene,revision,body) VALUES ('q','c','s',1,?)",
        [exchange.to_string()],
    )
    .unwrap();
    let turn = json!({"request_id":"q","user":"hello","assistant":"world"});
    db.execute(
        "INSERT INTO turns(scene,request,body) VALUES ('s','q',?)",
        [turn.to_string()],
    )
    .unwrap();
    let captured = json!({"scene":"s","scope_revisions":{"s":7},"request":{"request_id":"pending","conversation_id":"c"},"messages":[{"role":"system","text":"captured prompt"}],"connection":{"model":"captured"}});
    db.execute(
        "INSERT INTO requests(id,input) VALUES ('pending',?)",
        [captured.to_string()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO maintenance_requests(id,body) VALUES ('repair:s:7','[]')",
        [],
    )
    .unwrap();
    drop(db);
    let mut store = Store::open(d.path()).unwrap();
    let c: Conversation = store.get("conversation", "c").unwrap();
    assert_eq!(c.narrative.location, "station");
    assert!(store.list("scene").unwrap().is_empty());
    assert_eq!(store.history("c").unwrap()[0].conversation_id, "c");
    assert_eq!(store.revision("c").unwrap(), 7);
    assert_eq!(
        store.extraction_status("c").unwrap()["schedule"]["pending_turns"],
        1
    );
    assert_eq!(
        store
            .begin_extraction("c", true)
            .unwrap()
            .unwrap()
            .evidence
            .len(),
        1
    );
    let migrated = rusqlite::Connection::open(d.path().join("molite.sqlite3")).unwrap();
    let body: String = migrated
        .query_row("SELECT input FROM requests WHERE id='pending'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["conversation_id"], "c");
    assert!(body.get("scene").is_none());
    assert_eq!(body["messages"], captured["messages"]);
    assert_eq!(body["connection"], captured["connection"]);
    assert_eq!(body["scope_revisions"], json!({"c":7}));
    assert_eq!(
        migrated
            .query_row(
                "SELECT count(*) FROM maintenance_requests WHERE id='repair:c:7'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    drop(store);
    assert!(Store::open(d.path()).is_ok());
}
#[test]
fn ambiguous_legacy_shared_state_is_not_silently_split() {
    let d = tempfile::tempdir().unwrap();
    drop(Store::open(d.path()).unwrap());
    let db = rusqlite::Connection::open(d.path().join("molite.sqlite3")).unwrap();
    db.execute(
        "INSERT INTO resources(kind,id,body) VALUES ('scene','s',?)",
        [json!({"id":"s","title":"state"}).to_string()],
    )
    .unwrap();
    for id in ["a", "b"] {
        db.execute(
            "INSERT INTO resources(kind,id,body) VALUES ('conversation',?,?)",
            rusqlite::params![id, json!({"id":id,"scene_id":"s","title":id}).to_string()],
        )
        .unwrap();
    }
    assert!(Store::open(d.path()).is_err());
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM resources WHERE kind='scene'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}
