use molite_core::{domain::*, store::Store};
fn setup() -> (tempfile::TempDir, Store) {
    let d = tempfile::tempdir().unwrap();
    let mut s = Store::open(d.path()).unwrap();
    s.put_scene(
        Scene {
            id: "s".into(),
            title: "s".into(),
            injection_scope: vec![],
            narrative: Default::default(),
            ended: false,
            classification: Default::default(),
        },
        None,
    )
    .unwrap();
    s.put_conversation(Conversation {
        id: "c".into(),
        scene_id: "s".into(),
        character_ids: vec![],
        title: "c".into(),
        classification: Default::default(),
    })
    .unwrap();
    (d, s)
}
fn record(s: &mut Store, id: &str) {
    let revision = s.revision("s").unwrap();
    s.commit_exchange(
        &Exchange {
            request_id: id.into(),
            conversation_id: "c".into(),
            scene_id: "s".into(),
            revision: 1,
            user: "受伤了".into(),
            assistant: "包扎伤口".into(),
            attachments: vec![],
            deleted: false,
        },
        revision,
    )
    .unwrap();
}
#[test]
fn revision_invalidates_memory_and_repair_does_not_reset_cadence() {
    let (_d, mut s) = setup();
    record(&mut s, "a");
    let job = s.begin_extraction("s", true).unwrap().unwrap();
    let memory = Memory {
        id: "injury".into(),
        kind: "event".into(),
        title: "受伤".into(),
        body: "受伤了".into(),
        tags: vec!["wounded".into()],
        aliases: vec![],
        links: vec![],
        core: false,
        weight: 1.0,
        sources: vec![Source {
            request_id: "a".into(),
            revision: 1,
        }],
    };
    s.apply_extraction(
        &job,
        &MemoryPatch {
            upsert: vec![memory],
            narrative: None,
        },
    )
    .unwrap();
    assert!(!s.snapshot("s", "受伤").unwrap().dimensions["physiological_state"].is_empty());
    record(&mut s, "b");
    s.revise_exchange("a", 1, Some("没有受伤".into()), None, false)
        .unwrap();
    assert!(s.snapshot("s", "受伤").unwrap().memories.is_empty());
    assert!(s.apply_extraction(&job, &MemoryPatch::default()).is_err());
    let repair = s.begin_extraction("s", false).unwrap().unwrap();
    assert!(repair.repair);
    assert_eq!(repair.evidence.len(), 1);
    assert_eq!(repair.evidence[0].revision, 2);
    s.apply_extraction(&repair, &MemoryPatch::default())
        .unwrap();
    assert_eq!(
        s.extraction_status("s").unwrap()["schedule"]["pending_turns"],
        1
    );
}
#[test]
fn forged_sources_are_rejected_without_acknowledging_batch() {
    let (_d, mut s) = setup();
    record(&mut s, "a");
    let j = s.begin_extraction("s", true).unwrap().unwrap();
    let m = Memory {
        id: "m".into(),
        kind: "world".into(),
        title: "x".into(),
        body: "x".into(),
        tags: vec![],
        aliases: vec![],
        links: vec![],
        core: false,
        weight: 1.0,
        sources: vec![Source {
            request_id: "other".into(),
            revision: 1,
        }],
    };
    assert!(
        s.apply_extraction(
            &j,
            &MemoryPatch {
                upsert: vec![m],
                narrative: None
            }
        )
        .is_err()
    );
    assert_eq!(
        s.extraction_status("s").unwrap()["schedule"]["pending_turns"],
        1
    );
}
