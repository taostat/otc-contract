use crate::types::{
    AlphaAmount, AlphaListingId, Balance, BlockAge, FixedDecimal, NetUid, PauseState,
    PriceOffsetBps, TaoAmount, TaoOfferId,
};
use ink::prelude::vec::Vec;
use ink::primitives::AccountId;

#[ink::event]
pub struct AlphaListed {
    #[ink(topic)]
    pub seller: AccountId,
    pub hotkey: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub alpha_listing_id: AlphaListingId,
    pub amount: AlphaAmount,
    pub price_offset_bps: PriceOffsetBps,
}

#[ink::event]
pub struct TaoOfferCreated {
    #[ink(topic)]
    pub buyer: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub tao_offer_id: TaoOfferId,
    pub amount: TaoAmount,
    pub price_offset_bps: PriceOffsetBps,
}

#[ink::event]
pub struct AlphaListingCancelled {
    #[ink(topic)]
    pub seller: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub listing_id: AlphaListingId,
    pub amount_returned: AlphaAmount,
}

#[ink::event]
pub struct AlphaListingForceCancelled {
    #[ink(topic)]
    pub seller: AccountId,
    #[ink(topic)]
    pub initiated_by: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub listing_id: AlphaListingId,
    pub amount_returned: AlphaAmount,
}

#[ink::event]
pub struct TaoOfferCancelled {
    #[ink(topic)]
    pub buyer: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub offer_id: TaoOfferId,
    pub amount_returned: TaoAmount,
}

#[ink::event]
pub struct AlphaListingTaken {
    #[ink(topic)]
    pub seller: AccountId,
    #[ink(topic)]
    pub buyer: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub alpha_amount: AlphaAmount,
    pub tao_amount: TaoAmount,
    pub price_offset_bps: PriceOffsetBps,
    pub executed_price: u64, // Market price at execution (price * 1e9)
    pub fee: Balance,
    pub alpha_listing_id: Option<AlphaListingId>,
}

#[ink::event]
pub struct TaoOfferTaken {
    #[ink(topic)]
    pub seller: AccountId,
    #[ink(topic)]
    pub buyer: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub alpha_amount: AlphaAmount,
    pub tao_amount: TaoAmount,
    pub price_offset_bps: PriceOffsetBps,
    pub executed_price: u64, // Market price at execution (price * 1e9)
    pub fee: Balance,
    pub tao_offer_id: Option<TaoOfferId>,
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
pub struct MinOfferAmountUpdated {
    pub old_amount: TaoAmount,
    pub new_amount: TaoAmount,
}

#[ink::event]
pub struct MinListingAmountUpdated {
    pub old_amount: AlphaAmount,
    pub new_amount: AlphaAmount,
}

#[ink::event]
pub struct MinListingAgeUpdated {
    pub old_age: BlockAge,
    pub new_age: BlockAge,
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

#[ink::event]
pub struct DividendsClaimed {
    #[ink(topic)]
    pub owner: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub amount: AlphaAmount,
    pub reserved_after: AlphaAmount,
}

#[ink::event]
pub struct SubnetListingStatusChanged {
    #[ink(topic)]
    pub netuid: NetUid,
    pub frozen: bool,
    pub reason: Vec<u8>,
    pub changed_by: AccountId,
}
