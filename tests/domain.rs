use molite_core::{domain::*, store::Store};
use serde_json::json;
#[test]
fn configuration_is_reusable_and_scope_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(dir.path()).unwrap();
    s.put_provider(Provider {
        id: "p".into(),
        base_url: "http://127.0.0.1:1234/v1".into(),
        api_key_env: None,
        protocols: vec![Protocol::Responses],
    })
    .unwrap();
    s.put_model(Model {
        id: "m".into(),
        provider_id: "p".into(),
        model: "local".into(),
        vision: false,
    })
    .unwrap();
    s.put_group(ModelGroup {
        id: "g".into(),
        model_id: "m".into(),
        protocol: Protocol::Responses,
        stream: None,
        parameters: json!({"temperature":0.3}),
        timeout_seconds: 30,
    })
    .unwrap();
    assert!(
        s.set_purposes(Purposes {
            conversation: "g".into(),
            memory: "g".into(),
            vision_fallback: Some("g".into())
        })
        .is_err()
    );
    s.set_purposes(Purposes {
        conversation: "g".into(),
        memory: "g".into(),
        vision_fallback: None,
    })
    .unwrap();
    let scene = Scene {
        id: "s".into(),
        title: "room".into(),
        injection_scope: vec![],
        narrative: Default::default(),
        ended: false,
        classification: Default::default(),
    };
    s.put_scene(scene.clone(), Some(0)).unwrap();
    assert!(s.put_scene(scene, Some(0)).is_err());
    let f = s
        .add_file("notes.txt".into(), "text/plain".into(), b"hello")
        .unwrap();
    assert_eq!(s.read_file(&f.id).unwrap().1, b"hello");
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(s.list("group").unwrap().len(), 1);
    assert_eq!(s.events(0).unwrap().len(), 1);
}
