use crate::runtime::{AlphaCurrency, NetUid, TaoCurrency};
use crate::types::AlphaAmount;
use ink::primitives::AccountId;
use ink::scale::Compact;

#[derive(PartialEq, Eq, Clone, Debug)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub struct StakeInfo {
    hotkey: AccountId,
    coldkey: AccountId,
    netuid: Compact<NetUid>,
    stake: Compact<AlphaCurrency>,
    locked: Compact<u64>,
    emission: Compact<AlphaCurrency>,
    tao_emission: Compact<TaoCurrency>,
    drain: Compact<u64>,
    is_registered: bool,
}

impl StakeInfo {
    pub fn stake_amount(&self) -> AlphaAmount {
        // Extract from Compact<AlphaCurrency>
        let alpha_currency = self.stake.0.clone();
        alpha_currency.as_u64()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub enum SubtensorError {
    GetStakeInfoFailed,
    Unknown,
}

impl From<ink::scale::Error> for SubtensorError {
    fn from(_: ink::scale::Error) -> Self {
        panic!("encountered unexpected invalid SCALE encoding")
    }
}

impl ink::env::chain_extension::FromStatusCode for SubtensorError {
    fn from_status_code(status_code: u32) -> Result<(), Self> {
        match status_code {
            0 => Ok(()),
            1 => Err(Self::GetStakeInfoFailed),
            _ => panic!("encountered unknown status code"),
        }
    }
}

#[ink::chain_extension(extension = 0)]
pub trait SubtensorExtension {
    type ErrorCode = SubtensorError;

    #[ink(function = 1001)]
    fn get_stake_info(
        hotkey: AccountId,
        coldkey: AccountId,
        netuid: u16,
    ) -> Result<Option<StakeInfo>, SubtensorError>;
}
