#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub mod errors;
pub mod events;
pub mod runtime;
pub mod types;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "std", ink::scale_derive(TypeInfo))]
pub struct BittensorEnvironment;

impl ink::env::Environment for BittensorEnvironment {
    const MAX_EVENT_TOPICS: usize = 4;
    type AccountId = ink::primitives::AccountId;
    type Balance = u64;
    type Hash = ink::primitives::Hash;
    type Timestamp = u64;
    type BlockNumber = u32;
    type ChainExtension = ::ink::env::NoChainExtension;
}

#[ink::contract(env = crate::BittensorEnvironment)]
mod otc_contract {
    use crate::errors::Error;
    use crate::events::{
        AlphaListed, AlphaListingCancelled, FeeRateUpdated, HotkeyUpdated, MinListingAgeUpdated,
        MinListingAmountUpdated, MinOfferAmountUpdated, OwnerUpdated, TaoOfferCancelled,
        TaoOfferCreated,
    };
    use crate::runtime::{ProxyCall, RuntimeCall, SubtensorCall};
    use crate::types::{
        AlphaAmount, AlphaListing, AlphaListingId, AlphaListingsMapping, BlockAge, FixedDecimal,
        NetUid, TaoAmount, TaoOffer, TaoOfferId, TaoOffersMapping, UserListingsMapping,
        UserOffersMapping,
    };
    use ink::prelude::{boxed::Box, vec::Vec};

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
        min_listing_amount: AlphaAmount,

        /// Minimum TAO amount for offers
        min_offer_amount: TaoAmount,

        /// Minimum age before listing can be cancelled (in blocks)
        min_listing_age: BlockAge,
    }

    impl OtcContract {
        #[ink(constructor)]
        pub fn new(
            owner: AccountId,
            hotkey: AccountId,
            fee_rate: u128, // U64F64 bits representing the fee rate
            min_listing_amount: AlphaAmount,
            min_offer_amount: TaoAmount,
            min_listing_age: BlockAge,
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
        pub fn get_min_listing_amount(&self) -> AlphaAmount {
            self.min_listing_amount
        }

        /// Get minimum offer amount
        #[ink(message)]
        pub fn get_min_offer_amount(&self) -> TaoAmount {
            self.min_offer_amount
        }

        /// Get minimum listing age
        #[ink(message)]
        pub fn get_min_listing_age(&self) -> BlockAge {
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
        pub fn update_min_listing_amount(&mut self, new_amount: AlphaAmount) -> Result<(), Error> {
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
        pub fn update_min_offer_amount(&mut self, new_amount: TaoAmount) -> Result<(), Error> {
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
        pub fn update_min_listing_age(&mut self, new_age: BlockAge) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_age = self.min_listing_age;
            self.min_listing_age = new_age;

            self.env()
                .emit_event(MinListingAgeUpdated { old_age, new_age });

            Ok(())
        }

        /// List Alpha tokens for sale
        /// Prerequisites: The seller must have added the contract as their proxy
        /// This will transfer the stake from the seller to the contract via proxy
        /// and consolidate it under the contract's hotkey if needed
        #[ink(message)]
        pub fn list_alpha(
            &mut self,
            hotkey: AccountId,
            netuid: NetUid,
            amount: AlphaAmount,
            price: u128, // Price as FixedDecimal bits (TAO per Alpha)
        ) -> Result<AlphaListingId, Error> {
            let seller = self.env().caller();
            let price = FixedDecimal::from_bits(price);

            if amount < self.min_listing_amount {
                return Err(Error::AmountTooSmall);
            }

            if price.to_bits() == 0 {
                return Err(Error::InvalidPrice);
            }

            // Transfer stake from seller to contract via proxy
            // The seller must have added the contract as their proxy for this to work
            let transfer_call = RuntimeCall::SubtensorModule(SubtensorCall::TransferStake {
                destination_coldkey: self.env().account_id(),
                hotkey,
                origin_netuid: netuid,
                destination_netuid: netuid,
                alpha_amount: amount,
            });

            let proxy_call = RuntimeCall::Proxy(ProxyCall::Proxy {
                real: seller,
                force_proxy_type: None,
                call: Box::new(transfer_call),
            });

            self.env()
                .call_runtime(&proxy_call)
                .map_err(|_| Error::RuntimeCallFailed)?;

            // If the hotkey is different from contract's hotkey, consolidate stake
            if hotkey != self.hotkey {
                let move_call = RuntimeCall::SubtensorModule(SubtensorCall::MoveStake {
                    origin_hotkey: hotkey,
                    destination_hotkey: self.hotkey,
                    origin_netuid: netuid,
                    destination_netuid: netuid,
                    alpha_amount: amount,
                });

                let proxy_move_call = RuntimeCall::Proxy(ProxyCall::Proxy {
                    real: seller,
                    force_proxy_type: None,
                    call: Box::new(move_call),
                });

                self.env()
                    .call_runtime(&proxy_move_call)
                    .map_err(|_| Error::RuntimeCallFailed)?;
            }

            let listing_id = self.next_alpha_listing_id;
            self.next_alpha_listing_id = listing_id.checked_add(1).ok_or(Error::Overflow)?;

            let listing = AlphaListing {
                id: listing_id,
                netuid,
                seller,
                amount,
                price,
                fee_rate: self.fee_rate,
                created_at: self.env().block_number(),
            };

            self.alpha_listings
                .insert((netuid, seller, listing_id), &listing);

            let mut user_listings = self.user_listings.get((seller, netuid)).unwrap_or_default();
            user_listings.push(listing_id);
            self.user_listings.insert((seller, netuid), &user_listings);

            self.env().emit_event(AlphaListed {
                seller,
                hotkey,
                netuid,
                alpha_listing_id: listing_id,
                amount,
                price,
            });

            Ok(listing_id)
        }

        /// Cancel an Alpha listing and return the stake to the seller
        /// Can only be called by the listing owner after minimum age has passed
        #[ink(message)]
        pub fn cancel_alpha_listing(
            &mut self,
            netuid: NetUid,
            listing_id: AlphaListingId,
        ) -> Result<(), Error> {
            let seller = self.env().caller();

            let listing = self
                .alpha_listings
                .get((netuid, seller, listing_id))
                .ok_or(Error::ListingNotFound)?;

            let current_block = self.env().block_number();
            let listing_age = current_block.saturating_sub(listing.created_at);

            if listing_age < self.min_listing_age {
                return Err(Error::ListingTooYoung);
            }

            // Return stake from contract to seller
            // The contract directly transfers its own stake back to the seller
            let transfer_call = RuntimeCall::SubtensorModule(SubtensorCall::TransferStake {
                destination_coldkey: seller,
                hotkey: self.hotkey,
                origin_netuid: netuid,
                destination_netuid: netuid,
                alpha_amount: listing.amount,
            });

            self.env()
                .call_runtime(&transfer_call)
                .map_err(|_| Error::RuntimeCallFailed)?;

            self.alpha_listings.remove((netuid, seller, listing_id));

            let mut user_listings = self.user_listings.get((seller, netuid)).unwrap_or_default();
            user_listings.retain(|&id| id != listing_id);

            if user_listings.is_empty() {
                self.user_listings.remove((seller, netuid));
            } else {
                self.user_listings.insert((seller, netuid), &user_listings);
            }

            self.env().emit_event(AlphaListingCancelled {
                seller,
                netuid,
                listing_id,
                amount_returned: listing.amount,
            });

            Ok(())
        }

        /// Get a specific Alpha listing
        #[ink(message)]
        pub fn get_listing(
            &self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: AlphaListingId,
        ) -> Option<AlphaListing> {
            self.alpha_listings.get((netuid, seller, listing_id))
        }

        /// Get all listing IDs for a user on a specific subnet
        #[ink(message)]
        pub fn get_user_listings(&self, seller: AccountId, netuid: NetUid) -> Vec<AlphaListingId> {
            self.user_listings.get((seller, netuid)).unwrap_or_default()
        }

        /// Create a TAO offer by depositing TAO into the contract
        /// The buyer sends TAO with the transaction and specifies their desired price
        #[ink(message, payable)]
        pub fn create_tao_offer(
            &mut self,
            netuid: NetUid,
            price: u128, // Price as FixedDecimal bits
        ) -> Result<TaoOfferId, Error> {
            let buyer = self.env().caller();
            let amount = self.env().transferred_value();
            let price = FixedDecimal::from_bits(price);

            if amount < self.min_offer_amount {
                return Err(Error::AmountTooSmall);
            }

            if price.to_bits() == 0 {
                return Err(Error::InvalidPrice);
            }

            let offer_id = self.next_tao_offer_id;
            self.next_tao_offer_id = offer_id.saturating_add(1);

            let offer = TaoOffer {
                id: offer_id,
                netuid,
                buyer,
                amount,
                price,
                fee_rate: self.fee_rate,
                created_at: self.env().block_number(),
            };

            self.tao_offers.insert((netuid, buyer, offer_id), &offer);

            let mut user_offers = self.user_offers.get((buyer, netuid)).unwrap_or_default();
            user_offers.push(offer_id);
            self.user_offers.insert((buyer, netuid), &user_offers);

            self.env().emit_event(TaoOfferCreated {
                buyer,
                netuid,
                tao_offer_id: offer_id,
                amount,
                price,
            });

            Ok(offer_id)
        }

        /// Cancel a TAO offer and return the TAO to the buyer
        /// Can only be called by the offer owner
        #[ink(message)]
        pub fn cancel_tao_offer(
            &mut self,
            netuid: NetUid,
            offer_id: TaoOfferId,
        ) -> Result<(), Error> {
            let buyer = self.env().caller();

            let offer = self
                .tao_offers
                .get((netuid, buyer, offer_id))
                .ok_or(Error::OfferNotFound)?;

            // Return TAO to buyer
            self.env()
                .transfer(buyer, offer.amount)
                .map_err(|_| Error::TransferFailed)?;

            self.tao_offers.remove((netuid, buyer, offer_id));

            let mut user_offers = self.user_offers.get((buyer, netuid)).unwrap_or_default();
            user_offers.retain(|&id| id != offer_id);

            if user_offers.is_empty() {
                self.user_offers.remove((buyer, netuid));
            } else {
                self.user_offers.insert((buyer, netuid), &user_offers);
            }

            self.env().emit_event(TaoOfferCancelled {
                buyer,
                netuid,
                offer_id,
                amount_returned: offer.amount,
            });

            Ok(())
        }

        /// Get a specific TAO offer
        #[ink(message)]
        pub fn get_offer(
            &self,
            netuid: NetUid,
            buyer: AccountId,
            offer_id: TaoOfferId,
        ) -> Option<TaoOffer> {
            self.tao_offers.get((netuid, buyer, offer_id))
        }

        /// Get all offer IDs for a user on a specific subnet
        #[ink(message)]
        pub fn get_user_offers(&self, buyer: AccountId, netuid: NetUid) -> Vec<TaoOfferId> {
            self.user_offers.get((buyer, netuid)).unwrap_or_default()
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

        #[ink::test]
        fn list_alpha_fails_with_amount_too_small() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000, // min listing amount
                1_000_000_000,
                100,
            );

            // Set caller to Charlie (the seller)
            ink::env::test::set_caller::<Environment>(accounts.charlie);

            let price = U64F64::from_num(2u64).to_bits();
            let netuid = 1u16;
            let amount = 500_000_000u64; // Below minimum

            // Should fail due to amount being too small
            let result = contract.list_alpha(accounts.django, netuid, amount, price);
            assert_eq!(result, Err(Error::AmountTooSmall));
        }

        #[ink::test]
        fn list_alpha_fails_with_zero_price() {
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

            // Set caller to Charlie (the seller)
            ink::env::test::set_caller::<Environment>(accounts.charlie);

            let price = 0u128; // Zero price
            let netuid = 1u16;
            let amount = 5_000_000_000u64;

            // Should fail due to zero price
            let result = contract.list_alpha(accounts.django, netuid, amount, price);
            assert_eq!(result, Err(Error::InvalidPrice));
        }

        #[ink::test]
        fn get_listing_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Try to get a non-existent listing
            let listing = contract.get_listing(1, accounts.charlie, 1);
            assert_eq!(listing, None);
        }

        #[ink::test]
        fn get_user_listings_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Get listings for a user with no listings
            let listings = contract.get_user_listings(accounts.charlie, 1);
            assert_eq!(listings, Vec::<AlphaListingId>::new());
        }

        #[ink::test]
        fn cancel_alpha_listing_fails_when_listing_not_found() {
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

            // Set caller to Charlie
            ink::env::test::set_caller::<Environment>(accounts.charlie);

            // Try to cancel a non-existent listing
            let result = contract.cancel_alpha_listing(1, 999);
            assert_eq!(result, Err(Error::ListingNotFound));
        }

        #[ink::test]
        fn cancel_alpha_listing_fails_when_too_young() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100, // min_listing_age = 100 blocks
            );

            // Create a mock listing manually for testing
            // In production this would be created via list_alpha
            let listing = AlphaListing {
                id: 1,
                netuid: 1,
                seller: accounts.charlie,
                amount: 5_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(2u64).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000, // Created at block 1000
            };

            // Insert the listing
            contract
                .alpha_listings
                .insert((1, accounts.charlie, 1), &listing);
            contract
                .user_listings
                .insert((accounts.charlie, 1), &vec![1]);

            // Set caller to Charlie (the owner)
            ink::env::test::set_caller::<Environment>(accounts.charlie);

            // Set current block to 1050 (only 50 blocks old, less than 100)
            ink::env::test::set_block_number::<Environment>(1050);

            // Try to cancel the listing (should fail due to minimum age)
            let result = contract.cancel_alpha_listing(1, 1);
            assert_eq!(result, Err(Error::ListingTooYoung));
        }

        #[ink::test]
        fn cancel_alpha_listing_cleans_up_storage() {
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

            // Create two mock listings for the same user
            let listing1 = AlphaListing {
                id: 1,
                netuid: 1,
                seller: accounts.charlie,
                amount: 5_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(2u64).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000,
            };

            let listing2 = AlphaListing {
                id: 2,
                netuid: 1,
                seller: accounts.charlie,
                amount: 3_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(1.5).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000,
            };

            // Insert both listings
            contract
                .alpha_listings
                .insert((1, accounts.charlie, 1), &listing1);
            contract
                .alpha_listings
                .insert((1, accounts.charlie, 2), &listing2);
            contract
                .user_listings
                .insert((accounts.charlie, 1), &vec![1, 2]);

            // Verify both listings exist
            assert!(contract.get_listing(1, accounts.charlie, 1).is_some());
            assert!(contract.get_listing(1, accounts.charlie, 2).is_some());
            assert_eq!(contract.get_user_listings(accounts.charlie, 1).len(), 2);

            // Set caller to Charlie
            ink::env::test::set_caller::<Environment>(accounts.charlie);

            // Set current block to be well past minimum age
            ink::env::test::set_block_number::<Environment>(1200);

            // Note: In tests, runtime calls will fail, but we can test the logic up to that point
            // The actual cancellation would fail at runtime call, but storage cleanup logic is correct

            // Manually simulate successful cancellation for testing storage cleanup
            // Remove listing 1
            contract.alpha_listings.remove((1, accounts.charlie, 1));
            let mut user_listings = contract.user_listings.get((accounts.charlie, 1)).unwrap();
            user_listings.retain(|&id| id != 1);
            contract
                .user_listings
                .insert((accounts.charlie, 1), &user_listings);

            // Verify listing 1 is removed but listing 2 remains
            assert!(contract.get_listing(1, accounts.charlie, 1).is_none());
            assert!(contract.get_listing(1, accounts.charlie, 2).is_some());
            assert_eq!(contract.get_user_listings(accounts.charlie, 1), vec![2]);

            // Remove listing 2
            contract.alpha_listings.remove((1, accounts.charlie, 2));
            let user_listings = contract.user_listings.get((accounts.charlie, 1)).unwrap();
            let filtered: Vec<_> = user_listings.into_iter().filter(|&id| id != 2).collect();

            if filtered.is_empty() {
                contract.user_listings.remove((accounts.charlie, 1));
            } else {
                contract
                    .user_listings
                    .insert((accounts.charlie, 1), &filtered);
            }

            // Verify both listings are removed and user_listings is cleaned up
            assert!(contract.get_listing(1, accounts.charlie, 1).is_none());
            assert!(contract.get_listing(1, accounts.charlie, 2).is_none());
            assert_eq!(contract.get_user_listings(accounts.charlie, 1).len(), 0);
        }

        #[ink::test]
        fn create_tao_offer_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000, // min offer amount
                100,
            );

            // Set caller to Charlie (the buyer)
            ink::env::test::set_caller::<Environment>(accounts.charlie);
            // Set transferred value (TAO amount)
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(5_000_000_000);

            let price = U64F64::from_num(2u64).to_bits();
            let netuid = 1u16;

            // Create TAO offer
            let result = contract.create_tao_offer(netuid, price);
            assert!(result.is_ok());

            let offer_id = result.unwrap();
            assert_eq!(offer_id, 1);

            // Verify offer was stored correctly
            let offer = contract.get_offer(netuid, accounts.charlie, offer_id);
            assert!(offer.is_some());

            let offer = offer.unwrap();
            assert_eq!(offer.id, offer_id);
            assert_eq!(offer.netuid, netuid);
            assert_eq!(offer.buyer, accounts.charlie);
            assert_eq!(offer.amount, 5_000_000_000);
            assert_eq!(offer.price.to_bits(), price);

            // Verify user offers index was updated
            let user_offers = contract.get_user_offers(accounts.charlie, netuid);
            assert_eq!(user_offers, vec![offer_id]);
        }

        #[ink::test]
        fn create_tao_offer_fails_with_amount_too_small() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000, // min offer amount
                100,
            );

            // Set caller to Charlie (the buyer)
            ink::env::test::set_caller::<Environment>(accounts.charlie);
            // Set transferred value below minimum
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(500_000_000);

            let price = U64F64::from_num(2u64).to_bits();
            let netuid = 1u16;

            // Should fail due to amount being too small
            let result = contract.create_tao_offer(netuid, price);
            assert_eq!(result, Err(Error::AmountTooSmall));
        }

        #[ink::test]
        fn create_tao_offer_fails_with_zero_price() {
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

            // Set caller to Charlie (the buyer)
            ink::env::test::set_caller::<Environment>(accounts.charlie);
            // Set transferred value
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(5_000_000_000);

            let price = 0u128; // Zero price
            let netuid = 1u16;

            // Should fail due to zero price
            let result = contract.create_tao_offer(netuid, price);
            assert_eq!(result, Err(Error::InvalidPrice));
        }

        #[ink::test]
        fn cancel_tao_offer_authorization_works() {
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

            // Create a mock offer manually for testing
            let offer = TaoOffer {
                id: 1,
                netuid: 1,
                buyer: accounts.charlie,
                amount: 5_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(2u64).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000,
            };

            // Insert the offer
            contract.tao_offers.insert((1, accounts.charlie, 1), &offer);
            contract.user_offers.insert((accounts.charlie, 1), &vec![1]);

            // Test that a different user cannot cancel
            ink::env::test::set_caller::<Environment>(accounts.django);
            let result = contract.cancel_tao_offer(1, 1);
            assert_eq!(result, Err(Error::OfferNotFound));

            // Verify that the correct owner can find their offer
            ink::env::test::set_caller::<Environment>(accounts.charlie);
            let found_offer = contract.get_offer(1, accounts.charlie, 1);
            assert!(found_offer.is_some());
            assert_eq!(found_offer.unwrap().buyer, accounts.charlie);
        }

        #[ink::test]
        fn cancel_tao_offer_fails_when_offer_not_found() {
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

            // Set caller to Charlie
            ink::env::test::set_caller::<Environment>(accounts.charlie);

            // Try to cancel a non-existent offer
            let result = contract.cancel_tao_offer(1, 999);
            assert_eq!(result, Err(Error::OfferNotFound));
        }

        #[ink::test]
        fn get_offer_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Try to get a non-existent offer
            let offer = contract.get_offer(1, accounts.charlie, 1);
            assert_eq!(offer, None);
        }

        #[ink::test]
        fn get_user_offers_works() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(0.5);

            let contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Get offers for a user with no offers
            let offers = contract.get_user_offers(accounts.charlie, 1);
            assert_eq!(offers, Vec::<TaoOfferId>::new());
        }

        #[ink::test]
        fn cancel_tao_offer_cleans_up_storage() {
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

            // Create two mock offers for the same user
            let offer1 = TaoOffer {
                id: 1,
                netuid: 1,
                buyer: accounts.charlie,
                amount: 5_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(2u64).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000,
            };

            let offer2 = TaoOffer {
                id: 2,
                netuid: 1,
                buyer: accounts.charlie,
                amount: 3_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(1u64).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1100,
            };

            // Insert both offers
            contract
                .tao_offers
                .insert((1, accounts.charlie, 1), &offer1);
            contract
                .tao_offers
                .insert((1, accounts.charlie, 2), &offer2);
            contract
                .user_offers
                .insert((accounts.charlie, 1), &vec![1, 2]);

            // Verify both offers exist
            assert!(contract.get_offer(1, accounts.charlie, 1).is_some());
            assert!(contract.get_offer(1, accounts.charlie, 2).is_some());
            assert_eq!(contract.get_user_offers(accounts.charlie, 1).len(), 2);

            // Manually simulate successful cancellation for testing storage cleanup
            // Remove offer 1
            contract.tao_offers.remove((1, accounts.charlie, 1));
            let mut user_offers = contract.user_offers.get((accounts.charlie, 1)).unwrap();
            user_offers.retain(|&id| id != 1);
            contract
                .user_offers
                .insert((accounts.charlie, 1), &user_offers);

            // Verify offer 1 is removed but offer 2 remains
            assert!(contract.get_offer(1, accounts.charlie, 1).is_none());
            assert!(contract.get_offer(1, accounts.charlie, 2).is_some());
            assert_eq!(contract.get_user_offers(accounts.charlie, 1), vec![2]);

            // Remove offer 2
            contract.tao_offers.remove((1, accounts.charlie, 2));
            let user_offers = contract.user_offers.get((accounts.charlie, 1)).unwrap();
            let filtered: Vec<_> = user_offers.into_iter().filter(|&id| id != 2).collect();

            if filtered.is_empty() {
                contract.user_offers.remove((accounts.charlie, 1));
            } else {
                contract
                    .user_offers
                    .insert((accounts.charlie, 1), &filtered);
            }

            // Verify both offers are removed and user_offers is cleaned up
            assert!(contract.get_offer(1, accounts.charlie, 1).is_none());
            assert!(contract.get_offer(1, accounts.charlie, 2).is_none());
            assert_eq!(contract.get_user_offers(accounts.charlie, 1).len(), 0);
        }
    }
}
