#![allow(clippy::cast_possible_truncation)]

use crate::types::AlphaAmount;
use ink::prelude::boxed::Box;
use ink::primitives::AccountId;

/// Runtime call enum for interacting with Bittensor pallets
#[ink::scale_derive(Encode)]
pub enum RuntimeCall {
    /// SubtensorModule pallet (index 7)
    #[codec(index = 7)]
    SubtensorModule(SubtensorCall),
    /// Proxy pallet (index 16)
    #[codec(index = 16)]
    Proxy(ProxyCall),
}

/// Proxy pallet calls
#[ink::scale_derive(Encode)]
pub enum ProxyCall {
    /// proxy(real, force_proxy_type, call)
    #[codec(index = 0)]
    Proxy {
        real: AccountId,
        force_proxy_type: Option<()>, // Using unit type for None in our case
        call: Box<RuntimeCall>,
    },
}

/// Subtensor pallet calls
#[ink::scale_derive(Encode)]
pub enum SubtensorCall {
    /// move_stake - Moves stake between hotkeys
    #[codec(index = 85)]
    MoveStake {
        origin_hotkey: AccountId,
        destination_hotkey: AccountId,
        origin_netuid: u16,
        destination_netuid: u16,
        alpha_amount: AlphaAmount, // Alpha amount in rao
    },
    /// transfer_stake - Transfers stake between coldkeys
    #[codec(index = 86)]
    TransferStake {
        destination_coldkey: AccountId,
        hotkey: AccountId,
        origin_netuid: u16,
        destination_netuid: u16,
        alpha_amount: AlphaAmount, // Alpha amount in rao
    },
}
