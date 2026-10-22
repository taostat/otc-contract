use crate::types::TaoOfferId;
use ink::primitives::AccountId;
use otc_shared::{AlphaAmount, BlockNumber, FixedDecimal, NetUid, PriceOffsetBps, TaoAmount};

#[ink::event]
pub struct TaoOfferCreated {
    #[ink(topic)]
    pub buyer: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub tao_offer_id: TaoOfferId,
    pub amount: TaoAmount,
    pub price_offset_bps: PriceOffsetBps,
    pub max_price: u64,
}

#[ink::event]
pub struct TaoOfferCancelled {
    #[ink(topic)]
    pub buyer: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub tao_offer_id: TaoOfferId,
    pub amount_returned: TaoAmount,
}

#[ink::event]
pub struct TaoOfferTaken {
    #[ink(topic)]
    pub seller: AccountId,
    #[ink(topic)]
    pub buyer: AccountId,
    #[ink(topic)]
    pub netuid: NetUid,
    pub tao_offer_id: TaoOfferId,
    pub alpha_amount: AlphaAmount,
    /// TAO the seller received
    pub tao_amount: TaoAmount,
    pub executed_price: u64,
    pub fee: TaoAmount,
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
pub struct MinOfferAmountChanged {
    pub old_amount: TaoAmount,
    pub new_amount: TaoAmount,
}

/// New offers and fills are paused until `until`
#[ink::event]
pub struct TradingPaused {
    pub until: BlockNumber,
    pub paused_by: AccountId,
}

#[ink::event]
pub struct TradingResumed {
    pub resumed_by: AccountId,
}
