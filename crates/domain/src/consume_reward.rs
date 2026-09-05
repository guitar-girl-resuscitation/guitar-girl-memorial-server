//! 8.0.0's shared Consume contract, used by shop, pass and mailbox rewards.
//! IDs are semantic: changing 7 to 6 changes a multiplier into a literal count.
use crate::CurrencyKind;
use crate::DomainError;
use std::collections::BTreeMap;

/// Runtime-only master projection, not a player save and not original assets.
#[derive(Clone, Debug, Default)]
pub struct RewardRules {
    pub costume_fan_bonuses: BTreeMap<i64, (i32, f32)>,
}

/// Client 0x1E3FC80 accumulates owned costume bonuses with float32 additions;
/// 0x1D926C0/0x1D93730 promote to double before applying the reward quantity.
pub fn fan_reward_count(
    bonuses: impl IntoIterator<Item = f32>,
    multiplier: f64,
) -> Result<i32, DomainError> {
    if !multiplier.is_finite() || multiplier < 0.0 {
        return Err(DomainError::InvalidAmount);
    }
    let mut basis = 1.0_f32;
    for bonus in bonuses {
        if !bonus.is_finite() || bonus < 0.0 {
            return Err(DomainError::InvalidAmount);
        }
        basis += bonus;
    }
    let result = f64::from(basis) * multiplier;
    // Do not reproduce the client's integer-overflow sentinel as a negative
    // balance. Reject the whole transaction instead.
    if !result.is_finite() || !(0.0..2_147_483_648.0).contains(&result) {
        return Err(DomainError::InvalidAmount);
    }
    Ok(result.trunc() as i32)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuantityUnit {
    Count,
    ProductionMultiplier,
    FanGainMultiplier,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConsumeReward {
    pub id: i32,
    pub currency: Option<CurrencyKind>,
    pub area: i32,
    pub unit: QuantityUnit,
}

impl ConsumeReward {
    pub const fn from_id(id: i32) -> Option<Self> {
        use CurrencyKind::*;
        use QuantityUnit::*;
        let (currency, area, unit) = match id {
            1 => (Some(Chocolate), 0, Count),
            2 => (Some(Candy), 0, Count),
            3 => (Some(Ch1Like), 1, Count),
            4 => (Some(Ch1Like), 1, ProductionMultiplier),
            5 => (Some(Ch2Note), 2, Count),
            6 => (Some(Ch2Like), 2, Count),
            7 => (Some(Ch2Like), 2, ProductionMultiplier),
            // Despite Consume.i_Type=0, the reward applicator multiplies these
            // by the corresponding channel's current fan-gain basis.
            8 => (Some(Fans), 1, FanGainMultiplier),
            9 => (Some(Fans), 2, FanGainMultiplier),
            10 => (Some(Ch2Note), 2, ProductionMultiplier),
            // Cookie is CH3 energy, not a normal currencies-table balance.
            11 => (None, 3, Count),
            _ => return None,
        };
        Some(Self {
            id,
            currency,
            area,
            unit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fans_keep_owned_outfit_bonuses_and_client_float_order() {
        assert_eq!(fan_reward_count([], 1000.0).unwrap(), 1000);
        assert_eq!(fan_reward_count([0.3], 1000.0).unwrap(), 1299);
        assert_eq!(fan_reward_count([0.3, 0.3], 1000.0).unwrap(), 1599);
        assert_eq!(fan_reward_count([0.5], 3.5).unwrap(), 5);
        assert!(fan_reward_count([f32::NAN], 1.0).is_err());
        assert!(fan_reward_count([], f64::INFINITY).is_err());
        assert!(fan_reward_count([], 2_147_483_648.0).is_err());
    }

    #[test]
    fn literal_and_multiplier_ids_are_not_interchangeable() {
        for (literal, multiplier) in [(3, 4), (6, 7), (5, 10)] {
            let literal = ConsumeReward::from_id(literal).unwrap();
            let multiplier = ConsumeReward::from_id(multiplier).unwrap();
            assert_eq!(literal.currency, multiplier.currency);
            assert_eq!(literal.area, multiplier.area);
            assert_eq!(literal.unit, QuantityUnit::Count);
            assert_eq!(multiplier.unit, QuantityUnit::ProductionMultiplier);
        }
        for (id, area) in [(8, 1), (9, 2)] {
            let reward = ConsumeReward::from_id(id).unwrap();
            assert_eq!(reward.area, area);
            assert_eq!(reward.unit, QuantityUnit::FanGainMultiplier);
        }
        assert!(ConsumeReward::from_id(0).is_none());
        assert!(ConsumeReward::from_id(12).is_none());
    }
}
