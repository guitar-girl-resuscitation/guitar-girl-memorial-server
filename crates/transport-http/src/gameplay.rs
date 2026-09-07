use std::collections::BTreeMap;

use ggfm_domain::{
    AdLevelProgression, AdProfileGrant, AreaSavePatch, BookmarkSnapshot, Ch3StageDefinition,
    ContentKind, ContentPurchaseCommand, ContentSavePatch, CoreSavePatch, CurrencyKind,
    DeviceClock, FollowerQuestSavePatch, GameRewardClaimCommand, LoginSession, MailSnapshot,
    MessengerSavePatch, MusicReviewSnapshot, MusicScoreSnapshot, MusicStateSavePatch,
    PassProgressSavePatch, PlayerSnapshot, ProgressSavePatch, RewardGrant, RewardPurchaseCommand,
    SkillActivationSavePatch, UserSavePatch,
};
use ggfm_master_data::{MasterRow, MasterScalar, normalize_column_name};
use ggfm_persistence_sqlite::RpcMutation;
use ggfm_policy_memorial::{
    DEFAULT_SETTINGS, MemorialPolicy, memorial_upgrade_cost, pass_season, scaled_fan_requirement,
    scaled_requirement, scaled_requirement_text,
};
use ggfm_protocol::{
    I16, I32, LIST, NamedRequest, ProtocolEnvelopeError, ProtocolSchema, RpcClass, RpcName, STRING,
    STRUCT, Value, response_sparse_struct, response_struct, sparse_struct_value, struct_value,
};
use sha2::{Digest, Sha256};

use crate::AppState;

const GAME_DATA_RESPONSE_KEY: &str = "ret";

// Generation 11 invalidates rows cached before Patch preserved fractional
// Skill/Unit increments. A constructor-only fix cannot repair serialized rows
// that bypass the constructor on a same-day application upgrade.
const MASTER_OVERLAY_GENERATION: i64 = 11;

fn master_update_time(clock: DeviceClock, slot_revision: i64) -> i64 {
    let local_midnight_utc = clock.local_epoch_day().saturating_mul(86_400)
        - i64::from(clock.utc_offset_minutes) * 60;
    local_midnight_utc
        .saturating_add(MASTER_OVERLAY_GENERATION)
        .saturating_add(slot_revision)
}

#[cfg(test)]
#[path = "master_projection_tests.rs"]
mod master_projection_tests;

#[cfg(test)]
#[path = "master_cache_tests.rs"]
mod master_cache_tests;

#[derive(Debug, thiserror::Error)]
pub enum GameplayError {
    #[error(transparent)]
    Protocol(#[from] ProtocolEnvelopeError),
    #[error(transparent)]
    Store(#[from] ggfm_persistence_sqlite::StoreError),
    #[error("{0}")]
    Invalid(String),
}

pub fn is_implemented(rpc: RpcName) -> bool {
    rpc.class() == RpcClass::Active
        && matches!(
            rpc.as_str(),
            "init"
                | "getUpdateTime"
                | "getServerTime"
                | "defaultSettingList"
                | "completeLog"
                | "main"
                | "avatarList"
                | "titleList"
                | "getCollection"
                | "getLocalPush"
                | "getReviewPoint"
                | "getUserCollection"
                | "userTitleList"
                | "musicPointReview"
                | "getMusicReward"
                | "setBookmark"
                | "updateScore"
                | "setFollowerProfileGift"
                | "setFollowerQuestInfinite"
                | "setUserFollowerProfileReward"
                | "setUserFollowerQuest"
                | "updateAchievement"
                | "setGameReward"
                | "setReviewPopup"
                | "setAdReward"
                | "getSamSeckList"
                | "getSamSeckReward"
                | "getEventRewardList"
                | "setEventReward"
                | "getGameDataList"
                | "setSelectReward"
                | "getVarietyStore"
                | "buyVarietyStore"
                | "userJoin"
                | "userLogin"
                | "userLoad"
                | "getChoiceUser"
                | "changeUser"
                | "getAllPoint"
                | "lastSaveTime"
                | "buyCheck"
                | "checkBuyShop"
                | "checkPurchased"
                | "buyContents"
                | "buyMusic"
                | "buyPackage"
                | "buyShop"
                | "paidEventPoint"
                | "setPassReward"
                | "setSubscribe"
                | "getChThird"
                | "chThirdStage"
                | "chThirdStreamStage"
                | "getChThirdStarReward"
                | "getPostTime"
                | "getPost"
                | "readPost"
                | "providePost"
                | "deletePost"
                | "setAttendance"
                | "setTutorial"
                | "updateAvatar"
                | "setTutorialNew"
                | "setUserGP"
                | "userSave"
                | "userPurchaseErrorLog"
                | "userDel"
                | "updateNickName"
                | "updateUserTitle"
        )
}

/// Executes the behavior that is already backed by normalized storage. Calls
/// not yet migrated return `None`; the transport still emits their audited
/// compatibility shape and never grants side effects implicitly.
pub struct GameplayCall<'a> {
    pub rpc: RpcName,
    pub locale: &'a str,
    pub request: &'a NamedRequest,
    pub clock: DeviceClock,
    pub sequence: Option<i64>,
    pub raw_request: &'a [u8],
}

pub async fn execute(
    state: &AppState,
    session: &LoginSession,
    call: GameplayCall<'_>,
) -> Result<Option<Value>, GameplayError> {
    let GameplayCall {
        rpc,
        locale,
        request,
        clock,
        sequence,
        raw_request,
    } = call;
    let schema = ProtocolSchema::embedded();
    let spec = schema.call_typed(rpc);
    let rpc = rpc.as_str();
    let data = match rpc {
        "init" => Some(response_struct(
            spec,
            schema,
            [
                ("Idx".into(), Value::I16(1)),
                (
                    "Game_url".into(),
                    Value::String(state.endpoint.as_bytes().to_vec()),
                ),
                (
                    "Cdn_url".into(),
                    Value::String(format!("{}/AssetBundles/", state.endpoint).into_bytes()),
                ),
            ],
        )?),
        "getUpdateTime" => {
            // Stable within one device-local day, but advances at local
            // midnight so the client refreshes daily memorial policy (notably
            // Star Pass rotation). A non-zero generation also invalidates the
            // stock cache after a policy/schema revision without forcing a
            // multi-megabyte master refresh on every launch.
            let usn = session.identity.usn;
            let slot_revision = state
                .database
                .execute(move |database| database.state_revision(usn, "master-data"))
                .await?;
            let update_time = master_update_time(clock, slot_revision);
            tracing::debug!(usn, generation = MASTER_OVERLAY_GENERATION, slot_revision,
                update_time, "master: advertise client table cache revision");
            Some(response_sparse_struct(
                spec,
                schema,
                [(
                    "Upd_time".into(),
                    Value::I64(update_time),
                )],
            )?)
        }
        "getServerTime" => Some(response_struct(
            spec,
            schema,
            [
                ("Time".into(), Value::I64(clock.unix_seconds)),
                ("Datetime".into(), Value::I64(device_datetime_number(clock))),
            ],
        )?),
        "defaultSettingList" => Some(response_struct(
            spec,
            schema,
            [
                ("Status".into(), text("Y")),
                (
                    "Setting_list".into(),
                    Value::List {
                        element_type: STRUCT,
                        values: DEFAULT_SETTINGS
                            .into_iter()
                            .map(|(key, value)| default_setting_wire_value(key, value))
                            .collect::<Result<_, _>>()?,
                    },
                ),
            ],
        )?),
        "completeLog" => Some(response_struct(
            spec,
            schema,
            [("Status".into(), text("Y"))],
        )?),
        "getGameDataList" => {
            let usn = session.identity.usn;
            let pass = state
                .database
                .execute(move |database| database.pass_snapshot(usn))
                .await?;
            let (_, local_month, _) = civil_from_days(clock.local_epoch_day());
            let pass_overlay = PassMasterOverlay {
                active_season: pass_season(
                    clock,
                    pass.anchor.anchor_day,
                    pass.anchor.anchor_season,
                ),
                local_month,
            };
            let mut tables = Vec::with_capacity(state.master.game_data.len());
            for (field_name, rows) in &state.master.game_data {
                let result_type = schema
                    .type_spec("main_model::GetGameDataListRetDataInfo")
                    .and_then(|spec| spec.fields.iter().find(|field| field.name == *field_name))
                    .and_then(|field| field.wire_type.strip_prefix("[]"))
                    .ok_or_else(|| {
                        GameplayError::Invalid(format!(
                            "missing getGameDataList schema field {field_name}"
                        ))
                    })?;
                let values = rows
                    .iter()
                    .map(|row| {
                        game_data_row_value(state, field_name, result_type, row, pass_overlay)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                tables.push((
                    field_name.clone(),
                    Value::List {
                        element_type: STRUCT,
                        values,
                    },
                ));
            }
            let payload = struct_value(schema, "main_model", "GetGameDataListRetDataInfo", tables)?;
            Some(Value::Map {
                key_type: STRING,
                value_type: STRUCT,
                // Production requests use Game_type="ALL", but the captured
                // response and the tested Rust baseline always place the
                // aggregate master table under the literal key "ret".
                entries: vec![(text(GAME_DATA_RESPONSE_KEY), payload)],
            })
        }
        "getVarietyStore" => {
            let mut rows = variety_store_rows(state)?;
            let usn = session.identity.usn;
            let snapshot = state.database.execute(move |database| database.player_snapshot(usn)).await?;
            for row in &mut rows {
                let level = snapshot.content_levels.iter().find(|content|
                    content.kind == ContentKind::Prop && content.content_id == row.reward_id)
                    .map(|content| content.level).unwrap_or(0);
                row.cost = (1.0_f32 + row.price_increment as f32 * level as f32).trunc() as i32;
            }
            Some(Value::List {
                element_type: STRUCT,
                values: rows
                    .iter()
                    .map(variety_store_wire_value)
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
        "buyVarietyStore" => {
            let store_id = request.integer("Idx").unwrap_or(0);
            let row = variety_store_rows(state)?
                .into_iter()
                .find(|row| row.id == store_id)
                .ok_or_else(|| {
                    GameplayError::Invalid(format!("unknown variety store item {store_id}"))
                })?;
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let outcome = state
                .database
                .execute(move |database| {
                    database.purchase_content(
                        &mutation,
                        ContentPurchaseCommand {
                            kind: ContentKind::Prop,
                            area: row.area,
                            content_id: row.reward_id,
                            currency: CurrencyKind::Candy,
                            price: 1.0,
                            price_increment: row.price_increment,
                            max_level: i64::from(row.max_level),
                        },
                    )
                })
                .await?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("U_cp".into(), Value::I64(amount_i64(outcome.chocolate))),
                    ("U_candy".into(), Value::Double(outcome.candy)),
                    ("Reward_type".into(), Value::I16(4)),
                    ("Reward_id".into(), Value::I32(to_i32(outcome.content_id))),
                    ("Reward_value".into(), Value::I64(1)),
                    ("Status".into(), text("Y")),
                ],
            )?)
        }
        "avatarList" | "titleList" | "getCollection" | "getLocalPush" | "getReviewPoint"
        | "getUserCollection" => Some(Value::List {
            element_type: STRUCT,
            values: Vec::new(),
        }),
        "userTitleList" => {
            let usn = session.identity.usn;
            let snapshot = state
                .database
                .execute(move |database| database.player_snapshot(usn))
                .await?;
            let titles = snapshot
                .title_id
                .into_iter()
                .map(|id| Value::I32(id as i32))
                .collect();
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("U_seq".into(), Value::I32(to_i32(usn))),
                    (
                        "Title".into(),
                        Value::List {
                            element_type: I32,
                            values: titles,
                        },
                    ),
                ],
            )?)
        }
        "main" => Some(response_sparse_struct(
            spec,
            schema,
            [
                ("GameInfo".into(), main_game_info_wire_value(state, locale)?),
                (
                    "Features".into(),
                    Value::List {
                        element_type: STRUCT,
                        values: Vec::new(),
                    },
                ),
            ],
        )?),
        "musicPointReview" => {
            let reviews = parse_music_reviews(request)?;
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let rows = state
                .database
                .execute(move |database| database.save_music_reviews(&mutation, reviews))
                .await?;
            Some(Value::List {
                element_type: STRUCT,
                values: rows
                    .iter()
                    .map(music_review_wire_value)
                    .collect::<Result<_, _>>()?,
            })
        }
        "setBookmark" => {
            let bookmarks = parse_bookmarks(request)?;
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let rows = state
                .database
                .execute(move |database| database.save_bookmarks(&mutation, bookmarks))
                .await?;
            Some(Value::List {
                element_type: STRUCT,
                values: rows
                    .iter()
                    .map(bookmark_wire_value)
                    .collect::<Result<_, _>>()?,
            })
        }
        "updateScore" => {
            let scores = parse_music_scores(request)?;
            let keys: std::collections::BTreeSet<_> = scores
                .iter()
                .map(|row| (row.music_id, row.difficulty))
                .collect();
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let merged = state
                .database
                .execute(move |database| database.merge_music_scores(&mutation, scores))
                .await?;
            let rows = merged
                .iter()
                .filter(|row| keys.contains(&(row.music_id, row.difficulty)))
                .map(music_score_wire_value)
                .collect::<Result<_, _>>()?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    (
                        "Update_list".into(),
                        Value::List {
                            element_type: STRUCT,
                            values: rows,
                        },
                    ),
                    ("U_gold".into(), Value::I64(0)),
                    ("U_ep".into(), Value::I64(0)),
                    ("Event_flg".into(), Value::I16(0)),
                ],
            )?)
        }
        "getMusicReward" => {
            let music_ids = request.integers("I_ids").unwrap_or_default();
            let levels = request.integers("I_levels").unwrap_or_default();
            if music_ids.len() != levels.len() {
                return Err(GameplayError::Invalid(
                    "getMusicReward requires positional I_ids/I_levels pairs".into(),
                ));
            }
            let policy = MemorialPolicy::default();
            let mut requested = Vec::with_capacity(music_ids.len());
            for (music_id, requested_level) in music_ids.into_iter().zip(levels) {
                let music = state
                    .master
                    .music
                    .iter()
                    .find(|row| row.id == music_id && row.active)
                    .ok_or_else(|| GameplayError::Invalid(format!("unknown music {music_id}")))?;
                let level = i32::try_from(requested_level)
                    .unwrap_or(i32::MAX)
                    .clamp(1, music.max_level.max(1));
                let level_data = state
                    .master
                    .music_levels
                    .iter()
                    .find(|row| row.level == level)
                    .ok_or_else(|| {
                        GameplayError::Invalid(format!("missing music level {level}"))
                    })?;
                let cooldown = ggfm_policy_memorial::encore_cooldown(
                    level_data.official_encore_cooldown_seconds,
                    75_600.0,
                )
                .round() as i64;
                requested.push((
                    music_id,
                    level_data.gift_amount,
                    level_data.follower_profile_exp,
                    clock
                        .unix_seconds
                        .saturating_add(cooldown.max(policy.encore_base_seconds as i64 / 21)),
                ));
            }
            let thresholds = state
                .master
                .follower_profile_levels
                .iter()
                .filter(|row| row.active)
                .map(|row| {
                    (
                        row.profile_id,
                        row.level,
                        row.required_exp.max(0.0) as i64,
                        row.add_candy,
                    )
                })
                .collect();
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let outcome = state
                .database
                .execute(move |database| {
                    database.claim_music_encore_rewards(&mutation, requested, thresholds)
                })
                .await?;
            let total = outcome.rewards.iter().map(|(_, value)| *value).sum::<i32>();
            let reward_ids = outcome
                .rewards
                .iter()
                .map(|(id, _)| Value::I32(*id))
                .collect();
            let reward_values = outcome
                .rewards
                .iter()
                .map(|(_, value)| Value::I32(*value))
                .collect();
            let profiles = outcome
                .profiles
                .iter()
                .map(ch3_profile_wire_value)
                .collect::<Result<Vec<_>, _>>()?;
            let errors = outcome
                .rejected_music_ids
                .iter()
                .map(|id| {
                    Ok((
                        Value::I32(*id),
                        sparse_struct_value(
                            schema,
                            "common_model",
                            "ErrorRetCode",
                            [
                                ("Code".into(), Value::I32(100_103)),
                                ("Errmsg".into(), text("encore reward unavailable")),
                            ],
                        )?,
                    ))
                })
                .collect::<Result<Vec<_>, ProtocolEnvelopeError>>()?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("Total_reward_value".into(), Value::I32(total)),
                    (
                        "Reward_music_id".into(),
                        Value::List {
                            element_type: I32,
                            values: reward_ids,
                        },
                    ),
                    (
                        "Reward_value".into(),
                        Value::List {
                            element_type: I32,
                            values: reward_values,
                        },
                    ),
                    (
                        "User_follower_profile".into(),
                        Value::List {
                            element_type: STRUCT,
                            values: profiles,
                        },
                    ),
                    (
                        "Error_data".into(),
                        Value::Map {
                            key_type: I32,
                            value_type: STRUCT,
                            entries: errors,
                        },
                    ),
                ],
            )?)
        }
        "setFollowerProfileGift" => {
            let profile_id = request.integer("Profile_id").unwrap_or(0);
            let gift_id = request.integer("Gift_id").unwrap_or(0);
            let quantity = request.integer("Use_gitf_value").unwrap_or(0);
            let gift = state
                .master
                .follower_gifts
                .iter()
                .find(|row| row.id == gift_id && row.active)
                .ok_or_else(|| {
                    GameplayError::Invalid(format!("unknown follower gift {gift_id}"))
                })?;
            let thresholds = follower_profile_thresholds(state, profile_id);
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let gift_type = gift.gift_type;
            let experience = gift.experience;
            let outcome = state
                .database
                .execute(move |database| {
                    database.give_follower_gift(
                        &mutation, profile_id, gift_id, quantity, experience, thresholds,
                    )
                })
                .await?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("I_gift_type".into(), Value::I32(gift_type)),
                    (
                        "User_follower_giftitem".into(),
                        follower_gift_wire_value(gift_id, outcome.remaining)?,
                    ),
                    (
                        "User_follower_profile".into(),
                        ch3_profile_wire_value(&outcome.profile)?,
                    ),
                ],
            )?)
        }
        "setFollowerQuestInfinite" => {
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let quest = state
                .database
                .execute(move |database| database.set_follower_quest_infinite(&mutation))
                .await?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("U_seq".into(), Value::I32(to_i32(session.identity.usn))),
                    (
                        "User_follower_quest".into(),
                        follower_quest_wire_value(&quest)?,
                    ),
                ],
            )?)
        }
        "setUserFollowerProfileReward" => {
            let profile_id = request.integer("I_id").unwrap_or(0);
            let levels: std::collections::BTreeSet<i32> = request
                .string("S_level")
                .unwrap_or("")
                .split(|character: char| !character.is_ascii_digit())
                .filter_map(|part| part.parse().ok())
                .collect();
            let requested = levels
                .into_iter()
                .map(|level| {
                    let row = state
                        .master
                        .follower_profile_levels
                        .iter()
                        .find(|row| {
                            row.active && row.profile_id == profile_id && row.level == level
                        })
                        .ok_or_else(|| {
                            GameplayError::Invalid(format!(
                                "unknown profile {profile_id} reward level {level}"
                            ))
                        })?;
                    Ok((level, reward_group(state, row.reward_group, 1.0)))
                })
                .collect::<Result<Vec<_>, GameplayError>>()?;
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let outcomes = state
                .database
                .execute(move |database| {
                    database.claim_follower_profile_rewards(&mutation, profile_id, requested)
                })
                .await?;
            Some(Value::List {
                element_type: STRUCT,
                values: outcomes
                    .iter()
                    .map(follower_profile_reward_wire_value)
                    .collect::<Result<_, _>>()?,
            })
        }
        "setUserFollowerQuest" => {
            let current_id = request.integer("I_CurrentID").unwrap_or(0);
            let sub_id = request.integer("I_SubID").unwrap_or(0);
            let value = request
                .value("D_ConditionValue")
                .and_then(|value| value_double(Some(value)))
                .unwrap_or(0.0);
            let quest = state
                .master
                .follower_quests
                .iter()
                .find(|row| row.id == current_id && row.active)
                .ok_or_else(|| {
                    GameplayError::Invalid(format!("unknown follower quest {current_id}"))
                })?;
            let sub_index = usize::try_from(sub_id - 1)
                .ok()
                .filter(|index| *index < 3)
                .ok_or_else(|| {
                    GameplayError::Invalid(format!("invalid follower quest sub id {sub_id}"))
                })?;
            let rewards = reward_group(state, quest.reward_groups[sub_index], 1.0);
            // Tested Rust advances arithmetically, including the terminal
            // sentinel 43 after main stage 42. The active Type=2 rows beginning
            // at 902 are not the next stages of the Type=1 guide chain.
            let next_id = Some(current_id.saturating_add(1));
            let thresholds = quest.thresholds;
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let outcome = state
                .database
                .execute(move |database| {
                    database.advance_follower_quest(
                        &mutation, current_id, sub_id, value, thresholds, rewards, next_id,
                    )
                })
                .await?;
            Some(follower_quest_response(spec, &outcome)?)
        }
        "userJoin" => {
            record_noop(state, mutation(session, rpc, sequence, clock, raw_request)?).await?;
            Some(response_struct(
                spec,
                schema,
                [
                    ("U_seq".into(), Value::I32(to_i32(session.identity.usn))),
                    ("U_id".into(), text(&session.identity.user_id)),
                ],
            )?)
        }
        "userLogin" if request.integer("U_seq") != Some(session.identity.usn) => {
            // The stock client performs an identity-discovery login before it
            // has a usable U_seq. A slot switch may also send the previous
            // slot's non-zero U_seq once. Match the tested Rust baseline: any
            // identity mismatch receives only User.U_seq/U_id so no state is
            // ever serialized under the wrong requested identity.
            Some(user_login_identity_response(spec, &session.identity)?)
        }
        "userLogin" => {
            let usn = session.identity.usn;
            let mut snapshot = state
                .database
                .execute(move |database| database.player_snapshot(usn))
                .await?;
            // The embedded endpoint belongs to this device. Echo its audited
            // login marker even for older saves with the former placeholder;
            // the next committed userSave persists it. USN remains unchanged.
            if let Some(device) = request.string("Device_uuid").filter(|s| !s.is_empty() && s.len() <= 256) {
                snapshot.device_uuid = device.to_owned();
            }
            Some(player_response(spec, &snapshot, clock)?)
        }
        "userLoad" => {
            let usn = session.identity.usn;
            let snapshot = state
                .database
                .execute(move |database| database.player_snapshot(usn))
                .await?;
            let include_area = request.string("Type") != Some("buff");
            Some(player_response_projected(spec, &snapshot, include_area, clock)?)
        }
        "getChoiceUser" => {
            let usn = session.identity.usn;
            let selector = request.string("Choice_uuid").unwrap_or("").to_owned();
            let (current, choice) = state
                .database
                .execute(move |database| {
                    let current = database.player_snapshot(usn)?;
                    let choice = if selector.is_empty() {
                        current.clone()
                    } else {
                        let identity = database.resolve_slot(&selector)?;
                        database.player_snapshot(identity.usn)?
                    };
                    Ok((current, choice))
                })
                .await?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("User".into(), choice_user_value(&current)?),
                    ("Choice_user".into(), choice_user_value(&choice)?),
                ],
            )?)
        }
        "getAllPoint" => {
            let usn = session.identity.usn;
            let snapshot = state
                .database
                .execute(move |database| database.player_snapshot(usn))
                .await?;
            Some(response_struct(
                spec,
                schema,
                [
                    (
                        "U_cp".into(),
                        Value::I64(amount_i64(
                            snapshot.currency(ggfm_domain::CurrencyKind::Chocolate),
                        )),
                    ),
                    (
                        "U_mp".into(),
                        Value::I64(amount_i64(
                            snapshot.currency(ggfm_domain::CurrencyKind::Candy),
                        )),
                    ),
                    ("U_credit".into(), Value::I64(0)),
                    ("Next_credit_time".into(), Value::I64(0)),
                    ("Max_credit_time".into(), Value::I64(0)),
                ],
            )?)
        }
        "lastSaveTime" => {
            let usn = session.identity.usn;
            let (saved_at, device_uuid) = state
                .database
                .execute(move |database| database.last_save_time(usn))
                .await?;
            Some(response_struct(
                spec,
                schema,
                [
                    ("Last_save_time".into(), Value::I64(saved_at)),
                    ("Device_uuid".into(), text(&device_uuid)),
                ],
            )?)
        }
        "buyCheck" => Some(response_struct(
            spec,
            schema,
            [("Result".into(), text("ok"))],
        )?),
        "checkBuyShop" => Some(response_struct(
            spec,
            schema,
            // This RPC belongs to the platform-IAP recovery path, not the
            // ordinary buyCheck -> buyShop button flow.  Reporting owned here
            // sends the client into its recovery/consume branch and leaves the
            // visible purchase callback armed, which is why the same button
            // appeared to require a second press in the tested client.
            [("Is_owner".into(), Value::I16(0))],
        )?),
        "checkPurchased" => Some(response_struct(
            spec,
            schema,
            [("Is_purchased".into(), Value::I16(1))],
        )?),
        "buyContents" => {
            let requested_kind = request.string("Type").unwrap_or("");
            let content_id = request.integer("Idx").unwrap_or(0);
            let purchase = content_purchase_spec(state, requested_kind, content_id)?;
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let result = state
                .database
                .execute(move |database| {
                    database.purchase_content(
                        &mutation,
                        ContentPurchaseCommand {
                            kind: purchase.kind,
                            area: purchase.area,
                            content_id: purchase.content_id,
                            currency: purchase.currency,
                            price: purchase.price,
                            price_increment: purchase.price_increment,
                            max_level: purchase.max_level,
                        },
                    )
                })
                .await?;
            let mut fields = vec![
                ("U_cp".into(), Value::I64(amount_i64(result.chocolate))),
                ("U_candy".into(), Value::Double(result.candy)),
            ];
            match result.kind {
                ContentKind::Skill => fields.push((
                    "User_skill".into(),
                    sparse_struct_value(
                        schema,
                        "user_model",
                        "UserSkill",
                        [
                            ("I_id".into(), Value::I64(result.content_id)),
                            ("I_Level".into(), Value::I64(result.level)),
                        ],
                    )?,
                )),
                ContentKind::Unit => fields.push((
                    "User_unit".into(),
                    sparse_struct_value(
                        schema,
                        "user_model",
                        "UserUnit",
                        [
                            ("I_id".into(), Value::I64(result.content_id)),
                            ("I_Level".into(), Value::I64(result.level)),
                        ],
                    )?,
                )),
                _ => {}
            }
            Some(response_sparse_struct(spec, schema, fields)?)
        }
        "buyMusic" => {
            let content_id = request.integer("Idx").unwrap_or(0);
            let row = state
                .master
                .music
                .iter()
                .find(|row| row.id == content_id && row.active && row.acquisition_type == 0)
                .ok_or_else(|| {
                    GameplayError::Invalid(format!(
                        "music {content_id} is not directly purchasable"
                    ))
                })?;
            let currency = currency_for_goods(&row.currency, row.area)?;
            let price = if row.unlock_cost <= 0.0 {
                0.0
            } else {
                scaled_requirement(row.unlock_cost, false)
            };
            let area = row.area;
            let max_level = i64::from(row.max_level.max(1));
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let result = state
                .database
                .execute(move |database| {
                    database.purchase_content(
                        &mutation,
                        ContentPurchaseCommand {
                            kind: ContentKind::Music,
                            area,
                            content_id,
                            currency,
                            price,
                            price_increment: 0.0,
                            max_level,
                        },
                    )
                })
                .await?;
            let music = sparse_struct_value(
                schema,
                "user_model",
                "MusicData",
                [("Music_idx".into(), Value::I32(to_i32(content_id)))],
            )?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("U_gold".into(), Value::I32(0)),
                    (
                        "U_cp".into(),
                        Value::I32(to_i32(amount_i64(result.chocolate))),
                    ),
                    ("U_mp".into(), Value::I32(to_i32(amount_i64(result.candy)))),
                    (
                        "Music".into(),
                        Value::Map {
                            key_type: I32,
                            value_type: STRUCT,
                            entries: vec![(Value::I32(to_i32(content_id)), music)],
                        },
                    ),
                ],
            )?)
        }
        "buyPackage" => {
            let identifiers = [
                request.integer("Store_idx").unwrap_or_default(),
                request.integer("Product_id").unwrap_or_default(),
            ];
            let mut contents = Vec::new();
            let mut music_ids = Vec::new();
            if identifiers.iter().any(|value| matches!(value, 5 | 12)) {
                contents.push((
                    ContentKind::Music,
                    content_area(state, ContentKind::Music, 12),
                    12,
                ));
                music_ids.push(12);
            }
            if identifiers.iter().any(|value| matches!(value, 6 | 10)) {
                contents.push((
                    ContentKind::Guitar,
                    content_area(state, ContentKind::Guitar, 6),
                    6,
                ));
                contents.push((
                    ContentKind::Costume,
                    content_area(state, ContentKind::Costume, 10),
                    10,
                ));
            }
            if identifiers.iter().any(|value| matches!(value, 19 | 211)) {
                contents.push((
                    ContentKind::Music,
                    content_area(state, ContentKind::Music, 211),
                    211,
                ));
                music_ids.push(211);
            }
            if identifiers
                .iter()
                .any(|value| matches!(value, 23 | 209 | 214))
            {
                contents.push((
                    ContentKind::Guitar,
                    content_area(state, ContentKind::Guitar, 214),
                    214,
                ));
                contents.push((
                    ContentKind::Costume,
                    content_area(state, ContentKind::Costume, 209),
                    209,
                ));
            }
            contents.sort_unstable_by_key(|(kind, area, id)| (*kind, *area, *id));
            contents.dedup();
            music_ids.sort_unstable();
            music_ids.dedup();
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let result = state
                .database
                .execute(move |database| {
                    database.grant_package_contents(&mutation, &contents, &music_ids)
                })
                .await?;
            let music = result
                .music_ids
                .iter()
                .map(|id| {
                    sparse_struct_value(
                        schema,
                        "user_model",
                        "BuyPackageMusic",
                        [("Music_idx".into(), Value::I64(*id))],
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            Some(response_struct(
                spec,
                schema,
                [
                    ("U_gold".into(), Value::I64(0)),
                    ("U_cp".into(), Value::I64(amount_i64(result.chocolate))),
                    ("U_mp".into(), Value::I64(amount_i64(result.candy))),
                    ("U_credit".into(), Value::I64(0)),
                    (
                        "Music_data".into(),
                        Value::List {
                            element_type: STRUCT,
                            values: music,
                        },
                    ),
                ],
            )?)
        }
        "buyShop" => {
            let shop_id = request.integer("Idx").unwrap_or(0);
            let shop = state
                .master
                .shop(shop_id)
                .ok_or_else(|| GameplayError::Invalid(format!("unknown shop item {shop_id}")))?;
            let ad_progression = (shop.product_type == 5
                && state
                    .master
                    .ad_levels
                    .iter()
                    .any(|row| row.group == shop.reward_group && row.active))
            .then(|| AdLevelProgression {
                group_id: shop.reward_group,
                thresholds: state
                    .master
                    .ad_levels
                    .iter()
                    .filter(|row| row.group == shop.reward_group && row.active)
                    .map(|row| (row.level, row.required_experience))
                    .collect(),
            });
            let reward_group = if let Some(progression) = &ad_progression {
                let group_id = progression.group_id;
                let usn = session.identity.usn;
                let current = state
                    .database
                    .execute(move |database| database.ad_level(usn, group_id))
                    .await?;
                state
                    .master
                    .ad_levels
                    .iter()
                    .find(|row| row.group == group_id && row.level == current.level && row.active)
                    .map_or(shop.reward_group, |row| row.reward_group)
            } else {
                shop.reward_group
            };
            let rewards: Vec<_> = state
                .master
                .rewards(reward_group)
                .map(|row| RewardGrant {
                    reward_type: row.reward_type,
                    reward_id: row.reward_id,
                    amount: row.quantity,
                })
                .collect();
            let permanent = rewards
                .iter()
                .any(|reward| matches!(reward.reward_type, 2..=5 | 7..=10));
            let receipt = request
                .string("Purchase_id")
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    request
                        .string("Purchase_token")
                        .filter(|value| !value.is_empty())
                });
            let purchase_id = receipt.map_or_else(
                || {
                    if permanent {
                        format!("shop-once:{shop_id}")
                    } else {
                        format!("shop:{shop_id}:{}", sequence.unwrap_or_default())
                    }
                },
                |value| format!("receipt:{value}"),
            );
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let result = state
                .database
                .execute(move |database| {
                    let mut outcome = database.purchase_rewards(
                        &mutation,
                        RewardPurchaseCommand {
                            transaction_id: purchase_id,
                            purchase_kind: "shop".into(),
                            target_id: shop_id,
                            cost: None,
                            rewards,
                            ad_progression,
                        },
                    )?;
                    // The tested Rust response includes User_ad_level even
                    // for non-AdLevel products (notably direct Like purchases).
                    // Project the existing free-Likes row without advancing it.
                    // Keep this read in the same actor job as the purchase so
                    // another request cannot interleave a different snapshot.
                    if outcome.ad_level.is_none() {
                        outcome.ad_level = Some(database.ad_level(mutation.identity.usn, 210_010)?);
                    }
                    Ok(outcome)
                })
                .await?;
            let reward_data = result
                .rewards
                .iter()
                .map(store_reward_wire_value)
                .collect::<Result<Vec<_>, _>>()?;
            let mut fields = vec![
                ("U_cp".into(), Value::I64(amount_i64(result.chocolate))),
                ("U_candy".into(), Value::Double(result.candy)),
                (
                    "Reward_data".into(),
                    Value::List {
                        element_type: STRUCT,
                        values: reward_data,
                    },
                ),
            ];
            if let Some(level) = result.ad_level {
                fields.push((
                    "User_ad_level".into(),
                    struct_value(
                        schema,
                        "user_model",
                        "UserAdLevel",
                        [
                            ("I_id".into(), Value::I32(to_i32(level.ad_level_id))),
                            ("I_Level".into(), Value::I32(level.level)),
                            ("I_EXP".into(), Value::I32(level.experience)),
                        ],
                    )?,
                ));
            }
            Some(response_sparse_struct(spec, schema, fields)?)
        }
        "paidEventPoint" => {
            let season = request.integer("I_SubscribeID").unwrap_or(0);
            let version = request
                .integer("I_Version")
                .unwrap_or(0)
                .clamp(0, i64::from(i32::MAX)) as i32;
            let fill_type = request.integer("Type").unwrap_or(0);
            let pass = state
                .master
                .pass_seasons
                .iter()
                .find(|row| row.subscribe_id == season)
                .ok_or_else(|| GameplayError::Invalid(format!("unknown pass {season}")))?;
            let policy = MemorialPolicy::default();
            let cost = if fill_type == 1 {
                pass.point_price as f64
            } else {
                0.0
            };
            let added = if fill_type == 1 {
                pass.paid_point
            } else {
                pass.ad_point
            }
            .saturating_mul(policy.pass_point_multiplier);
            let thresholds: Vec<_> = state
                .master
                .pass_rewards
                .iter()
                .filter(|row| row.group == season)
                .map(|row| (row.step, row.goal))
                .collect();
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let result = state
                .database
                .execute(move |database| {
                    database.fill_pass(&mutation, season, version, cost, added, &thresholds)
                })
                .await?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("U_cp".into(), Value::I64(amount_i64(result.chocolate))),
                    ("U_candy".into(), Value::Double(result.candy)),
                    ("I_SubscribeID".into(), Value::I64(result.season)),
                    ("I_Point".into(), Value::I64(result.points)),
                    ("I_Version".into(), Value::I32(result.version)),
                ],
            )?)
        }
        "setPassReward" => {
            let season = request.integer("Group").unwrap_or(0);
            let step = request.integer("Step").unwrap_or(-1) as i32;
            let lane = request.integer("Type").unwrap_or(-1) as i16;
            let version = request.integer("I_Version").unwrap_or(1).max(1) as i32;
            if !(0..=1).contains(&lane) {
                return Err(GameplayError::Invalid(format!("invalid pass lane {lane}")));
            }
            let row = state.master.pass_reward(season, step).ok_or_else(|| {
                GameplayError::Invalid(format!("unknown pass reward {season}:{step}"))
            })?;
            let group = if lane == 0 {
                row.free_reward_group
            } else {
                row.paid_reward_group
            };
            let rewards: Vec<_> = state
                .master
                .rewards(group)
                .map(|reward| RewardGrant {
                    reward_type: reward.reward_type,
                    reward_id: reward.reward_id,
                    amount: reward.quantity,
                })
                .collect();
            let pass = state
                .master
                .pass_seasons
                .iter()
                .find(|row| row.subscribe_id == season)
                .ok_or_else(|| GameplayError::Invalid(format!("unknown pass {season}")))?;
            let paid_steps = state
                .master
                .pass_rewards
                .iter()
                .filter(|candidate| candidate.group == season)
                .count() as i64;
            let required_points = row.goal;
            let ticket_id = pass.ticket_collection_id;
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let result = state
                .database
                .execute(move |database| {
                    database.claim_pass_reward(
                        &mutation,
                        season,
                        version,
                        lane,
                        step,
                        required_points,
                        &rewards,
                        paid_steps,
                        ticket_id,
                    )
                })
                .await?;
            let claim = sparse_struct_value(
                schema,
                "user_model",
                "UserSubscribePassReward",
                [
                    ("I_SubscribeID".into(), Value::I64(result.season)),
                    ("I_Type".into(), Value::I64(i64::from(result.lane))),
                    ("I_Step".into(), Value::I64(i64::from(result.step))),
                    ("I_UpdateTime".into(), Value::I64(result.claimed_at)),
                    ("I_Version".into(), Value::I32(result.version)),
                ],
            )?;
            let rewards = result
                .rewards
                .iter()
                .map(user_reward_wire_value)
                .collect::<Result<Vec<_>, _>>()?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("Subscribe_pass_reward".into(), claim),
                    (
                        "Reward_data".into(),
                        Value::List {
                            element_type: STRUCT,
                            values: rewards,
                        },
                    ),
                ],
            )?)
        }
        "setSubscribe" => {
            let seasons = request.integers("I_ids").unwrap_or_default();
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let active = state
                .database
                .execute(move |database| database.activate_passes(&mutation, &seasons))
                .await?;
            let values = active
                .into_iter()
                .map(|season| subscribe_wire_value(season, clock.unix_seconds))
                .collect::<Result<Vec<_>, _>>()?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("U_seq".into(), Value::I32(to_i32(session.identity.usn))),
                    (
                        "User_subscribe_list".into(),
                        Value::List {
                            element_type: STRUCT,
                            values,
                        },
                    ),
                ],
            )?)
        }
        "getPostTime" => {
            let usn = session.identity.usn;
            let mailbox = state
                .database
                .execute(move |database| database.mailbox(usn))
                .await?;
            Some(response_struct(
                spec,
                schema,
                [("Time".into(), Value::I64(mailbox.cursor))],
            )?)
        }
        "getPost" => {
            let usn = session.identity.usn;
            let mailbox = state
                .database
                .execute(move |database| database.mailbox(usn))
                .await?;
            let post_list = mailbox
                .mail
                .iter()
                .map(mail_wire_value)
                .collect::<Result<Vec<_>, _>>()?;
            Some(response_struct(
                spec,
                schema,
                [
                    ("Server_time".into(), Value::I64(clock.unix_seconds)),
                    (
                        "Post_list".into(),
                        Value::List {
                            element_type: STRUCT,
                            values: post_list,
                        },
                    ),
                ],
            )?)
        }
        "setAttendance" => {
            let mode = request.string("Type").unwrap_or("check").to_owned();
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let result = state
                .database
                .execute(move |database| database.attendance(&mutation, clock, &mode))
                .await?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("Status".into(), text(&result.status)),
                    (
                        "Attendance_count".into(),
                        Value::I32(result.attendance_count),
                    ),
                    (
                        "Attendance_date".into(),
                        Value::I32((device_datetime_number(clock) / 1_000_000) as i32),
                    ),
                    (
                        "Max_coutinuous_attendance_count".into(),
                        Value::I32(result.max_continuous_count),
                    ),
                ],
            )?)
        }
        "setReviewPopup" => {
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            state
                .database
                .execute(move |database| {
                    database.set_one_time_flag(&mutation, "review-popup".into(), 1)
                })
                .await?;
            Some(response_struct(
                spec,
                schema,
                [("Status".into(), text("OK"))],
            )?)
        }
        "updateAchievement" => {
            let achievement = request.integer("Achievement").unwrap_or(0);
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let achievement = state
                .database
                .execute(move |database| {
                    database.set_one_time_flag(
                        &mutation,
                        "achievement-popup-index".into(),
                        achievement,
                    )
                })
                .await?;
            Some(response_sparse_struct(
                spec,
                schema,
                [("Achievement".into(), Value::I16(achievement as i16))],
            )?)
        }
        "setGameReward" => {
            let raw_kind = request.string("Type").unwrap_or("").to_ascii_lowercase();
            let kind = if raw_kind == "dailymission" { "daily_mission".to_owned() } else { raw_kind };
            let id = request.integer("Id").unwrap_or(0);
            let level = request.integer("Level").unwrap_or(0) as i32;
            let quantity = request.string("S_quantity")
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0)
                .or_else(|| request.value("Quantity").and_then(|value| value_double(Some(value))))
                .unwrap_or(0.0);
            if kind == "music" {
                record_noop(state, mutation(session, rpc, sequence, clock, raw_request)?).await?;
                Some(response_sparse_struct(
                    spec,
                    schema,
                    [
                        ("Type".into(), text(&kind)),
                        ("Id".into(), Value::I32(id as i32)),
                        ("Level".into(), Value::I16(level as i16)),
                        ("Reward_type".into(), text("")),
                        ("Reward_value".into(), Value::I64(0)),
                        ("Status".into(), text("N")),
                    ],
                )?)
            } else {
                let (threshold, reward_type, reward_value, reward) = if kind == "achievement" {
                    let row = state
                        .master
                        .achievements
                        .iter()
                        .find(|row| row.id == id && row.active)
                        .ok_or_else(|| {
                            GameplayError::Invalid(format!("unknown achievement {id}"))
                        })?;
                    let index = usize::try_from(level - 1)
                        .ok()
                        .filter(|index| *index < row.conditions.len() && level <= row.max_level)
                        .ok_or_else(|| {
                            GameplayError::Invalid(format!(
                                "invalid achievement {id} level {level}"
                            ))
                        })?;
                    (
                        MemorialPolicy::embedded().achievement_requirement(id, row.conditions[index]),
                        row.reward_type.clone(),
                        row.reward_values[index],
                        reward_from_named_currency(&row.reward_type, row.reward_values[index] as f64),
                    )
                } else if kind == "daily_mission" {
                    let row = state
                        .master
                        .daily_missions
                        .iter()
                        .find(|row| row.id == id && row.active)
                        .ok_or_else(|| {
                            GameplayError::Invalid(format!("unknown daily mission {id}"))
                        })?;
                    let reward =
                        reward_from_named_currency(&row.reward_type, row.reward_value as f64);
                    (
                        row.condition,
                        row.reward_type.clone(),
                        row.reward_value,
                        reward,
                    )
                } else {
                    return Err(GameplayError::Invalid(format!(
                        "unknown game reward type {kind}"
                    )));
                };
                let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
                let kind_for_db = kind.clone();
                let outcome = state
                    .database
                    .execute(move |database| {
                        database.claim_game_reward(
                            &mutation,
                            GameRewardClaimCommand {
                                kind: kind_for_db,
                                id,
                                level,
                                quantity,
                                threshold,
                                local_day: clock.local_epoch_day(),
                                reward,
                            },
                        )
                    })
                    .await?;
                Some(response_sparse_struct(
                    spec,
                    schema,
                    [
                        ("Type".into(), text(&kind)),
                        ("Id".into(), Value::I32(id as i32)),
                        ("Level".into(), Value::I16(outcome.0 as i16)),
                        ("Reward_type".into(), text(&reward_type)),
                        ("Reward_value".into(), Value::I64(reward_value)),
                        ("Status".into(), text("Y")),
                    ],
                )?)
            }
        }
        "setAdReward" => {
            let ad_id = request.integer("I_id").unwrap_or(0);
            let ad = state
                .master
                .ads
                .iter()
                .find(|row| row.id == ad_id && row.active)
                .ok_or_else(|| GameplayError::Invalid(format!("unknown ad reward {ad_id}")))?;
            let mut rewards = reward_group(state, ad.reward_group, 1.0);
            let profile = if ad.reward_type.eq_ignore_ascii_case("PROFILE_EXP") {
                let profile_id = request
                    .string("Param1")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(1);
                Some(AdProfileGrant {
                    profile_id,
                    experience: 100,
                    level_thresholds: follower_profile_thresholds(state, profile_id),
                })
            } else {
                None
            };
            if ad.reward_type.eq_ignore_ascii_case("PROFILE_EXP") {
                rewards.clear();
            }
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let outcome = state
                .database
                .execute(move |database| {
                    database.grant_ad_reward(
                        &mutation,
                        ad_id,
                        clock.local_epoch_day(),
                        rewards,
                        profile,
                    )
                })
                .await?;
            let mut fields = vec![
                ("I_id".into(), Value::I32(ad_id as i32)),
                ("User_ad_list".into(), ad_state_wire_value(&outcome)?),
                (
                    "Reward_data".into(),
                    Value::List {
                        element_type: STRUCT,
                        values: outcome
                            .rewards
                            .iter()
                            .map(user_reward_wire_value)
                            .collect::<Result<_, _>>()?,
                    },
                ),
            ];
            if let Some(profile) = &outcome.profile {
                fields.push((
                    "User_follower_profile".into(),
                    ch3_profile_wire_value(profile)?,
                ));
            }
            Some(response_sparse_struct(spec, schema, fields)?)
        }
        "getSamSeckList" => {
            let values = state
                .master
                .sam_seck
                .iter()
                .filter(|row| row.active)
                .map(|row| {
                    let reward = sam_seck_reward(row.id).ok_or_else(|| {
                        GameplayError::Invalid(format!(
                            "missing memorial SamSeck reward {}",
                            row.id
                        ))
                    })?;
                    sam_seck_reward_wire_value(row.id, &reward).map_err(GameplayError::from)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("Event_type".into(), text("samseck")),
                    (
                        "RewardList".into(),
                        Value::List {
                            element_type: STRUCT,
                            values,
                        },
                    ),
                ],
            )?)
        }
        "getSamSeckReward" => {
            let event_id = request.integer("I_id").unwrap_or(0);
            let row = state
                .master
                .sam_seck
                .iter()
                .find(|row| row.id == event_id && row.active)
                .ok_or_else(|| {
                    GameplayError::Invalid(format!("unknown SamSeck reward {event_id}"))
                })?;
            let reward = sam_seck_reward(event_id).ok_or_else(|| {
                GameplayError::Invalid(format!("missing memorial SamSeck reward {event_id}"))
            })?;
            let post_id = row.mail_reward_id;
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let step = state
                .database
                .execute(move |database| {
                    database.claim_sam_seck(&mutation, event_id, post_id, reward)
                })
                .await?;
            Some(response_sparse_struct(
                spec,
                schema,
                [("Step".into(), Value::I16(step as i16))],
            )?)
        }
        "getEventRewardList" => {
            let group = request.integer("Event_idx").unwrap_or(0);
            let usn = session.identity.usn;
            let claimed = state
                .database
                .execute(move |database| database.claimed_event_rewards(usn))
                .await?;
            let rows = state
                .master
                .daily_rewards
                .iter()
                .filter(|row| row.group == group)
                .map(|row| event_reward_wire_value(row, claimed.contains(&row.id)))
                .collect::<Result<_, _>>()?;
            let value = sparse_struct_value(
                schema,
                "main_model",
                "GetEventRewardListRetDataInfo",
                [
                    (
                        "Reward_list".into(),
                        Value::List {
                            element_type: STRUCT,
                            values: rows,
                        },
                    ),
                    ("Group_idx".into(), Value::I32(group as i32)),
                ],
            )?;
            Some(Value::Map {
                key_type: I32,
                value_type: STRUCT,
                entries: vec![(Value::I32(group as i32), value)],
            })
        }
        "setEventReward" => {
            let row_id = request.integer("Event_idx").unwrap_or(0);
            let row = state
                .master
                .daily_rewards
                .iter()
                .find(|row| row.id == row_id)
                .ok_or_else(|| GameplayError::Invalid(format!("unknown event reward {row_id}")))?;
            let reward = RewardGrant {
                reward_type: row.reward_type,
                reward_id: row.reward_id,
                amount: row.quantity,
            };
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let outcome = state
                .database
                .execute(move |database| database.claim_event_reward(&mutation, row_id, reward))
                .await?;
            let rewards = if row.reward_type == 13 {
                outcome
                    .0
                    .iter()
                    .map(user_reward_wire_value)
                    .collect::<Result<_, _>>()?
            } else {
                Vec::new()
            };
            let mut fields = vec![
                ("U_cp".into(), Value::I64(amount_i64(outcome.1))),
                ("U_candy".into(), Value::Double(outcome.2)),
                ("U_like".into(), Value::Double(outcome.3)),
                ("U_fans".into(), Value::I64(amount_i64(outcome.4))),
                ("Reward_type".into(), Value::I16(row.reward_type)),
                ("Reward_id".into(), Value::I32(row.reward_id)),
                ("Reward_value".into(), Value::I32(row.quantity as i32)),
                ("Status".into(), text("Y")),
            ];
            if row.reward_type == 13 {
                fields.push((
                    "Reward_data".into(),
                    Value::List {
                        element_type: STRUCT,
                        values: rewards,
                    },
                ));
            }
            Some(response_sparse_struct(spec, schema, fields)?)
        }
        "setSelectReward" => {
            let selection_id = request.integer("I_id").unwrap_or(0);
            let row = state
                .master
                .select_rewards
                .iter()
                .find(|row| row.id == selection_id && row.active)
                .cloned()
                .ok_or_else(|| {
                    GameplayError::Invalid(format!("unknown select reward {selection_id}"))
                })?;
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let selected = state
                .database
                .execute(move |database| {
                    database.select_reward(
                        &mutation,
                        selection_id,
                        to_i32(row.group),
                        to_i32(row.reward_group),
                        to_i32(row.alternate_reward_group),
                    )
                })
                .await?;
            let selected = selected
                .iter()
                .map(select_reward_wire_value)
                .collect::<Result<Vec<_>, _>>()?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("U_seq".into(), Value::I32(to_i32(session.identity.usn))),
                    (
                        "User_select_reward_list".into(),
                        Value::List {
                            element_type: STRUCT,
                            values: selected,
                        },
                    ),
                ],
            )?)
        }
        "updateNickName" => {
            let requested = request.string("Nickname").unwrap_or("").to_owned();
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let nickname = state
                .database
                .execute(move |database| database.update_nickname(&mutation, &requested))
                .await?;
            Some(response_struct(
                spec,
                schema,
                [("Nickname".into(), text(&nickname))],
            )?)
        }
        "updateUserTitle" => {
            let title = request.integer("U_title").unwrap_or(0);
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let title = state
                .database
                .execute(move |database| database.update_user_title(&mutation, title))
                .await?;
            Some(response_struct(
                spec,
                schema,
                [("U_title".into(), Value::I16(to_i16(title)))],
            )?)
        }
        "setUserGP" => {
            let gp = request.integer("Gp").unwrap_or(0);
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let gp = state
                .database
                .execute(move |database| database.update_user_gp(&mutation, gp))
                .await?;
            Some(response_struct(
                spec,
                schema,
                [("Gp".into(), Value::I32(to_i32(gp)))],
            )?)
        }
        "setTutorial" => {
            let raw_step = request.string("Step").unwrap_or("").to_owned();
            let step = request
                .integer("Step")
                .or_else(|| tutorial_step_from_text(&raw_step))
                .unwrap_or(0)
                .clamp(0, i64::from(i16::MAX));
            let tutorial_ids: Vec<_> = (1..=step).collect();
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            state
                .database
                .execute(move |database| database.merge_tutorials(&mutation, &tutorial_ids))
                .await?;
            Some(response_struct(
                spec,
                schema,
                [("Step".into(), text(&raw_step))],
            )?)
        }
        "updateAvatar" => {
            let avatar = request.integer("U_avatar").unwrap_or(0);
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let avatar = state
                .database
                .execute(move |database| database.update_avatar(&mutation, avatar))
                .await?;
            Some(response_struct(
                spec,
                schema,
                [("U_avatar".into(), Value::I16(to_i16(avatar)))],
            )?)
        }
        "setTutorialNew" => {
            let tutorial_ids = request.integers("I_ids").unwrap_or_default();
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let tutorials = state
                .database
                .execute(move |database| database.merge_tutorials(&mutation, &tutorial_ids))
                .await?;
            let values = tutorials
                .into_iter()
                .map(|id| {
                    struct_value(
                        schema,
                        "user_model",
                        "UserTutorial",
                        [("I_id".into(), Value::I32(to_i32(id)))],
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            Some(response_struct(
                spec,
                schema,
                [
                    ("U_seq".into(), Value::I32(to_i32(session.identity.usn))),
                    (
                        "Tutorial".into(),
                        Value::List {
                            element_type: STRUCT,
                            values,
                        },
                    ),
                ],
            )?)
        }
        "userSave" => {
            let patch = parse_user_save(state, request);
            let core_fans = patch.core.as_ref().and_then(|core| core.fans);
            let core_like = patch.core.as_ref().and_then(|core| core.ch1_like);
            let area_likes = patch.areas.iter()
                .map(|area| format!("{}:{:?}", area.area, area.like_amount))
                .collect::<Vec<_>>().join(",");
            let area_fans = patch
                .areas
                .iter()
                .map(|area| format!("{}:{:?}", area.area, area.fans))
                .collect::<Vec<_>>()
                .join(",");
            crate::runtime_log(&format!(
                "userSave parsed: usn={} core_like={core_like:?} area_likes=[{area_likes}] core_fans={core_fans:?} area_fans=[{area_fans}] areas={} content={} achievements={} missions={} music={} skills={}",
                session.identity.usn,
                patch.areas.len(),
                patch.content.len(),
                patch.achievements.len(),
                patch.daily_missions.len(),
                patch.music_states.len(),
                patch.skill_activations.len(),
            ));
            let device_uuid = request.string("Device_uuid").unwrap_or("").to_owned();
            let pass_point_multiplier = MemorialPolicy::default().pass_point_multiplier;
            let pass_thresholds = state
                .master
                .pass_rewards
                .iter()
                .map(|row| (row.group, row.step, row.goal))
                .collect::<Vec<_>>();
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let status = state
                .database
                .execute(move |database| {
                    database.merge_user_save(
                        &mutation,
                        &patch,
                        &device_uuid,
                        pass_point_multiplier,
                        &pass_thresholds,
                    )
                })
                .await?;
            crate::runtime_log(&format!(
                "userSave committed: usn={} seq={} status={status}",
                session.identity.usn,
                sequence.unwrap_or_default(),
            ));
            Some(response_struct(
                spec,
                schema,
                [("Status".into(), text(&status))],
            )?)
        }
        "userPurchaseErrorLog" => {
            record_noop(state, mutation(session, rpc, sequence, clock, raw_request)?).await?;
            Some(response_struct(spec, schema, [("Ret".into(), text("Y"))])?)
        }
        "changeUser" => {
            let selector = request.string("Change_uuid").unwrap_or("").to_owned();
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            state
                .database
                .execute(move |database| database.request_switch_rpc(&mutation, &selector))
                .await?;
            Some(response_struct(
                spec,
                schema,
                [("Result".into(), text("Y"))],
            )?)
        }
        "userDel" => {
            record_noop(state, mutation(session, rpc, sequence, clock, raw_request)?).await?;
            Some(response_struct(
                spec,
                schema,
                [("Result".into(), text("N"))],
            )?)
        }
        "getChThird" => {
            let usn = session.identity.usn;
            let cap = ggfm_policy_memorial::memorial_ch3_energy_cap() as i32;
            let snapshot = state
                .database
                .execute(move |database| database.ch3_snapshot_at(usn, clock.unix_seconds, cap))
                .await?;
            tracing::info!(usn, stages = snapshot.stages.len(),
                first_stage = snapshot.stages.first().map(|row| row.stage_id),
                last_stage = snapshot.stages.last().map(|row| row.stage_id),
                profiles = snapshot.profiles.len(), ap = snapshot.energy.amount,
                ap_cap = snapshot.energy.cap, device_time = clock.unix_seconds,
                "ch3: restored entry snapshot from SQLite");
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("U_seq".into(), Value::I32(to_i32(usn))),
                    ("User_ap".into(), ch3_energy_wire_value(&snapshot.energy)?),
                    (
                        "User_ch_third_stage".into(),
                        Value::List {
                            element_type: STRUCT,
                            values: ch3_completed_stage_values(&snapshot.stages)?,
                        },
                    ),
                    (
                        "User_ch_third_chapter_reward".into(),
                        Value::List {
                            element_type: STRUCT,
                            values: state
                                .master
                                .ch3_chapters
                                .iter()
                                .filter(|chapter| chapter.active)
                                .map(|chapter| {
                                    ch3_chapter_wire_value(
                                        chapter.chapter,
                                        &snapshot.chapter_claims,
                                    )
                                })
                                .collect::<Result<_, _>>()?,
                        },
                    ),
                ],
            )?)
        }
        "chThirdStage" => {
            let stage_id = request
                .integer("I_id")
                .ok_or_else(|| GameplayError::Invalid("chThirdStage missing I_id".into()))?;
            let stage = state
                .master
                .ch3_stages
                .iter()
                .find(|row| row.id == stage_id && row.active)
                .ok_or_else(|| GameplayError::Invalid(format!("unknown CH3 stage {stage_id}")))?
                .clone();
            let usn = session.identity.usn;
            let snapshot = state
                .database
                .execute(move |database| database.player_snapshot(usn))
                .await?;
            let scoring = calculate_ch3_score(&state.master, &snapshot, &stage, request)?;
            let definitions: Vec<_> = state
                .master
                .ch3_stages
                .iter()
                .filter(|row| row.active)
                .collect();
            let position = definitions
                .iter()
                .position(|row| row.id == stage.id)
                .ok_or_else(|| GameplayError::Invalid("CH3 stage ordering is incomplete".into()))?;
            let next = definitions
                .get(position + 1)
                .map(|row| stage_definition(row));
            let rewards = if scoring.clear && !is_story(&stage) {
                ch3_rewards(
                    state,
                    &stage,
                    scoring.star,
                    &mutation_key(session, rpc, sequence, raw_request)?,
                )
            } else {
                Vec::new()
            };
            let thresholds = ch3_profile_thresholds(state);
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let definition = stage_definition(&stage);
            let outcome = state
                .database
                .execute(move |database| {
                    database.settle_ch3_stage(
                        &mutation,
                        definition,
                        next,
                        scoring.star,
                        scoring.total_score,
                        if scoring.story { 0 } else { 5 },
                        ggfm_policy_memorial::memorial_ch3_energy_cap() as i32,
                        rewards,
                        MemorialPolicy::default().ch3_profile_exp_per_clear,
                        thresholds,
                    )
                })
                .await?;
            let music_level = snapshot
                .content_levels
                .iter()
                .find(|row| row.kind == ContentKind::Music && row.content_id == scoring.music_id)
                .map_or(1, |row| row.level);
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("Star".into(), Value::I32(scoring.star)),
                    (
                        "Character_score".into(),
                        Value::I32(scoring.character_score),
                    ),
                    ("Music_score".into(), Value::I32(scoring.music_score)),
                    (
                        "Follower_profile_score".into(),
                        Value::I32(scoring.follower_score),
                    ),
                    ("Bonus_score".into(), Value::I32(scoring.bonus_score)),
                    ("Total_score".into(), Value::I64(scoring.total_score)),
                    ("Clear".into(), Value::I32(i32::from(scoring.clear))),
                    ("User_ap".into(), ch3_energy_wire_value(&outcome.energy)?),
                    (
                        "User_ch_third_stage".into(),
                        ch3_stage_wire_value(&outcome.stage)?,
                    ),
                    (
                        "User_music".into(),
                        ch3_music_wire_value(scoring.music_id, music_level)?,
                    ),
                    (
                        "Reward_data".into(),
                        Value::List {
                            element_type: STRUCT,
                            values: outcome
                                .rewards
                                .iter()
                                .map(user_reward_wire_value)
                                .collect::<Result<_, _>>()?,
                        },
                    ),
                    (
                        "Bonus_follower_profile_ids".into(),
                        Value::List {
                            element_type: I32,
                            values: scoring
                                .bonus_profile_ids
                                .iter()
                                .copied()
                                .map(Value::I32)
                                .collect(),
                        },
                    ),
                    (
                        "User_follower_profile".into(),
                        ch3_profile_wire_value(&outcome.profile)?,
                    ),
                    (
                        "Bonus_music_score".into(),
                        Value::I32(scoring.bonus_music_score),
                    ),
                    (
                        "User_follower_profile_score".into(),
                        Value::List {
                            element_type: STRUCT,
                            values: scoring
                                .profile_scores
                                .iter()
                                .map(ch3_profile_score_wire_value)
                                .collect::<Result<_, _>>()?,
                        },
                    ),
                ],
            )?)
        }
        "chThirdStreamStage" => {
            let stage_id = request.integer("I_id").unwrap_or(0);
            let count = request.integer("Count").unwrap_or(0) as i32;
            if !(1..=5).contains(&count) {
                return Err(GameplayError::Invalid(format!(
                    "CH3 repeat count {count} is outside 1..=5"
                )));
            }
            let stage = state
                .master
                .ch3_stages
                .iter()
                .find(|row| row.id == stage_id && row.active && !is_story(row))
                .ok_or_else(|| {
                    GameplayError::Invalid(format!("CH3 stage {stage_id} cannot be repeated"))
                })?;
            let usn = session.identity.usn;
            let snapshot = state
                .database
                .execute(move |database| database.player_snapshot(usn))
                .await?;
            let best_star = snapshot
                .ch3
                .stages
                .iter()
                .find(|row| row.stage_id == stage_id && row.completed)
                .map_or(0, |row| row.best_star);
            if best_star <= 0 {
                return Err(GameplayError::Invalid(format!(
                    "CH3 stage {stage_id} has not been cleared"
                )));
            }
            let base_seed = mutation_key(session, rpc, sequence, raw_request)?;
            let reward_rolls: Vec<_> = (1..=count)
                .map(|roll| {
                    (
                        roll,
                        ch3_rewards(state, stage, best_star, &format!("{base_seed}:{roll}")),
                    )
                })
                .collect();
            let thresholds = ch3_profile_thresholds(state);
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let outcome = state
                .database
                .execute(move |database| {
                    database.settle_ch3_stream(
                        &mutation,
                        stage_id,
                        count,
                        5,
                        ggfm_policy_memorial::memorial_ch3_energy_cap() as i32,
                        reward_rolls,
                        MemorialPolicy::default().ch3_profile_exp_per_clear,
                        thresholds,
                    )
                })
                .await?;
            let entries = outcome
                .reward_rolls
                .iter()
                .map(|(roll, rewards)| {
                    Ok((
                        Value::I32(*roll),
                        Value::List {
                            element_type: STRUCT,
                            values: rewards
                                .iter()
                                .map(user_reward_wire_value)
                                .collect::<Result<_, _>>()?,
                        },
                    ))
                })
                .collect::<Result<Vec<_>, ProtocolEnvelopeError>>()?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("I_id".into(), Value::I32(stage_id as i32)),
                    ("User_ap".into(), ch3_energy_wire_value(&outcome.energy)?),
                    (
                        "User_follower_profile".into(),
                        ch3_profile_wire_value(&outcome.profile)?,
                    ),
                    (
                        "Reward_data".into(),
                        Value::Map {
                            key_type: I32,
                            value_type: LIST,
                            entries,
                        },
                    ),
                ],
            )?)
        }
        "getChThirdStarReward" => {
            let chapter_id = request.integer("I_id").unwrap_or(0) as i32;
            let reward_num = request.integer("Reward_num").unwrap_or(0) as i32;
            let chapter = state
                .master
                .ch3_chapters
                .iter()
                .find(|row| row.chapter == chapter_id && row.active)
                .ok_or_else(|| {
                    GameplayError::Invalid(format!("unknown CH3 chapter {chapter_id}"))
                })?;
            let index = usize::try_from(reward_num - 1)
                .ok()
                .filter(|index| *index < 3)
                .ok_or_else(|| {
                    GameplayError::Invalid(format!("invalid CH3 chapter reward {reward_num}"))
                })?;
            let rewards = reward_group(state, chapter.reward_groups[index], 1.0);
            let goal = chapter.goal_stars[index];
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let outcome = state
                .database
                .execute(move |database| {
                    database.claim_ch3_chapter_reward(
                        &mutation,
                        chapter_id,
                        reward_num,
                        goal,
                        rewards,
                        ggfm_policy_memorial::memorial_ch3_energy_cap() as i32,
                    )
                })
                .await?;
            Some(response_sparse_struct(
                spec,
                schema,
                [
                    ("I_id".into(), Value::I32(chapter_id)),
                    ("Reward_num".into(), Value::I32(reward_num)),
                    ("User_ap".into(), ch3_energy_wire_value(&outcome.0)?),
                    (
                        "Reward_data".into(),
                        Value::List {
                            element_type: STRUCT,
                            values: outcome
                                .1
                                .iter()
                                .map(user_reward_wire_value)
                                .collect::<Result<_, _>>()?,
                        },
                    ),
                ],
            )?)
        }
        "readPost" => {
            let post_id = request.integer("Idx").unwrap_or(0);
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let post_id = state
                .database
                .execute(move |database| database.read_mail(&mutation, post_id))
                .await?;
            Some(response_struct(
                spec,
                schema,
                [("Idx".into(), Value::I64(post_id))],
            )?)
        }
        "providePost" => {
            let post_id = request.integer("Idx").unwrap_or(0);
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let reward = state
                .database
                .execute(move |database| database.claim_mail_rpc(&mutation, post_id))
                .await?;
            let values = reward
                .iter()
                .map(post_reward_wire_value)
                .collect::<Result<Vec<_>, _>>()?;
            Some(Value::List {
                element_type: STRUCT,
                values,
            })
        }
        "deletePost" => {
            let post_id = request.integer("Idx").unwrap_or(0);
            let delete_all = request.integer("Type").unwrap_or(0) == 1;
            let mutation = mutation(session, rpc, sequence, clock, raw_request)?;
            let post_id = state
                .database
                .execute(move |database| database.delete_mail(&mutation, post_id, delete_all))
                .await?;
            Some(response_struct(
                spec,
                schema,
                [("Idx".into(), Value::I64(post_id))],
            )?)
        }
        _ if spec.rpc_class() != RpcClass::Active => {
            compatibility_baseline_response(spec, schema, session)?
        }
        _ => None,
    };
    Ok(data)
}

/// Typed, side-effect-free responses for calls whose gameplay path was not
/// observed in 8.0.0. Their wire behavior still comes from the tested Rust
/// baseline; "compatibility" means no mutation, not an all-zero placeholder.
fn compatibility_baseline_response(
    spec: &ggfm_protocol::RpcSpec,
    schema: &ProtocolSchema,
    session: &LoginSession,
) -> Result<Option<Value>, ProtocolEnvelopeError> {
    let singleton_list =
        |namespace: &str, wire_type: &str| -> Result<Value, ProtocolEnvelopeError> {
            Ok(Value::List {
                element_type: STRUCT,
                values: vec![baseline_compatibility_value(
                    schema,
                    namespace,
                    wire_type,
                    "Data",
                    Some(1),
                    session,
                    0,
                )?],
            })
        };
    let singleton_map =
        |namespace: &str, wire_type: &str, key_type: u8| -> Result<Value, ProtocolEnvelopeError> {
            let key = if key_type == I16 {
                Value::I16(1)
            } else {
                Value::I32(1)
            };
            Ok(Value::Map {
                key_type,
                value_type: STRUCT,
                entries: vec![(
                    key,
                    baseline_compatibility_value(
                        schema,
                        namespace,
                        wire_type,
                        "Data",
                        Some(1),
                        session,
                        0,
                    )?,
                )],
            })
        };
    Ok(Some(match spec.name.as_str() {
        "getAlbum" => singleton_list("music_model", "GetAlbumRetDataInfo")?,
        "getAlbumList" => singleton_list("music_model", "GetAlbumListRetDataInfo")?,
        "getMusicList" => singleton_list("music_model", "GetMusicListRetDataInfo")?,
        "getProfileMusic" => singleton_list("user_model", "GetProfileMusicRetDataInfo")?,
        "itemStoreList" => singleton_list("store_model", "ItemStoreListRetDataInfo")?,
        "getAlbumInfoList" => singleton_map("music_model", "GetAlbumInfoListRetDataInfo", I32)?,
        "getMusicInfoList" => singleton_map("music_model", "GetMusicInfoListRetDataInfo", I32)?,
        "getTabList" => singleton_map("main_model", "GetTabListRetDataInfo", I16)?,
        "newStoreList" => singleton_map("main_model", "NewStoreListRetDataInfo", I16)?,
        "storeList" => singleton_map("main_model", "StoreListRetDataInfo", I16)?,
        "userAvatarList" => response_struct(
            spec,
            schema,
            [
                ("U_seq".into(), Value::I32(to_i32(session.identity.usn))),
                (
                    "Avatar".into(),
                    Value::List {
                        element_type: I32,
                        values: vec![Value::I32(1)],
                    },
                ),
            ],
        )?,
        "userPurchase" => response_struct(
            spec,
            schema,
            [(
                "Music_data".into(),
                singleton_list("user_model", "UserPurchaseMusicIdx")?,
            )],
        )?,
        _ => {
            let wire = spec
                .response_wires
                .first()
                .ok_or_else(|| ProtocolEnvelopeError::MissingWire(spec.name.clone()))?;
            baseline_compatibility_value(
                schema,
                &wire.namespace,
                &wire.wire_type,
                "Data",
                None,
                session,
                0,
            )?
        }
    }))
}

fn baseline_compatibility_value(
    schema: &ProtocolSchema,
    namespace: &str,
    wire_type: &str,
    field_name: &str,
    entry_id: Option<i64>,
    session: &LoginSession,
    depth: usize,
) -> Result<Value, ProtocolEnvelopeError> {
    if depth > 32 {
        return Err(ProtocolEnvelopeError::RecursionLimit(wire_type.to_owned()));
    }
    let wire_type = wire_type.trim().trim_start_matches('*');
    if let Some(element) = wire_type.strip_prefix("[]") {
        return Ok(Value::List {
            element_type: compatibility_wire_kind(schema, namespace, element)?,
            values: Vec::new(),
        });
    }
    if let Some((key, value)) = compatibility_split_map(wire_type) {
        return Ok(Value::Map {
            key_type: compatibility_wire_kind(schema, namespace, key)?,
            value_type: compatibility_wire_kind(schema, namespace, value)?,
            entries: Vec::new(),
        });
    }
    let normalized = field_name.to_ascii_lowercase();
    Ok(match wire_type {
        "bool" => Value::Bool(!matches!(
            normalized.as_str(),
            "is_error" | "error" | "failed" | "is_failed" | "maintenance"
        )),
        "byte" | "int8" | "uint8" => Value::Byte(
            baseline_compatibility_integer(&normalized, entry_id, session)
                .clamp(i64::from(i8::MIN), i64::from(i8::MAX)) as i8,
        ),
        "int16" | "uint16" => Value::I16(to_i16(baseline_compatibility_integer(
            &normalized,
            entry_id,
            session,
        ))),
        "int" | "int32" | "uint" | "uint32" => Value::I32(to_i32(baseline_compatibility_integer(
            &normalized,
            entry_id,
            session,
        ))),
        "int64" | "uint64" => Value::I64(baseline_compatibility_integer(
            &normalized,
            entry_id,
            session,
        )),
        "float32" | "float64" => Value::Double(if normalized.contains("quantity") {
            1.0
        } else {
            0.0
        }),
        "string" => text(&baseline_compatibility_string(&normalized, session)),
        "any" | "interface{}" | "interface {}" => Value::Struct(Vec::new()),
        named => {
            let (resolved_namespace, resolved_name) =
                named.rsplit_once('.').unwrap_or((namespace, named));
            let type_spec = schema
                .type_spec(&format!("{resolved_namespace}::{resolved_name}"))
                .ok_or_else(|| {
                    ProtocolEnvelopeError::MissingType(format!(
                        "{resolved_namespace}::{resolved_name}"
                    ))
                })?;
            let mut fields = Vec::with_capacity(type_spec.fields.len());
            for field in &type_spec.fields {
                fields.push(ggfm_protocol::Field {
                    id: field.id,
                    value: baseline_compatibility_value(
                        schema,
                        &type_spec.namespace,
                        &field.wire_type,
                        &field.name,
                        entry_id,
                        session,
                        depth + 1,
                    )?,
                });
            }
            Value::Struct(fields)
        }
    })
}

fn baseline_compatibility_integer(
    field: &str,
    entry_id: Option<i64>,
    session: &LoginSession,
) -> i64 {
    match field {
        "u_seq" | "user_seq" => session.identity.usn,
        "u_fans_grade" | "i_userfangrade" => 1,
        "u_avatar" | "avatar_idx" | "u_title" | "title_idx" => 1,
        "is_owner" | "is_purchased" | "is_buy" | "buy_flg" => 1,
        "server_time" | "update_time" | "i_updatetime" => session.clock.unix_seconds,
        "start_time" | "i_activetime" | "i_purchasetime" => {
            session.clock.unix_seconds.saturating_sub(60)
        }
        "end_time" | "i_endtime" => session
            .clock
            .unix_seconds
            .saturating_add(365 * 24 * 60 * 60),
        value if value.contains("level") || value.ends_with("_grade") || value == "grade" => 1,
        value if compatibility_identifier(value) => entry_id.unwrap_or(1),
        value
            if value.contains("reward_value")
                || value.contains("quantity")
                || value.ends_with("_count")
                || value.ends_with("_cnt")
                || value.starts_with("is_")
                || value.starts_with("i_is")
                || value.starts_with("b_is") =>
        {
            1
        }
        "result" | "status" | "success" => 1,
        _ => 0,
    }
}

fn baseline_compatibility_string(field: &str, session: &LoginSession) -> String {
    match field {
        "u_id" | "userid" | "user_id" => session.identity.user_id.clone(),
        "u_name" | "u_nick" | "nickname" | "nick_name" => "Guitar Girl".to_owned(),
        "status" | "result" | "ret" | "success" | "is_success" => "Y".to_owned(),
        "u_review_popup" => "N".to_owned(),
        "u_create_time" => session.clock.unix_seconds.to_string(),
        "u_last_login" => "2026-09-03 00:00:00".to_owned(),
        "u_country" | "country" | "country_code" | "u_state" => "US".to_owned(),
        "buy_datetime" | "reg_datetime" | "update_datetime" => "2099-12-31 23:59:59".to_owned(),
        "device_uuid" => String::new(),
        "transfer_id" => format!("reborn-{}", session.identity.usn),
        "mode" | "sub_mode" | "s_type" => "normal".to_owned(),
        value if value.contains("title") || value.contains("name") => "Reborn".to_owned(),
        value if value.contains("description") || value.starts_with("desc_") => {
            "Restored offline service".to_owned()
        }
        _ => String::new(),
    }
}

fn compatibility_identifier(field: &str) -> bool {
    field == "id"
        || field == "idx"
        || field == "i_id"
        || field == "u_area_num"
        || field.ends_with("_id")
        || field.ends_with("_idx")
        || field.ends_with("id")
        || field.ends_with("idx")
}

fn compatibility_split_map(wire_type: &str) -> Option<(&str, &str)> {
    let rest = wire_type.strip_prefix("map[")?;
    let end = rest.find(']')?;
    Some((&rest[..end], &rest[end + 1..]))
}

fn compatibility_wire_kind(
    schema: &ProtocolSchema,
    namespace: &str,
    wire_type: &str,
) -> Result<u8, ProtocolEnvelopeError> {
    let wire_type = wire_type.trim().trim_start_matches('*');
    Ok(if wire_type.starts_with("[]") {
        LIST
    } else if wire_type.starts_with("map[") {
        ggfm_protocol::MAP
    } else {
        match wire_type {
            "bool" => ggfm_protocol::BOOL,
            "byte" | "int8" | "uint8" => ggfm_protocol::BYTE,
            "int16" | "uint16" => I16,
            "int" | "int32" | "uint" | "uint32" => I32,
            "int64" | "uint64" => ggfm_protocol::I64,
            "float32" | "float64" => ggfm_protocol::DOUBLE,
            "string" => STRING,
            named => {
                let (resolved_namespace, resolved_name) =
                    named.rsplit_once('.').unwrap_or((namespace, named));
                schema
                    .type_spec(&format!("{resolved_namespace}::{resolved_name}"))
                    .ok_or_else(|| {
                        ProtocolEnvelopeError::MissingType(format!(
                            "{resolved_namespace}::{resolved_name}"
                        ))
                    })?;
                STRUCT
            }
        }
    })
}

#[derive(Clone, Debug)]
struct Ch3ProfileScore {
    profile_id: i32,
    score: i32,
    bonus_score: i32,
}

fn nested_field(value: &Value, id: i16) -> Option<&Value> {
    let Value::Struct(fields) = value else {
        return None;
    };
    fields
        .iter()
        .find(|field| field.id == id)
        .map(|field| &field.value)
}

fn value_integer(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Byte(value) => Some(i64::from(*value)),
        Value::I16(value) => Some(i64::from(*value)),
        Value::I32(value) => Some(i64::from(*value)),
        Value::I64(value) => Some(*value),
        _ => None,
    }
}

fn value_double(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Double(value) => Some(*value),
        value => value_integer(Some(value)).map(|number| number as f64),
    }
}

fn value_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => String::from_utf8(value.clone()).ok(),
        _ => None,
    }
}

fn parse_music_reviews(request: &NamedRequest) -> Result<Vec<MusicReviewSnapshot>, GameplayError> {
    request
        .list("Review_point")
        .unwrap_or(&[])
        .iter()
        .map(|value| {
            Ok(MusicReviewSnapshot {
                music_id: value_integer(nested_field(value, 1))
                    .ok_or_else(|| GameplayError::Invalid("review row missing Music_idx".into()))?
                    as i32,
                point: value_double(nested_field(value, 2))
                    .ok_or_else(|| GameplayError::Invalid("review row missing Point".into()))?,
                difficulty: value_integer(nested_field(value, 3))
                    .ok_or_else(|| GameplayError::Invalid("review row missing Difficulty".into()))?
                    as i16,
            })
        })
        .collect()
}

fn parse_bookmarks(request: &NamedRequest) -> Result<Vec<BookmarkSnapshot>, GameplayError> {
    request
        .list("Bookmark_list")
        .unwrap_or(&[])
        .iter()
        .map(|value| {
            let flag = value_integer(nested_field(value, 2))
                .ok_or_else(|| GameplayError::Invalid("bookmark row missing Flag".into()))?
                as i16;
            if !matches!(flag, 0 | 1) {
                return Err(GameplayError::Invalid(format!(
                    "invalid bookmark flag {flag}"
                )));
            }
            Ok(BookmarkSnapshot {
                music_id: value_integer(nested_field(value, 1)).ok_or_else(|| {
                    GameplayError::Invalid("bookmark row missing Music_idx".into())
                })? as i32,
                flag,
            })
        })
        .collect()
}

fn parse_music_scores(request: &NamedRequest) -> Result<Vec<MusicScoreSnapshot>, GameplayError> {
    request
        .list("Score_list")
        .unwrap_or(&[])
        .iter()
        .map(|value| {
            Ok(MusicScoreSnapshot {
                music_id: value_integer(nested_field(value, 1)).unwrap_or(0) as i32,
                score: value_integer(nested_field(value, 2)).unwrap_or(0) as i32,
                grade: value_integer(nested_field(value, 3)).unwrap_or(0) as i16,
                play_count: value_integer(nested_field(value, 4)).unwrap_or(0) as i32,
                difficulty: value_integer(nested_field(value, 5)).unwrap_or(0) as i16,
                multiple: value_double(nested_field(value, 6)).unwrap_or(1.0),
                gp: value_integer(nested_field(value, 7)).unwrap_or(0) as i32,
                achievement: value_integer(nested_field(value, 8)).unwrap_or(0) as i16,
                play_type: value_integer(nested_field(value, 9)).unwrap_or(0) as i16,
                played_at: value_text(nested_field(value, 10)).unwrap_or_default(),
                mission_clear: value_integer(nested_field(value, 11)).unwrap_or(0) as i16,
                combo_grade: value_integer(nested_field(value, 12)).unwrap_or(0) as i16,
                is_credit: value_integer(nested_field(value, 13)).unwrap_or(0) as i16,
            })
        })
        .collect()
}

fn music_review_wire_value(row: &MusicReviewSnapshot) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "MusicPointReviewRetDataInfo",
        [
            ("Music_idx".into(), Value::I32(row.music_id)),
            ("Point".into(), Value::Double(row.point)),
            ("Difficulty".into(), Value::I16(row.difficulty)),
        ],
    )
}

fn bookmark_wire_value(row: &BookmarkSnapshot) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "SetBookmarkRetDataInfo",
        [
            ("Music_idx".into(), Value::I32(row.music_id)),
            ("Flag".into(), Value::I16(row.flag)),
        ],
    )
}

fn music_score_wire_value(row: &MusicScoreSnapshot) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "UpdateScoreDataList",
        [
            ("Music_idx".into(), Value::I32(row.music_id)),
            ("Score".into(), Value::I32(row.score)),
            ("Grade".into(), Value::I16(row.grade)),
            ("Play_cnt".into(), Value::I32(row.play_count)),
            ("Difficulty".into(), Value::I16(row.difficulty)),
            ("Combo_grade".into(), Value::I16(row.combo_grade)),
            (
                "Collection".into(),
                Value::List {
                    element_type: STRUCT,
                    values: Vec::new(),
                },
            ),
        ],
    )
}

fn follower_profile_thresholds(state: &AppState, profile_id: i64) -> Vec<(i32, i64, i32)> {
    state
        .master
        .follower_profile_levels
        .iter()
        .filter(|row| row.active && row.profile_id == profile_id)
        .map(|row| (row.level, row.required_exp.max(0.0) as i64, row.add_candy))
        .collect()
}

fn follower_gift_wire_value(gift_id: i64, remaining: i64) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "UserFollowerGiftItem",
        [
            ("I_id".into(), Value::I32(gift_id as i32)),
            ("I_Value".into(), Value::I32(remaining as i32)),
        ],
    )
}

fn follower_profile_reward_wire_value(
    outcome: &ggfm_domain::FollowerProfileRewardOutcome,
) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "SetUserFollowerProfileRewardRetDataInfo",
        [
            ("I_id".into(), Value::I32(outcome.profile_id as i32)),
            ("I_level".into(), Value::I32(outcome.level)),
            (
                "Reward_data".into(),
                Value::List {
                    element_type: STRUCT,
                    values: outcome
                        .rewards
                        .iter()
                        .map(user_reward_wire_value)
                        .collect::<Result<_, _>>()?,
                },
            ),
            (
                "User_follower_profile".into(),
                ch3_profile_wire_value(&outcome.profile)?,
            ),
        ],
    )
}

fn follower_quest_response(
    spec: &ggfm_protocol::RpcSpec,
    outcome: &ggfm_domain::FollowerQuestOutcome,
) -> Result<Value, ProtocolEnvelopeError> {
    let quest = &outcome.quest;
    response_sparse_struct(
        spec,
        ProtocolSchema::embedded(),
        [
            ("I_CurrentID".into(), Value::I32(quest.current_id as i32)),
            (
                "I_CompleteID".into(),
                Value::I32(outcome.complete_id as i32),
            ),
            (
                "D_ConditionValue1".into(),
                Value::Double(quest.condition_values[0]),
            ),
            (
                "D_ConditionValue2".into(),
                Value::Double(quest.condition_values[1]),
            ),
            (
                "D_ConditionValue3".into(),
                Value::Double(quest.condition_values[2]),
            ),
            (
                "I_RewardReceived1".into(),
                Value::I16(i16::from(quest.claimed[0])),
            ),
            (
                "I_RewardReceived2".into(),
                Value::I16(i16::from(quest.claimed[1])),
            ),
            (
                "I_RewardReceived3".into(),
                Value::I16(i16::from(quest.claimed[2])),
            ),
            ("Next_flg".into(), Value::I16(i16::from(outcome.next))),
            (
                "Reward_data".into(),
                Value::List {
                    element_type: STRUCT,
                    values: outcome
                        .rewards
                        .iter()
                        .map(user_reward_wire_value)
                        .collect::<Result<_, _>>()?,
                },
            ),
        ],
    )
}

fn follower_quest_wire_value(
    quest: &ggfm_domain::FollowerQuestSnapshot,
) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "UserFollowerQuest",
        [
            ("I_id".into(), Value::I64(1)),
            ("I_CurrentID".into(), Value::I64(quest.current_id)),
            (
                "I_CompleteID".into(),
                Value::I64(quest.complete_id),
            ),
            (
                "D_ConditionValue1".into(),
                Value::Double(quest.condition_values[0]),
            ),
            (
                "D_ConditionValue2".into(),
                Value::Double(quest.condition_values[1]),
            ),
            (
                "D_ConditionValue3".into(),
                Value::Double(quest.condition_values[2]),
            ),
            (
                "I_RewardReceived1".into(),
                Value::I16(i16::from(quest.claimed[0])),
            ),
            (
                "I_RewardReceived2".into(),
                Value::I16(i16::from(quest.claimed[1])),
            ),
            (
                "I_RewardReceived3".into(),
                Value::I16(i16::from(quest.claimed[2])),
            ),
            ("I_isInfinity".into(), Value::I16(i16::from(quest.infinite))),
        ],
    )
}

fn follower_quest_wire_values(
    quests: &[ggfm_domain::FollowerQuestSnapshot],
) -> Result<Vec<Value>, ProtocolEnvelopeError> {
    quests
        .iter()
        // Persist every stage independently, but project the newest one into
        // the stock client's single mutable row (I_id=1).
        .max_by_key(|quest| quest.current_id)
        .map(follower_quest_wire_value)
        .into_iter()
        .collect()
}

fn ad_state_wire_value(
    outcome: &ggfm_domain::AdRewardOutcome,
) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "UserAdList",
        [
            ("I_id".into(), Value::I32(outcome.ad_id as i32)),
            ("I_Count".into(), Value::I32(outcome.day_count)),
            ("I_TotalCount".into(), Value::I32(outcome.total_count)),
            (
                "I_LastViewTime".into(),
                Value::I32(outcome.last_view_time as i32),
            ),
        ],
    )
}

fn sam_seck_reward(event_id: i64) -> Option<RewardGrant> {
    Some(match event_id {
        1 => RewardGrant {
            reward_type: 1,
            reward_id: 2,
            amount: 100.0,
        },
        2 => RewardGrant {
            reward_type: 9,
            reward_id: 10,
            amount: 1.0,
        },
        3 => RewardGrant {
            reward_type: 3,
            reward_id: 13,
            amount: 1.0,
        },
        _ => return None,
    })
}

fn sam_seck_reward_wire_value(
    event_id: i64,
    reward: &RewardGrant,
) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "eventMode_model",
        "RewardListData",
        [
            ("I_id".into(), Value::I64(event_id)),
            (
                "I_RewardType".into(),
                Value::I32(i32::from(reward.reward_type)),
            ),
            ("I_RewardId".into(), Value::I32(reward.reward_id)),
            ("D_RewardQuantity".into(), Value::Double(reward.amount)),
        ],
    )
}

fn event_reward_wire_value(
    row: &ggfm_master_data::DailyRewardRow,
    claimed: bool,
) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "main_model",
        "GetEventRewardListData",
        [
            ("Idx".into(), Value::I64(row.id)),
            ("Event_name".into(), text("Guitar Girl Memorial")),
            ("Event_type".into(), text("DailyReward")),
            ("Reward_idx".into(), Value::I64(row.id)),
            ("Reward_num".into(), Value::I32(row.day)),
            ("Reward_type".into(), Value::I32(i32::from(row.reward_type))),
            ("Reward_id".into(), Value::I32(row.reward_id)),
            ("Reward_value".into(), Value::I32(row.quantity as i32)),
            ("Reward_flg".into(), text(if claimed { "Y" } else { "N" })),
            ("Get_date".into(), Value::I32(row.day)),
            ("S_CustomIconType".into(), text(&row.custom_icon_type)),
            ("S_CustomIconSprite".into(), text(&row.custom_icon_sprite)),
        ],
    )
}

#[derive(Clone, Debug)]
struct Ch3ScoreResult {
    story: bool,
    clear: bool,
    star: i32,
    music_id: i64,
    character_score: i32,
    music_score: i32,
    follower_score: i32,
    bonus_score: i32,
    bonus_music_score: i32,
    total_score: i64,
    bonus_profile_ids: Vec<i32>,
    profile_scores: Vec<Ch3ProfileScore>,
}

fn is_story(stage: &ggfm_master_data::Ch3StageRow) -> bool {
    stage.stage_type.eq_ignore_ascii_case("story")
}

fn stage_definition(stage: &ggfm_master_data::Ch3StageRow) -> Ch3StageDefinition {
    Ch3StageDefinition {
        stage_id: stage.id,
        chapter: stage.chapter,
        stage_index: stage.stage_index,
        story: is_story(stage),
    }
}

fn calculate_ch3_score(
    master: &ggfm_master_data::MasterCatalog,
    snapshot: &PlayerSnapshot,
    stage: &ggfm_master_data::Ch3StageRow,
    request: &NamedRequest,
) -> Result<Ch3ScoreResult, GameplayError> {
    if is_story(stage) {
        return Ok(Ch3ScoreResult {
            story: true,
            clear: true,
            star: 0,
            music_id: 0,
            character_score: 0,
            music_score: 0,
            follower_score: 0,
            bonus_score: 0,
            bonus_music_score: 0,
            total_score: 0,
            bonus_profile_ids: Vec::new(),
            profile_scores: Vec::new(),
        });
    }
    let music_id = request
        .integer("Music_id")
        .filter(|id| *id > 0)
        .unwrap_or_else(|| snapshot.areas.iter().find(|area| area.area == 1)
            .and_then(|area| area.current_music).unwrap_or(1).max(1));
    let lily_level = snapshot
        .ch3
        .profiles
        .iter()
        .find(|profile| profile.profile_id == 100_000)
        .map_or(1, |profile| profile.level);
    let music_level = snapshot
        .content_levels
        .iter()
        .find(|row| row.kind == ContentKind::Music && row.content_id == music_id)
        .map_or(1, |row| row.level as i32);
    let score_at = |level: i32| {
        master.ch3_scores
            .iter()
            .find(|row| row.active && row.level == level)
    };
    let character_score = score_at(lily_level).map_or(0, |row| row.character_score);
    let music_multiplier =
        score_at(music_level).map_or(0.0_f32, |row| row.music_score as f32 / 1000.0);
    let music_score = (character_score as f32 * music_multiplier) as i32;
    let with_bonus = (character_score as f32
        * (music_multiplier
            + if music_id == stage.bonus_music_id {
                0.2_f32
            } else {
                0.0_f32
            })) as i32;
    let bonus_music_score = with_bonus.saturating_sub(music_score);
    let mut seen = std::collections::BTreeSet::new();
    let selected = request
        .string("Profile_ids")
        .unwrap_or("")
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|part| part.parse::<i64>().ok())
        .filter(|id| *id > 0 && seen.insert(*id));
    let mut follower_score = 0_i32;
    let mut bonus_profile_ids = Vec::new();
    let mut profile_scores = Vec::new();
    for profile_id in selected {
        let level = snapshot
            .ch3
            .profiles
            .iter()
            .find(|profile| profile.profile_id == profile_id)
            .map_or(1, |profile| profile.level);
        let score = score_at(level).map_or(0, |row| row.follower_score);
        follower_score = follower_score.saturating_add(score);
        if stage.bonus_profile_ids.contains(&profile_id) {
            bonus_profile_ids.push(profile_id as i32);
        }
        profile_scores.push(Ch3ProfileScore {
            profile_id: profile_id as i32,
            score,
            bonus_score: 0,
        });
    }
    let bonus_score = bonus_music_score;
    let total_score = i64::from(character_score)
        + i64::from(music_score)
        + i64::from(follower_score)
        + i64::from(bonus_score);
    let star = stage
        .goal_scores
        .iter()
        .take_while(|goal| total_score >= **goal)
        .count() as i32;
    Ok(Ch3ScoreResult {
        story: false,
        clear: star > 0,
        star,
        music_id,
        character_score,
        music_score,
        follower_score,
        bonus_score,
        bonus_music_score,
        total_score,
        bonus_profile_ids,
        profile_scores,
    })
}

fn ch3_rewards(
    state: &AppState,
    stage: &ggfm_master_data::Ch3StageRow,
    star: i32,
    seed: &str,
) -> Vec<RewardGrant> {
    let groups: Vec<i64> = stage
        .percent_reward_groups
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|part| part.parse().ok())
        .collect();
    let multiplier = MemorialPolicy::default().ch3_reward_multiplier as f64;
    let mut result = Vec::new();
    if let Some(group) = groups.first() {
        let rows: Vec<_> = state.master.percent_rewards(*group).collect();
        if let Some(row) = rows.get(star.clamp(0, 3) as usize)
            && row.quantity > 0.0
        {
            result.push(RewardGrant {
                reward_type: row.reward_type,
                reward_id: row.reward_id,
                amount: row.quantity * multiplier,
            });
        }
    }
    if let Some(group) = groups.get(1) {
        let digest = Sha256::digest(seed.as_bytes());
        let roll = i32::from(digest[0]) * 100 / 256;
        let mut cumulative = 0;
        for row in state.master.percent_rewards(*group) {
            cumulative += row.percent.max(0);
            if roll < cumulative {
                if row.quantity > 0.0 {
                    result.push(RewardGrant {
                        reward_type: row.reward_type,
                        reward_id: row.reward_id,
                        amount: row.quantity * multiplier,
                    });
                }
                break;
            }
        }
    }
    result
}

fn reward_group(state: &AppState, group: i64, multiplier: f64) -> Vec<RewardGrant> {
    state
        .master
        .rewards(group)
        .filter(|row| row.quantity > 0.0)
        .map(|row| RewardGrant {
            reward_type: row.reward_type,
            reward_id: row.reward_id,
            amount: row.quantity * multiplier,
        })
        .collect()
}

fn reward_from_named_currency(kind: &str, amount: f64) -> Option<RewardGrant> {
    let reward_id = match kind.to_ascii_uppercase().as_str() {
        "CP" | "CHOCOLATE" => 1,
        "MP" | "CANDY" => 2,
        _ => return None,
    };
    Some(RewardGrant {
        reward_type: 1,
        reward_id,
        amount,
    })
}

fn ch3_profile_thresholds(state: &AppState) -> Vec<(i32, i64, i32)> {
    follower_profile_thresholds(state, 100_000)
}

fn mutation_key(
    session: &LoginSession,
    rpc: &str,
    sequence: Option<i64>,
    raw_request: &[u8],
) -> Result<String, GameplayError> {
    let sequence = sequence.filter(|value| *value > 0).ok_or_else(|| {
        GameplayError::Invalid(format!(
            "mutating RPC {rpc} is missing a positive request sequence"
        ))
    })?;
    Ok(format!(
        "{}:{}:{}:{}",
        session.nonce,
        sequence,
        rpc,
        hex::encode(Sha256::digest(raw_request))
    ))
}

fn ch3_energy_wire_value(
    energy: &ggfm_domain::Ch3EnergySnapshot,
) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "UserApData",
        [
            ("I_Ap".into(), Value::I32(energy.amount)),
            (
                "I_FullApTime".into(),
                // This is a Unix deadline, not a remaining duration. The
                // tested Rust baseline uses now+1 at full energy; retain that
                // sentinel and derive non-full deadlines at one AP/second.
                Value::I32(to_i32(energy.calculated_at.saturating_add(i64::from(
                    energy.cap.saturating_sub(energy.amount).max(1),
                )))),
            ),
            ("I_MaxAp".into(), Value::I32(energy.cap)),
        ],
    )
}

fn default_setting_wire_value(key: &str, value: &str) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "main_model",
        "DefaultSettingDataList",
        [
            ("Setting_key".into(), text(key)),
            ("Setting_value".into(), text(value)),
        ],
    )
}

fn main_game_info_wire_value(
    state: &AppState,
    locale: &str,
) -> Result<Value, ProtocolEnvelopeError> {
    let schema = ProtocolSchema::embedded();
    let locale = crate::normalized_notice_locale(locale);
    let (notice_title, _) = crate::memorial_notice_copy(locale);
    let notice = sparse_struct_value(
        schema,
        "main_model",
        "GetNoticeListRetDataInfo",
        [
            ("Seq".into(), Value::I32(1)),
            ("Notice_name".into(), text(notice_title)),
            (
                "Location_url".into(),
                text(&format!("{}/memorial/notice/{locale}", state.endpoint)),
            ),
            ("Img_url".into(), text("")),
        ],
    )?;
    sparse_struct_value(
        schema,
        "main_model",
        "MainGameInfo",
        [
            (
                "Album_list".into(),
                Value::List {
                    element_type: STRUCT,
                    values: Vec::new(),
                },
            ),
            (
                "Music_list".into(),
                Value::List {
                    element_type: STRUCT,
                    values: Vec::new(),
                },
            ),
            (
                "Album_language".into(),
                Value::Map {
                    key_type: I32,
                    value_type: STRUCT,
                    entries: Vec::new(),
                },
            ),
            (
                "Music_language".into(),
                Value::Map {
                    key_type: I32,
                    value_type: STRUCT,
                    entries: Vec::new(),
                },
            ),
            (
                "Tab".into(),
                Value::Map {
                    key_type: I16,
                    value_type: STRUCT,
                    entries: Vec::new(),
                },
            ),
            (
                "Notice".into(),
                Value::List {
                    element_type: STRUCT,
                    values: vec![notice],
                },
            ),
            (
                "Default_setting".into(),
                struct_value(
                    schema,
                    "main_model",
                    "GetDefaultSettingRetDataInfo",
                    std::iter::empty::<(String, Value)>(),
                )?,
            ),
        ],
    )
}

fn ch3_completed_stage_values(
    stages: &[ggfm_domain::Ch3StageSnapshot],
) -> Result<Vec<Value>, ProtocolEnvelopeError> {
    // GetIsAvailableStart checks the preceding row; GetIsClearStage treats
    // story-row presence as completion even when I_Star is zero.
    // Keep unlocked-but-unplayed rows internal to SQLite.
    stages.iter().filter(|stage| stage.completed).map(ch3_stage_wire_value).collect()
}

fn ch3_stage_wire_value(
    stage: &ggfm_domain::Ch3StageSnapshot,
) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "UserChThirdStage",
        [
            ("I_id".into(), Value::I32(stage.stage_id as i32)),
            ("I_ChapterId".into(), Value::I32(stage.chapter)),
            ("I_StageIndex".into(), Value::I32(stage.stage_index)),
            ("I_Star".into(), Value::I32(stage.best_star)),
        ],
    )
}

fn ch3_chapter_wire_value(
    chapter: i32,
    claims: &[ggfm_domain::Ch3ChapterClaimSnapshot],
) -> Result<Value, ProtocolEnvelopeError> {
    let claimed = |reward_num| {
        i32::from(
            claims
                .iter()
                .any(|row| row.chapter == chapter && row.reward_num == reward_num),
        )
    };
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "UserChThirdChapterReward",
        [
            ("I_id".into(), Value::I32(chapter)),
            ("I_ReceivedReward1".into(), Value::I32(claimed(1))),
            ("I_ReceivedReward2".into(), Value::I32(claimed(2))),
            ("I_ReceivedReward3".into(), Value::I32(claimed(3))),
        ],
    )
}

fn ch3_profile_wire_value(
    profile: &ggfm_domain::FollowerProfileSnapshot,
) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "UserFollowerProfile",
        [
            ("I_id".into(), Value::I32(profile.profile_id as i32)),
            ("I_Level".into(), Value::I32(profile.level)),
            ("D_Exp".into(), Value::I64(profile.experience)),
            ("I_AddCandy".into(), Value::I32(profile.add_candy)),
        ],
    )
}

fn ch3_music_wire_value(music_id: i64, level: i64) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "UserMusic",
        [
            ("I_id".into(), Value::I64(music_id)),
            ("I_Level".into(), Value::I64(level)),
            ("I_BonusLevel".into(), Value::I64(0)),
            ("B_EncoreBonusAppear".into(), Value::I64(0)),
            ("L_EncoreBonusActivateTime".into(), Value::I64(0)),
            ("I_EncoreBonusFollowerId".into(), Value::I64(0)),
            ("I_ChThirdActiveTime".into(), Value::I64(0)),
        ],
    )
}

fn ch3_profile_score_wire_value(score: &Ch3ProfileScore) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "StageFollowerProfileScore",
        [
            ("I_id".into(), Value::I32(score.profile_id)),
            ("Score".into(), Value::I32(score.score)),
            ("Bonus_score".into(), Value::I32(score.bonus_score)),
        ],
    )
}

fn mutation(
    session: &LoginSession,
    rpc: &str,
    sequence: Option<i64>,
    clock: DeviceClock,
    raw_request: &[u8],
) -> Result<RpcMutation, GameplayError> {
    let sequence = sequence.filter(|value| *value > 0).ok_or_else(|| {
        GameplayError::Invalid(format!(
            "mutating RPC {rpc} is missing a positive request sequence"
        ))
    })?;
    Ok(RpcMutation {
        nonce: session.nonce,
        identity: session.identity.clone(),
        request_seq: sequence,
        rpc: rpc.to_owned(),
        idempotency_key: format!(
            "{}:{}:{}:{}",
            session.nonce,
            sequence,
            rpc,
            hex::encode(Sha256::digest(raw_request))
        ),
        committed_at: clock.unix_seconds,
    })
}

async fn record_noop(state: &AppState, mutation: RpcMutation) -> Result<(), GameplayError> {
    state
        .database
        .execute(move |database| {
            database.commit_rpc_mutation(&mutation, |_transaction| {
                Ok(serde_json::json!({"accepted": true}))
            })
        })
        .await
        .map(|_: serde_json::Value| ())
        .map_err(Into::into)
}

fn player_response(
    spec: &ggfm_protocol::RpcSpec,
    snapshot: &PlayerSnapshot,
    clock: DeviceClock,
) -> Result<Value, ggfm_protocol::ProtocolEnvelopeError> {
    player_response_projected(spec, snapshot, true, clock)
}

fn player_response_projected(
    spec: &ggfm_protocol::RpcSpec,
    snapshot: &PlayerSnapshot,
    include_area: bool,
    clock: DeviceClock,
) -> Result<Value, ggfm_protocol::ProtocolEnvelopeError> {
    tracing::debug!(usn = snapshot.identity.usn, samseck = snapshot.samseck_step,
        guide_stage = snapshot.follower_quests.last().map(|row| row.current_id),
        profiles = snapshot.ch3.profiles.len(), gifts = snapshot.gifts.len(),
        profile_claims = snapshot.affection_claims.len(), buffs = snapshot.buff_timers.len(),
        "player: projecting durable reload records");
    let schema = ProtocolSchema::embedded();
    let primary = snapshot.areas.iter().find(|area| area.area == 1);
    // The tested pre-refactor Rust server emitted a complete typed UserData.
    // Keep that observable shape: in particular, U_create_time must be a
    // numeric string because the client parses it with System.Number during
    // the complete userLogin fan-out.
    let user = struct_value(
        schema,
        "user_model",
        "UserData",
        [
            ("U_seq".into(), Value::I32(to_i32(snapshot.identity.usn))),
            ("U_id".into(), text(&snapshot.identity.user_id)),
            ("U_name".into(), text("Guitar Girl")),
            ("U_nick".into(), text(&snapshot.nickname)),
            (
                "U_cp".into(),
                Value::I64(amount_i64(
                    snapshot.currency(ggfm_domain::CurrencyKind::Chocolate),
                )),
            ),
            (
                "U_candy".into(),
                Value::Double(snapshot.currency(ggfm_domain::CurrencyKind::Candy)),
            ),
            (
                "U_like".into(),
                Value::Double(snapshot.currency(ggfm_domain::CurrencyKind::Ch1Like)),
            ),
            (
                "U_fans".into(),
                Value::I64(amount_i64(
                    snapshot.currency(ggfm_domain::CurrencyKind::Fans),
                )),
            ),
            (
                "U_fans_grade".into(),
                Value::I16(to_i16(i64::from(snapshot.fan_level))),
            ),
            (
                "U_selected_costume_id".into(),
                Value::I64(primary.and_then(|area| area.current_costume).unwrap_or(1)),
            ),
            (
                "U_selected_music_id".into(),
                Value::I64(primary.and_then(|area| area.current_music).unwrap_or(1)),
            ),
            ("U_save_date".into(), text("")),
            ("U_samseck_step".into(), Value::I16(to_i16(i64::from(snapshot.samseck_step)))),
            ("U_last_login".into(), text("2026-09-03 00:00:00")),
            (
                "U_create_time".into(),
                text(&snapshot.last_save_time.to_string()),
            ),
            ("U_review_popup".into(), text("N")),
            ("Device_uuid".into(), text(&snapshot.device_uuid)),
        ],
    )?;

    let area_entries = snapshot
        .areas
        .iter()
        .map(|area| {
            let like_kind = if area.area == 2 {
                ggfm_domain::CurrencyKind::Ch2Like
            } else {
                ggfm_domain::CurrencyKind::Ch1Like
            };
            let gp1 = format!("{:.0}", snapshot.currency(like_kind));
            let gp2 = format!("{:.0}", snapshot.currency(if area.area == 2 {
                ggfm_domain::CurrencyKind::Ch2Note
            } else {
                ggfm_domain::CurrencyKind::Candy
            }));
            let value = struct_value(
                schema,
                "user_model",
                "UserAreaData",
                [
                    (
                        "U_area_num".into(),
                        Value::I16(to_i16(i64::from(area.area))),
                    ),
                    ("D_Candy".into(), Value::Double(area.candy)),
                    ("D_Like".into(), Value::Double(snapshot.currency(like_kind))),
                    ("I_UserFanCount".into(), Value::I64(amount_i64(area.fans))),
                    (
                        "I_UserFanGrade".into(),
                        Value::I16(to_i16(i64::from(area.area_level))),
                    ),
                    (
                        "I_SelectedCostumeId".into(),
                        Value::I64(area.current_costume.unwrap_or(1)),
                    ),
                    (
                        "I_SelectedMusicId".into(),
                        Value::I64(area.current_music.unwrap_or(1)),
                    ),
                    (
                        "I_SelectedGuitarId".into(),
                        Value::I64(area.current_guitar.unwrap_or(1)),
                    ),
                    ("S_Gp1".into(), text(&gp1)),
                    ("S_Gp2".into(), text(&gp2)),
                ],
            )?;
            Ok((Value::I32(area.area), value))
        })
        .collect::<Result<Vec<_>, ggfm_protocol::ProtocolEnvelopeError>>()?;

    // The production client uses userLoad(Type="buff") as a narrow refresh.
    // Its captured response keeps User_contents present, but as an empty
    // struct, and omits Area_data.  Re-sending the complete contents tree here
    // makes several screens replay their initialization path.
    let contents = if include_area {
        user_contents(snapshot, clock)?
    } else {
        sparse_struct_value(schema, "user_model", "UserContentsData", std::iter::empty())?
    };
    let mut fields = vec![("User".into(), user)];
    if include_area {
        fields.push((
            "Area_data".into(),
            Value::Map {
                key_type: I32,
                value_type: STRUCT,
                entries: area_entries,
            },
        ));
    }
    fields.push(("User_contents".into(), contents));
    response_sparse_struct(spec, schema, fields)
}

fn user_login_identity_response(
    spec: &ggfm_protocol::RpcSpec,
    identity: &ggfm_domain::UserIdentity,
) -> Result<Value, ggfm_protocol::ProtocolEnvelopeError> {
    let schema = ProtocolSchema::embedded();
    let user = sparse_struct_value(
        schema,
        "user_model",
        "UserData",
        [
            ("U_seq".into(), Value::I32(to_i32(identity.usn))),
            ("U_id".into(), text(&identity.user_id)),
        ],
    )?;
    response_sparse_struct(spec, schema, [("User".into(), user)])
}

fn user_contents(snapshot: &PlayerSnapshot, clock: DeviceClock) -> Result<Value, ggfm_protocol::ProtocolEnvelopeError> {
    let schema = ProtocolSchema::embedded();
    let levels: BTreeMap<_, _> = snapshot
        .content_levels
        .iter()
        .map(|level| {
            (
                (level.kind, level.content_id),
                (level.level, level.reward_level),
            )
        })
        .collect();
    let mut by_kind: BTreeMap<ContentKind, Vec<Value>> = BTreeMap::new();
    for owned in &snapshot.owned_content {
        let (level, reward_level) = levels
            .get(&(owned.kind, owned.content_id))
            .copied()
            .unwrap_or((1, 0));
        let (wire_type, fields) = match owned.kind {
            ContentKind::Costume => (
                "UserCostume",
                vec![
                    ("I_id".into(), Value::I64(owned.content_id)),
                    ("I_Level".into(), Value::I64(level)),
                    ("I_BonusLevel".into(), Value::I64(reward_level)),
                ],
            ),
            ContentKind::Guitar => (
                "UserGuitar",
                vec![
                    ("I_id".into(), Value::I64(owned.content_id)),
                    ("I_Level".into(), Value::I64(level)),
                    ("I_BonusLevel".into(), Value::I64(reward_level)),
                ],
            ),
            ContentKind::Music => ("UserMusic", {
                let music_state = snapshot
                    .music_states
                    .iter()
                    .find(|state| state.music_id == owned.content_id);
                vec![
                    ("I_id".into(), Value::I64(owned.content_id)),
                    ("I_Level".into(), Value::I64(level)),
                    ("I_BonusLevel".into(), Value::I64(reward_level)),
                    (
                        "B_EncoreBonusAppear".into(),
                        Value::I64(i64::from(
                            music_state.is_some_and(|state| state.encore_appears),
                        )),
                    ),
                    (
                        "L_EncoreBonusActivateTime".into(),
                        Value::I64(music_state.map_or(0, |state| state.encore_ready_at)),
                    ),
                    (
                        "I_EncoreBonusFollowerId".into(),
                        // A missing encore follower is the literal sentinel
                        // zero in production captures. Treating this foreign
                        // key like the UserMusic row id makes CH2 music point
                        // at nonexistent followers during UI initialization.
                        Value::I64(
                            music_state
                                .and_then(|state| state.encore_follower_id)
                                .unwrap_or(0),
                        ),
                    ),
                    (
                        "I_ChThirdActiveTime".into(),
                        Value::I64(music_state.map_or(0, |state| state.ch3_active_until)),
                    ),
                ]
            }),
            ContentKind::Skill => ("UserSkill", {
                let activation = snapshot
                    .skill_activations
                    .iter()
                    .find(|state| state.skill_id == owned.content_id);
                vec![
                    ("I_id".into(), Value::I64(owned.content_id)),
                    ("I_Level".into(), Value::I64(level)),
                    (
                        "B_Activate".into(),
                        Value::I16(i16::from(activation.is_some_and(|state| state.active))),
                    ),
                    (
                        "L_ActivateOnTicks".into(),
                        Value::I64(activation.map_or(0, |state| state.active_from)),
                    ),
                    (
                        "L_ActivateOffTicks".into(),
                        Value::I64(activation.map_or(0, |state| state.active_until)),
                    ),
                ]
            }),
            ContentKind::Unit => (
                "UserUnit",
                vec![
                    ("I_id".into(), Value::I64(owned.content_id)),
                    ("I_Level".into(), Value::I64(level)),
                ],
            ),
            ContentKind::Character => (
                "UserCharacter",
                vec![
                    ("I_id".into(), Value::I64(owned.content_id)),
                    ("I_Level".into(), Value::I64(level)),
                    ("I_BonusLevel".into(), Value::I64(reward_level)),
                ],
            ),
            ContentKind::Follower => (
                "UserFollower",
                vec![
                    ("I_id".into(), Value::I64(owned.content_id)),
                    ("I_Level".into(), Value::I64(level)),
                    ("I_BonusLevel".into(), Value::I64(reward_level)),
                ],
            ),
            ContentKind::Prop => (
                "UserProp",
                vec![
                    ("I_id".into(), Value::I64(owned.content_id)),
                    ("I_Level".into(), Value::I64(level)),
                ],
            ),
            ContentKind::Buff => ("UserBuff", {
                let timer = snapshot.buff_timers.iter().find(|timer| timer.buff_id == owned.content_id);
                vec![
                    ("I_id".into(), Value::I64(owned.content_id)),
                    ("I_Level".into(), Value::I64(level)),
                    ("I_ActiveTime".into(), Value::I64(timer.map_or(0, |timer| timer.active_at))),
                    ("I_EndTime".into(), Value::I64(timer.map_or(0, |timer| timer.expires_at))),
                ]
            }),
            _ => continue,
        };
        by_kind.entry(owned.kind).or_default().push(struct_value(
            schema,
            "user_model",
            wire_type,
            fields,
        )?);
    }
    let tutorials = snapshot
        .tutorials
        .iter()
        .map(|id| {
            struct_value(
                schema,
                "user_model",
                "UserTutorial",
                [("I_id".into(), Value::I32(to_i32(*id)))],
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    response_contents(
        schema,
        [
            ("User_costume", ContentKind::Costume),
            ("User_guitar", ContentKind::Guitar),
            ("User_music", ContentKind::Music),
            ("User_skill", ContentKind::Skill),
            ("User_unit", ContentKind::Unit),
            ("User_character", ContentKind::Character),
            ("User_follower", ContentKind::Follower),
            ("User_prop", ContentKind::Prop),
            ("User_buff", ContentKind::Buff),
        ],
        &by_kind,
        tutorials,
        snapshot,
        clock,
    )
}

fn response_contents(
    schema: &ProtocolSchema,
    fields: [(&str, ContentKind); 9],
    by_kind: &BTreeMap<ContentKind, Vec<Value>>,
    tutorials: Vec<Value>,
    snapshot: &PlayerSnapshot,
    clock: DeviceClock,
) -> Result<Value, ggfm_protocol::ProtocolEnvelopeError> {
    let mut overrides = Vec::new();
    for (field, kind) in fields {
        overrides.push((
            field.to_owned(),
            Value::List {
                element_type: STRUCT,
                values: by_kind.get(&kind).cloned().unwrap_or_default(),
            },
        ));
    }
    overrides.push((
        "User_tutorial".to_owned(),
        Value::List {
            element_type: STRUCT,
            values: tutorials,
        },
    ));
    let achievements = snapshot
        .achievements
        .iter()
        .map(|progress| {
            sparse_struct_value(
                schema,
                "user_model",
                "UserAchievement",
                [
                    ("I_id".into(), Value::I64(progress.id)),
                    ("I_Level".into(), Value::I64(progress.level)),
                    ("D_Quantity".into(), Value::Double(progress.quantity)),
                    ("S_Quantity".into(), text(&progress.quantity_text)),
                ],
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    overrides.push((
        "User_achievement".to_owned(),
        Value::List {
            element_type: STRUCT,
            values: achievements,
        },
    ));
    let daily = snapshot
        .daily_missions
        .iter()
        .map(|progress| {
            struct_value(
                schema,
                "user_model",
                "UserDailyMission",
                [
                    ("I_id".into(), Value::I64(progress.id)),
                    ("I_Level".into(), Value::I64(progress.level)),
                    (
                        "D_Quantity".into(),
                        Value::I64(amount_i64(progress.quantity)),
                    ),
                ],
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    overrides.push((
        "User_daily_mission".to_owned(),
        Value::List {
            element_type: STRUCT,
            values: daily,
        },
    ));
    let ad_levels = snapshot
        .ad_levels
        .iter()
        .map(|level| {
            struct_value(
                schema,
                "user_model",
                "UserAdLevel",
                [
                    ("I_id".into(), Value::I32(to_i32(level.ad_level_id))),
                    ("I_Level".into(), Value::I32(level.level)),
                    ("I_EXP".into(), Value::I32(level.experience)),
                ],
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    overrides.push((
        "User_ad_level".to_owned(),
        Value::List {
            element_type: STRUCT,
            values: ad_levels,
        },
    ));
    let ad_state = snapshot
        .ad_state
        .iter()
        .map(|state| {
            struct_value(
                schema,
                "user_model",
                "UserAdList",
                [
                    ("I_id".into(), Value::I32(to_i32(state.ad_id))),
                    ("I_Count".into(), Value::I32(state.day_count)),
                    ("I_TotalCount".into(), Value::I32(state.total_count)),
                    (
                        "I_LastViewTime".into(),
                        Value::I32(to_i32(state.last_view_time)),
                    ),
                ],
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    overrides.push((
        "User_ad_list".to_owned(),
        Value::List {
            element_type: STRUCT,
            values: ad_state,
        },
    ));
    let mut profiles: Vec<_> = snapshot.ch3.profiles.iter().filter(|profile| {
        // A fresh database keeps Lily's level-1 seed for scoring internally;
        // the tested baseline first owns her profile after a completed live.
        profile.profile_id != 100_000 || profile.level > 1 || profile.experience > 0
    }).cloned().collect();
    if snapshot.owned_content.iter().any(|row| row.kind == ContentKind::Follower && row.content_id == 1)
        && !profiles.iter().any(|profile| profile.profile_id == 1)
    {
        // Match the tested Rust fallback for the built-in Joie profile.
        profiles.push(ggfm_domain::FollowerProfileSnapshot { profile_id: 1, level: 1, experience: 0, add_candy: 0 });
    }
    profiles.sort_by_key(|profile| profile.profile_id);
    overrides.push(("User_follower_profile".into(), Value::List {
        element_type: STRUCT,
        values: profiles.iter().map(ch3_profile_wire_value).collect::<Result<_, _>>()?,
    }));
    overrides.push(("User_follower_giftitem".into(), Value::List {
        element_type: STRUCT,
        values: snapshot.gifts.iter().map(|gift| follower_gift_wire_value(gift.gift_id, gift.quantity)).collect::<Result<_, _>>()?,
    }));
    overrides.push(("User_follower_profile_reward".into(), Value::List {
        element_type: STRUCT,
        values: snapshot.affection_claims.iter().map(|claim| sparse_struct_value(
            schema, "user_model", "UserFollowerProfileReward", [
                ("I_id".into(), Value::I32(to_i32(claim.profile_id))),
                ("I_RewardLevel".into(), Value::I32(claim.level)),
            ],
        )).collect::<Result<_, _>>()?,
    }));
    let ch3_stages = ch3_completed_stage_values(&snapshot.ch3.stages)?;
    overrides.push((
        "User_chthird_stage".to_owned(),
        Value::List {
            element_type: STRUCT,
            values: ch3_stages,
        },
    ));
    let messengers = snapshot
        .messengers
        .iter()
        .map(|room| {
            sparse_struct_value(
                schema,
                "user_model",
                "UserMessenger",
                [
                    ("I_MessengerChatRoomId".into(), Value::I64(room.room_id)),
                    (
                        "I_LastConfirmIndex".into(),
                        Value::I64(room.last_confirm_index),
                    ),
                    ("S_UnlockGroupList".into(), text(&room.unlock_group_list)),
                    (
                        "L_UpdateTimeTicks".into(),
                        Value::I64(room.update_time_ticks),
                    ),
                ],
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    overrides.push((
        "User_messenger".to_owned(),
        Value::List {
            element_type: STRUCT,
            values: messengers,
        },
    ));
    let quests = follower_quest_wire_values(&snapshot.follower_quests)?;
    overrides.push((
        "User_follower_quest".to_owned(),
        Value::List {
            element_type: STRUCT,
            values: quests,
        },
    ));
    let active_season = pass_season(clock, snapshot.pass.anchor.anchor_day, snapshot.pass.anchor.anchor_season);
    // Match getGameDataList's device-local daily rotation, not the original
    // selection anchor. The subscription also starts in the current day.
    let active_at = clock.local_epoch_day() * 86_400
        - i64::from(clock.utc_offset_minutes) * 60;
    let subscriptions = snapshot
        .pass
        .premium_seasons
        .iter()
        // Entitlement is retained for every season, but the stock client
        // expects User_subscribe_list to describe the currently active pass,
        // not thirteen simultaneous subscriptions.
        .filter(|season| **season == active_season)
        .map(|season| subscribe_wire_value(*season, active_at))
        .collect::<Result<Vec<_>, _>>()?;
    overrides.push((
        "User_subscribe_list".to_owned(),
        Value::List {
            element_type: STRUCT,
            values: subscriptions,
        },
    ));
    let pass_progress = snapshot
        .pass
        .progress
        .iter()
        .map(|progress| {
            sparse_struct_value(
                schema,
                "user_model",
                "UserEventPoint",
                [
                    ("S_EventType".into(), text("Pass")),
                    ("I_DataID".into(), Value::I64(progress.season)),
                    ("I_Point".into(), Value::I64(progress.points)),
                    ("I_Step".into(), Value::I64(i64::from(progress.step))),
                    ("I_ADViewTime".into(), Value::I64(0)),
                    ("I_Version".into(), Value::I32(progress.version)),
                ],
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    overrides.push((
        "User_event_point".to_owned(),
        Value::List {
            element_type: STRUCT,
            values: pass_progress,
        },
    ));
    let pass_claims = snapshot
        .pass
        .claims
        .iter()
        .map(|claim| {
            sparse_struct_value(
                schema,
                "user_model",
                "UserSubscribePassReward",
                [
                    ("I_SubscribeID".into(), Value::I64(claim.season)),
                    ("I_Type".into(), Value::I64(i64::from(claim.lane))),
                    ("I_Step".into(), Value::I64(i64::from(claim.step))),
                    ("I_UpdateTime".into(), Value::I64(claim.claimed_at)),
                    ("I_Version".into(), Value::I32(claim.version)),
                ],
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    overrides.push((
        "User_subscribe_pass_reward".to_owned(),
        Value::List {
            element_type: STRUCT,
            values: pass_claims,
        },
    ));
    let tickets = snapshot
        .pass
        .tickets
        .iter()
        .map(|ticket| {
            sparse_struct_value(
                schema,
                "user_model",
                "UserTicketCollection",
                [("I_id".into(), Value::I64(*ticket))],
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    overrides.push((
        "User_ticketcollection".to_owned(),
        Value::List {
            element_type: STRUCT,
            values: tickets,
        },
    ));
    let selected_rewards = snapshot
        .selected_rewards
        .iter()
        .map(select_reward_wire_value)
        .collect::<Result<Vec<_>, _>>()?;
    overrides.push((
        "User_select_reward".to_owned(),
        Value::List {
            element_type: STRUCT,
            values: selected_rewards,
        },
    ));
    struct_value(schema, "user_model", "UserContentsData", overrides)
}

fn select_reward_wire_value(
    selected: &ggfm_domain::SelectRewardSnapshot,
) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "UserSelectReward",
        [
            ("I_GroupId".into(), Value::I32(selected.group_id)),
            (
                "I_RewardGroupId".into(),
                Value::I32(selected.reward_group_id),
            ),
            (
                "I_AltRewardGroupId".into(),
                Value::I32(selected.alternate_reward_group_id),
            ),
        ],
    )
}

fn parse_user_save(state: &AppState, request: &NamedRequest) -> UserSavePatch {
    let core = list_values(request.value("User_info"))
        .first()
        .and_then(|value| struct_fields(value))
        .map(|fields| CoreSavePatch {
            ch1_like: field_number(fields, 1),
            fans: field_integer(fields, 2),
            fan_level: field_integer(fields, 3).and_then(|value| i32::try_from(value).ok()),
            current_costume: validated_equipped_id(
                state,
                ContentKind::Costume,
                1,
                field_integer(fields, 4),
            ),
            current_music: validated_equipped_id(
                state,
                ContentKind::Music,
                1,
                field_integer(fields, 5),
            ),
        });
    let areas = list_values(request.value("User_area_info"))
        .iter()
        .filter_map(struct_fields)
        .filter_map(|fields| {
            let area = i32::try_from(field_integer(fields, 1)?).ok()?;
            Some(AreaSavePatch {
                area,
                like_amount: field_number(fields, 2),
                fans: field_integer(fields, 3),
                fan_level: field_integer(fields, 4).and_then(|value| i32::try_from(value).ok()),
                current_costume: validated_equipped_id(
                    state,
                    ContentKind::Costume,
                    area,
                    field_integer(fields, 5),
                ),
                current_music: validated_equipped_id(
                    state,
                    ContentKind::Music,
                    area,
                    field_integer(fields, 6),
                ),
                current_guitar: validated_equipped_id(
                    state,
                    ContentKind::Guitar,
                    area,
                    field_integer(fields, 7),
                ),
                candy: field_number(fields, 8),
                tutorial_list: field_text(fields, 9),
                gp1: field_text(fields, 10),
                gp2: field_text(fields, 11),
            })
        })
        .collect();
    let mut content = Vec::new();
    for (field, kind) in [
        ("User_character", ContentKind::Character),
        ("User_costume", ContentKind::Costume),
        ("User_follower", ContentKind::Follower),
        ("User_music", ContentKind::Music),
        ("User_guitar", ContentKind::Guitar),
    ] {
        for fields in list_values(request.value(field))
            .iter()
            .filter_map(struct_fields)
        {
            let Some(content_id) = field_integer(fields, 1) else {
                continue;
            };
            let Some(area) = content_area_exact(state, kind, content_id) else {
                continue;
            };
            content.push(ContentSavePatch {
                kind,
                area,
                content_id,
                level: field_integer(fields, 2),
                reward_level: field_integer(fields, 3),
            });
        }
    }
    UserSavePatch {
        core,
        areas,
        content,
        music_states: list_values(request.value("User_music"))
            .iter()
            .filter_map(struct_fields)
            .filter_map(|fields| {
                Some(MusicStateSavePatch {
                    music_id: field_integer(fields, 1)?,
                    encore_appears: field_integer(fields, 4).unwrap_or(0) != 0,
                    // userSave carries SaveUserMusic, whose follower field is
                    // field 5 (UserMusic responses use field 6).
                    encore_follower_id: field_integer(fields, 5).filter(|value| *value > 0),
                })
            })
            .collect(),
        achievements: parse_progress(request.value("User_achievement"), true),
        daily_missions: parse_progress(request.value("User_daily_mission"), false),
        skill_activations: list_values(request.value("User_skill"))
            .iter()
            .filter_map(struct_fields)
            .filter_map(|fields| {
                Some(SkillActivationSavePatch {
                    skill_id: field_integer(fields, 1)?,
                    active: field_integer(fields, 2).unwrap_or(0) != 0,
                    active_from: field_integer(fields, 3).unwrap_or(0),
                    active_until: field_integer(fields, 4).unwrap_or(0),
                })
            })
            .collect(),
        messengers: list_values(request.value("User_messenger"))
            .iter()
            .filter_map(struct_fields)
            .filter_map(|fields| {
                Some(MessengerSavePatch {
                    room_id: field_integer(fields, 1)?,
                    last_confirm_index: field_integer(fields, 2).unwrap_or(0),
                    unlock_group_list: field_text(fields, 3).unwrap_or_default(),
                    update_time_ticks: field_integer(fields, 4).unwrap_or(0),
                })
            })
            .collect(),
        pass_progress: list_values(request.value("User_event_point"))
            .iter()
            .filter_map(struct_fields)
            .filter_map(|fields| {
                Some(PassProgressSavePatch {
                    event_type: field_text(fields, 1).unwrap_or_default(),
                    season: field_integer(fields, 2)?,
                    points: field_integer(fields, 3).unwrap_or(0),
                    step: field_integer(fields, 4)
                        .unwrap_or(0)
                        .clamp(0, i64::from(i32::MAX)) as i32,
                    version: field_integer(fields, 5)
                        .unwrap_or(0)
                        .clamp(0, i64::from(i32::MAX)) as i32,
                })
            })
            .collect(),
        follower_quests: list_values(request.value("User_follower_quest"))
            .iter()
            .filter_map(struct_fields)
            .filter_map(|fields| {
                Some(FollowerQuestSavePatch {
                    current_id: field_integer(fields, 1)?,
                    condition_values: [
                        field_number(fields, 2).unwrap_or(0.0),
                        field_number(fields, 3).unwrap_or(0.0),
                        field_number(fields, 4).unwrap_or(0.0),
                    ],
                })
            })
            .collect(),
    }
}

fn parse_progress(value: Option<&Value>, text_quantity: bool) -> Vec<ProgressSavePatch> {
    list_values(value)
        .iter()
        .filter_map(struct_fields)
        .filter_map(|fields| {
            Some(ProgressSavePatch {
                id: field_integer(fields, 1)?,
                quantity: field_number(fields, 2)?,
                quantity_text: text_quantity.then(|| field_text(fields, 3)).flatten(),
            })
        })
        .collect()
}

fn content_area_exact(state: &AppState, kind: ContentKind, content_id: i64) -> Option<i32> {
    match kind {
        ContentKind::Costume => state
            .master
            .costumes
            .iter()
            .find(|row| row.id == content_id)
            .map(|row| row.area),
        ContentKind::Guitar => state
            .master
            .guitars
            .iter()
            .find(|row| row.id == content_id)
            .map(|row| row.area),
        ContentKind::Music => state
            .master
            .music
            .iter()
            .find(|row| row.id == content_id)
            .map(|row| row.area),
        _ => Some(1),
    }
}

fn content_area(state: &AppState, kind: ContentKind, content_id: i64) -> i32 {
    content_area_exact(state, kind, content_id).unwrap_or(1)
}

fn validated_equipped_id(
    state: &AppState,
    kind: ContentKind,
    expected_area: i32,
    content_id: Option<i64>,
) -> Option<i64> {
    content_id.filter(|content_id| {
        *content_id > 0 && content_area_exact(state, kind, *content_id) == Some(expected_area)
    })
}

fn tutorial_step_from_text(value: &str) -> Option<i64> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|part| part.parse::<i64>().ok())
        .max()
}

fn list_values(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::List { values, .. }) => values,
        _ => &[],
    }
}

fn struct_fields(value: &Value) -> Option<&[ggfm_protocol::Field]> {
    match value {
        Value::Struct(fields) => Some(fields),
        _ => None,
    }
}

fn field_value(fields: &[ggfm_protocol::Field], id: i16) -> Option<&Value> {
    fields
        .iter()
        .find(|field| field.id == id)
        .map(|field| &field.value)
}

fn field_integer(fields: &[ggfm_protocol::Field], id: i16) -> Option<i64> {
    match field_value(fields, id)? {
        Value::Byte(value) => Some(i64::from(*value)),
        Value::I16(value) => Some(i64::from(*value)),
        Value::I32(value) => Some(i64::from(*value)),
        Value::I64(value) => Some(*value),
        _ => None,
    }
}

fn field_number(fields: &[ggfm_protocol::Field], id: i16) -> Option<f64> {
    match field_value(fields, id)? {
        Value::Double(value) => Some(*value),
        Value::Byte(value) => Some(f64::from(*value)),
        Value::I16(value) => Some(f64::from(*value)),
        Value::I32(value) => Some(f64::from(*value)),
        Value::I64(value) => Some(*value as f64),
        _ => None,
    }
}

fn field_text(fields: &[ggfm_protocol::Field], id: i16) -> Option<String> {
    match field_value(fields, id)? {
        Value::String(value) => String::from_utf8(value.clone()).ok(),
        _ => None,
    }
}

fn text(value: &str) -> Value {
    Value::String(value.as_bytes().to_vec())
}

fn mail_wire_value(mail: &MailSnapshot) -> Result<Value, ProtocolEnvelopeError> {
    let schema = ProtocolSchema::embedded();
    let item = post_reward_item_wire_value(&mail.reward)?;
    let mut fields = vec![
        ("Idx".into(), Value::I64(mail.post_id)),
        ("Notice_type".into(), Value::I16(0)),
        ("Have_reward".into(), Value::I16(1)),
        ("Status".into(), Value::I16(i16::from(mail.claimed))),
        ("Unlimit_flg".into(), Value::I16(1)),
        ("Flg".into(), Value::I16(i16::from(mail.read))),
        ("Create_time".into(), Value::I64(mail.created_at)),
        ("Del_time".into(), Value::I64(i64::MAX / 2)),
        (
            "Item_list".into(),
            Value::List {
                element_type: STRUCT,
                values: vec![item],
            },
        ),
    ];
    for title in [
        "Title_ko",
        "Title_en",
        "Title_jp",
        "Title_zh_chs",
        "Title_zh_cht",
        "Title_vi",
        "Title_es",
        "Title_it",
        "Title_id",
        "Title_th",
        "Title_pt",
        "Title_hi",
    ] {
        fields.push((title.into(), text(crate::mail_copy::resolve(&mail.subject, &title[6..]))));
    }
    for memo in [
        "Memo_ko",
        "Memo_en",
        "Memo_jp",
        "Memo_zh_chs",
        "Memo_zh_cht",
        "Memo_vi",
        "Memo_es",
        "Memo_it",
        "Memo_id",
        "Memo_th",
        "Memo_pt",
        "Memo_hi",
    ] {
        fields.push((memo.into(), text(crate::mail_copy::resolve(&mail.body, &memo[5..]))));
    }
    struct_value(schema, "post_model", "GetPostList", fields)
}

fn post_reward_item_wire_value(reward: &RewardGrant) -> Result<Value, ProtocolEnvelopeError> {
    struct_value(
        ProtocolSchema::embedded(),
        "post_model",
        "ItemList",
        [
            (
                "I_RewardType".into(),
                Value::I32(i32::from(reward.reward_type)),
            ),
            ("I_RewardId".into(), Value::I32(reward.reward_id)),
            ("D_RewardQuantity".into(), Value::Double(reward.amount)),
        ],
    )
}

fn post_reward_wire_value(reward: &RewardGrant) -> Result<Value, ProtocolEnvelopeError> {
    struct_value(
        ProtocolSchema::embedded(),
        "post_model",
        "ProvidePostRetDataInfo",
        [
            (
                "I_RewardType".into(),
                Value::I32(i32::from(reward.reward_type)),
            ),
            ("I_RewardId".into(), Value::I32(reward.reward_id)),
            ("D_RewardQuantity".into(), Value::Double(reward.amount)),
        ],
    )
}

fn store_reward_wire_value(reward: &RewardGrant) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "store_model",
        "RetReward",
        [
            ("Reward_type".into(), Value::I16(reward.reward_type)),
            ("Reward_id".into(), Value::I32(reward.reward_id)),
            ("Reward_value".into(), Value::I64(amount_i64(reward.amount))),
        ],
    )
}

fn user_reward_wire_value(reward: &RewardGrant) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "RetReward",
        [
            ("Reward_type".into(), Value::I16(reward.reward_type)),
            ("Reward_id".into(), Value::I32(reward.reward_id)),
            ("Reward_value".into(), Value::Double(reward.amount)),
        ],
    )
}

fn subscribe_wire_value(season: i64, active_at: i64) -> Result<Value, ProtocolEnvelopeError> {
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "UserSubscribeList",
        [
            ("I_SubscribeID".into(), Value::I64(season)),
            ("I_ActiveTime".into(), Value::I64(active_at)),
            ("I_isActive".into(), Value::I64(1)),
        ],
    )
}

fn choice_user_value(snapshot: &PlayerSnapshot) -> Result<Value, ProtocolEnvelopeError> {
    let girl_level = snapshot
        .content_levels
        .iter()
        .find(|level| level.kind == ContentKind::Character && level.content_id == 1)
        .map_or(1, |level| level.level);
    sparse_struct_value(
        ProtocolSchema::embedded(),
        "user_model",
        "ChoiceUserData",
        [
            ("U_girl_level".into(), Value::I64(girl_level)),
            (
                "U_fans".into(),
                Value::I64(amount_i64(snapshot.currency(CurrencyKind::Fans))),
            ),
            (
                "U_fans_grade".into(),
                Value::I16(to_i16(i64::from(snapshot.fan_level))),
            ),
            (
                "U_cp".into(),
                Value::I64(amount_i64(snapshot.currency(CurrencyKind::Chocolate))),
            ),
            (
                "U_candy".into(),
                Value::Double(snapshot.currency(CurrencyKind::Candy)),
            ),
            (
                "U_like".into(),
                Value::Double(snapshot.currency(CurrencyKind::Ch1Like)),
            ),
            (
                "U_last_login".into(),
                text(&snapshot.last_save_time.to_string()),
            ),
        ],
    )
}

#[derive(Clone, Copy)]
struct ContentPurchaseSpec {
    kind: ContentKind,
    area: i32,
    content_id: i64,
    currency: CurrencyKind,
    price: f64,
    price_increment: f64,
    max_level: i64,
}

fn cosmetic_price_text(currency: &str, official: &str) -> String {
    if official == "0" { return "0".to_owned(); }
    if currency.eq_ignore_ascii_case("GP") || currency.eq_ignore_ascii_case("GP2") {
        scaled_requirement_text(official).unwrap_or_else(|| official.to_owned())
    } else {
        MemorialPolicy::embedded().cosmetic_price.to_string()
    }
}

fn content_purchase_spec(
    state: &AppState,
    requested_kind: &str,
    content_id: i64,
) -> Result<ContentPurchaseSpec, GameplayError> {
    // The 8.0.0 client uses both display names and save-model names depending
    // on which purchase button initiated the request. Keep the normalization
    // identical to the tested Rust baseline.
    let kind = requested_kind.to_ascii_lowercase().replace('_', "");
    match kind.as_str() {
        "costume" | "usercostume" | "3" => {
            let row = state
                .master
                .costumes
                .iter()
                .find(|row| row.id == content_id && row.active && row.acquisition_type == 0)
                .ok_or_else(|| {
                    GameplayError::Invalid(format!(
                        "costume {content_id} retains its non-shop acquisition source"
                    ))
                })?;
            Ok(ContentPurchaseSpec {
                kind: ContentKind::Costume,
                area: row.area,
                content_id,
                currency: currency_for_goods(&row.currency, row.area)?,
                price: cosmetic_price_text(&row.currency, &row.official_cost).parse()
                    .map_err(|_| GameplayError::Invalid("invalid costume price".into()))?,
                max_level: 1,
                price_increment: 0.0,
            })
        }
        "guitar" | "userguitar" | "4" => {
            let row = state
                .master
                .guitars
                .iter()
                .find(|row| row.id == content_id && row.active && row.acquisition_type == 0)
                .ok_or_else(|| {
                    GameplayError::Invalid(format!(
                        "guitar {content_id} retains its non-shop acquisition source"
                    ))
                })?;
            Ok(ContentPurchaseSpec {
                kind: ContentKind::Guitar,
                area: row.area,
                content_id,
                currency: currency_for_goods(&row.currency, row.area)?,
                price: cosmetic_price_text(&row.currency, &row.official_cost).parse()
                    .map_err(|_| GameplayError::Invalid("invalid guitar price".into()))?,
                max_level: 1,
                price_increment: 0.0,
            })
        }
        "skill" | "userskill" | "1" => {
            let row = state
                .master
                .skills
                .iter()
                .find(|row| row.id == content_id)
                .ok_or_else(|| GameplayError::Invalid(format!("unknown skill {content_id}")))?;
            Ok(ContentPurchaseSpec {
                kind: ContentKind::Skill,
                area: row.area,
                content_id,
                currency: currency_for_goods(&row.currency, row.area)?,
                price: mapped_content_upgrade_price(state, "Skill", content_id)?,
                price_increment: premium_increment(state, "Skill", content_id),
                max_level: i64::from(row.max_level.max(1)),
            })
        }
        "unit" | "userunit" | "mate" | "2" => {
            let row = state
                .master
                .units
                .iter()
                .find(|row| row.id == content_id)
                .ok_or_else(|| GameplayError::Invalid(format!("unknown unit {content_id}")))?;
            Ok(ContentPurchaseSpec {
                kind: ContentKind::Unit,
                area: row.area,
                content_id,
                currency: currency_for_goods(&row.currency, row.area)?,
                price: mapped_content_upgrade_price(state, "Unit", content_id)?,
                price_increment: premium_increment(state, "Unit", content_id),
                max_level: i64::from(row.max_level.max(1)),
            })
        }
        "follower" => {
            let row = state
                .master
                .game_data
                .get("Follower")
                .and_then(|rows| {
                    rows.iter()
                        .find(|row| master_i64(row, "id") == Some(content_id))
                })
                .ok_or_else(|| GameplayError::Invalid(format!("unknown follower {content_id}")))?;
            let area = master_i64(row, "area")
                .unwrap_or(1)
                .clamp(1, i64::from(i32::MAX)) as i32;
            let currency = row
                .get("goodstype")
                .and_then(master_string_value)
                .ok_or_else(|| {
                    GameplayError::Invalid(format!(
                        "follower {content_id} has no purchase currency"
                    ))
                })?;
            Ok(ContentPurchaseSpec {
                kind: ContentKind::Follower,
                area,
                content_id,
                currency: currency_for_goods(currency, area)?,
                price: mapped_content_upgrade_price(state, "Follower", content_id)?,
                price_increment: 0.0,
                max_level: master_i64(row, "maxlevel").unwrap_or(1).max(1),
            })
        }
        _ => Err(GameplayError::Invalid(format!(
            "unsupported buyContents type {requested_kind}"
        ))),
    }
}

fn premium_increment(state: &AppState, table: &str, id: i64) -> f64 {
    let Some(rows) = state.master.game_data.get(table) else { return 0.0; };
    let Some(row) = rows.iter().find(|row| master_i64(row, "id") == Some(id)) else { return 0.0; };
    let policy = MemorialPolicy::embedded();
    if !single_digit_upgrade_row(policy, table, row) { return 0.0; }
    let offset = if table == "Skill" { 2 } else { 1 };
    let base = |r: &BTreeMap<String, MasterScalar>| r.get("cost").and_then(master_f64_value).unwrap_or(0.0);
    let final_price = |r: &BTreeMap<String, MasterScalar>| {
        let increment = r.get("costincreasevalue").and_then(master_f64_value).unwrap_or(0.0) as f32;
        (base(r) as f32 + (master_i64(r, "maxlevel").unwrap_or(1) - offset).max(0) as f32 * increment).trunc() as f64
    };
    let currency = |r: &BTreeMap<String, MasterScalar>| r.get("goodstype").and_then(master_string_value).unwrap_or_default().to_ascii_lowercase();
    let prices: Vec<_> = rows.iter().filter(|r|
        base(r) > 0.0 && master_i64(r, "area").unwrap_or(1) == master_i64(row, "area").unwrap_or(1)
        && currency(r) == currency(row)).map(final_price).collect();
    let minimum = prices.iter().copied().fold(f64::INFINITY, f64::min);
    let maximum = prices.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let cap = ggfm_policy_memorial::premium_upgrade_cap(final_price(row), minimum, maximum);
    ggfm_policy_memorial::premium_upgrade_increment(
        master_i64(row, "maxlevel").unwrap_or(1), base(row), offset, cap)
}

/// PropLevel is the client's explicit candy price table. Build both RPC prices
/// and the published table from these same original per-prop terminal costs.
fn prop_price_curve(state: &AppState, id: i64) -> Option<(i64, f64)> {
    let rows = state.master.game_data.get("Proplevel")?;
    let mut limits: BTreeMap<i64, (i64, f64)> = BTreeMap::new();
    for row in rows {
        let prop = master_i64(row, "propid")?;
        let level = master_i64(row, "level")?;
        let cost = row.get("cost").and_then(master_f64_value)?;
        let entry = limits.entry(prop).or_insert((0, 0.0));
        entry.0 = entry.0.max(level);
        entry.1 = entry.1.max(cost);
    }
    let &(maxlevel, final_price) = limits.get(&id)?;
    let minimum = limits.values().map(|r| r.1).filter(|c| *c > 0.0).fold(f64::INFINITY, f64::min);
    let maximum = limits.values().map(|r| r.1).fold(0.0, f64::max);
    let cap = ggfm_policy_memorial::premium_upgrade_cap(final_price, minimum, maximum);
    Some((maxlevel, ggfm_policy_memorial::premium_upgrade_increment(maxlevel, final_price, 1, cap)))
}

fn mapped_content_upgrade_price(
    state: &AppState,
    table: &str,
    content_id: i64,
) -> Result<f64, GameplayError> {
    let row = state
        .master
        .game_data
        .get(table)
        .and_then(|rows| {
            rows.iter()
                .find(|row| master_i64(row, "id") == Some(content_id))
        })
        .ok_or_else(|| {
            GameplayError::Invalid(format!("missing {table} master row {content_id}"))
        })?;
    let official = row
        .get("cost")
        .ok_or_else(|| GameplayError::Invalid(format!("{table} row {content_id} has no cost")))?;
    master_f64_value(&mapped_upgrade_cost(state, table, row, official)).ok_or_else(|| {
        GameplayError::Invalid(format!("{table} row {content_id} has a non-numeric cost"))
    })
}

fn currency_for_goods(goods_type: &str, area: i32) -> Result<CurrencyKind, GameplayError> {
    match goods_type.to_ascii_lowercase().as_str() {
        "cp" => Ok(CurrencyKind::Chocolate),
        "candy" => Ok(CurrencyKind::Candy),
        "gp" if area == 2 => Ok(CurrencyKind::Ch2Like),
        "gp" => Ok(CurrencyKind::Ch1Like),
        "gp2" => Ok(CurrencyKind::Ch2Note),
        other => Err(GameplayError::Invalid(format!(
            "goods type {other} is not directly purchasable"
        ))),
    }
}

fn game_data_row_value(
    state: &AppState,
    table: &str,
    wire_type: &str,
    row: &MasterRow,
    pass_overlay: PassMasterOverlay,
) -> Result<Value, GameplayError> {
    let schema = ProtocolSchema::embedded();
    let type_spec = schema
        .type_spec(&format!("main_model::{wire_type}"))
        .ok_or_else(|| GameplayError::Invalid(format!("missing master wire type {wire_type}")))?;
    let mut values = Vec::new();
    for field in &type_spec.fields {
        let key = normalize_column_name(&field.name);
        let Some(source) = row.get(&key) else {
            continue;
        };
        let source = transform_master_value(state, table, &key, row, source, pass_overlay);
        if matches!(source, MasterScalar::Null) {
            continue;
        }
        values.push((
            field.name.clone(),
            master_scalar_wire_value(&field.wire_type, source)?,
        ));
    }
    Ok(struct_value(schema, "main_model", wire_type, values)?)
}

#[derive(Clone, Copy, Debug)]
struct PassMasterOverlay {
    active_season: i64,
    local_month: i32,
}

#[derive(Clone, Debug)]
struct VarietyStoreRow {
    id: i64,
    area: i32,
    cost: i32,
    price_increment: f64,
    reward_id: i64,
    max_level: i32,
    source: MasterRow,
}

fn variety_store_rows(state: &AppState) -> Result<Vec<VarietyStoreRow>, GameplayError> {
    let rows = state
        .master
        .game_data
        .get("Prop")
        .ok_or_else(|| GameplayError::Invalid("master data has no Prop table".into()))?;
    let active = rows
        .iter()
        .filter(|row| master_i64(row, "isactive").unwrap_or(0) != 0)
        .collect::<Vec<_>>();
    Ok(active
        .into_iter()
        .filter_map(|row| {
            let id = master_i64(row, "id")?;
            let (_, price_increment) = prop_price_curve(state, id)?;
            Some(VarietyStoreRow {
                id,
                area: master_i64(row, "area").unwrap_or(1) as i32,
                cost: 1,
                price_increment,
                reward_id: id,
                max_level: master_i64(row, "maxlevel").unwrap_or(20).max(1) as i32,
                source: row.clone(),
            })
        })
        .collect())
}

fn variety_store_wire_value(row: &VarietyStoreRow) -> Result<Value, GameplayError> {
    let schema = ProtocolSchema::embedded();
    let type_spec = schema
        .type_spec("store_model::GetVarietyStoreRetDataInfo")
        .expect("audited variety store wire type");
    let mut values = Vec::new();
    for field in &type_spec.fields {
        let key = normalize_column_name(&field.name);
        let value = match key.as_str() {
            "id" => Some(MasterScalar::Integer(row.id)),
            "cost" => Some(MasterScalar::Integer(i64::from(row.cost))),
            "rewardtype" => Some(MasterScalar::Integer(4)),
            "rewardid" => Some(MasterScalar::Integer(row.reward_id)),
            "rewardquantity" => Some(MasterScalar::Integer(1)),
            key if key.starts_with("title") => {
                row.source.get(&key.replacen("title", "name", 1)).cloned()
            }
            _ => row.source.get(&key).cloned(),
        };
        if let Some(value) = value.filter(|value| !matches!(value, MasterScalar::Null)) {
            values.push((
                field.name.clone(),
                master_scalar_wire_value(&field.wire_type, value)?,
            ));
        }
    }
    Ok(struct_value(
        schema,
        "store_model",
        "GetVarietyStoreRetDataInfo",
        values,
    )?)
}

fn transform_master_value(
    state: &AppState,
    table: &str,
    field: &str,
    row: &MasterRow,
    source: &MasterScalar,
    pass_overlay: PassMasterOverlay,
) -> MasterScalar {
    let policy = MemorialPolicy::embedded();
    let id = master_i64(row, "id").unwrap_or_default();
    let area = master_i64(row, "area").unwrap_or_default() as i32;
    let number = master_f64_value(source);
    if table == "SubscribeList"
        && let Some(value) = transform_subscribe_value(field, id, source, pass_overlay)
    {
        return value;
    }
    match (table, field) {
        // getGameDataList replaces the bundled client catalog at login. Publish
        // effective fill amounts here too, without mutating the original master
        // used by paidEventPoint (which applies the multiplier exactly once).
        ("SubscribePass", "paidpoint" | "adpoint") => match source {
            MasterScalar::Integer(points) => {
                MasterScalar::Integer(points.saturating_mul(policy.pass_point_multiplier))
            }
            _ => source.clone(),
        },
        ("Followergiftitem", "resourcename") => policy.follower_gift_icons.get(&id)
            .map(|name| MasterScalar::Text(name.clone()))
            .unwrap_or_else(|| source.clone()),
        ("Fan", "fancount") => {
            MasterScalar::Integer(scaled_fan_requirement(number.unwrap_or_default(), 1_000.0) as i64)
        }
        ("Achievement", field) if field.starts_with("condition") && !policy.scaled_achievement_ids.contains(&id) => source.clone(),
        ("Achievement", field) if field.starts_with("condition") => match source {
            MasterScalar::Text(value) => scaled_requirement_text(value)
                .map(MasterScalar::Text)
                .unwrap_or_else(|| source.clone()),
            _ => scalar_like(
                source,
                scaled_requirement(number.unwrap_or_default(), false),
            ),
        },
        // Gift/CH3 reward gains already carry the memorial acceleration.
        // Preserve every cumulative affection threshold, including Lily's.
        ("Followerprofilelevel", "requireexp") => source.clone(),
        ("Costume", "cost") => state
            .master
            .costumes
            .iter()
            .find(|candidate| candidate.id == id)
            .filter(|candidate| candidate.acquisition_type == 0 && candidate.official_cost != "0")
            .map_or_else(
                || source.clone(),
                |row| MasterScalar::Text(cosmetic_price_text(&row.currency, &row.official_cost)),
            ),
        ("Costume", "fanmultiply") => {
            state.master.costumes.iter().find(|row| row.id == id)
                .map_or_else(|| source.clone(), |row| MasterScalar::Real(super::memorial_costume_fan_bonus(&state.master, row)))
        }
        ("Guitar", "cost") => state
            .master
            .guitars
            .iter()
            .find(|candidate| candidate.id == id)
            .filter(|candidate| candidate.acquisition_type == 0 && candidate.official_cost != "0")
            .map_or_else(
                || source.clone(),
                |row| MasterScalar::Text(cosmetic_price_text(&row.currency, &row.official_cost)),
            ),
        ("Guitar", "incrementvalue") => {
            let is_default = state
                .master
                .guitars
                .iter()
                .filter(|candidate| candidate.area == area)
                .map(|candidate| candidate.id)
                .min()
                == Some(id);
            if is_default {
                source.clone()
            } else if area == 1 {
                MasterScalar::Real(1.20)
            } else {
                MasterScalar::Real(
                    state
                        .master
                        .guitars
                        .iter()
                        .filter(|candidate| candidate.area == area)
                        .map(|candidate| candidate.like_multiplier)
                        .fold(0.0, f64::max),
                )
            }
        }
        (table, "cost")
            if matches!(
                table,
                "Character" | "Follower" | "Music" | "Skill" | "Unit" | "Proplevel"
            ) =>
        {
            mapped_upgrade_cost(state, table, row, source)
        }
        ("Character" | "Follower" | "Music" | "Skill" | "Unit", "costincreasevalue")
            if single_digit_upgrade_row(policy, table, row) =>
        {
            scalar_like(source, if matches!(table, "Skill" | "Unit") {
                premium_increment(state, table, id)
            } else { 0.0 })
        }
        ("Character" | "Follower" | "Music" | "Skill" | "Unit", "costincreasevalue") => {
            scalar_like(
                source,
                number.unwrap_or_default() * policy.progression_multiplier,
            )
        }
        ("Costume" | "Guitar", key) if key.starts_with("description") => {
            let bonus_field = if table == "Costume" {
                "fanmultiply"
            } else {
                "incrementvalue"
            };
            let Some(bonus_source) = row.get(bonus_field) else {
                return source.clone();
            };
            let bonus =
                transform_master_value(state, table, bonus_field, row, bonus_source, pass_overlay);
            let Some(value) = master_f64_value(&bonus) else {
                return source.clone();
            };
            let base = if table == "Costume" { 0.0 } else { 1.0 };
            match source {
                MasterScalar::Text(text) if value > base => {
                    MasterScalar::Text(ggfm_policy_memorial::rewrite_percentage(
                        text,
                        ((value - base) * 100.0).round() as i32,
                    ))
                }
                _ => source.clone(),
            }
        }
        ("Skill", "unlocklevel" | "requirementcharacterlevel" | "requirementfollowerlevel" | "requirementpropcount")
            if policy.preserve_skill_unlock_requirements => source.clone(),
        (
            _,
            "unlocklevel"
            | "unlockcost"
            | "requirementcharacterlevel"
            | "requirementfollowerlevel"
            | "requirementpropcount",
        ) => scalar_like(
            source,
            scaled_requirement(number.unwrap_or_default(), false),
        ),
        ("Skill", "cooltime") => MasterScalar::Real(memorial_skill_cooldown(
            policy,
            id,
            number.unwrap_or_default(),
        )),
        ("Music_level", "encorebonusappearcooltimesec") => MasterScalar::Real(
            ggfm_policy_memorial::encore_cooldown(number.unwrap_or_default(), 75_600.0),
        ),
        ("Music_level", "encorebonusappearcooltimehour") => MasterScalar::Real(
            ggfm_policy_memorial::encore_cooldown(number.unwrap_or_default() * 3600.0, 75_600.0)
                / 3600.0,
        ),
        _ => source.clone(),
    }
}

fn memorial_skill_cooldown(policy: &MemorialPolicy, skill_id: i64, official: f64) -> f64 {
    if matches!(skill_id, 4 | 24) {
        policy.skill_reset_seconds as f64
    } else {
        official * policy.skill_cooldown_multiplier
    }
}

fn transform_subscribe_value(
    field: &str,
    id: i64,
    source: &MasterScalar,
    pass_overlay: PassMasterOverlay,
) -> Option<MasterScalar> {
    match field {
        "isactive" if (1..=13).contains(&id) => Some(scalar_like(
            source,
            if id == pass_overlay.active_season {
                1.0
            } else {
                0.0
            },
        )),
        "repeatmonth" if id == pass_overlay.active_season => {
            Some(scalar_like(source, f64::from(pass_overlay.local_month)))
        }
        _ => None,
    }
}

fn scalar_like(source: &MasterScalar, value: f64) -> MasterScalar {
    match source {
        MasterScalar::Integer(_) => MasterScalar::Integer(value.ceil() as i64),
        MasterScalar::Text(_) => MasterScalar::Text(value.ceil().to_string()),
        MasterScalar::Real(_) => MasterScalar::Real(value),
        MasterScalar::Null | MasterScalar::Blob(_) => source.clone(),
    }
}

fn mapped_upgrade_cost(
    state: &AppState,
    table: &str,
    row: &MasterRow,
    source: &MasterScalar,
) -> MasterScalar {
    let Some(official) = master_f64_value(source) else {
        return source.clone();
    };
    if official <= 0.0 {
        return source.clone();
    }
    let policy = MemorialPolicy::embedded();
    if !single_digit_upgrade_row(policy, table, row) {
        return scalar_like(source, official * policy.progression_multiplier);
    }
    if matches!(table, "Skill" | "Unit") {
        return scalar_like(source, 1.0);
    }
    if table == "Proplevel" {
        if let Some((_, increment)) = master_i64(row, "propid").and_then(|id| prop_price_curve(state, id)) {
            let level = master_i64(row, "level").unwrap_or(1);
            let price = (1.0_f32 + increment as f32 * (level-1).max(0) as f32).trunc();
            return scalar_like(source, f64::from(price));
        }
        return source.clone();
    }
    let currency = row.get("goodstype");
    let Some(rows) = state.master.game_data.get(table) else {
        return source.clone();
    };
    let mut distinct = rows
        .iter()
        .filter(|candidate| candidate.get("goodstype") == currency)
        .filter_map(|candidate| candidate.get("cost").and_then(master_f64_value))
        .filter(|value| *value > 0.0)
        .collect::<Vec<_>>();
    distinct.sort_by(f64::total_cmp);
    distinct.dedup_by(|left, right| left.total_cmp(right).is_eq());
    let Some(rank) = distinct
        .iter()
        .position(|value| value.total_cmp(&official).is_eq())
    else {
        return source.clone();
    };
    scalar_like(source, memorial_upgrade_cost(rank, distinct.len()) as f64)
}

fn single_digit_upgrade_row(policy: &MemorialPolicy, table: &str, row: &MasterRow) -> bool {
    let currency = row
        .get("goodstype")
        .and_then(master_string_value)
        .unwrap_or(if table == "Proplevel" { "Candy" } else { "GP" });
    policy.uses_single_digit_cost(currency)
}

fn master_i64(row: &MasterRow, field: &str) -> Option<i64> {
    row.get(field).and_then(|value| match value {
        MasterScalar::Integer(value) => Some(*value),
        MasterScalar::Real(value) => Some(*value as i64),
        MasterScalar::Text(value) => value.parse().ok(),
        MasterScalar::Null | MasterScalar::Blob(_) => None,
    })
}

fn master_f64_value(value: &MasterScalar) -> Option<f64> {
    match value {
        MasterScalar::Integer(value) => Some(*value as f64),
        MasterScalar::Real(value) => Some(*value),
        MasterScalar::Text(value) => value.parse().ok(),
        MasterScalar::Null | MasterScalar::Blob(_) => None,
    }
}

fn master_string_value(value: &MasterScalar) -> Option<&str> {
    match value {
        MasterScalar::Text(value) => Some(value),
        MasterScalar::Null
        | MasterScalar::Integer(_)
        | MasterScalar::Real(_)
        | MasterScalar::Blob(_) => None,
    }
}

fn master_scalar_wire_value(wire_type: &str, value: MasterScalar) -> Result<Value, GameplayError> {
    let integer = || master_f64_value(&value).unwrap_or_default() as i64;
    Ok(match wire_type {
        "bool" => Value::Bool(integer() != 0),
        "byte" | "int8" | "uint8" => {
            Value::Byte(integer().clamp(i64::from(i8::MIN), i64::from(i8::MAX)) as i8)
        }
        "int16" | "uint16" => Value::I16(to_i16(integer())),
        "int" | "int32" | "uint" | "uint32" => Value::I32(to_i32(integer())),
        "int64" | "uint64" => Value::I64(integer()),
        "float32" | "float64" => Value::Double(master_f64_value(&value).unwrap_or_default()),
        "string" => Value::String(match value {
            MasterScalar::Text(value) => value.into_bytes(),
            MasterScalar::Integer(value) => value.to_string().into_bytes(),
            MasterScalar::Real(value) => value.to_string().into_bytes(),
            MasterScalar::Blob(value) => value,
            MasterScalar::Null => Vec::new(),
        }),
        other => {
            return Err(GameplayError::Invalid(format!(
                "unsupported scalar master field type {other}"
            )));
        }
    })
}

fn amount_i64(value: f64) -> i64 {
    if !value.is_finite() {
        0
    } else {
        value.round().clamp(i64::MIN as f64, i64::MAX as f64) as i64
    }
}

fn to_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn to_i16(value: i64) -> i16 {
    value.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
}

fn device_datetime_number(clock: DeviceClock) -> i64 {
    let local = clock.unix_seconds + i64::from(clock.utc_offset_minutes) * 60;
    let days = local.div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3600;
    let minute = seconds % 3600 / 60;
    let second = seconds % 60;
    i64::from(year) * 10_000_000_000
        + i64::from(month) * 100_000_000
        + i64::from(day) * 1_000_000
        + hour * 10_000
        + minute * 100
        + second
}

fn civil_from_days(days_since_epoch: i64) -> (i32, i32, i32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year as i32, month as i32, day as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ggfm_persistence_sqlite::{Database, DatabaseActor};

    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(ggfm_private_oracle)]
    include!(env!("GGFM_PRIVATE_ORACLE_TESTS"));
    include!("ch3_baseline_tests.rs");
    #[cfg(ggfm_private_oracle)]
    include!("guide_baseline_tests.rs");

    fn request(call: &str, fields: BTreeMap<String, Value>) -> NamedRequest {
        NamedRequest {
            call: call.to_owned(),
            fields,
            raw_fields: Vec::new(),
        }
    }




    #[test]
    fn server_datetime_uses_device_offset() {
        assert_eq!(
            device_datetime_number(DeviceClock::new(0, 8 * 60).unwrap()),
            19700101080000
        );
    }

    #[test]
    fn pass_master_overlay_exposes_only_the_selected_daily_season() {
        let overlay = PassMasterOverlay {
            active_season: 2,
            local_month: 9,
        };
        for season in 1..=13 {
            assert_eq!(
                transform_subscribe_value("isactive", season, &MasterScalar::Integer(1), overlay,),
                Some(MasterScalar::Integer(i64::from(season == 2)))
            );
        }
        assert_eq!(
            transform_subscribe_value("repeatmonth", 2, &MasterScalar::Integer(8), overlay,),
            Some(MasterScalar::Integer(9))
        );
        assert_eq!(
            transform_subscribe_value("repeatmonth", 3, &MasterScalar::Integer(8), overlay,),
            None
        );
    }

    #[test]
    fn aggregate_game_data_uses_the_production_ret_map_key() {
        assert_eq!(GAME_DATA_RESPONSE_KEY, "ret");
        let captured_request = request(
            "getGameDataList",
            BTreeMap::from([("Game_type".into(), text("ALL"))]),
        );
        assert_eq!(captured_request.string("Game_type"), Some("ALL"));
        assert_ne!(
            captured_request.string("Game_type"),
            Some(GAME_DATA_RESPONSE_KEY)
        );
    }

    #[test]
    fn server_master_keeps_skill_reset_at_five_minutes() {
        let policy = MemorialPolicy::default();
        assert_eq!(memorial_skill_cooldown(&policy, 4, 900.0), 300.0);
        assert_eq!(memorial_skill_cooldown(&policy, 24, 900.0), 300.0);
        assert_eq!(memorial_skill_cooldown(&policy, 1, 600.0), 60.0);
    }

    #[test]
    fn daily_pass_subscription_matches_master_for_every_season_and_offset() {
        let mut db = Database::open_memory().unwrap();
        let identity = db.ensure_default_slot(1_788_360_000).unwrap();
        db.begin_login(b"pass-regression", DeviceClock::new(1_788_360_000, 0).unwrap()).unwrap();
        let mut snapshot = db.player_snapshot(identity.usn).unwrap();
        for offset in [-720, 0, 600, 840] {
            let start = DeviceClock::new(1_788_360_000, offset).unwrap();
            snapshot.pass.anchor.anchor_day = start.local_epoch_day();
            snapshot.pass.anchor.anchor_season = 1;
            for elapsed in 0..26 {
                let clock = DeviceClock::new(start.unix_seconds + elapsed * 86_400, offset).unwrap();
                let current = pass_season(clock, snapshot.pass.anchor.anchor_day, 1);
                let Value::Struct(fields) = user_contents(&snapshot, clock).unwrap() else { panic!() };
                let descriptor = ProtocolSchema::embedded().type_spec("user_model::UserContentsData").unwrap();
                let id = descriptor.fields.iter().find(|f| f.name == "User_subscribe_list").unwrap().id;
                let Value::List { values, .. } = &fields.iter().find(|f| f.id == id).unwrap().value else { panic!() };
                let midnight = clock.local_epoch_day() * 86_400 - i64::from(offset) * 60;
                assert_eq!(values, &vec![subscribe_wire_value(current, midnight).unwrap()]);
            }
        }
    }

    #[test]
    fn user_load_projects_v8_balances_from_authoritative_currencies() {
        use ggfm_domain::CurrencyKind;
        let mut database = Database::open_memory().unwrap();
        let identity = database.ensure_default_slot(1_788_360_000).unwrap();
        database.begin_login(b"v8-balance-projection", DeviceClock::new(1_788_360_000, 0).unwrap()).unwrap();
        let mut snapshot = database.player_snapshot(identity.usn).unwrap();
        snapshot.currencies = vec![
            (CurrencyKind::Ch1Like, 257432.0),
            (CurrencyKind::Ch2Like, 800.0),
            (CurrencyKind::Ch2Note, 590.0),
            (CurrencyKind::Candy, 110.0),
        ];
        let schema = ProtocolSchema::embedded();
        let spec = schema.call("userLoad").unwrap();
        let Value::Struct(fields) = player_response_projected(
            spec, &snapshot, true, DeviceClock::new(1_788_360_000, 0).unwrap(),
        ).unwrap() else { panic!() };
        let Value::Map { entries, .. } = &fields.iter().find(|field| field.id == 2).unwrap().value else { panic!() };
        let descriptor = schema.type_spec("user_model::UserAreaData").unwrap();
        for (area, gp1, gp2) in [(1, "257432", "110"), (2, "800", "590")] {
            let Value::Struct(fields) = &entries.iter().find(|(key, _)| *key == Value::I32(area)).unwrap().1 else { panic!() };
            for (name, expected) in [("S_Gp1", gp1), ("S_Gp2", gp2)] {
                let id = descriptor.fields.iter().find(|field| field.name == name).unwrap().id;
                assert_eq!(fields.iter().find(|field| field.id == id).unwrap().value, text(expected));
            }
        }
    }

    #[test]
    fn buff_user_load_matches_the_captured_narrow_projection() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ggfm-user-load-buff-{suffix}.sqlite"));
        let mut database = Database::open(&path).unwrap();
        let identity = database.ensure_default_slot(1_788_360_000).unwrap();
        database
            .begin_login(
                b"captured-buff-shape",
                DeviceClock::new(1_788_360_000, 0).unwrap(),
            )
            .unwrap();
        let snapshot = database.player_snapshot(identity.usn).unwrap();
        let spec = ProtocolSchema::embedded().call("userLoad").unwrap();

        let Value::Struct(fields) = player_response_projected(spec, &snapshot, false, DeviceClock::new(1_788_360_000, 0).unwrap()).unwrap()
        else {
            panic!("userLoad response is not a struct");
        };
        assert_eq!(
            fields.iter().map(|field| field.id).collect::<Vec<_>>(),
            vec![1, 3]
        );
        let contents = fields.iter().find(|field| field.id == 3).unwrap();
        assert_eq!(contents.value, Value::Struct(Vec::new()));

        drop(database);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn user_save_rejects_cross_area_and_unknown_catalog_ids() {
        use ggfm_master_data::{CostumeRow, GuitarRow, MusicRow};
        use ggfm_protocol::Field;

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ggfm-save-validate-{suffix}.sqlite"));
        let database = DatabaseActor::open(path.clone()).unwrap();
        let row_costume = |id, area| CostumeRow {
            id,
            area,
            currency: "CP".into(),
            official_cost: "10".into(),
            fan_multiplier: 0.3,
            acquisition_type: 0,
            acquisition_id: 0,
            active: true,
        };
        let row_guitar = |id, area| GuitarRow {
            id,
            area,
            currency: "CP".into(),
            official_cost: "10".into(),
            like_multiplier: 1.2,
            acquisition_type: 0,
            acquisition_id: 0,
            active: true,
        };
        let row_music = |id, area| MusicRow {
            id,
            area,
            max_level: 10,
            currency: "CP".into(),
            unlock_cost: 10.0,
            acquisition_type: 0,
            acquisition_id: 0,
            active: true,
        };
        let master = ggfm_master_data::MasterCatalog {
            costumes: vec![row_costume(2, 1), row_costume(201, 2)],
            guitars: vec![row_guitar(1, 1), row_guitar(201, 2)],
            music: vec![row_music(2, 1), row_music(201, 2)],
            ..Default::default()
        };
        let state = AppState {
            database,
            master: std::sync::Arc::new(master),
            session: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            capability: std::sync::Arc::from("save-validate"),
            capability_hash: std::sync::Arc::from(Vec::<u8>::new()),
            endpoint: std::sync::Arc::from("http://127.0.0.1:1"),
            asset_source: None,
            startup_notice_emitted: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let list = |values| Value::List {
            element_type: STRUCT,
            values,
        };
        let request = request(
            "userSave",
            BTreeMap::from([
                (
                    "User_info".into(),
                    list(vec![Value::Struct(vec![
                        Field {
                            id: 4,
                            value: Value::I64(201),
                        },
                        Field {
                            id: 5,
                            value: Value::I64(201),
                        },
                    ])]),
                ),
                (
                    "User_area_info".into(),
                    list(vec![Value::Struct(vec![
                        Field {
                            id: 1,
                            value: Value::I32(2),
                        },
                        Field {
                            id: 5,
                            value: Value::I64(2),
                        },
                        Field {
                            id: 6,
                            value: Value::I64(2),
                        },
                        Field {
                            id: 7,
                            value: Value::I64(1),
                        },
                    ])]),
                ),
                (
                    "User_music".into(),
                    list(vec![
                        Value::Struct(vec![Field {
                            id: 1,
                            value: Value::I64(2),
                        }]),
                        Value::Struct(vec![Field {
                            id: 1,
                            value: Value::I64(999),
                        }]),
                    ]),
                ),
            ]),
        );
        let patch = parse_user_save(&state, &request);
        let core = patch.core.unwrap();
        assert_eq!(core.current_costume, None);
        assert_eq!(core.current_music, None);
        assert_eq!(patch.areas[0].current_costume, None);
        assert_eq!(patch.areas[0].current_music, None);
        assert_eq!(patch.areas[0].current_guitar, None);
        assert_eq!(patch.content.len(), 1);
        assert_eq!(patch.content[0].content_id, 2);
        assert_eq!(patch.content[0].area, 1);

        drop(state);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn buy_contents_aliases_use_the_same_prices_published_in_master_data() {
        fn upgrade_row(id: i64, area: i64, currency: &str, cost: i64, max: i64) -> MasterRow {
            BTreeMap::from([
                ("id".to_owned(), MasterScalar::Integer(id)),
                ("area".to_owned(), MasterScalar::Integer(area)),
                (
                    "goodstype".to_owned(),
                    MasterScalar::Text(currency.to_owned()),
                ),
                ("cost".to_owned(), MasterScalar::Integer(cost)),
                ("maxlevel".to_owned(), MasterScalar::Integer(max)),
            ])
        }

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ggfm-content-spec-{suffix}.sqlite"));
        let database = DatabaseActor::open(path.clone()).unwrap();
        let mut master = ggfm_master_data::MasterCatalog {
            skills: vec![ggfm_master_data::SkillRow {
                id: 2,
                area: 1,
                max_level: 20,
                currency: "CP".into(),
                base_cost: 200,
                cost_increase: 0.0,
                cooldown_seconds: 60.0,
            }],
            units: vec![ggfm_master_data::UnitRow {
                id: 1,
                area: 1,
                max_level: 20,
                currency: "CP".into(),
                base_cost: 400,
                cost_increase: 0.0,
            }],
            ..ggfm_master_data::MasterCatalog::default()
        };
        master.game_data.insert(
            "Skill".into(),
            vec![
                upgrade_row(1, 1, "CP", 100, 20),
                upgrade_row(2, 1, "CP", 200, 20),
                upgrade_row(3, 1, "CP", 300, 20),
            ],
        );
        master
            .game_data
            .insert("Unit".into(), vec![upgrade_row(1, 1, "CP", 400, 20)]);
        master
            .game_data
            .insert("Follower".into(), vec![upgrade_row(7, 1, "GP", 900, 30)]);
        let state = AppState {
            database,
            master: std::sync::Arc::new(master),
            session: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            capability: std::sync::Arc::from("content-spec"),
            capability_hash: std::sync::Arc::from(Vec::<u8>::new()),
            endpoint: std::sync::Arc::from("http://127.0.0.1:1"),
            asset_source: None,
            startup_notice_emitted: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };

        let skill = content_purchase_spec(&state, "User_skill", 2).unwrap();
        assert_eq!(skill.kind, ContentKind::Skill);
        assert_eq!(skill.price, 1.0);
        assert_eq!(skill.price_increment, 14.0 / 18.0);
        let unit = content_purchase_spec(&state, "mate", 1).unwrap();
        assert_eq!(unit.kind, ContentKind::Unit);
        assert_eq!(unit.price, 1.0);
        assert_eq!(unit.price_increment, 9.0 / 19.0);
        let follower = content_purchase_spec(&state, "Follower", 7).unwrap();
        assert_eq!(follower.kind, ContentKind::Follower);
        assert_eq!(follower.currency, CurrencyKind::Ch1Like);
        assert_eq!(follower.price, 90.0);

        let pass = BTreeMap::from([
            ("id".into(), MasterScalar::Integer(7)),
            ("paidpoint".into(), MasterScalar::Integer(5_000)),
            ("adpoint".into(), MasterScalar::Integer(2_000)),
            ("pointprice".into(), MasterScalar::Integer(50)),
        ]);
        let overlay = PassMasterOverlay { active_season: 7, local_month: 9 };
        for (field, expected) in [("paidpoint", 50_000), ("adpoint", 20_000), ("pointprice", 50)] {
            assert_eq!(
                transform_master_value(&state, "SubscribePass", field, &pass, &pass[field], overlay),
                MasterScalar::Integer(expected),
            );
        }
        assert_eq!(pass["paidpoint"], MasterScalar::Integer(5_000));

        drop(state);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn discovery_login_contains_only_identity_fields() {
        let schema = ProtocolSchema::embedded();
        let value = user_login_identity_response(
            schema.call("userLogin").expect("userLogin schema"),
            &ggfm_domain::UserIdentity {
                usn: 7,
                user_id: "1000000000007".to_owned(),
            },
        )
        .expect("identity response");

        let Value::Struct(data_fields) = value else {
            panic!("userLogin response data must be a struct");
        };
        assert_eq!(
            data_fields.len(),
            1,
            "Area_data/User_contents must be absent"
        );
        assert_eq!(data_fields[0].id, 1, "the only outer field must be User");
        let Value::Struct(user_fields) = &data_fields[0].value else {
            panic!("User must be a struct");
        };
        assert_eq!(
            user_fields.iter().map(|field| field.id).collect::<Vec<_>>(),
            vec![1, 2],
            "discovery login must not recursively default gameplay fields"
        );
    }

    #[test]
    fn follower_quest_history_projects_as_one_legacy_active_row() {
        let values = follower_quest_wire_values(&[
            ggfm_domain::FollowerQuestSnapshot {
                current_id: 1,
                complete_id: 1,
                completed: true,
                infinite: false,
                condition_values: [10.0, 20.0, 30.0],
                claimed: [true, true, true],
            },
            ggfm_domain::FollowerQuestSnapshot {
                current_id: 2,
                complete_id: 1,
                completed: false,
                infinite: true,
                condition_values: [4.0, 5.0, 6.0],
                claimed: [true, false, false],
            },
        ])
        .unwrap();
        assert_eq!(values.len(), 1);
        let Value::Struct(fields) = &values[0] else {
            panic!("follower quest row must be a struct");
        };
        assert_eq!(field_integer(fields, 1), Some(1));
        assert_eq!(field_integer(fields, 2), Some(2));
        assert_eq!(field_integer(fields, 3), Some(1));
        assert_eq!(field_number(fields, 4), Some(4.0));
        assert_eq!(field_integer(fields, 7), Some(1));
        assert_eq!(field_integer(fields, 10), Some(1));
    }

    #[test]
    fn user_save_music_uses_the_v8_five_field_layout() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ggfm-save-parse-{suffix}.sqlite"));
        let database = DatabaseActor::open(path.clone()).unwrap();
        let state = AppState {
            database,
            master: std::sync::Arc::new(ggfm_master_data::MasterCatalog::default()),
            session: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            capability: std::sync::Arc::from("parse"),
            capability_hash: std::sync::Arc::from(Vec::<u8>::new()),
            endpoint: std::sync::Arc::from("http://127.0.0.1:1"),
            asset_source: None,
            startup_notice_emitted: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let request = request(
            "userSave",
            BTreeMap::from([(
                "User_music".into(),
                Value::List {
                    element_type: STRUCT,
                    values: vec![Value::Struct(vec![
                        ggfm_protocol::Field {
                            id: 1,
                            value: Value::I32(12),
                        },
                        ggfm_protocol::Field {
                            id: 2,
                            value: Value::I16(3),
                        },
                        ggfm_protocol::Field {
                            id: 3,
                            value: Value::I16(2),
                        },
                        ggfm_protocol::Field {
                            id: 4,
                            value: Value::I64(1),
                        },
                        ggfm_protocol::Field {
                            id: 5,
                            value: Value::I32(77),
                        },
                    ])],
                },
            )]),
        );

        let patch = parse_user_save(&state, &request);
        assert_eq!(patch.music_states.len(), 1);
        assert_eq!(patch.music_states[0].music_id, 12);
        assert!(patch.music_states[0].encore_appears);
        assert_eq!(patch.music_states[0].encore_follower_id, Some(77));

        drop(state);
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(ggfm_private_oracle)]
    fn value_shape(value: &Value) -> String {
        match value {
            Value::Bool(_) => "bool".to_owned(),
            Value::Byte(_) => "byte".to_owned(),
            Value::Double(_) => "double".to_owned(),
            Value::I16(_) => "i16".to_owned(),
            Value::I32(_) => "i32".to_owned(),
            Value::I64(_) => "i64".to_owned(),
            Value::String(_) => "string".to_owned(),
            Value::Struct(fields) => format!(
                "struct{{{}}}",
                fields
                    .iter()
                    .map(|field| format!("{}:{}", field.id, value_shape(&field.value)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Value::Map {
                key_type,
                value_type,
                entries,
            } => format!(
                "map<{key_type},{value_type}>[{}]{{{}}}",
                entries.len(),
                entries
                    .iter()
                    .map(|(key, value)| format!("{}=>{}", value_shape(key), value_shape(value)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Value::Set {
                element_type,
                values,
            } => format!(
                "set<{element_type}>[{}]{{{}}}",
                values.len(),
                values.iter().map(value_shape).collect::<Vec<_>>().join(",")
            ),
            Value::List {
                element_type,
                values,
            } => format!(
                "list<{element_type}>[{}]{{{}}}",
                values.len(),
                values.iter().map(value_shape).collect::<Vec<_>>().join(",")
            ),
        }
    }

    #[cfg(ggfm_private_oracle)]
    fn value_differences(path: &str, actual: &Value, expected: &Value, out: &mut Vec<String>) {
        match (actual, expected) {
            (Value::Struct(actual), Value::Struct(expected)) => {
                for (left, right) in actual.iter().zip(expected) {
                    if left.id != right.id {
                        out.push(format!("{path}: field {} != {}", left.id, right.id));
                        continue;
                    }
                    value_differences(
                        &format!("{path}.{}", left.id),
                        &left.value,
                        &right.value,
                        out,
                    );
                }
            }
            (
                Value::Map {
                    entries: actual, ..
                },
                Value::Map {
                    entries: expected, ..
                },
            ) => {
                for (index, ((actual_key, actual_value), (expected_key, expected_value))) in
                    actual.iter().zip(expected).enumerate()
                {
                    value_differences(
                        &format!("{path}[{index}].key"),
                        actual_key,
                        expected_key,
                        out,
                    );
                    value_differences(
                        &format!("{path}[{index}].value"),
                        actual_value,
                        expected_value,
                        out,
                    );
                }
            }
            (
                Value::List { values: actual, .. },
                Value::List {
                    values: expected, ..
                },
            )
            | (
                Value::Set { values: actual, .. },
                Value::Set {
                    values: expected, ..
                },
            ) => {
                for (index, (left, right)) in actual.iter().zip(expected).enumerate() {
                    value_differences(&format!("{path}[{index}]"), left, right, out);
                }
            }
            _ if actual != expected => out.push(format!("{path}: {actual:?} != {expected:?}")),
            _ => {}
        }
    }


    #[cfg(ggfm_private_oracle)]
    fn response_data(raw: &[u8], has_server_time: bool) -> Value {
        let data_id = if has_server_time { 5 } else { 4 };
        ggfm_protocol::read_struct(raw)
            .expect("decode response")
            .into_iter()
            .find(|field| field.id == data_id)
            .expect("response Data field")
            .value
    }





}
