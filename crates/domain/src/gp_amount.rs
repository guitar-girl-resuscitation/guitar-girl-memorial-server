//! Exact, resource-bounded nonnegative game points. Never route balances through
//! a floating-point multiplier. Limits reject a transaction, never clamp money.
use num_bigint::BigUint;
use num_traits::{FromPrimitive, ToPrimitive};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use crate::DomainError;

pub const MAX_GP_DIGITS: usize = 4096;

#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub struct GpAmount(BigUint);

impl GpAmount {
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        if value.is_empty() || value.len() > MAX_GP_DIGITS || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(DomainError::InvalidAmount);
        }
        BigUint::parse_bytes(value.as_bytes(), 10).map(Self).ok_or(DomainError::InvalidAmount)
    }

    /// Preserve the integer represented by an old finite REAL, not its short
    /// decimal display. Fractional game points are truncated like BigInteger.
    pub fn from_legacy(value: f64) -> Result<Self, DomainError> {
        if !value.is_finite() || value < 0.0 { return Err(DomainError::InvalidAmount); }
        BigUint::from_f64(value).map(Self).ok_or(DomainError::InvalidAmount)
    }

    pub fn checked_add(&self, other: &Self) -> Result<Self, DomainError> {
        let sum = Self(&self.0 + &other.0);
        if sum.to_string().len() > MAX_GP_DIGITS { return Err(DomainError::AmountTooLarge); }
        Ok(sum)
    }

    pub fn checked_sub(&self, other: &Self) -> Result<Self, DomainError> {
        if self < other { return Err(DomainError::InsufficientBalance); }
        Ok(Self(&self.0 - &other.0))
    }

    pub fn finite_double(&self) -> Option<f64> {
        self.0.to_f64().filter(|v| v.is_finite())
    }

    /// v8's retired double fields are not authoritative. Zero is the stock
    /// unused-field value when only S_Gp1/S_Gp2 can represent the live balance.
    pub fn legacy_wire_double(&self) -> f64 { self.finite_double().unwrap_or(0.0) }
}

impl std::fmt::Display for GpAmount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) }
}
impl Serialize for GpAmount {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> { s.serialize_str(&self.to_string()) }
}
impl<'de> Deserialize<'de> for GpAmount {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_beyond_double_and_reject_before_unbounded_parsing() {
        let n = GpAmount::parse(&format!("1{}", "0".repeat(318))).unwrap();
        let one = GpAmount::parse("1").unwrap();
        assert_eq!(n.checked_add(&one).unwrap().checked_sub(&one).unwrap(), n);
        assert!(n.finite_double().is_none());
        assert_eq!(GpAmount::parse("00042").unwrap().to_string(), "42");
        for s in ["inf", "NaN", "-1", "1e318", "1.5", ""] { assert!(GpAmount::parse(s).is_err()); }
        assert!(GpAmount::parse(&"1".repeat(MAX_GP_DIGITS + 1)).is_err());
        assert!(GpAmount::parse(&"9".repeat(MAX_GP_DIGITS)).unwrap().checked_add(&one).is_err());
        assert!(GpAmount::from_legacy(f64::INFINITY).is_err());
        assert_eq!(GpAmount::from_legacy(3.9).unwrap().to_string(), "3");
    }
}
