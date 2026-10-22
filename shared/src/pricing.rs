//! Market-relative pricing for listings: the chain's spot price, moved by the
//! listing's offset, times the amount, plus the fee.

use crate::types::{AlphaAmount, FixedDecimal, PriceOffsetBps, TaoAmount};
use fixed::types::U64F64;

/// Prices from `get_alpha_price` are TAO per Alpha, scaled by 1e9.
pub const PRICE_SCALE: u64 = 1_000_000_000;

/// The smallest remainder a partial fill may leave (2_000_000 rao), so what is
/// left stays sellable. Subtensor's own transfer minimum is measured in TAO at
/// the current price, so a remainder this size can still be refused when Alpha
/// is cheap.
pub const BITTENSOR_MIN_STAKE: AlphaAmount = 2_000_000;

/// Offsets are basis points: -10000 would price Alpha at zero.
const BPS: i64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub enum PricingError {
    /// The offset prices Alpha at or below zero
    InvalidPriceOffset,
    /// Arithmetic overflow
    Overflow,
}

/// What buying `amount` Alpha costs right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub struct Quote {
    /// Executed price, TAO per Alpha scaled by 1e9
    pub price: u64,
    /// TAO paid to the seller
    pub tao: TaoAmount,
    /// Fee paid on top of `tao`
    pub fee: TaoAmount,
    /// What the buyer must send: `tao + fee`
    pub total: TaoAmount,
}

/// `market_price` moved by `offset_bps`, in the same 1e9 scale.
pub fn apply_price_offset(
    market_price: u64,
    offset_bps: PriceOffsetBps,
) -> Result<u64, PricingError> {
    let multiplier = BPS
        .checked_add(i64::from(offset_bps))
        .ok_or(PricingError::Overflow)?;
    if multiplier <= 0 {
        return Err(PricingError::InvalidPriceOffset);
    }

    let multiplier = u128::try_from(multiplier).map_err(|_| PricingError::Overflow)?;
    let price = u128::from(market_price)
        .checked_mul(multiplier)
        .ok_or(PricingError::Overflow)?
        .checked_div(BPS.unsigned_abs().into())
        .ok_or(PricingError::Overflow)?;

    u64::try_from(price).map_err(|_| PricingError::Overflow)
}

/// A 1e9-scaled price as a fixed-point TAO-per-Alpha ratio.
pub fn price_to_fixed_decimal(scaled_price: u64) -> FixedDecimal {
    let price = U64F64::from_num(scaled_price)
        .checked_div(U64F64::from_num(PRICE_SCALE))
        .unwrap_or_default();
    FixedDecimal::from_bits(price.to_bits())
}

/// Price `amount` Alpha at `market_price` moved by `offset_bps`, with `fee_rate`
/// charged on top of what the seller receives.
pub fn quote(
    market_price: u64,
    offset_bps: PriceOffsetBps,
    fee_rate: FixedDecimal,
    amount: AlphaAmount,
) -> Result<Quote, PricingError> {
    let price = apply_price_offset(market_price, offset_bps)?;
    let tao = price_to_fixed_decimal(price)
        .mul(amount)
        .map_err(|_| PricingError::Overflow)?;
    let fee = fee_rate.mul(tao).map_err(|_| PricingError::Overflow)?;
    let total = tao.checked_add(fee).ok_or(PricingError::Overflow)?;

    Ok(Quote {
        price,
        tao,
        fee,
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1/64, exact in binary fixed point, so results are exact too.
    fn one_sixty_fourth() -> FixedDecimal {
        FixedDecimal::from_bits((U64F64::from_num(1) / 64).to_bits())
    }

    #[test]
    fn offsets_move_the_market_price() {
        assert_eq!(apply_price_offset(1_000_000_000, 0), Ok(1_000_000_000));
        assert_eq!(apply_price_offset(1_000_000_000, 500), Ok(1_050_000_000));
        assert_eq!(apply_price_offset(1_000_000_000, -500), Ok(950_000_000));
        assert_eq!(
            apply_price_offset(1_000_000_000, -10_000),
            Err(PricingError::InvalidPriceOffset)
        );
        assert_eq!(
            apply_price_offset(u64::MAX, i32::MAX),
            Err(PricingError::Overflow)
        );
    }

    #[test]
    fn quote_adds_the_fee_on_top() {
        // 2 Alpha at 0.5 TAO each, +25%, with a 1/64 fee.
        let quote = quote(500_000_000, 2_500, one_sixty_fourth(), 2_000_000_000).unwrap();
        assert_eq!(quote.price, 625_000_000);
        assert_eq!(quote.tao, 1_250_000_000);
        assert_eq!(quote.fee, 19_531_250);
        assert_eq!(quote.total, 1_269_531_250);
    }

    #[test]
    fn quote_without_a_fee() {
        let quote = quote(1_000_000_000, 0, FixedDecimal::default(), 3).unwrap();
        assert_eq!(quote.tao, 3);
        assert_eq!(quote.fee, 0);
        assert_eq!(quote.total, 3);
    }
}
