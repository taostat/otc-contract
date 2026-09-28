#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub mod errors;
pub mod events;
pub mod types;

pub use otc_shared::BittensorEnvironment;

#[ink::contract(env = otc_shared::BittensorEnvironment)]
mod lockup_listings {
    use crate::errors::Error;
    use crate::events::*;
    use crate::types::{
        EscrowsMapping, LockupListing, LockupListingId, LockupListingsMapping, PurchaseId,
        ReservedAlphaMapping, UserLockupListingsMapping,
    };
    use alpha_lockup::AlphaLockupRef;
    use fixed::types::U64F64;
    use ink::prelude::{boxed::Box, vec::Vec};
    use otc_shared::{
        stake_delta_verified, AlphaAmount, AlphaCurrency, BlockAge, FixedDecimal, NetUid,
        PauseState, PriceOffsetBps, ProxyCall, RuntimeCall, SubtensorCall, TaoAmount,
    };
    use sp_runtime::MultiAddress;

    /// Maximum number of active lockup listings a user can maintain per subnet
    const MAX_LISTINGS_PER_USER_PER_NETUID: usize = 25;

    /// Bittensor minimum stake transfer amount (2_000_000 rao = 0.002 TAO)
    const BITTENSOR_MIN_STAKE: u64 = 2_000_000;

    #[ink(storage)]
    pub struct LockupListingsContract {
        /// Lockup listings: (netuid, seller, listing_id) -> LockupListing
        pub lockup_listings: LockupListingsMapping,

        /// User's listing IDs for iteration: (seller, netuid) -> Vec<listing_id>
        pub user_lockup_listings: UserLockupListingsMapping,

        /// Track escrows created: (netuid, listing_id, purchase_id) -> escrow_account_id
        pub escrows: EscrowsMapping,

        /// Alpha currently reserved for lockup listings per subnet
        pub reserved_alpha: ReservedAlphaMapping,

        /// Global listing counter
        pub next_listing_id: LockupListingId,

        /// Global purchase counter (for unique escrow salts)
        pub next_purchase_id: PurchaseId,

        /// Contract owner
        pub owner: AccountId,

        /// Validator hotkey (escrows will use this hotkey)
        pub hotkey: AccountId,

        /// Code hash of the alpha_lockup escrow contract
        pub escrow_code_hash: Hash,

        /// Fee rate charged on trades (as decimal, e.g., 0.005 = 0.5%)
        pub fee_rate: FixedDecimal,

        /// Minimum Alpha amount for listings
        pub min_listing_amount: AlphaAmount,

        /// Minimum Alpha amount for purchases
        pub min_purchase_amount: AlphaAmount,

        /// Minimum lockup duration (in blocks)
        pub min_lockup_duration: BlockAge,

        /// Maximum lockup duration (in blocks)
        pub max_lockup_duration: BlockAge,

        /// Pause state for emergency control
        pub pause_state: PauseState,
    }

    impl LockupListingsContract {
        #[allow(clippy::too_many_arguments)]
        #[ink(constructor)]
        pub fn new(
            owner: AccountId,
            hotkey: AccountId,
            escrow_code_hash: Hash,
            fee_rate: u128,
            min_listing_amount: AlphaAmount,
            min_purchase_amount: AlphaAmount,
            min_lockup_duration: BlockAge,
            max_lockup_duration: BlockAge,
        ) -> Self {
            let fee_rate = FixedDecimal::from_bits(fee_rate);

            Self {
                lockup_listings: Default::default(),
                user_lockup_listings: Default::default(),
                escrows: Default::default(),
                reserved_alpha: Default::default(),
                next_listing_id: 1,
                next_purchase_id: 1,
                owner,
                hotkey,
                escrow_code_hash,
                fee_rate,
                min_listing_amount,
                min_purchase_amount,
                min_lockup_duration,
                max_lockup_duration,
                pause_state: PauseState::NotPaused,
            }
        }

        // ============ Listing Functions ============

        /// Create a lockup listing
        /// The seller's Alpha is transferred to this contract and held until purchased or cancelled
        #[ink(message)]
        pub fn create_lockup_listing(
            &mut self,
            hotkey: AccountId,
            netuid: NetUid,
            amount: AlphaAmount,
            price_offset_bps: PriceOffsetBps,
            lockup_duration: BlockAge,
        ) -> Result<LockupListingId, Error> {
            self.ensure_trading_enabled()?;

            let seller = self.env().caller();

            if amount < self.min_listing_amount {
                return Err(Error::AmountTooSmall);
            }

            if price_offset_bps <= -10000 {
                return Err(Error::InvalidPriceOffset);
            }

            if lockup_duration < self.min_lockup_duration {
                return Err(Error::LockupDurationTooShort);
            }

            if lockup_duration > self.max_lockup_duration {
                return Err(Error::LockupDurationTooLong);
            }

            let mut user_listings = self
                .user_lockup_listings
                .get((seller, netuid))
                .unwrap_or_default();
            if user_listings.len() >= MAX_LISTINGS_PER_USER_PER_NETUID {
                return Err(Error::TooManyListings);
            }

            let listing_id = self.next_listing_id;
            let next_id = listing_id.checked_add(1).ok_or(Error::Overflow)?;

            let seller_stake_before = self.get_stake_amount(seller, hotkey, netuid)?;
            if seller_stake_before < amount {
                return Err(Error::InsufficientStake);
            }

            let contract_stake_before = self
                .get_stake_amount(self.env().account_id(), hotkey, netuid)
                .unwrap_or(0);

            // Transfer stake from seller to contract via proxy
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

            // Verify transfer
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

            // Subtensor can round a transfer down by a few rao. The listing holds what
            // actually arrived, so later moves never ask for stake the contract lacks.
            let mut listed_amount = contract_increase;

            // Consolidate stake to contract's hotkey if needed
            if hotkey != self.hotkey {
                let contract_account = self.env().account_id();
                let target_stake_before = self
                    .get_stake_amount(contract_account, self.hotkey, netuid)
                    .unwrap_or(0);

                self.env()
                    .extension()
                    .move_stake(
                        hotkey,
                        self.hotkey,
                        netuid,
                        netuid,
                        AlphaCurrency::from(listed_amount),
                    )
                    .map_err(|_| Error::RuntimeCallFailed)?;

                let target_stake_after =
                    self.get_stake_amount(contract_account, self.hotkey, netuid)?;
                let target_increase = target_stake_after.saturating_sub(target_stake_before);
                if !stake_delta_verified(target_increase, listed_amount) {
                    Self::trap_stake_transfer_not_verified();
                }
                listed_amount = target_increase;
            }

            self.increase_reserved_alpha(netuid, listed_amount)?;

            self.next_listing_id = next_id;

            let listing = LockupListing {
                id: listing_id,
                netuid,
                seller,
                total_amount: listed_amount,
                remaining_amount: listed_amount,
                price_offset_bps,
                lockup_duration,
                fee_rate: self.fee_rate,
                created_at: self.env().block_number(),
            };

            self.lockup_listings
                .insert((netuid, seller, listing_id), &listing);

            user_listings.push(listing_id);
            self.user_lockup_listings
                .insert((seller, netuid), &user_listings);

            self.env().emit_event(LockupListingCreated {
                seller,
                hotkey,
                netuid,
                listing_id,
                amount: listed_amount,
                price_offset_bps,
                lockup_duration,
            });

            Ok(listing_id)
        }

        /// Take a portion (or all) of a lockup listing
        /// Creates a new escrow contract for the buyer
        #[ink(message, payable)]
        pub fn take_lockup_listing(
            &mut self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: LockupListingId,
            amount: AlphaAmount,
        ) -> Result<PurchaseId, Error> {
            self.ensure_trading_enabled()?;

            let buyer = self.env().caller();
            let tao_received = self.env().transferred_value();

            let mut listing = self
                .lockup_listings
                .get((netuid, seller, listing_id))
                .ok_or(Error::ListingNotFound)?;

            // Transfers round down, so a listing can hold slightly less than it was
            // created with. The final remainder can always be taken, even when it is
            // below the minimum purchase; otherwise it could never be filled.
            if amount < self.min_purchase_amount && amount != listing.remaining_amount {
                return Err(Error::AmountTooSmall);
            }

            if amount > listing.remaining_amount {
                return Err(Error::AmountExceedsRemaining);
            }

            // Calculate new remaining amount and validate against Bittensor minimum stake
            let new_remaining = listing
                .remaining_amount
                .checked_sub(amount)
                .ok_or(Error::Overflow)?;

            // Ensure remaining amount is either 0 or above Bittensor's minimum stake
            if new_remaining > 0 && new_remaining < BITTENSOR_MIN_STAKE {
                return Err(Error::AmountBelowMinimumStake);
            }

            // Calculate price at current market + offset
            let market_price = self.get_market_price(listing.netuid)?;
            let executed_price = Self::apply_price_offset(market_price, listing.price_offset_bps)?;
            let price_decimal = Self::price_to_fixed_decimal(executed_price);

            // Calculate TAO required: price * alpha_amount
            let tao_amount = price_decimal.mul(amount).map_err(|_| Error::Overflow)?;

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

            // Calculate unlock block
            let unlock_block = self
                .env()
                .block_number()
                .checked_add(listing.lockup_duration)
                .ok_or(Error::Overflow)?;

            // Get purchase ID for escrow salt
            let purchase_id = self.next_purchase_id;
            self.next_purchase_id = purchase_id.checked_add(1).ok_or(Error::Overflow)?;

            // Instantiate escrow contract
            let escrow_salt = Self::generate_escrow_salt(listing_id, purchase_id);

            let escrow = AlphaLockupRef::new(buyer, netuid, amount, unlock_block, self.hotkey)
                .code_hash(self.escrow_code_hash)
                .endowment(0)
                .salt_bytes(escrow_salt)
                .instantiate();

            let escrow_account = escrow.account_id();
            let contract_account = self.env().account_id();
            let contract_stake_before =
                self.get_stake_amount(contract_account, self.hotkey, netuid)?;
            let escrow_stake_before = self
                .get_stake_amount(escrow_account, self.hotkey, netuid)
                .unwrap_or(0);

            // Transfer Alpha from contract to escrow
            self.env()
                .extension()
                .transfer_stake(
                    escrow_account,
                    self.hotkey,
                    netuid,
                    netuid,
                    AlphaCurrency::from(amount),
                )
                .map_err(|_| Error::RuntimeCallFailed)?;

            let contract_stake_after =
                self.get_stake_amount(contract_account, self.hotkey, netuid)?;
            let escrow_stake_after = self.get_stake_amount(escrow_account, self.hotkey, netuid)?;
            let contract_decrease = contract_stake_before.saturating_sub(contract_stake_after);
            let escrow_increase = escrow_stake_after.saturating_sub(escrow_stake_before);

            if !stake_delta_verified(contract_decrease, amount)
                || !stake_delta_verified(escrow_increase, amount)
            {
                Self::trap_stake_transfer_not_verified();
            }

            // Update reserved alpha
            self.decrease_reserved_alpha(netuid, amount)?;

            // Update listing remaining amount (already validated above)
            listing.remaining_amount = new_remaining;

            if new_remaining == 0 {
                // Remove listing completely
                self.lockup_listings.remove((netuid, seller, listing_id));

                let mut user_listings = self
                    .user_lockup_listings
                    .get((seller, netuid))
                    .unwrap_or_default();
                user_listings.retain(|&id| id != listing_id);

                if user_listings.is_empty() {
                    self.user_lockup_listings.remove((seller, netuid));
                } else {
                    self.user_lockup_listings
                        .insert((seller, netuid), &user_listings);
                }

                // Emit event for indexer tracking
                self.env().emit_event(LockupListingFullyFilled {
                    seller,
                    netuid,
                    listing_id,
                });
            } else {
                // Update listing with new remaining amount
                self.lockup_listings
                    .insert((netuid, seller, listing_id), &listing);
            }

            // Store escrow reference
            self.escrows
                .insert((netuid, listing_id, purchase_id), &escrow_account);

            // Transfer TAO to seller (minus fee)
            self.env()
                .transfer(seller, tao_amount)
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

            self.env().emit_event(LockupListingTaken {
                seller,
                buyer,
                netuid,
                listing_id,
                purchase_id,
                alpha_amount: amount,
                tao_amount,
                executed_price,
                unlock_block,
                escrow_account,
                fee: fee_amount,
            });

            Ok(purchase_id)
        }

        /// Cancel a lockup listing and return remaining Alpha to seller
        #[ink(message)]
        pub fn cancel_lockup_listing(
            &mut self,
            netuid: NetUid,
            listing_id: LockupListingId,
        ) -> Result<(), Error> {
            self.ensure_not_fully_paused()?;

            let seller = self.env().caller();

            let listing = self
                .lockup_listings
                .get((netuid, seller, listing_id))
                .ok_or(Error::ListingNotFound)?;

            self.execute_listing_cancellation(listing, seller, false)
        }

        /// Force-cancel a lockup listing as the contract owner
        #[ink(message)]
        pub fn force_cancel_lockup_listing(
            &mut self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: LockupListingId,
        ) -> Result<(), Error> {
            self.ensure_owner()?;

            let listing = self
                .lockup_listings
                .get((netuid, seller, listing_id))
                .ok_or(Error::ListingNotFound)?;

            let initiator = self.env().caller();

            self.execute_listing_cancellation(listing, initiator, true)
        }

        // ============ Getters ============

        #[ink(message)]
        pub fn get_listing(
            &self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: LockupListingId,
        ) -> Option<LockupListing> {
            self.lockup_listings.get((netuid, seller, listing_id))
        }

        #[ink(message)]
        pub fn get_user_listings(&self, seller: AccountId, netuid: NetUid) -> Vec<LockupListingId> {
            self.user_lockup_listings
                .get((seller, netuid))
                .unwrap_or_default()
        }

        #[ink(message)]
        pub fn get_escrow(
            &self,
            netuid: NetUid,
            listing_id: LockupListingId,
            purchase_id: PurchaseId,
        ) -> Option<AccountId> {
            self.escrows.get((netuid, listing_id, purchase_id))
        }

        #[ink(message)]
        pub fn get_reserved_alpha(&self, netuid: NetUid) -> AlphaAmount {
            self.reserved_alpha_for(netuid)
        }

        #[ink(message)]
        pub fn get_owner(&self) -> AccountId {
            self.owner
        }

        #[ink(message)]
        pub fn get_hotkey(&self) -> AccountId {
            self.hotkey
        }

        #[ink(message)]
        pub fn get_fee_rate(&self) -> FixedDecimal {
            self.fee_rate
        }

        #[ink(message)]
        pub fn get_escrow_code_hash(&self) -> Hash {
            self.escrow_code_hash
        }

        #[ink(message)]
        pub fn get_min_listing_amount(&self) -> AlphaAmount {
            self.min_listing_amount
        }

        #[ink(message)]
        pub fn get_min_purchase_amount(&self) -> AlphaAmount {
            self.min_purchase_amount
        }

        #[ink(message)]
        pub fn get_lockup_duration_limits(&self) -> (BlockAge, BlockAge) {
            (self.min_lockup_duration, self.max_lockup_duration)
        }

        #[ink(message)]
        pub fn get_pause_state(&self) -> PauseState {
            self.pause_state
        }

        /// Estimate the price for taking a portion of a lockup listing at current market price
        /// Returns: (executed_price * 1e9, tao_amount, total_with_fee)
        #[ink(message)]
        pub fn estimate_lockup_price(
            &self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: LockupListingId,
            amount: AlphaAmount,
        ) -> Result<(u64, TaoAmount, TaoAmount), Error> {
            let listing = self
                .lockup_listings
                .get((netuid, seller, listing_id))
                .ok_or(Error::ListingNotFound)?;

            if amount > listing.remaining_amount {
                return Err(Error::AmountExceedsRemaining);
            }

            let market_price = self.get_market_price(listing.netuid)?;
            let executed_price = Self::apply_price_offset(market_price, listing.price_offset_bps)?;
            let price_decimal = Self::price_to_fixed_decimal(executed_price);

            let tao_amount = price_decimal.mul(amount).map_err(|_| Error::Overflow)?;
            let fee_amount = listing
                .fee_rate
                .mul(tao_amount)
                .map_err(|_| Error::Overflow)?;
            let total_required = tao_amount.checked_add(fee_amount).ok_or(Error::Overflow)?;

            Ok((executed_price, tao_amount, total_required))
        }

        // ============ Admin Functions ============

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

        #[ink(message)]
        pub fn update_fee_rate(&mut self, new_rate: u128) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_rate = self.fee_rate;
            let new_rate = FixedDecimal::from_bits(new_rate);
            self.fee_rate = new_rate;

            self.env().emit_event(FeeRateUpdated { old_rate, new_rate });

            Ok(())
        }

        #[ink(message)]
        pub fn update_escrow_code_hash(&mut self, new_hash: Hash) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_hash = self.escrow_code_hash;
            self.escrow_code_hash = new_hash;

            self.env().emit_event(EscrowCodeHashUpdated {
                old_hash: <[u8; 32]>::try_from(old_hash.as_ref()).expect("Hash is 32 bytes"),
                new_hash: <[u8; 32]>::try_from(new_hash.as_ref()).expect("Hash is 32 bytes"),
            });

            Ok(())
        }

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

        #[ink(message)]
        pub fn update_min_purchase_amount(&mut self, new_amount: AlphaAmount) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_amount = self.min_purchase_amount;
            self.min_purchase_amount = new_amount;

            self.env().emit_event(MinPurchaseAmountUpdated {
                old_amount,
                new_amount,
            });

            Ok(())
        }

        #[ink(message)]
        pub fn update_lockup_duration_limits(
            &mut self,
            new_min: BlockAge,
            new_max: BlockAge,
        ) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_min = self.min_lockup_duration;
            let old_max = self.max_lockup_duration;
            self.min_lockup_duration = new_min;
            self.max_lockup_duration = new_max;

            self.env().emit_event(LockupDurationUpdated {
                old_min,
                old_max,
                new_min,
                new_max,
            });

            Ok(())
        }

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

        #[ink(message)]
        pub fn resume(&mut self) -> Result<(), Error> {
            self.ensure_owner()?;

            self.pause_state = PauseState::NotPaused;

            self.env().emit_event(ContractResumed {
                resumed_by: self.env().caller(),
            });

            Ok(())
        }

        #[ink(message)]
        pub fn set_code(&mut self, code_hash: Hash) -> Result<(), Error> {
            self.ensure_owner()?;

            self.env()
                .set_code_hash(&code_hash)
                .map_err(|_| Error::CodeUpgradeFailed)?;

            Ok(())
        }

        // ============ Internal Helpers ============

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
                Ok(Some(info)) => Ok(info.stake_amount()),
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

        fn increase_reserved_alpha(
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

        fn decrease_reserved_alpha(
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

        fn get_market_price(&self, netuid: NetUid) -> Result<u64, Error> {
            self.env()
                .extension()
                .get_alpha_price(netuid)
                .map_err(|_| Error::MarketPriceFetchFailed)
        }

        pub fn apply_price_offset(
            market_price: u64,
            offset_bps: PriceOffsetBps,
        ) -> Result<u64, Error> {
            let base: i64 = 10000;
            let offset = i64::from(offset_bps);
            let multiplier = base.checked_add(offset).ok_or(Error::Overflow)?;

            if multiplier <= 0 {
                return Err(Error::InvalidPriceOffset);
            }

            let multiplier_u128 = u128::try_from(multiplier).map_err(|_| Error::Overflow)?;

            let result = u128::from(market_price)
                .checked_mul(multiplier_u128)
                .ok_or(Error::Overflow)?
                .checked_div(10000)
                .ok_or(Error::Overflow)?;

            u64::try_from(result).map_err(|_| Error::Overflow)
        }

        pub fn price_to_fixed_decimal(scaled_price: u64) -> FixedDecimal {
            let divisor = U64F64::from_num(1_000_000_000u64);
            let price = U64F64::from_num(scaled_price)
                .checked_div(divisor)
                .unwrap_or_default();
            FixedDecimal::from_bits(price.to_bits())
        }

        fn generate_escrow_salt(listing_id: LockupListingId, purchase_id: PurchaseId) -> [u8; 16] {
            let mut salt = [0u8; 16];
            salt[..8].copy_from_slice(&listing_id.to_le_bytes());
            salt[8..].copy_from_slice(&purchase_id.to_le_bytes());
            salt
        }

        fn execute_listing_cancellation(
            &mut self,
            listing: LockupListing,
            initiated_by: AccountId,
            forced: bool,
        ) -> Result<(), Error> {
            let netuid = listing.netuid;
            let seller = listing.seller;
            let listing_id = listing.id;
            let amount = listing.remaining_amount;

            // Remove listing
            self.lockup_listings.remove((netuid, seller, listing_id));

            let mut user_listings = self
                .user_lockup_listings
                .get((seller, netuid))
                .unwrap_or_default();
            user_listings.retain(|&id| id != listing_id);

            if user_listings.is_empty() {
                self.user_lockup_listings.remove((seller, netuid));
            } else {
                self.user_lockup_listings
                    .insert((seller, netuid), &user_listings);
            }

            let contract_account = self.env().account_id();
            let contract_stake_before =
                self.get_stake_amount(contract_account, self.hotkey, netuid)?;
            let seller_stake_before = self
                .get_stake_amount(seller, self.hotkey, netuid)
                .unwrap_or(0);

            // Transfer remaining Alpha back to seller
            self.env()
                .extension()
                .transfer_stake(
                    seller,
                    self.hotkey,
                    netuid,
                    netuid,
                    AlphaCurrency::from(amount),
                )
                .map_err(|_| Error::RuntimeCallFailed)?;

            let contract_stake_after =
                self.get_stake_amount(contract_account, self.hotkey, netuid)?;
            let seller_stake_after = self.get_stake_amount(seller, self.hotkey, netuid)?;
            let contract_decrease = contract_stake_before.saturating_sub(contract_stake_after);
            let seller_increase = seller_stake_after.saturating_sub(seller_stake_before);

            if !stake_delta_verified(contract_decrease, amount)
                || !stake_delta_verified(seller_increase, amount)
            {
                Self::trap_stake_transfer_not_verified();
            }

            // Update reserved alpha
            self.decrease_reserved_alpha(netuid, amount)?;

            if forced {
                self.env().emit_event(LockupListingForceCancelled {
                    seller,
                    initiated_by,
                    netuid,
                    listing_id,
                    amount_returned: amount,
                });
            } else {
                self.env().emit_event(LockupListingCancelled {
                    seller,
                    netuid,
                    listing_id,
                    amount_returned: amount,
                });
            }

            Ok(())
        }

        fn ensure_owner(&self) -> Result<(), Error> {
            if self.env().caller() != self.owner {
                return Err(Error::Unauthorized);
            }
            Ok(())
        }

        fn ensure_not_fully_paused(&self) -> Result<(), Error> {
            if self.pause_state == PauseState::FullyPaused {
                return Err(Error::ContractFullyPaused);
            }
            Ok(())
        }

        fn ensure_trading_enabled(&self) -> Result<(), Error> {
            match self.pause_state {
                PauseState::NotPaused => Ok(()),
                PauseState::TradingPaused => Err(Error::TradingPaused),
                PauseState::FullyPaused => Err(Error::ContractFullyPaused),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use ink::env::test;
        use otc_shared::BittensorEnvironment;

        fn default_accounts() -> test::DefaultAccounts<BittensorEnvironment> {
            test::default_accounts::<BittensorEnvironment>()
        }

        fn create_test_contract() -> LockupListingsContract {
            let accounts = default_accounts();
            let escrow_code_hash = Hash::from([0u8; 32]);
            let fee_rate = 3_000_000_000_000_000_000u128; // 3% as raw bits

            LockupListingsContract::new(
                accounts.alice, // owner
                accounts.bob,   // hotkey
                escrow_code_hash,
                fee_rate,
                1_000_000_000, // min_listing_amount (1 Alpha)
                100_000_000,   // min_purchase_amount (0.1 Alpha)
                7200,          // min_lockup_duration (~1 day)
                2_592_000,     // max_lockup_duration (~30 days)
            )
        }

        #[ink::test]
        fn constructor_works() {
            let accounts = default_accounts();
            let contract = create_test_contract();

            assert_eq!(contract.get_owner(), accounts.alice);
            assert_eq!(contract.get_hotkey(), accounts.bob);
            assert_eq!(contract.get_min_listing_amount(), 1_000_000_000);
            assert_eq!(contract.get_min_purchase_amount(), 100_000_000);
            let (min_duration, max_duration) = contract.get_lockup_duration_limits();
            assert_eq!(min_duration, 7200);
            assert_eq!(max_duration, 2_592_000);
            assert_eq!(contract.get_pause_state(), PauseState::NotPaused);
        }

        #[ink::test]
        fn update_owner_works() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            // Set caller to owner
            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let result = contract.update_owner(accounts.charlie);
            assert!(result.is_ok());
            assert_eq!(contract.get_owner(), accounts.charlie);
        }

        #[ink::test]
        fn update_owner_fails_if_not_owner() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            // Set caller to non-owner
            test::set_caller::<BittensorEnvironment>(accounts.bob);

            let result = contract.update_owner(accounts.charlie);
            assert_eq!(result, Err(Error::Unauthorized));
        }

        #[ink::test]
        fn update_hotkey_works() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let result = contract.update_hotkey(accounts.charlie);
            assert!(result.is_ok());
            assert_eq!(contract.get_hotkey(), accounts.charlie);
        }

        #[ink::test]
        fn update_fee_rate_works() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let new_fee_rate_bits = 5_000_000_000_000_000_000u128; // 5%
            let result = contract.update_fee_rate(new_fee_rate_bits);
            assert!(result.is_ok());
            assert_eq!(
                contract.get_fee_rate(),
                FixedDecimal::from_bits(new_fee_rate_bits)
            );
        }

        #[ink::test]
        fn update_min_listing_amount_works() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let result = contract.update_min_listing_amount(2_000_000_000);
            assert!(result.is_ok());
            assert_eq!(contract.get_min_listing_amount(), 2_000_000_000);
        }

        #[ink::test]
        fn update_min_purchase_amount_works() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let result = contract.update_min_purchase_amount(200_000_000);
            assert!(result.is_ok());
            assert_eq!(contract.get_min_purchase_amount(), 200_000_000);
        }

        #[ink::test]
        fn update_lockup_duration_limits_works() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let result = contract.update_lockup_duration_limits(1000, 5_000_000);
            assert!(result.is_ok());
            let (min_duration, max_duration) = contract.get_lockup_duration_limits();
            assert_eq!(min_duration, 1000);
            assert_eq!(max_duration, 5_000_000);
        }

        #[ink::test]
        fn pause_trading_works() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let reason = b"Maintenance".to_vec();
            let result = contract.pause_trading(reason);
            assert!(result.is_ok());
            assert_eq!(contract.get_pause_state(), PauseState::TradingPaused);
        }

        #[ink::test]
        fn pause_fully_works() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let reason = b"Emergency".to_vec();
            let result = contract.pause_fully(reason);
            assert!(result.is_ok());
            assert_eq!(contract.get_pause_state(), PauseState::FullyPaused);
        }

        #[ink::test]
        fn resume_works() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            // First pause
            contract.pause_trading(b"Test".to_vec()).unwrap();
            assert_eq!(contract.get_pause_state(), PauseState::TradingPaused);

            // Then resume
            let result = contract.resume();
            assert!(result.is_ok());
            assert_eq!(contract.get_pause_state(), PauseState::NotPaused);
        }

        #[ink::test]
        fn pause_requires_owner() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.bob);

            let result = contract.pause_trading(b"Test".to_vec());
            assert_eq!(result, Err(Error::Unauthorized));
        }

        #[ink::test]
        fn get_listing_returns_none_when_not_found() {
            let accounts = default_accounts();
            let contract = create_test_contract();

            let result = contract.get_listing(1, accounts.alice, 1);
            assert!(result.is_none());
        }

        #[ink::test]
        fn get_user_listings_returns_empty_vec() {
            let accounts = default_accounts();
            let contract = create_test_contract();

            let result = contract.get_user_listings(accounts.alice, 1);
            assert!(result.is_empty());
        }

        #[ink::test]
        fn cancel_listing_fails_when_not_found() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let result = contract.cancel_lockup_listing(1, 1);
            assert_eq!(result, Err(Error::ListingNotFound));
        }

        #[ink::test]
        fn force_cancel_requires_owner() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.bob);

            let result = contract.force_cancel_lockup_listing(1, accounts.alice, 1);
            assert_eq!(result, Err(Error::Unauthorized));
        }

        #[ink::test]
        fn update_escrow_code_hash_works() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let new_hash = Hash::from([1u8; 32]);
            let result = contract.update_escrow_code_hash(new_hash);
            assert!(result.is_ok());
        }

        #[ink::test]
        fn operations_blocked_when_fully_paused() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);
            contract.pause_fully(b"Emergency".to_vec()).unwrap();

            // Trying to cancel should fail when fully paused
            let result = contract.cancel_lockup_listing(1, 1);
            assert_eq!(result, Err(Error::ContractFullyPaused));
        }

        #[ink::test]
        fn admin_functions_work_when_paused() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);
            contract.pause_fully(b"Emergency".to_vec()).unwrap();

            // Admin functions should still work
            let result = contract.update_min_listing_amount(5_000_000_000);
            assert!(result.is_ok());
        }

        #[ink::test]
        fn reserved_alpha_tracking_works() {
            let mut contract = create_test_contract();

            // Initially zero
            assert_eq!(contract.get_reserved_alpha(1), 0);

            // Increase reserved alpha
            contract.increase_reserved_alpha(1, 1_000_000_000).unwrap();
            assert_eq!(contract.get_reserved_alpha(1), 1_000_000_000);

            // Increase again
            contract.increase_reserved_alpha(1, 500_000_000).unwrap();
            assert_eq!(contract.get_reserved_alpha(1), 1_500_000_000);

            // Decrease
            let result = contract.decrease_reserved_alpha(1, 300_000_000);
            assert!(result.is_ok());
            assert_eq!(contract.get_reserved_alpha(1), 1_200_000_000);
        }

        #[ink::test]
        fn decrease_reserved_alpha_prevents_underflow() {
            let mut contract = create_test_contract();

            contract.increase_reserved_alpha(1, 100).unwrap();

            let result = contract.decrease_reserved_alpha(1, 200);
            assert_eq!(result, Err(Error::Overflow));
        }

        #[ink::test]
        fn generate_escrow_salt_is_unique() {
            let salt1 = LockupListingsContract::generate_escrow_salt(1, 1);
            let salt2 = LockupListingsContract::generate_escrow_salt(1, 2);
            let salt3 = LockupListingsContract::generate_escrow_salt(2, 1);

            assert_ne!(salt1, salt2);
            assert_ne!(salt1, salt3);
            assert_ne!(salt2, salt3);
        }

        #[ink::test]
        fn bittensor_min_stake_constant_is_correct() {
            assert_eq!(
                BITTENSOR_MIN_STAKE, 2_000_000,
                "Bittensor's DefaultMinStake is 2_000_000 rao (0.002 TAO)"
            );
        }

        #[ink::test]
        fn min_stake_validation_logic() {
            // Test the validation logic for remaining amounts
            let total_amount: u64 = 5_000_000; // 5M rao
            let purchase_amount: u64 = 4_000_000; // 4M rao
            let new_remaining = total_amount - purchase_amount; // 1M rao

            assert!(
                new_remaining > 0 && new_remaining < BITTENSOR_MIN_STAKE,
                "Should fail: 1M < 2M and > 0"
            );

            // If remaining would be exactly 0, it should pass
            let full_purchase = total_amount;
            let zero_remaining = total_amount - full_purchase;
            assert!(!(zero_remaining > 0 && zero_remaining < BITTENSOR_MIN_STAKE));

            // If remaining would be >= BITTENSOR_MIN_STAKE, it should pass
            let smaller_purchase: u64 = 2_500_000;
            let valid_remaining = total_amount - smaller_purchase; // 2.5M rao
            assert!(!(valid_remaining > 0 && valid_remaining < BITTENSOR_MIN_STAKE));
        }
    }
}
