use molite_core::{domain::*, store::Store};
fn setup() -> (tempfile::TempDir, Store) {
    let d = tempfile::tempdir().unwrap();
    let mut s = Store::open(d.path()).unwrap();
    s.put_conversation(
        Conversation {
            id: "c".into(),
            character_ids: vec![],
            title: "c".into(),
            classification: Default::default(),
            injection_scope: vec![],
            narrative: Default::default(),
            ended: false,
        },
        None,
    )
    .unwrap();
    (d, s)
}
fn record(s: &mut Store, id: &str) {
    let revision = s.revision("c").unwrap();
    s.commit_exchange(
        &Exchange {
            request_id: id.into(),
            conversation_id: "c".into(),
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
    let job = s.begin_extraction("c", true).unwrap().unwrap();
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
    assert!(!s.snapshot("c", "受伤").unwrap().dimensions["physiological_state"].is_empty());
    record(&mut s, "b");
    s.revise_exchange("a", 1, Some("没有受伤".into()), None, false)
        .unwrap();
    assert!(s.snapshot("c", "受伤").unwrap().memories.is_empty());
    assert!(s.apply_extraction(&job, &MemoryPatch::default()).is_err());
    let repair = s.begin_extraction("c", false).unwrap().unwrap();
    assert!(repair.repair);
    assert_eq!(repair.evidence.len(), 1);
    assert_eq!(repair.evidence[0].revision, 2);
    s.apply_extraction(&repair, &MemoryPatch::default())
        .unwrap();
    assert_eq!(
        s.extraction_status("c").unwrap()["schedule"]["pending_turns"],
        1
    );
}
#[test]
fn forged_sources_are_rejected_without_acknowledging_batch() {
    let (_d, mut s) = setup();
    record(&mut s, "a");
    let j = s.begin_extraction("c", true).unwrap().unwrap();
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
        s.extraction_status("c").unwrap()["schedule"]["pending_turns"],
        1
    );
}

#[test]
fn repair_preserves_unrelated_facts_and_scopes_require_explicit_selection() {
    let (_d, mut s) = setup();
    for (request, word) in [("a", "伤口"), ("b", "旅行")] {
        record(&mut s, request);
        let job = s.begin_extraction("c", true).unwrap().unwrap();
        let m = Memory {
            id: request.into(),
            kind: "event".into(),
            title: word.into(),
            body: word.into(),
            tags: vec![],
            aliases: vec![],
            links: vec![],
            core: false,
            weight: 1.0,
            sources: vec![Source {
                request_id: request.into(),
                revision: 1,
            }],
        };
        s.apply_extraction(
            &job,
            &MemoryPatch {
                upsert: vec![m],
                narrative: None,
            },
        )
        .unwrap();
    }
    s.revise_exchange("a", 1, None, None, true).unwrap();
    let repair = s.begin_extraction("c", false).unwrap().unwrap();
    assert_eq!(repair.affected_ids, vec!["a"]);
    assert!(repair.evidence.is_empty());
    s.apply_extraction(&repair, &MemoryPatch::default())
        .unwrap();
    assert_eq!(s.snapshot("c", "旅行").unwrap().memories.len(), 1);
    let mut other = Conversation {
        character_ids: vec![],
        id: "other".into(),
        title: "other".into(),
        injection_scope: vec![],
        narrative: Default::default(),
        ended: false,
        classification: Default::default(),
    };
    s.put_conversation(other.clone(), None).unwrap();
    assert!(s.snapshot("other", "旅行").unwrap().memories.is_empty());
    other.injection_scope = vec!["c".into()];
    s.put_conversation(other, None).unwrap();
    assert_eq!(s.snapshot("other", "旅行").unwrap().memories.len(), 1);
    assert!(
        s.delete_conversation("c", s.revision("c").unwrap())
            .is_err()
    );
}
