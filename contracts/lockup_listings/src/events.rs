use crate::types::{LockupListingId, PurchaseId};
use ink::prelude::vec::Vec;
use ink::primitives::AccountId;
use otc_shared::{
    AlphaAmount, Balance, BlockAge, BlockNumber, FixedDecimal, NetUid, PauseState, PriceOffsetBps,
    TaoAmount,
};

#[ink::event]
pub struct LockupListingCreated {
    #[ink(topic)]
    pub seller: AccountId,
    pub hotkey: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub listing_id: LockupListingId,
    pub amount: AlphaAmount,
    pub price_offset_bps: PriceOffsetBps,
    pub lockup_duration: BlockAge,
}

#[ink::event]
pub struct LockupListingTaken {
    #[ink(topic)]
    pub seller: AccountId,
    #[ink(topic)]
    pub buyer: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub listing_id: LockupListingId,
    pub purchase_id: PurchaseId,
    pub alpha_amount: AlphaAmount,
    pub tao_amount: TaoAmount,
    pub executed_price: u64,
    pub unlock_block: BlockNumber,
    pub escrow_account: AccountId,
    pub fee: Balance,
}

#[ink::event]
pub struct LockupListingCancelled {
    #[ink(topic)]
    pub seller: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub listing_id: LockupListingId,
    pub amount_returned: AlphaAmount,
}

#[ink::event]
pub struct LockupListingForceCancelled {
    #[ink(topic)]
    pub seller: AccountId,
    #[ink(topic)]
    pub initiated_by: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub listing_id: LockupListingId,
    pub amount_returned: AlphaAmount,
}

#[ink::event]
pub struct LockupListingFullyFilled {
    #[ink(topic)]
    pub seller: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub listing_id: LockupListingId,
}

#[ink::event]
pub struct OwnerUpdated {
    pub old_owner: AccountId,
    pub new_owner: AccountId,
}

#[ink::event]
pub struct HotkeyUpdated {
    pub old_hotkey: AccountId,
    pub new_hotkey: AccountId,
}

#[ink::event]
pub struct FeeRateUpdated {
    pub old_rate: FixedDecimal,
    pub new_rate: FixedDecimal,
}

#[ink::event]
pub struct MinListingAmountUpdated {
    pub old_amount: AlphaAmount,
    pub new_amount: AlphaAmount,
}

#[ink::event]
pub struct MinPurchaseAmountUpdated {
    pub old_amount: AlphaAmount,
    pub new_amount: AlphaAmount,
}

#[ink::event]
pub struct LockupDurationUpdated {
    pub old_min: BlockAge,
    pub old_max: BlockAge,
    pub new_min: BlockAge,
    pub new_max: BlockAge,
}

#[ink::event]
pub struct EscrowCodeHashUpdated {
    pub old_hash: [u8; 32],
    pub new_hash: [u8; 32],
}

#[ink::event]
pub struct ContractPaused {
    #[ink(topic)]
    pub pause_state: PauseState,
    pub reason: Vec<u8>,
    pub paused_by: AccountId,
}

#[ink::event]
pub struct ContractResumed {
    pub resumed_by: AccountId,
}
