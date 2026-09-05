use serde::{Deserialize, Serialize};

mod consume_reward;
pub use consume_reward::{ConsumeReward, QuantityUnit, RewardRules, fan_reward_count};

pub type Usn = i64;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserIdentity {
    pub usn: Usn,
    pub user_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceClock {
    pub unix_seconds: i64,
    pub utc_offset_minutes: i32,
}

impl DeviceClock {
    pub fn new(unix_seconds: i64, utc_offset_minutes: i32) -> Result<Self, DomainError> {
        if !(-14 * 60..=14 * 60).contains(&utc_offset_minutes) {
            return Err(DomainError::InvalidUtcOffset(utc_offset_minutes));
        }
        Ok(Self {
            unix_seconds,
            utc_offset_minutes,
        })
    }

    pub fn local_epoch_day(self) -> i64 {
        (self.unix_seconds + i64::from(self.utc_offset_minutes) * 60).div_euclid(86_400)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LoginSession {
    pub identity: UserIdentity,
    pub nonce: i64,
    pub capability: String,
    pub clock: DeviceClock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrencyKind {
    Ch1Like,
    Ch2Like,
    Ch2Note,
    Candy,
    Chocolate,
    Fans,
    Gingerbread,
    ChocoMilk,
}

impl CurrencyKind {
    pub const ALL: [Self; 8] = [
        Self::Ch1Like,
        Self::Ch2Like,
        Self::Ch2Note,
        Self::Candy,
        Self::Chocolate,
        Self::Fans,
        Self::Gingerbread,
        Self::ChocoMilk,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Ch1Like => "ch1_like",
            Self::Ch2Like => "ch2_like",
            Self::Ch2Note => "ch2_note",
            Self::Candy => "candy",
            Self::Chocolate => "chocolate",
            Self::Fans => "fans",
            Self::Gingerbread => "gingerbread",
            Self::ChocoMilk => "choco_milk",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Character,
    Costume,
    Guitar,
    Music,
    Skill,
    Unit,
    Follower,
    Prop,
    Buff,
    Gift,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reward {
    Currency {
        currency: CurrencyKind,
        amount: f64,
    },
    Content {
        content: ContentKind,
        area: i32,
        content_id: i64,
    },
}

/// Reward tuple used by the stock client's shared reward merger.  Keeping the
/// original numeric type/id here lets master-data driven rewards round-trip
/// without inventing lossy per-RPC representations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RewardGrant {
    pub reward_type: i16,
    pub reward_id: i32,
    pub amount: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MailSnapshot {
    pub post_id: i64,
    pub subject: String,
    pub body: String,
    pub created_at: i64,
    pub read: bool,
    pub claimed: bool,
    pub reward: RewardGrant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MailboxSnapshot {
    pub cursor: i64,
    pub mail: Vec<MailSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PurchaseOutcome {
    pub chocolate: f64,
    pub candy: f64,
    pub rewards: Vec<RewardGrant>,
    pub ad_level: Option<AdLevelSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentPurchaseOutcome {
    pub chocolate: f64,
    pub candy: f64,
    pub kind: ContentKind,
    pub content_id: i64,
    pub level: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PackagePurchaseOutcome {
    pub chocolate: f64,
    pub candy: f64,
    pub music_ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PassAnchorSnapshot {
    pub anchor_day: i64,
    pub anchor_season: i64,
    pub utc_offset_minutes: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PassProgressSnapshot {
    pub season: i64,
    pub points: i64,
    pub step: i32,
    pub version: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PassClaimSnapshot {
    pub season: i64,
    pub version: i32,
    pub lane: i16,
    pub step: i32,
    pub claimed_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PassSnapshot {
    pub anchor: PassAnchorSnapshot,
    pub progress: Vec<PassProgressSnapshot>,
    pub claims: Vec<PassClaimSnapshot>,
    pub premium_seasons: Vec<i64>,
    pub tickets: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PassFillOutcome {
    pub season: i64,
    pub points: i64,
    pub version: i32,
    pub chocolate: f64,
    pub candy: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PassClaimOutcome {
    pub season: i64,
    pub lane: i16,
    pub step: i32,
    pub version: i32,
    pub claimed_at: i64,
    pub rewards: Vec<RewardGrant>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ch3EnergySnapshot {
    pub amount: i32,
    pub cap: i32,
    pub calculated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ch3StageSnapshot {
    pub stage_id: i64,
    pub chapter: i32,
    pub stage_index: i32,
    pub completed: bool,
    pub best_star: i32,
    pub best_score: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ch3ChapterClaimSnapshot {
    pub chapter: i32,
    pub reward_num: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FollowerProfileSnapshot {
    pub profile_id: i64,
    pub level: i32,
    pub experience: i64,
    pub add_candy: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ch3Snapshot {
    pub energy: Ch3EnergySnapshot,
    pub stages: Vec<Ch3StageSnapshot>,
    pub chapter_claims: Vec<Ch3ChapterClaimSnapshot>,
    pub profiles: Vec<FollowerProfileSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ch3Settlement {
    pub stage: Ch3StageSnapshot,
    pub energy: Ch3EnergySnapshot,
    pub profile: FollowerProfileSnapshot,
    pub rewards: Vec<RewardGrant>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ch3StreamSettlement {
    pub stage_id: i64,
    pub energy: Ch3EnergySnapshot,
    pub profile: FollowerProfileSnapshot,
    pub reward_rolls: Vec<(i32, Vec<RewardGrant>)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ch3StageDefinition {
    pub stage_id: i64,
    pub chapter: i32,
    pub stage_index: i32,
    pub story: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MusicReviewSnapshot {
    pub music_id: i32,
    pub difficulty: i16,
    pub point: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookmarkSnapshot {
    pub music_id: i32,
    pub flag: i16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MusicScoreSnapshot {
    pub music_id: i32,
    pub difficulty: i16,
    pub score: i32,
    pub grade: i16,
    pub play_count: i32,
    pub combo_grade: i16,
    pub gp: i32,
    pub multiple: f64,
    pub achievement: i16,
    pub play_type: i16,
    pub played_at: String,
    pub mission_clear: i16,
    pub is_credit: i16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MusicStateSnapshot {
    pub music_id: i64,
    pub encore_appears: bool,
    pub encore_ready_at: i64,
    pub encore_follower_id: Option<i64>,
    pub ch3_active_until: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MusicEncoreRewardOutcome {
    pub rewards: Vec<(i32, i32)>,
    pub profiles: Vec<FollowerProfileSnapshot>,
    pub rejected_music_ids: Vec<i32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FollowerGiftOutcome {
    pub gift_id: i64,
    pub remaining: i64,
    pub profile: FollowerProfileSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FollowerProfileRewardOutcome {
    pub profile_id: i64,
    pub level: i32,
    pub rewards: Vec<RewardGrant>,
    pub profile: FollowerProfileSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FollowerQuestOutcome {
    pub quest: FollowerQuestSnapshot,
    pub complete_id: i64,
    pub next: bool,
    pub rewards: Vec<RewardGrant>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdRewardOutcome {
    pub ad_id: i64,
    pub day_count: i32,
    pub total_count: i32,
    pub last_view_time: i64,
    pub rewards: Vec<RewardGrant>,
    pub profile: Option<FollowerProfileSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectRewardSnapshot {
    pub group_id: i32,
    pub reward_group_id: i32,
    pub alternate_reward_group_id: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    pub identity: UserIdentity,
    pub nickname: String,
    pub fan_level: i32,
    pub avatar_id: Option<i64>,
    pub title_id: Option<i64>,
    pub last_save_time: i64,
    pub samseck_step: i32,
    pub device_uuid: String,
    pub currencies: Vec<(CurrencyKind, f64)>,
    pub areas: Vec<AreaSnapshot>,
    pub owned_content: Vec<OwnedContentSnapshot>,
    pub content_levels: Vec<ContentLevelSnapshot>,
    pub tutorials: Vec<i64>,
    pub pass: PassSnapshot,
    pub skill_activations: Vec<SkillActivationSnapshot>,
    pub buff_timers: Vec<BuffTimerSnapshot>,
    pub gifts: Vec<FollowerGiftSnapshot>,
    pub affection_claims: Vec<FollowerAffectionClaimSnapshot>,
    pub ad_levels: Vec<AdLevelSnapshot>,
    pub ad_state: Vec<AdStateSnapshot>,
    pub messengers: Vec<MessengerSnapshot>,
    pub follower_quests: Vec<FollowerQuestSnapshot>,
    pub achievements: Vec<ProgressSnapshot>,
    pub daily_missions: Vec<ProgressSnapshot>,
    pub ch3: Ch3Snapshot,
    pub music_reviews: Vec<MusicReviewSnapshot>,
    pub bookmarks: Vec<BookmarkSnapshot>,
    pub music_scores: Vec<MusicScoreSnapshot>,
    pub music_states: Vec<MusicStateSnapshot>,
    pub selected_rewards: Vec<SelectRewardSnapshot>,
}

impl PlayerSnapshot {
    pub fn currency(&self, kind: CurrencyKind) -> f64 {
        self.currencies
            .iter()
            .find_map(|(candidate, amount)| (*candidate == kind).then_some(*amount))
            .unwrap_or(0.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AreaSnapshot {
    pub area: i32,
    pub area_level: i32,
    pub fans: f64,
    pub candy: f64,
    pub current_costume: Option<i64>,
    pub current_guitar: Option<i64>,
    pub current_music: Option<i64>,
    pub gp: f64,
    pub unlocked: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OwnedContentSnapshot {
    pub kind: ContentKind,
    pub area: i32,
    pub content_id: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentLevelSnapshot {
    pub kind: ContentKind,
    pub content_id: i64,
    pub level: i64,
    pub reward_level: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillActivationSnapshot {
    pub skill_id: i64,
    pub active: bool,
    pub active_from: i64,
    pub active_until: i64,
    pub reset_ready_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdLevelSnapshot {
    pub ad_level_id: i64,
    pub level: i32,
    pub experience: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdLevelProgression {
    pub group_id: i64,
    pub thresholds: Vec<(i32, i32)>,
}

/// Optional follower-profile experience attached to an advertisement reward.
/// Keeping this named avoids leaking the legacy wire tuple into persistence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdProfileGrant {
    pub profile_id: i64,
    pub experience: i64,
    pub level_thresholds: Vec<(i32, i64, i32)>,
}

/// Authoritative request to claim one achievement or daily-mission tier.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameRewardClaimCommand {
    pub kind: String,
    pub id: i64,
    pub level: i32,
    pub quantity: f64,
    pub threshold: f64,
    pub local_day: i64,
    pub reward: Option<RewardGrant>,
}

/// Atomic purchase whose result consists of one or more reward grants.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RewardPurchaseCommand {
    pub transaction_id: String,
    pub purchase_kind: String,
    pub target_id: i64,
    pub cost: Option<(CurrencyKind, f64)>,
    pub rewards: Vec<RewardGrant>,
    pub ad_progression: Option<AdLevelProgression>,
}

/// Atomic purchase or level-up of a single content record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentPurchaseCommand {
    pub kind: ContentKind,
    pub area: i32,
    pub content_id: i64,
    pub currency: CurrencyKind,
    pub price: f64,
    /// Client float32 increment, evaluated against the transaction's current level.
    pub price_increment: f64,
    pub max_level: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdStateSnapshot {
    pub ad_id: i64,
    pub day_count: i32,
    pub total_count: i32,
    pub last_view_time: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessengerSnapshot {
    pub room_id: i64,
    pub last_confirm_index: i64,
    pub unlock_group_list: String,
    pub update_time_ticks: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FollowerQuestSnapshot {
    pub current_id: i64,
    /// Last durably completed stage in this USN's guide chain, not the
    /// currently displayed (possibly only partially claimed) stage.
    #[serde(default)]
    pub complete_id: i64,
    pub completed: bool,
    pub infinite: bool,
    pub condition_values: [f64; 3],
    pub claimed: [bool; 3],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuffTimerSnapshot {
    pub buff_id: i64,
    pub active_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FollowerGiftSnapshot {
    pub gift_id: i64,
    pub quantity: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FollowerAffectionClaimSnapshot {
    pub profile_id: i64,
    pub level: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProgressSnapshot {
    pub id: i64,
    pub level: i64,
    pub quantity: f64,
    pub quantity_text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CoreSavePatch {
    pub ch1_like: Option<f64>,
    pub fans: Option<i64>,
    pub fan_level: Option<i32>,
    pub current_costume: Option<i64>,
    pub current_music: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AreaSavePatch {
    pub area: i32,
    pub like_amount: Option<f64>,
    pub fans: Option<i64>,
    pub fan_level: Option<i32>,
    pub current_costume: Option<i64>,
    pub current_music: Option<i64>,
    pub current_guitar: Option<i64>,
    pub candy: Option<f64>,
    pub tutorial_list: Option<String>,
    pub gp1: Option<String>,
    pub gp2: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentSavePatch {
    pub kind: ContentKind,
    pub area: i32,
    pub content_id: i64,
    pub level: Option<i64>,
    pub reward_level: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MusicStateSavePatch {
    pub music_id: i64,
    pub encore_appears: bool,
    pub encore_follower_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProgressSavePatch {
    pub id: i64,
    pub quantity: f64,
    pub quantity_text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillActivationSavePatch {
    pub skill_id: i64,
    pub active: bool,
    pub active_from: i64,
    pub active_until: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessengerSavePatch {
    pub room_id: i64,
    pub last_confirm_index: i64,
    pub unlock_group_list: String,
    pub update_time_ticks: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PassProgressSavePatch {
    pub event_type: String,
    pub season: i64,
    pub points: i64,
    pub step: i32,
    pub version: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FollowerQuestSavePatch {
    pub current_id: i64,
    pub condition_values: [f64; 3],
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UserSavePatch {
    pub core: Option<CoreSavePatch>,
    pub areas: Vec<AreaSavePatch>,
    pub content: Vec<ContentSavePatch>,
    pub music_states: Vec<MusicStateSavePatch>,
    pub achievements: Vec<ProgressSavePatch>,
    pub daily_missions: Vec<ProgressSavePatch>,
    pub skill_activations: Vec<SkillActivationSavePatch>,
    pub messengers: Vec<MessengerSavePatch>,
    pub pass_progress: Vec<PassProgressSavePatch>,
    pub follower_quests: Vec<FollowerQuestSavePatch>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttendanceResult {
    pub status: String,
    pub attendance_count: i32,
    pub attendance_date: i32,
    pub max_continuous_count: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    #[error("UTC offset {0} minutes is outside Android's supported range")]
    InvalidUtcOffset(i32),
    #[error("amount must be finite and non-negative")]
    InvalidAmount,
    #[error("identity does not belong to this login session")]
    IdentityMismatch,
    #[error("request sequence is stale")]
    StaleRequest,
    #[error("reward was already claimed")]
    AlreadyClaimed,
    #[error("unique content is already owned")]
    AlreadyOwned,
    #[error("insufficient balance")]
    InsufficientBalance,
    #[error("content is locked")]
    Locked,
    #[error("invalid stage")]
    InvalidStage,
    #[error("insufficient CH3 energy")]
    InsufficientEnergy,
}
