#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub mod errors;
pub mod events;
pub mod types;

#[ink::contract]
mod otc_contract {
    use crate::types::{AlphaListing, AlphaListingId, FixedDecimal, NetUid, TaoOffer, TaoOfferId};
    use ink::prelude::vec::Vec;
    use ink::storage::Mapping;

    #[ink(storage)]
    pub struct OtcContract {
        /// Listings: (netuid, seller, listing_id) -> AlphaListing
        alpha_listings: Mapping<(NetUid, AccountId, AlphaListingId), AlphaListing>,

        /// User's listing IDs for iteration: (seller, netuid) -> Vec<listing_id>
        user_listings: Mapping<(AccountId, NetUid), Vec<AlphaListingId>>,

        /// Offers: (netuid, buyer, offer_id) -> TaoOffer
        tao_offers: Mapping<(NetUid, AccountId, TaoOfferId), TaoOffer>,

        /// User's offer IDs for iteration: (buyer, netuid) -> Vec<offer_id>
        user_offers: Mapping<(AccountId, NetUid), Vec<TaoOfferId>>,

        /// Global listing counter
        next_alpha_listing_id: AlphaListingId,

        /// Global offer counter
        next_tao_offer_id: TaoOfferId,

        /// Contract owner
        owner: AccountId,

        /// Validator hotkey
        hotkey: AccountId,

        /// Fee rate charged on trades (as decimal, e.g., 0.005 = 0.5%)
        fee_rate: FixedDecimal,

        /// Minimum Alpha amount for listings
        min_listing_amount: u64,

        /// Minimum TAO amount for offers
        min_offer_amount: u64,

        /// Minimum age before listing can be cancelled (in blocks)
        min_listing_age: u64,
    }

    impl OtcContract {
        #[ink(constructor)]
        pub fn new(
            owner: AccountId,
            hotkey: AccountId,
            fee_rate: u128, // U64F64 bits representing the fee rate
            min_listing_amount: u64,
            min_offer_amount: u64,
            min_listing_age: u64,
        ) -> Self {
            let fee_rate = FixedDecimal::from_bits(fee_rate);

            Self {
                alpha_listings: Default::default(),
                user_listings: Default::default(),
                tao_offers: Default::default(),
                user_offers: Default::default(),
                next_alpha_listing_id: 1,
                next_tao_offer_id: 1,
                owner,
                hotkey,
                fee_rate,
                min_listing_amount,
                min_offer_amount,
                min_listing_age,
            }
        }

        /// Get the contract owner
        #[ink(message)]
        pub fn get_owner(&self) -> AccountId {
            self.owner
        }

        /// Get the validator hotkey
        #[ink(message)]
        pub fn get_hotkey(&self) -> AccountId {
            self.hotkey
        }

        /// Get the fee rate
        #[ink(message)]
        pub fn get_fee_rate(&self) -> FixedDecimal {
            self.fee_rate
        }

        /// Get minimum listing amount
        #[ink(message)]
        pub fn get_min_listing_amount(&self) -> u64 {
            self.min_listing_amount
        }

        /// Get minimum offer amount
        #[ink(message)]
        pub fn get_min_offer_amount(&self) -> u64 {
            self.min_offer_amount
        }

        /// Get minimum listing age
        #[ink(message)]
        pub fn get_min_listing_age(&self) -> u64 {
            self.min_listing_age
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use fixed::types::U64F64;

        /// Helper function to create fee rate bits from percentage
        fn fee_rate_from_percentage(percentage: f64) -> u128 {
            let fee_as_decimal = percentage / 100.0;
            let fixed = U64F64::from_num(fee_as_decimal);
            fixed.to_bits()
        }

        #[ink::test]
        fn constructor_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();

            // Test with custom parameters (0.5% fee)
            let fee_rate = fee_rate_from_percentage(0.5);

            let contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            assert_eq!(contract.get_owner(), accounts.alice);
            assert_eq!(contract.get_hotkey(), accounts.bob);
            assert_eq!(contract.get_min_listing_amount(), 1_000_000_000);
            assert_eq!(contract.get_min_offer_amount(), 1_000_000_000);
            assert_eq!(contract.get_min_listing_age(), 100);
        }
    }
}
