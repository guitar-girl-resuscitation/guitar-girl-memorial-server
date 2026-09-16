use ggfm_domain::{CurrencyKind, DomainError, GpAmount, Usn};
use rusqlite::{Connection, params};
use crate::StoreError;

pub(crate) fn read(db: &Connection, usn: Usn, kind: CurrencyKind) -> Result<GpAmount, StoreError> {
    let text: String = db.query_row("SELECT amount FROM gp_balances WHERE usn=?1 AND kind=?2",
        params![usn, kind.key()], |r| r.get(0))?;
    Ok(GpAmount::parse(&text)?)
}

pub(crate) fn set(db: &Connection, usn: Usn, kind: CurrencyKind, value: &GpAmount) -> Result<(), StoreError> {
    if !kind.is_gp() { return Err(DomainError::InvalidAmount.into()); }
    let changed = db.execute("UPDATE gp_balances SET amount=?1 WHERE usn=?2 AND kind=?3",
        params![value.to_string(), usn, kind.key()])?;
    if changed != 1 { return Err(StoreError::Integrity("missing game-point balance".into())); }
    Ok(())
}

pub(crate) fn credit(db: &Connection, usn: Usn, kind: CurrencyKind, amount: f64) -> Result<(), StoreError> {
    let value = read(db, usn, kind)?.checked_add(&GpAmount::from_legacy(amount)?)?;
    set(db, usn, kind, &value)
}

pub(crate) fn debit(db: &Connection, usn: Usn, kind: CurrencyKind, amount: f64) -> Result<(), StoreError> {
    let value = read(db, usn, kind)?.checked_sub(&GpAmount::from_legacy(amount)?)?;
    set(db, usn, kind, &value)
}

pub(crate) fn migrate_v4(db: &mut Connection) -> Result<(), StoreError> {
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute_batch(include_str!("../migrations/0005_exact_gp.sql"))?;
    {
        let mut stmt = tx.prepare("SELECT usn,kind,amount FROM currencies WHERE kind IN ('ch1_like','ch2_like','ch2_note')")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let usn: i64 = row.get(0)?;
            let kind: String = row.get(1)?;
            let old: f64 = row.get(2)?;
            // Explicit memorial repair policy, not an inverse of float overflow:
            // positive infinity is estimated as 1 DB (10^318). Never apply this
            // to finite balances, negative values, or future invalid saves.
            let exact = if old == f64::INFINITY {
                let estimated = format!("1{}", "0".repeat(318));
                let summary = serde_json::json!({
                    "kind": kind, "previous": "+Infinity", "replacement": estimated,
                    "estimated": true, "reason": "legacy_gp_overflow_1DB_fallback"
                }).to_string();
                tx.execute(concat!(
                    "INSERT INTO mutation_ledger(usn,ledger_id,request_seq,rpc,idempotency_key,committed_at,summary_json) ",
                    "SELECT ?1,COALESCE(MAX(ledger_id),0)+1,0,'migration.gp.v5',?2,unixepoch(),?3 ",
                    "FROM mutation_ledger WHERE usn=?1"
                ), params![usn,format!("migration.gp.v5.infinity.{kind}"),summary])?;
                GpAmount::parse(&estimated)?
            } else {
                GpAmount::from_legacy(old).map_err(|_| StoreError::UnrecoverableBalance(
                    format!("{kind}, slot {usn}, non-finite or negative value")))?
            };
            tx.execute("INSERT INTO gp_balances(usn,kind,amount) VALUES(?1,?2,?3)", params![usn,kind,exact.to_string()])?;
        }
    }
    tx.execute("DELETE FROM currencies WHERE kind IN ('ch1_like','ch2_like','ch2_note')", [])?;
    tx.execute("INSERT INTO schema_migrations(version,applied_at) VALUES(5,unixepoch())", [])?;
    tx.pragma_update(None, "user_version", 5)?;
    tx.commit()?;
    Ok(())
}

/// Float currencies retain their protocol representation, but additions must
/// not persist infinity even if both operands were individually finite.
pub(crate) fn credit_currency(db: &Connection, usn: Usn, kind: CurrencyKind, amount: f64) -> Result<(), StoreError> {
    if kind.is_gp() { return credit(db, usn, kind, amount); }
    if !amount.is_finite() || amount < 0.0 { return Err(DomainError::InvalidAmount.into()); }
    let old: f64 = db.query_row("SELECT amount FROM currencies WHERE usn=?1 AND kind=?2",
        params![usn,kind.key()], |r| r.get(0))?;
    let next = old + amount;
    if !next.is_finite() { return Err(DomainError::AmountTooLarge.into()); }
    db.execute("UPDATE currencies SET amount=?1 WHERE usn=?2 AND kind=?3", params![next,usn,kind.key()])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Database, MIGRATION_1, RpcMutation};
    use ggfm_domain::{AreaSavePatch, DeviceClock, UserSavePatch};

    fn legacy(amount: f64) -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(MIGRATION_1).unwrap();
        c.pragma_update(None, "user_version", 4).unwrap();
        for usn in [1, 2] {
            c.execute("INSERT INTO save_slots VALUES(?1,?2,'preserved',10,20)", params![usn,usn.to_string()]).unwrap();
            for kind in ["ch1_like", "ch2_like", "ch2_note"] {
                c.execute("INSERT INTO currencies VALUES(?1,?2,?3)", params![usn,kind,amount]).unwrap();
            }
            c.execute("INSERT INTO currencies VALUES(?1,'candy',777)", [usn]).unwrap();
            c.execute("INSERT INTO one_time_flags VALUES(?1,'tutorial',1,20)", [usn]).unwrap();
        }
        c
    }

    #[test]
    fn v4_migration_preserves_slots_and_exact_representable_legacy_integer() {
        let mut c = legacy(1e308);
        migrate_v4(&mut c).unwrap();
        for usn in [1, 2] {
            for kind in [CurrencyKind::Ch1Like,CurrencyKind::Ch2Like,CurrencyKind::Ch2Note] {
                assert_eq!(read(&c,usn,kind).unwrap(),GpAmount::from_legacy(1e308).unwrap());
            }
        }
        assert_eq!(c.query_row("SELECT sum(amount) FROM currencies",[],|r|r.get::<_,f64>(0)).unwrap(),1554.0);
        assert_eq!(c.query_row("SELECT count(*) FROM one_time_flags WHERE value=1",[],|r|r.get::<_,i64>(0)).unwrap(),2);
        assert_eq!(c.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0)).unwrap(),5);
        assert!(!c.prepare("PRAGMA foreign_key_check").unwrap().exists([]).unwrap());
    }

    #[test]
    fn infinity_is_estimated_once_and_finite_balances_are_preserved() {
        let mut c = legacy(f64::INFINITY);
        c.execute("UPDATE currencies SET amount=42 WHERE usn=2 AND kind='ch1_like'",[]).unwrap();
        migrate_v4(&mut c).unwrap();
        assert_eq!(read(&c,1,CurrencyKind::Ch1Like).unwrap().to_string(),format!("1{}","0".repeat(318)));
        assert_eq!(read(&c,2,CurrencyKind::Ch1Like).unwrap().to_string(),"42");
        assert_eq!(c.query_row("SELECT count(*) FROM mutation_ledger WHERE json_extract(summary_json,'$.estimated')=1",[],|r|r.get::<_,i64>(0)).unwrap(),5);
        assert_eq!(c.query_row("SELECT sum(amount) FROM currencies",[],|r|r.get::<_,f64>(0)).unwrap(),1554.0);
        let db=Database::configure(c).unwrap();
        let tx=db.connection.unchecked_transaction().unwrap();
        debit(&tx,1,CurrencyKind::Ch1Like,7.0).unwrap();
        tx.commit().unwrap();
        let db=Database::configure(db.connection).unwrap();
        assert_eq!(db.gp_balance(1,CurrencyKind::Ch1Like).unwrap(),GpAmount::parse(&format!("1{}","0".repeat(318))).unwrap().checked_sub(&GpAmount::parse("7").unwrap()).unwrap());
        assert_eq!(db.connection.query_row("SELECT count(*) FROM mutation_ledger",[],|r|r.get::<_,i64>(0)).unwrap(),5);
    }

    #[test]
    fn invalid_negative_rolls_back_migration_and_estimation_records() {
        let mut c = legacy(f64::INFINITY);
        // Simulate an already malformed legacy database; normal writes forbid it.
        c.pragma_update(None,"ignore_check_constraints",true).unwrap();
        c.execute("UPDATE currencies SET amount=-1 WHERE usn=2 AND kind='ch2_note'",[]).unwrap();
        c.pragma_update(None,"ignore_check_constraints",false).unwrap();
        assert!(migrate_v4(&mut c).is_err());
        assert_eq!(c.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0)).unwrap(),4);
        assert!(!c.prepare("SELECT 1 FROM sqlite_schema WHERE name='gp_balances'").unwrap().exists([]).unwrap());
        assert_eq!(c.query_row("SELECT count(*) FROM currencies",[],|r|r.get::<_,i64>(0)).unwrap(),8);
        assert_eq!(c.query_row("SELECT count(*) FROM mutation_ledger",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }

    #[test]
    fn huge_save_spend_replay_limit_and_slot_isolation() {
        let mut db = Database::open_memory().unwrap();
        let user = db.ensure_default_slot(10).unwrap();
        let second = db.create_slot("other",11).unwrap();
        let session = db.begin_login(b"huge-save", DeviceClock::new(12,0).unwrap()).unwrap();
        db.connection.execute("INSERT INTO achievement_progress(usn,achievement_id,quantity,quantity_text,updated_at) VALUES(?1,3,500,'0',12) ON CONFLICT(usn,achievement_id) DO UPDATE SET quantity=500,quantity_text='0'",[user.usn]).unwrap();
        let mutation = RpcMutation { nonce:session.nonce,identity:user.clone(),request_seq:1,rpc:"userSave".into(),idempotency_key:"big-1".into(),committed_at:13 };
        let huge = format!("1{}7", "0".repeat(317));
        let patch = UserSavePatch { areas: vec![AreaSavePatch{area:1,gp1:Some(huge.clone()),..Default::default()},AreaSavePatch{area:2,gp1:Some(huge.clone()),gp2:Some(huge.clone()),..Default::default()}],
            achievements:vec![ggfm_domain::ProgressSavePatch{id:2,quantity:f64::INFINITY,quantity_text:Some(huge.clone())},
                ggfm_domain::ProgressSavePatch{id:3,quantity:1.0,quantity_text:Some("1".into())}],..Default::default() };
        db.merge_user_save(&mutation,&patch,"big-device",10,&[]).unwrap();
        assert_eq!(db.player_snapshot(user.usn).unwrap().achievements.iter().find(|p|p.id==2).unwrap().quantity_text,huge);
        assert_eq!(db.player_snapshot(user.usn).unwrap().achievements.iter().find(|p|p.id==3).unwrap().quantity_text,"500");
        db.merge_user_save(&mutation,&patch,"big-device",10,&[]).unwrap();
        for kind in [CurrencyKind::Ch1Like,CurrencyKind::Ch2Like,CurrencyKind::Ch2Note] {
            assert_eq!(db.gp_balance(user.usn,kind).unwrap().to_string(),huge);
            assert_eq!(db.gp_balance(second.usn,kind).unwrap().to_string(),"0");
        }
        let tx = db.connection.transaction().unwrap();
        debit(&tx,user.usn,CurrencyKind::Ch1Like,4.0).unwrap();
        credit(&tx,user.usn,CurrencyKind::Ch1Like,4.0).unwrap();
        tx.commit().unwrap();
        assert_eq!(db.gp_balance(user.usn,CurrencyKind::Ch1Like).unwrap().to_string(),huge);
        let invalid = UserSavePatch { areas:vec![AreaSavePatch{area:1,gp1:Some("1".repeat(ggfm_domain::MAX_GP_DIGITS+1)),..Default::default()}],..Default::default() };
        let next = RpcMutation{request_seq:2,idempotency_key:"too-large".into(),..mutation};
        assert!(db.merge_user_save(&next,&invalid,"bad-device",10,&[]).is_err());
        assert_eq!(db.player_snapshot(user.usn).unwrap().device_uuid,"big-device");
        assert_eq!(db.gp_balance(user.usn,CurrencyKind::Ch1Like).unwrap().to_string(),huge);
        // A fresh connection sees the same schema and exact balance.
        let file=std::env::temp_dir().join(format!("ggfm-gp-{}-{}.sqlite",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        db.backup_to(&file).unwrap();
        let reopened=Database::open(&file).unwrap();
        assert_eq!(reopened.gp_balance(user.usn,CurrencyKind::Ch1Like).unwrap().to_string(),huge);
        drop(reopened);
        std::fs::remove_file(file).unwrap();
    }
}
