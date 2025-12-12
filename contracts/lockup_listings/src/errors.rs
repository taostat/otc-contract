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
}
