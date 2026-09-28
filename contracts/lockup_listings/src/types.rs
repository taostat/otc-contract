use ink::prelude::vec::Vec;
use ink::primitives::{AccountId, Hash};
use ink::storage::Mapping;
use otc_shared::{
    AlphaAmount, BlockAge, BlockNumber, FixedDecimal, NetUid, PriceOffsetBps, TaoAmount,
};

// ID types
pub type LockupListingId = u64;
pub type PurchaseId = u64;

// Mapping types
pub type LockupListingsMapping = Mapping<(NetUid, AccountId, LockupListingId), LockupListing>;
pub type UserLockupListingsMapping = Mapping<(AccountId, NetUid), Vec<LockupListingId>>;
pub type EscrowsMapping = Mapping<(NetUid, LockupListingId, PurchaseId), AccountId>;
pub type ReservedAlphaMapping = Mapping<NetUid, AlphaAmount>;
pub type ReservedAlphaByHotkeyMapping = Mapping<(NetUid, AccountId), AlphaAmount>;
pub type ReservedAlphaByGenerationMapping = Mapping<(NetUid, u64), AlphaAmount>;
pub type StaleListingRecoveryPoolsMapping = Mapping<(NetUid, u64), StaleListingRecoveryPool>;
pub type ActiveHotkeysMapping = Mapping<NetUid, AccountId>;

/// Owner-funded TAO recovery pool for stale open listings after subnet deregistration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
pub struct StaleListingRecoveryPool {
    pub netuid: NetUid,
    pub subnet_generation: u64,
    pub tao_total: TaoAmount,
    pub tao_remaining: TaoAmount,
    pub alpha_total: AlphaAmount,
    pub alpha_remaining: AlphaAmount,
    pub evidence_hash: Hash,
    pub opened_by: AccountId,
    pub opened_at: BlockNumber,
    pub closed: bool,
}

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
    pub custody_hotkey: AccountId,     // Hotkey where factory-held Alpha currently sits
    pub subnet_generation: u64,
    pub price_offset_bps: PriceOffsetBps,
    pub lockup_duration: BlockAge, // Duration in blocks
    pub fee_rate: FixedDecimal,    // Fee rate at listing time
    pub created_at: BlockNumber,
}
