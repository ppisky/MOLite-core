use rusqlite::Connection;

#[test]
fn bundled_database_keeps_required_features_without_unused_extensions() {
    let db = Connection::open_in_memory().unwrap();
    for option in [
        "ENABLE_FTS3",
        "ENABLE_FTS5",
        "ENABLE_RTREE",
        "ENABLE_DBSTAT_VTAB",
        "SOUNDEX",
        "ENABLE_COLUMN_METADATA",
    ] {
        let enabled: bool = db
            .query_row("SELECT sqlite_compileoption_used(?1)", [option], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(!enabled, "unused SQLite option enabled: {option}");
    }
    let omitted: bool = db
        .query_row(
            "SELECT sqlite_compileoption_used('OMIT_LOAD_EXTENSION')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        omitted,
        "build must use the repository SQLite configuration"
    );
    let value: i64 = db
        .query_row(
            "SELECT sum(value) FROM json_each(json_extract('{\"ids\":[1,2]}', '$.ids'))",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(value, 3);
    db.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE parent(id INTEGER PRIMARY KEY);
         CREATE TABLE child(id INTEGER REFERENCES parent(id), n INTEGER CHECK(n>0));",
    )
    .unwrap();
    assert!(db.execute("INSERT INTO child VALUES (1,1)", []).is_err());
    db.execute("INSERT INTO parent VALUES (1)", []).unwrap();
    assert!(db.execute("INSERT INTO child VALUES (1,0)", []).is_err());
    db.execute("INSERT INTO child VALUES (1,1)", []).unwrap();
}
