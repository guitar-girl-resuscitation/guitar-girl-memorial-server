use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use rusqlite::{Connection, OpenFlags, types::ValueRef};

pub const MASTER_SCHEMA_VERSION: i64 = 2;

#[derive(Debug, thiserror::Error)]
pub enum MasterDataError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error("master.sqlite schema {actual} is not supported (expected {expected})")]
    UnsupportedSchema { actual: i64, expected: i64 },
    #[error("master.sqlite failed integrity_check: {0}")]
    Integrity(String),
    #[error("master.sqlite is missing required table {0}")]
    MissingTable(&'static str),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MasterMetadata {
    pub version: i64,
    pub source_asset_count: i64,
    pub source_table_count: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FanRequirement {
    pub area: i32,
    pub grade: i32,
    pub official_count: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CostumeRow {
    pub id: i64,
    pub area: i32,
    pub currency: String,
    pub official_cost: String,
    pub fan_multiplier: f64,
    pub acquisition_type: i32,
    pub acquisition_id: i32,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GuitarRow {
    pub id: i64,
    pub area: i32,
    pub currency: String,
    pub official_cost: String,
    pub like_multiplier: f64,
    pub acquisition_type: i32,
    pub acquisition_id: i32,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkillRow {
    pub id: i64,
    pub area: i32,
    pub max_level: i32,
    pub currency: String,
    pub base_cost: i64,
    pub cost_increase: f64,
    pub cooldown_seconds: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UnitRow {
    pub id: i64,
    pub area: i32,
    pub max_level: i32,
    pub currency: String,
    pub base_cost: i64,
    pub cost_increase: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MusicRow {
    pub id: i64,
    pub area: i32,
    pub max_level: i32,
    pub currency: String,
    pub unlock_cost: f64,
    pub acquisition_type: i32,
    pub acquisition_id: i32,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MusicLevelRow {
    pub level: i32,
    pub official_encore_cooldown_seconds: f64,
    pub gift_amount: i32,
    pub follower_profile_exp: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ch3StageRow {
    pub id: i64,
    pub chapter: i32,
    pub stage_index: i32,
    pub stage_type: String,
    pub reward_group_id: i64,
    pub percent_reward_groups: String,
    pub goal_scores: [i64; 3],
    pub bonus_profile_ids: Vec<i64>,
    pub bonus_music_id: i64,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PassSeasonRow {
    pub season: i64,
    pub subscribe_id: i64,
    pub ticket_collection_id: i64,
    pub point_price: i64,
    pub paid_point: i64,
    pub ad_cooldown_seconds: i64,
    pub ad_point: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RewardGroupRow {
    pub row_id: i64,
    pub group: i64,
    pub reward_type: i16,
    pub reward_id: i32,
    pub quantity: f64,
    pub first_purchase_quantity: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShopRow {
    pub id: i64,
    pub product_type: i32,
    pub reward_group: i64,
    pub sell_type: i32,
    pub sell_value: i64,
    pub active: bool,
    pub condition: i32,
    pub condition_value: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PassRewardRow {
    pub id: i64,
    pub group: i64,
    pub step: i32,
    pub goal: i64,
    pub free_reward_group: i64,
    pub paid_reward_group: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FollowerProfileLevelRow {
    pub id: i64,
    pub profile_id: i64,
    pub level: i32,
    pub required_exp: f64,
    pub reward_group: i64,
    pub add_candy: i32,
    pub add_max_ap: i32,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ch3ScoreRow {
    pub level: i32,
    pub character_score: i32,
    pub follower_score: i32,
    pub music_score: i32,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ch3ChapterRow {
    pub chapter: i32,
    pub unlock_level: i32,
    pub reward_groups: [i64; 3],
    pub goal_stars: [i32; 3],
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PercentRewardRow {
    pub row_id: i64,
    pub group: i64,
    pub percent: i32,
    pub reward_type: i16,
    pub reward_id: i32,
    pub quantity: f64,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FollowerGiftRow {
    pub id: i64,
    pub gift_type: i32,
    pub experience: i64,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FollowerQuestRow {
    pub id: i64,
    pub quest_type: i32,
    pub thresholds: [f64; 3],
    pub reward_groups: [i64; 3],
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectRewardRow {
    pub id: i64,
    pub group: i64,
    pub reward_group: i64,
    pub alternate_reward_group: i64,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SamSeckRow {
    pub id: i64,
    pub condition_type: String,
    pub condition: f64,
    pub mail_reward_id: i64,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AchievementRow {
    pub id: i64,
    pub conditions: Vec<f64>,
    pub reward_type: String,
    pub reward_values: Vec<i64>,
    pub max_level: i32,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DailyMissionRow {
    pub id: i64,
    pub condition: f64,
    pub reward_type: String,
    pub reward_value: i64,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdRow {
    pub id: i64,
    pub reward_type: String,
    pub reward_group: i64,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdLevelRow {
    pub id: i64,
    pub group: i64,
    pub level: i32,
    pub required_experience: i32,
    pub reward_group: i64,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DailyRewardRow {
    pub id: i64,
    pub group: i64,
    pub day: i32,
    pub reward_type: i16,
    pub reward_id: i32,
    pub quantity: f64,
    pub custom_icon_type: String,
    pub custom_icon_sprite: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MasterScalar {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

pub type MasterRow = BTreeMap<String, MasterScalar>;

/// The field names are those of GetGameDataListRetDataInfo. The table names
/// remain private input-package facts and are read only from generated
/// master.sqlite; no original game row is compiled into the server.
pub const GAME_DATA_TABLES: &[(&str, &str)] = &[
    ("Consume", "Consume"),
    ("Music", "Music"),
    ("Costume", "Costume"),
    ("Prop", "Prop"),
    ("Follower", "Follower"),
    ("Buff", "Buff"),
    ("Unit", "Unit"),
    ("Skill", "Skill"),
    ("Achievement", "Achievement"),
    ("Daily_mission", "DailyMission"),
    ("Music_level", "MusicLevel"),
    ("Shop", "Shop"),
    ("Reward_group", "RewardGroup"),
    ("Fan", "Fan"),
    ("SystemNotification", "SystemNotification"),
    ("SystemString", "SystemString"),
    ("SubscribeList", "SubscribeList"),
    ("SubscribePassReward", "SubscribePassReward"),
    ("SubscribePass", "SubscribePass"),
    ("Guitar", "Guitar"),
    (
        "SubscribePassRewardInformation",
        "SubscribePassRewardInformation",
    ),
    ("Ticketcollection", "TicketCollection"),
    ("Character", "Character"),
    ("Samseckevent", "SamSeckEvent"),
    ("Localpush", "LocalPush"),
    ("Followerquest", "FollowerQuest"),
    ("Proplevel", "PropLevel"),
    ("Follower_profile", "FollowerProfile"),
    ("Passgoodsshop", "PassGoodsShop"),
    ("Followergiftitem", "FollowerGiftItem"),
    ("Followerprofilelevel", "FollowerProfileLevel"),
    ("Ad_list", "ADList"),
    ("Chthird_stage", "ChThirdStage"),
    ("Chthird_score", "ChThirdScore"),
    ("Chthird_chapter", "ChThirdChapter"),
    ("Percent", "Percent"),
    ("Ad_level", "ADLevel"),
    ("Event_list", "EventList"),
    ("Select_reward", "SelectReward"),
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MasterCatalog {
    pub metadata: MasterMetadata,
    pub fan_requirements: Vec<FanRequirement>,
    pub costumes: Vec<CostumeRow>,
    pub guitars: Vec<GuitarRow>,
    pub skills: Vec<SkillRow>,
    pub units: Vec<UnitRow>,
    pub music: Vec<MusicRow>,
    pub music_levels: Vec<MusicLevelRow>,
    pub ch3_stages: Vec<Ch3StageRow>,
    pub pass_seasons: Vec<PassSeasonRow>,
    pub reward_groups: Vec<RewardGroupRow>,
    pub shops: Vec<ShopRow>,
    pub pass_rewards: Vec<PassRewardRow>,
    pub follower_profile_levels: Vec<FollowerProfileLevelRow>,
    pub ch3_scores: Vec<Ch3ScoreRow>,
    pub ch3_chapters: Vec<Ch3ChapterRow>,
    pub percent_rewards: Vec<PercentRewardRow>,
    pub follower_gifts: Vec<FollowerGiftRow>,
    pub follower_quests: Vec<FollowerQuestRow>,
    pub select_rewards: Vec<SelectRewardRow>,
    pub sam_seck: Vec<SamSeckRow>,
    pub achievements: Vec<AchievementRow>,
    pub daily_missions: Vec<DailyMissionRow>,
    pub ads: Vec<AdRow>,
    pub ad_levels: Vec<AdLevelRow>,
    pub daily_rewards: Vec<DailyRewardRow>,
    pub game_data: BTreeMap<String, Vec<MasterRow>>,
}

impl MasterCatalog {
    pub fn rewards(&self, group: i64) -> impl Iterator<Item = &RewardGroupRow> {
        self.reward_groups
            .iter()
            .filter(move |row| row.group == group)
    }

    pub fn shop(&self, id: i64) -> Option<&ShopRow> {
        self.shops.iter().find(|row| row.id == id)
    }

    pub fn pass_reward(&self, group: i64, step: i32) -> Option<&PassRewardRow> {
        self.pass_rewards
            .iter()
            .find(|row| row.group == group && row.step == step)
    }

    pub fn percent_rewards(&self, group: i64) -> impl Iterator<Item = &PercentRewardRow> {
        self.percent_rewards
            .iter()
            .filter(move |row| row.group == group && row.active)
    }
}

pub struct MasterData {
    connection: Connection,
    metadata: MasterMetadata,
    tables: BTreeSet<String>,
}

impl MasterData {
    pub fn open_read_only(path: &Path) -> Result<Self, MasterDataError> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.pragma_update(None, "query_only", true)?;
        let integrity: String =
            connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(MasterDataError::Integrity(integrity));
        }
        let metadata = connection.query_row(
            "SELECT version,source_asset_count,source_table_count FROM ggfm_master_schema LIMIT 1",
            [],
            |row| {
                Ok(MasterMetadata {
                    version: row.get(0)?,
                    source_asset_count: row.get(1)?,
                    source_table_count: row.get(2)?,
                })
            },
        )?;
        if metadata.version != MASTER_SCHEMA_VERSION {
            return Err(MasterDataError::UnsupportedSchema {
                actual: metadata.version,
                expected: MASTER_SCHEMA_VERSION,
            });
        }
        let mut statement = connection.prepare(
            "SELECT destination_table FROM ggfm_master_tables ORDER BY destination_table",
        )?;
        let tables: BTreeSet<String> = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        drop(statement);
        for required in [
            "Achievement",
            "ADList",
            "ChThirdChapter",
            "ChThirdScore",
            "ChThirdStage",
            "Costume",
            "DailyMission",
            "DailyReward",
            "Fan",
            "FollowerGiftItem",
            "FollowerQuest",
            "Guitar",
            "Music",
            "Percent",
            "FollowerProfileLevel",
            "RewardGroup",
            "SamSeckEvent",
            "SelectReward",
            "Shop",
            "Skill",
            "SubscribePass",
            "SubscribePassReward",
            "Unit",
        ] {
            if !tables.contains(required) {
                return Err(MasterDataError::MissingTable(required));
            }
        }
        for (_, required) in GAME_DATA_TABLES {
            if !tables.contains(*required) {
                return Err(MasterDataError::MissingTable(required));
            }
        }
        Ok(Self {
            connection,
            metadata,
            tables,
        })
    }

    pub fn metadata(&self) -> &MasterMetadata {
        &self.metadata
    }

    pub fn has_table(&self, name: &str) -> bool {
        self.tables.contains(name)
    }

    pub fn fan_requirements(&self) -> Result<Vec<FanRequirement>, MasterDataError> {
        let mut statement = self
            .connection
            .prepare("SELECT i_Area,i_Grade,i_FanCount FROM Fan ORDER BY i_Area,i_Grade")?;
        Ok(statement
            .query_map([], |row| {
                Ok(FanRequirement {
                    area: row.get(0)?,
                    grade: row.get(1)?,
                    official_count: row.get(2)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn costumes(&self) -> Result<Vec<CostumeRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_Area,s_GoodsType,s_Cost,f_FanMultiply,",
            "i_AcquisitionType,i_AcquisitionId,b_IsActive FROM Costume ORDER BY i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(CostumeRow {
                    id: row.get(0)?,
                    area: row.get(1)?,
                    currency: row.get(2)?,
                    official_cost: row.get(3)?,
                    fan_multiplier: row.get(4)?,
                    acquisition_type: row.get(5)?,
                    acquisition_id: row.get(6)?,
                    active: row.get::<_, i64>(7)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn guitars(&self) -> Result<Vec<GuitarRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_Area,s_GoodsType,s_Cost,d_IncrementValue,",
            "i_AcquisitionType,i_AcquisitionId,b_IsActive FROM Guitar ORDER BY i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(GuitarRow {
                    id: row.get(0)?,
                    area: row.get(1)?,
                    currency: row.get(2)?,
                    official_cost: row.get(3)?,
                    like_multiplier: row.get(4)?,
                    acquisition_type: row.get(5)?,
                    acquisition_id: row.get(6)?,
                    active: row.get::<_, i64>(7)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn skills(&self) -> Result<Vec<SkillRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_Area,i_MaxLevel,s_GoodsType,i_Cost,",
            "f_CostIncreaseValue,f_Cooltime FROM Skill ORDER BY i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(SkillRow {
                    id: row.get(0)?,
                    area: row.get(1)?,
                    max_level: row.get(2)?,
                    currency: row.get(3)?,
                    base_cost: row.get(4)?,
                    cost_increase: row.get(5)?,
                    cooldown_seconds: row.get(6)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn units(&self) -> Result<Vec<UnitRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_Area,i_MaxLevel,s_GoodsType,i_Cost,",
            "f_CostIncreaseValue FROM Unit WHERE b_IsActive!=0 ORDER BY i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(UnitRow {
                    id: row.get(0)?,
                    area: row.get(1)?,
                    max_level: row.get(2)?,
                    currency: row.get(3)?,
                    base_cost: row.get(4)?,
                    cost_increase: row.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn music(&self) -> Result<Vec<MusicRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_Area,i_MaxLevel,s_GoodsType,d_UnlockCost,i_AcquisitionType,",
            "i_AcquisitionId,b_IsActive FROM Music ORDER BY i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(MusicRow {
                    id: row.get(0)?,
                    area: row.get(1)?,
                    max_level: row.get(2)?,
                    currency: row.get(3)?,
                    unlock_cost: row.get(4)?,
                    acquisition_type: row.get(5)?,
                    acquisition_id: row.get(6)?,
                    active: row.get::<_, i64>(7)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn music_levels(&self) -> Result<Vec<MusicLevelRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Level,f_EncoreBonusAppearCooltime_Sec,i_EncoreBonusGiftAmount,",
            "i_EncoreFollowerProfileExp FROM MusicLevel ORDER BY i_Level"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(MusicLevelRow {
                    level: row.get(0)?,
                    official_encore_cooldown_seconds: row.get(1)?,
                    gift_amount: row.get(2)?,
                    follower_profile_exp: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn ch3_stages(&self) -> Result<Vec<Ch3StageRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_Chapter,i_StageIndex,s_StageType,i_RewardGroupID,",
            "s_PercentRewardGroupID,i_GoalScore1,i_GoalScore2,i_GoalScore3,",
            "s_BonusProfileID,i_BonusMusicID,b_IsActive ",
            "FROM ChThirdStage ORDER BY i_Chapter,i_StageIndex,i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(Ch3StageRow {
                    id: row.get(0)?,
                    chapter: row.get(1)?,
                    stage_index: row.get(2)?,
                    stage_type: row.get(3)?,
                    reward_group_id: row.get(4)?,
                    percent_reward_groups: row.get(5)?,
                    goal_scores: [row.get(6)?, row.get(7)?, row.get(8)?],
                    bonus_profile_ids: parse_id_list(&row.get::<_, String>(9)?),
                    bonus_music_id: row.get(10)?,
                    active: row.get::<_, i64>(11)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn pass_seasons(&self) -> Result<Vec<PassSeasonRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_SubscribeID,i_TicketCollectionId,i_PointPrice,i_PaidPoint,",
            "i_ADCoolTime,i_ADPoint FROM SubscribePass ORDER BY i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(PassSeasonRow {
                    season: row.get(0)?,
                    subscribe_id: row.get(1)?,
                    ticket_collection_id: row.get(2)?,
                    point_price: row.get(3)?,
                    paid_point: row.get(4)?,
                    ad_cooldown_seconds: row.get(5)?,
                    ad_point: row.get(6)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn reward_groups(&self) -> Result<Vec<RewardGroupRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_Group,i_RewardType,i_RewardID,l_RewardQuantity,i_BuyFirstQuantity ",
            "FROM RewardGroup ORDER BY i_Group,i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(RewardGroupRow {
                    row_id: row.get(0)?,
                    group: row.get(1)?,
                    reward_type: row.get(2)?,
                    reward_id: row.get(3)?,
                    quantity: row.get::<_, f64>(4)?,
                    first_purchase_quantity: row.get::<_, f64>(5)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn shops(&self) -> Result<Vec<ShopRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_ProductType,i_RewardGroup,i_SellType,i_SellValue,b_IsActive,",
            "i_Condition,i_ConditionValue FROM Shop ORDER BY i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(ShopRow {
                    id: row.get(0)?,
                    product_type: row.get(1)?,
                    reward_group: row.get(2)?,
                    sell_type: row.get(3)?,
                    sell_value: row.get(4)?,
                    active: row.get::<_, i64>(5)? != 0,
                    condition: row.get(6)?,
                    condition_value: row.get(7)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn pass_rewards(&self) -> Result<Vec<PassRewardRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_Group,i_Step,i_Goal,i_FreeRewardGroup,i_PaidRewardGroup ",
            "FROM SubscribePassReward ORDER BY i_Group,i_Step"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(PassRewardRow {
                    id: row.get(0)?,
                    group: row.get(1)?,
                    step: row.get(2)?,
                    goal: row.get(3)?,
                    free_reward_group: row.get(4)?,
                    paid_reward_group: row.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn follower_profile_levels(&self) -> Result<Vec<FollowerProfileLevelRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_ProfileID,i_Level,d_RequireEXP,i_RewardGroup,i_AddCandy,",
            "i_AddMaxAp,b_IsActive FROM FollowerProfileLevel ORDER BY i_ProfileID,i_Level"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(FollowerProfileLevelRow {
                    id: row.get(0)?,
                    profile_id: row.get(1)?,
                    level: row.get(2)?,
                    required_exp: row.get(3)?,
                    reward_group: row.get(4)?,
                    add_candy: row.get(5)?,
                    add_max_ap: row.get(6)?,
                    active: row.get::<_, i64>(7)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn ch3_scores(&self) -> Result<Vec<Ch3ScoreRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Level,i_CharacterScore,i_FollowerScore,i_MusicScore,b_IsActive ",
            "FROM ChThirdScore ORDER BY i_Level"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(Ch3ScoreRow {
                    level: row.get(0)?,
                    character_score: row.get(1)?,
                    follower_score: row.get(2)?,
                    music_score: row.get(3)?,
                    active: row.get::<_, i64>(4)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn ch3_chapters(&self) -> Result<Vec<Ch3ChapterRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_unlockLevel,i_RewardGroupID1,i_GoalStar1,i_RewardGroupID2,",
            "i_GoalStar2,i_RewardGroupID3,i_GoalStar3,b_IsActive FROM ChThirdChapter ORDER BY i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(Ch3ChapterRow {
                    chapter: row.get(0)?,
                    unlock_level: row.get(1)?,
                    reward_groups: [row.get(2)?, row.get(4)?, row.get(6)?],
                    goal_stars: [row.get(3)?, row.get(5)?, row.get(7)?],
                    active: row.get::<_, i64>(8)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn percent_rewards(&self) -> Result<Vec<PercentRewardRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_GroupID,i_Percent,i_RewardType,i_RewardID,l_RewardQuantity,b_IsActive ",
            "FROM Percent ORDER BY i_GroupID,i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(PercentRewardRow {
                    row_id: row.get(0)?,
                    group: row.get(1)?,
                    percent: row.get(2)?,
                    reward_type: row.get(3)?,
                    reward_id: row.get(4)?,
                    quantity: row.get(5)?,
                    active: row.get::<_, i64>(6)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn follower_gifts(&self) -> Result<Vec<FollowerGiftRow>, MasterDataError> {
        let mut statement = self.connection.prepare(
            "SELECT i_Id,i_GiftType,d_Value,b_IsActive FROM FollowerGiftItem ORDER BY i_Id",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(FollowerGiftRow {
                    id: row.get(0)?,
                    gift_type: row.get(1)?,
                    experience: row.get::<_, f64>(2)?.max(0.0) as i64,
                    active: row.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn follower_quests(&self) -> Result<Vec<FollowerQuestRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,d_Condition1_1,d_Condition2_1,d_Condition3_1,",
            "i_RewardGroup1,i_RewardGroup2,i_RewardGroup3,b_IsActive,i_Type FROM FollowerQuest ORDER BY i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(FollowerQuestRow {
                    id: row.get(0)?,
                    thresholds: [row.get(1)?, row.get(2)?, row.get(3)?],
                    reward_groups: [row.get(4)?, row.get(5)?, row.get(6)?],
                    active: row.get::<_, i64>(7)? != 0,
                    quest_type: row.get(8)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn select_rewards(&self) -> Result<Vec<SelectRewardRow>, MasterDataError> {
        let mut statement = self.connection.prepare(
            "SELECT i_Id,i_GroupId,i_RewardGroupId,i_AltRewardGroupId,b_IsActive FROM SelectReward ORDER BY i_Id",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(SelectRewardRow {
                    id: row.get(0)?,
                    group: row.get(1)?,
                    reward_group: row.get(2)?,
                    alternate_reward_group: row.get(3)?,
                    active: row.get::<_, i64>(4)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn sam_seck(&self) -> Result<Vec<SamSeckRow>, MasterDataError> {
        let mut statement = self.connection.prepare(
            "SELECT i_Id,s_ConditionType,d_Condition,i_MailRewardID,b_IsActive FROM SamSeckEvent ORDER BY i_Id",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(SamSeckRow {
                    id: row.get(0)?,
                    condition_type: row.get(1)?,
                    condition: row.get(2)?,
                    mail_reward_id: row.get(3)?,
                    active: row.get::<_, i64>(4)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn achievements(&self) -> Result<Vec<AchievementRow>, MasterDataError> {
        let condition_columns = (1..=20)
            .map(|index| format!("s_Condition_{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let reward_columns = (1..=20)
            .map(|index| format!("i_Reward_{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT i_Id,{condition_columns},s_RewardType,{reward_columns},i_MaxLevel,b_IsActive FROM Achievement ORDER BY i_Id"
        );
        let mut statement = self.connection.prepare(&sql)?;
        Ok(statement
            .query_map([], |row| {
                let conditions = (0..20)
                    .map(|index| {
                        row.get::<_, String>(1 + index)
                            .ok()
                            .and_then(|value| value.parse::<f64>().ok())
                            .unwrap_or(0.0)
                    })
                    .collect();
                let reward_values = (0..20)
                    .map(|index| row.get::<_, i64>(22 + index).unwrap_or(0))
                    .collect();
                Ok(AchievementRow {
                    id: row.get(0)?,
                    conditions,
                    reward_type: row.get(21)?,
                    reward_values,
                    max_level: row.get(42)?,
                    active: row.get::<_, i64>(43)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn daily_missions(&self) -> Result<Vec<DailyMissionRow>, MasterDataError> {
        let mut statement = self.connection.prepare(
            "SELECT i_Id,d_Condition_1,s_RewardType,i_Reward_1,b_IsActive FROM DailyMission ORDER BY i_Id",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(DailyMissionRow {
                    id: row.get(0)?,
                    condition: row.get(1)?,
                    reward_type: row.get(2)?,
                    reward_value: row.get(3)?,
                    active: row.get::<_, i64>(4)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn ads(&self) -> Result<Vec<AdRow>, MasterDataError> {
        let mut statement = self.connection.prepare(
            "SELECT i_Id,s_RewardType,i_RewardGroup,b_IsActive FROM ADList ORDER BY i_Id",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(AdRow {
                    id: row.get(0)?,
                    reward_type: row.get(1)?,
                    reward_group: row.get(2)?,
                    active: row.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn ad_levels(&self) -> Result<Vec<AdLevelRow>, MasterDataError> {
        let mut statement = self.connection.prepare(concat!(
            "SELECT i_Id,i_GroupID,i_Level,i_RequireEXP,i_RewardGroup,b_IsActive ",
            "FROM ADLevel ORDER BY i_GroupID,i_Level,i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(AdLevelRow {
                    id: row.get(0)?,
                    group: row.get(1)?,
                    level: row.get(2)?,
                    required_experience: row.get(3)?,
                    reward_group: row.get(4)?,
                    active: row.get::<_, i64>(5)? != 0,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn daily_rewards(&self) -> Result<Vec<DailyRewardRow>, MasterDataError> {
        let mut statement=self.connection.prepare(concat!(
            "SELECT i_Id,i_Group,i_Day,i_RewardType,i_RewardID,l_RewardQuantity,s_CustomIconType,s_CustomIconSprite ",
            "FROM DailyReward ORDER BY i_Group,i_Day,i_Id"
        ))?;
        Ok(statement
            .query_map([], |row| {
                Ok(DailyRewardRow {
                    id: row.get(0)?,
                    group: row.get(1)?,
                    day: row.get(2)?,
                    reward_type: row.get(3)?,
                    reward_id: row.get(4)?,
                    quantity: row.get(5)?,
                    custom_icon_type: row.get(6)?,
                    custom_icon_sprite: row.get(7)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn load_catalog(&self) -> Result<MasterCatalog, MasterDataError> {
        Ok(MasterCatalog {
            metadata: self.metadata.clone(),
            fan_requirements: self.fan_requirements()?,
            costumes: self.costumes()?,
            guitars: self.guitars()?,
            skills: self.skills()?,
            units: self.units()?,
            music: self.music()?,
            music_levels: self.music_levels()?,
            ch3_stages: self.ch3_stages()?,
            pass_seasons: self.pass_seasons()?,
            reward_groups: self.reward_groups()?,
            shops: self.shops()?,
            pass_rewards: self.pass_rewards()?,
            follower_profile_levels: self.follower_profile_levels()?,
            ch3_scores: self.ch3_scores()?,
            ch3_chapters: self.ch3_chapters()?,
            percent_rewards: self.percent_rewards()?,
            follower_gifts: self.follower_gifts()?,
            follower_quests: self.follower_quests()?,
            select_rewards: self.select_rewards()?,
            sam_seck: self.sam_seck()?,
            achievements: self.achievements()?,
            daily_missions: self.daily_missions()?,
            ads: self.ads()?,
            ad_levels: self.ad_levels()?,
            daily_rewards: self.daily_rewards()?,
            game_data: self.game_data()?,
        })
    }

    pub fn game_data(&self) -> Result<BTreeMap<String, Vec<MasterRow>>, MasterDataError> {
        let mut result = BTreeMap::new();
        for (field_name, table_name) in GAME_DATA_TABLES {
            let sql = format!("SELECT * FROM \"{table_name}\"");
            let mut statement = self.connection.prepare(&sql)?;
            let names = statement
                .column_names()
                .into_iter()
                .map(normalize_column_name)
                .collect::<Vec<_>>();
            let rows = statement
                .query_map([], |row| {
                    let mut values = MasterRow::new();
                    for (index, name) in names.iter().enumerate() {
                        let value = match row.get_ref(index)? {
                            ValueRef::Null => MasterScalar::Null,
                            ValueRef::Integer(value) => MasterScalar::Integer(value),
                            ValueRef::Real(value) => MasterScalar::Real(value),
                            ValueRef::Text(value) => {
                                MasterScalar::Text(String::from_utf8_lossy(value).into_owned())
                            }
                            ValueRef::Blob(value) => MasterScalar::Blob(value.to_vec()),
                        };
                        values.insert(name.clone(), value);
                    }
                    Ok(values)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            result.insert((*field_name).to_owned(), rows);
        }
        Ok(result)
    }
}

pub fn normalize_column_name(value: &str) -> String {
    let value = value
        .get(2..)
        .filter(|_| value.as_bytes().get(1) == Some(&b'_'))
        .unwrap_or(value);
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn parse_id_list(value: &str) -> Vec<i64> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|part| part.parse().ok())
        .filter(|id| *id > 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn rejects_incomplete_master_database() {
        let path = std::env::temp_dir().join(format!(
            "ggfm-incomplete-master-{}.sqlite",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE ggfm_master_schema(version,source_asset_count,source_table_count);\
                 INSERT INTO ggfm_master_schema VALUES(2,1,0);\
                 CREATE TABLE ggfm_master_tables(destination_table TEXT);",
            )
            .unwrap();
        drop(connection);
        assert!(matches!(
            MasterData::open_read_only(&path),
            Err(MasterDataError::MissingTable(_))
        ));
        std::fs::remove_file(path).unwrap();
    }
}
