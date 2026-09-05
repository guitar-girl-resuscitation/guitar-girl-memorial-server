use ggfm_domain::{ContentKind, DeviceClock};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const POLICY_VERSION: u32 = 1;
pub const PASS_SEASONS: i64 = 13;
pub const SKILL_RESET_SECONDS: i64 = 5 * 60;
pub const ENCORE_BASE_SECONDS: f64 = 60.0 * 60.0;

pub fn costume_fan_bonus(area: i32, is_default: bool, official: f64, category_max: f64) -> f64 {
    if is_default { official } else if area == 1 { 0.30 } else { category_max }
}
pub const MAX_SINGLE_STAGE_ENERGY_COST: i64 = 5;
pub const PASS_POINT_MULTIPLIER: i64 = 10;

/// Settings required by the tested 8.0.0 client during bootstrap.  Values that
/// intentionally differ from the retired service are memorial policy, not
/// transport guesses (notably the five-run CH3 energy cap).
pub const DEFAULT_SETTINGS: [(&str, &str); 10] = [
    ("shop_call_wait", "0"),
    ("ad_cancel_delay", "0"),
    ("force_menu_restric", "N"),
    ("max_bundle_texutre_loading", "10"),
    ("weather_snow", "N"),
    ("check_time_cheat", "N"),
    ("ap_max", "200"),
    ("ap_time", "1"),
    ("ap_use", "5"),
    ("character_exp", "450"),
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorialPolicy {
    pub version: u32,
    pub follower_gift_icons: BTreeMap<i64, String>,
    pub legacy_items: Vec<LegacyItemPolicy>,
    pub progression_multiplier: f64,
    pub scaled_achievement_ids: Vec<i64>,
    pub preserve_skill_unlock_requirements: bool,
    pub cosmetic_price: i64,
    pub skill_cooldown_multiplier: f64,
    pub skill_reset_seconds: i64,
    pub encore_base_seconds: f64,
    pub max_single_stage_energy_cost: i64,
    pub ch3_energy_cap: i64,
    pub ch3_energy_restore_seconds: i64,
    pub ch3_reward_multiplier: i64,
    pub ch3_profile_exp_per_clear: i64,
    pub pass_point_multiplier: i64,
    pub upgrade_costs: UpgradeCostPolicy,
    pub memorial_shop: MemorialShopPolicy,
}

/// Audited 8.0.0 discontinued catalogue, preserved from the tested Rust baseline.
/// IDs are not assets; names/resources must come from the user's own master.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegacyItemPolicy {
    pub kind: ContentKind,
    pub area: i32,
    pub id: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeCostPolicy {
    pub single_digit_currencies: Vec<String>,
    pub formula_cap_min: i64,
    pub formula_cap_max: i64,
}

impl MemorialPolicy {
    pub fn embedded() -> &'static Self {
        static POLICY: std::sync::OnceLock<MemorialPolicy> = std::sync::OnceLock::new();
        POLICY.get_or_init(|| {
            serde_json::from_str(POLICY_MANIFEST).expect("embedded memorial policy must be valid")
        })
    }

    pub fn uses_single_digit_cost(&self, currency: &str) -> bool {
        self.upgrade_costs
            .single_digit_currencies
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(currency))
    }

    pub fn achievement_requirement(&self, id: i64, official: f64) -> f64 {
        if self.scaled_achievement_ids.contains(&id) { scaled_requirement(official, false) }
        else { official }
    }
}

/// Cosmetic descriptions are static localized copy in the original tables.
/// Replace numeric percentages only; preserve NGUI tags and unrelated numbers.
pub fn rewrite_percentage(source: &str, percent: i32) -> String {
    let mut result = String::with_capacity(source.len());
    let mut chars = source.char_indices().peekable();
    while let Some((start, character)) = chars.next() {
        if !character.is_ascii_digit() {
            result.push(character);
            continue;
        }
        let mut end = start + character.len_utf8();
        while let Some(&(index, next)) = chars.peek() {
            if !next.is_ascii_digit() && next != '.' {
                break;
            }
            end = index + next.len_utf8();
            chars.next();
        }
        if source[end..].trim_start().starts_with(['%', '％']) {
            result.push_str(&percent.to_string());
        } else {
            result.push_str(&source[start..end]);
        }
    }
    result
}

impl Default for MemorialPolicy {
    fn default() -> Self {
        Self::embedded().clone()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorialShopPolicy {
    pub shop_category: i64,
    pub product_type: i64,
    pub sell_type: i64,
    pub sell_value: i64,
    pub area_list: String,
    pub items: Vec<MemorialShopItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorialShopItem {
    pub id: i64,
    pub product_id_ios: String,
    pub product_id_android: String,
    pub resource_name: String,
    pub reward_group: i64,
    pub sort_index: i16,
    pub title: BTreeMap<String, String>,
    pub description: BTreeMap<String, String>,
    pub rewards: Vec<MemorialShopReward>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemorialShopReward {
    #[serde(rename = "type")]
    pub reward_type: i16,
    pub id: i32,
    pub quantity: f64,
}

pub const POLICY_MANIFEST: &str = include_str!("../../../policy/memorial-policy.json");
include!(concat!(env!("OUT_DIR"), "/policy_fingerprint.rs"));

pub fn scaled_requirement(official: f64, preserve_first_fan_level: bool) -> f64 {
    if preserve_first_fan_level || official <= 0.0 {
        official
    } else {
        (official * 0.1).ceil().max(1.0)
    }
}

pub fn scaled_fan_requirement(official: f64, first_level: f64) -> f64 {
    if official <= first_level {
        official
    } else {
        first_level + ((official - first_level) * 0.1).ceil()
    }
}

/// Exact ceil(value / 10) for the integer decimal strings used by achievement
/// conditions. This avoids converting enormous like-count thresholds through
/// f64 and silently changing their digits.
pub fn scaled_requirement_text(official: &str) -> Option<String> {
    let value = official.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let canonical = value.trim_start_matches('0');
    if canonical.is_empty() {
        return Some("0".into());
    }
    let (quotient, remainder) = canonical.split_at(canonical.len() - 1);
    let mut scaled = if quotient.is_empty() {
        "0".to_owned()
    } else {
        quotient.to_owned()
    };
    if remainder != "0" {
        let mut bytes = scaled.into_bytes();
        let mut carry = 1_u8;
        for digit in bytes.iter_mut().rev() {
            let value = (*digit - b'0') + carry;
            *digit = b'0' + value % 10;
            carry = value / 10;
            if carry == 0 {
                break;
            }
        }
        if carry != 0 {
            bytes.insert(0, b'1');
        }
        scaled = String::from_utf8(bytes).expect("decimal digits are UTF-8");
    }
    Some(scaled)
}

/// Maps sorted distinct positive prices monotonically onto the inclusive 1..=10 range.
pub fn memorial_upgrade_cost(rank: usize, distinct_price_count: usize) -> i64 {
    match distinct_price_count {
        0 => 0,
        1 => 1,
        count => 1 + (9 * rank.min(count - 1) / (count - 1)) as i64,
    }
}

/// Relative expense is compared only within one system, area and currency.
/// Free entries must be excluded from the range. A lone price uses the lower cap.
pub fn premium_upgrade_cap(final_price: f64, minimum: f64, maximum: f64) -> i64 {
    let policy = &MemorialPolicy::embedded().upgrade_costs;
    if final_price <= 0.0 { return 0; }
    if maximum <= minimum { return policy.formula_cap_min; }
    let ratio = ((final_price - minimum) / (maximum - minimum)).clamp(0.0, 1.0);
    policy.formula_cap_min + (ratio * (policy.formula_cap_max - policy.formula_cap_min) as f64).round() as i64
}

pub fn premium_upgrade_increment(max_level: i64, official_base: f64, first_target_level: i64, cap: i64) -> f64 {
    if official_base > 0.0 && max_level > first_target_level {
        (cap - 1) as f64 / (max_level - first_target_level) as f64
    } else { 0.0 }
}

pub fn skill_cooldown(official_seconds: f64) -> f64 {
    (official_seconds * 0.1).max(0.0)
}

pub fn encore_cooldown(official_at_level: f64, official_base: f64) -> f64 {
    if official_base <= 0.0 {
        ENCORE_BASE_SECONDS
    } else {
        ENCORE_BASE_SECONDS * (official_at_level / official_base).clamp(0.0, 1.0)
    }
}

pub fn pass_season(clock: DeviceClock, anchor_day: i64, anchor_season: i64) -> i64 {
    let elapsed = clock.local_epoch_day().saturating_sub(anchor_day);
    (anchor_season - 1 + elapsed).rem_euclid(PASS_SEASONS) + 1
}

pub fn ch3_energy_cap(max_single_run_cost: i64) -> i64 {
    max_single_run_cost.saturating_mul(5)
}

pub fn memorial_ch3_energy_cap() -> i64 {
    let policy = MemorialPolicy::embedded();
    policy.ch3_energy_cap.max(ch3_energy_cap(policy.max_single_stage_energy_cost))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premium_formula_caps_keep_relative_original_expense() {
        assert_eq!(premium_upgrade_cap(0.0, 280.0, 640.0), 0);
        assert_eq!(premium_upgrade_cap(280.0, 280.0, 640.0), 10);
        assert_eq!(premium_upgrade_cap(460.0, 280.0, 640.0), 15);
        assert_eq!(premium_upgrade_cap(640.0, 280.0, 640.0), 20);
        assert_eq!(premium_upgrade_cap(200.0, 200.0, 200.0), 10);
        for cap in 10..=20 {
            for (maximum, offset) in [(20, 2), (20, 1), (6, 1)] {
                let increment = premium_upgrade_increment(maximum, 100.0, offset, cap) as f32;
                let prices: Vec<_> = (offset..=maximum).map(|target|
                    (1.0_f32 + (target-offset) as f32 * increment).trunc() as i64).collect();
                assert_eq!(prices[0], 1);
                assert_eq!(*prices.last().unwrap(), cap);
                assert!(prices.windows(2).all(|p| p[0] <= p[1]));
            }
        }
    }

    #[test]
    fn only_fan_count_and_total_likes_achievements_are_scaled() {
        let policy = MemorialPolicy::embedded();
        assert_eq!(policy.scaled_achievement_ids, [3, 4, 203, 204]);
        for id in (1..=10).chain(201..=207).chain([999]) {
            let expected = if [3, 4, 203, 204].contains(&id) { 31.0 } else { 301.0 };
            assert_eq!(policy.achievement_requirement(id, 301.0), expected, "id={id}");
            assert_eq!(policy.achievement_requirement(id, 0.0), 0.0);
        }
    }

    #[test]
    fn single_digits_apply_only_to_premium_upgrade_currencies() {
        let policy = MemorialPolicy::default();
        for currency in ["CP", "cp", "Candy"] {
            assert!(policy.uses_single_digit_cost(currency));
        }
        for currency in ["GP", "GP2", "", "NotForSale"] {
            assert!(!policy.uses_single_digit_cost(currency));
        }
        assert_eq!(
            rewrite_percentage("[000000]Lv.20: +5%, 10 % / 2.5％", 30),
            "[000000]Lv.20: +30%, 30 % / 30％"
        );
    }

    #[test]
    fn costs_are_monotonic_and_single_digit() {
        let values: Vec<_> = (0..20)
            .map(|rank| memorial_upgrade_cost(rank, 20))
            .collect();
        assert!(values.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(values.first(), Some(&1));
        assert_eq!(values.last(), Some(&10));
    }

    #[test]
    fn pass_rotates_on_device_local_day() {
        let clock = DeviceClock::new(86_400 * 10, 0).unwrap();
        assert_eq!(pass_season(clock, 9, 13), 1);
    }

    #[test]
    fn requirements_keep_zero_and_fan_curve_keeps_the_first_segment() {
        assert_eq!(scaled_requirement(0.0, false), 0.0);
        assert_eq!(scaled_fan_requirement(1_000.0, 1_000.0), 1_000.0);
        assert_eq!(scaled_fan_requirement(10_000.0, 1_000.0), 1_900.0);
    }

    #[test]
    fn huge_decimal_requirements_are_scaled_exactly() {
        assert_eq!(
            scaled_requirement_text("1000000000000000000000000000000000001").as_deref(),
            Some("100000000000000000000000000000000001")
        );
        assert_eq!(scaled_requirement_text("0").as_deref(), Some("0"));
        assert_eq!(scaled_requirement_text("not-a-number"), None);
    }

    #[test]
    fn embedded_manifest_matches_typed_defaults() {
        let parsed: MemorialPolicy = serde_json::from_str(POLICY_MANIFEST).unwrap();
        assert_eq!(parsed.version, POLICY_VERSION);
        assert_eq!(parsed.legacy_items.len(), 29);
        assert!(parsed.legacy_items.iter().any(|item|
            item.kind == ContentKind::Costume && item.area == 1 && item.id == 12));
        assert_eq!(parsed.pass_point_multiplier, PASS_POINT_MULTIPLIER);
        assert_eq!(parsed.memorial_shop.items.len(), 8);
        assert_eq!(parsed.memorial_shop.items[0].id, 50_001);
        assert_eq!(parsed.memorial_shop.items[7].rewards[0].id, 5);
        assert_eq!(
            parsed.max_single_stage_energy_cost,
            MAX_SINGLE_STAGE_ENERGY_COST
        );
        assert_eq!(memorial_ch3_energy_cap(), 200);
        assert_eq!(POLICY_SHA256.len(), 64);
        assert!(POLICY_SHA256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(DEFAULT_SETTINGS.len(), 10);
        assert_eq!(
            DEFAULT_SETTINGS.iter().find(|(key, _)| *key == "ap_max"),
            Some(&("ap_max", "200"))
        );
    }
}
