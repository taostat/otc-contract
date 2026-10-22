use ink::prelude::vec::Vec;
use ink::primitives::AccountId;
use ink::storage::Mapping;
use otc_shared::{BlockNumber, FixedDecimal, NetUid, PriceOffsetBps, TaoAmount};

pub type TaoOfferId = u64;

/// Where an offer sits: `(netuid, buyer, offer_id)`.
pub type OfferKey = (NetUid, AccountId, TaoOfferId);

/// A buyer's open offers per subnet: `(buyer, netuid) -> offer ids`.
pub type UserOffers = Mapping<(AccountId, NetUid), Vec<TaoOfferId>>;

/// TAO a buyer deposited to buy Alpha on a subnet
#[derive(Debug, Clone, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
pub struct TaoOffer {
    pub id: TaoOfferId,
    pub netuid: NetUid,
    pub buyer: AccountId,
    /// TAO deposited, in rao; the fee comes out of it
    pub amount: TaoAmount,
    /// Offset from the market price, in basis points
    pub price_offset_bps: PriceOffsetBps,
    /// The buyer's ceiling, TAO per Alpha scaled by 1e9; 0 means none
    pub max_price: u64,
    /// Fee rate snapshot at creation
    pub fee_rate: FixedDecimal,
    pub created_at: BlockNumber,
}

/// What filling an offer pays at the current market price
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub struct OfferQuote {
    /// Executed price, TAO per Alpha scaled by 1e9
    pub price: u64,
    /// Alpha the seller sends
    pub alpha: otc_shared::AlphaAmount,
    /// TAO the seller receives: the offer minus the fee
    pub tao: TaoAmount,
    /// Fee paid out of the offer
    pub fee: TaoAmount,
}

/// The contract's settings
#[derive(Debug, Clone, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub struct Config {
    pub owner: AccountId,
    pub registry: AccountId,
    pub fee_rate: FixedDecimal,
    pub fee_recipient: AccountId,
    pub min_offer_amount: TaoAmount,
    pub paused_until: BlockNumber,
}
