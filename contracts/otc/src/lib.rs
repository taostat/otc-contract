#![cfg_attr(not(feature = "std"), no_std, no_main)]

#[cfg(test)]
mod tests;

pub mod errors;
pub mod events;
pub mod types;

pub use otc_shared::BittensorEnvironment;

#[ink::contract(env = otc_shared::BittensorEnvironment)]
mod otc_contract {
    use crate::errors::Error;
    use crate::events::*;
    use crate::types::{
        AlphaListing, AlphaListingId, AlphaListingsMapping, FrozenSubnetsMapping,
        ReservedAlphaMapping, TaoOffer, TaoOfferId, TaoOffersMapping, UserListingsMapping,
        UserOffersMapping,
    };
    use fixed::types::U64F64;
    use ink::prelude::{boxed::Box, vec::Vec};
    use otc_shared::{
        stake_delta_verified, AlphaAmount, AlphaCurrency, BlockAge, FixedDecimal, NetUid,
        PauseState, PriceOffsetBps, ProxyCall, RuntimeCall, SubtensorCall, TaoAmount,
    };
    use sp_runtime::MultiAddress;

    /// Maximum number of active listings a user can maintain per subnet
    const MAX_LISTINGS_PER_USER_PER_NETUID: usize = 25;

    /// Maximum number of active offers a user can maintain per subnet
    const MAX_OFFERS_PER_USER_PER_NETUID: usize = 25;

    #[ink(storage)]
    pub struct OtcContract {
        /// Listings: (netuid, seller, listing_id) -> AlphaListing
        pub alpha_listings: AlphaListingsMapping,

        /// User's listing IDs for iteration: (seller, netuid) -> Vec<listing_id>
        pub user_listings: UserListingsMapping,

        /// Offers: (netuid, buyer, offer_id) -> TaoOffer
        pub tao_offers: TaoOffersMapping,

        /// User's offer IDs for iteration: (buyer, netuid) -> Vec<offer_id>
        pub user_offers: UserOffersMapping,

        /// Alpha currently reserved for open listings per subnet
        pub reserved_alpha: ReservedAlphaMapping,

        /// Subnets that are currently frozen for new listings
        pub frozen_subnets: FrozenSubnetsMapping,

        /// Global listing counter
        pub next_alpha_listing_id: AlphaListingId,

        /// Global offer counter
        pub next_tao_offer_id: TaoOfferId,

        /// Contract owner
        pub owner: AccountId,

        /// Validator hotkey
        pub hotkey: AccountId,

        /// Fee rate charged on trades (as decimal, e.g., 0.005 = 0.5%)
        pub fee_rate: FixedDecimal,

        /// Minimum Alpha amount for listings
        pub min_listing_amount: AlphaAmount,

        /// Minimum TAO amount for offers
        pub min_offer_amount: TaoAmount,

        /// Minimum age before listing can be cancelled (in blocks)
        pub min_listing_age: BlockAge,

        /// Pause state for emergency control
        pub pause_state: PauseState,
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
                reserved_alpha: Default::default(),
                frozen_subnets: Default::default(),
                next_alpha_listing_id: 1,
                next_tao_offer_id: 1,
                owner,
                hotkey,
                fee_rate,
                min_listing_amount,
                min_offer_amount,
                min_listing_age,
                pause_state: PauseState::NotPaused,
            }
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

        /// A transfer that completed but moved the wrong amount must revert the whole
        /// call, including the transfer itself, rather than leave accounting skewed.
        fn trap_stake_transfer_not_verified() -> ! {
            panic!("post-transfer stake verification failed")
        }

        fn reserved_alpha_for(&self, netuid: NetUid) -> AlphaAmount {
            self.reserved_alpha.get(netuid).unwrap_or(0)
        }

        pub fn increase_reserved_alpha(
            &mut self,
            netuid: NetUid,
            amount: AlphaAmount,
        ) -> Result<(), Error> {
            if amount == 0 {
                return Ok(());
            }

            let current = self.reserved_alpha_for(netuid);
            let new_total = current.checked_add(amount).ok_or(Error::Overflow)?;
            self.reserved_alpha.insert(netuid, &new_total);
            Ok(())
        }

        pub fn decrease_reserved_alpha(
            &mut self,
            netuid: NetUid,
            amount: AlphaAmount,
        ) -> Result<(), Error> {
            if amount == 0 {
                return Ok(());
            }

            let current = self.reserved_alpha_for(netuid);
            let new_total = current.checked_sub(amount).ok_or(Error::Overflow)?;

            if new_total == 0 {
                self.reserved_alpha.remove(netuid);
            } else {
                self.reserved_alpha.insert(netuid, &new_total);
            }

            Ok(())
        }

        fn is_subnet_frozen_internal(&self, netuid: NetUid) -> bool {
            self.frozen_subnets.get(netuid).unwrap_or(false)
        }

        fn ensure_subnet_not_frozen(&self, netuid: NetUid) -> Result<(), Error> {
            if self.is_subnet_frozen_internal(netuid) {
                return Err(Error::SubnetListingsFrozen);
            }
            Ok(())
        }

        pub fn calculate_claimable_dividends(
            &self,
            netuid: NetUid,
            contract_stake: AlphaAmount,
        ) -> Result<AlphaAmount, Error> {
            let reserved = self.reserved_alpha_for(netuid);

            if contract_stake <= reserved {
                return Err(Error::NoDividendsAvailable);
            }

            contract_stake.checked_sub(reserved).ok_or(Error::Overflow)
        }

        /// Get the current market price for an Alpha token on a subnet
        fn get_market_price(&self, netuid: NetUid) -> Result<u64, Error> {
            self.env()
                .extension()
                .get_alpha_price(netuid)
                .map_err(|_| Error::MarketPriceFetchFailed)
        }

        /// Calculate actual price from market price and basis points offset
        /// market_price is in format: TAO_per_Alpha * 1e9
        /// Returns: adjusted price in same format (TAO_per_Alpha * 1e9)
        pub fn apply_price_offset(
            market_price: u64,
            offset_bps: PriceOffsetBps,
        ) -> Result<u64, Error> {
            // offset_bps: -500 = -5%, 1000 = +10%
            // formula: market_price * (10000 + offset_bps) / 10000
            let base: i64 = 10000;
            let offset = i64::from(offset_bps);
            let multiplier = base.checked_add(offset).ok_or(Error::Overflow)?;

            if multiplier <= 0 {
                return Err(Error::InvalidPriceOffset);
            }

            // Safe cast: multiplier is guaranteed positive after the check above
            let multiplier_u128 = u128::try_from(multiplier).map_err(|_| Error::Overflow)?;

            let result = u128::from(market_price)
                .checked_mul(multiplier_u128)
                .ok_or(Error::Overflow)?
                .checked_div(10000)
                .ok_or(Error::Overflow)?;

            u64::try_from(result).map_err(|_| Error::Overflow)
        }

        /// Convert price from chain extension format (price * 1e9) to FixedDecimal
        pub fn price_to_fixed_decimal(scaled_price: u64) -> FixedDecimal {
            // scaled_price = TAO_per_Alpha * 1e9
            // We need FixedDecimal representing TAO_per_Alpha
            let divisor = U64F64::from_num(1_000_000_000u64);
            let price = U64F64::from_num(scaled_price)
                .checked_div(divisor)
                .unwrap_or_default();
            FixedDecimal::from_bits(price.to_bits())
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

        /// Get Alpha currently reserved for listings on a subnet
        #[ink(message)]
        pub fn get_reserved_alpha(&self, netuid: NetUid) -> AlphaAmount {
            self.reserved_alpha_for(netuid)
        }

        /// Check if listings are frozen for a subnet
        #[ink(message)]
        pub fn is_subnet_frozen(&self, netuid: NetUid) -> bool {
            self.is_subnet_frozen_internal(netuid)
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

        /// Update subnet listing status (freeze/unfreeze)
        /// Can only be called by the contract owner
        #[ink(message)]
        pub fn set_subnet_listing_status(
            &mut self,
            netuid: NetUid,
            frozen: bool,
            reason: Vec<u8>,
        ) -> Result<(), Error> {
            self.ensure_owner()?;

            if frozen {
                self.frozen_subnets.insert(netuid, &true);
            } else {
                self.frozen_subnets.remove(netuid);
            }

            self.env().emit_event(SubnetListingStatusChanged {
                netuid,
                frozen,
                reason,
                changed_by: self.env().caller(),
            });

            Ok(())
        }

        /// List Alpha tokens for sale with dynamic market-relative pricing
        /// Prerequisites: The seller must have added the contract as their proxy
        /// This will transfer the stake from the seller to the contract via proxy
        /// and consolidate it under the contract's hotkey if needed
        /// price_offset_bps: Price offset from market in basis points (-500 = -5%, 1000 = +10%)
        #[ink(message)]
        pub fn list_alpha(
            &mut self,
            hotkey: AccountId,
            netuid: NetUid,
            amount: AlphaAmount,
            price_offset_bps: PriceOffsetBps,
        ) -> Result<AlphaListingId, Error> {
            self.ensure_trading_enabled()?;
            self.ensure_subnet_not_frozen(netuid)?;

            let seller = self.env().caller();

            if amount < self.min_listing_amount {
                return Err(Error::AmountTooSmall);
            }

            // Validate price offset (cannot be <= -100% as that would result in zero or negative price)
            if price_offset_bps <= -10000 {
                return Err(Error::InvalidPriceOffset);
            }

            let mut user_listings = self.user_listings.get((seller, netuid)).unwrap_or_default();
            if user_listings.len() >= MAX_LISTINGS_PER_USER_PER_NETUID {
                return Err(Error::TooManyListings);
            }

            let listing_id = self.next_alpha_listing_id;
            let next_id = listing_id.checked_add(1).ok_or(Error::Overflow)?;

            let seller_stake_before = self.get_stake_amount(seller, hotkey, netuid)?;
            if seller_stake_before < amount {
                return Err(Error::InsufficientStake);
            }

            let contract_stake_before = self
                .get_stake_amount(self.env().account_id(), hotkey, netuid)
                .unwrap_or(0);

            let transfer_call = RuntimeCall::SubtensorModule(SubtensorCall::TransferStake {
                destination_coldkey: self.env().account_id(),
                hotkey,
                origin_netuid: otc_shared::runtime::NetUid::from(netuid),
                destination_netuid: otc_shared::runtime::NetUid::from(netuid),
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

            let seller_stake_after = self.get_stake_amount(seller, hotkey, netuid).unwrap_or(0);
            let contract_stake_after =
                self.get_stake_amount(self.env().account_id(), hotkey, netuid)?;

            let seller_decrease = seller_stake_before.saturating_sub(seller_stake_after);
            let contract_increase = contract_stake_after.saturating_sub(contract_stake_before);

            if !stake_delta_verified(seller_decrease, amount)
                || !stake_delta_verified(contract_increase, amount)
            {
                Self::trap_stake_transfer_not_verified();
            }

            self.increase_reserved_alpha(netuid, amount)?;

            // Consolidate stake if needed (move to contract's hotkey)
            if hotkey != self.hotkey {
                self.env()
                    .extension()
                    .move_stake(
                        hotkey,
                        self.hotkey,
                        netuid,
                        netuid,
                        AlphaCurrency::from(amount),
                    )
                    .map_err(|_| Error::RuntimeCallFailed)?;
            }

            self.next_alpha_listing_id = next_id;

            let listing = AlphaListing {
                id: listing_id,
                netuid,
                seller,
                amount,
                price_offset_bps,
                fee_rate: self.fee_rate,
                created_at: self.env().block_number(),
            };

            self.alpha_listings
                .insert((netuid, seller, listing_id), &listing);

            user_listings.push(listing_id);
            self.user_listings.insert((seller, netuid), &user_listings);

            self.env().emit_event(AlphaListed {
                seller,
                hotkey,
                netuid,
                alpha_listing_id: listing_id,
                amount,
                price_offset_bps,
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
            self.ensure_not_fully_paused()?;

            let seller = self.env().caller();

            let current_block = self.env().block_number();
            let listing = self
                .alpha_listings
                .get((netuid, seller, listing_id))
                .ok_or(Error::ListingNotFound)?;
            let listing_age = current_block.saturating_sub(listing.created_at);

            if listing_age < self.min_listing_age {
                return Err(Error::ListingTooYoung);
            }

            self.execute_listing_cancellation(listing, seller, false)
        }

        /// Force-cancel an Alpha listing as the contract owner
        #[ink(message)]
        pub fn force_cancel_alpha_listing(
            &mut self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: AlphaListingId,
        ) -> Result<(), Error> {
            self.ensure_owner()?;

            let listing = self
                .alpha_listings
                .get((netuid, seller, listing_id))
                .ok_or(Error::ListingNotFound)?;

            let initiator = self.env().caller();

            self.execute_listing_cancellation(listing, initiator, true)
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

        /// Create a TAO offer by depositing TAO into the contract with dynamic market-relative pricing
        /// The buyer sends TAO with the transaction and specifies their price offset from market
        /// price_offset_bps: Price offset from market in basis points (-500 = -5%, 1000 = +10%)
        #[ink(message, payable)]
        pub fn create_tao_offer(
            &mut self,
            netuid: NetUid,
            price_offset_bps: PriceOffsetBps,
        ) -> Result<TaoOfferId, Error> {
            self.ensure_trading_enabled()?;

            let buyer = self.env().caller();
            let amount = self.env().transferred_value();

            if amount < self.min_offer_amount {
                return Err(Error::AmountTooSmall);
            }

            // Validate price offset (cannot be <= -100% as that would result in zero or negative price)
            if price_offset_bps <= -10000 {
                return Err(Error::InvalidPriceOffset);
            }

            let mut user_offers = self.user_offers.get((buyer, netuid)).unwrap_or_default();
            if user_offers.len() >= MAX_OFFERS_PER_USER_PER_NETUID {
                return Err(Error::TooManyOffers);
            }

            let offer_id = self.next_tao_offer_id;
            self.next_tao_offer_id = offer_id.checked_add(1).ok_or(Error::Overflow)?;

            let offer = TaoOffer {
                id: offer_id,
                netuid,
                buyer,
                amount,
                price_offset_bps,
                fee_rate: self.fee_rate,
                created_at: self.env().block_number(),
            };

            self.tao_offers.insert((netuid, buyer, offer_id), &offer);

            user_offers.push(offer_id);
            self.user_offers.insert((buyer, netuid), &user_offers);

            self.env().emit_event(TaoOfferCreated {
                buyer,
                netuid,
                tao_offer_id: offer_id,
                amount,
                price_offset_bps,
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
            self.ensure_not_fully_paused()?;

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

        /// Estimate the price for taking an Alpha listing at current market price
        /// Returns: (executed_price * 1e9, tao_amount, total_with_fee)
        #[ink(message)]
        pub fn estimate_listing_price(
            &self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: AlphaListingId,
        ) -> Result<(u64, TaoAmount, TaoAmount), Error> {
            let listing = self
                .alpha_listings
                .get((netuid, seller, listing_id))
                .ok_or(Error::ListingNotFound)?;

            let market_price = self.get_market_price(listing.netuid)?;
            let executed_price = Self::apply_price_offset(market_price, listing.price_offset_bps)?;
            let price_decimal = Self::price_to_fixed_decimal(executed_price);

            let tao_amount = price_decimal
                .mul(listing.amount)
                .map_err(|_| Error::Overflow)?;
            let fee_amount = listing
                .fee_rate
                .mul(tao_amount)
                .map_err(|_| Error::Overflow)?;
            let total_required = tao_amount.checked_add(fee_amount).ok_or(Error::Overflow)?;

            Ok((executed_price, tao_amount, total_required))
        }

        /// Estimate the Alpha amount needed to take a TAO offer at current market price
        /// Returns: (executed_price * 1e9, alpha_amount_needed, tao_for_seller)
        #[ink(message)]
        pub fn estimate_offer_price(
            &self,
            netuid: NetUid,
            buyer: AccountId,
            offer_id: TaoOfferId,
        ) -> Result<(u64, AlphaAmount, TaoAmount), Error> {
            let offer = self
                .tao_offers
                .get((netuid, buyer, offer_id))
                .ok_or(Error::OfferNotFound)?;

            let fee_amount = offer
                .fee_rate
                .mul(offer.amount)
                .map_err(|_| Error::Overflow)?;
            let tao_for_seller = offer
                .amount
                .checked_sub(fee_amount)
                .ok_or(Error::Overflow)?;

            let market_price = self.get_market_price(offer.netuid)?;
            let executed_price = Self::apply_price_offset(market_price, offer.price_offset_bps)?;
            let price_decimal = Self::price_to_fixed_decimal(executed_price);

            let alpha_amount = price_decimal
                .as_divisor_of(tao_for_seller)
                .map_err(|_| Error::Overflow)?;

            Ok((executed_price, alpha_amount, tao_for_seller))
        }

        /// Take an Alpha listing by paying TAO with dynamic market-relative pricing
        /// The buyer sends TAO with the transaction to purchase Alpha tokens
        /// Price is calculated based on current market price + listing's price offset
        /// Allows overpayment and refunds excess TAO to buyer
        #[ink(message, payable)]
        pub fn take_alpha_listing(
            &mut self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: AlphaListingId,
        ) -> Result<(), Error> {
            self.ensure_trading_enabled()?;

            let buyer = self.env().caller();
            let tao_received = self.env().transferred_value();

            let listing = self
                .alpha_listings
                .get((netuid, seller, listing_id))
                .ok_or(Error::ListingNotFound)?;

            let contract_stake_before =
                self.get_stake_amount(self.env().account_id(), self.hotkey, netuid)?;

            // Fetch current market price (returns TAO_per_Alpha * 1e9)
            let market_price = self.get_market_price(listing.netuid)?;

            // Apply offset to get executed price
            let executed_price = Self::apply_price_offset(market_price, listing.price_offset_bps)?;

            // Convert to FixedDecimal for calculation
            let price_decimal = Self::price_to_fixed_decimal(executed_price);

            // Calculate TAO required: price * alpha_amount
            let tao_amount = price_decimal
                .mul(listing.amount)
                .map_err(|_| Error::Overflow)?;

            // Calculate fee
            let fee_amount = listing
                .fee_rate
                .mul(tao_amount)
                .map_err(|_| Error::Overflow)?;

            let total_tao_required = tao_amount.checked_add(fee_amount).ok_or(Error::Overflow)?;

            // Verify payment (allow overpayment, will refund excess)
            if tao_received < total_tao_required {
                return Err(Error::InsufficientPayment);
            }

            let excess_tao = tao_received
                .checked_sub(total_tao_required)
                .ok_or(Error::Overflow)?;

            self.alpha_listings.remove((netuid, seller, listing_id));

            let mut user_listings = self.user_listings.get((seller, netuid)).unwrap_or_default();
            user_listings.retain(|&id| id != listing_id);

            if user_listings.is_empty() {
                self.user_listings.remove((seller, netuid));
            } else {
                self.user_listings.insert((seller, netuid), &user_listings);
            }

            // Transfer Alpha from contract to buyer
            self.env()
                .extension()
                .transfer_stake(
                    buyer,
                    self.hotkey,
                    netuid,
                    netuid,
                    AlphaCurrency::from(listing.amount),
                )
                .map_err(|_| Error::RuntimeCallFailed)?;

            let contract_stake_after =
                self.get_stake_amount(self.env().account_id(), self.hotkey, netuid)?;
            let contract_decrease = contract_stake_before.saturating_sub(contract_stake_after);
            if !stake_delta_verified(contract_decrease, listing.amount) {
                Self::trap_stake_transfer_not_verified();
            }

            self.decrease_reserved_alpha(netuid, listing.amount)?;

            // Transfer TAO to seller (minus fee)
            self.env()
                .transfer(listing.seller, tao_amount)
                .map_err(|_| Error::TransferFailed)?;

            if fee_amount > 0 {
                self.env()
                    .transfer(self.owner, fee_amount)
                    .map_err(|_| Error::TransferFailed)?;
            }

            // Refund excess TAO to buyer if any
            if excess_tao > 0 {
                self.env()
                    .transfer(buyer, excess_tao)
                    .map_err(|_| Error::TransferFailed)?;
            }

            self.env().emit_event(AlphaListingTaken {
                seller,
                buyer,
                netuid,
                alpha_amount: listing.amount,
                tao_amount,
                price_offset_bps: listing.price_offset_bps,
                executed_price,
                fee: fee_amount,
                alpha_listing_id: Some(listing_id),
            });

            Ok(())
        }

        /// Claim staking rewards accumulated by the contract for a subnet
        /// Only callable by the contract owner while the contract is not fully paused
        #[ink(message)]
        pub fn claim_dividends(&mut self, netuid: NetUid) -> Result<(), Error> {
            self.ensure_owner()?;
            self.ensure_not_fully_paused()?;

            let contract_coldkey = self.env().account_id();
            let contract_stake_before =
                self.get_stake_amount(contract_coldkey, self.hotkey, netuid)?;

            let claimable = self.calculate_claimable_dividends(netuid, contract_stake_before)?;

            self.env()
                .extension()
                .transfer_stake(
                    self.owner,
                    self.hotkey,
                    netuid,
                    netuid,
                    AlphaCurrency::from(claimable),
                )
                .map_err(|_| Error::RuntimeCallFailed)?;

            let contract_stake_after =
                self.get_stake_amount(contract_coldkey, self.hotkey, netuid)?;
            let contract_decrease = contract_stake_before.saturating_sub(contract_stake_after);

            if !stake_delta_verified(contract_decrease, claimable) {
                Self::trap_stake_transfer_not_verified();
            }

            self.env().emit_event(DividendsClaimed {
                owner: self.owner,
                netuid,
                amount: contract_decrease,
                reserved_after: self.reserved_alpha_for(netuid),
            });

            Ok(())
        }

        /// Take a TAO offer by providing Alpha tokens with dynamic market-relative pricing
        /// The seller provides Alpha tokens to claim the TAO at the market price + offer's offset
        /// Prerequisites: The seller must have added the contract as their proxy
        #[ink(message)]
        pub fn take_tao_offer(
            &mut self,
            netuid: NetUid,
            buyer: AccountId,
            offer_id: TaoOfferId,
            hotkey: AccountId,
        ) -> Result<(), Error> {
            self.ensure_trading_enabled()?;

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

            // Fetch current market price (returns TAO_per_Alpha * 1e9)
            let market_price = self.get_market_price(offer.netuid)?;

            // Apply offset to get executed price
            let executed_price = Self::apply_price_offset(market_price, offer.price_offset_bps)?;

            // Convert to FixedDecimal for calculation
            let price_decimal = Self::price_to_fixed_decimal(executed_price);

            // Calculate Alpha amount needed: tao_for_seller / price
            let alpha_amount = price_decimal
                .as_divisor_of(tao_for_seller)
                .map_err(|_| Error::Overflow)?;

            let seller_stake_before = self.get_stake_amount(seller, hotkey, netuid)?;
            if seller_stake_before < alpha_amount {
                return Err(Error::InsufficientStake);
            }
            let buyer_stake_before = self.get_stake_amount(buyer, hotkey, netuid).unwrap_or(0);

            // Transfer Alpha directly from seller to buyer via proxy
            let transfer_call = RuntimeCall::SubtensorModule(SubtensorCall::TransferStake {
                destination_coldkey: buyer,
                hotkey, // Seller's original hotkey
                origin_netuid: otc_shared::runtime::NetUid::from(netuid),
                destination_netuid: otc_shared::runtime::NetUid::from(netuid),
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

            let seller_stake_after = self.get_stake_amount(seller, hotkey, netuid).unwrap_or(0);
            let buyer_stake_after = self.get_stake_amount(buyer, hotkey, netuid)?;

            let seller_decrease = seller_stake_before.saturating_sub(seller_stake_after);
            let buyer_increase = buyer_stake_after.saturating_sub(buyer_stake_before);

            if !stake_delta_verified(seller_decrease, alpha_amount)
                || !stake_delta_verified(buyer_increase, alpha_amount)
            {
                Self::trap_stake_transfer_not_verified();
            }

            self.tao_offers.remove((netuid, buyer, offer_id));

            let mut user_offers = self.user_offers.get((buyer, netuid)).unwrap_or_default();
            user_offers.retain(|&id| id != offer_id);

            if user_offers.is_empty() {
                self.user_offers.remove((buyer, netuid));
            } else {
                self.user_offers.insert((buyer, netuid), &user_offers);
            }

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
                price_offset_bps: offer.price_offset_bps,
                executed_price,
                fee: fee_amount,
                tao_offer_id: Some(offer_id),
            });

            Ok(())
        }

        /// Replace contract code with new implementation
        /// Only callable by contract owner
        #[ink(message)]
        pub fn set_code(&mut self, code_hash: Hash) -> Result<(), Error> {
            self.ensure_owner()?;

            self.env()
                .set_code_hash(&code_hash)
                .map_err(|_| Error::CodeUpgradeFailed)?;

            Ok(())
        }

        /// Pause trading operations (no new trades, but cancellations allowed)
        /// Only callable by contract owner
        #[ink(message)]
        pub fn pause_trading(&mut self, reason: Vec<u8>) -> Result<(), Error> {
            self.ensure_owner()?;

            self.pause_state = PauseState::TradingPaused;

            self.env().emit_event(ContractPaused {
                pause_state: PauseState::TradingPaused,
                reason,
                paused_by: self.env().caller(),
            });

            Ok(())
        }

        /// Fully pause the contract (no operations except admin functions)
        /// Only callable by contract owner
        #[ink(message)]
        pub fn pause_fully(&mut self, reason: Vec<u8>) -> Result<(), Error> {
            self.ensure_owner()?;

            self.pause_state = PauseState::FullyPaused;

            self.env().emit_event(ContractPaused {
                pause_state: PauseState::FullyPaused,
                reason,
                paused_by: self.env().caller(),
            });

            Ok(())
        }

        /// Resume normal contract operations
        /// Only callable by contract owner
        #[ink(message)]
        pub fn resume(&mut self) -> Result<(), Error> {
            self.ensure_owner()?;

            self.pause_state = PauseState::NotPaused;

            self.env().emit_event(ContractResumed {
                resumed_by: self.env().caller(),
            });

            Ok(())
        }

        /// Get the current pause state
        #[ink(message)]
        pub fn get_pause_state(&self) -> PauseState {
            self.pause_state
        }

        fn execute_listing_cancellation(
            &mut self,
            listing: AlphaListing,
            initiated_by: AccountId,
            forced: bool,
        ) -> Result<(), Error> {
            let contract_coldkey = self.env().account_id();
            let netuid = listing.netuid;
            let seller = listing.seller;
            let listing_id = listing.id;

            let contract_stake_before =
                self.get_stake_amount(contract_coldkey, self.hotkey, netuid)?;

            self.alpha_listings.remove((netuid, seller, listing_id));

            let mut user_listings = self.user_listings.get((seller, netuid)).unwrap_or_default();
            user_listings.retain(|&id| id != listing_id);

            if user_listings.is_empty() {
                self.user_listings.remove((seller, netuid));
            } else {
                self.user_listings.insert((seller, netuid), &user_listings);
            }

            self.env()
                .extension()
                .transfer_stake(
                    seller,
                    self.hotkey,
                    netuid,
                    netuid,
                    AlphaCurrency::from(listing.amount),
                )
                .map_err(|_| Error::RuntimeCallFailed)?;

            let contract_stake_after =
                self.get_stake_amount(contract_coldkey, self.hotkey, netuid)?;
            let contract_decrease = contract_stake_before.saturating_sub(contract_stake_after);
            if !stake_delta_verified(contract_decrease, listing.amount) {
                Self::trap_stake_transfer_not_verified();
            }

            self.decrease_reserved_alpha(netuid, listing.amount)?;

            if forced {
                self.env().emit_event(AlphaListingForceCancelled {
                    seller,
                    initiated_by,
                    netuid,
                    listing_id,
                    amount_returned: listing.amount,
                });
            } else {
                self.env().emit_event(AlphaListingCancelled {
                    seller,
                    netuid,
                    listing_id,
                    amount_returned: listing.amount,
                });
            }

            Ok(())
        }

        /// Access control helper: ensure caller is the owner
        fn ensure_owner(&self) -> Result<(), Error> {
            if self.env().caller() != self.owner {
                return Err(Error::Unauthorized);
            }
            Ok(())
        }

        /// Pause guard: ensure contract is not fully paused
        pub fn ensure_not_fully_paused(&self) -> Result<(), Error> {
            if self.pause_state == PauseState::FullyPaused {
                return Err(Error::ContractFullyPaused);
            }
            Ok(())
        }

        /// Pause guard: ensure trading is enabled (not paused or trading paused)
        fn ensure_trading_enabled(&self) -> Result<(), Error> {
            match self.pause_state {
                PauseState::NotPaused => Ok(()),
                PauseState::TradingPaused => Err(Error::TradingPaused),
                PauseState::FullyPaused => Err(Error::ContractFullyPaused),
            }
        }
    }
}
