use crate::chain_extension::{SubtensorError, SubtensorExtension};
use crate::runtime::AlphaCurrency;
use crate::types::{AlphaAmount, NetUid};
use crate::BittensorEnvironment;
use ink::primitives::AccountId;
use ink::ChainExtensionInstance;

/// Tolerance for post-transfer stake verification (in rao).
/// Accounts for potential rounding or micro-fees in the Subtensor pallet.
pub const TRANSFER_TOLERANCE: u64 = 10;

/// Verify an observed stake delta matches the expected transfer amount, within
/// `TRANSFER_TOLERANCE` rao either way. Stake is a share of a hotkey's pool, so
/// rounding can go both ways: a recipient that already holds a position on the
/// hotkey can gain a rao more than was sent.
pub fn stake_delta_verified(delta: AlphaAmount, amount: AlphaAmount) -> bool {
    delta.abs_diff(amount) <= TRANSFER_TOLERANCE
}

/// Why a verified stake transfer did not complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferError {
    /// Subtensor rejected the transfer; nothing moved
    Rejected(SubtensorError),
    /// A stake query failed
    QueryFailed,
    /// The transfer completed but moved the wrong amount. Callers must trap, so
    /// the whole call, including the transfer, reverts.
    NotVerified,
}

/// Stake `coldkey` holds on `hotkey` in `netuid`; a missing entry counts as 0.
pub fn stake_of(
    hotkey: AccountId,
    coldkey: AccountId,
    netuid: NetUid,
) -> Result<AlphaAmount, SubtensorError> {
    let extension = <SubtensorExtension as ChainExtensionInstance>::instantiate();
    let info = extension.get_stake_info(hotkey, coldkey, netuid)?;
    Ok(info.map_or(0, |info| info.stake_amount()))
}

/// Transfer `amount` of the calling contract's stake on `hotkey` to `destination`
/// in the same subnet, then check that the contract's stake fell by `amount` and
/// the destination's rose by `amount`, both within `TRANSFER_TOLERANCE`.
pub fn transfer_stake_verified(
    destination: AccountId,
    hotkey: AccountId,
    netuid: NetUid,
    amount: AlphaAmount,
) -> Result<(), TransferError> {
    let own_account = ink::env::account_id::<BittensorEnvironment>();
    let query = |coldkey| stake_of(hotkey, coldkey, netuid).map_err(|_| TransferError::QueryFailed);

    let own_before = query(own_account)?;
    let destination_before = query(destination)?;

    let extension = <SubtensorExtension as ChainExtensionInstance>::instantiate();
    extension
        .transfer_stake(
            destination,
            hotkey,
            netuid,
            netuid,
            AlphaCurrency::from(amount),
        )
        .map_err(TransferError::Rejected)?;

    let own_decrease = own_before.saturating_sub(query(own_account)?);
    let destination_increase = query(destination)?.saturating_sub(destination_before);
    if !stake_delta_verified(own_decrease, amount)
        || !stake_delta_verified(destination_increase, amount)
    {
        return Err(TransferError::NotVerified);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, MockSubtensor};
    use ink::env::test;

    #[test]
    fn verifies_exact_and_tolerated_stake_deltas() {
        let amount = 100;

        assert!(stake_delta_verified(amount, amount));
        assert!(stake_delta_verified(amount - TRANSFER_TOLERANCE, amount));
        assert!(stake_delta_verified(amount + TRANSFER_TOLERANCE, amount));
        assert!(!stake_delta_verified(
            amount - TRANSFER_TOLERANCE - 1,
            amount
        ));
        assert!(!stake_delta_verified(
            amount + TRANSFER_TOLERANCE + 1,
            amount
        ));
    }

    #[test]
    fn tolerance_saturates_for_small_amounts() {
        assert!(stake_delta_verified(0, TRANSFER_TOLERANCE - 1));
        assert!(!stake_delta_verified(
            2 * TRANSFER_TOLERANCE,
            TRANSFER_TOLERANCE - 1
        ));
    }

    fn accounts() -> test::DefaultAccounts<BittensorEnvironment> {
        test::default_accounts::<BittensorEnvironment>()
    }

    #[ink::test]
    fn verified_transfer_moves_stake_between_coldkeys() {
        let accounts = accounts();
        MockSubtensor::install(accounts.alice);
        test::set_callee::<BittensorEnvironment>(accounts.alice);
        testing::set_stake(accounts.bob, accounts.alice, 1, 1_000);

        assert_eq!(
            transfer_stake_verified(accounts.charlie, accounts.bob, 1, 400),
            Ok(())
        );
        assert_eq!(testing::stake(accounts.bob, accounts.alice, 1), 600);
        assert_eq!(testing::stake(accounts.bob, accounts.charlie, 1), 400);
    }

    #[ink::test]
    fn verified_transfer_tolerates_rounding_loss() {
        let accounts = accounts();
        MockSubtensor::install(accounts.alice);
        test::set_callee::<BittensorEnvironment>(accounts.alice);
        testing::set_stake(accounts.bob, accounts.alice, 1, 1_000);
        testing::set_transfer_loss(1);

        assert_eq!(
            transfer_stake_verified(accounts.charlie, accounts.bob, 1, 1_000),
            Ok(())
        );
        assert_eq!(testing::stake(accounts.bob, accounts.charlie, 1), 999);
    }

    #[ink::test]
    fn verified_transfer_tolerates_a_recipient_rounding_gain() {
        let accounts = accounts();
        MockSubtensor::install(accounts.alice);
        test::set_callee::<BittensorEnvironment>(accounts.alice);
        testing::set_stake(accounts.bob, accounts.alice, 1, 1_000);
        testing::set_stake(accounts.bob, accounts.charlie, 1, 1_000);
        testing::set_transfer_gain(1);

        assert_eq!(
            transfer_stake_verified(accounts.charlie, accounts.bob, 1, 1_000),
            Ok(())
        );
        assert_eq!(testing::stake(accounts.bob, accounts.charlie, 1), 2_001);
    }

    #[ink::test]
    fn verified_transfer_reports_rejections() {
        let accounts = accounts();
        MockSubtensor::install(accounts.alice);
        test::set_callee::<BittensorEnvironment>(accounts.alice);
        testing::set_stake(accounts.bob, accounts.alice, 1, 10);

        assert_eq!(
            transfer_stake_verified(accounts.charlie, accounts.bob, 1, 400),
            Err(TransferError::Rejected(
                SubtensorError::NotEnoughStakeToWithdraw
            ))
        );
        assert_eq!(testing::stake(accounts.bob, accounts.alice, 1), 10);
    }

    #[ink::test]
    fn verified_transfer_flags_a_wrong_amount() {
        let accounts = accounts();
        MockSubtensor::install(accounts.alice);
        test::set_callee::<BittensorEnvironment>(accounts.alice);
        testing::set_stake(accounts.bob, accounts.alice, 1, 1_000);
        testing::set_transfer_loss(TRANSFER_TOLERANCE + 1);

        assert_eq!(
            transfer_stake_verified(accounts.charlie, accounts.bob, 1, 1_000),
            Err(TransferError::NotVerified)
        );
    }
}
