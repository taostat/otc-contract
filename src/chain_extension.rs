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
    /// Unknow status code
    UnknownStatusCode,
    /// Unknown error
    RuntimeError = 1,
    /// Not enough balance to stake
    NotEnoughBalanceToStake = 2,
    /// Coldkey is not associated with the hotkey
    NonAssociatedColdKey = 3,
    /// Error withdrawing balance
    BalanceWithdrawalError = 4,
    /// Hotkey is not registered
    NotRegistered = 5,
    /// Not enough stake to withdraw
    NotEnoughStakeToWithdraw = 6,
    /// Transaction rate limit exceeded
    TxRateLimitExceeded = 7,
    /// Slippage is too high for the transaction
    SlippageTooHigh = 8,
    /// Subnet does not exist
    SubnetNotExists = 9,
    /// Hotkey is not registered in subnet
    HotKeyNotRegisteredInSubNet = 10,
    /// Same auto stake hotkey already set
    SameAutoStakeHotkeyAlreadySet = 11,
    /// Insufficient balance
    InsufficientBalance = 12,
    /// Amount is too low
    AmountTooLow = 13,
    /// Insufficient liquidity
    InsufficientLiquidity = 14,
    /// Same netuid
    SameNetuid = 15,
    /// Encountered unexpected invalid SCALE encoding
    InvalidScaleEncoding,
}

impl From<ink::scale::Error> for SubtensorError {
    fn from(_: ink::scale::Error) -> Self {
        SubtensorError::InvalidScaleEncoding
    }
}

impl ink::env::chain_extension::FromStatusCode for SubtensorError {
    fn from_status_code(status_code: u32) -> Result<(), Self> {
        match status_code {
            0 => Ok(()),
            1 => Err(SubtensorError::RuntimeError),
            2 => Err(SubtensorError::NotEnoughBalanceToStake),
            3 => Err(SubtensorError::NonAssociatedColdKey),
            4 => Err(SubtensorError::BalanceWithdrawalError),
            5 => Err(SubtensorError::NotRegistered),
            6 => Err(SubtensorError::NotEnoughStakeToWithdraw),
            7 => Err(SubtensorError::TxRateLimitExceeded),
            8 => Err(SubtensorError::SlippageTooHigh),
            9 => Err(SubtensorError::SubnetNotExists),
            10 => Err(SubtensorError::HotKeyNotRegisteredInSubNet),
            11 => Err(SubtensorError::SameAutoStakeHotkeyAlreadySet),
            12 => Err(SubtensorError::InsufficientBalance),
            13 => Err(SubtensorError::AmountTooLow),
            14 => Err(SubtensorError::InsufficientLiquidity),
            15 => Err(SubtensorError::SameNetuid),
            _ => Err(SubtensorError::UnknownStatusCode),
        }
    }
}

#[ink::chain_extension(extension = 0)]
pub trait SubtensorExtension {
    type ErrorCode = SubtensorError;

    #[ink(function = 0)]
    fn get_stake_info(
        hotkey: AccountId,
        coldkey: AccountId,
        netuid: u16,
    ) -> Result<Option<StakeInfo>, SubtensorError>;

    #[ink(function = 5)]
    fn move_stake(
        origin_hotkey: AccountId,
        destination_hotkey: AccountId,
        origin_netuid: u16,
        destination_netuid: u16,
        alpha_amount: AlphaCurrency,
    ) -> Result<(), SubtensorError>;

    #[ink(function = 6)]
    fn transfer_stake(
        destination_coldkey: AccountId,
        hotkey: AccountId,
        origin_netuid: u16,
        destination_netuid: u16,
        alpha_amount: AlphaCurrency,
    ) -> Result<(), SubtensorError>;
}
