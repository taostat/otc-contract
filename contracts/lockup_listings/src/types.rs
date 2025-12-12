use ink::prelude::vec::Vec;
use ink::primitives::AccountId;
use ink::storage::Mapping;
use otc_shared::{AlphaAmount, BlockAge, BlockNumber, FixedDecimal, NetUid, PriceOffsetBps};

// ID types
pub type LockupListingId = u64;
pub type PurchaseId = u64;

// Mapping types
pub type LockupListingsMapping = Mapping<(NetUid, AccountId, LockupListingId), LockupListing>;
pub type UserLockupListingsMapping = Mapping<(AccountId, NetUid), Vec<LockupListingId>>;
pub type EscrowsMapping = Mapping<(NetUid, LockupListingId, PurchaseId), AccountId>;
pub type ReservedAlphaMapping = Mapping<NetUid, AlphaAmount>;

/// Lockup listing structure
#[derive(Debug, Clone, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
pub struct LockupListing {
    pub id: LockupListingId,
    pub netuid: NetUid,
    pub seller: AccountId,
    pub total_amount: AlphaAmount,     // Original total amount
    pub remaining_amount: AlphaAmount, // Amount not yet purchased
    pub price_offset_bps: PriceOffsetBps,
    pub lockup_duration: BlockAge, // Duration in blocks
    pub fee_rate: FixedDecimal,    // Fee rate at listing time
    pub created_at: BlockNumber,
}
