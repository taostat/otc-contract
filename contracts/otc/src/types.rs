use ink::prelude::vec::Vec;
use ink::primitives::AccountId;
use ink::storage::Mapping;
use otc_shared::{AlphaAmount, BlockNumber, FixedDecimal, NetUid, PriceOffsetBps};

// ID types
pub type AlphaListingId = u64;
pub type TaoOfferId = u64;

// Mapping types
pub type AlphaListingsMapping = Mapping<(NetUid, AccountId, AlphaListingId), AlphaListing>;
pub type UserListingsMapping = Mapping<(AccountId, NetUid), Vec<AlphaListingId>>;
pub type TaoOffersMapping = Mapping<(NetUid, AccountId, TaoOfferId), TaoOffer>;
pub type UserOffersMapping = Mapping<(AccountId, NetUid), Vec<TaoOfferId>>;
pub type ReservedAlphaMapping = Mapping<NetUid, AlphaAmount>;
pub type FrozenSubnetsMapping = Mapping<NetUid, bool>;

/// Alpha listing structure
#[derive(Debug, Clone, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
pub struct AlphaListing {
    pub id: AlphaListingId,
    pub netuid: NetUid,
    pub seller: AccountId,
    pub amount: AlphaAmount,              // Alpha amount in rao
    pub price_offset_bps: PriceOffsetBps, // Price offset from market in basis points
    pub fee_rate: FixedDecimal, // Fee rate at listing time (as decimal, e.g., 0.005 = 0.5%)
    pub created_at: BlockNumber,
}

/// TAO offer structure
#[derive(Debug, Clone, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
pub struct TaoOffer {
    pub id: TaoOfferId,
    pub netuid: NetUid,
    pub buyer: AccountId,
    pub amount: otc_shared::TaoAmount, // TAO amount offered in rao
    pub price_offset_bps: PriceOffsetBps, // Price offset from market in basis points
    pub fee_rate: FixedDecimal,        // Fee rate at offer time (as decimal, e.g., 0.005 = 0.5%)
    pub created_at: BlockNumber,
}
