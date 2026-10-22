//! The interface listing vaults use to read the factory that created them.

use crate::types::NetUid;
use ink::primitives::AccountId;

/// What a listing vault asks its factory. Factories hold no Alpha: a vault
/// consults its factory only when a buyer takes (the trading pause and the fee
/// recipient) and to learn which validator hotkey to delegate to. Cancelling and
/// sweeping never depend on the factory, so a broken or hostile factory can
/// block takes but never trap a seller's funds.
#[ink::trait_definition]
pub trait VaultFactory {
    /// Whether new takes are paused.
    #[ink(message)]
    fn trading_paused(&self) -> bool;

    /// Who receives trading fees.
    #[ink(message)]
    fn fee_recipient(&self) -> AccountId;

    /// The validator hotkey listings on `netuid` delegate to.
    #[ink(message)]
    fn validator_hotkey(&self, netuid: NetUid) -> AccountId;

    /// Called by a vault when its listing closes, so the factory can drop it from
    /// its index. Calls from anything but one of the factory's vaults are ignored.
    #[ink(message)]
    fn listing_closed(&mut self);
}
