use crate::errors::Error;
use fixed::types::U64F64;
use ink::prelude::vec::Vec;
use ink::primitives::AccountId;
use ink::storage::Mapping;

// Pause state for emergency control
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
#[derive(Default)]
pub enum PauseState {
    #[default]
    NotPaused, // Normal operation
    TradingPaused, // No trading, but cancellations allowed
    FullyPaused,   // All operations frozen except admin functions
}

// ID types
pub type AlphaListingId = u64;
pub type TaoOfferId = u64;

// Network and blockchain types
pub type NetUid = u16;
pub type BlockNumber = u32;
pub type BlockAge = u32; // Age/duration in blocks

// Balance and amount types
pub type Balance = u64; // Generic balance/amount type
pub type AlphaAmount = u64; // Amount of Alpha tokens in rao
pub type TaoAmount = u64; // Amount of TAO tokens in rao

/// Price offset in basis points (1 bp = 0.01%)
/// -500 = -5% below market, 1000 = +10% above market
/// Range: -10000 to i32::MAX (cannot go below -100%)
pub type PriceOffsetBps = i32;

pub type AlphaListingsMapping = Mapping<(NetUid, AccountId, AlphaListingId), AlphaListing>;
pub type UserListingsMapping = Mapping<(AccountId, NetUid), Vec<AlphaListingId>>;
pub type TaoOffersMapping = Mapping<(NetUid, AccountId, TaoOfferId), TaoOffer>;
pub type UserOffersMapping = Mapping<(AccountId, NetUid), Vec<TaoOfferId>>;
pub type ReservedAlphaMapping = Mapping<NetUid, AlphaAmount>;
pub type FrozenSubnetsMapping = Mapping<NetUid, bool>;

/// Fixed-point decimal representation with 64 bits integer, 64 bits fraction
/// Used for prices (TAO per Alpha ratios) and rates (fee percentages)
/// Uses U64F64 to handle large rao values (1 TAO = 10^9 rao) and precise percentages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
#[derive(Default)]
pub struct FixedDecimal {
    value: u128, // Store U64F64 as raw bits for SCALE codec compatibility
}

impl FixedDecimal {
    /// Multiply this decimal by an integer value
    /// Returns the result as a u64
    pub fn mul(&self, value: u64) -> Result<u64, Error> {
        let self_fixed = U64F64::from_bits(self.value);
        let operand = U64F64::from_num(value);
        let result = self_fixed.checked_mul(operand).ok_or(Error::Overflow)?;

        if result > U64F64::from_num(u64::MAX) {
            return Err(Error::Overflow);
        }

        Ok(result.to_num::<u64>())
    }

    /// Divide an integer value by this decimal
    /// Returns the result as a u64
    pub fn div_by(&self, value: u64) -> Result<u64, Error> {
        if self.is_zero() {
            return Err(Error::DivisionByZero);
        }

        let numerator = U64F64::from_num(value);
        let divisor = U64F64::from_bits(self.value);
        let result = numerator.checked_div(divisor).ok_or(Error::Overflow)?;

        if result > U64F64::from_num(u64::MAX) {
            return Err(Error::Overflow);
        }

        Ok(result.to_num::<u64>())
    }

    /// Create a FixedDecimal from raw bits
    pub fn from_bits(bits: u128) -> Self {
        FixedDecimal { value: bits }
    }

    /// Get the raw bits representation
    pub fn to_bits(&self) -> u128 {
        self.value
    }

    /// Check if this decimal is zero
    pub fn is_zero(&self) -> bool {
        self.value == 0
    }

    /// Use this decimal as the divisor of the given value (value / self)
    /// Used for calculating how much Alpha is needed for a given TAO amount
    /// Example: If price is 2 TAO per Alpha, price.as_divisor_of(10 TAO) returns 5 Alpha
    /// Returns the result as a u64
    pub fn as_divisor_of(&self, value: u64) -> Result<u64, Error> {
        if self.is_zero() {
            return Err(Error::DivisionByZero);
        }

        // Create FixedDecimal from the u64 value
        let value_fixed = U64F64::from_num(value);
        let divisor = U64F64::from_bits(self.value);

        // Perform the division: value / self
        let result = value_fixed.checked_div(divisor).ok_or(Error::Overflow)?;

        if result > U64F64::from_num(u64::MAX) {
            return Err(Error::Overflow);
        }

        Ok(result.to_num::<u64>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixed::types::U64F64;

    #[test]
    fn fixed_decimal_mul_works() {
        // Create a decimal representing 2.5 using raw bits
        // 2.5 = 2.5 * 2^64 = 46116860184273879040 in U64F64
        let decimal = FixedDecimal::from_bits(U64F64::from_num(2.5).to_bits());

        // 2.5 * 100 = 250
        assert_eq!(decimal.mul(100).unwrap(), 250);

        // 2.5 * 0 = 0
        assert_eq!(decimal.mul(0).unwrap(), 0);

        // Test with 1 TAO = 10^9 rao
        let tao_in_rao = 1_000_000_000u64;
        assert_eq!(decimal.mul(tao_in_rao).unwrap(), 2_500_000_000);
    }

    #[test]
    fn fixed_decimal_div_by_works() {
        // Create a decimal representing 2.0 using raw bits
        let decimal = FixedDecimal::from_bits(U64F64::from_num(2u64).to_bits());

        // 100 / 2.0 = 50
        assert_eq!(decimal.div_by(100).unwrap(), 50);

        // 0 / 2.0 = 0
        assert_eq!(decimal.div_by(0).unwrap(), 0);

        // Test with TAO amounts
        let tao_amount = 10_000_000_000u64; // 10 TAO
        assert_eq!(decimal.div_by(tao_amount).unwrap(), 5_000_000_000);
    }

    #[test]
    fn fixed_decimal_is_zero_works() {
        let zero = FixedDecimal::from_bits(0);
        assert!(zero.is_zero());

        // Create a non-zero value using raw bits
        let non_zero = FixedDecimal::from_bits(U64F64::from_num(0.01).to_bits());
        assert!(!non_zero.is_zero());
    }

    #[test]
    fn fixed_decimal_edge_cases() {
        // Test with very small decimal
        let small = FixedDecimal::from_bits(U64F64::from_num(0.000000001).to_bits());
        let result = small.div_by(1);
        assert!(result.is_ok());

        // Test multiplication with large numbers
        let large_multiplier = FixedDecimal::from_bits(U64F64::from_num(1_000_000u64).to_bits());
        let result = large_multiplier.mul(1_000_000);
        assert_eq!(result.unwrap(), 1_000_000_000_000);
    }

    #[test]
    fn fixed_decimal_as_divisor_of_works() {
        // Create a decimal representing 2.0 (price of 2 TAO per Alpha)
        let price = FixedDecimal::from_bits(U64F64::from_num(2u64).to_bits());

        // If we have 100 TAO, we should get 50 Alpha
        assert_eq!(price.as_divisor_of(100).unwrap(), 50);

        // If we have 0 TAO, we should get 0 Alpha
        assert_eq!(price.as_divisor_of(0).unwrap(), 0);

        // Test with TAO amounts (1 TAO = 10^9 rao)
        let tao_amount = 10_000_000_000u64; // 10 TAO
        assert_eq!(price.as_divisor_of(tao_amount).unwrap(), 5_000_000_000); // 5 Alpha

        // Test with fractional price (1.5 TAO per Alpha)
        let price_fractional = FixedDecimal::from_bits(U64F64::from_num(1.5).to_bits());
        assert_eq!(price_fractional.as_divisor_of(150).unwrap(), 100);
        assert_eq!(
            price_fractional.as_divisor_of(15_000_000_000).unwrap(),
            10_000_000_000
        );
    }

    #[test]
    fn fixed_decimal_as_divisor_of_division_by_zero() {
        let zero = FixedDecimal::from_bits(0);
        let result = zero.as_divisor_of(100);
        assert_eq!(result, Err(Error::DivisionByZero));
    }
}

/// Alpha listing structure
#[derive(Debug, Clone, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
pub struct AlphaListing {
    pub id: AlphaListingId,
    pub netuid: NetUid,
    pub seller: AccountId,
    pub amount: AlphaAmount,              // Alpha amount in rao
    pub price_offset_bps: PriceOffsetBps, // Price offset from market in basis points
    pub fee_rate: FixedDecimal, // Fee rate at listing time (as decimal, e.g., 0.005 = 0.5%)
    pub created_at: BlockNumber,
}

/// TAO offer structure
#[derive(Debug, Clone, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
pub struct TaoOffer {
    pub id: TaoOfferId,
    pub netuid: NetUid,
    pub buyer: AccountId,
    pub amount: TaoAmount,                // TAO amount offered in rao
    pub price_offset_bps: PriceOffsetBps, // Price offset from market in basis points
    pub fee_rate: FixedDecimal,           // Fee rate at offer time (as decimal, e.g., 0.005 = 0.5%)
    pub created_at: BlockNumber,
}
