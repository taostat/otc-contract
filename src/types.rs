use crate::errors::Error;
use fixed::types::U64F64;
use ink::prelude::vec::Vec;
use ink::primitives::AccountId;
use ink::storage::Mapping;

// ID types
pub type AlphaListingId = u64;
pub type TaoOfferId = u64;

// Network and blockchain types
pub type NetUid = u16;
pub type BlockNumber = u32;
pub type BlockAge = u32; // Age/duration in blocks

// Balance and amount types (all u64 in Bittensor)
pub type Balance = u64; // Generic balance/amount type
pub type AlphaAmount = u64; // Amount of Alpha tokens in rao
pub type TaoAmount = u64; // Amount of TAO tokens in rao

pub type AlphaListingsMapping = Mapping<(NetUid, AccountId, AlphaListingId), AlphaListing>;
pub type UserListingsMapping = Mapping<(AccountId, NetUid), Vec<AlphaListingId>>;
pub type TaoOffersMapping = Mapping<(NetUid, AccountId, TaoOfferId), TaoOffer>;
pub type UserOffersMapping = Mapping<(AccountId, NetUid), Vec<TaoOfferId>>;

/// Fixed-point decimal representation with 64 bits integer, 64 bits fraction
/// Used for prices (TAO per Alpha ratios) and rates (fee percentages)
/// Uses U64F64 to handle large rao values (1 TAO = 10^9 rao) and precise percentages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
#[derive(Default)]
pub struct FixedDecimal {
    value: u128, // Store U64F64 as raw bits for SCALE codec compatibility
}

impl FixedDecimal {
    pub fn multiply_u64(&self, amount: Balance) -> Result<Balance, Error> {
        let fixed = U64F64::from_bits(self.value);
        let amount_fixed = U64F64::from_num(amount);
        let result = fixed.checked_mul(amount_fixed).ok_or(Error::Overflow)?;

        if result > U64F64::from_num(u64::MAX) {
            return Err(Error::Overflow);
        }

        Ok(result.to_num::<u64>())
    }

    pub fn from_bits(bits: u128) -> Self {
        FixedDecimal { value: bits }
    }

    pub fn to_bits(&self) -> u128 {
        self.value
    }

    pub fn gt(&self, other: &FixedDecimal) -> bool {
        U64F64::from_bits(self.value) > U64F64::from_bits(other.value)
    }

    pub fn gte(&self, other: &FixedDecimal) -> bool {
        U64F64::from_bits(self.value) >= U64F64::from_bits(other.value)
    }

    pub fn lt(&self, other: &FixedDecimal) -> bool {
        U64F64::from_bits(self.value) < U64F64::from_bits(other.value)
    }

    pub fn lte(&self, other: &FixedDecimal) -> bool {
        U64F64::from_bits(self.value) <= U64F64::from_bits(other.value)
    }
}

/// Alpha listing structure
#[derive(Debug, Clone, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
pub struct AlphaListing {
    pub id: AlphaListingId,
    pub netuid: NetUid,
    pub seller: AccountId,
    pub amount: AlphaAmount,    // Alpha amount in rao
    pub price: FixedDecimal,    // TAO price per Alpha token
    pub fee_rate: FixedDecimal, // Fee rate at listing time (as decimal, e.g., 0.005 = 0.5%)
    pub created_at: BlockNumber,
}

/// TAO offer structure
#[derive(Debug, Clone, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
pub struct TaoOffer {
    pub id: TaoOfferId,
    pub netuid: NetUid,
    pub buyer: AccountId,
    pub amount: TaoAmount,      // TAO amount offered in rao
    pub price: FixedDecimal,    // TAO price willing to pay per Alpha
    pub fee_rate: FixedDecimal, // Fee rate at offer time (as decimal, e.g., 0.005 = 0.5%)
    pub created_at: BlockNumber,
}
