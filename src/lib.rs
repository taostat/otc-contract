#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub mod errors;
pub mod events;
pub mod types;

#[ink::contract]
mod otc_contract {
    use crate::errors::Error;
    use crate::events::{
        FeeRateUpdated, HotkeyUpdated, MinListingAgeUpdated, MinListingAmountUpdated,
        MinOfferAmountUpdated, OwnerUpdated,
    };
    use crate::types::{
        AlphaListingId, AlphaListingsMapping, FixedDecimal, TaoOfferId, TaoOffersMapping,
        UserListingsMapping, UserOffersMapping,
    };

    #[ink(storage)]
    pub struct OtcContract {
        /// Listings: (netuid, seller, listing_id) -> AlphaListing
        alpha_listings: AlphaListingsMapping,

        /// User's listing IDs for iteration: (seller, netuid) -> Vec<listing_id>
        user_listings: UserListingsMapping,

        /// Offers: (netuid, buyer, offer_id) -> TaoOffer
        tao_offers: TaoOffersMapping,

        /// User's offer IDs for iteration: (buyer, netuid) -> Vec<offer_id>
        user_offers: UserOffersMapping,

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

        /// Update the contract owner
        /// Can only be called by the current owner
        #[ink(message)]
        pub fn update_owner(&mut self, new_owner: AccountId) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_owner = self.owner;
            self.owner = new_owner;

            self.env().emit_event(OwnerUpdated {
                old_owner,
                new_owner,
            });

            Ok(())
        }

        /// Update the validator hotkey
        /// Can only be called by the contract owner
        #[ink(message)]
        pub fn update_hotkey(&mut self, new_hotkey: AccountId) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_hotkey = self.hotkey;
            self.hotkey = new_hotkey;

            self.env().emit_event(HotkeyUpdated {
                old_hotkey,
                new_hotkey,
            });

            Ok(())
        }

        /// Update the fee rate
        /// Can only be called by the contract owner
        #[ink(message)]
        pub fn update_fee_rate(&mut self, new_rate: u128) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_rate = self.fee_rate;
            let new_rate = FixedDecimal::from_bits(new_rate);
            self.fee_rate = new_rate;

            self.env().emit_event(FeeRateUpdated { old_rate, new_rate });

            Ok(())
        }

        /// Update the minimum listing amount
        /// Can only be called by the contract owner
        #[ink(message)]
        pub fn update_min_listing_amount(&mut self, new_amount: u64) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_amount = self.min_listing_amount;
            self.min_listing_amount = new_amount;

            self.env().emit_event(MinListingAmountUpdated {
                old_amount,
                new_amount,
            });

            Ok(())
        }

        /// Update the minimum offer amount
        /// Can only be called by the contract owner
        #[ink(message)]
        pub fn update_min_offer_amount(&mut self, new_amount: u64) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_amount = self.min_offer_amount;
            self.min_offer_amount = new_amount;

            self.env().emit_event(MinOfferAmountUpdated {
                old_amount,
                new_amount,
            });

            Ok(())
        }

        /// Update the minimum listing age
        /// Can only be called by the contract owner
        #[ink(message)]
        pub fn update_min_listing_age(&mut self, new_age: u64) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_age = self.min_listing_age;
            self.min_listing_age = new_age;

            self.env()
                .emit_event(MinListingAgeUpdated { old_age, new_age });

            Ok(())
        }

        /// Access control helper: ensure caller is the owner
        fn ensure_owner(&self) -> Result<(), Error> {
            if self.env().caller() != self.owner {
                return Err(Error::Unauthorized);
            }
            Ok(())
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

        #[ink::test]
        fn update_owner_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Set caller to Alice (the owner)
            ink::env::test::set_caller::<Environment>(accounts.alice);

            // Update owner to Charlie
            assert_eq!(contract.update_owner(accounts.charlie), Ok(()));
            assert_eq!(contract.get_owner(), accounts.charlie);

            // Verify emitted event
            let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
            assert_eq!(emitted_events.len(), 1);

            let decoded_event =
                <OwnerUpdated as ink::scale::Decode>::decode(&mut &emitted_events[0].data[..])
                    .expect("Failed to decode event");
            assert_eq!(decoded_event.old_owner, accounts.alice);
            assert_eq!(decoded_event.new_owner, accounts.charlie);
        }

        #[ink::test]
        fn update_owner_fails_if_not_owner() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Set caller to Bob (not the owner)
            ink::env::test::set_caller::<Environment>(accounts.bob);

            // Try to update owner
            assert_eq!(
                contract.update_owner(accounts.charlie),
                Err(Error::Unauthorized)
            );
            assert_eq!(contract.get_owner(), accounts.alice);
        }

        #[ink::test]
        fn update_hotkey_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Set caller to Alice (the owner)
            ink::env::test::set_caller::<Environment>(accounts.alice);

            // Update hotkey to Charlie
            assert_eq!(contract.update_hotkey(accounts.charlie), Ok(()));
            assert_eq!(contract.get_hotkey(), accounts.charlie);

            // Verify emitted event
            let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
            assert_eq!(emitted_events.len(), 1);

            let decoded_event =
                <HotkeyUpdated as ink::scale::Decode>::decode(&mut &emitted_events[0].data[..])
                    .expect("Failed to decode event");
            assert_eq!(decoded_event.old_hotkey, accounts.bob);
            assert_eq!(decoded_event.new_hotkey, accounts.charlie);
        }

        #[ink::test]
        fn update_hotkey_fails_if_not_owner() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Set caller to Bob (not the owner)
            ink::env::test::set_caller::<Environment>(accounts.bob);

            // Try to update hotkey
            assert_eq!(
                contract.update_hotkey(accounts.charlie),
                Err(Error::Unauthorized)
            );
            assert_eq!(contract.get_hotkey(), accounts.bob);
        }

        #[ink::test]
        fn ownership_transfer_chain_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Alice transfers ownership to Bob
            ink::env::test::set_caller::<Environment>(accounts.alice);
            assert_eq!(contract.update_owner(accounts.bob), Ok(()));

            // Now Bob can perform owner actions
            ink::env::test::set_caller::<Environment>(accounts.bob);
            assert_eq!(contract.update_hotkey(accounts.charlie), Ok(()));

            // Alice can no longer perform owner actions
            ink::env::test::set_caller::<Environment>(accounts.alice);
            assert_eq!(
                contract.update_hotkey(accounts.django),
                Err(Error::Unauthorized)
            );
        }

        #[ink::test]
        fn update_fee_rate_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let initial_fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                initial_fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Set caller to Alice (the owner)
            ink::env::test::set_caller::<Environment>(accounts.alice);

            // Update fee rate to 1%
            let new_fee_rate = fee_rate_from_percentage(1.0);
            assert_eq!(contract.update_fee_rate(new_fee_rate), Ok(()));
            assert_eq!(contract.get_fee_rate().to_bits(), new_fee_rate);

            // Verify emitted event
            let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
            assert_eq!(emitted_events.len(), 1);

            let decoded_event =
                <FeeRateUpdated as ink::scale::Decode>::decode(&mut &emitted_events[0].data[..])
                    .expect("Failed to decode event");
            assert_eq!(decoded_event.old_rate.to_bits(), initial_fee_rate);
            assert_eq!(decoded_event.new_rate.to_bits(), new_fee_rate);
        }

        #[ink::test]
        fn update_fee_rate_fails_if_not_owner() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Set caller to Bob (not the owner)
            ink::env::test::set_caller::<Environment>(accounts.bob);

            let new_fee_rate = fee_rate_from_percentage(1.0);
            assert_eq!(
                contract.update_fee_rate(new_fee_rate),
                Err(Error::Unauthorized)
            );
            assert_eq!(contract.get_fee_rate().to_bits(), fee_rate);
        }

        #[ink::test]
        fn update_min_listing_amount_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Set caller to Alice (the owner)
            ink::env::test::set_caller::<Environment>(accounts.alice);

            // Update minimum listing amount
            let new_amount = 2_000_000_000;
            assert_eq!(contract.update_min_listing_amount(new_amount), Ok(()));
            assert_eq!(contract.get_min_listing_amount(), new_amount);

            // Verify emitted event
            let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
            assert_eq!(emitted_events.len(), 1);

            let decoded_event = <MinListingAmountUpdated as ink::scale::Decode>::decode(
                &mut &emitted_events[0].data[..],
            )
            .expect("Failed to decode event");
            assert_eq!(decoded_event.old_amount, 1_000_000_000);
            assert_eq!(decoded_event.new_amount, new_amount);
        }

        #[ink::test]
        fn update_min_listing_amount_fails_if_not_owner() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Set caller to Bob (not the owner)
            ink::env::test::set_caller::<Environment>(accounts.bob);

            assert_eq!(
                contract.update_min_listing_amount(2_000_000_000),
                Err(Error::Unauthorized)
            );
            assert_eq!(contract.get_min_listing_amount(), 1_000_000_000);
        }

        #[ink::test]
        fn update_min_offer_amount_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Set caller to Alice (the owner)
            ink::env::test::set_caller::<Environment>(accounts.alice);

            // Update minimum offer amount
            let new_amount = 500_000_000;
            assert_eq!(contract.update_min_offer_amount(new_amount), Ok(()));
            assert_eq!(contract.get_min_offer_amount(), new_amount);

            // Verify emitted event
            let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
            assert_eq!(emitted_events.len(), 1);

            let decoded_event = <MinOfferAmountUpdated as ink::scale::Decode>::decode(
                &mut &emitted_events[0].data[..],
            )
            .expect("Failed to decode event");
            assert_eq!(decoded_event.old_amount, 1_000_000_000);
            assert_eq!(decoded_event.new_amount, new_amount);
        }

        #[ink::test]
        fn update_min_offer_amount_fails_if_not_owner() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Set caller to Bob (not the owner)
            ink::env::test::set_caller::<Environment>(accounts.bob);

            assert_eq!(
                contract.update_min_offer_amount(500_000_000),
                Err(Error::Unauthorized)
            );
            assert_eq!(contract.get_min_offer_amount(), 1_000_000_000);
        }

        #[ink::test]
        fn update_min_listing_age_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Set caller to Alice (the owner)
            ink::env::test::set_caller::<Environment>(accounts.alice);

            // Update minimum listing age
            let new_age = 200;
            assert_eq!(contract.update_min_listing_age(new_age), Ok(()));
            assert_eq!(contract.get_min_listing_age(), new_age);

            // Verify emitted event
            let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
            assert_eq!(emitted_events.len(), 1);

            let decoded_event = <MinListingAgeUpdated as ink::scale::Decode>::decode(
                &mut &emitted_events[0].data[..],
            )
            .expect("Failed to decode event");
            assert_eq!(decoded_event.old_age, 100);
            assert_eq!(decoded_event.new_age, new_age);
        }

        #[ink::test]
        fn update_min_listing_age_fails_if_not_owner() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Set caller to Bob (not the owner)
            ink::env::test::set_caller::<Environment>(accounts.bob);

            assert_eq!(
                contract.update_min_listing_age(200),
                Err(Error::Unauthorized)
            );
            assert_eq!(contract.get_min_listing_age(), 100);
        }

        #[ink::test]
        fn update_multiple_configurations_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let initial_fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                initial_fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Set caller to Alice (the owner)
            ink::env::test::set_caller::<Environment>(accounts.alice);

            // Update multiple configurations
            assert_eq!(
                contract.update_fee_rate(fee_rate_from_percentage(0.75)),
                Ok(())
            );
            assert_eq!(contract.update_min_listing_amount(2_000_000_000), Ok(()));
            assert_eq!(contract.update_min_offer_amount(500_000_000), Ok(()));
            assert_eq!(contract.update_min_listing_age(150), Ok(()));

            // Verify all values were updated
            assert_eq!(
                contract.get_fee_rate().to_bits(),
                fee_rate_from_percentage(0.75)
            );
            assert_eq!(contract.get_min_listing_amount(), 2_000_000_000);
            assert_eq!(contract.get_min_offer_amount(), 500_000_000);
            assert_eq!(contract.get_min_listing_age(), 150);

            // Verify 4 events were emitted
            let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
            assert_eq!(emitted_events.len(), 4);
        }
    }
}
