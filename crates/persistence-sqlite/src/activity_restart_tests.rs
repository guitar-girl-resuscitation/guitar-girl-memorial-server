#[test]
fn fever_reward_seconds_extend_once_and_expired_timer_restarts_from_device_time() {
    let (mut db, user) = setup();
    let other = db.create_slot("Other fever", 100).unwrap();
    let session = db
        .begin_login(b"fever", DeviceClock::new(200, 600).unwrap())
        .unwrap();
    for (seq, now, seconds, expected_end) in [
        (1, 200, 300.0, 500),
        (2, 250, 600.0, 1100),
        (3, 2000, 60.0, 2060),
    ] {
        let mutation = RpcMutation {
            nonce: session.nonce,
            identity: user.clone(),
            request_seq: seq,
            rpc: "buyShop".into(),
            idempotency_key: format!("fever:{seq}"),
            committed_at: now,
        };
        let command = RewardPurchaseCommand {
            transaction_id: format!("fever-purchase:{seq}"),
            purchase_kind: "shop".into(),
            target_id: 9,
            cost: None,
            rewards: vec![RewardGrant {
                reward_type: 6,
                reward_id: 2,
                amount: seconds,
            }],
            ad_progression: None,
        };
        assert_eq!(
            db.purchase_rewards(&mutation, command.clone())
                .unwrap()
                .rewards
                .len(),
            1
        );
        assert!(
            db.purchase_rewards(&mutation, command)
                .unwrap()
                .rewards
                .is_empty()
        );
        let snapshot = db.player_snapshot(user.usn).unwrap();
        assert_eq!(
            snapshot.buff_timers,
            vec![ggfm_domain::BuffTimerSnapshot {
                buff_id: 2,
                active_at: now,
                expires_at: expected_end,
            }]
        );
        assert_eq!(
            snapshot
                .owned_content
                .iter()
                .filter(|row| row.kind == ContentKind::Buff)
                .count(),
            1
        );
    }
    let path = std::env::temp_dir().join(format!(
        "ggfm-fever-restart-{}.sqlite",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    db.backup_to(&path).unwrap();
    drop(db);
    let mut db = Database::open(&path).unwrap();
    db.begin_login(b"fever-reopened", DeviceClock::new(10000, -480).unwrap())
        .unwrap();
    // Restart/timezone changes do not refresh expired Fever or add 900 seconds.
    assert_eq!(
        db.player_snapshot(user.usn).unwrap().buff_timers[0].expires_at,
        2060
    );
    assert!(
        db.connection
            .prepare("SELECT 1 FROM buff_timers WHERE usn=?1")
            .unwrap()
            .query_row([other.usn], |_| Ok(()))
            .optional()
            .unwrap()
            .is_none()
    );
    drop(db);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn fan_multipliers_share_shop_pass_and_mail_settlement_with_owned_bonuses() {
    let (mut db, user) = setup();
    let other = db.create_slot("Other fans", 101).unwrap();
    db.configure_reward_rules(ggfm_domain::RewardRules {
        costume_fan_bonuses: [
            (1, (1, 0.0)),
            (2, (1, 0.3)),
            (201, (2, 0.0)),
            (202, (2, 0.5)),
        ]
        .into_iter()
        .collect(),
    })
    .unwrap();
    for (area, id) in [(1, 2), (2, 202)] {
        db.connection.execute("INSERT INTO owned_content(usn,kind,area,content_id,acquired_at,source) VALUES(?1,'costume',?2,?3,101,'fixture')",
            params![user.usn, area, id]).unwrap();
    }
    let session = db
        .begin_login(b"fan-paths", DeviceClock::new(200, 0).unwrap())
        .unwrap();
    let make = |seq, rpc: &str| RpcMutation {
        nonce: session.nonce,
        identity: user.clone(),
        request_seq: seq,
        rpc: rpc.into(),
        idempotency_key: format!("fan:{seq}:{rpc}"),
        committed_at: 200 + seq,
    };
    for (index, id) in [8, 9].into_iter().enumerate() {
        let reward = RewardGrant {
            reward_type: 1,
            reward_id: id,
            amount: 1000.0,
        };
        let seq = index as i64 * 3;
        let command = || RewardPurchaseCommand {
            transaction_id: format!("fan-shop:{id}"),
            purchase_kind: "shop".into(),
            target_id: i64::from(id),
            cost: None,
            rewards: vec![reward.clone()],
            ad_progression: None,
        };
        let mutation = make(seq + 1, "buyShop");
        assert_eq!(
            db.purchase_rewards(&mutation, command()).unwrap().rewards,
            vec![reward.clone()]
        );
        assert!(
            db.purchase_rewards(&mutation, command())
                .unwrap()
                .rewards
                .is_empty()
        );
        let mutation = make(seq + 2, "provideSubscribePassReward");
        db.claim_pass_reward(
            &mutation,
            1,
            0,
            0,
            id,
            0,
            std::slice::from_ref(&reward),
            1,
            0,
        )
        .unwrap();
        assert!(
            db.claim_pass_reward(
                &mutation,
                1,
                0,
                0,
                id,
                0,
                std::slice::from_ref(&reward),
                1,
                0
            )
            .unwrap()
            .rewards
            .is_empty()
        );
        let tx = db.connection.transaction().unwrap();
        queue_reward_mail_tx(&tx, user.usn, i64::from(id), "s", "b", &reward, 200).unwrap();
        tx.commit().unwrap();
        let mutation = make(seq + 3, "providePost");
        db.claim_mail_rpc(&mutation, i64::from(id)).unwrap();
        db.claim_mail_rpc(&mutation, i64::from(id)).unwrap();
    }
    let snapshot = db.player_snapshot(user.usn).unwrap();
    // CH1 1.3f32 * 1000 truncates to 1299, CH2 1.5f32 * 1000 to 1500.
    assert_eq!(snapshot.currency(CurrencyKind::Fans), 3897.0);
    assert_eq!(
        snapshot
            .areas
            .iter()
            .find(|row| row.area == 1)
            .unwrap()
            .fans,
        3897.0
    );
    assert_eq!(
        snapshot
            .areas
            .iter()
            .find(|row| row.area == 2)
            .unwrap()
            .fans,
        4500.0
    );
    assert_eq!(db.currency(other.usn, CurrencyKind::Fans).unwrap(), 0.0);
}

#[test]
fn guide_each_task_and_reward_remain_independent_through_four_stage_restarts() {
    let (mut initial, user) = setup();
    let other = initial.create_slot("Other guide", 101).unwrap();
    initial
        .begin_login(b"guide-initial", DeviceClock::new(200, 0).unwrap())
        .unwrap();
    let path = std::env::temp_dir().join(format!(
        "ggfm-guide-restart-{}.sqlite",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    initial.backup_to(&path).unwrap();
    drop(initial);
    for stage in 1..=4 {
        for task in 1..=3 {
            // Actually close/reopen the database between individual tasks,
            // not only between stages or after the whole chain is complete.
            let mut db = Database::open(&path).unwrap();
            let session = db
                .begin_login(
                    format!("guide:{stage}:{task}").as_bytes(),
                    DeviceClock::new(200 + stage * 10 + task, 0).unwrap(),
                )
                .unwrap();
            let mutation = RpcMutation {
                nonce: session.nonce,
                identity: user.clone(),
                request_seq: 1,
                rpc: "setUserFollowerQuest".into(),
                idempotency_key: format!("guide:{stage}:{task}"),
                committed_at: session.clock.unix_seconds,
            };
            let run = |db: &mut Database, mutation: &RpcMutation| {
                db.advance_follower_quest(
                    mutation,
                    stage,
                    task,
                    task as f64,
                    [1.0, 2.0, 3.0],
                    vec![RewardGrant {
                        reward_type: 1,
                        reward_id: 1,
                        amount: 1.0,
                    }],
                    Some(stage + 1),
                )
            };
            let result = run(&mut db, &mutation).unwrap();
            assert_eq!(result.next, task == 3);
            assert_eq!(
                result.complete_id,
                if task == 3 { stage } else { stage - 1 }
            );
            assert_eq!(result.rewards.len(), 1);
            assert!(run(&mut db, &mutation).unwrap().rewards.is_empty());
            assert!(!run(&mut db, &mutation).unwrap().next);
            // A new transport request for the same claimed task is also safe.
            let duplicate = RpcMutation {
                request_seq: 2,
                idempotency_key: format!("duplicate:{stage}:{task}"),
                ..mutation
            };
            assert!(run(&mut db, &duplicate).unwrap().rewards.is_empty());
            let snapshot = db.player_snapshot(user.usn).unwrap();
            assert_eq!(
                snapshot.follower_quests.last().unwrap().complete_id,
                result.complete_id
            );
            for previous in 1..stage {
                let row = snapshot
                    .follower_quests
                    .iter()
                    .find(|row| row.current_id == previous)
                    .unwrap();
                assert!(row.completed);
                assert_eq!(row.claimed, [true; 3]);
            }
            let row = snapshot
                .follower_quests
                .iter()
                .find(|row| row.current_id == stage)
                .unwrap();
            assert_eq!(row.claimed, [true, task >= 2, task == 3]);
            assert_eq!(
                row.condition_values,
                [
                    1.0,
                    if task >= 2 { 2.0 } else { 0.0 },
                    if task == 3 { 3.0 } else { 0.0 }
                ]
            );
            if task == 3 {
                let next = snapshot
                    .follower_quests
                    .iter()
                    .find(|row| row.current_id == stage + 1)
                    .unwrap();
                assert_eq!(next.claimed, [false; 3]);
                assert_eq!(next.condition_values, [0.0; 3]);
            }
            assert_eq!(
                db.currency(user.usn, CurrencyKind::Chocolate).unwrap(),
                ((stage - 1) * 3 + task) as f64
            );
            assert_eq!(
                db.currency(other.usn, CurrencyKind::Chocolate).unwrap(),
                0.0
            );
        }
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn ch3_energy_recovers_while_server_is_closed_without_login_date_side_effects() {
    let (mut db, user) = setup();
    let other = db.create_slot("Other energy", 100).unwrap();
    db.connection
        .execute(
            "UPDATE ch3_energy SET amount=3,cap=25,calculated_at=200 WHERE usn=?1",
            [user.usn],
        )
        .unwrap();
    let path = std::env::temp_dir().join(format!(
        "ggfm-energy-restart-{}.sqlite",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    db.backup_to(&path).unwrap();
    drop(db);
    let mut reopened = Database::open(&path).unwrap();
    // No ticking task ran between 200 and 207. Offset is intentionally not
    // involved in elapsed-duration restoration.
    assert_eq!(
        reopened
            .ch3_snapshot_at(user.usn, 207, 25)
            .unwrap()
            .energy
            .amount,
        10
    );
    assert_eq!(
        reopened
            .ch3_snapshot_at(user.usn, 207, 25)
            .unwrap()
            .energy
            .amount,
        10
    );
    assert_eq!(
        reopened
            .ch3_snapshot_at(user.usn, 190, 25)
            .unwrap()
            .energy
            .amount,
        10
    );
    assert_eq!(
        reopened
            .ch3_snapshot_at(user.usn, 193, 25)
            .unwrap()
            .energy
            .amount,
        13
    );
    assert_eq!(
        reopened
            .ch3_snapshot_at(user.usn, 100000, 25)
            .unwrap()
            .energy
            .amount,
        25
    );
    assert_eq!(
        reopened
            .ch3_snapshot_raw(other.usn)
            .unwrap()
            .energy
            .calculated_at,
        100
    );
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn samseck_completion_survives_relogin_reopen_and_does_not_resend_mail() {
    let (mut db, user) = setup();
    let other = db.create_slot("Other", 101).unwrap();
    db.select_pass_season(other.usn, DeviceClock::new(101, 0).unwrap(), 1)
        .unwrap();
    // Fixture eligibility, not an alternate implementation of the activity.
    db.connection.execute(
        "UPDATE content_levels SET level=200 WHERE usn=?1 AND kind='character' AND content_id=1",
        [user.usn],
    ).unwrap();
    let session = db
        .begin_login(b"samseck-first", DeviceClock::new(200, 0).unwrap())
        .unwrap();
    let make = |seq, nonce| RpcMutation {
        nonce,
        identity: user.clone(),
        request_seq: seq,
        rpc: "setSamSeck".into(),
        idempotency_key: format!("samseck:{nonce}:{seq}"),
        committed_at: 200 + seq,
    };
    let reward = || RewardGrant {
        reward_type: 1,
        reward_id: 1,
        amount: 5.0,
    };
    assert_eq!(
        db.claim_sam_seck(&make(1, session.nonce), 1, 8001, reward())
            .unwrap(),
        1
    );
    assert_eq!(
        db.claim_sam_seck(&make(1, session.nonce), 1, 8001, reward())
            .unwrap(),
        1
    );
    assert_eq!(db.mailbox(user.usn).unwrap().mail.len(), 1);
    let claim = RpcMutation {
        rpc: "providePost".into(),
        ..make(2, session.nonce)
    };
    db.claim_mail_rpc(&claim, 8001).unwrap();
    assert_eq!(db.currency(user.usn, CurrencyKind::Chocolate).unwrap(), 5.0);

    let session = db
        .begin_login(b"samseck-relogin", DeviceClock::new(900, 600).unwrap())
        .unwrap();
    assert_eq!(db.player_snapshot(user.usn).unwrap().samseck_step, 1);
    assert_eq!(db.player_snapshot(other.usn).unwrap().samseck_step, 0);
    assert_eq!(
        db.claim_sam_seck(&make(1, session.nonce), 1, 8001, reward())
            .unwrap(),
        1
    );
    assert_eq!(
        db.mailbox(user.usn)
            .unwrap()
            .mail
            .iter()
            .filter(|mail| !mail.claimed)
            .count(),
        0
    );

    let path = std::env::temp_dir().join(format!(
        "ggfm-samseck-restart-{}.sqlite",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    db.backup_to(&path).unwrap();
    drop(db);
    let mut reopened = Database::open(&path).unwrap();
    let session = reopened
        .begin_login(b"samseck-reopen", DeviceClock::new(90000, -480).unwrap())
        .unwrap();
    assert_eq!(reopened.player_snapshot(user.usn).unwrap().samseck_step, 1);
    assert_eq!(
        reopened
            .claim_sam_seck(&make(1, session.nonce), 1, 8001, reward())
            .unwrap(),
        1
    );
    assert_eq!(reopened.mailbox(user.usn).unwrap().mail.len(), 1);
    assert_eq!(
        reopened
            .currency(user.usn, CurrencyKind::Chocolate)
            .unwrap(),
        5.0
    );
    assert!(reopened.mailbox(other.usn).unwrap().mail.is_empty());
    assert_eq!(reopened.player_snapshot(other.usn).unwrap().samseck_step, 0);
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}
