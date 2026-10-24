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
