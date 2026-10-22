#[derive(Debug, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub enum Error {
    /// Caller is not allowed to perform this action
    Unauthorized,
    /// New offers and fills are paused
    TradingPaused,
    /// Amount is below the minimum, or rounds to nothing
    AmountTooSmall,
    /// The price offset prices Alpha at or below zero
    InvalidPriceOffset,
    /// The buyer already has the maximum number of offers on this subnet
    TooManyOffers,
    /// No such offer
    OfferNotFound,
    /// Arithmetic overflow
    Overflow,
    /// Failed to fetch the market price
    MarketPriceFetchFailed,
    /// The executed price is above the buyer's ceiling
    PriceAboveMaximum,
    /// Filling the offer would take more Alpha than the seller allowed
    AlphaAboveMaximum,
    /// Failed to query stake
    StakeQueryFailed,
    /// The seller holds less stake than the fill needs
    InsufficientStake,
    /// The proxied stake transfer failed; the seller may not have added the proxy
    RuntimeCallFailed,
    /// Failed to transfer TAO
    TransferFailed,
    /// The fee rate exceeds `MAX_FEE_RATE`
    FeeRateTooHigh,
    /// A pause must last between 1 block and `MAX_PAUSE`
    InvalidPauseDuration,
    /// The code registry has no current code for this contract
    NoCurrentCode,
    /// The requested code is not the registry's current otc code
    NotCurrentCode,
    /// Failed to query the code registry
    RegistryQueryFailed,
    /// Failed to replace the contract code
    CodeUpgradeFailed,
}
