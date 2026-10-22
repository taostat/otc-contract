#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub mod chain_extension;
pub mod errors;
pub mod events;
pub mod factory;
pub mod pricing;
pub mod runtime;
pub mod stake;
#[cfg(all(feature = "std", any(test, feature = "testing")))]
pub mod testing;
pub mod types;
pub mod upgrade;

// Re-export chain extension types
pub use chain_extension::{StakeInfo, SubtensorError, SubtensorExtension};

// Re-export shared error types
pub use errors::SharedError;

// Re-export runtime types (for SCALE encoding)
pub use runtime::{AlphaCurrency, ProxyCall, ProxyType, RuntimeCall, SubtensorCall, TaoCurrency};

// Re-export stake transfer verification helpers
pub use stake::{
    move_stake_verified, stake_delta_verified, stake_of, transfer_stake_verified, TransferError,
    TRANSFER_TOLERANCE,
};

// Re-export listing pricing and the interface vaults use to read their factory
pub use factory::VaultFactory;
pub use pricing::{quote, PricingError, Quote, BITTENSOR_MIN_STAKE, MAX_FEE_RATE};

// Re-export code registry kinds and the position upgrade rule
pub use upgrade::{check_upgrade, CodeKind, UpgradeError, UpgradePolicy};

// Re-export common types (type aliases and FixedDecimal)
pub use types::{
    AlphaAmount, Balance, BlockAge, BlockNumber, FixedDecimal, NetUid, PauseState, PriceOffsetBps,
    TaoAmount, MAX_PAUSE,
};

/// Custom environment for Bittensor contracts
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "std", ink::scale_derive(TypeInfo))]
pub struct BittensorEnvironment;

impl ink::env::Environment for BittensorEnvironment {
    const MAX_EVENT_TOPICS: usize = 4;
    type AccountId = ink::primitives::AccountId;
    type Balance = u64;
    type Hash = ink::primitives::Hash;
    type Timestamp = u64;
    type BlockNumber = u32;
    type ChainExtension = crate::chain_extension::SubtensorExtension;
}
