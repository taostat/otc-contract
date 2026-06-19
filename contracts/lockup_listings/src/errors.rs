#[derive(Debug, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub enum Error {
    /// Unauthorized access
    Unauthorized,
    /// Listing not found
    ListingNotFound,
    /// Amount below minimum threshold
    AmountTooSmall,
    /// Arithmetic overflow
    Overflow,
    /// Division by zero error
    DivisionByZero,
    /// Runtime call failed
    RuntimeCallFailed,
    /// Transfer failed
    TransferFailed,
    /// Chain extension query failed
    StakeQueryFailed,
    /// Stake transfer verification failed
    StakeTransferNotVerified,
    /// Insufficient stake for operation
    InsufficientStake,
    /// Contract is fully paused
    ContractFullyPaused,
    /// Trading is paused
    TradingPaused,
    /// Maximum active listings reached
    TooManyListings,
    /// Failed to fetch market price from chain extension
    MarketPriceFetchFailed,
    /// Invalid price offset (e.g., <= -100%)
    InvalidPriceOffset,
    /// Insufficient payment (buyer sent less TAO than required)
    InsufficientPayment,
    /// Lockup duration is below minimum
    LockupDurationTooShort,
    /// Lockup duration exceeds maximum
    LockupDurationTooLong,
    /// Amount exceeds remaining in listing
    AmountExceedsRemaining,
    /// Failed to instantiate escrow contract
    EscrowInstantiationFailed,
    /// Code upgrade failed
    CodeUpgradeFailed,
    /// Amount would leave listing below Bittensor minimum stake requirement
    AmountBelowMinimumStake,
    /// Escrow not found for a purchase
    EscrowNotFound,
    /// Proposed hotkey is invalid
    InvalidHotkey,
    /// Escrow hotkey sync failed
    EscrowSyncFailed,
    /// Failed to query subnet registration state from chain extension
    SubnetRegistrationQueryFailed,
    /// The requested subnet does not currently exist
    SubnetNotFound,
    /// Runtime returned an invalid zero subnet generation
    InvalidSubnetGeneration,
    /// Listing was created for an older subnet generation
    SubnetGenerationMismatch,
    /// Alpha is currently unavailable because of conviction locks
    StakeUnavailable,
    /// A stale listing recovery pool already exists for this subnet generation
    RecoveryPoolAlreadyExists,
    /// A stale listing recovery pool was not found
    RecoveryPoolNotFound,
    /// The stale listing recovery pool has already been closed
    RecoveryPoolClosed,
    /// Runtime state does not prove the listing subnet generation is stale
    RecoveryPoolNotStale,
    /// No reserved stale Alpha exists for this subnet generation
    NoRecoverableAlpha,
    /// Contract TAO balance outside recovery pools is too low
    InsufficientUnreservedTao,
    /// Recovery payout would be zero before the final dust assignment
    RecoveryPayoutTooSmall,
}
