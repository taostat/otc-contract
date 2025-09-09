#![allow(clippy::cast_possible_truncation)]

use ink::prelude::boxed::Box;
use ink::primitives::AccountId;
use sp_runtime::MultiAddress;

#[repr(transparent)]
#[ink::scale_derive(Encode)]
pub struct AlphaCurrency(pub u64);

impl From<u64> for AlphaCurrency {
    fn from(value: u64) -> Self {
        AlphaCurrency(value)
    }
}

#[repr(transparent)]
#[ink::scale_derive(Encode)]
pub struct NetUid(pub u16);

impl From<u16> for NetUid {
    fn from(value: u16) -> Self {
        NetUid(value)
    }
}

#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub enum ProxyType {
    Any,
    Owner,
    NonCritical,
    NonTransfer,
    Senate,
    NonFungibile,
    Triumvirate,
    Governance,
    Staking,
    Registration,
    Transfer,
    SmallTransfer,
    RootWeights,
    ChildKeys,
    SudoUncheckedSetCode,
    SwapHotkey,
    SubnetLeaseBeneficiary,
}

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
        real: MultiAddress<AccountId, ()>,
        force_proxy_type: Option<ProxyType>,
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
        origin_netuid: NetUid,
        destination_netuid: NetUid,
        alpha_amount: AlphaCurrency,
    },
    /// transfer_stake - Transfers stake between coldkeys
    #[codec(index = 86)]
    TransferStake {
        destination_coldkey: AccountId,
        hotkey: AccountId,
        origin_netuid: NetUid,
        destination_netuid: NetUid,
        alpha_amount: AlphaCurrency,
    },
}
