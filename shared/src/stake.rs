use crate::types::AlphaAmount;

/// Tolerance for post-transfer stake verification (in rao).
/// Accounts for potential rounding or micro-fees in the Subtensor pallet.
pub const TRANSFER_TOLERANCE: u64 = 10;

/// Verify an observed stake delta matches the expected transfer amount,
/// allowing up to `TRANSFER_TOLERANCE` rao less than expected (never more).
pub fn stake_delta_verified(delta: AlphaAmount, amount: AlphaAmount) -> bool {
    delta >= amount.saturating_sub(TRANSFER_TOLERANCE) && delta <= amount
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_exact_and_tolerated_stake_deltas() {
        let amount = 100;

        assert!(stake_delta_verified(amount, amount));
        assert!(stake_delta_verified(amount - TRANSFER_TOLERANCE, amount));
        assert!(!stake_delta_verified(
            amount - TRANSFER_TOLERANCE - 1,
            amount
        ));
        assert!(!stake_delta_verified(amount + 1, amount));
    }

    #[test]
    fn tolerance_saturates_for_small_amounts() {
        assert!(stake_delta_verified(0, TRANSFER_TOLERANCE - 1));
        assert!(!stake_delta_verified(
            TRANSFER_TOLERANCE,
            TRANSFER_TOLERANCE - 1
        ));
    }
}
