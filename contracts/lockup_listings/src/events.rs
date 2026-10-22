use crate::types::ListingId;
use ink::prelude::vec::Vec;
use ink::primitives::AccountId;
use otc_shared::{AlphaAmount, BlockAge, BlockNumber, FixedDecimal, NetUid, PriceOffsetBps};

/// A seller created a listing; it opens once the seller funds and activates its vault
#[ink::event]
pub struct ListingCreated {
    #[ink(topic)]
    pub seller: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub listing_id: ListingId,
    #[ink(topic)]
    pub vault: AccountId,
    pub hotkey: AccountId,
    pub amount: AlphaAmount,
    pub price_offset_bps: PriceOffsetBps,
    pub min_price: u64,
    /// `None` for a spot listing
    pub lockup_duration: Option<BlockAge>,
}

/// A vault reported that its listing filled or was cancelled
#[ink::event]
pub struct ListingClosed {
    #[ink(topic)]
    pub seller: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub listing_id: ListingId,
    #[ink(topic)]
    pub vault: AccountId,
}

/// The owner proposed a new owner, or withdrew the proposal with `None`
#[ink::event]
pub struct OwnershipTransferProposed {
    pub new_owner: Option<AccountId>,
}

#[ink::event]
pub struct OwnerChanged {
    pub old_owner: AccountId,
    pub new_owner: AccountId,
}

#[ink::event]
pub struct DefaultHotkeyChanged {
    pub old_hotkey: AccountId,
    pub new_hotkey: AccountId,
}

/// `hotkey` is `None` when the subnet falls back to the default hotkey
#[ink::event]
pub struct ValidatorHotkeyChanged {
    #[ink(topic)]
    pub netuid: NetUid,
    pub hotkey: Option<AccountId>,
}

#[ink::event]
pub struct FeeRateChanged {
    pub old_rate: FixedDecimal,
    pub new_rate: FixedDecimal,
}

#[ink::event]
pub struct FeeRecipientChanged {
    pub old_recipient: AccountId,
    pub new_recipient: AccountId,
}

#[ink::event]
pub struct MinListingAmountChanged {
    pub old_amount: AlphaAmount,
    pub new_amount: AlphaAmount,
}

#[ink::event]
pub struct MinPurchaseAmountChanged {
    pub old_amount: AlphaAmount,
    pub new_amount: AlphaAmount,
}

#[ink::event]
pub struct LockupDurationLimitsChanged {
    pub min: BlockAge,
    pub max: BlockAge,
}

#[ink::event]
pub struct MinListingAgeChanged {
    pub old_age: BlockAge,
    pub new_age: BlockAge,
}

/// New listings on `netuid` were frozen or unfrozen
#[ink::event]
pub struct SubnetFrozenChanged {
    #[ink(topic)]
    pub netuid: NetUid,
    pub frozen: bool,
    pub reason: Vec<u8>,
}

/// New listings and takes are paused until `until`
#[ink::event]
pub struct TradingPaused {
    pub until: BlockNumber,
    pub paused_by: AccountId,
}

#[ink::event]
pub struct TradingResumed {
    pub resumed_by: AccountId,
}
