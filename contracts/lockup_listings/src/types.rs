use ink::prelude::vec::Vec;
use ink::primitives::AccountId;
use ink::storage::Mapping;
use otc_shared::{AlphaAmount, BlockAge, BlockNumber, FixedDecimal, NetUid};

pub type ListingId = u64;

/// Where a listing sits in the index: `(netuid, seller, listing_id)`.
pub type ListingKey = (NetUid, AccountId, ListingId);

/// A seller's open listings per subnet: `(seller, netuid) -> listing ids`.
pub type UserListings = Mapping<(AccountId, NetUid), Vec<ListingId>>;

/// The factory's settings. Vaults copy the fee rate and minimum purchase at
/// creation; they read the fee recipient, validator hotkeys and the pause live.
#[derive(Debug, Clone, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub struct Config {
    pub owner: AccountId,
    pub registry: AccountId,
    pub default_hotkey: AccountId,
    pub fee_rate: FixedDecimal,
    pub fee_recipient: AccountId,
    pub min_listing_amount: AlphaAmount,
    pub min_purchase_amount: AlphaAmount,
    pub min_lockup_duration: BlockAge,
    pub max_lockup_duration: BlockAge,
    pub min_listing_age: BlockAge,
    pub paused_until: BlockNumber,
}
