//! Events every position contract (listing vaults and escrows) emits the same
//! way. They are defined once here: ink! puts the events of every linked
//! contract in a contract's metadata, and two events with the same name and
//! fields would share a signature topic.

use crate::types::{AlphaAmount, Balance};
use crate::upgrade::UpgradePolicy;
use ink::primitives::{AccountId, Hash};

/// The position's TAO balance was sent to its owner
#[ink::event]
pub struct TaoSwept {
    #[ink(topic)]
    pub owner: AccountId,
    #[ink(topic)]
    pub initiated_by: AccountId,
    pub amount: Balance,
}

/// The position now points at the hotkey its stake moved to
#[ink::event]
pub struct HotkeyResynced {
    #[ink(topic)]
    pub old_hotkey: AccountId,
    #[ink(topic)]
    pub new_hotkey: AccountId,
    #[ink(topic)]
    pub initiated_by: AccountId,
    pub new_hotkey_stake: AlphaAmount,
}

/// The position installed new code
#[ink::event]
pub struct Upgraded {
    #[ink(topic)]
    pub code_hash: Hash,
    #[ink(topic)]
    pub initiated_by: AccountId,
}

/// The owner changed who may upgrade the position
#[ink::event]
pub struct UpgradePolicyChanged {
    #[ink(topic)]
    pub owner: AccountId,
    pub policy: UpgradePolicy,
}
