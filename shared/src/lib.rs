#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub mod chain_extension;
pub mod errors;
pub mod runtime;
pub mod stake;
pub mod types;

// Re-export chain extension types
pub use chain_extension::{StakeInfo, SubtensorError, SubtensorExtension};

// Re-export shared error types
pub use errors::SharedError;

// Re-export runtime types (for SCALE encoding)
pub use runtime::{AlphaCurrency, ProxyCall, ProxyType, RuntimeCall, SubtensorCall, TaoCurrency};

// Re-export stake transfer verification helpers
pub use stake::{stake_delta_verified, TRANSFER_TOLERANCE};

// Re-export common types (type aliases and FixedDecimal)
pub use types::{
    AlphaAmount, Balance, BlockAge, BlockNumber, FixedDecimal, NetUid, PauseState, PriceOffsetBps,
    TaoAmount,
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
