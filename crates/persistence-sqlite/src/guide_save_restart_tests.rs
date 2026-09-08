#[test]
fn guide_unclaimed_progress_claims_and_stages_survive_restart() {
    let path = std::env::temp_dir().join(format!("ggfm-guide-save-{}.sqlite",
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    let mut db = Database::open(&path).unwrap();
    let first = db.create_slot("First", 100).unwrap();
    let second = db.create_slot("Other", 100).unwrap();
    let session = db.begin_login(b"guide-save", DeviceClock::new(100, 0).unwrap()).unwrap();
    let make = |seq, rpc: &str| RpcMutation {
        nonce: session.nonce.clone(), identity: first.clone(), request_seq: seq,
        rpc: rpc.into(), idempotency_key: format!("guide-save:{seq}"), committed_at: 100 + seq,
    };
    let patch = |stage, quantities| UserSavePatch {
        follower_quests: vec![ggfm_domain::FollowerQuestSavePatch {
            current_id: stage, condition_values: quantities,
        }], ..Default::default()
    };
    db.merge_user_save(&make(1, "userSave"), &patch(1, [2.0, 3.0, 4.0]), "phone", 10, &[]).unwrap();
    let snapshot = db.player_snapshot(first.usn).unwrap();
    assert_eq!(snapshot.follower_quests[0].condition_values, [2.0, 3.0, 4.0]);
    assert_eq!(snapshot.follower_quests[0].claimed, [false; 3]);
    // Lower, malformed, absent and wrong-stage snapshots cannot erase progress.
    db.merge_user_save(&make(2, "userSave"), &patch(1, [0.0, f64::NAN, -1.0]), "phone", 10, &[]).unwrap();
    db.merge_user_save(&make(3, "userSave"), &patch(2, [99.0; 3]), "phone", 10, &[]).unwrap();
    db.merge_user_save(&make(4, "userSave"), &UserSavePatch::default(), "phone", 10, &[]).unwrap();
    assert_eq!(db.player_snapshot(first.usn).unwrap().follower_quests, snapshot.follower_quests);
    assert_eq!(db.connection.query_row("SELECT COUNT(*) FROM follower_quests WHERE usn=?1", [second.usn], |r| r.get::<_, i64>(0)).unwrap(), 0);
    // Saved progress can satisfy a claim even if the request carries zero.
    let claim = db.advance_follower_quest(&make(5, "setUserFollowerQuest"), 1, 1, 0.0,
        [2.0, 3.0, 4.0], Vec::new(), Some(2)).unwrap();
    assert_eq!(claim.quest.claimed, [true, false, false]);
    assert!(!claim.next);
    let replay = db.advance_follower_quest(&make(5, "setUserFollowerQuest"), 1, 1, 0.0,
        [2.0, 3.0, 4.0], Vec::new(), Some(2)).unwrap();
    assert!(replay.rewards.is_empty());
    let stale = RpcMutation { idempotency_key: "late-save".into(), ..make(3, "userSave") };
    assert!(matches!(db.merge_user_save(&stale, &patch(1, [0.0; 3]), "phone", 10, &[]),
        Err(StoreError::Domain(DomainError::StaleRequest))));
    let expected = db.player_snapshot(first.usn).unwrap();
    drop(db);
    let mut db = Database::open(&path).unwrap();
    assert_eq!(db.player_snapshot(first.usn).unwrap(), expected);
    let session = db.begin_login(b"guide-restart", DeviceClock::new(200, -300).unwrap()).unwrap();
    let make = |seq, rpc: &str| RpcMutation {
        nonce: session.nonce.clone(), identity: first.clone(), request_seq: seq,
        rpc: rpc.into(), idempotency_key: format!("guide-restart:{seq}"), committed_at: 200 + seq,
    };
    db.advance_follower_quest(&make(1, "setUserFollowerQuest"), 1, 2, 0.0,
        [2.0, 3.0, 4.0], Vec::new(), Some(2)).unwrap();
    let complete = db.advance_follower_quest(&make(2, "setUserFollowerQuest"), 1, 3, 0.0,
        [2.0, 3.0, 4.0], Vec::new(), Some(2)).unwrap();
    assert!(complete.next);
    assert_eq!(complete.quest.current_id, 2);
    assert_eq!(complete.quest.condition_values, [0.0; 3]);
    assert_eq!(complete.quest.claimed, [false; 3]);
    // Late prior-stage data cannot be relabeled as progress for the new stage.
    db.merge_user_save(&make(3, "userSave"), &patch(1, [99.0; 3]), "phone", 10, &[]).unwrap();
    db.merge_user_save(&make(4, "userSave"), &patch(2, [1.0, 0.0, 0.0]), "phone", 10, &[]).unwrap();
    let repeated = db.advance_follower_quest(&make(5, "setUserFollowerQuest"), 1, 3, 99.0,
        [2.0, 3.0, 4.0], Vec::new(), Some(2)).unwrap();
    assert!(!repeated.next);
    assert!(repeated.rewards.is_empty());
    assert_eq!(repeated.quest.condition_values, [1.0, 0.0, 0.0]);
    let expected = db.player_snapshot(first.usn).unwrap();
    assert_eq!(expected.follower_quests[0].claimed, [true; 3]);
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(db.player_snapshot(first.usn).unwrap(), expected);
    assert_eq!(db.connection.query_row("SELECT COUNT(*) FROM follower_quests WHERE usn=?1", [second.usn], |r| r.get::<_, i64>(0)).unwrap(), 0);
    drop(db);
    std::fs::remove_file(path).unwrap();
}
