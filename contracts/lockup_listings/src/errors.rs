#[derive(Debug, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub enum Error {
    /// Caller is not allowed to perform this action
    Unauthorized,
    /// New listings and takes are paused
    TradingPaused,
    /// Amount is below the minimum listing amount
    AmountTooSmall,
    /// The price offset prices Alpha at or below zero
    InvalidPriceOffset,
    /// Lockup duration is below the minimum
    LockupDurationTooShort,
    /// Lockup duration exceeds the maximum
    LockupDurationTooLong,
    /// The seller already has the maximum number of listings on this subnet
    TooManyListings,
    /// Arithmetic overflow
    Overflow,
    /// The fee rate exceeds `MAX_FEE_RATE`
    FeeRateTooHigh,
    /// The minimum lockup duration exceeds the maximum
    InvalidLockupDurationLimits,
    /// A pause must last between 1 block and `MAX_PAUSE`
    InvalidPauseDuration,
    /// Failed to create the listing vault
    VaultInstantiationFailed,
    /// The code registry has no current code for this kind
    NoCurrentCode,
    /// The requested code is not the registry's current factory code
    NotCurrentCode,
    /// Failed to query the code registry
    RegistryQueryFailed,
    /// Failed to replace the contract code
    CodeUpgradeFailed,
    /// No such listing in the index
    ListingNotFound,
    /// The listing's vault is still pending or open
    ListingStillOpen,
    /// Failed to query the listing's vault
    VaultQueryFailed,
}
