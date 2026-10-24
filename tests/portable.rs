use molite_core::{domain::*, store::Store};
#[test]
fn native_moc_roundtrip_with_files_and_conflict_rollback() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path().join("source")).unwrap();
    s.put_character(Character {
        id: id(),
        name: "Alice".into(),
        version: "2.0.0".into(),
        author_name: "Author".into(),
        character_markdown: "A traveller.".into(),
        user_markdown: String::new(),
        opening_markdown: Some("Hello".into()),
        classification: Default::default(),
    })
    .unwrap();
    let file = s
        .add_file("test.pdf".into(), "application/pdf".into(), b"%PDF-test")
        .unwrap();
    let archive = d.path().join("test.moc");
    s.export_moc(&archive, true, None).unwrap();
    let mut target = Store::open(d.path().join("target")).unwrap();
    target.import_moc(&archive, None).unwrap();
    assert_eq!(target.list("character").unwrap().len(), 1);
    assert_eq!(target.read_file(&file.id).unwrap().1, b"%PDF-test");
    assert!(target.import_moc(&archive, None).is_err());
    assert_eq!(target.list("character").unwrap().len(), 1);
}
#[test]
fn encrypted_moc_requires_correct_passphrase() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path().join("source")).unwrap();
    let archive = d.path().join("private.moc");
    s.export_moc(&archive, false, Some("test-passphrase"))
        .unwrap();
    let mut target = Store::open(d.path().join("target")).unwrap();
    assert!(target.import_moc(&archive, Some("wrong")).is_err());
    target
        .import_moc(&archive, Some("test-passphrase"))
        .unwrap();
}

#[test]
fn moc_preserves_memory_provenance_and_unfinished_extraction() {
    let d = tempfile::tempdir().unwrap();
    let mut s = Store::open(d.path().join("source")).unwrap();
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
        title: "c".into(),
        character_ids: vec![],
        classification: Default::default(),
    })
    .unwrap();
    for request in ["one", "two"] {
        s.commit_exchange(
            &Exchange {
                request_id: request.into(),
                conversation_id: "c".into(),
                scene_id: "s".into(),
                revision: 1,
                user: "旅行".into(),
                assistant: "出发".into(),
                attachments: vec![],
                deleted: false,
            },
            s.revision("s").unwrap(),
        )
        .unwrap();
        let job = s.begin_extraction("s", true).unwrap().unwrap();
        if request == "one" {
            s.apply_extraction(
                &job,
                &MemoryPatch {
                    upsert: vec![Memory {
                        id: "trip".into(),
                        kind: "event".into(),
                        title: "旅行".into(),
                        body: "出发旅行".into(),
                        tags: vec![],
                        aliases: vec![],
                        links: vec![],
                        core: false,
                        weight: 1.0,
                        sources: vec![Source {
                            request_id: request.into(),
                            revision: 1,
                        }],
                    }],
                    narrative: None,
                },
            )
            .unwrap();
        }
    }
    let archive = d.path().join("state.moc");
    s.export_moc(&archive, false, None).unwrap();
    let mut target = Store::open(d.path().join("target")).unwrap();
    target.import_moc(&archive, None).unwrap();
    assert_eq!(target.snapshot("s", "旅行").unwrap().memories.len(), 1);
    let job = target.begin_extraction("s", false).unwrap().unwrap();
    assert_eq!(job.evidence[0].request_id, "two");
    target
        .apply_extraction(&job, &MemoryPatch::default())
        .unwrap();
    assert_eq!(
        target.extraction_status("s").unwrap()["schedule"]["pending_turns"],
        0
    );
    target.revise_exchange("one", 1, None, None, true).unwrap();
    assert!(target.snapshot("s", "旅行").unwrap().memories.is_empty());
}
