//! Off-chain stand-in for the Subtensor chain extension, for contract unit tests.
//! Contracts enable it with otc-shared's `testing` feature in their dev-dependencies.
//!
//! It keeps stake per (hotkey, coldkey, netuid) and moves it on `transfer_stake`
//! and `move_stake`, using the contract under test as the origin coldkey, like the
//! real extension. Call [`MockSubtensor::install`] at the start of each test with
//! that contract's account (the off-chain environment cannot be queried from
//! inside a chain-extension call). State is thread-local, so tests stay isolated.

use crate::chain_extension::StakeInfo;
use crate::runtime::{AlphaCurrency, NetUid as RuntimeNetUid, TaoCurrency};
use crate::types::{AlphaAmount, NetUid};
use ink::primitives::AccountId;
use ink::scale::{Compact, Decode, Encode};
use std::cell::RefCell;
use std::collections::BTreeMap;

const SUBTENSOR_EXTENSION_ID: u16 = 0;
const GET_STAKE_INFO: u16 = 0;
const MOVE_STAKE: u16 = 5;
const TRANSFER_STAKE: u16 = 6;
const GET_ALPHA_PRICE: u16 = 15;

const STATUS_OK: u32 = 0;
const STATUS_RUNTIME_ERROR: u32 = 1;
const STATUS_NOT_ENOUGH_STAKE: u32 = 6;

/// One transfer or move the mock executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StakeMovement {
    pub origin_coldkey: AccountId,
    pub origin_hotkey: AccountId,
    pub destination_coldkey: AccountId,
    pub destination_hotkey: AccountId,
    pub netuid: NetUid,
    pub amount: AlphaAmount,
}

#[derive(Default)]
struct State {
    contract: Option<AccountId>,
    stakes: BTreeMap<(AccountId, AccountId, NetUid), AlphaAmount>,
    transfer_loss: AlphaAmount,
    transfer_gain: AlphaAmount,
    reject_transfers: bool,
    alpha_price: u64,
    movements: Vec<StakeMovement>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// The registered chain extension.
#[derive(Clone, Copy)]
pub struct MockSubtensor;

impl MockSubtensor {
    /// Reset all mock state and register the mock for the current test.
    /// `contract` is the account of the contract under test; it is the origin
    /// coldkey of every transfer and move.
    pub fn install(contract: AccountId) {
        STATE.with(|state| {
            *state.borrow_mut() = State {
                contract: Some(contract),
                ..State::default()
            }
        });
        ink::env::test::register_chain_extension(MockSubtensor);
    }
}

/// Set the stake `coldkey` holds on `hotkey` in `netuid`.
pub fn set_stake(hotkey: AccountId, coldkey: AccountId, netuid: NetUid, amount: AlphaAmount) {
    STATE.with(|state| {
        state
            .borrow_mut()
            .stakes
            .insert((hotkey, coldkey, netuid), amount);
    });
}

/// The stake `coldkey` holds on `hotkey` in `netuid`.
pub fn stake(hotkey: AccountId, coldkey: AccountId, netuid: NetUid) -> AlphaAmount {
    STATE.with(|state| {
        state
            .borrow()
            .stakes
            .get(&(hotkey, coldkey, netuid))
            .copied()
            .unwrap_or(0)
    })
}

/// Rao the destination loses on every transfer or move, like Subtensor rounding.
pub fn set_transfer_loss(rao: AlphaAmount) {
    STATE.with(|state| state.borrow_mut().transfer_loss = rao);
}

/// Rao the destination gains on every transfer or move, like share-pool rounding
/// in favour of a recipient that already holds a position.
pub fn set_transfer_gain(rao: AlphaAmount) {
    STATE.with(|state| state.borrow_mut().transfer_gain = rao);
}

/// Make every transfer and move fail with a runtime error.
pub fn set_reject_transfers(reject: bool) {
    STATE.with(|state| state.borrow_mut().reject_transfers = reject);
}

/// The price `get_alpha_price` returns (TAO per Alpha, scaled by 1e9).
pub fn set_alpha_price(price: u64) {
    STATE.with(|state| state.borrow_mut().alpha_price = price);
}

/// Every transfer and move executed since `install`.
pub fn movements() -> Vec<StakeMovement> {
    STATE.with(|state| state.borrow().movements.clone())
}

fn decode_input(input: &[u8]) -> Vec<u8> {
    // The off-chain engine hands chain extensions their arguments as a
    // length-prefixed byte vector.
    Vec::<u8>::decode(&mut &input[..]).expect("mock: decode input bytes")
}

fn encode_stake_info(
    output: &mut Vec<u8>,
    hotkey: AccountId,
    coldkey: AccountId,
    netuid: NetUid,
    amount: AlphaAmount,
) {
    if amount == 0 {
        output.extend(Encode::encode(&Option::<StakeInfo>::None));
        return;
    }
    output.push(1);
    output.extend(Encode::encode(&hotkey));
    output.extend(Encode::encode(&coldkey));
    output.extend(Encode::encode(&Compact(RuntimeNetUid::from(netuid))));
    output.extend(Encode::encode(&Compact(AlphaCurrency::from(amount))));
    output.extend(Encode::encode(&Compact(0u64)));
    output.extend(Encode::encode(&Compact(AlphaCurrency::from(0))));
    output.extend(Encode::encode(&Compact(TaoCurrency::from(0))));
    output.extend(Encode::encode(&Compact(0u64)));
    output.extend(Encode::encode(&true));
}

fn move_stake(movement: StakeMovement) -> u32 {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.reject_transfers {
            return STATUS_RUNTIME_ERROR;
        }

        let origin_key = (
            movement.origin_hotkey,
            movement.origin_coldkey,
            movement.netuid,
        );
        let origin_stake = state.stakes.get(&origin_key).copied().unwrap_or(0);
        if origin_stake < movement.amount {
            return STATUS_NOT_ENOUGH_STAKE;
        }
        state
            .stakes
            .insert(origin_key, origin_stake.saturating_sub(movement.amount));

        let destination_key = (
            movement.destination_hotkey,
            movement.destination_coldkey,
            movement.netuid,
        );
        let received = movement
            .amount
            .saturating_sub(state.transfer_loss)
            .saturating_add(state.transfer_gain);
        let destination_stake = state.stakes.entry(destination_key).or_insert(0);
        *destination_stake = destination_stake.saturating_add(received);
        state.movements.push(movement);
        STATUS_OK
    })
}

impl ink::env::test::ChainExtension for MockSubtensor {
    fn ext_id(&self) -> u16 {
        SUBTENSOR_EXTENSION_ID
    }

    fn call(&mut self, func_id: u16, input: &[u8], output: &mut Vec<u8>) -> u32 {
        let payload = decode_input(input);
        let mut input = &payload[..];
        let caller = STATE
            .with(|state| state.borrow().contract)
            .expect("mock: call MockSubtensor::install first");

        match func_id {
            GET_STAKE_INFO => {
                let hotkey = AccountId::decode(&mut input).expect("mock: hotkey");
                let coldkey = AccountId::decode(&mut input).expect("mock: coldkey");
                let netuid = NetUid::decode(&mut input).expect("mock: netuid");
                encode_stake_info(
                    output,
                    hotkey,
                    coldkey,
                    netuid,
                    stake(hotkey, coldkey, netuid),
                );
                STATUS_OK
            }
            TRANSFER_STAKE => {
                let destination_coldkey = AccountId::decode(&mut input).expect("mock: destination");
                let hotkey = AccountId::decode(&mut input).expect("mock: hotkey");
                let origin_netuid = NetUid::decode(&mut input).expect("mock: origin netuid");
                let _destination_netuid = NetUid::decode(&mut input).expect("mock: netuid");
                let amount = AlphaCurrency::decode(&mut input).expect("mock: amount");
                move_stake(StakeMovement {
                    origin_coldkey: caller,
                    origin_hotkey: hotkey,
                    destination_coldkey,
                    destination_hotkey: hotkey,
                    netuid: origin_netuid,
                    amount: amount.as_u64(),
                })
            }
            MOVE_STAKE => {
                let origin_hotkey = AccountId::decode(&mut input).expect("mock: origin hotkey");
                let destination_hotkey = AccountId::decode(&mut input).expect("mock: hotkey");
                let origin_netuid = NetUid::decode(&mut input).expect("mock: origin netuid");
                let _destination_netuid = NetUid::decode(&mut input).expect("mock: netuid");
                let amount = AlphaCurrency::decode(&mut input).expect("mock: amount");
                move_stake(StakeMovement {
                    origin_coldkey: caller,
                    origin_hotkey,
                    destination_coldkey: caller,
                    destination_hotkey,
                    netuid: origin_netuid,
                    amount: amount.as_u64(),
                })
            }
            GET_ALPHA_PRICE => {
                let price = STATE.with(|state| state.borrow().alpha_price);
                output.extend(Encode::encode(&price));
                STATUS_OK
            }
            _ => STATUS_RUNTIME_ERROR,
        }
    }
}
