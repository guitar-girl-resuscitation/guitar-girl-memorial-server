mod actor;
pub mod transfer;

use std::{collections::BTreeMap, path::Path};

use ggfm_domain::{
    AdLevelProgression, AdLevelSnapshot, AdProfileGrant, AdRewardOutcome, AdStateSnapshot,
    AreaSnapshot, AttendanceResult, BookmarkSnapshot, Ch3ChapterClaimSnapshot, Ch3EnergySnapshot,
    Ch3Settlement, Ch3Snapshot, Ch3StageDefinition, Ch3StageSnapshot, Ch3StreamSettlement,
    ContentKind, ContentLevelSnapshot, ContentPurchaseCommand, ContentPurchaseOutcome,
    CurrencyKind, DeviceClock, DomainError, FollowerGiftOutcome, FollowerProfileRewardOutcome,
    FollowerProfileSnapshot, FollowerQuestOutcome, FollowerQuestSnapshot, GameRewardClaimCommand,
    LoginSession, MailSnapshot, MailboxSnapshot, MessengerSnapshot, MusicEncoreRewardOutcome,
    MusicReviewSnapshot, MusicScoreSnapshot, MusicStateSnapshot, OwnedContentSnapshot,
    PackagePurchaseOutcome, PassAnchorSnapshot, PassClaimOutcome, PassClaimSnapshot,
    PassFillOutcome, PassProgressSnapshot, PassSnapshot, PlayerSnapshot, ProgressSnapshot,
    PurchaseOutcome, Reward, RewardGrant, RewardPurchaseCommand, SelectRewardSnapshot,
    SkillActivationSnapshot, UserIdentity, UserSavePatch, Usn,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};

pub use actor::DatabaseActor;

const MIGRATION_1: &str = include_str!("../migrations/0001_initial.sql");
const USER_ID_BASE: i64 = 1_788_360_000_000;
const DATABASE_SCHEMA_VERSION: i64 = 4;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("slot {0} does not exist")]
    MissingSlot(Usn),
    #[error("no active save slot")]
    NoActiveSlot,
    #[error("mail {0} does not exist")]
    MissingMail(i64),
    #[error("mail {0} was already claimed")]
    MailAlreadyClaimed(i64),
    #[error("database actor unavailable: {0}")]
    ActorUnavailable(String),
    #[error("mutation replay payload is invalid: {0}")]
    InvalidReplay(String),
    #[error("idempotency key was reused for a different request")]
    IdempotencyConflict,
    #[error("active or pending slot {0} cannot be deleted")]
    ActiveSlot(Usn),
    #[error("database integrity check failed: {0}")]
    Integrity(String),
    #[error(
        "save database schema {actual} is unsupported (expected {expected}); this development build intentionally does not migrate test saves"
    )]
    UnsupportedSchema { actual: i64, expected: i64 },
    #[error("reward type {0} is not supported by the memorial inventory")]
    UnsupportedReward(String),
}

#[derive(Clone, Debug)]
pub struct RpcMutation {
    pub nonce: i64,
    pub identity: UserIdentity,
    pub request_seq: i64,
    pub rpc: String,
    pub idempotency_key: String,
    pub committed_at: i64,
}

pub struct Database {
    connection: Connection,
    reward_rules: std::sync::Arc<ggfm_domain::RewardRules>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotView {
    pub usn: Usn,
    pub user_id: String,
    pub display_name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub active: bool,
    pub pending: bool,
}

impl Database {
    /// Actual connection settings for startup diagnostics; contains no player data.
    pub fn connection_diagnostics(&self) -> Result<String, StoreError> {
        let schema: i64 = self.connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let journal: String = self.connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        let foreign_keys: i64 = self.connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        let synchronous: i64 = self.connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
        Ok(format!("schema={schema} journal={journal} foreign_keys={foreign_keys} synchronous={synchronous}"))
    }

    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        Self::configure(connection)
    }

    pub fn open_memory() -> Result<Self, StoreError> {
        Self::configure(Connection::open_in_memory()?)
    }

    fn configure(connection: Connection) -> Result<Self, StoreError> {
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let integrity: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(StoreError::Integrity(integrity));
        }
        let version =
            connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
        if version == 0 {
            connection.execute_batch(MIGRATION_1)?;
            connection.pragma_update(None, "user_version", DATABASE_SCHEMA_VERSION)?;
        } else if version != DATABASE_SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema {
                actual: version,
                expected: DATABASE_SCHEMA_VERSION,
            });
        }
        Ok(Self { connection, reward_rules: std::sync::Arc::default() })
    }

    pub fn configure_reward_rules(&mut self, rules: ggfm_domain::RewardRules) -> Result<(), StoreError> {
        if rules.costume_fan_bonuses.iter().any(|(id, (area, bonus))|
            *id <= 0 || !matches!(area, 1 | 2) || !bonus.is_finite() || *bonus < 0.0) {
            return Err(DomainError::InvalidAmount.into());
        }
        self.reward_rules = std::sync::Arc::new(rules);
        Ok(())
    }

    fn commit_reward_mutation<T, F>(&mut self, mutation: &RpcMutation, command: F) -> Result<T, StoreError>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce(&rusqlite::Transaction<'_>, &ggfm_domain::RewardRules) -> Result<(T, Option<T>), StoreError>,
    {
        let rules = self.reward_rules.clone();
        self.commit_rpc_mutation_with_replay(mutation, |tx| command(tx, &rules))
    }

    /// Writes a self-contained, transactionally consistent SQLite snapshot.
    /// The caller supplies a new path and publishes it only after this returns.
    pub fn backup_to(&mut self, destination: &Path) -> Result<(), StoreError> {
        let destination = destination.to_string_lossy().into_owned();
        self.connection.execute("VACUUM INTO ?1", [&destination])?;
        Ok(())
    }

    /// Flushes committed WAL pages without ending the current login session.
    ///
    /// Android can move the Activity to the background without terminating the
    /// process.  That lifecycle edge is a durability boundary, not a logout.
    pub fn checkpoint(&mut self) -> Result<(), StoreError> {
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(FULL)")?;
        Ok(())
    }

    pub fn create_slot(
        &mut self,
        display_name: &str,
        now: i64,
    ) -> Result<UserIdentity, StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let identity = Self::create_slot_in_transaction(&tx, display_name, now)?;
        tx.commit()?;
        Ok(identity)
    }

    pub fn create_slot_rpc(
        &mut self,
        mutation: &RpcMutation,
        display_name: &str,
    ) -> Result<UserIdentity, StoreError> {
        let display_name = display_name.to_owned();
        self.commit_rpc_mutation(mutation, |tx| {
            Self::create_slot_in_transaction(tx, &display_name, mutation.committed_at)
        })
    }

    fn create_slot_in_transaction(
        tx: &rusqlite::Transaction<'_>,
        display_name: &str,
        now: i64,
    ) -> Result<UserIdentity, StoreError> {
        let usn: i64 =
            tx.query_row("SELECT COALESCE(MAX(usn),0)+1 FROM save_slots", [], |row| {
                row.get(0)
            })?;
        let user_id = (USER_ID_BASE + usn).to_string();
        tx.execute("INSERT INTO save_slots(usn,user_id,display_name,created_at,updated_at) VALUES(?1,?2,?3,?4,?4)", params![usn,user_id,display_name,now])?;
        tx.execute(
            concat!(
                "INSERT INTO user_core(",
                "usn,nickname,fan_level,avatar_id,title_id,last_save_time,device_uuid",
                ") VALUES(?1,'Guitar Girl',1,1,1,?2,'reborn-local-device')"
            ),
            params![usn, now],
        )?;
        tx.execute("INSERT INTO attendance_state(usn) VALUES(?1)", [usn])?;
        tx.execute("INSERT INTO mail_state(usn) VALUES(?1)", [usn])?;
        tx.execute(
            concat!(
                "INSERT INTO area_state(usn,area,current_costume,current_guitar,current_music,unlocked) ",
                "VALUES(?1,1,1,1,1,1)"
            ),
            [usn],
        )?;
        // The tested Rust login baseline always serializes both Area_data map
        // entries. Area 2 starts present but locked; omitting the row changes
        // the complete userLogin response tree and wakes the client's CH2
        // initialization path as if the save were malformed.
        tx.execute(
            concat!(
                "INSERT INTO area_state(usn,area,current_costume,current_guitar,current_music,unlocked) ",
                "VALUES(?1,2,201,201,201,0)"
            ),
            [usn],
        )?;
        // These seven row-1 records are present in a fresh complete login.
        // The production capture gives every one of them level 1 and keeps
        // the independently claimed bonus level at 0.
        for kind in [
            "character",
            "costume",
            "guitar",
            "music",
            "prop",
            "skill",
            "unit",
        ] {
            tx.execute(
                concat!(
                    "INSERT INTO owned_content(usn,kind,area,content_id,acquired_at,source) ",
                    "VALUES(?1,?2,1,1,?3,'initial')"
                ),
                params![usn, kind, now],
            )?;
            tx.execute(
                concat!(
                    "INSERT INTO content_levels(",
                    "usn,kind,content_id,level,reward_level",
                    ") VALUES(?1,?2,1,1,?3)"
                ),
                params![usn, kind, 0_i64],
            )?;
        }
        for kind in CurrencyKind::ALL {
            tx.execute(
                "INSERT INTO currencies(usn,kind,amount) VALUES(?1,?2,0)",
                params![usn, kind.key()],
            )?;
        }
        for achievement_id in 1..=10 {
            tx.execute(
                concat!(
                    "INSERT INTO achievement_progress(",
                    "usn,achievement_id,quantity,quantity_text,updated_at",
                    ") VALUES(?1,?2,0,'0',?3)"
                ),
                params![usn, achievement_id, now],
            )?;
        }
        for mission_id in 1..=6 {
            tx.execute(
                "INSERT INTO daily_missions(usn,mission_id,quantity,updated_at) VALUES(?1,?2,1,?3)",
                params![usn, mission_id, now],
            )?;
        }
        for ad_level_id in [200_010, 210_010, 220_010] {
            tx.execute(
                "INSERT INTO ad_levels(usn,ad_level_id,level,experience) VALUES(?1,?2,1,0)",
                params![usn, ad_level_id],
            )?;
        }
        for season in 1..=13 {
            tx.execute(
                "INSERT INTO pass_entitlements(usn,season,premium,lounge) VALUES(?1,?2,1,0)",
                params![usn, season],
            )?;
        }
        tx.execute(
            "INSERT INTO affection(usn,follower_id,experience,level) VALUES(?1,100000,0,1)",
            [usn],
        )?;
        tx.execute(
            "INSERT INTO ch3_energy(usn,amount,cap,calculated_at) VALUES(?1,200,200,?2)",
            params![usn, now],
        )?;
        tx.execute(
            concat!(
                "INSERT INTO ch3_stages(usn,stage_id,chapter,stage_index,completed,best_star,best_score,unlocked) ",
                "VALUES(?1,1001,1,1,0,0,0,1)"
            ),
            [usn],
        )?;
        tx.execute(
            "UPDATE runtime_selection SET active_usn=COALESCE(active_usn,?1)",
            [usn],
        )?;
        Ok(UserIdentity { usn, user_id })
    }

    pub fn ensure_default_slot(&mut self, now: i64) -> Result<UserIdentity, StoreError> {
        if let Some(identity) = self
            .connection
            .query_row(
                "SELECT usn,user_id FROM save_slots ORDER BY usn LIMIT 1",
                [],
                |row| {
                    Ok(UserIdentity {
                        usn: row.get(0)?,
                        user_id: row.get(1)?,
                    })
                },
            )
            .optional()?
        {
            Ok(identity)
        } else {
            self.create_slot("Save 1", now)
        }
    }

    pub fn list_slots(&self) -> Result<Vec<UserIdentity>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT usn,user_id FROM save_slots ORDER BY usn")?;
        Ok(statement
            .query_map([], |row| {
                Ok(UserIdentity {
                    usn: row.get(0)?,
                    user_id: row.get(1)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn slot_views(&self) -> Result<Vec<SlotView>, StoreError> {
        let (active, pending): (Option<i64>, Option<i64>) = self.connection.query_row(
            "SELECT active_usn,pending_usn FROM runtime_selection WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let mut statement = self.connection.prepare(
            "SELECT usn,user_id,display_name,created_at,updated_at FROM save_slots ORDER BY usn",
        )?;
        Ok(statement
            .query_map([], |row| {
                let usn = row.get(0)?;
                Ok(SlotView {
                    usn,
                    user_id: row.get(1)?,
                    display_name: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    active: active == Some(usn),
                    pending: pending == Some(usn),
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn request_switch(&mut self, usn: Usn) -> Result<(), StoreError> {
        let exists = self
            .connection
            .query_row("SELECT 1 FROM save_slots WHERE usn=?1", [usn], |_| Ok(()))
            .optional()?
            .is_some();
        if !exists {
            return Err(StoreError::MissingSlot(usn));
        }
        self.connection.execute(
            "UPDATE runtime_selection SET pending_usn=?1 WHERE singleton=1",
            [usn],
        )?;
        Ok(())
    }

    pub fn delete_inactive_slot(
        &mut self,
        usn: Usn,
        typed_display_name: &str,
    ) -> Result<(), StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::delete_inactive_slot_in_transaction(&tx, usn, typed_display_name)?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_inactive_slot_rpc(
        &mut self,
        mutation: &RpcMutation,
        usn: Usn,
        typed_display_name: &str,
    ) -> Result<(), StoreError> {
        let typed_display_name = typed_display_name.to_owned();
        self.commit_rpc_mutation(mutation, |tx| {
            Self::delete_inactive_slot_in_transaction(tx, usn, &typed_display_name)
        })
    }

    fn delete_inactive_slot_in_transaction(
        tx: &rusqlite::Transaction<'_>,
        usn: Usn,
        typed_display_name: &str,
    ) -> Result<(), StoreError> {
        let display_name: String = tx
            .query_row(
                "SELECT display_name FROM save_slots WHERE usn=?1",
                [usn],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StoreError::MissingSlot(usn))?;
        if display_name != typed_display_name {
            return Err(DomainError::IdentityMismatch.into());
        }
        let selected: bool = tx.query_row(
            concat!(
                "SELECT COALESCE(active_usn=?1,0) OR COALESCE(pending_usn=?1,0) ",
                "FROM runtime_selection WHERE singleton=1"
            ),
            [usn],
            |row| row.get(0),
        )?;
        if selected {
            return Err(StoreError::ActiveSlot(usn));
        }
        tx.execute("DELETE FROM save_slots WHERE usn=?1", [usn])?;
        Ok(())
    }

    pub fn resolve_slot(&self, selector: &str) -> Result<UserIdentity, StoreError> {
        let numeric = selector.parse::<i64>().ok();
        self.connection
            .query_row(
                concat!(
                    "SELECT usn,user_id FROM save_slots WHERE user_id=?1 OR usn=?2 ",
                    "ORDER BY usn LIMIT 1"
                ),
                params![selector, numeric],
                |row| {
                    Ok(UserIdentity {
                        usn: row.get(0)?,
                        user_id: row.get(1)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::MissingSlot(numeric.unwrap_or_default()))
    }

    pub fn request_switch_rpc(
        &mut self,
        mutation: &RpcMutation,
        selector: &str,
    ) -> Result<UserIdentity, StoreError> {
        let selector = selector.to_owned();
        self.commit_rpc_mutation(mutation, |tx| {
            let numeric = selector.parse::<i64>().ok();
            let identity = tx
                .query_row(
                    "SELECT usn,user_id FROM save_slots WHERE user_id=?1 OR usn=?2 ORDER BY usn LIMIT 1",
                    params![selector, numeric],
                    |row| {
                        Ok(UserIdentity {
                            usn: row.get(0)?,
                            user_id: row.get(1)?,
                        })
                    },
                )
                .optional()?
                .ok_or_else(|| StoreError::MissingSlot(numeric.unwrap_or_default()))?;
            tx.execute(
                "UPDATE runtime_selection SET pending_usn=?1 WHERE singleton=1",
                [identity.usn],
            )?;
            Ok(identity)
        })
    }

    pub fn begin_login(
        &mut self,
        capability_hash: &[u8],
        clock: DeviceClock,
    ) -> Result<LoginSession, StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE runtime_selection SET active_usn=COALESCE(pending_usn,active_usn),pending_usn=NULL,session_nonce=session_nonce+1 WHERE singleton=1", [])?;
        let (usn, nonce): (i64, i64) = tx
            .query_row(
                "SELECT active_usn,session_nonce FROM runtime_selection WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| StoreError::NoActiveSlot)?;
        let user_id: String = tx.query_row(
            "SELECT user_id FROM save_slots WHERE usn=?1",
            [usn],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE login_sessions SET ended_at=?1 WHERE ended_at IS NULL",
            [clock.unix_seconds],
        )?;
        tx.execute("INSERT INTO login_sessions(nonce,usn,capability_hash,started_at,device_time,utc_offset_minutes) VALUES(?1,?2,?3,?4,?4,?5)", params![nonce,usn,capability_hash,clock.unix_seconds,clock.utc_offset_minutes])?;
        let local_day = clock.local_epoch_day();
        let season = local_day.rem_euclid(13) + 1;
        tx.execute(
            concat!(
                "INSERT OR IGNORE INTO pass_anchors(usn,local_day,season,utc_offset_minutes,updated_at) ",
                "VALUES(?1,?2,?3,?4,?5)"
            ),
            params![usn, local_day, season, clock.utc_offset_minutes, clock.unix_seconds],
        )?;
        tx.commit()?;
        Ok(LoginSession {
            identity: UserIdentity { usn, user_id },
            nonce,
            capability: String::new(),
            clock,
        })
    }

    /// Execute a gameplay command in one immediate transaction. Sequence
    /// authorization, domain writes and the idempotency ledger either all
    /// commit or all roll back. A retried idempotency key replays the exact
    /// serialized result without applying the command again.
    pub fn commit_rpc_mutation<T, F>(
        &mut self,
        mutation: &RpcMutation,
        command: F,
    ) -> Result<T, StoreError>
    where
        T: Clone + Serialize + DeserializeOwned,
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, StoreError>,
    {
        self.commit_rpc_mutation_with_replay(mutation, |transaction| {
            let result = command(transaction)?;
            Ok((result.clone(), None))
        })
    }

    /// Variant for commands whose first response contains consumable rewards.
    /// The ledger stores `replay` instead of the first response, preventing a
    /// network retry from feeding the same reward list to the client twice.
    pub fn commit_rpc_mutation_with_replay<T, F>(
        &mut self,
        mutation: &RpcMutation,
        command: F,
    ) -> Result<T, StoreError>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<(T, Option<T>), StoreError>,
    {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active_session: bool = tx.query_row(
            concat!(
                "SELECT EXISTS(SELECT 1 FROM login_sessions l JOIN save_slots s ON s.usn=l.usn ",
                "WHERE l.nonce=?1 AND l.usn=?2 AND l.ended_at IS NULL AND s.user_id=?3)"
            ),
            params![
                mutation.nonce,
                mutation.identity.usn,
                mutation.identity.user_id
            ],
            |row| row.get(0),
        )?;
        if !active_session {
            return Err(StoreError::Domain(DomainError::StaleRequest));
        }
        if let Some((request_seq, rpc, serialized)) = tx
            .query_row(
                concat!(
                    "SELECT request_seq,rpc,summary_json FROM mutation_ledger ",
                    "WHERE usn=?1 AND idempotency_key=?2"
                ),
                params![mutation.identity.usn, mutation.idempotency_key],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
        {
            if request_seq != mutation.request_seq || rpc != mutation.rpc {
                return Err(StoreError::IdempotencyConflict);
            }
            return serde_json::from_str(&serialized)
                .map_err(|error| StoreError::InvalidReplay(error.to_string()));
        }
        let changed = tx.execute(
            concat!(
                "UPDATE login_sessions SET last_request_seq=?1 ",
                "WHERE nonce=?2 AND usn=?3 AND ended_at IS NULL AND last_request_seq<?1 ",
                "AND EXISTS(SELECT 1 FROM save_slots WHERE usn=?3 AND user_id=?4)"
            ),
            params![
                mutation.request_seq,
                mutation.nonce,
                mutation.identity.usn,
                mutation.identity.user_id
            ],
        )?;
        if changed == 0 {
            return Err(StoreError::Domain(DomainError::StaleRequest));
        }
        let (result, replay) = command(&tx)?;
        let serialized = serde_json::to_string(replay.as_ref().unwrap_or(&result))
            .map_err(|error| StoreError::InvalidReplay(error.to_string()))?;
        let ledger_id: i64 = tx.query_row(
            "SELECT COALESCE(MAX(ledger_id),0)+1 FROM mutation_ledger WHERE usn=?1",
            [mutation.identity.usn],
            |row| row.get(0),
        )?;
        tx.execute(
            concat!(
                "INSERT INTO mutation_ledger(usn,ledger_id,request_seq,rpc,idempotency_key,committed_at,summary_json) ",
                "VALUES(?1,?2,?3,?4,?5,?6,?7)"
            ),
            params![
                mutation.identity.usn,
                ledger_id,
                mutation.request_seq,
                mutation.rpc,
                mutation.idempotency_key,
                mutation.committed_at,
                serialized
            ],
        )?;
        tx.execute(
            "UPDATE save_slots SET updated_at=?1 WHERE usn=?2",
            params![mutation.committed_at, mutation.identity.usn],
        )?;
        tx.commit()?;
        Ok(result)
    }

    pub fn end_session(&mut self, nonce: i64, now: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE login_sessions SET ended_at=COALESCE(ended_at,?1) WHERE nonce=?2",
            params![now, nonce],
        )?;
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(FULL)")?;
        Ok(())
    }

    pub fn currency(&self, usn: Usn, kind: CurrencyKind) -> Result<f64, StoreError> {
        Ok(self.connection.query_row(
            "SELECT amount FROM currencies WHERE usn=?1 AND kind=?2",
            params![usn, kind.key()],
            |row| row.get(0),
        )?)
    }

    pub fn ad_level(&self, usn: Usn, group_id: i64) -> Result<AdLevelSnapshot, StoreError> {
        Ok(self.connection.query_row(
            "SELECT ad_level_id,level,experience FROM ad_levels WHERE usn=?1 AND ad_level_id=?2",
            params![usn, group_id],
            |row| {
                Ok(AdLevelSnapshot {
                    ad_level_id: row.get(0)?,
                    level: row.get(1)?,
                    experience: row.get(2)?,
                })
            },
        )?)
    }

    pub fn player_snapshot(&self, usn: Usn) -> Result<PlayerSnapshot, StoreError> {
        let (user_id, nickname, fan_level, avatar_id, title_id, last_save_time, device_uuid) =
            self.connection.query_row(
                concat!(
                    "SELECT s.user_id,u.nickname,u.fan_level,u.avatar_id,u.title_id,",
                    "u.last_save_time,u.device_uuid FROM save_slots s ",
                    "JOIN user_core u ON u.usn=s.usn WHERE s.usn=?1"
                ),
                [usn],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )?;
        let mut currency_statement = self
            .connection
            .prepare("SELECT kind,amount FROM currencies WHERE usn=?1 ORDER BY kind")?;
        let currencies = currency_statement
            .query_map([usn], |row| {
                let key: String = row.get(0)?;
                let kind = currency_from_key(&key).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        format!("unknown currency {key}").into(),
                    )
                })?;
                Ok((kind, row.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut area_statement = self.connection.prepare(concat!(
            "SELECT area,area_level,fans,candy,current_costume,current_guitar,current_music,gp,unlocked ",
            "FROM area_state WHERE usn=?1 ORDER BY area"
        ))?;
        let areas = area_statement
            .query_map([usn], |row| {
                Ok(AreaSnapshot {
                    area: row.get(0)?,
                    area_level: row.get(1)?,
                    fans: row.get(2)?,
                    candy: row.get(3)?,
                    current_costume: row.get(4)?,
                    current_guitar: row.get(5)?,
                    current_music: row.get(6)?,
                    gp: row.get(7)?,
                    unlocked: row.get::<_, i64>(8)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?;
        let mut owned_statement = self.connection.prepare(
            "SELECT kind,area,content_id FROM owned_content WHERE usn=?1 ORDER BY kind,area,content_id",
        )?;
        let owned_content = owned_statement
            .query_map([usn], |row| {
                let key: String = row.get(0)?;
                let kind = content_from_key(&key).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        format!("unknown content kind {key}").into(),
                    )
                })?;
                Ok(OwnedContentSnapshot {
                    kind,
                    area: row.get(1)?,
                    content_id: row.get(2)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        let mut level_statement = self.connection.prepare(concat!(
            "SELECT kind,content_id,level,reward_level FROM content_levels ",
            "WHERE usn=?1 ORDER BY kind,content_id"
        ))?;
        let content_levels = level_statement
            .query_map([usn], |row| {
                let key: String = row.get(0)?;
                let kind = content_from_key(&key).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        format!("unknown content kind {key}").into(),
                    )
                })?;
                Ok(ContentLevelSnapshot {
                    kind,
                    content_id: row.get(1)?,
                    level: row.get(2)?,
                    reward_level: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        let mut tutorial_statement = self.connection.prepare(
            "SELECT tutorial_id FROM tutorial_completed WHERE usn=?1 ORDER BY tutorial_id",
        )?;
        let tutorials = tutorial_statement
            .query_map([usn], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let mut skill_statement = self.connection.prepare(concat!(
            "SELECT skill_id,active,active_from,active_until,reset_ready_at FROM skill_activation ",
            "WHERE usn=?1 ORDER BY skill_id"
        ))?;
        let skill_activations = skill_statement
            .query_map([usn], |row| {
                Ok(SkillActivationSnapshot {
                    skill_id: row.get(0)?,
                    active: row.get::<_, i64>(1)? != 0,
                    active_from: row.get(2)?,
                    active_until: row.get(3)?,
                    reset_ready_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut ad_level_statement = self.connection.prepare(
            "SELECT ad_level_id,level,experience FROM ad_levels WHERE usn=?1 ORDER BY ad_level_id",
        )?;
        let ad_levels = ad_level_statement
            .query_map([usn], |row| {
                Ok(AdLevelSnapshot {
                    ad_level_id: row.get(0)?,
                    level: row.get(1)?,
                    experience: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut ad_state_statement = self.connection.prepare(concat!(
            "SELECT ad_id,day_count,total_count,last_view_time FROM ad_state ",
            "WHERE usn=?1 ORDER BY ad_id"
        ))?;
        let ad_state = ad_state_statement
            .query_map([usn], |row| {
                Ok(AdStateSnapshot {
                    ad_id: row.get(0)?,
                    day_count: row.get(1)?,
                    total_count: row.get(2)?,
                    last_view_time: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut messenger_statement = self.connection.prepare(concat!(
            "SELECT room_id,last_confirm_index,unlock_group_list,update_time_ticks FROM messenger_rooms ",
            "WHERE usn=?1 ORDER BY room_id"
        ))?;
        let messengers = messenger_statement
            .query_map([usn], |row| {
                Ok(MessengerSnapshot {
                    room_id: row.get(0)?,
                    last_confirm_index: row.get(1)?,
                    unlock_group_list: row.get(2)?,
                    update_time_ticks: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let gifts = self.connection.prepare(
            "SELECT gift_id,quantity FROM gifts WHERE usn=?1 ORDER BY gift_id"
        )?.query_map([usn], |row| Ok(ggfm_domain::FollowerGiftSnapshot {
            gift_id: row.get(0)?, quantity: row.get(1)?,
        }))?.collect::<Result<Vec<_>, _>>()?;
        let affection_claims = self.connection.prepare(
            "SELECT follower_id,reward_level FROM affection_claims WHERE usn=?1 ORDER BY follower_id,reward_level"
        )?.query_map([usn], |row| Ok(ggfm_domain::FollowerAffectionClaimSnapshot {
            profile_id: row.get(0)?, level: row.get(1)?,
        }))?.collect::<Result<Vec<_>, _>>()?;
        let buff_timers = self.connection.prepare(
            "SELECT buff_id,active_at,expires_at FROM buff_timers WHERE usn=?1 ORDER BY buff_id"
        )?.query_map([usn], |row| Ok(ggfm_domain::BuffTimerSnapshot {
            buff_id: row.get(0)?, active_at: row.get(1)?, expires_at: row.get(2)?,
        }))?.collect::<Result<Vec<_>, _>>()?;
        let complete_id = completed_guide_stage(&self.connection, usn)?;
        let mut quest_rows: BTreeMap<i64, FollowerQuestSnapshot> = BTreeMap::new();
        let mut quest_statement = self.connection.prepare(concat!(
            "SELECT q.stage_id,q.completed,q.infinite,c.condition_id,c.quantity,c.claimed FROM follower_quests q ",
            "LEFT JOIN quest_conditions c ON c.usn=q.usn AND c.chain_id=q.chain_id AND c.stage_id=q.stage_id ",
            "WHERE q.usn=?1 AND q.chain_id=1 ORDER BY q.stage_id,c.condition_id"
        ))?;
        let rows = quest_statement.query_map([usn], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)? != 0,
                row.get::<_, i64>(2)? != 0,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0,
            ))
        })?;
        for row in rows {
            let (current_id, completed, infinite, condition, quantity, claimed) = row?;
            let entry = quest_rows
                .entry(current_id)
                .or_insert(FollowerQuestSnapshot {
                    current_id,
                    complete_id,
                    completed,
                    infinite,
                    condition_values: [0.0; 3],
                    claimed: [false; 3],
                });
            if let Some(index) = condition.and_then(|value| usize::try_from(value - 1).ok())
                && index < 3
            {
                entry.condition_values[index] = quantity.unwrap_or(0.0);
                entry.claimed[index] = claimed;
            }
        }
        let follower_quests = quest_rows.into_values().collect();
        let mut achievement_statement = self.connection.prepare(concat!(
            "SELECT achievement_id,quantity,quantity_text,(SELECT COUNT(*)+1 FROM achievement_claims c ",
            "WHERE c.usn=achievement_progress.usn AND c.achievement_id=achievement_progress.achievement_id) ",
            "FROM achievement_progress ",
            "WHERE usn=?1 ORDER BY achievement_id"
        ))?;
        let achievements = achievement_statement
            .query_map([usn], |row| {
                Ok(ProgressSnapshot {
                    id: row.get(0)?,
                    level: row.get(3)?,
                    quantity: row.get(1)?,
                    quantity_text: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut daily_statement = self.connection.prepare(concat!(
            "SELECT mission_id,quantity,(SELECT COUNT(*)+1 FROM daily_mission_claims c ",
            "WHERE c.usn=daily_missions.usn AND c.mission_id=daily_missions.mission_id) ",
            "FROM daily_missions WHERE usn=?1 ORDER BY mission_id"
        ))?;
        let daily_missions = daily_statement
            .query_map([usn], |row| {
                let quantity: f64 = row.get(1)?;
                Ok(ProgressSnapshot {
                    id: row.get(0)?,
                    level: row.get(2)?,
                    quantity,
                    quantity_text: quantity.to_string(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let pass = self.pass_snapshot(usn)?;
        let ch3 = self.ch3_snapshot_raw(usn)?;
        let music_reviews = self.music_reviews(usn)?;
        let bookmarks = self.bookmarks(usn)?;
        let music_scores = self.music_scores(usn)?;
        let mut music_state_statement = self.connection.prepare(concat!(
            "SELECT music_id,encore_appears,encore_ready_at,encore_follower_id,ch3_active_until ",
            "FROM music_state WHERE usn=?1 ORDER BY music_id"
        ))?;
        let music_states = music_state_statement
            .query_map([usn], |row| {
                Ok(MusicStateSnapshot {
                    music_id: row.get(0)?,
                    encore_appears: row.get::<_, i64>(1)? != 0,
                    encore_ready_at: row.get(2)?,
                    encore_follower_id: row.get(3)?,
                    ch3_active_until: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut selected_reward_statement = self.connection.prepare(concat!(
            "SELECT group_id,reward_group_id,alternate_reward_group_id ",
            "FROM selected_rewards WHERE usn=?1 ORDER BY group_id"
        ))?;
        let selected_rewards = selected_reward_statement
            .query_map([usn], |row| {
                Ok(SelectRewardSnapshot {
                    group_id: row.get(0)?,
                    reward_group_id: row.get(1)?,
                    alternate_reward_group_id: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PlayerSnapshot {
            identity: UserIdentity { usn, user_id },
            nickname,
            fan_level,
            avatar_id,
            title_id,
            last_save_time,
            samseck_step: self.sam_seck_step(usn)?,
            device_uuid,
            currencies,
            areas,
            owned_content,
            content_levels,
            tutorials,
            pass,
            skill_activations,
            buff_timers,
            gifts,
            affection_claims,
            ad_levels,
            ad_state,
            messengers,
            follower_quests,
            achievements,
            daily_missions,
            ch3,
            music_reviews,
            bookmarks,
            music_scores,
            music_states,
            selected_rewards,
        })
    }

    pub fn give_follower_gift(
        &mut self,
        mutation: &RpcMutation,
        profile_id: i64,
        gift_id: i64,
        quantity: i64,
        experience_each: i64,
        level_thresholds: Vec<(i32, i64, i32)>,
    ) -> Result<FollowerGiftOutcome, StoreError> {
        if quantity <= 0 {
            return Err(DomainError::InvalidAmount.into());
        }
        self.commit_rpc_mutation(mutation, |tx| {
            let remaining: i64 = tx
                .query_row(
                    "SELECT quantity FROM gifts WHERE usn=?1 AND gift_id=?2",
                    params![mutation.identity.usn, gift_id],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or(0);
            if remaining < quantity {
                return Err(DomainError::InsufficientBalance.into());
            }
            let remaining = remaining - quantity;
            tx.execute(
                "UPDATE gifts SET quantity=?1 WHERE usn=?2 AND gift_id=?3",
                params![remaining, mutation.identity.usn, gift_id],
            )?;
            let profile = update_follower_profile(
                tx,
                mutation.identity.usn,
                profile_id,
                experience_each.saturating_mul(quantity),
                &level_thresholds,
            )?;
            Ok(FollowerGiftOutcome {
                gift_id,
                remaining,
                profile,
            })
        })
    }

    pub fn select_reward(
        &mut self,
        mutation: &RpcMutation,
        selection_id: i64,
        group_id: i32,
        reward_group_id: i32,
        alternate_reward_group_id: i32,
    ) -> Result<Vec<SelectRewardSnapshot>, StoreError> {
        self.commit_rpc_mutation_with_replay(mutation, |tx| {
            tx.execute(
                concat!(
                    "INSERT INTO selected_rewards(usn,group_id,selection_id,reward_group_id,alternate_reward_group_id,selected_at) ",
                    "VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(usn,group_id) DO UPDATE SET ",
                    "selection_id=excluded.selection_id,reward_group_id=excluded.reward_group_id,",
                    "alternate_reward_group_id=excluded.alternate_reward_group_id,selected_at=excluded.selected_at"
                ),
                params![
                    mutation.identity.usn,
                    group_id,
                    selection_id,
                    reward_group_id,
                    alternate_reward_group_id,
                    mutation.committed_at
                ],
            )?;
            let mut statement = tx.prepare(concat!(
                "SELECT group_id,reward_group_id,alternate_reward_group_id FROM selected_rewards ",
                "WHERE usn=?1 ORDER BY group_id"
            ))?;
            let result = statement
                .query_map([mutation.identity.usn], |row| {
                    Ok(SelectRewardSnapshot {
                        group_id: row.get(0)?,
                        reward_group_id: row.get(1)?,
                        alternate_reward_group_id: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((result.clone(), Some(result)))
        })
    }

    pub fn claim_music_encore_rewards(
        &mut self,
        mutation: &RpcMutation,
        rewards: Vec<(i64, i32, i64, i64)>,
        profile_level_thresholds: Vec<(i64, i32, i64, i32)>,
    ) -> Result<MusicEncoreRewardOutcome, StoreError> {
        self.commit_rpc_mutation_with_replay(mutation, |tx| {
            let usn = mutation.identity.usn;
            let mut granted = Vec::new();
            let mut rejected = Vec::new();
            let mut profiles = BTreeMap::new();
            let mut total_candy = 0_i64;
            for (music_id, gift_amount, profile_exp, ready_at) in rewards {
                let owned: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM owned_content WHERE usn=?1 AND kind='music' AND content_id=?2)",
                    params![usn,music_id],
                    |row| row.get(0),
                )?;
                let state: Option<(bool, i64, Option<i64>)> = tx
                    .query_row(
                        "SELECT encore_appears,encore_ready_at,encore_follower_id FROM music_state WHERE usn=?1 AND music_id=?2",
                        params![usn,music_id],
                        |row| Ok((row.get::<_,i64>(0)? != 0,row.get(1)?,row.get(2)?)),
                    )
                    .optional()?;
                if !owned
                    || state
                        .as_ref()
                        .is_some_and(|(appears, current_ready_at, _)| {
                            !*appears && *current_ready_at > mutation.committed_at
                        })
                {
                    rejected.push(to_i32_saturated(music_id));
                    continue;
                }
                let follower_id = state.and_then(|(_, _, follower_id)| follower_id);
                tx.execute(
                    concat!(
                        "INSERT INTO music_state(usn,music_id,encore_appears,encore_follower_id,encore_ready_at,ch3_active_until) ",
                        "VALUES(?1,?2,0,?3,?4,0) ON CONFLICT(usn,music_id) DO UPDATE SET ",
                        "encore_appears=0,encore_ready_at=?4"
                    ),
                    params![usn,music_id,follower_id,ready_at],
                )?;
                total_candy = total_candy.saturating_add(i64::from(gift_amount.max(0)));
                granted.push((to_i32_saturated(music_id), gift_amount.max(0)));
                if let Some(profile_id) = follower_id.filter(|id| *id > 0) {
                    let thresholds = profile_level_thresholds
                        .iter()
                        .filter(|(candidate, _, _, _)| *candidate == profile_id)
                        .map(|(_, level, requirement, add_candy)| {
                            (*level, *requirement, *add_candy)
                        })
                        .collect::<Vec<_>>();
                    let profile =
                        update_follower_profile(tx, usn, profile_id, profile_exp, &thresholds)?;
                    profiles.insert(profile_id, profile);
                }
            }
            if total_candy > 0 {
                tx.execute(
                    "UPDATE currencies SET amount=amount+?1 WHERE usn=?2 AND kind='candy'",
                    params![total_candy as f64,usn],
                )?;
            }
            let result = MusicEncoreRewardOutcome {
                rewards: granted,
                profiles: profiles.into_values().collect(),
                rejected_music_ids: rejected,
            };
            Ok((result.clone(), Some(result)))
        })
    }

    pub fn claim_follower_profile_rewards(
        &mut self,
        mutation: &RpcMutation,
        profile_id: i64,
        requested: Vec<(i32, Vec<RewardGrant>)>,
    ) -> Result<Vec<FollowerProfileRewardOutcome>, StoreError> {
        self.commit_reward_mutation(mutation, |tx, rules| {
            let (experience, profile_level): (i64, i32) = tx.query_row(
                "SELECT experience,level FROM affection WHERE usn=?1 AND follower_id=?2",
                params![mutation.identity.usn,profile_id], |row| Ok((row.get(0)?,row.get(1)?)),
            )?;
            let profile = FollowerProfileSnapshot { profile_id, level: profile_level, experience, add_candy: 0 };
            let mut outcomes = Vec::new();
            for (level, rewards) in requested {
                if level <= 0 || level > profile_level { return Err(DomainError::Locked.into()); }
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO affection_claims(usn,follower_id,reward_level,claimed_at) VALUES(?1,?2,?3,?4)",
                    params![mutation.identity.usn,profile_id,level,mutation.committed_at],
                )?;
                if inserted == 0 { continue; }
                let mut applied = Vec::new();
                for reward in rewards {
                    if apply_reward_grant(tx, rules, mutation.identity.usn, &reward, mutation.committed_at, "follower-profile")? { applied.push(reward); }
                }
                outcomes.push(FollowerProfileRewardOutcome { profile_id, level, rewards: applied, profile: profile.clone() });
            }
            Ok((outcomes, Some(Vec::new())))
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn advance_follower_quest(
        &mut self,
        mutation: &RpcMutation,
        current_id: i64,
        sub_id: i64,
        condition_value: f64,
        thresholds: [f64; 3],
        rewards: Vec<RewardGrant>,
        next_id: Option<i64>,
    ) -> Result<FollowerQuestOutcome, StoreError> {
        if !(1..=3).contains(&sub_id) || !condition_value.is_finite() || condition_value < 0.0 {
            return Err(DomainError::InvalidAmount.into());
        }
        self.commit_reward_mutation(mutation, |tx, rules| {
            let usn = mutation.identity.usn;
            let authoritative: i64 = tx.query_row(
                "SELECT COALESCE(MAX(stage_id),1) FROM follower_quests WHERE usn=?1 AND chain_id=1",
                [usn], |row| row.get(0),
            )?;
            let active_id = authoritative.max(1);
            tx.execute(
                "INSERT OR IGNORE INTO follower_quests(usn,chain_id,stage_id,completed,updated_at) VALUES(?1,1,?2,0,?3)",
                params![usn,active_id,mutation.committed_at],
            )?;
            if current_id != active_id {
                let snapshot = quest_snapshot(tx, usn, active_id)?;
                return Ok((FollowerQuestOutcome { complete_id: snapshot.complete_id, quest: snapshot, next: false, rewards: Vec::new() }, None));
            }
            tx.execute(concat!(
                "INSERT INTO quest_conditions(usn,chain_id,stage_id,condition_id,quantity,claimed) VALUES(?1,1,?2,?3,?4,0) ",
                "ON CONFLICT(usn,chain_id,stage_id,condition_id) DO UPDATE SET quantity=MAX(quantity,excluded.quantity)"
            ), params![usn,current_id,sub_id,condition_value])?;
            let index = (sub_id - 1) as usize;
            let quantity: f64 = tx.query_row(
                "SELECT quantity FROM quest_conditions WHERE usn=?1 AND chain_id=1 AND stage_id=?2 AND condition_id=?3",
                params![usn,current_id,sub_id], |row| row.get(0),
            )?;
            let mut applied = Vec::new();
            if quantity >= thresholds[index] {
                let changed = tx.execute(
                    "UPDATE quest_conditions SET claimed=1 WHERE usn=?1 AND chain_id=1 AND stage_id=?2 AND condition_id=?3 AND claimed=0",
                    params![usn,current_id,sub_id],
                )?;
                if changed != 0 {
                    for reward in rewards {
                        if apply_reward_grant(tx, rules, usn, &reward, mutation.committed_at, "follower-quest")? { applied.push(reward); }
                    }
                }
            }
            let claimed: i64 = tx.query_row(
                "SELECT COUNT(*) FROM quest_conditions WHERE usn=?1 AND chain_id=1 AND stage_id=?2 AND claimed=1",
                params![usn,current_id], |row| row.get(0),
            )?;
            let next = claimed == 3;
            let mut result_id = current_id;
            if next {
                tx.execute("UPDATE follower_quests SET completed=1,updated_at=?1 WHERE usn=?2 AND chain_id=1 AND stage_id=?3", params![mutation.committed_at,usn,current_id])?;
                if let Some(next_id) = next_id {
                    tx.execute("INSERT OR IGNORE INTO follower_quests(usn,chain_id,stage_id,completed,updated_at) VALUES(?1,1,?2,0,?3)", params![usn,next_id,mutation.committed_at])?;
                    result_id = next_id;
                }
            }
            let snapshot = quest_snapshot(tx, usn, result_id)?;
            let result = FollowerQuestOutcome { complete_id: snapshot.complete_id, quest: snapshot, next, rewards: applied };
            let mut replay = result.clone(); replay.rewards.clear(); replay.next = false;
            Ok((result, Some(replay)))
        })
    }

    pub fn set_one_time_flag(
        &mut self,
        mutation: &RpcMutation,
        flag: String,
        value: i64,
    ) -> Result<i64, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            tx.execute(concat!(
                "INSERT INTO one_time_flags(usn,flag,value,updated_at) VALUES(?1,?2,?3,?4) ",
                "ON CONFLICT(usn,flag) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at"
            ), params![mutation.identity.usn,flag,value,mutation.committed_at])?;
            Ok(value)
        })
    }

    pub fn grant_ad_reward(
        &mut self,
        mutation: &RpcMutation,
        ad_id: i64,
        local_day: i64,
        rewards: Vec<RewardGrant>,
        profile: Option<AdProfileGrant>,
    ) -> Result<AdRewardOutcome, StoreError> {
        self.commit_reward_mutation(mutation, |tx, rules| {
            let usn = mutation.identity.usn;
            tx.execute(concat!(
                "INSERT INTO ad_state(usn,ad_id,local_day,day_count,total_count,last_view_time) VALUES(?1,?2,?3,1,1,?4) ",
                "ON CONFLICT(usn,ad_id) DO UPDATE SET day_count=CASE WHEN local_day=excluded.local_day THEN day_count+1 ELSE 1 END,",
                "total_count=total_count+1,local_day=excluded.local_day,last_view_time=excluded.last_view_time"
            ), params![usn,ad_id,local_day,mutation.committed_at])?;
            let (day_count,total_count,last_view_time):(i32,i32,i64)=tx.query_row(
                "SELECT day_count,total_count,last_view_time FROM ad_state WHERE usn=?1 AND ad_id=?2",
                params![usn,ad_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            )?;
            let mut applied=Vec::new();
            for reward in rewards { if apply_reward_grant(tx, rules,usn,&reward,mutation.committed_at,"ad")? { applied.push(reward); } }
            let profile = if let Some(profile) = profile {
                Some(update_follower_profile(
                    tx,
                    usn,
                    profile.profile_id,
                    profile.experience,
                    &profile.level_thresholds,
                )?)
            } else { None };
            let result=AdRewardOutcome { ad_id,day_count,total_count,last_view_time,rewards:applied,profile };
            let mut replay=result.clone(); replay.rewards.clear();
            Ok((result,Some(replay)))
        })
    }

    pub fn sam_seck_step(&self, usn: Usn) -> Result<i32, StoreError> {
        Ok(self.connection.query_row(
            "SELECT COALESCE(MAX(data_id),0) FROM event_points WHERE usn=?1 AND event_type='samseck' AND points>0",
            [usn], |row| row.get(0),
        )?)
    }

    pub fn claim_sam_seck(
        &mut self,
        mutation: &RpcMutation,
        event_id: i64,
        post_id: i64,
        reward: RewardGrant,
    ) -> Result<i32, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            let usn=mutation.identity.usn;
            let eligible = match event_id {
                1 => tx.query_row("SELECT level FROM content_levels WHERE usn=?1 AND kind='character' AND content_id=1", [usn], |row| row.get::<_,i64>(0)).optional()?.unwrap_or(1) >= 200,
                2 => tx.query_row("SELECT COUNT(*) FROM owned_content WHERE usn=?1 AND kind='music'", [usn], |row| row.get::<_,i64>(0))? >= 3,
                3 => tx.query_row("SELECT COUNT(*) FROM owned_content WHERE usn=?1 AND kind='follower'", [usn], |row| row.get::<_,i64>(0))? >= 1,
                _ => false,
            };
            if !eligible { return Err(DomainError::Locked.into()); }
            let inserted=tx.execute(
                "INSERT OR IGNORE INTO event_points(usn,event_type,data_id,points,step,version) VALUES(?1,'samseck',?2,1,?2,1)",
                params![usn,event_id],
            )?;
            if inserted!=0 {
                queue_reward_mail_tx(tx,usn,post_id,"memorial.samseck.subject","memorial.samseck.body",&reward,mutation.committed_at)?;
            }
            let step=tx.query_row("SELECT COALESCE(MAX(data_id),0) FROM event_points WHERE usn=?1 AND event_type='samseck' AND points>0",[usn],|row|row.get(0))?;
            Ok(step)
        })
    }

    pub fn claimed_event_rewards(&self, usn: Usn) -> Result<Vec<i64>, StoreError> {
        let mut statement=self.connection.prepare("SELECT data_id FROM event_points WHERE usn=?1 AND event_type='daily_reward' AND points>0 ORDER BY data_id")?;
        Ok(statement
            .query_map([usn], |row| row.get(0))?
            .collect::<Result<_, _>>()?)
    }

    pub fn claim_event_reward(
        &mut self,
        mutation: &RpcMutation,
        reward_row_id: i64,
        reward: RewardGrant,
    ) -> Result<(Vec<RewardGrant>, f64, f64, f64, f64), StoreError> {
        self.commit_reward_mutation(mutation,|tx, rules|{
            let usn=mutation.identity.usn;
            let inserted=tx.execute("INSERT OR IGNORE INTO event_points(usn,event_type,data_id,points,step,version) VALUES(?1,'daily_reward',?2,1,1,1)",params![usn,reward_row_id])?;
            if inserted==0{return Err(DomainError::AlreadyClaimed.into());}
            let mut applied=Vec::new();
            if apply_reward_grant(tx, rules,usn,&reward,mutation.committed_at,"daily-reward")?{applied.push(reward);}
            let choco=tx.query_row("SELECT amount FROM currencies WHERE usn=?1 AND kind='chocolate'",[usn],|row|row.get(0))?;
            let candy=tx.query_row("SELECT amount FROM currencies WHERE usn=?1 AND kind='candy'",[usn],|row|row.get(0))?;
            let likes=tx.query_row("SELECT amount FROM currencies WHERE usn=?1 AND kind='ch1_like'",[usn],|row|row.get(0))?;
            let fans=tx.query_row("SELECT amount FROM currencies WHERE usn=?1 AND kind='fans'",[usn],|row|row.get(0))?;
            let result=(applied,choco,candy,likes,fans);
            Ok((result.clone(),Some((Vec::new(),choco,candy,likes,fans))))
        })
    }

    pub fn claim_game_reward(
        &mut self,
        mutation: &RpcMutation,
        command: GameRewardClaimCommand,
    ) -> Result<(i32, Vec<RewardGrant>), StoreError> {
        let GameRewardClaimCommand {
            kind,
            id,
            level,
            quantity,
            threshold,
            local_day,
            reward,
        } = command;
        self.commit_reward_mutation(mutation, |tx, rules| {
            let usn = mutation.identity.usn;
            if !quantity.is_finite() || quantity < 0.0 || !threshold.is_finite() || threshold < 0.0 {
                return Err(DomainError::InvalidAmount.into());
            }
            let (table, id_column) = if kind == "achievement" { ("achievement_progress", "achievement_id") } else if kind == "daily_mission" { ("daily_missions", "mission_id") } else { return Err(StoreError::UnsupportedReward(kind.clone())); };
            let quantity = if kind == "achievement" && id == 10 {
                tx.query_row("SELECT attendance_count FROM attendance_state WHERE usn=?1", [usn], |r| r.get::<_, i64>(0))? as f64
            } else { quantity };
            // Attendance comes only from distinct local login dates. A stale
            // inflated achievement snapshot must not make a later claim valid.
            let merge = if kind == "achievement" && id == 10 {
                "excluded.quantity"
            } else {
                "MAX(quantity,excluded.quantity)"
            };
            let upsert = format!("INSERT INTO {table}(usn,{id_column},quantity,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(usn,{id_column}) DO UPDATE SET quantity={merge},updated_at=excluded.updated_at");
            tx.execute(&upsert, params![usn,id,quantity,mutation.committed_at])?;
            let query = format!("SELECT quantity FROM {table} WHERE usn=?1 AND {id_column}=?2");
            let authoritative: f64 = tx.query_row(&query, params![usn,id], |row| row.get(0))?;
            if authoritative < threshold { return Err(DomainError::Locked.into()); }
            let claims_table = if kind == "achievement" { "achievement_claims" } else { "daily_mission_claims" };
            let count_query = if kind == "achievement" {
                format!("SELECT COUNT(*) FROM {claims_table} WHERE usn=?1 AND achievement_id=?2")
            } else {
                format!("SELECT COUNT(*) FROM {claims_table} WHERE usn=?1 AND mission_id=?2 AND local_day=?3")
            };
            let claimed_count: i32 = if kind == "achievement" {
                tx.query_row(&count_query, params![usn,id], |row| row.get(0))?
            } else {
                tx.query_row(&count_query, params![usn,id,local_day], |row| row.get(0))?
            };
            if level != claimed_count.saturating_add(1) { return Err(DomainError::AlreadyClaimed.into()); }
            let inserted = match kind.as_str() {
                "achievement" => tx.execute(
                    "INSERT OR IGNORE INTO achievement_claims(usn,achievement_id,level,claimed_at) VALUES(?1,?2,?3,?4)",
                    params![usn,id,level,mutation.committed_at],
                )?,
                "daily_mission" => tx.execute(
                    "INSERT OR IGNORE INTO daily_mission_claims(usn,local_day,mission_id,level,claimed_at) VALUES(?1,?2,?3,?4,?5)",
                    params![usn,local_day,id,level,mutation.committed_at],
                )?,
                _ => return Err(StoreError::UnsupportedReward(kind.clone())),
            };
            if inserted == 0 { return Err(DomainError::AlreadyClaimed.into()); }
            let mut rewards = Vec::new();
            if let Some(reward) = reward
                && apply_reward_grant(tx, rules, usn, &reward, mutation.committed_at, "game-reward")?
            {
                rewards.push(reward);
            }
            let result = (level.saturating_add(1), rewards);
            Ok((result, Some((level.saturating_add(1), Vec::new()))))
        })
    }

    pub fn music_reviews(&self, usn: Usn) -> Result<Vec<MusicReviewSnapshot>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT music_id,difficulty,point FROM music_reviews WHERE usn=?1 ORDER BY music_id,difficulty",
        )?;
        Ok(statement
            .query_map([usn], |row| {
                Ok(MusicReviewSnapshot {
                    music_id: row.get(0)?,
                    difficulty: row.get(1)?,
                    point: row.get(2)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn bookmarks(&self, usn: Usn) -> Result<Vec<BookmarkSnapshot>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT music_id,flag FROM music_bookmarks WHERE usn=?1 ORDER BY music_id")?;
        Ok(statement
            .query_map([usn], |row| {
                Ok(BookmarkSnapshot {
                    music_id: row.get(0)?,
                    flag: row.get(1)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn music_scores(&self, usn: Usn) -> Result<Vec<MusicScoreSnapshot>, StoreError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT music_id,difficulty,score,grade,play_count,combo_grade,gp,multiple,achievement,",
            "play_type,played_at,mission_clear,is_credit FROM music_scores WHERE usn=?1 ORDER BY music_id,difficulty"
        ))?;
        Ok(statement
            .query_map([usn], |row| {
                Ok(MusicScoreSnapshot {
                    music_id: row.get(0)?,
                    difficulty: row.get(1)?,
                    score: row.get(2)?,
                    grade: row.get(3)?,
                    play_count: row.get(4)?,
                    combo_grade: row.get(5)?,
                    gp: row.get(6)?,
                    multiple: row.get(7)?,
                    achievement: row.get(8)?,
                    play_type: row.get(9)?,
                    played_at: row.get(10)?,
                    mission_clear: row.get(11)?,
                    is_credit: row.get(12)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn save_music_reviews(
        &mut self,
        mutation: &RpcMutation,
        reviews: Vec<MusicReviewSnapshot>,
    ) -> Result<Vec<MusicReviewSnapshot>, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            for row in &reviews {
                tx.execute(concat!(
                    "INSERT INTO music_reviews(usn,music_id,difficulty,point,updated_at) VALUES(?1,?2,?3,?4,?5) ",
                    "ON CONFLICT(usn,music_id,difficulty) DO UPDATE SET point=excluded.point,updated_at=excluded.updated_at"
                ), params![mutation.identity.usn,row.music_id,row.difficulty,row.point,mutation.committed_at])?;
            }
            Ok(reviews)
        })
    }

    pub fn save_bookmarks(
        &mut self,
        mutation: &RpcMutation,
        bookmarks: Vec<BookmarkSnapshot>,
    ) -> Result<Vec<BookmarkSnapshot>, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            for row in &bookmarks {
                tx.execute(concat!(
                    "INSERT INTO music_bookmarks(usn,music_id,flag,updated_at) VALUES(?1,?2,?3,?4) ",
                    "ON CONFLICT(usn,music_id) DO UPDATE SET flag=excluded.flag,updated_at=excluded.updated_at"
                ), params![mutation.identity.usn,row.music_id,row.flag,mutation.committed_at])?;
            }
            Ok(bookmarks)
        })
    }

    pub fn merge_music_scores(
        &mut self,
        mutation: &RpcMutation,
        scores: Vec<MusicScoreSnapshot>,
    ) -> Result<Vec<MusicScoreSnapshot>, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            for row in &scores {
                tx.execute(concat!(
                    "INSERT INTO music_scores(usn,music_id,difficulty,score,grade,play_count,combo_grade,gp,multiple,achievement,play_type,played_at,mission_clear,is_credit,updated_at) ",
                    "VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15) ON CONFLICT(usn,music_id,difficulty) DO UPDATE SET ",
                    "score=MAX(score,excluded.score),grade=MAX(grade,excluded.grade),play_count=MAX(play_count,excluded.play_count),",
                    "combo_grade=MAX(combo_grade,excluded.combo_grade),gp=MAX(gp,excluded.gp),multiple=excluded.multiple,achievement=MAX(achievement,excluded.achievement),",
                    "play_type=excluded.play_type,played_at=excluded.played_at,mission_clear=MAX(mission_clear,excluded.mission_clear),is_credit=excluded.is_credit,updated_at=excluded.updated_at"
                ), params![mutation.identity.usn,row.music_id,row.difficulty,row.score,row.grade,row.play_count,row.combo_grade,row.gp,row.multiple,row.achievement,row.play_type,row.played_at,row.mission_clear,row.is_credit,mutation.committed_at])?;
            }
            let mut statement = tx.prepare(concat!(
                "SELECT music_id,difficulty,score,grade,play_count,combo_grade,gp,multiple,achievement,play_type,played_at,mission_clear,is_credit ",
                "FROM music_scores WHERE usn=?1 ORDER BY music_id,difficulty"
            ))?;
            let result = statement.query_map([mutation.identity.usn], |row| Ok(MusicScoreSnapshot {
                music_id: row.get(0)?, difficulty: row.get(1)?, score: row.get(2)?, grade: row.get(3)?, play_count: row.get(4)?, combo_grade: row.get(5)?, gp: row.get(6)?, multiple: row.get(7)?, achievement: row.get(8)?, play_type: row.get(9)?, played_at: row.get(10)?, mission_clear: row.get(11)?, is_credit: row.get(12)?,
            }))?.collect::<Result<_, _>>()?;
            Ok(result)
        })
    }

    pub fn ch3_snapshot_raw(&self, usn: Usn) -> Result<Ch3Snapshot, StoreError> {
        let energy = self.connection.query_row(
            "SELECT amount,cap,calculated_at FROM ch3_energy WHERE usn=?1",
            [usn],
            |row| {
                Ok(Ch3EnergySnapshot {
                    amount: row.get(0)?,
                    cap: row.get(1)?,
                    calculated_at: row.get(2)?,
                })
            },
        )?;
        let mut stage_statement = self.connection.prepare(concat!(
            "SELECT stage_id,chapter,stage_index,completed,best_star,best_score FROM ch3_stages ",
            "WHERE usn=?1 AND unlocked=1 ORDER BY chapter,stage_index,stage_id"
        ))?;
        let stages = stage_statement
            .query_map([usn], |row| {
                Ok(Ch3StageSnapshot {
                    stage_id: row.get(0)?,
                    chapter: row.get(1)?,
                    stage_index: row.get(2)?,
                    completed: row.get::<_, i64>(3)? != 0,
                    best_star: row.get(4)?,
                    best_score: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut claim_statement = self.connection.prepare(
            "SELECT chapter,reward_num FROM ch3_chapter_claims WHERE usn=?1 ORDER BY chapter,reward_num",
        )?;
        let chapter_claims = claim_statement
            .query_map([usn], |row| {
                Ok(Ch3ChapterClaimSnapshot {
                    chapter: row.get(0)?,
                    reward_num: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut profile_statement = self.connection.prepare(
            "SELECT follower_id,level,experience FROM affection WHERE usn=?1 ORDER BY follower_id",
        )?;
        let profiles = profile_statement
            .query_map([usn], |row| {
                Ok(FollowerProfileSnapshot {
                    profile_id: row.get(0)?,
                    level: row.get(1)?,
                    experience: row.get(2)?,
                    add_candy: 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Ch3Snapshot {
            energy,
            stages,
            chapter_claims,
            profiles,
        })
    }

    pub fn ch3_snapshot_at(
        &mut self,
        usn: Usn,
        now: i64,
        cap: i32,
    ) -> Result<Ch3Snapshot, StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        restore_ch3_energy(&tx, usn, now, cap)?;
        tx.commit()?;
        self.ch3_snapshot_raw(usn)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn settle_ch3_stage(
        &mut self,
        mutation: &RpcMutation,
        stage: Ch3StageDefinition,
        next: Option<Ch3StageDefinition>,
        star: i32,
        score: i64,
        energy_cost: i32,
        energy_cap: i32,
        rewards: Vec<RewardGrant>,
        profile_exp_gain: i64,
        profile_level_thresholds: Vec<(i32, i64, i32)>,
    ) -> Result<Ch3Settlement, StoreError> {
        self.commit_reward_mutation(mutation, |tx, rules| {
            let usn = mutation.identity.usn;
            let unlocked = tx
                .query_row(
                    "SELECT unlocked FROM ch3_stages WHERE usn=?1 AND stage_id=?2",
                    params![usn, stage.stage_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0)
                != 0;
            if !unlocked {
                return Err(DomainError::Locked.into());
            }
            let mut energy = restore_ch3_energy(tx, usn, mutation.committed_at, energy_cap)?;
            if energy.amount < energy_cost {
                return Err(DomainError::InsufficientEnergy.into());
            }
            energy.amount -= energy_cost;
            energy.calculated_at = mutation.committed_at;
            tx.execute(
                "UPDATE ch3_energy SET amount=?1,cap=?2,calculated_at=?3 WHERE usn=?4",
                params![energy.amount, energy.cap, energy.calculated_at, usn],
            )?;

            let clear = stage.story || star > 0;
            if clear {
                tx.execute(
                    concat!(
                        "UPDATE ch3_stages SET completed=1,best_star=MAX(best_star,?1),",
                        "best_score=MAX(best_score,?2) WHERE usn=?3 AND stage_id=?4"
                    ),
                    params![star, score, usn, stage.stage_id],
                )?;
                if let Some(next) = &next {
                    tx.execute(
                        concat!(
                            "INSERT OR IGNORE INTO ch3_stages(usn,stage_id,chapter,stage_index,unlocked) ",
                            "VALUES(?1,?2,?3,?4,1)"
                        ),
                        params![usn, next.stage_id, next.chapter, next.stage_index],
                    )?;
                }
            }

            let mut applied = Vec::new();
            if clear {
                for (index, reward) in rewards.iter().enumerate() {
                    if apply_reward_grant(tx, rules, usn, reward, mutation.committed_at, "ch3-stage")? {
                        applied.push(reward.clone());
                    }
                    tx.execute(
                        concat!(
                            "INSERT OR IGNORE INTO ch3_drops(usn,transaction_id,roll,reward_index,reward_kind,reward_id,amount) ",
                            "VALUES(?1,?2,1,?3,?4,?5,?6)"
                        ),
                        params![usn, mutation.idempotency_key, index as i64, reward.reward_type.to_string(), reward.reward_id, reward.amount],
                    )?;
                }
            }
            let profile = update_ch3_profile(
                tx,
                usn,
                if clear && !stage.story { profile_exp_gain } else { 0 },
                &profile_level_thresholds,
            )?;
            let row = tx.query_row(
                concat!(
                    "SELECT stage_id,chapter,stage_index,completed,best_star,best_score FROM ch3_stages ",
                    "WHERE usn=?1 AND stage_id=?2"
                ),
                params![usn, stage.stage_id],
                |row| Ok(Ch3StageSnapshot { stage_id: row.get(0)?, chapter: row.get(1)?, stage_index: row.get(2)?, completed: row.get::<_, i64>(3)? != 0, best_star: row.get(4)?, best_score: row.get(5)? }),
            )?;
            let result = Ch3Settlement { stage: row, energy, profile, rewards: applied };
            let mut replay = result.clone();
            replay.rewards.clear();
            Ok((result, Some(replay)))
        })
    }

    pub fn claim_ch3_chapter_reward(
        &mut self,
        mutation: &RpcMutation,
        chapter: i32,
        reward_num: i32,
        goal: i32,
        rewards: Vec<RewardGrant>,
        energy_cap: i32,
    ) -> Result<(Ch3EnergySnapshot, Vec<RewardGrant>), StoreError> {
        self.commit_reward_mutation(mutation, |tx, rules| {
            let usn = mutation.identity.usn;
            let stars: i64 = tx.query_row(
                "SELECT COALESCE(SUM(best_star),0) FROM ch3_stages WHERE usn=?1 AND chapter=?2",
                params![usn, chapter],
                |row| row.get(0),
            )?;
            if stars < i64::from(goal) {
                return Err(DomainError::Locked.into());
            }
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO ch3_chapter_claims(usn,chapter,reward_num,claimed_at) VALUES(?1,?2,?3,?4)",
                params![usn, chapter, reward_num, mutation.committed_at],
            )?;
            if inserted == 0 {
                return Err(DomainError::AlreadyClaimed.into());
            }
            let mut applied = Vec::new();
            for reward in &rewards {
                if apply_reward_grant(tx, rules, usn, reward, mutation.committed_at, "ch3-chapter")? {
                    applied.push(reward.clone());
                }
            }
            let energy = restore_ch3_energy(tx, usn, mutation.committed_at, energy_cap)?;
            let first = (energy.clone(), applied);
            Ok((first, Some((energy, Vec::new()))))
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn settle_ch3_stream(
        &mut self,
        mutation: &RpcMutation,
        stage_id: i64,
        count: i32,
        energy_cost_each: i32,
        energy_cap: i32,
        reward_rolls: Vec<(i32, Vec<RewardGrant>)>,
        profile_exp_each: i64,
        profile_level_thresholds: Vec<(i32, i64, i32)>,
    ) -> Result<Ch3StreamSettlement, StoreError> {
        if !(1..=5).contains(&count) {
            return Err(DomainError::InvalidStage.into());
        }
        self.commit_reward_mutation(mutation, |tx, rules| {
            let usn = mutation.identity.usn;
            let completed = tx
                .query_row(
                    "SELECT completed FROM ch3_stages WHERE usn=?1 AND stage_id=?2",
                    params![usn, stage_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0)
                != 0;
            if !completed {
                return Err(DomainError::Locked.into());
            }
            let mut energy = restore_ch3_energy(tx, usn, mutation.committed_at, energy_cap)?;
            let cost = energy_cost_each.saturating_mul(count);
            if energy.amount < cost {
                return Err(DomainError::InsufficientEnergy.into());
            }
            energy.amount -= cost;
            tx.execute(
                "UPDATE ch3_energy SET amount=?1,cap=?2,calculated_at=?3 WHERE usn=?4",
                params![energy.amount, energy.cap, mutation.committed_at, usn],
            )?;
            let mut applied_rolls = Vec::new();
            for (roll, rewards) in &reward_rolls {
                let mut applied = Vec::new();
                for (reward_index, reward) in rewards.iter().enumerate() {
                    if apply_reward_grant(tx, rules, usn, reward, mutation.committed_at, "ch3-stream")? {
                        applied.push(reward.clone());
                    }
                    tx.execute(
                        concat!(
                            "INSERT OR IGNORE INTO ch3_drops(usn,transaction_id,roll,reward_index,reward_kind,reward_id,amount) ",
                            "VALUES(?1,?2,?3,?4,?5,?6,?7)"
                        ),
                        params![usn, mutation.idempotency_key, roll, reward_index as i64, reward.reward_type.to_string(), reward.reward_id, reward.amount],
                    )?;
                }
                applied_rolls.push((*roll, applied));
            }
            let profile = update_ch3_profile(
                tx,
                usn,
                profile_exp_each.saturating_mul(i64::from(count)),
                &profile_level_thresholds,
            )?;
            let result = Ch3StreamSettlement { stage_id, energy, profile, reward_rolls: applied_rolls };
            let mut replay = result.clone();
            replay.reward_rolls.clear();
            Ok((result, Some(replay)))
        })
    }

    pub fn last_save_time(&self, usn: Usn) -> Result<(i64, String), StoreError> {
        Ok(self.connection.query_row(
            "SELECT last_save_time,device_uuid FROM user_core WHERE usn=?1",
            [usn],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    pub fn update_avatar(
        &mut self,
        mutation: &RpcMutation,
        avatar_id: i64,
    ) -> Result<i64, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            tx.execute(
                "UPDATE user_core SET avatar_id=?1 WHERE usn=?2",
                params![avatar_id, mutation.identity.usn],
            )?;
            Ok(avatar_id)
        })
    }

    pub fn update_nickname(
        &mut self,
        mutation: &RpcMutation,
        nickname: &str,
    ) -> Result<String, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            if !nickname.is_empty() {
                tx.execute(
                    "UPDATE user_core SET nickname=?1 WHERE usn=?2",
                    params![nickname, mutation.identity.usn],
                )?;
            }
            Ok(tx.query_row(
                "SELECT nickname FROM user_core WHERE usn=?1",
                [mutation.identity.usn],
                |row| row.get(0),
            )?)
        })
    }

    pub fn update_user_title(
        &mut self,
        mutation: &RpcMutation,
        title_id: i64,
    ) -> Result<i64, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            tx.execute(
                "UPDATE user_core SET title_id=?1 WHERE usn=?2",
                params![title_id, mutation.identity.usn],
            )?;
            Ok(title_id)
        })
    }

    pub fn update_user_gp(&mut self, mutation: &RpcMutation, gp: i64) -> Result<i64, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            tx.execute(
                "UPDATE area_state SET gp=MAX(gp,?1) WHERE usn=?2 AND area=1",
                params![gp.max(0) as f64, mutation.identity.usn],
            )?;
            let stored: f64 = tx.query_row(
                "SELECT gp FROM area_state WHERE usn=?1 AND area=1",
                [mutation.identity.usn],
                |row| row.get(0),
            )?;
            Ok(stored.clamp(0.0, i64::MAX as f64) as i64)
        })
    }

    pub fn set_follower_quest_infinite(
        &mut self,
        mutation: &RpcMutation,
    ) -> Result<FollowerQuestSnapshot, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            let stage_id = tx
                .query_row(
                    concat!(
                        "SELECT stage_id FROM follower_quests WHERE usn=?1 AND chain_id=1 ",
                        "ORDER BY stage_id DESC LIMIT 1"
                    ),
                    [mutation.identity.usn],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(1);
            tx.execute(
                concat!(
                    "INSERT INTO follower_quests(usn,chain_id,stage_id,completed,infinite,updated_at) ",
                    "VALUES(?1,1,?2,0,1,?3) ON CONFLICT(usn,chain_id,stage_id) DO UPDATE SET ",
                    "infinite=1,updated_at=excluded.updated_at"
                ),
                params![mutation.identity.usn, stage_id, mutation.committed_at],
            )?;
            quest_snapshot(tx, mutation.identity.usn, stage_id)
        })
    }

    pub fn grant_package_contents(
        &mut self,
        mutation: &RpcMutation,
        contents: &[(ContentKind, i32, i64)],
        music_ids: &[i64],
    ) -> Result<PackagePurchaseOutcome, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            for (kind, area, content_id) in contents {
                let key = content_key(*kind);
                tx.execute(
                    concat!(
                        "INSERT OR IGNORE INTO owned_content(usn,kind,area,content_id,acquired_at,source) ",
                        "VALUES(?1,?2,?3,?4,?5,'package')"
                    ),
                    params![
                        mutation.identity.usn,
                        key,
                        area,
                        content_id,
                        mutation.committed_at
                    ],
                )?;
                let reward_level = i64::from(*kind == ContentKind::Costume);
                tx.execute(
                    concat!(
                        "INSERT INTO content_levels(usn,kind,content_id,level,reward_level) ",
                        "VALUES(?1,?2,?3,1,?4) ON CONFLICT(usn,kind,content_id) DO UPDATE SET ",
                        "level=MAX(content_levels.level,1),",
                        "reward_level=MAX(content_levels.reward_level,excluded.reward_level)"
                    ),
                    params![mutation.identity.usn, key, content_id, reward_level],
                )?;
            }
            let balances = currency_balances(tx, mutation.identity.usn)?;
            Ok(PackagePurchaseOutcome {
                chocolate: balances.0,
                candy: balances.1,
                music_ids: music_ids.to_vec(),
            })
        })
    }

    pub fn merge_tutorials(
        &mut self,
        mutation: &RpcMutation,
        tutorial_ids: &[i64],
    ) -> Result<Vec<i64>, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            for tutorial_id in tutorial_ids.iter().copied().filter(|value| *value > 0) {
                tx.execute(
                    "INSERT OR IGNORE INTO tutorial_completed(usn,tutorial_id,completed_at) VALUES(?1,?2,?3)",
                    params![mutation.identity.usn, tutorial_id, mutation.committed_at],
                )?;
            }
            let mut statement = tx.prepare(
                "SELECT tutorial_id FROM tutorial_completed WHERE usn=?1 ORDER BY tutorial_id",
            )?;
            Ok(statement
                .query_map([mutation.identity.usn], |row| row.get(0))?
                .collect::<Result<_, _>>()?)
        })
    }

    pub fn merge_user_save(
        &mut self,
        mutation: &RpcMutation,
        patch: &UserSavePatch,
        _device_uuid: &str,
        pass_point_multiplier: i64,
        pass_thresholds: &[(i64, i32, i64)],
    ) -> Result<String, StoreError> {
        let pass_thresholds = pass_thresholds.to_vec();
        self.commit_rpc_mutation(mutation, |tx| {
            if let Some(core) = &patch.core {
                if let Some(value) = core.ch1_like.filter(|value| value.is_finite() && *value >= 0.0)
                {
                    tx.execute(
                        "UPDATE currencies SET amount=?1 WHERE usn=?2 AND kind='ch1_like'",
                        params![value, mutation.identity.usn],
                    )?;
                }
                if let Some(value) = core.fans.filter(|value| *value >= 0) {
                    tx.execute(
                        "UPDATE currencies SET amount=MAX(amount,?1) WHERE usn=?2 AND kind='fans'",
                        params![value as f64, mutation.identity.usn],
                    )?;
                }
                tx.execute(
                    "UPDATE user_core SET last_save_time=?1 WHERE usn=?2",
                    params![mutation.committed_at, mutation.identity.usn],
                )?;
                tx.execute(
                    concat!(
                        "UPDATE area_state SET current_costume=COALESCE(?1,current_costume),",
                        "current_music=COALESCE(?2,current_music) WHERE usn=?3 AND area=1"
                    ),
                    params![
                        core.current_costume,
                        core.current_music,
                        mutation.identity.usn
                    ],
                )?;
            }
            for area in &patch.areas {
                if !(1..=2).contains(&area.area) {
                    continue;
                }
                tx.execute(
                    concat!(
                        "INSERT INTO area_state(usn,area,area_level,fans,candy,current_costume,current_guitar,current_music,unlocked) ",
                        "VALUES(?1,?2,COALESCE(?3,1),COALESCE(?4,0),COALESCE(?5,0),?6,?7,?8,CASE WHEN ?2=1 THEN 1 ELSE 0 END) ",
                        "ON CONFLICT(usn,area) DO UPDATE SET ",
                        "area_level=COALESCE(?3,area_state.area_level),",
                        "fans=COALESCE(?4,area_state.fans),candy=COALESCE(?5,area_state.candy),",
                        "current_costume=COALESCE(excluded.current_costume,area_state.current_costume),",
                        "current_guitar=COALESCE(excluded.current_guitar,area_state.current_guitar),",
                        "current_music=COALESCE(excluded.current_music,area_state.current_music)"
                    ),
                    params![
                        mutation.identity.usn,
                        area.area,
                        area.fan_level,
                        area.fans.filter(|value| *value >= 0).map(|value| value as f64),
                        area.candy.filter(|value| value.is_finite() && *value >= 0.0),
                        area.current_costume,
                        area.current_guitar,
                        area.current_music
                    ],
                )?;
                // Selection is not proof of ownership.  The stock client
                // serializes locked CH2 defaults (201) in every fresh save;
                // promoting those ids here injects locked rows into
                // User_music/User_costume/User_guitar and corrupts the
                // client's content dictionaries. Ownership comes only from
                // the explicit user-content lists or a business transaction.
                let like_kind = if area.area == 2 { "ch2_like" } else { "ch1_like" };
                if let Some(value) = area.like_amount.filter(|value| value.is_finite() && *value >= 0.0)
                {
                    tx.execute(
                        "UPDATE currencies SET amount=?1 WHERE usn=?2 AND kind=?3",
                        params![value, mutation.identity.usn, like_kind],
                    )?;
                }
                if let Some(value) = area.fans.filter(|value| *value >= 0) {
                    tx.execute(
                        "UPDATE currencies SET amount=MAX(amount,?1) WHERE usn=?2 AND kind='fans'",
                        params![value as f64, mutation.identity.usn],
                    )?;
                }
                if let Some(value) = area
                    .candy
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    && area.area == 1
                {
                    tx.execute(
                        "UPDATE currencies SET amount=?1 WHERE usn=?2 AND kind='candy'",
                        params![value, mutation.identity.usn],
                    )?;
                }
                if area.area == 1
                    && let Some(level) = area.fan_level.filter(|value| *value > 0)
                {
                    tx.execute(
                        "UPDATE user_core SET fan_level=?1 WHERE usn=?2",
                        params![level, mutation.identity.usn],
                    )?;
                }
            }
            for content in &patch.content {
                if content.content_id <= 0 {
                    continue;
                }
                let kind = content_key(content.kind);
                // A level-zero (locked) row or an id-only partial snapshot
                // cannot acquire content. Existing ownership may still receive
                // partial updates; new ownership requires a positive level.
                if content.level.is_none_or(|level| level <= 0)
                    && !tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM owned_content WHERE usn=?1 AND kind=?2 AND content_id=?3)",
                        params![mutation.identity.usn, kind, content.content_id],
                        |row| row.get::<_, bool>(0),
                    )?
                {
                    continue;
                }
                tx.execute(
                    concat!(
                        "INSERT OR IGNORE INTO owned_content(usn,kind,area,content_id,acquired_at,source) ",
                        "VALUES(?1,?2,?3,?4,?5,'userSave')"
                    ),
                    params![
                        mutation.identity.usn,
                        kind,
                        content.area,
                        content.content_id,
                        mutation.committed_at
                    ],
                )?;
                if let Some(level) = content.level.filter(|value| *value > 0) {
                    tx.execute(
                        concat!(
                            "INSERT INTO content_levels(usn,kind,content_id,level,reward_level) ",
                            "VALUES(?1,?2,?3,?4,COALESCE(?5,0)) ",
                            "ON CONFLICT(usn,kind,content_id) DO UPDATE SET ",
                            "level=MAX(content_levels.level,excluded.level),",
                            "reward_level=MAX(content_levels.reward_level,excluded.reward_level)"
                        ),
                        params![
                            mutation.identity.usn,
                            kind,
                            content.content_id,
                            level,
                            content.reward_level
                        ],
                    )?;
                }
            }
            for music in &patch.music_states {
                if music.music_id <= 0 || !tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM owned_content WHERE usn=?1 AND kind='music' AND content_id=?2)",
                    params![mutation.identity.usn, music.music_id],
                    |row| row.get::<_, bool>(0),
                )? {
                    continue;
                }
                tx.execute(
                    concat!(
                        "INSERT INTO music_state(usn,music_id,encore_appears,encore_follower_id,encore_ready_at,ch3_active_until) ",
                        "VALUES(?1,?2,?3,?4,0,0) ON CONFLICT(usn,music_id) DO UPDATE SET ",
                        "encore_appears=excluded.encore_appears,",
                        "encore_follower_id=COALESCE(excluded.encore_follower_id,music_state.encore_follower_id)"
                    ),
                    params![
                        mutation.identity.usn,
                        music.music_id,
                        i64::from(music.encore_appears),
                        music.encore_follower_id
                    ],
                )?;
            }
            for achievement in &patch.achievements {
                if achievement.id <= 0 || !achievement.quantity.is_finite() || achievement.quantity < 0.0 {
                    continue;
                }
                if achievement.id == 10 {
                    let attendance_count: i64 = tx.query_row(
                        "SELECT attendance_count FROM attendance_state WHERE usn=?1",
                        [mutation.identity.usn],
                        |row| row.get(0),
                    )?;
                    tx.execute(
                        concat!(
                            "INSERT INTO achievement_progress(usn,achievement_id,quantity,quantity_text,updated_at) ",
                            "VALUES(?1,10,?2,CAST(?2 AS TEXT),?3) ON CONFLICT(usn,achievement_id) DO UPDATE SET ",
                            "quantity=excluded.quantity,quantity_text=excluded.quantity_text,updated_at=excluded.updated_at"
                        ),
                        params![
                            mutation.identity.usn,
                            attendance_count,
                            mutation.committed_at
                        ],
                    )?;
                } else {
                    tx.execute(
                        concat!(
                            "INSERT INTO achievement_progress(usn,achievement_id,quantity,quantity_text,updated_at) ",
                            "VALUES(?1,?2,?3,COALESCE(?4,'0'),?5) ",
                            "ON CONFLICT(usn,achievement_id) DO UPDATE SET ",
                            "quantity=MAX(achievement_progress.quantity,excluded.quantity),",
                            "quantity_text=CASE WHEN excluded.quantity>=achievement_progress.quantity ",
                            "THEN excluded.quantity_text ELSE achievement_progress.quantity_text END,updated_at=?5"
                        ),
                        params![mutation.identity.usn, achievement.id, achievement.quantity, achievement.quantity_text, mutation.committed_at],
                    )?;
                }
            }
            for mission in &patch.daily_missions {
                if mission.id <= 0 || !mission.quantity.is_finite() || mission.quantity < 0.0 {
                    continue;
                }
                tx.execute(
                    concat!(
                        "INSERT INTO daily_missions(usn,mission_id,quantity,updated_at) VALUES(?1,?2,?3,?4) ",
                        "ON CONFLICT(usn,mission_id) DO UPDATE SET ",
                        "quantity=MAX(daily_missions.quantity,excluded.quantity),updated_at=?4"
                    ),
                    params![mutation.identity.usn, mission.id, mission.quantity, mutation.committed_at],
                )?;
            }
            for skill in &patch.skill_activations {
                if skill.skill_id <= 0 {
                    continue;
                }
                tx.execute(
                    concat!(
                        "INSERT INTO skill_activation(usn,skill_id,active,active_from,active_until,reset_ready_at) ",
                        "VALUES(?1,?2,?3,?4,?5,0) ON CONFLICT(usn,skill_id) DO UPDATE SET ",
                        "active=CASE WHEN excluded.active_from>=skill_activation.active_from THEN excluded.active ELSE skill_activation.active END,",
                        "active_from=MAX(skill_activation.active_from,excluded.active_from),",
                        "active_until=MAX(skill_activation.active_until,excluded.active_until)"
                    ),
                    params![
                        mutation.identity.usn,
                        skill.skill_id,
                        i64::from(skill.active),
                        skill.active_from,
                        skill.active_until
                    ],
                )?;
            }
            for messenger in &patch.messengers {
                if messenger.room_id <= 0 {
                    continue;
                }
                tx.execute(
                    concat!(
                        "INSERT INTO messenger_rooms(usn,room_id,state,last_confirm_index,unlock_group_list,update_time_ticks,updated_at) ",
                        "VALUES(?1,?2,0,?3,?4,?5,?6) ON CONFLICT(usn,room_id) DO UPDATE SET ",
                        "last_confirm_index=MAX(messenger_rooms.last_confirm_index,excluded.last_confirm_index),",
                        "unlock_group_list=CASE WHEN excluded.update_time_ticks>=messenger_rooms.update_time_ticks ",
                        "THEN excluded.unlock_group_list ELSE messenger_rooms.unlock_group_list END,",
                        "update_time_ticks=MAX(messenger_rooms.update_time_ticks,excluded.update_time_ticks),updated_at=?6"
                    ),
                    params![
                        mutation.identity.usn,
                        messenger.room_id,
                        messenger.last_confirm_index,
                        messenger.unlock_group_list,
                        messenger.update_time_ticks,
                        mutation.committed_at
                    ],
                )?;
            }
            for pass in &patch.pass_progress {
                if !(1..=13).contains(&pass.season) || pass.points < 0 || pass.version < 0 {
                    continue;
                }
                let (current_points, current_step) = tx
                    .query_row(
                        "SELECT points,step FROM pass_progress WHERE usn=?1 AND season=?2 AND version=?3",
                        params![mutation.identity.usn, pass.season, pass.version],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i32>(1)?)),
                    )
                    .optional()?
                    .unwrap_or((0, 0));
                let points = if pass.points > current_points {
                    current_points.saturating_add(
                        pass.points
                            .saturating_sub(current_points)
                            .saturating_mul(pass_point_multiplier.max(1)),
                    )
                } else {
                    current_points
                };
                let inferred_step = pass_thresholds
                    .iter()
                    .filter(|(season, _, goal)| *season == pass.season && *goal <= points)
                    .map(|(_, step, _)| *step)
                    .max()
                    .unwrap_or(0);
                let step = current_step.max(pass.step).max(inferred_step);
                tx.execute(
                    concat!(
                        "INSERT INTO pass_progress(usn,season,points,step,version) VALUES(?1,?2,?3,?4,?5) ",
                        "ON CONFLICT(usn,season,version) DO UPDATE SET ",
                        "points=excluded.points,step=excluded.step"
                    ),
                    params![
                        mutation.identity.usn,
                        pass.season,
                        points,
                        step,
                        pass.version
                    ],
                )?;
            }
            if !patch.follower_quests.is_empty() {
                // The tested Rust baseline deliberately ignores the counters and CurrentID in
                // asynchronous userSave snapshots. setUserFollowerQuest owns this ledger; the
                // client can otherwise relabel a completed stage as the next stage and make it
                // immediately claimable.
                let current_id = tx
                    .query_row(
                        "SELECT MAX(stage_id) FROM follower_quests WHERE usn=?1 AND chain_id=1",
                        [mutation.identity.usn],
                        |row| row.get::<_, Option<i64>>(0),
                    )?
                    .unwrap_or(1)
                    .max(1);
                tx.execute(
                    concat!(
                        "INSERT INTO follower_quests(usn,chain_id,stage_id,completed,updated_at) ",
                        "VALUES(?1,1,?2,0,?3) ON CONFLICT(usn,chain_id,stage_id) DO UPDATE SET updated_at=?3"
                    ),
                    params![mutation.identity.usn, current_id, mutation.committed_at],
                )?;
                for condition in 1..=3_i64 {
                    tx.execute(
                        concat!(
                            "INSERT INTO quest_conditions(usn,chain_id,stage_id,condition_id,quantity,claimed) ",
                            "VALUES(?1,1,?2,?3,0,0) ON CONFLICT(usn,chain_id,stage_id,condition_id) DO NOTHING"
                        ),
                        params![mutation.identity.usn, current_id, condition],
                    )?;
                }
            }
            tx.execute(
                "UPDATE user_core SET last_save_time=?1 WHERE usn=?2",
                params![mutation.committed_at, mutation.identity.usn],
            )?;
            Ok("Y".to_owned())
        })
    }

    pub fn attendance(
        &mut self,
        mutation: &RpcMutation,
        clock: DeviceClock,
        mode: &str,
    ) -> Result<AttendanceResult, StoreError> {
        let mode = mode.to_ascii_lowercase();
        self.commit_rpc_mutation(mutation, |tx| {
            let local_day = clock.local_epoch_day();
            let already = tx
                .query_row(
                    "SELECT 1 FROM attendance_days WHERE usn=?1 AND local_day=?2",
                    params![mutation.identity.usn, local_day],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            let mut status = if already { "N" } else { "Y" };
            if mode == "add" && !already {
                let (continuous, maximum, last_day): (i32, i32, Option<i64>) = tx.query_row(
                    "SELECT continuous_count,max_continuous_count,last_local_day FROM attendance_state WHERE usn=?1",
                    [mutation.identity.usn],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
                let current_streak = if last_day == Some(local_day - 1) {
                    continuous.saturating_add(1)
                } else {
                    1
                };
                tx.execute(
                    "INSERT INTO attendance_days(usn,local_day,claimed_at) VALUES(?1,?2,?3)",
                    params![mutation.identity.usn, local_day, mutation.committed_at],
                )?;
                tx.execute(
                    concat!(
                        "UPDATE attendance_state SET attendance_count=attendance_count+1,",
                        "continuous_count=?1,max_continuous_count=MAX(?2,?1),",
                        "last_local_day=?3 WHERE usn=?4"
                    ),
                    params![current_streak, maximum, local_day, mutation.identity.usn],
                )?;
                status = "ADD";
            }
            let (count, maximum): (i32, i32) = tx.query_row(
                "SELECT attendance_count,max_continuous_count FROM attendance_state WHERE usn=?1",
                [mutation.identity.usn],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok(AttendanceResult {
                status: status.to_owned(),
                attendance_count: count,
                attendance_date: local_day.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                max_continuous_count: maximum,
            })
        })
    }

    pub fn grant_currency(
        &mut self,
        usn: Usn,
        kind: CurrencyKind,
        amount: f64,
        transaction_id: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        if !amount.is_finite() || amount < 0.0 {
            return Err(DomainError::InvalidAmount.into());
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted = tx.execute("INSERT OR IGNORE INTO reward_transactions(usn,transaction_id,source,reward_json,created_at) VALUES(?1,?2,'currency',?3,?4)", params![usn,transaction_id,serde_json::to_string(&Reward::Currency { currency: kind, amount }).unwrap(),now])?;
        if inserted != 0 {
            tx.execute(
                "UPDATE currencies SET amount=amount+?1 WHERE usn=?2 AND kind=?3",
                params![amount, usn, kind.key()],
            )?;
        }
        tx.commit()?;
        Ok(inserted != 0)
    }

    pub fn queue_mail(
        &mut self,
        usn: Usn,
        post_id: i64,
        subject_key: &str,
        body_key: &str,
        reward: &Reward,
        now: i64,
    ) -> Result<bool, StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::ensure_unique_content_can_be_mailed(&tx, usn, reward)?;
        let inserted = Self::insert_mail_in_transaction(
            &tx,
            usn,
            post_id,
            subject_key,
            body_key,
            reward,
            now,
        )?;
        tx.commit()?;
        Ok(inserted)
    }

    pub fn queue_generated_mail(
        &mut self,
        usn: Usn,
        subject_key: &str,
        body_key: &str,
        reward: &Reward,
        now: i64,
    ) -> Result<i64, StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let post_id = Self::insert_generated_mail_in_transaction(
            &tx,
            usn,
            subject_key,
            body_key,
            reward,
            now,
        )?;
        tx.commit()?;
        Ok(post_id)
    }

    pub fn queue_generated_mail_rpc(
        &mut self,
        mutation: &RpcMutation,
        subject_key: &str,
        body_key: &str,
        reward: &Reward,
    ) -> Result<i64, StoreError> {
        let subject_key = subject_key.to_owned();
        let body_key = body_key.to_owned();
        let reward = reward.clone();
        self.commit_rpc_mutation(mutation, |tx| {
            Self::insert_generated_mail_in_transaction(
                tx,
                mutation.identity.usn,
                &subject_key,
                &body_key,
                &reward,
                mutation.committed_at,
            )
        })
    }

    fn insert_generated_mail_in_transaction(
        tx: &rusqlite::Transaction<'_>,
        usn: Usn,
        subject_key: &str,
        body_key: &str,
        reward: &Reward,
        now: i64,
    ) -> Result<i64, StoreError> {
        Self::ensure_unique_content_can_be_mailed(tx, usn, reward)?;
        let post_id = tx.query_row(
            "SELECT COALESCE(MAX(post_id),0)+1 FROM mail WHERE usn=?1",
            [usn],
            |row| row.get(0),
        )?;
        if !Self::insert_mail_in_transaction(tx, usn, post_id, subject_key, body_key, reward, now)?
        {
            return Err(StoreError::IdempotencyConflict);
        }
        Ok(post_id)
    }

    /// Shared by the Memorial catalog and the transactional mail guard. Reward
    /// grants without a chapter use area 0; those rows still mean the player
    /// owns this globally identified item and must not permit a second grant.
    pub fn content_delivery_state(
        &self, usn: Usn, content: ContentKind, area: i32, content_id: i64,
    ) -> Result<(bool, bool), StoreError> {
        Self::content_delivery_state_on(&self.connection, usn, content, area, content_id)
    }

    fn content_delivery_state_on(
        connection: &Connection, usn: Usn, content: ContentKind, area: i32, content_id: i64,
    ) -> Result<(bool, bool), StoreError> {
        let kind = content_key(content);
        let owned = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM owned_content WHERE usn=?1 AND kind=?2 AND area IN (0,?3) AND content_id=?4)",
            params![usn, kind, area, content_id], |row| row.get(0),
        )?;
        let pending = connection.query_row(
            concat!(
                "SELECT EXISTS(SELECT 1 FROM mail m JOIN mail_rewards r ON r.usn=m.usn AND r.post_id=m.post_id ",
                "WHERE m.usn=?1 AND m.claimed_at IS NULL AND m.deleted_at IS NULL ",
                "AND r.reward_kind=?2 AND r.reward_id=?3 AND r.area IN (0,?4))"
            ),
            params![usn, kind, content_id, area], |row| row.get(0),
        )?;
        Ok((owned, pending))
    }

    fn ensure_unique_content_can_be_mailed(
        tx: &rusqlite::Transaction<'_>,
        usn: Usn,
        reward: &Reward,
    ) -> Result<(), StoreError> {
        let Reward::Content {
            content,
            area,
            content_id,
        } = reward
        else {
            return Ok(());
        };
        let (already_owned, already_queued) =
            Self::content_delivery_state_on(tx, usn, *content, *area, *content_id)?;
        if already_owned || already_queued {
            return Err(StoreError::Domain(DomainError::AlreadyOwned));
        }
        Ok(())
    }

    fn insert_mail_in_transaction(
        tx: &rusqlite::Transaction<'_>,
        usn: Usn,
        post_id: i64,
        subject_key: &str,
        body_key: &str,
        reward: &Reward,
        now: i64,
    ) -> Result<bool, StoreError> {
        let inserted = tx.execute("INSERT OR IGNORE INTO mail(usn,post_id,subject_key,body_key,created_at) VALUES(?1,?2,?3,?4,?5)", params![usn,post_id,subject_key,body_key,now])?;
        if inserted == 0 {
            return Ok(false);
        }
        match reward {
            Reward::Currency { currency, amount } => tx.execute("INSERT INTO mail_rewards(usn,post_id,reward_kind,amount) VALUES(?1,?2,?3,?4)", params![usn,post_id,currency.key(),amount])?,
            Reward::Content { content, area, content_id } => tx.execute("INSERT INTO mail_rewards(usn,post_id,reward_kind,reward_id,area) VALUES(?1,?2,?3,?4,?5)", params![usn,post_id,content_key(*content),content_id,area])?,
        };
        bump_mail_cursor(tx, usn)?;
        Ok(true)
    }

    pub fn mailbox(&self, usn: Usn) -> Result<MailboxSnapshot, StoreError> {
        let cursor = self.connection.query_row(
            "SELECT cursor FROM mail_state WHERE usn=?1",
            [usn],
            |row| row.get(0),
        )?;
        let mut statement = self.connection.prepare(concat!(
            "SELECT m.post_id,m.subject_key,m.body_key,m.created_at,m.read_at,m.claimed_at,",
            "r.reward_kind,r.reward_id,r.amount,r.area FROM mail m JOIN mail_rewards r ",
            "ON r.usn=m.usn AND r.post_id=m.post_id WHERE m.usn=?1 AND m.deleted_at IS NULL ",
            "ORDER BY m.created_at DESC,m.post_id DESC"
        ))?;
        let mail = statement
            .query_map([usn], |row| {
                let reward_kind: String = row.get(6)?;
                let reward_id: Option<i64> = row.get(7)?;
                let amount: Option<f64> = row.get(8)?;
                let area: i32 = row.get(9)?;
                let reward = stored_reward_grant(&reward_kind, reward_id, amount, area)
                    .map_err(store_to_sql)?;
                Ok(MailSnapshot {
                    post_id: row.get(0)?,
                    subject: row.get(1)?,
                    body: row.get(2)?,
                    created_at: row.get(3)?,
                    read: row.get::<_, Option<i64>>(4)?.is_some(),
                    claimed: row.get::<_, Option<i64>>(5)?.is_some(),
                    reward,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(MailboxSnapshot { cursor, mail })
    }

    pub fn read_mail(&mut self, mutation: &RpcMutation, post_id: i64) -> Result<i64, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            let changed = tx.execute(
                concat!(
                    "UPDATE mail SET read_at=COALESCE(read_at,?1) ",
                    "WHERE usn=?2 AND post_id=?3 AND deleted_at IS NULL"
                ),
                params![mutation.committed_at, mutation.identity.usn, post_id],
            )?;
            if changed == 0 {
                return Err(StoreError::MissingMail(post_id));
            }
            bump_mail_cursor(tx, mutation.identity.usn)?;
            Ok(post_id)
        })
    }

    pub fn claim_mail_rpc(
        &mut self,
        mutation: &RpcMutation,
        post_id: i64,
    ) -> Result<Option<RewardGrant>, StoreError> {
        self.commit_reward_mutation(mutation, |tx, rules| {
            let row = mail_reward_row(tx, mutation.identity.usn, post_id)?;
            if row.claimed_at.is_some() {
                return Err(StoreError::MailAlreadyClaimed(post_id));
            }
            let grant = stored_reward_grant(&row.kind, row.reward_id, row.amount, row.area)?;
            apply_stored_reward(
                tx, rules,
                mutation.identity.usn,
                &row,
                mutation.committed_at,
                "mail",
            )?;
            tx.execute(
                "UPDATE mail SET claimed_at=?1,read_at=COALESCE(read_at,?1) WHERE usn=?2 AND post_id=?3",
                params![mutation.committed_at, mutation.identity.usn, post_id],
            )?;
            bump_mail_cursor(tx, mutation.identity.usn)?;
            Ok((Some(grant), Some(None)))
        })
    }

    pub fn delete_mail(
        &mut self,
        mutation: &RpcMutation,
        post_id: i64,
        delete_all_confirmed: bool,
    ) -> Result<i64, StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            let changed = if delete_all_confirmed {
                tx.execute(
                    concat!(
                        "UPDATE mail SET deleted_at=?1 WHERE usn=?2 AND deleted_at IS NULL AND ",
                        "((claimed_at IS NOT NULL) OR (NOT EXISTS(SELECT 1 FROM mail_rewards r ",
                        "WHERE r.usn=mail.usn AND r.post_id=mail.post_id) AND read_at IS NOT NULL))"
                    ),
                    params![mutation.committed_at, mutation.identity.usn],
                )?
            } else {
                tx.execute(
                    concat!(
                        "UPDATE mail SET deleted_at=?1 WHERE usn=?2 AND post_id=?3 AND deleted_at IS NULL ",
                        "AND claimed_at IS NOT NULL"
                    ),
                    params![mutation.committed_at, mutation.identity.usn, post_id],
                )?
            };
            if changed != 0 {
                bump_mail_cursor(tx, mutation.identity.usn)?;
            }
            Ok(post_id)
        })
    }

    pub fn purchase_rewards(
        &mut self,
        mutation: &RpcMutation,
        command: RewardPurchaseCommand,
    ) -> Result<PurchaseOutcome, StoreError> {
        let RewardPurchaseCommand {
            transaction_id: purchase_id,
            purchase_kind,
            target_id,
            cost,
            rewards,
            ad_progression,
        } = command;
        self.commit_reward_mutation(mutation, |tx, rules| {
            let previous = tx
                .query_row(
                    "SELECT 1 FROM purchase_transactions WHERE usn=?1 AND transaction_id=?2",
                    params![mutation.identity.usn, purchase_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if previous {
                let balances = currency_balances(tx, mutation.identity.usn)?;
                let empty = PurchaseOutcome {
                    chocolate: balances.0,
                    candy: balances.1,
                    rewards: Vec::new(),
                    ad_level: ad_progression
                        .as_ref()
                        .map(|progression| ad_level_snapshot(tx, mutation.identity.usn, progression.group_id))
                        .transpose()?,
                };
                return Ok((empty.clone(), Some(empty)));
            }
            if let Some((kind, amount)) = cost {
                if !amount.is_finite() || amount < 0.0 {
                    return Err(DomainError::InvalidAmount.into());
                }
                deduct_currency(tx, mutation.identity.usn, kind, amount)?;
            }
            let mut granted = Vec::new();
            for reward in &rewards {
                if apply_reward_grant(
                    tx, rules,
                    mutation.identity.usn,
                    reward,
                    mutation.committed_at,
                    &purchase_kind,
                )? {
                    granted.push(reward.clone());
                }
            }
            let ad_level = ad_progression
                .as_ref()
                .map(|progression| {
                    advance_ad_level(tx, mutation.identity.usn, progression)
                })
                .transpose()?;
            tx.execute(
                concat!(
                    "INSERT INTO purchase_transactions(usn,transaction_id,purchase_kind,target_id,status,created_at) ",
                    "VALUES(?1,?2,?3,?4,'complete',?5)"
                ),
                params![
                    mutation.identity.usn,
                    purchase_id,
                    purchase_kind,
                    target_id,
                    mutation.committed_at
                ],
            )?;
            let balances = currency_balances(tx, mutation.identity.usn)?;
            let initial = PurchaseOutcome {
                chocolate: balances.0,
                candy: balances.1,
                rewards: granted,
                ad_level: ad_level.clone(),
            };
            let replay = PurchaseOutcome {
                chocolate: balances.0,
                candy: balances.1,
                rewards: Vec::new(),
                ad_level,
            };
            Ok((initial, Some(replay)))
        })
    }

    pub fn purchase_content(
        &mut self,
        mutation: &RpcMutation,
        command: ContentPurchaseCommand,
    ) -> Result<ContentPurchaseOutcome, StoreError> {
        let ContentPurchaseCommand {
            kind,
            area,
            content_id,
            currency,
            price,
            price_increment,
            max_level,
        } = command;
        let key = content_key(kind);
        self.commit_rpc_mutation(mutation, |tx| {
            if !price.is_finite() || price < 0.0 || !price_increment.is_finite() || price_increment < 0.0 {
                return Err(DomainError::InvalidAmount.into());
            }
            let current_level = tx
                .query_row(
                    "SELECT level FROM content_levels WHERE usn=?1 AND kind=?2 AND content_id=?3",
                    params![mutation.identity.usn, key, content_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0);
            if current_level >= max_level.max(1) {
                let balances = currency_balances(tx, mutation.identity.usn)?;
                return Ok(ContentPurchaseOutcome {
                    chocolate: balances.0,
                    candy: balances.1,
                    kind,
                    content_id,
                    level: current_level,
                });
            }
            // 8.0.0 displays/casts the skill/unit float32 formula to an integer.
            // Read the level inside this same transaction; never trust a UI level.
            let price = if price_increment > 0.0 {
                let first_target_level = if kind == ContentKind::Skill { 2 } else { 1 };
                ((price as f32) + (price_increment as f32) *
                    (current_level + 1 - first_target_level).max(0) as f32).trunc() as f64
            } else { price };
            deduct_currency(tx, mutation.identity.usn, currency, price)?;
            tx.execute(
                concat!(
                    "INSERT OR IGNORE INTO owned_content(usn,kind,area,content_id,acquired_at,source) ",
                    "VALUES(?1,?2,?3,?4,?5,'purchase')"
                ),
                params![
                    mutation.identity.usn,
                    key,
                    area,
                    content_id,
                    mutation.committed_at
                ],
            )?;
            let next_level = current_level.saturating_add(1).max(1);
            tx.execute(
                concat!(
                    "INSERT INTO content_levels(usn,kind,content_id,level) VALUES(?1,?2,?3,?4) ",
                    "ON CONFLICT(usn,kind,content_id) DO UPDATE SET level=excluded.level"
                ),
                params![mutation.identity.usn, key, content_id, next_level],
            )?;
            let balances = currency_balances(tx, mutation.identity.usn)?;
            Ok(ContentPurchaseOutcome {
                chocolate: balances.0,
                candy: balances.1,
                kind,
                content_id,
                level: next_level,
            })
        })
    }

    pub fn pass_snapshot(&self, usn: Usn) -> Result<PassSnapshot, StoreError> {
        let anchor = self.connection.query_row(
            "SELECT local_day,season,utc_offset_minutes FROM pass_anchors WHERE usn=?1",
            [usn],
            |row| {
                Ok(PassAnchorSnapshot {
                    anchor_day: row.get(0)?,
                    anchor_season: row.get(1)?,
                    utc_offset_minutes: row.get(2)?,
                })
            },
        )?;
        let mut progress_statement = self.connection.prepare(
            "SELECT season,points,step,version FROM pass_progress WHERE usn=?1 ORDER BY season",
        )?;
        let progress = progress_statement
            .query_map([usn], |row| {
                Ok(PassProgressSnapshot {
                    season: row.get(0)?,
                    points: row.get(1)?,
                    step: row.get(2)?,
                    version: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut claim_statement = self.connection.prepare(concat!(
            "SELECT season,version,lane,step,claimed_at FROM pass_claims WHERE usn=?1 ",
            "ORDER BY season,version,lane,step"
        ))?;
        let claims = claim_statement
            .query_map([usn], |row| {
                Ok(PassClaimSnapshot {
                    season: row.get(0)?,
                    version: row.get(1)?,
                    lane: row.get(2)?,
                    step: row.get(3)?,
                    claimed_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut premium_statement = self.connection.prepare(
            "SELECT season FROM pass_entitlements WHERE usn=?1 AND premium=1 ORDER BY season",
        )?;
        let premium_seasons = premium_statement
            .query_map([usn], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut ticket_statement = self
            .connection
            .prepare("SELECT ticket_id FROM ticket_collection WHERE usn=?1 ORDER BY ticket_id")?;
        let tickets = ticket_statement
            .query_map([usn], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PassSnapshot {
            anchor,
            progress,
            claims,
            premium_seasons,
            tickets,
        })
    }

    pub fn state_revision(&self, usn: Usn, partition: &str) -> Result<i64, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT revision FROM state_revisions WHERE usn=?1 AND partition=?2",
                params![usn, partition],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    pub fn select_pass_season(
        &mut self,
        usn: Usn,
        clock: DeviceClock,
        season: i64,
    ) -> Result<(), StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::select_pass_season_in_transaction(&tx, usn, clock, season)?;
        tx.commit()?;
        Ok(())
    }

    pub fn select_pass_season_rpc(
        &mut self,
        mutation: &RpcMutation,
        clock: DeviceClock,
        season: i64,
    ) -> Result<(), StoreError> {
        self.commit_rpc_mutation(mutation, |tx| {
            Self::select_pass_season_in_transaction(tx, mutation.identity.usn, clock, season)
        })
    }

    fn select_pass_season_in_transaction(
        tx: &rusqlite::Transaction<'_>,
        usn: Usn,
        clock: DeviceClock,
        season: i64,
    ) -> Result<(), StoreError> {
        if !(1..=13).contains(&season) {
            return Err(StoreError::UnsupportedReward(format!(
                "pass-season:{season}"
            )));
        }
        tx.execute(
            concat!(
                "INSERT INTO pass_anchors(usn,local_day,season,utc_offset_minutes,updated_at) ",
                "VALUES(?1,?2,?3,?4,?5) ON CONFLICT(usn) DO UPDATE SET ",
                "local_day=excluded.local_day,season=excluded.season,",
                "utc_offset_minutes=excluded.utc_offset_minutes,updated_at=excluded.updated_at"
            ),
            params![
                usn,
                clock.local_epoch_day(),
                season,
                clock.utc_offset_minutes,
                clock.unix_seconds
            ],
        )?;
        tx.execute(
            concat!(
                "INSERT INTO state_revisions(usn,partition,revision,last_request_seq,updated_at) ",
                "VALUES(?1,'master-data',1,0,?2) ON CONFLICT(usn,partition) DO UPDATE SET ",
                "revision=revision+1,updated_at=excluded.updated_at"
            ),
            params![usn, clock.unix_seconds],
        )?;
        Ok(())
    }

    pub fn fill_pass(
        &mut self,
        mutation: &RpcMutation,
        season: i64,
        version: i32,
        chocolate_cost: f64,
        added_points: i64,
        thresholds: &[(i32, i64)],
    ) -> Result<PassFillOutcome, StoreError> {
        let thresholds = thresholds.to_vec();
        self.commit_rpc_mutation(mutation, |tx| {
            if chocolate_cost > 0.0 {
                deduct_currency(
                    tx,
                    mutation.identity.usn,
                    CurrencyKind::Chocolate,
                    chocolate_cost,
                )?;
            }
            tx.execute(
                concat!(
                    "INSERT INTO pass_progress(usn,season,points,step,version) VALUES(?1,?2,?3,0,?4) ",
                    "ON CONFLICT(usn,season,version) DO UPDATE SET points=points+excluded.points"
                ),
                params![
                    mutation.identity.usn,
                    season,
                    added_points.max(0),
                    version
                ],
            )?;
            let (points, actual_version): (i64, i32) = tx.query_row(
                concat!(
                    "SELECT points,version FROM pass_progress ",
                    "WHERE usn=?1 AND season=?2 AND version=?3"
                ),
                params![mutation.identity.usn, season, version],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let step = thresholds
                .iter()
                .filter(|(_, goal)| points >= *goal)
                .map(|(step, _)| *step)
                .max()
                .unwrap_or(0);
            tx.execute(
                "UPDATE pass_progress SET step=MAX(step,?1) WHERE usn=?2 AND season=?3 AND version=?4",
                params![step, mutation.identity.usn, season, version],
            )?;
            let balances = currency_balances(tx, mutation.identity.usn)?;
            Ok(PassFillOutcome {
                season,
                points,
                version: actual_version,
                chocolate: balances.0,
                candy: balances.1,
            })
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn claim_pass_reward(
        &mut self,
        mutation: &RpcMutation,
        season: i64,
        version: i32,
        lane: i16,
        step: i32,
        required_points: i64,
        rewards: &[RewardGrant],
        paid_lane_step_count: i64,
        ticket_id: i64,
    ) -> Result<PassClaimOutcome, StoreError> {
        let rewards = rewards.to_vec();
        self.commit_reward_mutation(mutation, |tx, rules| {
            let points: i64 = tx
                .query_row(
                    "SELECT points FROM pass_progress WHERE usn=?1 AND season=?2 AND version=?3",
                    params![mutation.identity.usn, season, version],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or(0);
            if points < required_points {
                return Err(DomainError::InsufficientBalance.into());
            }
            if lane == 1 {
                let premium = tx
                    .query_row(
                        "SELECT premium FROM pass_entitlements WHERE usn=?1 AND season=?2",
                        params![mutation.identity.usn, season],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
                    .unwrap_or(0);
                if premium == 0 {
                    return Err(DomainError::InsufficientBalance.into());
                }
            }
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO pass_claims(usn,season,version,lane,step,claimed_at) VALUES(?1,?2,?3,?4,?5,?6)",
                params![mutation.identity.usn, season, version, lane, step, mutation.committed_at],
            )?;
            let mut granted = Vec::new();
            if inserted != 0 {
                for reward in &rewards {
                    if apply_reward_grant(
                        tx, rules,
                        mutation.identity.usn,
                        reward,
                        mutation.committed_at,
                        "pass",
                    )? {
                        granted.push(reward.clone());
                    }
                }
                if lane == 1 && ticket_id > 0 {
                    let count: i64 = tx.query_row(
                        "SELECT COUNT(*) FROM pass_claims WHERE usn=?1 AND season=?2 AND version=?3 AND lane=1",
                        params![mutation.identity.usn, season, version],
                        |row| row.get(0),
                    )?;
                    if count >= paid_lane_step_count {
                        tx.execute(
                            "INSERT OR IGNORE INTO ticket_collection(usn,ticket_id,acquired_at) VALUES(?1,?2,?3)",
                            params![mutation.identity.usn, ticket_id, mutation.committed_at],
                        )?;
                    }
                }
            }
            let initial = PassClaimOutcome {
                season,
                lane,
                step,
                version,
                claimed_at: mutation.committed_at,
                rewards: granted,
            };
            let replay = PassClaimOutcome {
                rewards: Vec::new(),
                ..initial.clone()
            };
            Ok((initial, Some(replay)))
        })
    }

    pub fn activate_passes(
        &mut self,
        mutation: &RpcMutation,
        seasons: &[i64],
    ) -> Result<Vec<i64>, StoreError> {
        let seasons = seasons.to_vec();
        self.commit_rpc_mutation(mutation, |tx| {
            for season in seasons
                .iter()
                .copied()
                .filter(|season| (1..=13).contains(season))
            {
                tx.execute(
                    concat!(
                        "INSERT INTO pass_entitlements(usn,season,premium,lounge) VALUES(?1,?2,1,0) ",
                        "ON CONFLICT(usn,season) DO UPDATE SET premium=1"
                    ),
                    params![mutation.identity.usn, season],
                )?;
            }
            let mut statement = tx.prepare(
                "SELECT season FROM pass_entitlements WHERE usn=?1 AND premium=1 ORDER BY season",
            )?;
            Ok(statement
                .query_map([mutation.identity.usn], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
    }
}

#[derive(Debug)]
struct StoredRewardRow {
    kind: String,
    reward_id: Option<i64>,
    amount: Option<f64>,
    area: i32,
    claimed_at: Option<i64>,
}

fn mail_reward_row(
    tx: &rusqlite::Transaction<'_>,
    usn: Usn,
    post_id: i64,
) -> Result<StoredRewardRow, StoreError> {
    tx.query_row(
        concat!(
            "SELECT r.reward_kind,r.reward_id,r.amount,r.area,m.claimed_at FROM mail_rewards r ",
            "JOIN mail m ON m.usn=r.usn AND m.post_id=r.post_id ",
            "WHERE r.usn=?1 AND r.post_id=?2 AND m.deleted_at IS NULL"
        ),
        params![usn, post_id],
        |row| {
            Ok(StoredRewardRow {
                kind: row.get(0)?,
                reward_id: row.get(1)?,
                amount: row.get(2)?,
                area: row.get(3)?,
                claimed_at: row.get(4)?,
            })
        },
    )
    .optional()?
    .ok_or(StoreError::MissingMail(post_id))
}

fn bump_mail_cursor(tx: &rusqlite::Transaction<'_>, usn: Usn) -> Result<(), StoreError> {
    tx.execute("UPDATE mail_state SET cursor=cursor+1 WHERE usn=?1", [usn])?;
    Ok(())
}

fn queue_reward_mail_tx(
    tx: &rusqlite::Transaction<'_>,
    usn: Usn,
    post_id: i64,
    subject: &str,
    body: &str,
    reward: &RewardGrant,
    now: i64,
) -> Result<bool, StoreError> {
    let inserted=tx.execute(
        "INSERT OR IGNORE INTO mail(usn,post_id,subject_key,body_key,created_at) VALUES(?1,?2,?3,?4,?5)",
        params![usn,post_id,subject,body,now],
    )?;
    if inserted == 0 {
        return Ok(false);
    }
    let (kind, reward_id, area) = if reward.reward_type == 1 {
        let consume = ggfm_domain::ConsumeReward::from_id(reward.reward_id)
            .ok_or_else(|| StoreError::UnsupportedReward(format!("mail-currency:{}", reward.reward_id)))?;
        let currency = consume.currency
            .ok_or_else(|| StoreError::UnsupportedReward(format!("mail-currency:{}", reward.reward_id)))?;
        // Preserve the original Consume ID and channel. In particular 4/7/10
        // are production multipliers and 8/9 use different fan-gain bases.
        (currency.key().to_owned(), Some(i64::from(consume.id)), consume.area)
    } else {
        let kind = match reward.reward_type {
            2 => "music",
            3 => "costume",
            4 => "prop",
            5 => "follower",
            6 => "buff",
            7 => "unit",
            8 => "skill",
            9 => "guitar",
            11 => "gift",
            other => return Err(StoreError::UnsupportedReward(format!("mail-type:{other}"))),
        };
        (kind.to_owned(), Some(i64::from(reward.reward_id)), 0)
    };
    tx.execute("INSERT INTO mail_rewards(usn,post_id,reward_kind,reward_id,amount,area) VALUES(?1,?2,?3,?4,?5,?6)",params![usn,post_id,kind,reward_id,reward.amount,area])?;
    bump_mail_cursor(tx, usn)?;
    Ok(true)
}

fn stored_reward_grant(
    kind: &str,
    reward_id: Option<i64>,
    amount: Option<f64>,
    _area: i32,
) -> Result<RewardGrant, StoreError> {
    let (reward_type, id) = if let Some(currency) = currency_from_key(kind) {
        let legacy_id = match currency {
            CurrencyKind::Chocolate => 1,
            CurrencyKind::Candy => 2,
            CurrencyKind::Ch1Like => 4,
            CurrencyKind::Ch2Note => 5,
            CurrencyKind::Ch2Like => 6,
            CurrencyKind::Fans => 8,
            other => return Err(StoreError::UnsupportedReward(other.key().to_owned())),
        };
        let id = match reward_id {
            Some(id) => {
                let id = i32::try_from(id).map_err(|_| StoreError::UnsupportedReward(format!("consume:{id}")))?;
                let consume = ggfm_domain::ConsumeReward::from_id(id)
                    .filter(|consume| consume.currency == Some(currency))
                    .ok_or_else(|| StoreError::UnsupportedReward(format!("consume:{kind}:{id}")))?;
                consume.id
            }
            // Rows created before the ID-preserving representation have no ID.
            // Keep that old representation readable; do not reinterpret saves.
            None => legacy_id,
        };
        (1, id)
    } else {
        let content =
            content_from_key(kind).ok_or_else(|| StoreError::UnsupportedReward(kind.to_owned()))?;
        let reward_type = match content {
            ggfm_domain::ContentKind::Music => 2,
            ggfm_domain::ContentKind::Costume => 3,
            ggfm_domain::ContentKind::Prop => 4,
            ggfm_domain::ContentKind::Follower => 5,
            ggfm_domain::ContentKind::Buff => 6,
            ggfm_domain::ContentKind::Unit => 7,
            ggfm_domain::ContentKind::Skill => 8,
            ggfm_domain::ContentKind::Guitar => 9,
            ggfm_domain::ContentKind::Gift => 11,
            other => return Err(StoreError::UnsupportedReward(format!("{other:?}"))),
        };
        (
            reward_type,
            reward_id.unwrap_or_default().clamp(0, i64::from(i32::MAX)) as i32,
        )
    };
    Ok(RewardGrant {
        reward_type,
        reward_id: id,
        amount: amount.unwrap_or(1.0),
    })
}

fn apply_stored_reward(
    tx: &rusqlite::Transaction<'_>,
    rules: &ggfm_domain::RewardRules,
    usn: Usn,
    row: &StoredRewardRow,
    now: i64,
    source: &str,
) -> Result<(), StoreError> {
    if let Some(currency) = currency_from_key(&row.kind) {
        if currency == CurrencyKind::Fans {
            let grant = stored_reward_grant(&row.kind, row.reward_id, row.amount, row.area)?;
            apply_reward_grant(tx, rules, usn, &grant, now, source)?;
            return Ok(());
        }
        let amount = row.amount.unwrap_or_default();
        if !amount.is_finite() || amount < 0.0 {
            return Err(DomainError::InvalidAmount.into());
        }
        tx.execute(
            "UPDATE currencies SET amount=amount+?1 WHERE usn=?2 AND kind=?3",
            params![amount, usn, currency.key()],
        )?;
        return Ok(());
    }
    let content = content_from_key(&row.kind)
        .ok_or_else(|| StoreError::UnsupportedReward(row.kind.clone()))?;
    let content_id = row
        .reward_id
        .ok_or_else(|| StoreError::UnsupportedReward(row.kind.clone()))?;
    match content {
        ggfm_domain::ContentKind::Gift => {
            let amount = row.amount.unwrap_or(1.0).round().max(0.0) as i64;
            tx.execute(
                concat!(
                    "INSERT INTO gifts(usn,gift_id,quantity) VALUES(?1,?2,?3) ",
                    "ON CONFLICT(usn,gift_id) DO UPDATE SET quantity=quantity+excluded.quantity"
                ),
                params![usn, content_id, amount],
            )?;
        }
        _ => {
            tx.execute(
                concat!(
                    "INSERT OR IGNORE INTO owned_content(usn,kind,area,content_id,acquired_at,source) ",
                    "VALUES(?1,?2,?3,?4,?5,?6)"
                ),
                params![usn, row.kind, row.area, content_id, now, source],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO content_levels(usn,kind,content_id,level) VALUES(?1,?2,?3,1)",
                params![usn, row.kind, content_id],
            )?;
        }
    }
    Ok(())
}

fn restore_ch3_energy(
    tx: &rusqlite::Transaction<'_>,
    usn: Usn,
    now: i64,
    cap: i32,
) -> Result<Ch3EnergySnapshot, StoreError> {
    let (stored, calculated_at): (i32, i64) = tx.query_row(
        "SELECT amount,calculated_at FROM ch3_energy WHERE usn=?1",
        [usn],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let restored = now.saturating_sub(calculated_at).max(0).min(i64::from(i32::MAX)) as i32;
    let amount = stored.saturating_add(restored).min(cap);
    tx.execute(
        "UPDATE ch3_energy SET amount=?1,cap=?2,calculated_at=?3 WHERE usn=?4",
        params![amount, cap, now, usn],
    )?;
    Ok(Ch3EnergySnapshot {
        amount,
        cap,
        calculated_at: now,
    })
}

fn update_ch3_profile(
    tx: &rusqlite::Transaction<'_>,
    usn: Usn,
    exp_gain: i64,
    levels: &[(i32, i64, i32)],
) -> Result<FollowerProfileSnapshot, StoreError> {
    let (old_exp, old_level): (i64, i32) = tx.query_row(
        "SELECT experience,level FROM affection WHERE usn=?1 AND follower_id=100000",
        [usn],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let experience = old_exp.saturating_add(exp_gain.max(0));
    let mut level = old_level.max(1);
    let mut add_candy = 0;
    for (candidate, threshold, candy) in levels {
        if experience >= *threshold && *candidate >= level {
            level = *candidate;
            add_candy = *candy;
        }
    }
    tx.execute(
        "UPDATE affection SET experience=?1,level=?2 WHERE usn=?3 AND follower_id=100000",
        params![experience, level, usn],
    )?;
    Ok(FollowerProfileSnapshot {
        profile_id: 100_000,
        level,
        experience,
        add_candy,
    })
}

fn update_follower_profile(
    tx: &rusqlite::Transaction<'_>,
    usn: Usn,
    profile_id: i64,
    exp_gain: i64,
    levels: &[(i32, i64, i32)],
) -> Result<FollowerProfileSnapshot, StoreError> {
    tx.execute(
        "INSERT OR IGNORE INTO affection(usn,follower_id,experience,level) VALUES(?1,?2,0,1)",
        params![usn, profile_id],
    )?;
    let (old_exp, old_level): (i64, i32) = tx.query_row(
        "SELECT experience,level FROM affection WHERE usn=?1 AND follower_id=?2",
        params![usn, profile_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let experience = old_exp.saturating_add(exp_gain.max(0));
    let mut level = old_level.max(1);
    let mut add_candy = 0;
    for (candidate, threshold, candy) in levels {
        if experience >= *threshold && *candidate >= level {
            level = *candidate;
            add_candy = *candy;
        }
    }
    tx.execute(
        "UPDATE affection SET experience=?1,level=?2 WHERE usn=?3 AND follower_id=?4",
        params![experience, level, usn, profile_id],
    )?;
    Ok(FollowerProfileSnapshot {
        profile_id,
        level,
        experience,
        add_candy,
    })
}

fn completed_guide_stage(connection: &Connection, usn: Usn) -> Result<i64, StoreError> {
    Ok(connection.query_row(
        "SELECT COALESCE(MAX(stage_id),0) FROM follower_quests WHERE usn=?1 AND chain_id=1 AND completed=1",
        [usn], |row| row.get(0),
    )?)
}

fn quest_snapshot(
    tx: &rusqlite::Transaction<'_>,
    usn: Usn,
    stage_id: i64,
) -> Result<FollowerQuestSnapshot, StoreError> {
    let (completed, infinite) = tx.query_row(
        "SELECT completed,infinite FROM follower_quests WHERE usn=?1 AND chain_id=1 AND stage_id=?2",
        params![usn, stage_id],
        |row| Ok((row.get::<_, i64>(0)? != 0, row.get::<_, i64>(1)? != 0)),
    )?;
    let mut result = FollowerQuestSnapshot {
        current_id: stage_id,
        complete_id: completed_guide_stage(tx, usn)?,
        completed,
        infinite,
        condition_values: [0.0; 3],
        claimed: [false; 3],
    };
    let mut statement = tx.prepare(
        "SELECT condition_id,quantity,claimed FROM quest_conditions WHERE usn=?1 AND chain_id=1 AND stage_id=?2 ORDER BY condition_id",
    )?;
    for row in statement.query_map(params![usn, stage_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, f64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })? {
        let (id, quantity, claimed) = row?;
        if let Some(index) = usize::try_from(id - 1).ok().filter(|index| *index < 3) {
            result.condition_values[index] = quantity;
            result.claimed[index] = claimed != 0;
        }
    }
    Ok(result)
}

fn ad_level_snapshot(
    tx: &rusqlite::Transaction<'_>,
    usn: Usn,
    group_id: i64,
) -> Result<AdLevelSnapshot, StoreError> {
    Ok(tx.query_row(
        "SELECT ad_level_id,level,experience FROM ad_levels WHERE usn=?1 AND ad_level_id=?2",
        params![usn, group_id],
        |row| {
            Ok(AdLevelSnapshot {
                ad_level_id: row.get(0)?,
                level: row.get(1)?,
                experience: row.get(2)?,
            })
        },
    )?)
}

fn advance_ad_level(
    tx: &rusqlite::Transaction<'_>,
    usn: Usn,
    progression: &AdLevelProgression,
) -> Result<AdLevelSnapshot, StoreError> {
    let previous = ad_level_snapshot(tx, usn, progression.group_id)?;
    let experience = previous.experience.saturating_add(1);
    let level = progression
        .thresholds
        .iter()
        .filter(|(_, required)| *required <= experience)
        .map(|(level, _)| *level)
        .max()
        .unwrap_or(previous.level)
        .max(previous.level);
    tx.execute(
        "UPDATE ad_levels SET level=?1,experience=?2 WHERE usn=?3 AND ad_level_id=?4",
        params![level, experience, usn, progression.group_id],
    )?;
    Ok(AdLevelSnapshot {
        ad_level_id: progression.group_id,
        level,
        experience,
    })
}

fn currency_balances(tx: &rusqlite::Transaction<'_>, usn: Usn) -> Result<(f64, f64), StoreError> {
    let chocolate = tx.query_row(
        "SELECT amount FROM currencies WHERE usn=?1 AND kind='chocolate'",
        [usn],
        |row| row.get(0),
    )?;
    let candy = tx.query_row(
        "SELECT amount FROM currencies WHERE usn=?1 AND kind='candy'",
        [usn],
        |row| row.get(0),
    )?;
    Ok((chocolate, candy))
}

fn deduct_currency(
    tx: &rusqlite::Transaction<'_>,
    usn: Usn,
    kind: CurrencyKind,
    amount: f64,
) -> Result<(), StoreError> {
    if !amount.is_finite() || amount < 0.0 {
        return Err(DomainError::InvalidAmount.into());
    }
    if amount == 0.0 {
        return Ok(());
    }
    let changed = tx.execute(
        "UPDATE currencies SET amount=amount-?1 WHERE usn=?2 AND kind=?3 AND amount>=?1",
        params![amount, usn, kind.key()],
    )?;
    if changed == 0 {
        return Err(DomainError::InsufficientBalance.into());
    }
    Ok(())
}

fn grant_fan_reward(
    tx: &rusqlite::Transaction<'_>,
    rules: &ggfm_domain::RewardRules,
    usn: Usn,
    area: i32,
    multiplier: f64,
) -> Result<bool, StoreError> {
    let mut statement = tx.prepare(
        "SELECT content_id FROM owned_content WHERE usn=?1 AND kind='costume' ORDER BY content_id",
    )?;
    let mut bonuses = Vec::new();
    for id in statement.query_map([usn], |row| row.get::<_, i64>(0))? {
        let id = id?;
        let (costume_area, bonus) = rules.costume_fan_bonuses.get(&id)
            .ok_or_else(|| StoreError::UnsupportedReward(format!("missing costume bonus:{id}")))?;
        if *costume_area == area { bonuses.push(*bonus); }
    }
    let amount = f64::from(ggfm_domain::fan_reward_count(bonuses, multiplier)?);
    if area == 1 {
        tx.execute("UPDATE currencies SET amount=amount+?1 WHERE usn=?2 AND kind='fans'", params![amount, usn])?;
    }
    // CH2 viewers are not CH1 fans. Keep the explicit Area_data channel too.
    let changed = tx.execute("UPDATE area_state SET fans=fans+?1 WHERE usn=?2 AND area=?3", params![amount, usn, area])?;
    if changed != 1 { return Err(StoreError::UnsupportedReward(format!("fan area:{area}"))); }
    Ok(true)
}

fn apply_reward_grant(
    tx: &rusqlite::Transaction<'_>,
    rules: &ggfm_domain::RewardRules,
    usn: Usn,
    reward: &RewardGrant,
    now: i64,
    source: &str,
) -> Result<bool, StoreError> {
    if !reward.amount.is_finite() || reward.amount < 0.0 {
        return Err(DomainError::InvalidAmount.into());
    }
    if reward.reward_type == 1 {
        if matches!(reward.reward_id, 8 | 9) {
            return grant_fan_reward(tx, rules, usn, reward.reward_id - 7, reward.amount);
        }
        let currency = match reward.reward_id {
            1 => CurrencyKind::Chocolate,
            2 => CurrencyKind::Candy,
            3 | 4 => CurrencyKind::Ch1Like,
            5 | 10 => CurrencyKind::Ch2Note,
            6 | 7 => CurrencyKind::Ch2Like,
            11 => {
                let amount = reward.amount.round().max(0.0) as i64;
                tx.execute(
                    concat!(
                        "INSERT INTO ch3_energy(usn,amount,cap,calculated_at) VALUES(?1,?2,?2,?3) ",
                        "ON CONFLICT(usn) DO UPDATE SET amount=MIN(ch3_energy.cap,ch3_energy.amount+excluded.amount),calculated_at=?3"
                    ),
                    params![usn, amount, now],
                )?;
                return Ok(true);
            }
            other => return Err(StoreError::UnsupportedReward(format!("consume:{other}"))),
        };
        tx.execute(
            "UPDATE currencies SET amount=amount+?1 WHERE usn=?2 AND kind=?3",
            params![reward.amount, usn, currency.key()],
        )?;
        return Ok(true);
    }
    if reward.reward_type == 6 && reward.reward_id == 2 {
        // RewardGroup quantities for Fever are seconds, not stack counts.
        // A second independent reward extends the absolute deadline; request
        // and claim ledgers above this helper prevent retry extensions.
        let old_end: Option<i64> = tx.query_row(
            "SELECT expires_at FROM buff_timers WHERE usn=?1 AND buff_id=2",
            [usn], |row| row.get(0),
        ).optional()?;
        let duration = (reward.amount.round() as i64).max(1);
        let expires_at = old_end.unwrap_or(now).max(now).saturating_add(duration);
        tx.execute(
            "INSERT INTO buff_timers(usn,buff_id,active_at,expires_at) VALUES(?1,2,?2,?3) ON CONFLICT(usn,buff_id) DO UPDATE SET active_at=excluded.active_at,expires_at=excluded.expires_at",
            params![usn,now,expires_at],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO owned_content(usn,kind,area,content_id,acquired_at,source) VALUES(?1,'buff',0,2,?2,?3)",
            params![usn,now,source],
        )?;
        tx.execute("INSERT OR IGNORE INTO content_levels(usn,kind,content_id,level) VALUES(?1,'buff',2,1)", [usn])?;
        return Ok(true);
    }
    let kind = match reward.reward_type {
        2 => ContentKind::Music,
        3 => ContentKind::Costume,
        4 => ContentKind::Prop,
        5 => ContentKind::Follower,
        6 => ContentKind::Buff,
        7 => ContentKind::Unit,
        8 => ContentKind::Skill,
        9 => ContentKind::Guitar,
        10 => {
            let changed = tx.execute(
                "INSERT OR IGNORE INTO ticket_collection(usn,ticket_id,acquired_at) VALUES(?1,?2,?3)",
                params![usn, reward.reward_id, now],
            )?;
            return Ok(changed != 0);
        }
        11 => {
            let amount = reward.amount.round().max(0.0) as i64;
            tx.execute(
                concat!(
                    "INSERT INTO gifts(usn,gift_id,quantity) VALUES(?1,?2,?3) ",
                    "ON CONFLICT(usn,gift_id) DO UPDATE SET quantity=quantity+excluded.quantity"
                ),
                params![usn, reward.reward_id, amount],
            )?;
            return Ok(true);
        }
        other => return Err(StoreError::UnsupportedReward(format!("type:{other}"))),
    };
    let key = content_key(kind);
    let changed = tx.execute(
        concat!(
            "INSERT OR IGNORE INTO owned_content(usn,kind,area,content_id,acquired_at,source) ",
            "VALUES(?1,?2,0,?3,?4,?5)"
        ),
        params![usn, key, reward.reward_id, now, source],
    )?;
    if changed != 0 {
        tx.execute(
            "INSERT OR IGNORE INTO content_levels(usn,kind,content_id,level) VALUES(?1,?2,?3,1)",
            params![usn, key, reward.reward_id],
        )?;
    }
    Ok(changed != 0)
}

fn store_to_sql(error: StoreError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn currency_from_key(key: &str) -> Option<CurrencyKind> {
    CurrencyKind::ALL.into_iter().find(|kind| kind.key() == key)
}
fn content_from_key(key: &str) -> Option<ggfm_domain::ContentKind> {
    use ggfm_domain::ContentKind::*;
    Some(match key {
        "character" => Character,
        "costume" => Costume,
        "guitar" => Guitar,
        "music" => Music,
        "skill" => Skill,
        "unit" => Unit,
        "follower" => Follower,
        "prop" => Prop,
        "buff" => Buff,
        "gift" => Gift,
        _ => return None,
    })
}

fn content_key(kind: ggfm_domain::ContentKind) -> &'static str {
    use ggfm_domain::ContentKind::*;
    match kind {
        Character => "character",
        Costume => "costume",
        Guitar => "guitar",
        Music => "music",
        Skill => "skill",
        Unit => "unit",
        Follower => "follower",
        Prop => "prop",
        Buff => "buff",
        Gift => "gift",
    }
}

fn to_i32_saturated(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    include!("activity_restart_tests.rs");

    fn setup() -> (Database, UserIdentity) {
        let mut db = Database::open_memory().unwrap();
        db.configure_reward_rules(ggfm_domain::RewardRules {
            costume_fan_bonuses: [(1, (1, 0.0)), (201, (2, 0.0))].into_iter().collect(),
        }).unwrap();
        let identity = db.create_slot("One", 100).unwrap();
        (db, identity)
    }

    #[test]
    fn fresh_slot_keeps_ch2_locked_with_ch2_catalog_defaults() {
        let (mut db, identity) = setup();
        db.begin_login(b"fresh-area", DeviceClock::new(101, 0).unwrap())
            .unwrap();
        let snapshot = db.player_snapshot(identity.usn).unwrap();
        let area2 = snapshot.areas.iter().find(|area| area.area == 2).unwrap();
        assert!(!area2.unlocked);
        assert_eq!(area2.current_costume, Some(201));
        assert_eq!(area2.current_guitar, Some(201));
        assert_eq!(area2.current_music, Some(201));
        for kind in [
            ContentKind::Character,
            ContentKind::Costume,
            ContentKind::Guitar,
            ContentKind::Music,
        ] {
            let level = snapshot
                .content_levels
                .iter()
                .find(|level| level.kind == kind && level.content_id == 1)
                .unwrap();
            assert_eq!(level.level, 1);
            assert_eq!(level.reward_level, 0);
        }
    }

    #[test]
    fn user_save_area_snapshot_does_not_grant_area_unlock() {
        let (mut db, identity) = setup();
        let session = db
            .begin_login(b"area-lock", DeviceClock::new(101, 0).unwrap())
            .unwrap();
        let mutation = RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: 1,
            rpc: "userSave".into(),
            idempotency_key: "area-lock:1".into(),
            committed_at: 102,
        };
        db.merge_user_save(
            &mutation,
            &UserSavePatch {
                areas: vec![ggfm_domain::AreaSavePatch {
                    area: 2,
                    like_amount: Some(10.0),
                    current_costume: Some(201),
                    current_guitar: Some(201),
                    current_music: Some(201),
                    ..Default::default()
                }],
                content: vec![
                    ggfm_domain::ContentSavePatch {
                        kind: ContentKind::Music,
                        area: 2,
                        content_id: 201,
                        level: Some(0),
                        reward_level: Some(0),
                    },
                    ggfm_domain::ContentSavePatch {
                        kind: ContentKind::Costume,
                        area: 2,
                        content_id: 201,
                        level: None,
                        reward_level: None,
                    },
                    ggfm_domain::ContentSavePatch {
                        kind: ContentKind::Guitar,
                        area: 2,
                        content_id: 201,
                        level: Some(-1),
                        reward_level: None,
                    },
                ],
                music_states: vec![ggfm_domain::MusicStateSavePatch {
                    music_id: 201,
                    encore_appears: false,
                    encore_follower_id: None,
                }],
                ..Default::default()
            },
            "device-a",
            10,
            &[],
        )
        .unwrap();
        let snapshot = db.player_snapshot(identity.usn).unwrap();
        assert!(
            !snapshot
                .areas
                .iter()
                .find(|area| area.area == 2)
                .unwrap()
                .unlocked
        );
        for kind in [
            ContentKind::Costume,
            ContentKind::Guitar,
            ContentKind::Music,
        ] {
            assert!(!snapshot.owned_content.iter().any(|content| {
                content.kind == kind && content.area == 2 && content.content_id == 201
            }));
        }
        assert!(!snapshot.music_states.iter().any(|row| row.music_id == 201));
    }

    #[test]
    fn content_identity_cannot_be_duplicated_across_areas() {
        let (db, identity) = setup();
        db.connection
            .execute(
                "INSERT INTO owned_content(usn,kind,area,content_id,acquired_at,source) VALUES(?1,'music',1,2,101,'test')",
                [identity.usn],
            )
            .unwrap();
        assert!(
            db.connection
                .execute(
                    "INSERT INTO owned_content(usn,kind,area,content_id,acquired_at,source) VALUES(?1,'music',2,2,102,'test')",
                    [identity.usn],
                )
                .is_err()
        );
    }

    #[test]
    fn old_development_database_is_rejected_instead_of_partially_opened() {
        let connection = Connection::open_in_memory().unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        assert!(matches!(
            Database::configure(connection),
            Err(StoreError::UnsupportedSchema {
                actual: 1,
                expected: DATABASE_SCHEMA_VERSION
            })
        ));
    }

    #[test]
    fn backup_is_self_contained_and_reopenable() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let backup = std::env::temp_dir().join(format!("ggfm-backup-{suffix}.sqlite"));
        let (mut db, identity) = setup();
        db.grant_currency(
            identity.usn,
            CurrencyKind::Chocolate,
            17.0,
            "backup-seed",
            101,
        )
        .unwrap();
        db.backup_to(&backup).unwrap();
        let restored = Database::open(&backup).unwrap();
        assert_eq!(
            restored
                .currency(identity.usn, CurrencyKind::Chocolate)
                .unwrap(),
            17.0
        );
        drop(restored);
        std::fs::remove_file(backup).unwrap();
    }

    #[test]
    fn checkpoint_does_not_end_the_active_session() {
        let (mut db, identity) = setup();
        let session = db
            .begin_login(b"checkpoint-session", DeviceClock::new(101, 0).unwrap())
            .unwrap();
        db.checkpoint().unwrap();
        let mutation = RpcMutation {
            identity,
            nonce: session.nonce,
            request_seq: 1,
            rpc: "checkpointProbe".into(),
            idempotency_key: "checkpoint:1".into(),
            committed_at: 102,
        };
        db.commit_rpc_mutation(&mutation, |_transaction| Ok(()))
            .unwrap();
    }

    #[test]
    fn slots_and_progress_are_isolated() {
        let (mut db, first) = setup();
        let second = db.create_slot("Two", 101).unwrap();
        db.grant_currency(first.usn, CurrencyKind::Chocolate, 7.0, "one", 102)
            .unwrap();
        assert_eq!(
            db.currency(first.usn, CurrencyKind::Chocolate).unwrap(),
            7.0
        );
        assert_eq!(
            db.currency(second.usn, CurrencyKind::Chocolate).unwrap(),
            0.0
        );
    }

    #[test]
    fn every_player_state_table_is_usn_scoped_and_foreign_keyed() {
        let (db, _) = setup();
        let global_tables = ["schema_migrations", "save_slots", "runtime_selection"];
        let mut statement = db
            .connection
            .prepare(
                "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap();
        let tables: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        drop(statement);
        for table in tables
            .iter()
            .filter(|table| !global_tables.contains(&table.as_str()))
        {
            let columns: Vec<String> = db
                .connection
                .prepare(&format!("PRAGMA table_info([{table}])"))
                .unwrap()
                .query_map([], |row| row.get(1))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert!(
                columns.iter().any(|column| column == "usn"),
                "{table} has player state without a USN column"
            );
            let parents: Vec<String> = db
                .connection
                .prepare(&format!("PRAGMA foreign_key_list([{table}])"))
                .unwrap()
                .query_map([], |row| row.get(2))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert!(
                parents.iter().any(|parent| parent == "save_slots"),
                "{table}.usn is not anchored to save_slots"
            );
        }
    }

    #[test]
    fn mail_preserves_each_consume_id_and_channel_without_coalescing() {
        let (mut db, user) = setup();
        let other = db.create_slot("Other", 101).unwrap();
        let tx = db.connection.transaction().unwrap();
        for id in 1..=10 {
            let grant = RewardGrant { reward_type: 1, reward_id: id, amount: 2.5 };
            assert!(queue_reward_mail_tx(&tx, user.usn, i64::from(id), "s", "b", &grant, 102).unwrap());
            assert!(!queue_reward_mail_tx(&tx, user.usn, i64::from(id), "s", "b", &grant, 102).unwrap());
        }
        tx.commit().unwrap();
        let mailbox = db.mailbox(user.usn).unwrap();
        assert_eq!(mailbox.mail.len(), 10);
        assert_eq!(mailbox.cursor, 10);
        for mail in mailbox.mail {
            assert_eq!(mail.reward.reward_id, mail.post_id as i32);
            assert_eq!(mail.reward.amount, 2.5);
            let area: i32 = db.connection.query_row(
                "SELECT area FROM mail_rewards WHERE usn=?1 AND post_id=?2",
                params![user.usn, mail.post_id], |r| r.get(0),
            ).unwrap();
            assert_eq!(area, ggfm_domain::ConsumeReward::from_id(mail.reward.reward_id).unwrap().area);
        }
        assert!(db.mailbox(other.usn).unwrap().mail.is_empty());
        assert!(stored_reward_grant("ch1_like", Some(7), Some(2.5), 1).is_err());
        assert!(stored_reward_grant("fans", Some(i64::MAX), Some(2.5), 1).is_err());
    }

    #[test]
    fn attendance_claim_uses_distinct_dates_even_with_inflated_saved_progress() {
        let (mut db, user) = setup();
        db.connection.execute(
            "UPDATE achievement_progress SET quantity=999 WHERE usn=?1 AND achievement_id=10",
            [user.usn],
        ).unwrap();
        let command = || GameRewardClaimCommand {
            kind: "achievement".into(), id: 10, level: 1,
            quantity: 999.0, threshold: 3.0, local_day: 0,
            reward: Some(RewardGrant { reward_type: 1, reward_id: 1, amount: 5.0 }),
        };
        for (index, now) in [101, 102, 86_501, 86_502, 172_901].into_iter().enumerate() {
            let session = db.begin_login(format!("attendance:{index}").as_bytes(), DeviceClock::new(now, 0).unwrap()).unwrap();
            let mutation = RpcMutation {
                nonce: session.nonce, identity: user.clone(), request_seq: 2,
                rpc: "setGameReward".into(), idempotency_key: format!("attendance:{index}"),
                committed_at: now,
            };
            let add = RpcMutation {
                request_seq: 1, rpc: "setAttendance".into(),
                idempotency_key: format!("attendance-add:{index}"), ..mutation.clone()
            };
            db.attendance(&add, DeviceClock::new(now, 0).unwrap(), "add").unwrap();
            if index < 4 {
                assert!(matches!(db.claim_game_reward(&mutation, command()), Err(StoreError::Domain(DomainError::Locked))));
                assert_eq!(db.currency(user.usn, CurrencyKind::Chocolate).unwrap(), 0.0);
            } else {
                let (next, rewards) = db.claim_game_reward(&mutation, command()).unwrap();
                assert_eq!(next, 2);
                assert_eq!(rewards.len(), 1);
                assert!(db.claim_game_reward(&mutation, command()).unwrap().1.is_empty());
                assert_eq!(db.currency(user.usn, CurrencyKind::Chocolate).unwrap(), 5.0);
            }
        }
    }

    #[test]
    fn reward_transactions_and_mail_are_idempotent() {
        let (mut db, user) = setup();
        assert!(
            db.grant_currency(user.usn, CurrencyKind::Candy, 2.0, "tx", 101)
                .unwrap()
        );
        assert!(
            !db.grant_currency(user.usn, CurrencyKind::Candy, 2.0, "tx", 101)
                .unwrap()
        );
        let reward = Reward::Currency {
            currency: CurrencyKind::Chocolate,
            amount: 4.0,
        };
        assert!(db.queue_mail(user.usn, 10, "s", "b", &reward, 102).unwrap());
        assert!(!db.queue_mail(user.usn, 10, "s", "b", &reward, 102).unwrap());
        assert_eq!(db.mailbox(user.usn).unwrap().cursor, 1);
        let session = db
            .begin_login(b"mail-capability", DeviceClock::new(103, 0).unwrap())
            .unwrap();
        let mutation = RpcMutation {
            nonce: session.nonce,
            identity: user.clone(),
            request_seq: 1,
            rpc: "providePost".into(),
            idempotency_key: "mail-claim-10".into(),
            committed_at: 103,
        };
        let grant = db.claim_mail_rpc(&mutation, 10).unwrap().unwrap();
        assert_eq!(grant.reward_type, 1);
        assert_eq!(grant.reward_id, 1);
        assert_eq!(grant.amount, 4.0);
        assert_eq!(db.currency(user.usn, CurrencyKind::Chocolate).unwrap(), 4.0);
        assert_eq!(db.claim_mail_rpc(&mutation, 10).unwrap(), None);
        assert_eq!(db.currency(user.usn, CurrencyKind::Chocolate).unwrap(), 4.0);
        let mailbox = db.mailbox(user.usn).unwrap();
        assert_eq!(mailbox.cursor, 2);
        assert!(mailbox.mail[0].claimed);

        let fan_reward = Reward::Currency {
            currency: CurrencyKind::Fans,
            amount: 1_000.0,
        };
        assert!(
            db.queue_mail(user.usn, 11, "fans", "fans", &fan_reward, 104)
                .unwrap()
        );
        let fan_claim = RpcMutation {
            request_seq: 2,
            idempotency_key: "mail-claim-11".into(),
            committed_at: 105,
            ..mutation
        };
        let grant = db.claim_mail_rpc(&fan_claim, 11).unwrap().unwrap();
        assert_eq!(grant.reward_id, 8);
        assert_eq!(grant.amount, 1_000.0);
        assert_eq!(db.currency(user.usn, CurrencyKind::Fans).unwrap(), 1_000.0);
    }

    #[test]
    fn delivery_state_and_duplicate_guard_share_usn_and_unscoped_ownership_rules() {
        let (mut db, first) = setup();
        let other = db.create_slot("Other", 101).unwrap();
        let item = Reward::Content { content: ContentKind::Guitar, area: 1, content_id: 99 };
        assert_eq!(db.content_delivery_state(first.usn, ContentKind::Guitar, 1, 99).unwrap(), (false, false));
        db.queue_generated_mail(first.usn, "subject", "body", &item, 102).unwrap();
        assert_eq!(db.content_delivery_state(first.usn, ContentKind::Guitar, 1, 99).unwrap(), (false, true));
        assert_eq!(db.content_delivery_state(other.usn, ContentKind::Guitar, 1, 99).unwrap(), (false, false));
        db.connection.execute(
            "INSERT INTO owned_content(usn,kind,area,content_id,acquired_at,source) VALUES(?1,'guitar',0,99,103,'reward')",
            [other.usn],
        ).unwrap();
        assert_eq!(db.content_delivery_state(other.usn, ContentKind::Guitar, 1, 99).unwrap(), (true, false));
        assert!(matches!(db.queue_generated_mail(other.usn, "subject", "body", &item, 104),
            Err(StoreError::Domain(DomainError::AlreadyOwned))));
        assert!(db.mailbox(other.usn).unwrap().mail.is_empty());
        assert_eq!(db.mailbox(first.usn).unwrap().mail.len(), 1);
    }

    #[test]
    fn generated_unique_mail_is_ledgered_and_rejects_pending_duplicates() {
        let (mut db, identity) = setup();
        let session = db
            .begin_login(b"admin-mail", DeviceClock::new(200, 0).unwrap())
            .unwrap();
        let mutation = RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: 1,
            rpc: "memorial.sendLegacyItem".into(),
            idempotency_key: "admin:mail:1".into(),
            committed_at: 201,
        };
        let reward = Reward::Content {
            content: ContentKind::Costume,
            area: 1,
            content_id: 99,
        };
        let first = db
            .queue_generated_mail_rpc(&mutation, "subject", "body", &reward)
            .unwrap();
        let replay = db
            .queue_generated_mail_rpc(&mutation, "subject", "body", &reward)
            .unwrap();
        assert_eq!(replay, first);
        assert_eq!(db.mailbox(identity.usn).unwrap().mail.len(), 1);

        let duplicate = RpcMutation {
            request_seq: 2,
            idempotency_key: "admin:mail:2".into(),
            committed_at: 202,
            ..mutation.clone()
        };
        assert!(matches!(
            db.queue_generated_mail_rpc(&duplicate, "subject", "body", &reward),
            Err(StoreError::Domain(DomainError::AlreadyOwned))
        ));

        let stale = RpcMutation {
            request_seq: 1,
            idempotency_key: "admin:mail:stale".into(),
            committed_at: 203,
            ..mutation
        };
        assert!(matches!(
            db.queue_generated_mail_rpc(
                &stale,
                "currency",
                "body",
                &Reward::Currency {
                    currency: CurrencyKind::Candy,
                    amount: 1.0,
                },
            ),
            Err(StoreError::Domain(DomainError::StaleRequest))
        ));
    }

    #[test]
    fn slot_and_pass_admin_commands_replay_without_reapplying() {
        let (mut db, identity) = setup();
        let clock = DeviceClock::new(86_400 * 20, 120).unwrap();
        let session = db.begin_login(b"admin-slots", clock).unwrap();
        let create = RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: 1,
            rpc: "memorial.createSlot".into(),
            idempotency_key: "admin:create:1".into(),
            committed_at: clock.unix_seconds,
        };
        let second = db.create_slot_rpc(&create, "Two").unwrap();
        assert_eq!(db.create_slot_rpc(&create, "Two").unwrap(), second);
        assert_eq!(db.list_slots().unwrap().len(), 2);

        let select = RpcMutation {
            request_seq: 2,
            rpc: "memorial.selectPass".into(),
            idempotency_key: "admin:pass:2".into(),
            committed_at: clock.unix_seconds + 1,
            ..create
        };
        db.select_pass_season_rpc(&select, clock, 9).unwrap();
        db.select_pass_season_rpc(&select, clock, 9).unwrap();
        assert_eq!(
            db.pass_snapshot(identity.usn).unwrap().anchor.anchor_season,
            9
        );
    }

    #[test]
    fn pending_slot_activates_only_at_next_login() {
        let (mut db, first) = setup();
        let second = db.create_slot("Two", 101).unwrap();
        let clock = DeviceClock::new(200, 0).unwrap();
        let a = db.begin_login(b"a", clock).unwrap();
        assert_eq!(a.identity, first);
        db.request_switch(second.usn).unwrap();
        assert_eq!(a.identity, first);
        let b = db.begin_login(b"b", clock).unwrap();
        assert_eq!(b.identity, second);
    }

    #[test]
    fn cold_slot_switch_invalidates_old_session_and_never_cross_writes() {
        let (mut db, first) = setup();
        let second = db.create_slot("Two", 101).unwrap();
        let clock = DeviceClock::new(200, 0).unwrap();
        let first_session = db.begin_login(b"slot-a", clock).unwrap();
        let first_write = RpcMutation {
            nonce: first_session.nonce,
            identity: first.clone(),
            request_seq: 1,
            rpc: "updateNickName".into(),
            idempotency_key: "slot-a:1".into(),
            committed_at: 201,
        };
        db.update_nickname(&first_write, "First").unwrap();

        db.request_switch(second.usn).unwrap();
        let final_old_save = RpcMutation {
            request_seq: 2,
            rpc: "setUserGP".into(),
            idempotency_key: "slot-a:2".into(),
            committed_at: 202,
            ..first_write.clone()
        };
        assert_eq!(db.update_user_gp(&final_old_save, 25).unwrap(), 25);

        let second_session = db
            .begin_login(b"slot-b", DeviceClock::new(203, 0).unwrap())
            .unwrap();
        assert_eq!(second_session.identity, second);
        let stale_old_write = RpcMutation {
            request_seq: 3,
            rpc: "updateUserTitle".into(),
            idempotency_key: "slot-a:3".into(),
            committed_at: 204,
            ..first_write.clone()
        };
        assert!(matches!(
            db.update_user_title(&stale_old_write, 9),
            Err(StoreError::Domain(DomainError::StaleRequest))
        ));
        assert!(matches!(
            db.update_nickname(&first_write, "First"),
            Err(StoreError::Domain(DomainError::StaleRequest))
        ));

        let second_write = RpcMutation {
            nonce: second_session.nonce,
            identity: second.clone(),
            request_seq: 1,
            rpc: "updateNickName".into(),
            idempotency_key: "slot-b:1".into(),
            committed_at: 205,
        };
        db.update_nickname(&second_write, "Second").unwrap();
        assert_eq!(db.player_snapshot(first.usn).unwrap().nickname, "First");
        assert_eq!(db.player_snapshot(first.usn).unwrap().areas[0].gp, 25.0);
        assert_eq!(db.player_snapshot(second.usn).unwrap().nickname, "Second");
        assert_eq!(db.player_snapshot(second.usn).unwrap().areas[0].gp, 0.0);

        db.delete_inactive_slot(first.usn, "One").unwrap();
        assert!(matches!(
            db.resolve_slot(&first.user_id),
            Err(StoreError::MissingSlot(_))
        ));
        assert_eq!(db.resolve_slot(&second.user_id).unwrap(), second);
    }

    #[test]
    fn recovered_baseline_writes_survive_restart_and_remain_usn_isolated() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let backup = std::env::temp_dir().join(format!("ggfm-baseline-writes-{suffix}.sqlite"));
        let (mut db, first) = setup();
        let second = db.create_slot("Two", 101).unwrap();
        db.select_pass_season(second.usn, DeviceClock::new(200, 0).unwrap(), 1)
            .unwrap();
        let session = db
            .begin_login(b"baseline-writes", DeviceClock::new(200, 0).unwrap())
            .unwrap();
        let make = |seq, rpc: &str| RpcMutation {
            nonce: session.nonce,
            identity: first.clone(),
            request_seq: seq,
            rpc: rpc.into(),
            idempotency_key: format!("{rpc}:{seq}"),
            committed_at: 200 + seq,
        };

        assert_eq!(
            db.update_nickname(&make(1, "updateNickName"), "Memorial")
                .unwrap(),
            "Memorial"
        );
        assert_eq!(
            db.update_user_title(&make(2, "updateUserTitle"), 7)
                .unwrap(),
            7
        );
        assert_eq!(db.update_user_gp(&make(3, "setUserGP"), 90).unwrap(), 90);
        assert_eq!(db.update_user_gp(&make(4, "setUserGP"), 12).unwrap(), 90);
        assert_eq!(
            db.merge_tutorials(&make(5, "setTutorial"), &[1, 2, 3])
                .unwrap(),
            vec![1, 2, 3]
        );
        assert!(
            db.set_follower_quest_infinite(&make(6, "setFollowerQuestInfinite"))
                .unwrap()
                .infinite
        );
        db.grant_package_contents(
            &make(7, "buyPackage"),
            &[(ContentKind::Guitar, 1, 6), (ContentKind::Costume, 1, 10)],
            &[],
        )
        .unwrap();

        let before_restart = db.player_snapshot(first.usn).unwrap();
        assert_eq!(before_restart.nickname, "Memorial");
        assert_eq!(before_restart.title_id, Some(7));
        assert_eq!(before_restart.areas[0].gp, 90.0);
        assert_eq!(before_restart.tutorials, vec![1, 2, 3]);
        assert!(before_restart.follower_quests[0].infinite);
        assert!(
            before_restart
                .owned_content
                .iter()
                .any(|row| { row.kind == ContentKind::Guitar && row.content_id == 6 })
        );

        let untouched = db.player_snapshot(second.usn).unwrap();
        assert_eq!(untouched.nickname, "Guitar Girl");
        assert_eq!(untouched.title_id, Some(1));
        assert_eq!(untouched.areas[0].gp, 0.0);
        assert!(untouched.tutorials.is_empty());
        assert!(untouched.follower_quests.is_empty());
        assert!(
            !untouched
                .owned_content
                .iter()
                .any(|row| { row.kind == ContentKind::Guitar && row.content_id == 6 })
        );

        db.backup_to(&backup).unwrap();
        let restored = Database::open(&backup).unwrap();
        assert_eq!(restored.player_snapshot(first.usn).unwrap(), before_restart);
        assert_eq!(restored.player_snapshot(second.usn).unwrap(), untouched);
        drop(restored);
        std::fs::remove_file(backup).unwrap();
    }

    #[test]
    fn follower_guide_stages_keep_each_task_state_independently() {
        let (mut db, identity) = setup();
        let session = db
            .begin_login(b"guide-stages", DeviceClock::new(300, 0).unwrap())
            .unwrap();
        let make = |seq| RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: seq,
            rpc: "setUserFollowerQuest".into(),
            idempotency_key: format!("guide:{seq}"),
            committed_at: 300 + seq,
        };
        for sub_id in 1..=3 {
            let result = db
                .advance_follower_quest(
                    &make(sub_id),
                    1,
                    sub_id,
                    f64::from(sub_id as i32),
                    [1.0, 2.0, 3.0],
                    Vec::new(),
                    Some(2),
                )
                .unwrap();
            assert_eq!(result.next, sub_id == 3);
        }

        let snapshot = db.player_snapshot(identity.usn).unwrap();
        assert_eq!(snapshot.follower_quests.len(), 2);
        let first = snapshot
            .follower_quests
            .iter()
            .find(|quest| quest.current_id == 1)
            .unwrap();
        assert!(first.completed);
        assert_eq!(first.condition_values, [1.0, 2.0, 3.0]);
        assert_eq!(first.claimed, [true, true, true]);
        let second = snapshot
            .follower_quests
            .iter()
            .find(|quest| quest.current_id == 2)
            .unwrap();
        assert!(!second.completed);
        assert_eq!(second.condition_values, [0.0; 3]);
        assert_eq!(second.claimed, [false; 3]);
    }

    #[test]
    fn user_save_and_guide_rpc_share_the_same_quest_chain() {
        let (mut db, identity) = setup();
        let session = db
            .begin_login(b"guide-merge", DeviceClock::new(400, 0).unwrap())
            .unwrap();
        let save = RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: 1,
            rpc: "userSave".into(),
            idempotency_key: "guide-save".into(),
            committed_at: 401,
        };
        db.merge_user_save(
            &save,
            &UserSavePatch {
                follower_quests: vec![ggfm_domain::FollowerQuestSavePatch {
                    current_id: 9,
                    condition_values: [99.0, 99.0, 99.0],
                }],
                ..Default::default()
            },
            "device-a",
            10,
            &[],
        )
        .unwrap();
        let rpc = RpcMutation {
            request_seq: 2,
            rpc: "setUserFollowerQuest".into(),
            idempotency_key: "guide-rpc".into(),
            committed_at: 402,
            ..save
        };
        db.advance_follower_quest(&rpc, 1, 2, 2.0, [1.0, 2.0, 0.0], Vec::new(), Some(2))
            .unwrap();

        let chain_count: i64 = db
            .connection
            .query_row(
                "SELECT COUNT(DISTINCT chain_id) FROM follower_quests WHERE usn=?1",
                [identity.usn],
                |row| row.get(0),
            )
            .unwrap();
        let chain_id: i64 = db
            .connection
            .query_row(
                "SELECT chain_id FROM follower_quests WHERE usn=?1 AND stage_id=1",
                [identity.usn],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(chain_count, 1);
        assert_eq!(chain_id, 1);
        let snapshot = db.player_snapshot(identity.usn).unwrap();
        assert!(
            snapshot
                .follower_quests
                .iter()
                .all(|quest| quest.current_id != 9),
            "the asynchronous client snapshot must not select the authoritative guide stage"
        );
        assert_eq!(
            snapshot.follower_quests[0].condition_values,
            [0.0, 2.0, 0.0]
        );
    }

    #[test]
    fn rpc_sequence_domain_write_and_ledger_are_one_transaction() {
        let (mut db, identity) = setup();
        let session = db
            .begin_login(b"capability", DeviceClock::new(200, 0).unwrap())
            .unwrap();
        let mutation = RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: 1,
            rpc: "testMutation".into(),
            idempotency_key: "same-request".into(),
            committed_at: 201,
        };
        let first: serde_json::Value = db
            .commit_rpc_mutation(&mutation, |tx| {
                tx.execute(
                    "UPDATE currencies SET amount=amount+5 WHERE usn=?1 AND kind='candy'",
                    [identity.usn],
                )?;
                Ok(serde_json::json!({"amount": 5}))
            })
            .unwrap();
        assert_eq!(first, serde_json::json!({"amount": 5}));
        let replay: serde_json::Value = db
            .commit_rpc_mutation(&mutation, |_tx| {
                panic!("a replay must not execute the domain command")
            })
            .unwrap();
        assert_eq!(replay, first);
        assert_eq!(db.currency(identity.usn, CurrencyKind::Candy).unwrap(), 5.0);

        let stale = RpcMutation {
            request_seq: 1,
            idempotency_key: "different-request".into(),
            ..mutation
        };
        let result = db.commit_rpc_mutation::<serde_json::Value, _>(&stale, |tx| {
            tx.execute(
                "UPDATE currencies SET amount=amount+7 WHERE usn=?1 AND kind='candy'",
                [identity.usn],
            )?;
            Ok(serde_json::json!({"amount": 12}))
        });
        assert!(matches!(
            result,
            Err(StoreError::Domain(DomainError::StaleRequest))
        ));
        assert_eq!(db.currency(identity.usn, CurrencyKind::Candy).unwrap(), 5.0);
    }

    #[test]
    fn user_save_keeps_spendable_balances_exact_and_progress_monotonic() {
        let (mut db, identity) = setup();
        let session = db
            .begin_login(b"save-capability", DeviceClock::new(200, 0).unwrap())
            .unwrap();
        let mutation = RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: 1,
            rpc: "userSave".into(),
            idempotency_key: "save-1".into(),
            committed_at: 201,
        };
        let patch = UserSavePatch {
            core: Some(ggfm_domain::CoreSavePatch {
                ch1_like: Some(100.0),
                fans: Some(500),
                fan_level: Some(99),
                current_costume: Some(2),
                current_music: None,
            }),
            areas: vec![
                ggfm_domain::AreaSavePatch {
                    area: 1,
                    like_amount: Some(100.0),
                    fans: Some(400),
                    fan_level: Some(2),
                    candy: Some(50.0),
                    ..Default::default()
                },
                ggfm_domain::AreaSavePatch {
                    area: 2,
                    like_amount: Some(200.0),
                    fans: Some(500),
                    fan_level: Some(3),
                    candy: Some(75.0),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        db.merge_user_save(&mutation, &patch, "device-a", 10, &[])
            .unwrap();
        let second = RpcMutation {
            request_seq: 2,
            idempotency_key: "save-2".into(),
            committed_at: 202,
            ..mutation
        };
        let older = UserSavePatch {
            core: Some(ggfm_domain::CoreSavePatch {
                ch1_like: Some(10.0),
                fans: Some(50),
                fan_level: Some(88),
                current_costume: None,
                current_music: None,
            }),
            areas: vec![ggfm_domain::AreaSavePatch {
                area: 1,
                like_amount: Some(10.0),
                fans: Some(40),
                fan_level: Some(1),
                candy: Some(5.0),
                ..Default::default()
            }],
            ..Default::default()
        };
        db.merge_user_save(&second, &older, "device-a", 10, &[])
            .unwrap();
        let snapshot = db.player_snapshot(identity.usn).unwrap();
        assert_eq!(snapshot.currency(CurrencyKind::Ch1Like), 10.0);
        assert_eq!(snapshot.currency(CurrencyKind::Ch2Like), 200.0);
        assert_eq!(snapshot.currency(CurrencyKind::Candy), 5.0);
        assert_eq!(snapshot.currency(CurrencyKind::Fans), 500.0);
        assert_eq!(snapshot.fan_level, 1);
        assert_eq!(snapshot.areas[0].fans, 40.0);
        assert_eq!(snapshot.areas[0].area_level, 1);
        assert_eq!(snapshot.areas[0].candy, 5.0);
        assert_eq!(snapshot.areas[1].fans, 500.0);
        assert_eq!(snapshot.areas[1].area_level, 3);
        assert_eq!(snapshot.areas[1].candy, 75.0);
        assert_eq!(snapshot.areas[0].current_costume, Some(2));
        assert!(!snapshot.owned_content.iter().any(|content| {
            content.kind == ContentKind::Costume && content.area == 1 && content.content_id == 2
        }));
    }

    #[test]
    fn user_save_music_does_not_invent_or_overwrite_server_timestamps() {
        let (mut db, identity) = setup();
        db.connection
            .execute(
                concat!(
                    "INSERT INTO music_state(usn,music_id,encore_appears,encore_follower_id,encore_ready_at,ch3_active_until) ",
                    "VALUES(?1,1,0,NULL,999,888) ON CONFLICT(usn,music_id) DO UPDATE SET ",
                    "encore_ready_at=999,ch3_active_until=888"
                ),
                [identity.usn],
            )
            .unwrap();
        let session = db
            .begin_login(b"music-save", DeviceClock::new(500, 0).unwrap())
            .unwrap();
        let mutation = RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: 1,
            rpc: "userSave".into(),
            idempotency_key: "music-save-1".into(),
            committed_at: 501,
        };
        db.merge_user_save(
            &mutation,
            &UserSavePatch {
                music_states: vec![ggfm_domain::MusicStateSavePatch {
                    music_id: 1,
                    encore_appears: true,
                    encore_follower_id: Some(77),
                }],
                ..Default::default()
            },
            "device-a",
            10,
            &[],
        )
        .unwrap();

        let music = db
            .player_snapshot(identity.usn)
            .unwrap()
            .music_states
            .into_iter()
            .find(|state| state.music_id == 1)
            .unwrap();
        assert!(music.encore_appears);
        assert_eq!(music.encore_follower_id, Some(77));
        assert_eq!(music.encore_ready_at, 999);
        assert_eq!(music.ch3_active_until, 888);
    }

    #[test]
    fn user_save_keeps_attendance_authoritative_and_accelerates_pass_delta() {
        let (mut db, identity) = setup();
        db.connection
            .execute(
                "UPDATE attendance_state SET attendance_count=2 WHERE usn=?1",
                [identity.usn],
            )
            .unwrap();
        let session = db
            .begin_login(b"authority-save", DeviceClock::new(600, 0).unwrap())
            .unwrap();
        let mutation = RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: 1,
            rpc: "userSave".into(),
            idempotency_key: "authority-save-1".into(),
            committed_at: 601,
        };
        db.merge_user_save(
            &mutation,
            &UserSavePatch {
                achievements: vec![ggfm_domain::ProgressSavePatch {
                    id: 10,
                    quantity: 999.0,
                    quantity_text: Some("999".into()),
                }],
                pass_progress: vec![ggfm_domain::PassProgressSavePatch {
                    event_type: "SUBSCRIBE_PASS".into(),
                    season: 2,
                    points: 12_345,
                    step: 3,
                    version: 6,
                }],
                ..Default::default()
            },
            "device-a",
            10,
            &[(2, 0, 0), (2, 14, 106_000), (2, 15, 121_000)],
        )
        .unwrap();

        let snapshot = db.player_snapshot(identity.usn).unwrap();
        let attendance = snapshot
            .achievements
            .iter()
            .find(|achievement| achievement.id == 10)
            .unwrap();
        assert_eq!(attendance.quantity, 2.0);
        assert_eq!(attendance.quantity_text, "2");
        let pass = snapshot
            .pass
            .progress
            .iter()
            .find(|progress| progress.season == 2 && progress.version == 6)
            .unwrap();
        assert_eq!(pass.points, 123_450);
        assert_eq!(pass.step, 15);
    }

    #[test]
    fn attendance_check_and_same_day_add_do_not_advance_twice() {
        let (mut db, identity) = setup();
        let session = db
            .begin_login(b"attendance", DeviceClock::new(86_400 * 10, 0).unwrap())
            .unwrap();
        let make = |seq, key: &str| RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: seq,
            rpc: "setAttendance".into(),
            idempotency_key: key.into(),
            committed_at: 86_400 * 10,
        };
        let check = db
            .attendance(
                &make(1, "check"),
                DeviceClock::new(86_400 * 10, 0).unwrap(),
                "check",
            )
            .unwrap();
        assert_eq!(check.status, "Y");
        assert_eq!(check.attendance_count, 0);
        let added = db
            .attendance(
                &make(2, "add"),
                DeviceClock::new(86_400 * 10, 0).unwrap(),
                "add",
            )
            .unwrap();
        assert_eq!(added.status, "ADD");
        assert_eq!(added.attendance_count, 1);
        let duplicate = db
            .attendance(
                &make(3, "add-again"),
                DeviceClock::new(86_400 * 10, 0).unwrap(),
                "add",
            )
            .unwrap();
        assert_eq!(duplicate.status, "N");
        assert_eq!(duplicate.attendance_count, 1);
    }

    #[test]
    fn purchase_is_atomic_and_reward_replays_are_empty() {
        let (mut db, identity) = setup();
        db.grant_currency(identity.usn, CurrencyKind::Chocolate, 20.0, "seed", 101)
            .unwrap();
        let session = db
            .begin_login(b"purchase", DeviceClock::new(102, 0).unwrap())
            .unwrap();
        let first = RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: 1,
            rpc: "buyShop".into(),
            idempotency_key: "request-1".into(),
            committed_at: 103,
        };
        let rewards = vec![
            RewardGrant {
                reward_type: 1,
                reward_id: 2,
                amount: 3.0,
            },
            RewardGrant {
                reward_type: 3,
                reward_id: 10,
                amount: 1.0,
            },
        ];
        let result = db
            .purchase_rewards(
                &first,
                RewardPurchaseCommand {
                    transaction_id: "shop-once:6".into(),
                    purchase_kind: "shop".into(),
                    target_id: 6,
                    cost: Some((CurrencyKind::Chocolate, 10.0)),
                    rewards: rewards.clone(),
                    ad_progression: None,
                },
            )
            .unwrap();
        assert_eq!(result.chocolate, 10.0);
        assert_eq!(result.candy, 3.0);
        assert_eq!(result.rewards, rewards);
        assert!(
            db.purchase_rewards(
                &first,
                RewardPurchaseCommand {
                    transaction_id: "shop-once:6".into(),
                    purchase_kind: "shop".into(),
                    target_id: 6,
                    cost: Some((CurrencyKind::Chocolate, 10.0)),
                    rewards: rewards.clone(),
                    ad_progression: None,
                },
            )
            .unwrap()
            .rewards
            .is_empty()
        );
        assert_eq!(
            db.currency(identity.usn, CurrencyKind::Chocolate).unwrap(),
            10.0
        );

        let second = RpcMutation {
            request_seq: 2,
            idempotency_key: "request-2".into(),
            committed_at: 104,
            ..first
        };
        let replay = db
            .purchase_rewards(
                &second,
                RewardPurchaseCommand {
                    transaction_id: "shop-once:6".into(),
                    purchase_kind: "shop".into(),
                    target_id: 6,
                    cost: Some((CurrencyKind::Chocolate, 10.0)),
                    rewards: rewards.clone(),
                    ad_progression: None,
                },
            )
            .unwrap();
        assert!(replay.rewards.is_empty());
        assert_eq!(replay.chocolate, 10.0);
        assert_eq!(replay.candy, 3.0);
    }

    #[test]
    fn free_shop_claim_advances_its_own_ad_level_exactly_once() {
        let (mut db, identity) = setup();
        let session = db
            .begin_login(b"free-shop-level", DeviceClock::new(700, 0).unwrap())
            .unwrap();
        let progression = AdLevelProgression {
            group_id: 200_010,
            thresholds: vec![(1, 0), (2, 2), (3, 4)],
        };
        let reward = RewardGrant {
            reward_type: 1,
            reward_id: 1,
            amount: 10.0,
        };
        let mutation = |sequence| RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: sequence,
            rpc: "buyShop".into(),
            idempotency_key: format!("free-shop-level:{sequence}"),
            committed_at: 700 + sequence,
        };

        let first = db
            .purchase_rewards(
                &mutation(1),
                RewardPurchaseCommand {
                    transaction_id: "free-shop:1".into(),
                    purchase_kind: "shop".into(),
                    target_id: 3,
                    cost: None,
                    rewards: vec![reward.clone()],
                    ad_progression: Some(progression.clone()),
                },
            )
            .unwrap();
        assert_eq!(
            first.ad_level,
            Some(AdLevelSnapshot {
                ad_level_id: 200_010,
                level: 1,
                experience: 1,
            })
        );

        let second_mutation = mutation(2);
        let second = db
            .purchase_rewards(
                &second_mutation,
                RewardPurchaseCommand {
                    transaction_id: "free-shop:2".into(),
                    purchase_kind: "shop".into(),
                    target_id: 3,
                    cost: None,
                    rewards: vec![reward.clone()],
                    ad_progression: Some(progression.clone()),
                },
            )
            .unwrap();
        assert_eq!(second.ad_level.as_ref().unwrap().level, 2);
        assert_eq!(second.ad_level.as_ref().unwrap().experience, 2);

        let replay = db
            .purchase_rewards(
                &second_mutation,
                RewardPurchaseCommand {
                    transaction_id: "free-shop:2".into(),
                    purchase_kind: "shop".into(),
                    target_id: 3,
                    cost: None,
                    rewards: vec![reward.clone()],
                    ad_progression: Some(progression.clone()),
                },
            )
            .unwrap();
        assert_eq!(replay.ad_level, second.ad_level);
        assert_eq!(db.ad_level(identity.usn, 200_010).unwrap().experience, 2);
    }

    #[test]
    fn premium_upgrade_curve_is_charged_at_transaction_level_and_replay_is_free() {
        let (mut db, identity) = setup();
        let clock = DeviceClock::new(500, 0).unwrap();
        let session = db.begin_login(b"curve", clock).unwrap();
        db.grant_currency(identity.usn, CurrencyKind::Chocolate, 1000.0, "seed-curve", 500).unwrap();
        let mut balance = 1000.0;
        let mut seq = 0;
        for (kind, id, maximum, increment, offset, cap) in [
            (ContentKind::Skill, 2, 20, 0.5, 2, 10.0),
            (ContentKind::Skill, 3, 20, 14.0 / 18.0, 2, 15.0),
            (ContentKind::Skill, 23, 20, 19.0 / 18.0, 2, 20.0),
            (ContentKind::Unit, 3, 6, 3.8, 1, 20.0),
        ] {
            let mut last_cost = 0.0;
            for target in 1..=maximum {
                seq += 1;
                let mutation = RpcMutation { nonce: session.nonce, identity: identity.clone(),
                    request_seq: seq, rpc: "buyContents".into(), idempotency_key: format!("curve:{seq}"), committed_at: 500 + seq };
                let command = ContentPurchaseCommand { kind, area: 1, content_id: id,
                    currency: CurrencyKind::Chocolate, price: 1.0, price_increment: increment, max_level: maximum };
                let cost = (1.0_f32 + (target - offset).max(0) as f32 * increment as f32).trunc() as f64;
                assert!(cost >= last_cost && cost <= cap);
                balance -= cost;
                let result = db.purchase_content(&mutation, command.clone()).unwrap();
                assert_eq!(result.level, target);
                assert_eq!(result.chocolate, balance);
                let replay = db.purchase_content(&mutation, command).unwrap();
                assert_eq!(replay, result);
                last_cost = cost;
            }
            assert_eq!(last_cost, cap);
        }
    }

    #[test]
    fn purchases_reject_insufficient_balance_without_partial_mutation() {
        let (mut db, identity) = setup();
        let clock = DeviceClock::new(500, 0).unwrap();
        let session = db.begin_login(b"free-purchase", clock).unwrap();
        let make = |seq, rpc: &str| RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: seq,
            rpc: rpc.into(),
            idempotency_key: format!("{rpc}:{seq}"),
            committed_at: clock.unix_seconds + seq,
        };

        let rejected_content = db.purchase_content(
            &make(1, "buyContents"),
            ContentPurchaseCommand {
                kind: ContentKind::Skill,
                area: 1,
                content_id: 2,
                currency: CurrencyKind::Chocolate,
                price: 9.0,
                price_increment: 0.0,
                max_level: 2,
            },
        );
        assert!(matches!(
            rejected_content,
            Err(StoreError::Domain(DomainError::InsufficientBalance))
        ));
        assert_eq!(
            db.player_snapshot(identity.usn)
                .unwrap()
                .content_levels
                .iter()
                .find(|row| row.kind == ContentKind::Skill && row.content_id == 2),
            None
        );

        db.grant_currency(
            identity.usn,
            CurrencyKind::Chocolate,
            20.0,
            "purchase-seed-1",
            501,
        )
        .unwrap();
        let content = db
            .purchase_content(
                &make(1, "buyContents"),
                ContentPurchaseCommand {
                    kind: ContentKind::Skill,
                    area: 1,
                    content_id: 2,
                    currency: CurrencyKind::Chocolate,
                    price: 9.0,
                    price_increment: 0.0,
                    max_level: 2,
                },
            )
            .unwrap();
        assert_eq!(content.level, 1);
        assert_eq!(content.chocolate, 11.0);

        let maxed = db
            .purchase_content(
                &make(2, "buyContents"),
                ContentPurchaseCommand {
                    kind: ContentKind::Skill,
                    area: 1,
                    content_id: 2,
                    currency: CurrencyKind::Chocolate,
                    price: 9.0,
                    price_increment: 0.0,
                    max_level: 1,
                },
            )
            .unwrap();
        assert_eq!(maxed.level, 1);
        assert_eq!(maxed.chocolate, 11.0);

        let reward = RewardGrant {
            reward_type: 1,
            reward_id: 2,
            amount: 4.0,
        };
        let rejected_shop = db.purchase_rewards(
            &make(3, "buyShop"),
            RewardPurchaseCommand {
                transaction_id: "empty-balance-shop".into(),
                purchase_kind: "shop".into(),
                target_id: 1,
                cost: Some((CurrencyKind::Chocolate, 99.0)),
                rewards: vec![reward.clone()],
                ad_progression: None,
            },
        );
        assert!(matches!(
            rejected_shop,
            Err(StoreError::Domain(DomainError::InsufficientBalance))
        ));
        assert_eq!(db.currency(identity.usn, CurrencyKind::Candy).unwrap(), 0.0);

        db.grant_currency(
            identity.usn,
            CurrencyKind::Chocolate,
            150.0,
            "purchase-seed-2",
            503,
        )
        .unwrap();
        let shop = db
            .purchase_rewards(
                &make(3, "buyShop"),
                RewardPurchaseCommand {
                    transaction_id: "empty-balance-shop".into(),
                    purchase_kind: "shop".into(),
                    target_id: 1,
                    cost: Some((CurrencyKind::Chocolate, 99.0)),
                    rewards: vec![reward.clone()],
                    ad_progression: None,
                },
            )
            .unwrap();
        assert_eq!(shop.chocolate, 62.0);
        assert_eq!(shop.candy, 4.0);
        assert_eq!(shop.rewards, vec![reward]);

        let pass = db
            .fill_pass(&make(4, "paidEventPoint"), 3, 0, 50.0, 100, &[(1, 100)])
            .unwrap();
        assert_eq!(pass.chocolate, 12.0);
        assert_eq!(pass.points, 100);
        assert_eq!(pass.version, 0);
    }

    #[test]
    fn pass_progress_claims_and_versions_are_independent() {
        let (mut db, identity) = setup();
        db.grant_currency(
            identity.usn,
            CurrencyKind::Chocolate,
            100.0,
            "pass-seed",
            101,
        )
        .unwrap();
        let clock = DeviceClock::new(86_400 * 20, 120).unwrap();
        let session = db.begin_login(b"pass", clock).unwrap();
        let make = |seq, rpc: &str| RpcMutation {
            nonce: session.nonce,
            identity: identity.clone(),
            request_seq: seq,
            rpc: rpc.into(),
            idempotency_key: format!("{rpc}:{seq}"),
            committed_at: clock.unix_seconds + seq,
        };
        let fill = db
            .fill_pass(
                &make(1, "paidEventPoint"),
                2,
                1,
                50.0,
                500,
                &[(0, 0), (1, 500)],
            )
            .unwrap();
        assert_eq!(fill.points, 500);
        assert_eq!(fill.chocolate, 50.0);
        let reward = vec![RewardGrant {
            reward_type: 1,
            reward_id: 2,
            amount: 2.0,
        }];
        let claimed = db
            .claim_pass_reward(&make(2, "setPassReward"), 2, 1, 0, 1, 500, &reward, 32, 2)
            .unwrap();
        assert_eq!(claimed.rewards, reward);
        let retry = db
            .claim_pass_reward(&make(3, "setPassReward"), 2, 1, 0, 1, 500, &reward, 32, 2)
            .unwrap();
        assert!(retry.rewards.is_empty());
        db.fill_pass(&make(4, "paidEventPoint"), 2, 2, 0.0, 500, &[(1, 500)])
            .map(|fill| {
                assert_eq!(fill.version, 2);
                assert_eq!(fill.points, 500);
            })
            .unwrap();
        let version_two = db
            .claim_pass_reward(&make(5, "setPassReward"), 2, 2, 0, 1, 500, &reward, 32, 2)
            .unwrap();
        assert_eq!(version_two.rewards, reward);
        let snapshot = db.pass_snapshot(identity.usn).unwrap();
        assert_eq!(snapshot.progress.len(), 2);
        assert_eq!(snapshot.claims.len(), 2);
        db.select_pass_season(identity.usn, clock, 9).unwrap();
        assert_eq!(
            db.pass_snapshot(identity.usn).unwrap().anchor.anchor_season,
            9
        );
    }

    #[test]
    fn pass_selection_revision_and_anchor_survive_restart() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ggfm-pass-anchor-{suffix}.sqlite"));
        let clock = DeviceClock::new(86_400 * 30, 600).unwrap();

        let identity = {
            let mut db = Database::open(&path).unwrap();
            let identity = db.create_slot("Pass", clock.unix_seconds).unwrap();
            assert_eq!(db.state_revision(identity.usn, "master-data").unwrap(), 0);
            db.select_pass_season(identity.usn, clock, 9).unwrap();
            assert_eq!(db.state_revision(identity.usn, "master-data").unwrap(), 1);
            db.checkpoint().unwrap();
            identity
        };

        let db = Database::open(&path).unwrap();
        assert_eq!(db.state_revision(identity.usn, "master-data").unwrap(), 1);
        let pass = db.pass_snapshot(identity.usn).unwrap();
        assert_eq!(pass.anchor.anchor_day, clock.local_epoch_day());
        assert_eq!(pass.anchor.anchor_season, 9);
        assert_eq!(pass.anchor.utc_offset_minutes, 600);
        drop(db);

        for candidate in [
            path.clone(),
            std::path::PathBuf::from(format!("{}-wal", path.display())),
            std::path::PathBuf::from(format!("{}-shm", path.display())),
        ] {
            if candidate.exists() {
                std::fs::remove_file(candidate).unwrap();
            }
        }
    }
}
