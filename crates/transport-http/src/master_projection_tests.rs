//! Opt-in private-fixture checks. Neither original tables nor captures are
//! committed: callers provide databases extracted from their verified package.
use super::*;
use ggfm_master_data::MasterData;
use ggfm_persistence_sqlite::DatabaseActor;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[tokio::test]
#[ignore = "requires GGFM_ORIGINAL_MASTER and GGFM_PATCHED_MASTER from a locally verified XAPK"]
async fn static_master_bundle_matches_server_policy() {
    let original =
        PathBuf::from(std::env::var_os("GGFM_ORIGINAL_MASTER").expect("original master path"));
    let patched =
        PathBuf::from(std::env::var_os("GGFM_PATCHED_MASTER").expect("patched master path"));
    let mut master = MasterData::open_read_only(&original)
        .unwrap()
        .load_catalog()
        .unwrap();
    crate::apply_memorial_shop_policy(&mut master).unwrap();
    let patched = MasterData::open_read_only(&patched)
        .unwrap()
        .load_catalog()
        .unwrap();
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("ggfm-master-parity-{suffix}.sqlite"));
    let state = AppState {
        database: DatabaseActor::open(path.clone()).unwrap(),
        master: Arc::new(master),
        session: Arc::new(tokio::sync::Mutex::new(None)),
        capability: Arc::from("static-master-test"),
        capability_hash: Arc::from(Vec::<u8>::new()),
        endpoint: Arc::from("http://127.0.0.1:1"),
        asset_source: None,
        startup_notice_emitted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let schema = ProtocolSchema::embedded();
    let mut differences = Vec::new();
    let mut checked = 0;
    let overlay = PassMasterOverlay {
        active_season: 1,
        local_month: 9,
    };
    for (table, rows) in &state.master.game_data {
        // SubscribeList is a per-save daily server overlay, not a static patch.
        if table == "SubscribeList" {
            continue;
        }
        let result_type = schema
            .type_spec("main_model::GetGameDataListRetDataInfo")
            .unwrap()
            .fields
            .iter()
            .find(|field| field.name == *table)
            .unwrap()
            .wire_type
            .strip_prefix("[]")
            .unwrap();
        let fields = &schema
            .type_spec(&format!("main_model::{result_type}"))
            .unwrap()
            .fields;
        let patched_rows = &patched.game_data[table];
        assert_eq!(rows.len(), patched_rows.len(), "{table} row count");
        for (index, row) in rows.iter().enumerate() {
            let row_id = master_i64(row, "id");
            let id = row_id.unwrap_or(index as i64);
            let patched_row = match row_id {
                Some(id) => patched_rows
                    .iter()
                    .find(|r| master_i64(r, "id") == Some(id)),
                None => patched_rows.get(index),
            }
            .unwrap_or_else(|| panic!("patched {table} missing row {id}"));
            for field in fields {
                let key = normalize_column_name(&field.name);
                let Some(source) = row.get(&key) else {
                    continue;
                };
                let actual = transform_master_value(&state, table, &key, row, source, overlay);
                let expected = &patched_row[&key];
                let matches = match (&actual, expected) {
                    (MasterScalar::Text(a), MasterScalar::Text(b)) => a == b,
                    _ => match (master_f64_value(&actual), master_f64_value(expected)) {
                        (Some(a), Some(b)) => {
                            a == b || (a - b).abs() <= 1e-10 * a.abs().max(b.abs()).max(1.0)
                        }
                        _ => actual == *expected,
                    },
                };
                checked += 1;
                if !matches && differences.len() < 30 {
                    differences.push(format!(
                        "{table}[{id}].{key}: server={actual:?} patch={expected:?}"
                    ));
                }
            }
        }
    }
    // A matching published master alone is not sufficient: every mutation
    // must calculate levels with those same thresholds, not original ones.
    for row in &patched.follower_profile_levels {
        if !row.active { continue; }
        let thresholds = follower_profile_thresholds(&state, row.profile_id);
        assert_eq!(thresholds.iter().find(|r| r.0 == row.level).unwrap().1,
            row.required_exp as i64, "profile={} level={}", row.profile_id, row.level);
    }
    assert_eq!(ch3_profile_thresholds(&state), follower_profile_thresholds(&state, 100_000));
    let thresholds = follower_profile_thresholds(&state, 1);
    state.database.execute(move |db| {
        use ggfm_domain::{DeviceClock, RewardGrant, RewardPurchaseCommand};
        use ggfm_persistence_sqlite::RpcMutation;
        let user = db.ensure_default_slot(1000)?;
        let session = db.begin_login(b"gift-curve-regression", DeviceClock::new(1000, 0).unwrap())?;
        let mutation = |seq| RpcMutation { nonce: session.nonce, identity: user.clone(),
            request_seq: seq, rpc: "setFollowerProfileGift".into(),
            idempotency_key: format!("gift-curve:{seq}"), committed_at: 1000+seq };
        db.purchase_rewards(&mutation(1), RewardPurchaseCommand {
            transaction_id: "gift-fixture".into(), purchase_kind: "test".into(), target_id: 1,
            cost: None, rewards: vec![RewardGrant { reward_type: 11, reward_id: 1, amount: 10.0 }],
            ad_progression: None,
        })?;
        for seq in 2..=7 {
            let result = db.give_follower_gift(&mutation(seq), 1, 1, 1, 20, thresholds.clone())?;
            let next = thresholds.iter().find(|row| row.0 == result.profile.level + 1).unwrap();
            assert!(next.1 > result.profile.experience, "client must not calculate a negative send quantity");
            assert_eq!(db.give_follower_gift(&mutation(seq), 1, 1, 1, 20, thresholds.clone())?, result);
        }
        let snapshot = db.player_snapshot(user.usn)?;
        let joie = snapshot.ch3.profiles.iter().find(|row| row.profile_id == 1).unwrap();
        assert_eq!((joie.experience, joie.level), (120, 2));
        assert_eq!(snapshot.gifts.iter().find(|row| row.gift_id == 1).unwrap().quantity, 4);
        let batch = db.give_follower_gift(&mutation(8), 1, 1, 3, 20, thresholds.clone())?;
        assert_eq!((batch.remaining, batch.profile.experience, batch.profile.level), (1, 180, 3));
        assert_eq!(db.give_follower_gift(&mutation(8), 1, 1, 3, 20, thresholds.clone())?, batch);
        assert!(db.give_follower_gift(&mutation(9), 1, 1, -5, 20, thresholds).is_err());
        assert_eq!(db.player_snapshot(user.usn)?.gifts.iter().find(|row| row.gift_id == 1).unwrap().quantity, 1);
        Ok(())
    }).await.unwrap();
    drop(state);
    std::fs::remove_file(path).unwrap();
    eprintln!("Compared {checked} master scalar fields across 38 static tables");
    assert!(
        differences.is_empty(),
        "master policy drift:\n{}",
        differences.join("\n")
    );
}
