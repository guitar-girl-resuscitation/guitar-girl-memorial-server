#[test]
fn attendance_and_pass_use_local_midnight_not_utc_or_restart() {
    for offset in [-720, -360, -300, -210, 0, 330, 345, 540, 600, 840] {
        let (mut db, user) = setup();
        let midnight = 20_000 * 86_400 - i64::from(offset) * 60;
        let initial = DeviceClock::new(midnight + 1, offset).unwrap();
        db.select_pass_season(user.usn, initial, 8).unwrap();
        for (index, seconds, expected_count) in
            [(0, 1, 1), (1, 43_200, 1), (2, 86_399, 1),
             (3, 86_400, 2), (4, 86_401, 2)]
        {
            let clock = DeviceClock::new(midnight + seconds, offset).unwrap();
            // Re-login invalidates the previous session, not the day's claim.
            let session = db.begin_login(format!("tz:{offset}:{index}").as_bytes(), clock).unwrap();
            let mutation = RpcMutation {
                nonce: session.nonce, identity: user.clone(), request_seq: 1,
                rpc: "setAttendance".into(),
                idempotency_key: format!("tz:{offset}:{index}"),
                committed_at: clock.unix_seconds,
            };
            let result = db.attendance(&mutation, clock, "add").unwrap();
            assert_eq!(result.attendance_count, expected_count, "offset={offset} seconds={seconds}");
            let anchor = db.pass_snapshot(user.usn).unwrap().anchor;
            assert_eq!(anchor.anchor_day, initial.local_epoch_day());
            assert_eq!(anchor.anchor_season, 8);
        }
    }
}

#[test]
fn daylight_saving_repeated_and_skipped_hours_do_not_create_a_new_day() {
    let (mut db, user) = setup();
    // Fall back: 01:59:59 UTC-5 becomes 01:00:00 UTC-6 one real second later.
    // Spring forward: 01:59:59 UTC-6 becomes 03:00:00 UTC-5.
    for (case, offsets, utc_hour) in [(0, [-300, -360], 7), (1, [-360, -300], 8)] {
        let day = 20_000 + case;
        for (index, offset) in offsets.into_iter().enumerate() {
            let now = day * 86_400 + utc_hour * 3600 - 1 + index as i64;
            let clock = DeviceClock::new(now, offset).unwrap();
            assert_eq!(clock.local_epoch_day(), day);
            let session = db.begin_login(format!("dst:{case}:{index}").as_bytes(), clock).unwrap();
            let mutation = RpcMutation {
                nonce: session.nonce, identity: user.clone(), request_seq: 1,
                rpc: "setAttendance".into(), idempotency_key: format!("dst:{case}:{index}"),
                committed_at: now,
            };
            assert_eq!(db.attendance(&mutation, clock, "add").unwrap().attendance_count, (case + 1) as i32);
        }
    }
}
