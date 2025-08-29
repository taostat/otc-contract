use crate::types::{AlphaListingId, FixedDecimal, NetUid, TaoOfferId};
use ink::primitives::AccountId;

#[ink::event]
pub struct AlphaListed {
    #[ink(topic)]
    pub seller: AccountId,
    pub hotkey: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub alpha_listing_id: AlphaListingId,
    pub amount: u64,
    pub price: FixedDecimal,
}

#[ink::event]
pub struct TaoOfferCreated {
    #[ink(topic)]
    pub buyer: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub tao_offer_id: TaoOfferId,
    pub amount: u64,
    pub price: FixedDecimal,
}

#[ink::event]
pub struct AlphaListingCancelled {
    #[ink(topic)]
    pub seller: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub listing_id: AlphaListingId,
    pub amount_returned: u64,
}

#[ink::event]
pub struct TaoOfferCancelled {
    #[ink(topic)]
    pub buyer: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub offer_id: TaoOfferId,
    pub amount_returned: u64,
}

#[ink::event]
pub struct AlphaListingTaken {
    #[ink(topic)]
    pub seller: AccountId,
    #[ink(topic)]
    pub buyer: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub alpha_amount: u64,
    pub tao_amount: u64,
    pub price: FixedDecimal,
    pub fee: u64,
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
    pub alpha_amount: u64,
    pub tao_amount: u64,
    pub price: FixedDecimal,
    pub fee: u64,
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
    pub old_amount: u64,
    pub new_amount: u64,
}

#[ink::event]
pub struct MinListingAmountUpdated {
    pub old_amount: u64,
    pub new_amount: u64,
}

#[ink::event]
pub struct MinListingAgeUpdated {
    pub old_age: u64,
    pub new_age: u64,
}
