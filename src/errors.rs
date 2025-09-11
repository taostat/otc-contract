#[derive(Debug, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub enum Error {
    /// Insufficient Alpha balance
    InsufficientAlphaBalance,
    /// Insufficient TAO balance
    InsufficientTaoBalance,
    /// Listing not found
    ListingNotFound,
    /// Offer not found
    OfferNotFound,
    /// Not the owner of the listing/offer
    NotOwner,
    /// Amount below minimum threshold
    AmountTooSmall,
    /// Invalid price
    InvalidPrice,
    /// Unauthorized access
    Unauthorized,
    /// Listing too young to cancel
    ListingTooYoung,
    /// Division by zero error
    DivisionByZero,
    /// Arithmetic overflow
    Overflow,
    /// Runtime call failed
    RuntimeCallFailed,
    /// Transfer failed
    TransferFailed,
    /// Invalid hotkey
    InvalidHotkey,
    /// Invalid netuid
    InvalidNetuid,
    /// Chain extension query failed
    StakeQueryFailed,
    /// Stake transfer verification failed
    StakeTransferNotVerified,
    /// Insufficient stake for operation
    InsufficientStake,
    /// TAO balance query failed
    TaoBalanceQueryFailed,
    /// Code upgrade failed
    CodeUpgradeFailed,
}
