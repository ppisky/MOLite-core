use molite_core::{Scheduler, Turn};
use serde_json::json;

fn turn(n: usize) -> Turn {
    Turn {
        request_id: n.to_string(),
        user: format!("input {n}"),
        assistant: format!("reply {n}"),
    }
}

#[test]
fn early_completion_restarts_cadence_without_losing_inflight_arrivals() {
    let mut s = Scheduler::open(":memory:").unwrap();
    for n in 0..5 {
        s.record("a", &turn(n)).unwrap();
    }
    assert!(s.begin("a", false).unwrap().is_none());
    let batch = s.begin("a", true).unwrap().unwrap();
    s.record("a", &turn(5)).unwrap();
    s.complete(&batch, &json!({"patches":[]})).unwrap();
    assert_eq!(s.status("a").unwrap().turns_until_due, 11);
    for n in 6..17 {
        s.record("a", &turn(n)).unwrap();
    }
    let next = s.begin("a", false).unwrap().unwrap();
    assert_eq!(next.turns.len(), 12);
    assert_eq!(next.turns[0].request_id, "5");
}

#[test]
fn restart_retry_and_configuration_change_preserve_frozen_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("core.sqlite3");
    let mut s = Scheduler::open(&path).unwrap();
    s.configure("a", 3).unwrap();
    for n in 0..3 {
        s.record("a", &turn(n)).unwrap();
    }
    let original = s.begin("a", false).unwrap().unwrap();
    drop(s); // model failed or process stopped
    let mut s = Scheduler::open(&path).unwrap();
    s.configure("a", 1).unwrap();
    assert_eq!(s.begin("a", true).unwrap().unwrap(), original);
    s.complete(&original, &json!({})).unwrap();
    s.complete(&original, &json!({})).unwrap();
    assert_eq!(s.status("a").unwrap().pending_turns, 0);
    s.record("a", &turn(3)).unwrap();
    assert_eq!(s.begin("a", false).unwrap().unwrap().turns.len(), 1);
}

#[test]
fn replay_is_idempotent_and_scenes_do_not_mix() {
    let mut s = Scheduler::open(":memory:").unwrap();
    s.record("a", &turn(0)).unwrap();
    s.record("a", &turn(0)).unwrap();
    let mut changed = turn(0);
    changed.user = "changed".into();
    assert!(s.record("a", &changed).is_err());
    assert_eq!(s.status("a").unwrap().pending_turns, 1);
    assert!(s.begin("b", true).unwrap().is_none());
    assert!(s.configure("a", 0).is_err());
    let mut batch = s.begin("a", true).unwrap().unwrap();
    batch.turns[0].user = "forged".into();
    assert!(s.complete(&batch, &json!({})).is_err());
    assert_eq!(s.status("a").unwrap().pending_turns, 1);
}
