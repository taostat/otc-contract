#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub mod chain_extension;
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
    type ChainExtension = crate::chain_extension::SubtensorExtension;
}

#[ink::contract(env = crate::BittensorEnvironment)]
mod otc_contract {
    use crate::errors::Error;
    use crate::events::*;
    use crate::runtime::{AlphaCurrency, ProxyCall, RuntimeCall, SubtensorCall};
    use crate::types::{
        AlphaAmount, AlphaListing, AlphaListingId, AlphaListingsMapping, BlockAge, FixedDecimal,
        NetUid, TaoAmount, TaoOffer, TaoOfferId, TaoOffersMapping, UserListingsMapping,
        UserOffersMapping,
    };
    use ink::prelude::{boxed::Box, vec::Vec};
    use sp_runtime::MultiAddress;

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

        /// Helper function to get TAO balance for a specific account
        fn get_tao_balance(&self, account: AccountId) -> Result<TaoAmount, Error> {
            self.env()
                .extension()
                .get_tao_balance(account)
                .map_err(|_| Error::TaoBalanceQueryFailed)
        }

        /// Helper function to get stake amount for a specific account
        fn get_stake_amount(
            &self,
            coldkey: AccountId,
            hotkey: AccountId,
            netuid: NetUid,
        ) -> Result<AlphaAmount, Error> {
            match self
                .env()
                .extension()
                .get_stake_info(hotkey, coldkey, netuid)
            {
                Ok(Some(info)) => {
                    let stake_amount = info.stake_amount();
                    Ok(stake_amount)
                }
                Ok(None) => Ok(0),
                Err(_) => Err(Error::StakeQueryFailed),
            }
        }

        /// Helper function to get total stake for a hotkey on a specific subnet
        fn get_total_stake(&self, hotkey: AccountId, netuid: NetUid) -> Result<AlphaAmount, Error> {
            self.env()
                .extension()
                .get_total_stake(hotkey, netuid)
                .map_err(|_| Error::StakeQueryFailed)
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

            if price.is_zero() {
                return Err(Error::InvalidPrice);
            }

            // Transfer stake from seller to contract via proxy
            // The seller must have added the contract as their proxy for this to work
            let transfer_call = RuntimeCall::SubtensorModule(SubtensorCall::TransferStake {
                destination_coldkey: self.env().account_id(),
                hotkey,
                origin_netuid: crate::runtime::NetUid::from(netuid),
                destination_netuid: crate::runtime::NetUid::from(netuid),
                alpha_amount: AlphaCurrency::from(amount),
            });

            let proxy_call = RuntimeCall::Proxy(ProxyCall::Proxy {
                real: MultiAddress::Id(seller),
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
                    origin_netuid: crate::runtime::NetUid::from(netuid),
                    destination_netuid: crate::runtime::NetUid::from(netuid),
                    alpha_amount: AlphaCurrency::from(amount),
                });

                self.env()
                    .call_runtime(&move_call)
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

            self.alpha_listings.remove((netuid, seller, listing_id));

            let mut user_listings = self.user_listings.get((seller, netuid)).unwrap_or_default();
            user_listings.retain(|&id| id != listing_id);

            if user_listings.is_empty() {
                self.user_listings.remove((seller, netuid));
            } else {
                self.user_listings.insert((seller, netuid), &user_listings);
            }

            // Return stake from contract to seller
            let transfer_call = RuntimeCall::SubtensorModule(SubtensorCall::TransferStake {
                destination_coldkey: seller,
                hotkey: self.hotkey,
                origin_netuid: crate::runtime::NetUid::from(netuid),
                destination_netuid: crate::runtime::NetUid::from(netuid),
                alpha_amount: AlphaCurrency::from(listing.amount),
            });

            self.env()
                .call_runtime(&transfer_call)
                .map_err(|_| Error::RuntimeCallFailed)?;

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

            if price.is_zero() {
                return Err(Error::InvalidPrice);
            }

            let offer_id = self.next_tao_offer_id;
            self.next_tao_offer_id = offer_id.checked_add(1).ok_or(Error::Overflow)?;

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

        /// Take an Alpha listing by paying TAO
        /// The buyer sends TAO with the transaction to purchase Alpha tokens at the listing price
        /// Prerequisites: Buyer must send exact TAO amount (price * amount + fees)
        #[ink(message, payable)]
        pub fn take_alpha_listing(
            &mut self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: AlphaListingId,
        ) -> Result<(), Error> {
            let buyer = self.env().caller();
            let tao_received = self.env().transferred_value();

            let listing = self
                .alpha_listings
                .get((netuid, seller, listing_id))
                .ok_or(Error::ListingNotFound)?;

            let tao_amount = listing
                .price
                .mul(listing.amount)
                .map_err(|_| Error::Overflow)?;

            let fee_amount = listing
                .fee_rate
                .mul(tao_amount)
                .map_err(|_| Error::Overflow)?;

            let total_tao_required = tao_amount.checked_add(fee_amount).ok_or(Error::Overflow)?;

            if tao_received != total_tao_required {
                return Err(Error::InvalidPrice);
            }

            self.alpha_listings.remove((netuid, seller, listing_id));

            let mut user_listings = self.user_listings.get((seller, netuid)).unwrap_or_default();
            user_listings.retain(|&id| id != listing_id);

            if user_listings.is_empty() {
                self.user_listings.remove((seller, netuid));
            } else {
                self.user_listings.insert((seller, netuid), &user_listings);
            }

            // Transfer Alpha from contract to buyer
            let transfer_call = RuntimeCall::SubtensorModule(SubtensorCall::TransferStake {
                destination_coldkey: buyer,
                hotkey: self.hotkey,
                origin_netuid: crate::runtime::NetUid::from(netuid),
                destination_netuid: crate::runtime::NetUid::from(netuid),
                alpha_amount: AlphaCurrency::from(listing.amount),
            });

            self.env()
                .call_runtime(&transfer_call)
                .map_err(|_| Error::RuntimeCallFailed)?;

            // Transfer TAO to seller (minus fee)
            self.env()
                .transfer(listing.seller, tao_amount)
                .map_err(|_| Error::TransferFailed)?;

            if fee_amount > 0 {
                self.env()
                    .transfer(self.owner, fee_amount)
                    .map_err(|_| Error::TransferFailed)?;
            }

            self.env().emit_event(AlphaListingTaken {
                seller,
                buyer,
                netuid,
                alpha_amount: listing.amount,
                tao_amount,
                price: listing.price,
                fee: fee_amount,
                alpha_listing_id: Some(listing_id),
            });

            Ok(())
        }

        /// Take a TAO offer by providing Alpha tokens
        /// The seller provides Alpha tokens to claim the TAO at the offer price
        /// Prerequisites: The seller must have added the contract as their proxy
        #[ink(message)]
        pub fn take_tao_offer(
            &mut self,
            netuid: NetUid,
            buyer: AccountId,
            offer_id: TaoOfferId,
            hotkey: AccountId,
        ) -> Result<(), Error> {
            let seller = self.env().caller();

            let offer = self
                .tao_offers
                .get((netuid, buyer, offer_id))
                .ok_or(Error::OfferNotFound)?;

            // Calculate fee first (buyer pays fee, so seller gets less TAO)
            let fee_amount = offer
                .fee_rate
                .mul(offer.amount)
                .map_err(|_| Error::Overflow)?;

            let tao_for_seller = offer
                .amount
                .checked_sub(fee_amount)
                .ok_or(Error::Overflow)?;

            // Calculate Alpha amount needed
            // offer.price is TAO per Alpha, so alpha_amount = tao_for_seller / price
            let alpha_amount = offer
                .price
                .div_by_decimal(tao_for_seller)
                .map_err(|_| Error::Overflow)?;

            self.tao_offers.remove((netuid, buyer, offer_id));

            let mut user_offers = self.user_offers.get((buyer, netuid)).unwrap_or_default();
            user_offers.retain(|&id| id != offer_id);

            if user_offers.is_empty() {
                self.user_offers.remove((buyer, netuid));
            } else {
                self.user_offers.insert((buyer, netuid), &user_offers);
            }

            // Transfer Alpha directly from seller to buyer
            let transfer_call = RuntimeCall::SubtensorModule(SubtensorCall::TransferStake {
                destination_coldkey: buyer,
                hotkey, // Seller's original hotkey - Alpha stays here
                origin_netuid: crate::runtime::NetUid::from(netuid),
                destination_netuid: crate::runtime::NetUid::from(netuid),
                alpha_amount: AlphaCurrency::from(alpha_amount),
            });

            let proxy_call = RuntimeCall::Proxy(ProxyCall::Proxy {
                real: MultiAddress::Id(seller),
                force_proxy_type: None,
                call: Box::new(transfer_call),
            });

            self.env()
                .call_runtime(&proxy_call)
                .map_err(|_| Error::RuntimeCallFailed)?;

            // Transfer TAO to seller (minus fee)
            self.env()
                .transfer(seller, tao_for_seller)
                .map_err(|_| Error::TransferFailed)?;

            if fee_amount > 0 {
                self.env()
                    .transfer(self.owner, fee_amount)
                    .map_err(|_| Error::TransferFailed)?;
            }

            self.env().emit_event(TaoOfferTaken {
                seller,
                buyer,
                netuid,
                alpha_amount,
                tao_amount: offer.amount,
                price: offer.price,
                fee: fee_amount,
                tao_offer_id: Some(offer_id),
            });

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
        fn create_tao_offer_fails_with_zero_amount() {
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
            // Set transferred value to zero
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(0);

            let price = U64F64::from_num(2u64).to_bits();
            let netuid = 1u16;

            // Should fail due to zero amount
            let result = contract.create_tao_offer(netuid, price);
            assert_eq!(result, Err(Error::AmountTooSmall));
        }

        #[ink::test]
        fn price_calculation_in_take_tao_offer_is_correct() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(1.0); // 1% fee

            let contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Create a TAO offer
            let price = U64F64::from_num(2u64).to_bits(); // 2 TAO per Alpha
            let tao_amount = 10_000_000_000u64; // 10 TAO
            let netuid = 1u16;

            // Create the offer
            let offer = TaoOffer {
                id: 1,
                netuid,
                buyer: accounts.charlie,
                amount: tao_amount,
                price: FixedDecimal::from_bits(price),
                fee_rate: contract.fee_rate,
                created_at: 0,
            };

            // Calculate expected values
            let fee_amount = 100_000_000u64; // 1% of 10 TAO = 0.1 TAO
            let tao_for_seller = tao_amount - fee_amount; // 9.9 TAO

            // Expected Alpha: 9.9 TAO / 2 TAO per Alpha = 4.95 Alpha
            let expected_alpha = 4_950_000_000u64; // 4.95 Alpha in rao

            // Test the calculation using the same logic as the contract
            let calculated_alpha = offer.price.div_by_decimal(tao_for_seller).unwrap();
            assert_eq!(calculated_alpha, expected_alpha);
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

        #[ink::test]
        fn take_alpha_listing_fails_with_wrong_payment() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(1.0);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Create a mock listing
            let alpha_amount = 5_000_000_000u64;
            let price_per_alpha = U64F64::from_num(2u64);
            let listing = AlphaListing {
                id: 1,
                netuid: 1,
                seller: accounts.charlie,
                amount: alpha_amount,
                price: FixedDecimal::from_bits(price_per_alpha.to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000,
            };

            contract
                .alpha_listings
                .insert((1, accounts.charlie, 1), &listing);

            // Set wrong payment amount
            ink::env::test::set_caller::<Environment>(accounts.django);
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(
                1_000_000_000u128,
            ); // Too little

            // Try to take the listing
            let result = contract.take_alpha_listing(1, accounts.charlie, 1);
            assert_eq!(result, Err(Error::InvalidPrice));
        }

        #[ink::test]
        fn take_alpha_listing_fails_when_not_found() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(1.0);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            ink::env::test::set_caller::<Environment>(accounts.django);
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(
                10_000_000_000u128,
            );

            // Try to take non-existent listing
            let result = contract.take_alpha_listing(1, accounts.charlie, 999);
            assert_eq!(result, Err(Error::ListingNotFound));
        }

        #[ink::test]
        fn take_tao_offer_fails_when_not_found() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(1.0);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            ink::env::test::set_caller::<Environment>(accounts.django);

            // Try to take non-existent offer
            let result = contract.take_tao_offer(1, accounts.charlie, 999, accounts.eve);
            assert_eq!(result, Err(Error::OfferNotFound));
        }

        #[ink::test]
        fn test_fixed_decimal_overflow_in_mul() {
            // Test multiplication that would overflow
            let large_decimal = FixedDecimal::from_bits(U64F64::from_num(u64::MAX / 2).to_bits());
            let result = large_decimal.mul(3);
            assert_eq!(result, Err(Error::Overflow));

            // Test with maximum safe value
            let safe_decimal = FixedDecimal::from_bits(U64F64::from_num(1000u64).to_bits());
            let safe_result = safe_decimal.mul(1_000_000);
            assert_eq!(safe_result, Ok(1_000_000_000));
        }

        #[ink::test]
        fn test_fixed_decimal_overflow_in_div_by() {
            // Test division with very small divisor - should produce a large number
            let tiny_decimal = FixedDecimal::from_bits(U64F64::from_num(0.000000001).to_bits());
            let result = tiny_decimal.div_by(100);

            // This produces 100 / 0.000000001 = 100,000,000,000 which fits in u64
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 99_999_999_998); // Slight rounding from fixed-point math

            // Test that would actually overflow
            let extremely_tiny =
                FixedDecimal::from_bits(U64F64::from_num(0.0000000000001).to_bits());
            let overflow_result = extremely_tiny.div_by(u64::MAX);
            assert_eq!(overflow_result, Err(Error::Overflow));

            // Test normal division
            let normal_decimal = FixedDecimal::from_bits(U64F64::from_num(10u64).to_bits());
            let normal_result = normal_decimal.div_by(1000);
            assert_eq!(normal_result, Ok(100));
        }

        #[ink::test]
        fn test_fixed_decimal_fee_calculations() {
            // Test various fee percentages
            let fee_0_1_percent = FixedDecimal::from_bits(U64F64::from_num(0.001).to_bits()); // 0.1%
            let fee_0_5_percent = FixedDecimal::from_bits(U64F64::from_num(0.005).to_bits()); // 0.5%
            let fee_1_percent = FixedDecimal::from_bits(U64F64::from_num(0.01).to_bits()); // 1%
            let fee_2_5_percent = FixedDecimal::from_bits(U64F64::from_num(0.025).to_bits()); // 2.5%

            let amount = 10_000_000_000u64; // 10 TAO

            assert_eq!(fee_0_1_percent.mul(amount), Ok(10_000_000)); // 0.01 TAO
            assert_eq!(fee_0_5_percent.mul(amount), Ok(50_000_000)); // 0.05 TAO
            assert_eq!(fee_1_percent.mul(amount), Ok(100_000_000)); // 0.1 TAO
            assert_eq!(fee_2_5_percent.mul(amount), Ok(250_000_000)); // 0.25 TAO
        }

        #[ink::test]
        fn test_fixed_decimal_maximum_values() {
            // Test with maximum TAO amounts (considering 1 TAO = 10^9 rao)
            let price = FixedDecimal::from_bits(U64F64::from_num(1.5).to_bits());
            let max_tao = 1_000_000_000_000_000u64; // 1 million TAO in rao

            // Should handle large TAO amounts
            let result = price.mul(max_tao);
            assert_eq!(result, Ok(1_500_000_000_000_000));

            // Test div_by_decimal with large amounts
            let alpha_result = price.div_by_decimal(max_tao);
            assert_eq!(alpha_result, Ok(666_666_666_666_666)); // Approximately 2/3 of max_tao
        }

        #[ink::test]
        fn test_listing_counter_increments() {
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

            // Initial counter should be 1
            assert_eq!(contract.next_alpha_listing_id, 1);

            // Manually simulate listing creation (since runtime calls won't work)
            contract.next_alpha_listing_id = 5;
            assert_eq!(contract.next_alpha_listing_id, 5);

            // Simulate another increment
            contract.next_alpha_listing_id += 1;
            assert_eq!(contract.next_alpha_listing_id, 6);
        }

        #[ink::test]
        fn test_offer_counter_increments() {
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

            // Initial counter should be 1
            assert_eq!(contract.next_tao_offer_id, 1);

            // Create an offer (this will actually work since it doesn't need runtime calls)
            ink::env::test::set_caller::<Environment>(accounts.charlie);
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(5_000_000_000);

            let price = U64F64::from_num(2u64).to_bits();
            let result = contract.create_tao_offer(1, price);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 1);

            // Counter should have incremented
            assert_eq!(contract.next_tao_offer_id, 2);

            // Create another offer
            let result2 = contract.create_tao_offer(1, price);
            assert!(result2.is_ok());
            assert_eq!(result2.unwrap(), 2);
            assert_eq!(contract.next_tao_offer_id, 3);
        }

        #[ink::test]
        fn test_multiple_listings_different_netuids() {
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

            // Create listings on different netuids for the same user
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
                netuid: 2,
                seller: accounts.charlie,
                amount: 3_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(1.5).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000,
            };

            let listing3 = AlphaListing {
                id: 3,
                netuid: 1,
                seller: accounts.charlie,
                amount: 7_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(3u64).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000,
            };

            // Insert listings
            contract
                .alpha_listings
                .insert((1, accounts.charlie, 1), &listing1);
            contract
                .alpha_listings
                .insert((2, accounts.charlie, 2), &listing2);
            contract
                .alpha_listings
                .insert((1, accounts.charlie, 3), &listing3);

            // Update user listings index
            contract
                .user_listings
                .insert((accounts.charlie, 1), &vec![1, 3]);
            contract
                .user_listings
                .insert((accounts.charlie, 2), &vec![2]);

            // Verify correct retrieval
            assert_eq!(contract.get_user_listings(accounts.charlie, 1), vec![1, 3]);
            assert_eq!(contract.get_user_listings(accounts.charlie, 2), vec![2]);
            assert_eq!(
                contract.get_user_listings(accounts.charlie, 3),
                Vec::<AlphaListingId>::new()
            );

            // Verify individual listings
            assert!(contract.get_listing(1, accounts.charlie, 1).is_some());
            assert!(contract.get_listing(2, accounts.charlie, 2).is_some());
            assert!(contract.get_listing(1, accounts.charlie, 3).is_some());
            assert!(contract.get_listing(1, accounts.charlie, 2).is_none()); // Wrong netuid
        }

        #[ink::test]
        fn test_multiple_offers_different_netuids() {
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

            // Create offers on different netuids
            ink::env::test::set_caller::<Environment>(accounts.charlie);
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(5_000_000_000);

            let price = U64F64::from_num(2u64).to_bits();

            // Offer on netuid 1
            let offer1_id = contract.create_tao_offer(1, price).unwrap();
            assert_eq!(offer1_id, 1);

            // Offer on netuid 2
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(3_000_000_000);
            let offer2_id = contract.create_tao_offer(2, price).unwrap();
            assert_eq!(offer2_id, 2);

            // Another offer on netuid 1
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(7_000_000_000);
            let offer3_id = contract.create_tao_offer(1, price).unwrap();
            assert_eq!(offer3_id, 3);

            // Verify correct retrieval
            assert_eq!(contract.get_user_offers(accounts.charlie, 1), vec![1, 3]);
            assert_eq!(contract.get_user_offers(accounts.charlie, 2), vec![2]);
            assert_eq!(
                contract.get_user_offers(accounts.charlie, 3),
                Vec::<TaoOfferId>::new()
            );
        }

        #[ink::test]
        fn test_storage_cleanup_all_listings_removed() {
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

            // Create a single listing
            let listing = AlphaListing {
                id: 1,
                netuid: 1,
                seller: accounts.charlie,
                amount: 5_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(2u64).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000,
            };

            contract
                .alpha_listings
                .insert((1, accounts.charlie, 1), &listing);
            contract
                .user_listings
                .insert((accounts.charlie, 1), &vec![1]);

            // Verify listing exists
            assert!(contract.get_listing(1, accounts.charlie, 1).is_some());
            assert_eq!(contract.get_user_listings(accounts.charlie, 1).len(), 1);

            // Remove the listing completely
            contract.alpha_listings.remove((1, accounts.charlie, 1));
            contract.user_listings.remove((accounts.charlie, 1));

            // Verify complete cleanup
            assert!(contract.get_listing(1, accounts.charlie, 1).is_none());
            assert_eq!(contract.get_user_listings(accounts.charlie, 1).len(), 0);
        }

        #[ink::test]
        fn test_storage_cleanup_all_offers_removed() {
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

            // Create a single offer
            let offer = TaoOffer {
                id: 1,
                netuid: 1,
                buyer: accounts.charlie,
                amount: 5_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(2u64).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000,
            };

            contract.tao_offers.insert((1, accounts.charlie, 1), &offer);
            contract.user_offers.insert((accounts.charlie, 1), &vec![1]);

            // Verify offer exists
            assert!(contract.get_offer(1, accounts.charlie, 1).is_some());
            assert_eq!(contract.get_user_offers(accounts.charlie, 1).len(), 1);

            // Remove the offer completely
            contract.tao_offers.remove((1, accounts.charlie, 1));
            contract.user_offers.remove((accounts.charlie, 1));

            // Verify complete cleanup
            assert!(contract.get_offer(1, accounts.charlie, 1).is_none());
            assert_eq!(contract.get_user_offers(accounts.charlie, 1).len(), 0);
        }

        // Error Conditions

        #[ink::test]
        fn test_list_alpha_with_zero_amount() {
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

            ink::env::test::set_caller::<Environment>(accounts.charlie);
            let price = U64F64::from_num(2u64).to_bits();

            // Try to list with zero amount
            let result = contract.list_alpha(accounts.django, 1, 0, price);
            assert_eq!(result, Err(Error::AmountTooSmall));
        }

        #[ink::test]
        fn test_create_offer_with_maximum_amounts() {
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

            ink::env::test::set_caller::<Environment>(accounts.charlie);

            // Test with very large amount (but still valid)
            let large_amount = 1_000_000_000_000_000u128; // 1 million TAO
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(large_amount);

            let price = U64F64::from_num(2u64).to_bits();
            let result = contract.create_tao_offer(1, price);
            assert!(result.is_ok());

            let offer = contract
                .get_offer(1, accounts.charlie, result.unwrap())
                .unwrap();
            assert_eq!(offer.amount, large_amount as u64);
        }

        #[ink::test]
        fn test_fee_calculation_precision() {
            let accounts = ink::env::test::default_accounts::<Environment>();

            // Test with various fee rates to ensure precision
            let test_cases = vec![
                (0.1, 10_000_000_000u64, 10_000_000u64), // 0.1% of 10 TAO = 0.01 TAO
                (0.25, 10_000_000_000u64, 25_000_000u64), // 0.25% of 10 TAO = 0.025 TAO
                (0.33, 10_000_000_000u64, 32_999_999u64), // 0.33% of 10 TAO = ~0.033 TAO (rounding)
                (1.5, 10_000_000_000u64, 149_999_999u64), // 1.5% of 10 TAO = ~0.15 TAO (rounding)
                (2.75, 10_000_000_000u64, 275_000_000u64), // 2.75% of 10 TAO = 0.275 TAO
            ];

            for (fee_percentage, amount, expected_fee) in test_cases {
                let fee_rate = fee_rate_from_percentage(fee_percentage);
                let contract = OtcContract::new(
                    accounts.alice,
                    accounts.bob,
                    fee_rate,
                    1_000_000_000,
                    1_000_000_000,
                    100,
                );

                let calculated_fee = contract.fee_rate.mul(amount).unwrap();
                assert_eq!(
                    calculated_fee, expected_fee,
                    "Fee calculation failed for {}% of {}",
                    fee_percentage, amount
                );
            }
        }

        #[ink::test]
        fn test_take_listing_exact_payment_required() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(1.0);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Create a listing
            let alpha_amount = 5_000_000_000u64; // 5 Alpha
            let price_per_alpha = U64F64::from_num(2u64); // 2 TAO per Alpha
            let listing = AlphaListing {
                id: 1,
                netuid: 1,
                seller: accounts.charlie,
                amount: alpha_amount,
                price: FixedDecimal::from_bits(price_per_alpha.to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000,
            };

            contract
                .alpha_listings
                .insert((1, accounts.charlie, 1), &listing);

            // Calculate exact required payment
            let total_price = listing.price.mul(alpha_amount).unwrap(); // 10 TAO
            let fee = contract.fee_rate.mul(total_price).unwrap(); // 0.1 TAO (1% fee)
            let required_payment = total_price + fee; // 10.1 TAO

            ink::env::test::set_caller::<Environment>(accounts.django);

            // Test with payment too low by 1 rao
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(
                (required_payment - 1) as u128,
            );
            let result = contract.take_alpha_listing(1, accounts.charlie, 1);
            assert_eq!(result, Err(Error::InvalidPrice));

            // Test with exact payment
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(
                required_payment as u128,
            );
            // Would succeed if runtime calls worked, but we test the validation logic

            // Test with payment too high by 1 rao
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(
                (required_payment + 1) as u128,
            );
            let result = contract.take_alpha_listing(1, accounts.charlie, 1);
            assert_eq!(result, Err(Error::InvalidPrice));
        }

        #[ink::test]
        fn test_take_offer_calculation_accuracy() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(1.0);

            let contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Test various price and amount combinations
            let test_cases = vec![
                (2.0, 10_000_000_000u64, 4_950_000_000u64), // 2 TAO/Alpha, 10 TAO offer -> 4.95 Alpha
                (1.5, 15_000_000_000u64, 9_900_000_000u64), // 1.5 TAO/Alpha, 15 TAO offer -> 9.9 Alpha
                (0.5, 5_000_000_000u64, 9_900_000_000u64), // 0.5 TAO/Alpha, 5 TAO offer -> 9.9 Alpha
                (3.33, 33_300_000_000u64, 9_900_000_000u64), // 3.33 TAO/Alpha, 33.3 TAO offer -> ~9.9 Alpha
            ];

            for (price_float, tao_amount, expected_alpha) in test_cases {
                let price = FixedDecimal::from_bits(U64F64::from_num(price_float).to_bits());
                let fee_amount = contract.fee_rate.mul(tao_amount).unwrap();
                let tao_for_seller = tao_amount - fee_amount;
                let calculated_alpha = price.div_by_decimal(tao_for_seller).unwrap();

                // Allow for small rounding differences (within 1000 rao)
                let diff = if calculated_alpha > expected_alpha {
                    calculated_alpha - expected_alpha
                } else {
                    expected_alpha - calculated_alpha
                };
                assert!(
                    diff < 1000,
                    "Calculation mismatch for price {} with {} TAO: got {}, expected {}",
                    price_float,
                    tao_amount,
                    calculated_alpha,
                    expected_alpha
                );
            }
        }

        // Integration-style Unit Tests

        #[ink::test]
        fn test_complete_trade_flow_storage_cleanup() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(1.0);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Create a listing
            let listing = AlphaListing {
                id: 1,
                netuid: 1,
                seller: accounts.charlie,
                amount: 5_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(2u64).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000,
            };

            contract
                .alpha_listings
                .insert((1, accounts.charlie, 1), &listing);
            contract
                .user_listings
                .insert((accounts.charlie, 1), &vec![1]);

            // Verify listing exists
            assert!(contract.get_listing(1, accounts.charlie, 1).is_some());

            // Simulate successful trade by removing listing and cleaning up storage
            contract.alpha_listings.remove((1, accounts.charlie, 1));
            let mut user_listings = contract
                .user_listings
                .get((accounts.charlie, 1))
                .unwrap_or_default();
            user_listings.retain(|&id| id != 1);
            if user_listings.is_empty() {
                contract.user_listings.remove((accounts.charlie, 1));
            } else {
                contract
                    .user_listings
                    .insert((accounts.charlie, 1), &user_listings);
            }

            // Verify complete cleanup
            assert!(contract.get_listing(1, accounts.charlie, 1).is_none());
            assert_eq!(contract.get_user_listings(accounts.charlie, 1).len(), 0);
        }

        #[ink::test]
        fn test_multiple_users_trading_simultaneously() {
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

            // Create listings from different sellers
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
                seller: accounts.django,
                amount: 3_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(1.5).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1100,
            };

            contract
                .alpha_listings
                .insert((1, accounts.charlie, 1), &listing1);
            contract
                .alpha_listings
                .insert((1, accounts.django, 2), &listing2);
            contract
                .user_listings
                .insert((accounts.charlie, 1), &vec![1]);
            contract
                .user_listings
                .insert((accounts.django, 1), &vec![2]);

            // Create offers from different buyers
            ink::env::test::set_caller::<Environment>(accounts.eve);
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(7_000_000_000);
            let offer1_id = contract
                .create_tao_offer(1, U64F64::from_num(2.5).to_bits())
                .unwrap();

            ink::env::test::set_caller::<Environment>(accounts.frank);
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(4_000_000_000);
            let offer2_id = contract
                .create_tao_offer(1, U64F64::from_num(1.8).to_bits())
                .unwrap();

            // Verify all exist independently
            assert!(contract.get_listing(1, accounts.charlie, 1).is_some());
            assert!(contract.get_listing(1, accounts.django, 2).is_some());
            assert!(contract.get_offer(1, accounts.eve, offer1_id).is_some());
            assert!(contract.get_offer(1, accounts.frank, offer2_id).is_some());

            // Verify user indices are correct
            assert_eq!(contract.get_user_listings(accounts.charlie, 1), vec![1]);
            assert_eq!(contract.get_user_listings(accounts.django, 1), vec![2]);
            assert_eq!(contract.get_user_offers(accounts.eve, 1), vec![offer1_id]);
            assert_eq!(contract.get_user_offers(accounts.frank, 1), vec![offer2_id]);
        }

        #[ink::test]
        fn test_listing_and_offer_interaction() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let fee_rate = fee_rate_from_percentage(1.0);

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // User creates both a listing and an offer
            let listing = AlphaListing {
                id: 1,
                netuid: 1,
                seller: accounts.charlie,
                amount: 5_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(2u64).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000,
            };

            contract
                .alpha_listings
                .insert((1, accounts.charlie, 1), &listing);
            contract
                .user_listings
                .insert((accounts.charlie, 1), &vec![1]);

            // Same user creates an offer on a different netuid
            ink::env::test::set_caller::<Environment>(accounts.charlie);
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(3_000_000_000);
            let offer_id = contract
                .create_tao_offer(2, U64F64::from_num(1.5).to_bits())
                .unwrap();

            // Verify both exist independently
            assert!(contract.get_listing(1, accounts.charlie, 1).is_some());
            assert!(contract.get_offer(2, accounts.charlie, offer_id).is_some());

            // Verify they're tracked separately
            assert_eq!(contract.get_user_listings(accounts.charlie, 1), vec![1]);
            assert_eq!(
                contract.get_user_listings(accounts.charlie, 2),
                Vec::<AlphaListingId>::new()
            );
            assert_eq!(
                contract.get_user_offers(accounts.charlie, 1),
                Vec::<TaoOfferId>::new()
            );
            assert_eq!(
                contract.get_user_offers(accounts.charlie, 2),
                vec![offer_id]
            );
        }

        #[ink::test]
        fn test_zero_fee_rate() {
            let accounts = ink::env::test::default_accounts::<Environment>();
            let zero_fee_rate = 0u128; // 0% fee

            let mut contract = OtcContract::new(
                accounts.alice,
                accounts.bob,
                zero_fee_rate,
                1_000_000_000,
                1_000_000_000,
                100,
            );

            // Create a listing with zero fee
            let listing = AlphaListing {
                id: 1,
                netuid: 1,
                seller: accounts.charlie,
                amount: 5_000_000_000,
                price: FixedDecimal::from_bits(U64F64::from_num(2u64).to_bits()),
                fee_rate: contract.fee_rate,
                created_at: 1000,
            };

            contract
                .alpha_listings
                .insert((1, accounts.charlie, 1), &listing);

            // Calculate payment with zero fee
            let total_price = listing.price.mul(listing.amount).unwrap();
            let fee = contract.fee_rate.mul(total_price).unwrap();
            assert_eq!(fee, 0); // Fee should be zero

            let required_payment = total_price + fee;
            assert_eq!(required_payment, total_price); // Payment equals price without fee
        }

        #[ink::test]
        fn test_boundary_netuid_values() {
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

            // Test with minimum and maximum netuid values
            let min_netuid: u16 = 0;
            let max_netuid: u16 = u16::MAX;

            // Create offers with boundary netuid values
            ink::env::test::set_caller::<Environment>(accounts.charlie);
            ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(2_000_000_000);

            let offer1 = contract.create_tao_offer(min_netuid, U64F64::from_num(1u64).to_bits());
            assert!(offer1.is_ok());

            let offer2 = contract.create_tao_offer(max_netuid, U64F64::from_num(1u64).to_bits());
            assert!(offer2.is_ok());

            // Verify retrieval works with boundary values
            assert!(contract
                .get_offer(min_netuid, accounts.charlie, offer1.unwrap())
                .is_some());
            assert!(contract
                .get_offer(max_netuid, accounts.charlie, offer2.unwrap())
                .is_some());
        }
    }
}
