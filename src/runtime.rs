#![allow(clippy::cast_possible_truncation)]

use ink::prelude::boxed::Box;
use ink::primitives::AccountId;
use ink::scale::{Compact, CompactAs, Error as CodecError};
use sp_runtime::MultiAddress;

#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(transparent)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub struct AlphaCurrency(u64);

impl From<u64> for AlphaCurrency {
    fn from(value: u64) -> Self {
        AlphaCurrency(value)
    }
}

impl CompactAs for AlphaCurrency {
    type As = u64;

    fn encode_as(&self) -> &Self::As {
        &self.0
    }

    fn decode_from(v: Self::As) -> Result<Self, CodecError> {
        Ok(Self(v))
    }
}

impl From<Compact<AlphaCurrency>> for AlphaCurrency {
    fn from(c: Compact<AlphaCurrency>) -> Self {
        c.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(transparent)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub struct TaoCurrency(u64);

impl From<u64> for TaoCurrency {
    fn from(value: u64) -> Self {
        TaoCurrency(value)
    }
}

impl CompactAs for TaoCurrency {
    type As = u64;

    fn encode_as(&self) -> &Self::As {
        &self.0
    }

    fn decode_from(v: Self::As) -> Result<Self, CodecError> {
        Ok(Self(v))
    }
}

impl From<Compact<TaoCurrency>> for TaoCurrency {
    fn from(c: Compact<TaoCurrency>) -> Self {
        c.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(transparent)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub struct NetUid(u16);

impl From<u16> for NetUid {
    fn from(value: u16) -> Self {
        NetUid(value)
    }
}

impl CompactAs for NetUid {
    type As = u16;

    fn encode_as(&self) -> &Self::As {
        &self.0
    }

    fn decode_from(v: Self::As) -> Result<Self, CodecError> {
        Ok(Self(v))
    }
}

impl From<Compact<NetUid>> for NetUid {
    fn from(c: Compact<NetUid>) -> Self {
        c.0
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
