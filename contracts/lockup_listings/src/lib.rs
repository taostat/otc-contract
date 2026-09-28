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
        ActiveHotkeysMapping, EscrowsMapping, LockupListing, LockupListingId,
        LockupListingsMapping, PurchaseId, ReservedAlphaByGenerationMapping,
        ReservedAlphaByHotkeyMapping, ReservedAlphaMapping, StaleListingRecoveryPool,
        StaleListingRecoveryPoolsMapping, UserLockupListingsMapping,
    };
    use alpha_lockup::AlphaLockupRef;
    use fixed::types::U64F64;
    use ink::env::call::FromAccountId;
    use ink::prelude::{boxed::Box, vec::Vec};
    use otc_shared::{
        stake_delta_verified, AlphaAmount, AlphaCurrency, BlockAge, FixedDecimal, NetUid,
        PauseState, PriceOffsetBps, ProxyCall, RuntimeCall, SubnetRegistrationState, SubtensorCall,
        TaoAmount,
    };
    use sp_runtime::MultiAddress;

    /// Maximum number of active lockup listings a user can maintain per subnet
    const MAX_LISTINGS_PER_USER_PER_NETUID: usize = 25;

    /// Bittensor minimum stake transfer amount (2_000_000 rao = 0.002 TAO)
    const BITTENSOR_MIN_STAKE: u64 = 2_000_000;

    enum ListingConsolidationResult {
        AlreadyCurrent,
        Consolidated {
            old_hotkey: AccountId,
            new_hotkey: AccountId,
            amount: AlphaAmount,
        },
        Synced {
            old_hotkey: AccountId,
            new_hotkey: AccountId,
            amount: AlphaAmount,
        },
    }

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

        /// Alpha currently reserved for lockup listings per subnet and custody hotkey
        pub reserved_alpha_by_hotkey: ReservedAlphaByHotkeyMapping,

        /// Active OTC hotkey per subnet; falls back to `hotkey` when unset
        pub active_hotkeys: ActiveHotkeysMapping,

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

        /// Automation account allowed to force-cancel at-risk live listings
        pub risk_canceller: Option<AccountId>,

        /// Alpha currently reserved for lockup listings per subnet generation
        pub reserved_alpha_by_generation: ReservedAlphaByGenerationMapping,

        /// Owner-funded TAO recovery pools for stale open listings
        pub stale_listing_recovery_pools: StaleListingRecoveryPoolsMapping,

        /// TAO balance reserved inside stale listing recovery pools
        pub reserved_recovery_tao: TaoAmount,
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
                reserved_alpha_by_hotkey: Default::default(),
                reserved_alpha_by_generation: Default::default(),
                stale_listing_recovery_pools: Default::default(),
                reserved_recovery_tao: 0,
                active_hotkeys: Default::default(),
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
                risk_canceller: None,
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

            let subnet_generation = self.current_active_subnet_generation(netuid)?;

            let listing_id = self.next_listing_id;
            let next_id = listing_id.checked_add(1).ok_or(Error::Overflow)?;

            let seller_stake_before = self.get_stake_amount(seller, hotkey, netuid)?;
            if seller_stake_before < amount {
                return Err(Error::InsufficientStake);
            }

            self.ensure_available_alpha(seller, netuid, amount)?;

            let contract_account = self.env().account_id();
            self.ensure_available_alpha(contract_account, netuid, self.reserved_alpha_for(netuid))?;

            let contract_stake_before = self
                .get_stake_amount(contract_account, hotkey, netuid)
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

            self.increase_reserved_alpha(netuid, amount)?;
            self.increase_reserved_alpha_for_hotkey(netuid, hotkey, amount)?;
            self.increase_reserved_alpha_for_generation(netuid, subnet_generation, amount)?;

            self.next_listing_id = next_id;

            let mut listing = LockupListing {
                id: listing_id,
                netuid,
                seller,
                total_amount: amount,
                remaining_amount: amount,
                custody_hotkey: hotkey,
                subnet_generation,
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
                custody_hotkey: listing.custody_hotkey,
                netuid,
                listing_id,
                amount,
                subnet_generation,
                price_offset_bps,
                lockup_duration,
            });

            self.env().emit_event(SubnetGenerationRecorded {
                netuid,
                generation: subnet_generation,
            });

            self.refresh_listing_custody_best_effort(&mut listing);

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

            if amount < self.min_purchase_amount {
                return Err(Error::AmountTooSmall);
            }

            let mut listing = self
                .lockup_listings
                .get((netuid, seller, listing_id))
                .ok_or(Error::ListingNotFound)?;

            self.ensure_listing_generation_current(&listing)?;
            self.refresh_listing_custody_best_effort(&mut listing);
            let custody_hotkey = listing.custody_hotkey;

            if amount > listing.remaining_amount {
                return Err(Error::AmountExceedsRemaining);
            }

            let contract_account = self.env().account_id();
            self.ensure_available_alpha(contract_account, netuid, amount)?;

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
            let next_purchase_id = purchase_id.checked_add(1).ok_or(Error::Overflow)?;

            // Instantiate escrow contract
            let escrow_salt = Self::generate_escrow_salt(listing_id, purchase_id);

            let escrow = AlphaLockupRef::new(
                buyer,
                netuid,
                amount,
                unlock_block,
                custody_hotkey,
                listing.subnet_generation,
            )
            .code_hash(self.escrow_code_hash)
            .endowment(0)
            .salt_bytes(escrow_salt)
            .instantiate();

            let escrow_account = escrow.account_id();
            let contract_stake_before =
                self.get_stake_amount(contract_account, custody_hotkey, netuid)?;
            let escrow_stake_before = self
                .get_stake_amount(escrow_account, custody_hotkey, netuid)
                .unwrap_or(0);

            // Transfer Alpha from contract to escrow
            self.env()
                .extension()
                .transfer_stake(
                    escrow_account,
                    custody_hotkey,
                    netuid,
                    netuid,
                    AlphaCurrency::from(amount),
                )
                .map_err(|_| Error::RuntimeCallFailed)?;

            let contract_stake_after =
                self.get_stake_amount(contract_account, custody_hotkey, netuid)?;
            let escrow_stake_after =
                self.get_stake_amount(escrow_account, custody_hotkey, netuid)?;
            let contract_decrease = contract_stake_before.saturating_sub(contract_stake_after);
            let escrow_increase = escrow_stake_after.saturating_sub(escrow_stake_before);

            if !stake_delta_verified(contract_decrease, amount)
                || !stake_delta_verified(escrow_increase, amount)
            {
                Self::trap_stake_transfer_not_verified();
            }

            self.next_purchase_id = next_purchase_id;

            // Update reserved alpha
            self.decrease_reserved_alpha(netuid, amount)?;
            self.decrease_reserved_alpha_for_hotkey(netuid, custody_hotkey, amount)?;
            self.decrease_reserved_alpha_for_generation(netuid, listing.subnet_generation, amount)?;

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
                subnet_generation: listing.subnet_generation,
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
            self.ensure_owner_or_risk_canceller()?;

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
        pub fn get_reserved_alpha_for_hotkey(
            &self,
            netuid: NetUid,
            hotkey: AccountId,
        ) -> AlphaAmount {
            self.reserved_alpha_for_hotkey(netuid, hotkey)
        }

        #[ink(message)]
        pub fn get_reserved_alpha_for_generation(
            &self,
            netuid: NetUid,
            subnet_generation: u64,
        ) -> AlphaAmount {
            self.reserved_alpha_for_generation(netuid, subnet_generation)
        }

        #[ink(message)]
        pub fn get_stale_listing_recovery_pool(
            &self,
            netuid: NetUid,
            subnet_generation: u64,
        ) -> Option<StaleListingRecoveryPool> {
            self.stale_listing_recovery_pools
                .get((netuid, subnet_generation))
        }

        #[ink(message)]
        pub fn get_reserved_recovery_tao(&self) -> TaoAmount {
            self.reserved_recovery_tao
        }

        #[ink(message)]
        pub fn get_owner(&self) -> AccountId {
            self.owner
        }

        #[ink(message)]
        pub fn get_risk_canceller(&self) -> Option<AccountId> {
            self.risk_canceller
        }

        #[ink(message)]
        pub fn get_hotkey(&self) -> AccountId {
            self.hotkey
        }

        #[ink(message)]
        pub fn get_active_hotkey_for_subnet(&self, netuid: NetUid) -> AccountId {
            self.active_hotkey_for_netuid(netuid)
        }

        #[ink(message)]
        pub fn get_current_subnet_registration_state(
            &self,
            netuid: NetUid,
        ) -> Result<SubnetRegistrationState, Error> {
            self.query_subnet_registration_state(netuid)
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

            self.ensure_listing_generation_current(&listing)?;

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

        /// Best-effort consolidation of an open listing to the active company hotkey.
        /// Anyone can trigger this for backend or frontend maintenance.
        #[ink(message)]
        pub fn consolidate_listing_hotkey(
            &mut self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: LockupListingId,
        ) -> Result<(), Error> {
            self.ensure_not_fully_paused()?;

            let mut listing = self
                .lockup_listings
                .get((netuid, seller, listing_id))
                .ok_or(Error::ListingNotFound)?;

            self.ensure_listing_generation_current(&listing)?;

            let result = self.try_consolidate_listing_hotkey(&mut listing)?;
            self.lockup_listings
                .insert((netuid, seller, listing_id), &listing);
            self.emit_listing_consolidation_result(seller, netuid, listing_id, result);

            Ok(())
        }

        /// Sync a purchased escrow directly to the current active hotkey after an OTC hotkey swap.
        /// Anyone can trigger this, but the parent controls the active hotkey registry.
        #[ink(message)]
        pub fn sync_escrow_hotkey(
            &mut self,
            netuid: NetUid,
            listing_id: LockupListingId,
            purchase_id: PurchaseId,
        ) -> Result<(), Error> {
            let initiated_by = self.env().caller();
            let escrow_account = self
                .escrows
                .get((netuid, listing_id, purchase_id))
                .ok_or(Error::EscrowNotFound)?;

            let mut escrow = AlphaLockupRef::from_account_id(escrow_account);
            let old_hotkey = escrow.get_hotkey();
            let new_hotkey = self.active_hotkey_for_netuid(netuid);

            if old_hotkey == new_hotkey {
                return Ok(());
            }

            escrow
                .sync_hotkey_from_parent(old_hotkey, new_hotkey, initiated_by)
                .map_err(|_| Error::EscrowSyncFailed)?;

            self.env().emit_event(EscrowHotkeySynced {
                escrow: escrow_account,
                initiated_by,
                netuid,
                listing_id,
                purchase_id,
                old_hotkey,
                new_hotkey,
            });

            Ok(())
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
        pub fn set_risk_canceller(&mut self, account: AccountId) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_canceller = self.risk_canceller;
            self.risk_canceller = Some(account);

            self.env().emit_event(RiskCancellerUpdated {
                old_canceller,
                new_canceller: self.risk_canceller,
            });

            Ok(())
        }

        #[ink(message)]
        pub fn clear_risk_canceller(&mut self) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_canceller = self.risk_canceller;
            self.risk_canceller = None;

            self.env().emit_event(RiskCancellerUpdated {
                old_canceller,
                new_canceller: None,
            });

            Ok(())
        }

        #[ink(message)]
        pub fn open_stale_listing_recovery_pool(
            &mut self,
            netuid: NetUid,
            subnet_generation: u64,
            tao_amount: TaoAmount,
            evidence_hash: Hash,
        ) -> Result<(), Error> {
            self.ensure_owner()?;

            if tao_amount == 0 {
                return Err(Error::AmountTooSmall);
            }

            if self
                .stale_listing_recovery_pools
                .get((netuid, subnet_generation))
                .is_some()
            {
                return Err(Error::RecoveryPoolAlreadyExists);
            }

            self.ensure_subnet_generation_stale(netuid, subnet_generation)?;

            let alpha_total = self.reserved_alpha_for_generation(netuid, subnet_generation);
            if alpha_total == 0 {
                return Err(Error::NoRecoverableAlpha);
            }

            self.reserve_recovery_tao(tao_amount)?;

            let opened_by = self.env().caller();
            let pool = StaleListingRecoveryPool {
                netuid,
                subnet_generation,
                tao_total: tao_amount,
                tao_remaining: tao_amount,
                alpha_total,
                alpha_remaining: alpha_total,
                evidence_hash,
                opened_by,
                opened_at: self.env().block_number(),
                closed: false,
            };

            self.stale_listing_recovery_pools
                .insert((netuid, subnet_generation), &pool);

            self.env().emit_event(StaleListingRecoveryPoolOpened {
                netuid,
                subnet_generation,
                tao_amount,
                alpha_amount: alpha_total,
                evidence_hash,
                opened_by,
            });

            Ok(())
        }

        #[ink(message)]
        pub fn increase_stale_listing_recovery_pool(
            &mut self,
            netuid: NetUid,
            subnet_generation: u64,
            additional_tao: TaoAmount,
            evidence_hash: Hash,
        ) -> Result<(), Error> {
            self.ensure_owner()?;

            if additional_tao == 0 {
                return Err(Error::AmountTooSmall);
            }

            self.ensure_subnet_generation_stale(netuid, subnet_generation)?;

            let mut pool = self
                .stale_listing_recovery_pools
                .get((netuid, subnet_generation))
                .ok_or(Error::RecoveryPoolNotFound)?;

            if pool.closed {
                return Err(Error::RecoveryPoolClosed);
            }

            let new_tao_total = pool
                .tao_total
                .checked_add(additional_tao)
                .ok_or(Error::Overflow)?;
            let new_tao_remaining = pool
                .tao_remaining
                .checked_add(additional_tao)
                .ok_or(Error::Overflow)?;

            self.reserve_recovery_tao(additional_tao)?;

            pool.tao_total = new_tao_total;
            pool.tao_remaining = new_tao_remaining;
            pool.evidence_hash = evidence_hash;

            self.stale_listing_recovery_pools
                .insert((netuid, subnet_generation), &pool);

            self.env().emit_event(StaleListingRecoveryPoolIncreased {
                netuid,
                subnet_generation,
                additional_tao,
                tao_remaining: pool.tao_remaining,
                evidence_hash,
                increased_by: self.env().caller(),
            });

            Ok(())
        }

        #[ink(message)]
        pub fn recover_stale_listing_tao(
            &mut self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: LockupListingId,
        ) -> Result<(), Error> {
            let listing = self
                .lockup_listings
                .get((netuid, seller, listing_id))
                .ok_or(Error::ListingNotFound)?;

            self.ensure_subnet_generation_stale(listing.netuid, listing.subnet_generation)?;

            let mut pool = self
                .stale_listing_recovery_pools
                .get((listing.netuid, listing.subnet_generation))
                .ok_or(Error::RecoveryPoolNotFound)?;

            if pool.closed {
                return Err(Error::RecoveryPoolClosed);
            }

            let alpha_amount = listing.remaining_amount;
            if alpha_amount == 0 || alpha_amount > pool.alpha_remaining {
                return Err(Error::NoRecoverableAlpha);
            }

            self.ensure_reserved_accounting_can_decrease(&listing)?;

            let tao_amount = Self::stale_listing_recovery_payout(&pool, alpha_amount)?;

            if tao_amount > 0 {
                self.env()
                    .transfer(listing.seller, tao_amount)
                    .map_err(|_| Error::TransferFailed)?;
            }

            pool.alpha_remaining = pool
                .alpha_remaining
                .checked_sub(alpha_amount)
                .ok_or(Error::Overflow)?;
            pool.tao_remaining = pool
                .tao_remaining
                .checked_sub(tao_amount)
                .ok_or(Error::Overflow)?;
            self.reserved_recovery_tao = self
                .reserved_recovery_tao
                .checked_sub(tao_amount)
                .ok_or(Error::Overflow)?;

            if pool.alpha_remaining == 0 {
                pool.tao_remaining = 0;
                pool.closed = true;
            }

            self.stale_listing_recovery_pools
                .insert((listing.netuid, listing.subnet_generation), &pool);

            self.decrease_reserved_alpha(listing.netuid, alpha_amount)?;
            self.decrease_reserved_alpha_for_hotkey(
                listing.netuid,
                listing.custody_hotkey,
                alpha_amount,
            )?;
            self.decrease_reserved_alpha_for_generation(
                listing.netuid,
                listing.subnet_generation,
                alpha_amount,
            )?;
            self.remove_listing_records(listing.netuid, listing.seller, listing.id);

            self.env().emit_event(StaleListingTaoRecovered {
                seller: listing.seller,
                initiated_by: self.env().caller(),
                netuid: listing.netuid,
                listing_id: listing.id,
                subnet_generation: listing.subnet_generation,
                alpha_amount_closed: alpha_amount,
                tao_amount,
            });

            if pool.closed {
                self.env().emit_event(StaleListingRecoveryPoolClosed {
                    netuid: listing.netuid,
                    subnet_generation: listing.subnet_generation,
                    tao_total: pool.tao_total,
                });
            }

            Ok(())
        }

        #[ink(message)]
        pub fn update_hotkey(&mut self, new_hotkey: AccountId) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_hotkey = self.hotkey;
            if new_hotkey == old_hotkey {
                return Err(Error::InvalidHotkey);
            }

            self.hotkey = new_hotkey;

            self.env().emit_event(HotkeyUpdated {
                old_hotkey,
                new_hotkey,
            });

            Ok(())
        }

        #[ink(message)]
        pub fn update_hotkey_for_subnet(
            &mut self,
            netuid: NetUid,
            new_hotkey: AccountId,
        ) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_hotkey = self.active_hotkey_for_netuid(netuid);
            if new_hotkey == old_hotkey {
                return Err(Error::InvalidHotkey);
            }

            self.active_hotkeys.insert(netuid, &new_hotkey);

            self.env().emit_event(SubnetHotkeyUpdated {
                netuid,
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

        fn reserved_alpha_for(&self, netuid: NetUid) -> AlphaAmount {
            self.reserved_alpha.get(netuid).unwrap_or(0)
        }

        fn reserved_alpha_for_hotkey(&self, netuid: NetUid, hotkey: AccountId) -> AlphaAmount {
            self.reserved_alpha_by_hotkey
                .get((netuid, hotkey))
                .unwrap_or(0)
        }

        fn reserved_alpha_for_generation(
            &self,
            netuid: NetUid,
            subnet_generation: u64,
        ) -> AlphaAmount {
            self.reserved_alpha_by_generation
                .get((netuid, subnet_generation))
                .unwrap_or(0)
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

        fn increase_reserved_alpha_for_hotkey(
            &mut self,
            netuid: NetUid,
            hotkey: AccountId,
            amount: AlphaAmount,
        ) -> Result<(), Error> {
            if amount == 0 {
                return Ok(());
            }

            let current = self.reserved_alpha_for_hotkey(netuid, hotkey);
            let new_total = current.checked_add(amount).ok_or(Error::Overflow)?;
            self.reserved_alpha_by_hotkey
                .insert((netuid, hotkey), &new_total);

            Ok(())
        }

        fn increase_reserved_alpha_for_generation(
            &mut self,
            netuid: NetUid,
            subnet_generation: u64,
            amount: AlphaAmount,
        ) -> Result<(), Error> {
            if amount == 0 {
                return Ok(());
            }

            let current = self.reserved_alpha_for_generation(netuid, subnet_generation);
            let new_total = current.checked_add(amount).ok_or(Error::Overflow)?;
            self.reserved_alpha_by_generation
                .insert((netuid, subnet_generation), &new_total);

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

        fn decrease_reserved_alpha_for_hotkey(
            &mut self,
            netuid: NetUid,
            hotkey: AccountId,
            amount: AlphaAmount,
        ) -> Result<(), Error> {
            if amount == 0 {
                return Ok(());
            }

            let current = self.reserved_alpha_for_hotkey(netuid, hotkey);
            let new_total = current.checked_sub(amount).ok_or(Error::Overflow)?;

            if new_total == 0 {
                self.reserved_alpha_by_hotkey.remove((netuid, hotkey));
            } else {
                self.reserved_alpha_by_hotkey
                    .insert((netuid, hotkey), &new_total);
            }

            Ok(())
        }

        fn decrease_reserved_alpha_for_generation(
            &mut self,
            netuid: NetUid,
            subnet_generation: u64,
            amount: AlphaAmount,
        ) -> Result<(), Error> {
            if amount == 0 {
                return Ok(());
            }

            let current = self.reserved_alpha_for_generation(netuid, subnet_generation);
            let new_total = current.checked_sub(amount).ok_or(Error::Overflow)?;

            if new_total == 0 {
                self.reserved_alpha_by_generation
                    .remove((netuid, subnet_generation));
            } else {
                self.reserved_alpha_by_generation
                    .insert((netuid, subnet_generation), &new_total);
            }

            Ok(())
        }

        fn move_reserved_alpha_between_hotkeys(
            &mut self,
            netuid: NetUid,
            old_hotkey: AccountId,
            new_hotkey: AccountId,
            amount: AlphaAmount,
        ) -> Result<(), Error> {
            if old_hotkey == new_hotkey || amount == 0 {
                return Ok(());
            }

            self.decrease_reserved_alpha_for_hotkey(netuid, old_hotkey, amount)?;
            self.increase_reserved_alpha_for_hotkey(netuid, new_hotkey, amount)
        }

        fn reserve_recovery_tao(&mut self, amount: TaoAmount) -> Result<(), Error> {
            if self.unreserved_tao_balance() < amount {
                return Err(Error::InsufficientUnreservedTao);
            }

            self.reserved_recovery_tao = self
                .reserved_recovery_tao
                .checked_add(amount)
                .ok_or(Error::Overflow)?;

            Ok(())
        }

        fn unreserved_tao_balance(&self) -> TaoAmount {
            self.env()
                .balance()
                .saturating_sub(self.reserved_recovery_tao)
        }

        fn ensure_subnet_generation_stale(
            &self,
            netuid: NetUid,
            subnet_generation: u64,
        ) -> Result<(), Error> {
            let state = self.query_subnet_registration_state(netuid)?;
            if !state.exists {
                return Ok(());
            }

            if state.registered_subnet_counter == 0 {
                return Err(Error::InvalidSubnetGeneration);
            }

            if state.registered_subnet_counter == subnet_generation {
                return Err(Error::RecoveryPoolNotStale);
            }

            Ok(())
        }

        fn ensure_reserved_accounting_can_decrease(
            &self,
            listing: &LockupListing,
        ) -> Result<(), Error> {
            let amount = listing.remaining_amount;
            if self.reserved_alpha_for(listing.netuid) < amount
                || self.reserved_alpha_for_hotkey(listing.netuid, listing.custody_hotkey) < amount
                || self.reserved_alpha_for_generation(listing.netuid, listing.subnet_generation)
                    < amount
            {
                return Err(Error::Overflow);
            }

            Ok(())
        }

        fn stale_listing_recovery_payout(
            pool: &StaleListingRecoveryPool,
            alpha_amount: AlphaAmount,
        ) -> Result<TaoAmount, Error> {
            if pool.alpha_remaining == 0 || alpha_amount > pool.alpha_remaining {
                return Err(Error::NoRecoverableAlpha);
            }

            if alpha_amount == pool.alpha_remaining {
                return Ok(pool.tao_remaining);
            }

            let payout = u128::from(pool.tao_remaining)
                .checked_mul(u128::from(alpha_amount))
                .ok_or(Error::Overflow)?
                .checked_div(u128::from(pool.alpha_remaining))
                .ok_or(Error::DivisionByZero)?;
            let payout = u64::try_from(payout).map_err(|_| Error::Overflow)?;

            if payout == 0 {
                return Err(Error::RecoveryPayoutTooSmall);
            }

            Ok(payout)
        }

        fn get_market_price(&self, netuid: NetUid) -> Result<u64, Error> {
            self.env()
                .extension()
                .get_alpha_price(netuid)
                .map_err(|_| Error::MarketPriceFetchFailed)
        }

        fn query_subnet_registration_state(
            &self,
            netuid: NetUid,
        ) -> Result<SubnetRegistrationState, Error> {
            self.env()
                .extension()
                .get_subnet_registration_state(netuid)
                .map_err(|_| Error::SubnetRegistrationQueryFailed)
        }

        fn ensure_available_alpha(
            &self,
            coldkey: AccountId,
            netuid: NetUid,
            amount: AlphaAmount,
        ) -> Result<(), Error> {
            if amount == 0 {
                return Ok(());
            }

            let availability = self
                .env()
                .extension()
                .get_stake_availability(coldkey, netuid)
                .map_err(|_| Error::StakeQueryFailed)?;

            if availability.available < amount {
                return Err(Error::StakeUnavailable);
            }

            Ok(())
        }

        fn trap_stake_transfer_not_verified() -> ! {
            panic!("post-transfer stake verification failed")
        }

        fn current_active_subnet_generation(&self, netuid: NetUid) -> Result<u64, Error> {
            let state = self.query_subnet_registration_state(netuid)?;
            if !state.exists {
                return Err(Error::SubnetNotFound);
            }

            if state.registered_subnet_counter == 0 {
                return Err(Error::InvalidSubnetGeneration);
            }

            Ok(state.registered_subnet_counter)
        }

        fn ensure_listing_generation_current(&self, listing: &LockupListing) -> Result<(), Error> {
            let current_generation = self.current_active_subnet_generation(listing.netuid)?;
            if current_generation != listing.subnet_generation {
                return Err(Error::SubnetGenerationMismatch);
            }

            Ok(())
        }

        fn active_hotkey_for_netuid(&self, netuid: NetUid) -> AccountId {
            self.active_hotkeys.get(netuid).unwrap_or(self.hotkey)
        }

        fn refresh_listing_custody_best_effort(&mut self, listing: &mut LockupListing) {
            let seller = listing.seller;
            let netuid = listing.netuid;
            let listing_id = listing.id;
            let target_hotkey = self.active_hotkey_for_netuid(netuid);

            match self.try_consolidate_listing_hotkey(listing) {
                Ok(ListingConsolidationResult::AlreadyCurrent) => {}
                Ok(result) => {
                    self.lockup_listings
                        .insert((netuid, seller, listing_id), listing);
                    self.emit_listing_consolidation_result(seller, netuid, listing_id, result);
                }
                Err(_) => {
                    self.env().emit_event(ListingHotkeyConsolidationFailed {
                        seller,
                        netuid,
                        listing_id,
                        custody_hotkey: listing.custody_hotkey,
                        target_hotkey,
                        amount: listing.remaining_amount,
                    });
                }
            }
        }

        fn try_consolidate_listing_hotkey(
            &mut self,
            listing: &mut LockupListing,
        ) -> Result<ListingConsolidationResult, Error> {
            let target_hotkey = self.active_hotkey_for_netuid(listing.netuid);
            let old_hotkey = listing.custody_hotkey;
            let amount = listing.remaining_amount;

            if amount == 0 || old_hotkey == target_hotkey {
                return Ok(ListingConsolidationResult::AlreadyCurrent);
            }

            let contract_account = self.env().account_id();
            self.ensure_available_alpha(contract_account, listing.netuid, amount)?;

            let old_hotkey_stake =
                self.get_stake_amount(contract_account, old_hotkey, listing.netuid)?;

            if old_hotkey_stake == 0 {
                self.ensure_active_hotkey_has_reserved_capacity(
                    listing.netuid,
                    target_hotkey,
                    amount,
                )?;
                self.move_reserved_alpha_between_hotkeys(
                    listing.netuid,
                    old_hotkey,
                    target_hotkey,
                    amount,
                )?;
                listing.custody_hotkey = target_hotkey;

                return Ok(ListingConsolidationResult::Synced {
                    old_hotkey,
                    new_hotkey: target_hotkey,
                    amount,
                });
            }

            if old_hotkey_stake < amount {
                return Err(Error::InsufficientStake);
            }

            let target_hotkey_stake_before =
                self.get_stake_amount(contract_account, target_hotkey, listing.netuid)?;

            self.env()
                .extension()
                .move_stake(
                    old_hotkey,
                    target_hotkey,
                    listing.netuid,
                    listing.netuid,
                    AlphaCurrency::from(amount),
                )
                .map_err(|_| Error::RuntimeCallFailed)?;

            let old_hotkey_stake_after =
                self.get_stake_amount(contract_account, old_hotkey, listing.netuid)?;
            let target_hotkey_stake_after =
                self.get_stake_amount(contract_account, target_hotkey, listing.netuid)?;
            let old_decrease = old_hotkey_stake.saturating_sub(old_hotkey_stake_after);
            let target_increase =
                target_hotkey_stake_after.saturating_sub(target_hotkey_stake_before);

            if !stake_delta_verified(old_decrease, amount)
                || !stake_delta_verified(target_increase, amount)
            {
                Self::trap_stake_transfer_not_verified();
            }

            self.move_reserved_alpha_between_hotkeys(
                listing.netuid,
                old_hotkey,
                target_hotkey,
                amount,
            )?;
            listing.custody_hotkey = target_hotkey;

            Ok(ListingConsolidationResult::Consolidated {
                old_hotkey,
                new_hotkey: target_hotkey,
                amount,
            })
        }

        fn ensure_active_hotkey_has_reserved_capacity(
            &self,
            netuid: NetUid,
            active_hotkey: AccountId,
            additional_amount: AlphaAmount,
        ) -> Result<(), Error> {
            let reserved_on_active = self.reserved_alpha_for_hotkey(netuid, active_hotkey);
            let required = reserved_on_active
                .checked_add(additional_amount)
                .ok_or(Error::Overflow)?;
            let active_stake =
                self.get_stake_amount(self.env().account_id(), active_hotkey, netuid)?;

            if active_stake < required {
                return Err(Error::InsufficientStake);
            }

            Ok(())
        }

        fn emit_listing_consolidation_result(
            &self,
            seller: AccountId,
            netuid: NetUid,
            listing_id: LockupListingId,
            result: ListingConsolidationResult,
        ) {
            match result {
                ListingConsolidationResult::AlreadyCurrent => {}
                ListingConsolidationResult::Consolidated {
                    old_hotkey,
                    new_hotkey,
                    amount,
                } => self.env().emit_event(ListingHotkeyConsolidated {
                    seller,
                    netuid,
                    listing_id,
                    old_hotkey,
                    new_hotkey,
                    amount,
                }),
                ListingConsolidationResult::Synced {
                    old_hotkey,
                    new_hotkey,
                    amount,
                } => self.env().emit_event(ListingHotkeySynced {
                    seller,
                    netuid,
                    listing_id,
                    old_hotkey,
                    new_hotkey,
                    amount,
                }),
            }
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

        fn remove_listing_records(
            &mut self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: LockupListingId,
        ) {
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
        }

        fn execute_listing_cancellation(
            &mut self,
            mut listing: LockupListing,
            initiated_by: AccountId,
            forced: bool,
        ) -> Result<(), Error> {
            let netuid = listing.netuid;
            let seller = listing.seller;
            let listing_id = listing.id;
            let amount = listing.remaining_amount;
            self.ensure_listing_generation_current(&listing)?;
            self.refresh_listing_custody_best_effort(&mut listing);
            let custody_hotkey = listing.custody_hotkey;
            let contract_account = self.env().account_id();
            self.ensure_available_alpha(contract_account, netuid, amount)?;

            let contract_stake_before =
                self.get_stake_amount(contract_account, custody_hotkey, netuid)?;
            let seller_stake_before = self
                .get_stake_amount(seller, custody_hotkey, netuid)
                .unwrap_or(0);

            // Transfer remaining Alpha back to seller
            self.env()
                .extension()
                .transfer_stake(
                    seller,
                    custody_hotkey,
                    netuid,
                    netuid,
                    AlphaCurrency::from(amount),
                )
                .map_err(|_| Error::RuntimeCallFailed)?;

            let contract_stake_after =
                self.get_stake_amount(contract_account, custody_hotkey, netuid)?;
            let seller_stake_after = self.get_stake_amount(seller, custody_hotkey, netuid)?;
            let contract_decrease = contract_stake_before.saturating_sub(contract_stake_after);
            let seller_increase = seller_stake_after.saturating_sub(seller_stake_before);

            if !stake_delta_verified(contract_decrease, amount)
                || !stake_delta_verified(seller_increase, amount)
            {
                Self::trap_stake_transfer_not_verified();
            }

            // Update reserved alpha
            self.decrease_reserved_alpha(netuid, amount)?;
            self.decrease_reserved_alpha_for_hotkey(netuid, custody_hotkey, amount)?;
            self.decrease_reserved_alpha_for_generation(netuid, listing.subnet_generation, amount)?;

            // Remove listing only after stake has been returned successfully.
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

        fn ensure_owner_or_risk_canceller(&self) -> Result<(), Error> {
            let caller = self.env().caller();
            if caller == self.owner || self.risk_canceller == Some(caller) {
                return Ok(());
            }

            Err(Error::Unauthorized)
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
        use core::panic::AssertUnwindSafe;
        use ink::env::test;
        use ink::prelude::vec::Vec;
        use ink::scale::{Decode, Encode};
        use otc_shared::runtime::NetUid as RuntimeNetUid;
        use otc_shared::{
            BittensorEnvironment, StakeAvailability, StakeInfo, SubnetRegistrationState,
        };
        use std::cell::RefCell;

        const SUBTENSOR_EXTENSION_ID: u16 = 0;
        const GET_STAKE_INFO_FN_ID: u16 = 0;
        const MOVE_STAKE_FN_ID: u16 = 5;
        const TRANSFER_STAKE_FN_ID: u16 = 6;
        const GET_ALPHA_PRICE_FN_ID: u16 = 15;
        const GET_SUBNET_REGISTRATION_STATE_FN_ID: u16 = 34;
        const GET_STAKE_AVAILABILITY_FN_ID: u16 = 36;
        const TEST_NETUID: NetUid = 1;
        const TEST_LISTING_ID: LockupListingId = 77;
        const TEST_SUBNET_GENERATION: u64 = 1;
        const TEST_LISTING_AMOUNT: AlphaAmount = 1_000_000_000;

        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        struct ExtensionCallCounts {
            subnet_state: u32,
            market_price: u32,
            stake_info: u32,
            stake_availability: u32,
            move_stake: u32,
            transfer_stake: u32,
        }

        #[derive(Clone)]
        struct MockState {
            subnet_exists: bool,
            subnet_generation: u64,
            stake_amount: AlphaAmount,
            stake_availability: StakeAvailability,
            calls: ExtensionCallCounts,
        }

        impl Default for MockState {
            fn default() -> Self {
                Self {
                    subnet_exists: true,
                    subnet_generation: TEST_SUBNET_GENERATION,
                    stake_amount: 0,
                    stake_availability: StakeAvailability {
                        netuid: RuntimeNetUid::from(TEST_NETUID),
                        total: u64::MAX,
                        locked: 0,
                        available: u64::MAX,
                    },
                    calls: ExtensionCallCounts::default(),
                }
            }
        }

        thread_local! {
            static MOCK_STATE: RefCell<MockState> = RefCell::new(MockState::default());
        }

        #[derive(Clone, Copy)]
        struct MockSubtensorExtension;

        impl ink::env::test::ChainExtension for MockSubtensorExtension {
            fn ext_id(&self) -> u16 {
                SUBTENSOR_EXTENSION_ID
            }

            fn call(&mut self, func_id: u16, input: &[u8], output: &mut Vec<u8>) -> u32 {
                match func_id {
                    GET_SUBNET_REGISTRATION_STATE_FN_ID => {
                        let mut input = input;
                        let netuid = NetUid::decode(&mut input).expect("mock decode netuid");

                        MOCK_STATE.with(|state| {
                            let mut state = state.borrow_mut();
                            state.calls.subnet_state += 1;
                            output.extend(Encode::encode(&SubnetRegistrationState {
                                netuid: RuntimeNetUid::from(netuid),
                                exists: state.subnet_exists,
                                registered_subnet_counter: state.subnet_generation,
                            }));
                        });

                        0
                    }
                    GET_ALPHA_PRICE_FN_ID => {
                        MOCK_STATE.with(|state| {
                            state.borrow_mut().calls.market_price += 1;
                        });
                        output.extend(Encode::encode(&1_000_000_000u64));
                        0
                    }
                    GET_STAKE_INFO_FN_ID => {
                        let mut input = input;
                        let hotkey = AccountId::decode(&mut input).expect("mock decode hotkey");
                        let coldkey = AccountId::decode(&mut input).expect("mock decode coldkey");
                        let netuid = NetUid::decode(&mut input).expect("mock decode netuid");

                        MOCK_STATE.with(|state| {
                            let mut state = state.borrow_mut();
                            state.calls.stake_info += 1;
                            if state.stake_amount == 0 {
                                output.extend(Encode::encode(&Option::<StakeInfo>::None));
                            } else {
                                encode_some_stake(
                                    output,
                                    hotkey,
                                    coldkey,
                                    netuid,
                                    state.stake_amount,
                                );
                            }
                        });
                        0
                    }
                    GET_STAKE_AVAILABILITY_FN_ID => {
                        let mut input = input;
                        let _coldkey = AccountId::decode(&mut input).expect("mock decode coldkey");
                        let _netuid = NetUid::decode(&mut input).expect("mock decode netuid");

                        MOCK_STATE.with(|state| {
                            let mut state = state.borrow_mut();
                            state.calls.stake_availability += 1;
                            output.extend(Encode::encode(&state.stake_availability));
                        });

                        0
                    }
                    MOVE_STAKE_FN_ID => {
                        MOCK_STATE.with(|state| {
                            state.borrow_mut().calls.move_stake += 1;
                        });
                        0
                    }
                    TRANSFER_STAKE_FN_ID => {
                        MOCK_STATE.with(|state| {
                            state.borrow_mut().calls.transfer_stake += 1;
                        });
                        0
                    }
                    _ => 1,
                }
            }
        }

        fn default_accounts() -> test::DefaultAccounts<BittensorEnvironment> {
            test::default_accounts::<BittensorEnvironment>()
        }

        fn register_mock_extension(subnet_exists: bool, subnet_generation: u64) {
            MOCK_STATE.with(|state| {
                *state.borrow_mut() = MockState {
                    subnet_exists,
                    subnet_generation,
                    stake_amount: 0,
                    stake_availability: StakeAvailability {
                        netuid: RuntimeNetUid::from(TEST_NETUID),
                        total: u64::MAX,
                        locked: 0,
                        available: u64::MAX,
                    },
                    calls: ExtensionCallCounts::default(),
                };
            });
            test::register_chain_extension(MockSubtensorExtension);
        }

        fn set_mock_subnet_state(subnet_exists: bool, subnet_generation: u64) {
            MOCK_STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.subnet_exists = subnet_exists;
                state.subnet_generation = subnet_generation;
            });
        }

        fn extension_call_counts() -> ExtensionCallCounts {
            MOCK_STATE.with(|state| state.borrow().calls)
        }

        fn set_mock_stake_amount(amount: AlphaAmount) {
            MOCK_STATE.with(|state| {
                state.borrow_mut().stake_amount = amount;
            });
        }

        fn set_stake_availability(total: AlphaAmount, locked: AlphaAmount, available: AlphaAmount) {
            MOCK_STATE.with(|state| {
                state.borrow_mut().stake_availability = StakeAvailability {
                    netuid: RuntimeNetUid::from(TEST_NETUID),
                    total,
                    locked,
                    available,
                };
            });
        }

        fn encode_some_stake(
            output: &mut Vec<u8>,
            hotkey: AccountId,
            coldkey: AccountId,
            netuid: NetUid,
            stake_amount: AlphaAmount,
        ) {
            output.push(1);
            output.extend(Encode::encode(&hotkey));
            output.extend(Encode::encode(&coldkey));
            output.extend(Encode::encode(&ink::scale::Compact(RuntimeNetUid::from(
                netuid,
            ))));
            output.extend(Encode::encode(&ink::scale::Compact(AlphaCurrency::from(
                stake_amount,
            ))));
            output.extend(Encode::encode(&ink::scale::Compact(0u64)));
            output.extend(Encode::encode(&ink::scale::Compact(AlphaCurrency::from(0))));
            output.extend(Encode::encode(&ink::scale::Compact(
                otc_shared::TaoCurrency::from(0),
            )));
            output.extend(Encode::encode(&ink::scale::Compact(0u64)));
            output.extend(Encode::encode(&true));
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

        fn insert_test_listing(
            contract: &mut LockupListingsContract,
            seller: AccountId,
            custody_hotkey: AccountId,
            subnet_generation: u64,
        ) {
            insert_listing_with_amount(
                contract,
                seller,
                custody_hotkey,
                TEST_LISTING_ID,
                TEST_LISTING_AMOUNT,
                subnet_generation,
            );
        }

        fn insert_listing_with_amount(
            contract: &mut LockupListingsContract,
            seller: AccountId,
            custody_hotkey: AccountId,
            listing_id: LockupListingId,
            amount: AlphaAmount,
            subnet_generation: u64,
        ) {
            let listing = LockupListing {
                id: listing_id,
                netuid: TEST_NETUID,
                seller,
                total_amount: amount,
                remaining_amount: amount,
                custody_hotkey,
                subnet_generation,
                price_offset_bps: 0,
                lockup_duration: 7200,
                fee_rate: contract.fee_rate,
                created_at: 1,
            };

            contract
                .lockup_listings
                .insert((TEST_NETUID, seller, listing_id), &listing);

            let user_listings = vec![listing_id];
            contract
                .user_lockup_listings
                .insert((seller, TEST_NETUID), &user_listings);

            contract
                .increase_reserved_alpha(TEST_NETUID, amount)
                .unwrap();
            contract
                .increase_reserved_alpha_for_hotkey(TEST_NETUID, custody_hotkey, amount)
                .unwrap();
            contract
                .increase_reserved_alpha_for_generation(TEST_NETUID, subnet_generation, amount)
                .unwrap();
        }

        fn assert_test_listing_state_unchanged(
            contract: &LockupListingsContract,
            seller: AccountId,
            custody_hotkey: AccountId,
        ) {
            let listing = contract
                .get_listing(TEST_NETUID, seller, TEST_LISTING_ID)
                .expect("listing should remain stored");

            assert_eq!(listing.custody_hotkey, custody_hotkey);
            assert_eq!(listing.subnet_generation, TEST_SUBNET_GENERATION);
            assert_eq!(listing.remaining_amount, TEST_LISTING_AMOUNT);
            assert_eq!(
                contract.get_reserved_alpha(TEST_NETUID),
                TEST_LISTING_AMOUNT
            );
            assert_eq!(
                contract.get_reserved_alpha_for_hotkey(TEST_NETUID, custody_hotkey),
                TEST_LISTING_AMOUNT
            );
            assert_eq!(
                contract.get_reserved_alpha_for_generation(TEST_NETUID, TEST_SUBNET_GENERATION),
                TEST_LISTING_AMOUNT
            );
        }

        #[ink::test]
        fn constructor_works() {
            let accounts = default_accounts();
            let contract = create_test_contract();

            assert_eq!(contract.get_owner(), accounts.alice);
            assert_eq!(contract.get_hotkey(), accounts.bob);
            assert_eq!(contract.get_active_hotkey_for_subnet(1), accounts.bob);
            assert_eq!(contract.get_min_listing_amount(), 1_000_000_000);
            assert_eq!(contract.get_min_purchase_amount(), 100_000_000);
            let (min_duration, max_duration) = contract.get_lockup_duration_limits();
            assert_eq!(min_duration, 7200);
            assert_eq!(max_duration, 2_592_000);
            assert_eq!(contract.get_pause_state(), PauseState::NotPaused);
            assert_eq!(contract.get_risk_canceller(), None);
            assert_eq!(contract.get_reserved_recovery_tao(), 0);
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
        fn owner_can_set_and_clear_risk_canceller() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            assert_eq!(contract.set_risk_canceller(accounts.bob), Ok(()));
            assert_eq!(contract.get_risk_canceller(), Some(accounts.bob));

            assert_eq!(contract.clear_risk_canceller(), Ok(()));
            assert_eq!(contract.get_risk_canceller(), None);
        }

        #[ink::test]
        fn non_owner_cannot_set_risk_canceller() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.bob);

            assert_eq!(
                contract.set_risk_canceller(accounts.charlie),
                Err(Error::Unauthorized)
            );
            assert_eq!(contract.get_risk_canceller(), None);
        }

        #[ink::test]
        fn risk_canceller_can_initiate_force_cancel_but_not_admin_actions() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);
            contract.set_risk_canceller(accounts.bob).unwrap();

            test::set_caller::<BittensorEnvironment>(accounts.charlie);
            assert_eq!(
                contract.force_cancel_lockup_listing(TEST_NETUID, accounts.alice, TEST_LISTING_ID),
                Err(Error::Unauthorized)
            );

            test::set_caller::<BittensorEnvironment>(accounts.bob);
            assert_eq!(
                contract.force_cancel_lockup_listing(TEST_NETUID, accounts.alice, TEST_LISTING_ID),
                Err(Error::ListingNotFound)
            );
            assert_eq!(
                contract.update_hotkey(accounts.charlie),
                Err(Error::Unauthorized)
            );
            assert_eq!(
                contract.open_stale_listing_recovery_pool(
                    TEST_NETUID,
                    TEST_SUBNET_GENERATION,
                    1,
                    Hash::from([1u8; 32]),
                ),
                Err(Error::Unauthorized)
            );
        }

        #[ink::test]
        fn update_hotkey_works() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let result = contract.update_hotkey(accounts.charlie);
            assert!(result.is_ok());
            assert_eq!(contract.get_hotkey(), accounts.charlie);
            assert_eq!(contract.get_active_hotkey_for_subnet(1), accounts.charlie);
        }

        #[ink::test]
        fn update_hotkey_for_subnet_sets_scoped_active_hotkey() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            assert_eq!(
                contract.update_hotkey_for_subnet(1, accounts.charlie),
                Ok(())
            );
            assert_eq!(contract.get_hotkey(), accounts.bob);
            assert_eq!(contract.get_active_hotkey_for_subnet(1), accounts.charlie);
            assert_eq!(contract.get_active_hotkey_for_subnet(2), accounts.bob);
        }

        #[ink::test]
        fn update_hotkey_fails_if_not_owner() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.bob);

            let result = contract.update_hotkey(accounts.charlie);
            assert_eq!(result, Err(Error::Unauthorized));
            assert_eq!(contract.get_hotkey(), accounts.bob);
        }

        #[ink::test]
        fn update_hotkey_rejects_same_hotkey() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let result = contract.update_hotkey(accounts.bob);
            assert_eq!(result, Err(Error::InvalidHotkey));
        }

        #[ink::test]
        fn update_hotkey_for_subnet_rejects_same_active_hotkey() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            assert_eq!(
                contract.update_hotkey_for_subnet(1, accounts.bob),
                Err(Error::InvalidHotkey)
            );
        }

        #[ink::test]
        fn sync_escrow_hotkey_fails_when_escrow_missing() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.django);

            let result = contract.sync_escrow_hotkey(1, 1, 1);
            assert_eq!(result, Err(Error::EscrowNotFound));
        }

        #[ink::test]
        fn create_lockup_listing_rejects_unavailable_seller_alpha_before_transfer() {
            register_mock_extension(true, TEST_SUBNET_GENERATION);
            set_mock_stake_amount(TEST_LISTING_AMOUNT);
            set_stake_availability(TEST_LISTING_AMOUNT, 1, TEST_LISTING_AMOUNT - 1);
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            test::set_caller::<BittensorEnvironment>(accounts.alice);
            let result = contract.create_lockup_listing(
                accounts.bob,
                TEST_NETUID,
                TEST_LISTING_AMOUNT,
                0,
                7200,
            );

            assert_eq!(result, Err(Error::StakeUnavailable));
            assert_eq!(
                contract.get_listing(TEST_NETUID, accounts.alice, TEST_LISTING_ID),
                None
            );
            assert_eq!(contract.get_reserved_alpha(TEST_NETUID), 0);

            let calls = extension_call_counts();
            assert_eq!(calls.transfer_stake, 0);
            assert_eq!(calls.move_stake, 0);
        }

        #[ink::test]
        fn consolidate_listing_hotkey_rejects_unavailable_factory_alpha_before_move() {
            register_mock_extension(true, TEST_SUBNET_GENERATION);
            set_stake_availability(TEST_LISTING_AMOUNT, 1, TEST_LISTING_AMOUNT - 1);
            let accounts = default_accounts();
            let mut contract = create_test_contract();
            insert_test_listing(
                &mut contract,
                accounts.alice,
                accounts.charlie,
                TEST_SUBNET_GENERATION,
            );

            let result =
                contract.consolidate_listing_hotkey(TEST_NETUID, accounts.alice, TEST_LISTING_ID);

            assert_eq!(result, Err(Error::StakeUnavailable));
            assert_test_listing_state_unchanged(&contract, accounts.alice, accounts.charlie);

            let calls = extension_call_counts();
            assert_eq!(calls.move_stake, 0);
            assert_eq!(calls.transfer_stake, 0);
        }

        #[ink::test]
        fn cancel_lockup_listing_rejects_unavailable_factory_alpha_before_transfer() {
            register_mock_extension(true, TEST_SUBNET_GENERATION);
            set_stake_availability(TEST_LISTING_AMOUNT, 1, TEST_LISTING_AMOUNT - 1);
            let accounts = default_accounts();
            let mut contract = create_test_contract();
            insert_test_listing(
                &mut contract,
                accounts.alice,
                accounts.bob,
                TEST_SUBNET_GENERATION,
            );

            test::set_caller::<BittensorEnvironment>(accounts.alice);
            let result = contract.cancel_lockup_listing(TEST_NETUID, TEST_LISTING_ID);

            assert_eq!(result, Err(Error::StakeUnavailable));
            assert_test_listing_state_unchanged(&contract, accounts.alice, accounts.bob);

            let calls = extension_call_counts();
            assert_eq!(calls.transfer_stake, 0);
        }

        #[ink::test]
        fn cancel_lockup_listing_traps_when_post_transfer_verification_fails() {
            register_mock_extension(true, TEST_SUBNET_GENERATION);
            set_mock_stake_amount(TEST_LISTING_AMOUNT);
            let accounts = default_accounts();
            let mut contract = create_test_contract();
            insert_test_listing(
                &mut contract,
                accounts.alice,
                accounts.bob,
                TEST_SUBNET_GENERATION,
            );

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                contract.cancel_lockup_listing(TEST_NETUID, TEST_LISTING_ID)
            }));

            assert!(result.is_err());
            assert_test_listing_state_unchanged(&contract, accounts.alice, accounts.bob);

            let calls = extension_call_counts();
            assert_eq!(calls.transfer_stake, 1);
        }

        #[ink::test]
        fn take_lockup_listing_rejects_unavailable_factory_alpha_before_escrow_side_effects() {
            register_mock_extension(true, TEST_SUBNET_GENERATION);
            set_stake_availability(TEST_LISTING_AMOUNT, 1, TEST_LISTING_AMOUNT - 1);
            let accounts = default_accounts();
            let mut contract = create_test_contract();
            insert_test_listing(
                &mut contract,
                accounts.alice,
                accounts.bob,
                TEST_SUBNET_GENERATION,
            );

            test::set_caller::<BittensorEnvironment>(accounts.charlie);
            test::set_value_transferred::<ink::env::DefaultEnvironment>(TEST_LISTING_AMOUNT.into());
            let result = contract.take_lockup_listing(
                TEST_NETUID,
                accounts.alice,
                TEST_LISTING_ID,
                TEST_LISTING_AMOUNT,
            );

            assert_eq!(result, Err(Error::StakeUnavailable));
            assert_test_listing_state_unchanged(&contract, accounts.alice, accounts.bob);
            assert_eq!(contract.next_purchase_id, 1);

            let calls = extension_call_counts();
            assert_eq!(calls.market_price, 0);
            assert_eq!(calls.transfer_stake, 0);
        }

        #[ink::test]
        fn estimate_lockup_price_rejects_stale_generation_before_market_price() {
            register_mock_extension(true, TEST_SUBNET_GENERATION + 1);
            let accounts = default_accounts();
            let mut contract = create_test_contract();
            insert_test_listing(
                &mut contract,
                accounts.alice,
                accounts.bob,
                TEST_SUBNET_GENERATION,
            );

            let result = contract.estimate_lockup_price(
                TEST_NETUID,
                accounts.alice,
                TEST_LISTING_ID,
                TEST_LISTING_AMOUNT,
            );

            assert_eq!(result, Err(Error::SubnetGenerationMismatch));
            let calls = extension_call_counts();
            assert_eq!(calls.subnet_state, 1);
            assert_eq!(calls.market_price, 0);
            assert_eq!(calls.stake_info, 0);
            assert_eq!(calls.move_stake, 0);
            assert_eq!(calls.transfer_stake, 0);
            assert_test_listing_state_unchanged(&contract, accounts.alice, accounts.bob);
        }

        #[ink::test]
        fn consolidate_listing_hotkey_rejects_stale_generation_before_custody_refresh() {
            register_mock_extension(true, TEST_SUBNET_GENERATION + 1);
            let accounts = default_accounts();
            let mut contract = create_test_contract();
            insert_test_listing(
                &mut contract,
                accounts.alice,
                accounts.bob,
                TEST_SUBNET_GENERATION,
            );

            test::set_caller::<BittensorEnvironment>(accounts.django);

            let result =
                contract.consolidate_listing_hotkey(TEST_NETUID, accounts.alice, TEST_LISTING_ID);

            assert_eq!(result, Err(Error::SubnetGenerationMismatch));
            let calls = extension_call_counts();
            assert_eq!(calls.subnet_state, 1);
            assert_eq!(calls.market_price, 0);
            assert_eq!(calls.stake_info, 0);
            assert_eq!(calls.move_stake, 0);
            assert_eq!(calls.transfer_stake, 0);
            assert_test_listing_state_unchanged(&contract, accounts.alice, accounts.bob);
        }

        #[ink::test]
        fn cancel_listing_rejects_stale_generation_before_transfer() {
            register_mock_extension(true, TEST_SUBNET_GENERATION + 1);
            let accounts = default_accounts();
            let mut contract = create_test_contract();
            insert_test_listing(
                &mut contract,
                accounts.alice,
                accounts.bob,
                TEST_SUBNET_GENERATION,
            );

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let result = contract.cancel_lockup_listing(TEST_NETUID, TEST_LISTING_ID);

            assert_eq!(result, Err(Error::SubnetGenerationMismatch));
            let calls = extension_call_counts();
            assert_eq!(calls.subnet_state, 1);
            assert_eq!(calls.market_price, 0);
            assert_eq!(calls.stake_info, 0);
            assert_eq!(calls.move_stake, 0);
            assert_eq!(calls.transfer_stake, 0);
            assert_test_listing_state_unchanged(&contract, accounts.alice, accounts.bob);
        }

        #[ink::test]
        fn force_cancel_listing_rejects_missing_subnet_before_transfer() {
            register_mock_extension(false, TEST_SUBNET_GENERATION);
            let accounts = default_accounts();
            let mut contract = create_test_contract();
            insert_test_listing(
                &mut contract,
                accounts.bob,
                accounts.charlie,
                TEST_SUBNET_GENERATION,
            );

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let result =
                contract.force_cancel_lockup_listing(TEST_NETUID, accounts.bob, TEST_LISTING_ID);

            assert_eq!(result, Err(Error::SubnetNotFound));
            let calls = extension_call_counts();
            assert_eq!(calls.subnet_state, 1);
            assert_eq!(calls.market_price, 0);
            assert_eq!(calls.stake_info, 0);
            assert_eq!(calls.move_stake, 0);
            assert_eq!(calls.transfer_stake, 0);
            assert_test_listing_state_unchanged(&contract, accounts.bob, accounts.charlie);
        }

        #[ink::test]
        fn take_listing_rejects_stale_generation_before_purchase_side_effects() {
            register_mock_extension(true, TEST_SUBNET_GENERATION + 1);
            let accounts = default_accounts();
            let mut contract = create_test_contract();
            insert_test_listing(
                &mut contract,
                accounts.alice,
                accounts.bob,
                TEST_SUBNET_GENERATION,
            );

            test::set_caller::<BittensorEnvironment>(accounts.charlie);

            let result = contract.take_lockup_listing(
                TEST_NETUID,
                accounts.alice,
                TEST_LISTING_ID,
                TEST_LISTING_AMOUNT,
            );

            assert_eq!(result, Err(Error::SubnetGenerationMismatch));
            let calls = extension_call_counts();
            assert_eq!(calls.subnet_state, 1);
            assert_eq!(calls.market_price, 0);
            assert_eq!(calls.stake_info, 0);
            assert_eq!(calls.move_stake, 0);
            assert_eq!(calls.transfer_stake, 0);
            assert_test_listing_state_unchanged(&contract, accounts.alice, accounts.bob);
        }

        #[ink::test]
        fn open_stale_recovery_pool_requires_stale_generation_reserved_alpha_and_tao() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            register_mock_extension(true, TEST_SUBNET_GENERATION);
            insert_test_listing(
                &mut contract,
                accounts.alice,
                accounts.bob,
                TEST_SUBNET_GENERATION,
            );
            test::set_caller::<BittensorEnvironment>(accounts.alice);

            assert_eq!(
                contract.open_stale_listing_recovery_pool(
                    TEST_NETUID,
                    TEST_SUBNET_GENERATION,
                    1,
                    Hash::from([1u8; 32]),
                ),
                Err(Error::RecoveryPoolNotStale)
            );

            set_mock_subnet_state(true, TEST_SUBNET_GENERATION + 1);
            assert_eq!(
                contract.open_stale_listing_recovery_pool(
                    TEST_NETUID,
                    TEST_SUBNET_GENERATION + 5,
                    1,
                    Hash::from([2u8; 32]),
                ),
                Err(Error::NoRecoverableAlpha)
            );

            assert_eq!(
                contract.open_stale_listing_recovery_pool(
                    TEST_NETUID,
                    TEST_SUBNET_GENERATION,
                    TEST_LISTING_AMOUNT + 1,
                    Hash::from([3u8; 32]),
                ),
                Err(Error::InsufficientUnreservedTao)
            );
        }

        #[ink::test]
        fn increasing_recovery_pool_checks_overflow_before_reserving_tao() {
            register_mock_extension(true, TEST_SUBNET_GENERATION + 1);
            let accounts = default_accounts();
            let mut contract = create_test_contract();
            contract.reserved_recovery_tao = 123;
            contract.stale_listing_recovery_pools.insert(
                (TEST_NETUID, TEST_SUBNET_GENERATION),
                &StaleListingRecoveryPool {
                    netuid: TEST_NETUID,
                    subnet_generation: TEST_SUBNET_GENERATION,
                    tao_total: u64::MAX,
                    tao_remaining: u64::MAX,
                    alpha_total: TEST_LISTING_AMOUNT,
                    alpha_remaining: TEST_LISTING_AMOUNT,
                    evidence_hash: Hash::from([7u8; 32]),
                    opened_by: accounts.alice,
                    opened_at: 1,
                    closed: false,
                },
            );

            test::set_caller::<BittensorEnvironment>(accounts.alice);

            assert_eq!(
                contract.increase_stale_listing_recovery_pool(
                    TEST_NETUID,
                    TEST_SUBNET_GENERATION,
                    1,
                    Hash::from([8u8; 32]),
                ),
                Err(Error::Overflow)
            );
            assert_eq!(contract.get_reserved_recovery_tao(), 123);
        }

        #[ink::test]
        fn stale_recovery_payout_prorates_and_assigns_final_dust() {
            let accounts = default_accounts();
            let mut pool = StaleListingRecoveryPool {
                netuid: TEST_NETUID,
                subnet_generation: TEST_SUBNET_GENERATION,
                tao_total: 101,
                tao_remaining: 101,
                alpha_total: 100,
                alpha_remaining: 100,
                evidence_hash: Hash::from([7u8; 32]),
                opened_by: accounts.alice,
                opened_at: 1,
                closed: false,
            };

            assert_eq!(
                LockupListingsContract::stale_listing_recovery_payout(&pool, 60),
                Ok(60)
            );

            pool.tao_remaining = 41;
            pool.alpha_remaining = 40;
            assert_eq!(
                LockupListingsContract::stale_listing_recovery_payout(&pool, 40),
                Ok(41)
            );
        }

        #[ink::test]
        fn recover_stale_listing_tao_removes_listing_and_updates_accounting() {
            register_mock_extension(true, TEST_SUBNET_GENERATION + 1);
            let accounts = default_accounts();
            let mut contract = create_test_contract();
            insert_listing_with_amount(
                &mut contract,
                accounts.alice,
                accounts.bob,
                1,
                60,
                TEST_SUBNET_GENERATION,
            );
            contract.stale_listing_recovery_pools.insert(
                (TEST_NETUID, TEST_SUBNET_GENERATION),
                &StaleListingRecoveryPool {
                    netuid: TEST_NETUID,
                    subnet_generation: TEST_SUBNET_GENERATION,
                    tao_total: 0,
                    tao_remaining: 0,
                    alpha_total: 60,
                    alpha_remaining: 60,
                    evidence_hash: Hash::from([7u8; 32]),
                    opened_by: accounts.alice,
                    opened_at: 1,
                    closed: false,
                },
            );

            test::set_caller::<BittensorEnvironment>(accounts.django);
            assert_eq!(
                contract.recover_stale_listing_tao(TEST_NETUID, accounts.alice, 1),
                Ok(())
            );
            assert_eq!(contract.get_listing(TEST_NETUID, accounts.alice, 1), None);
            assert_eq!(contract.get_reserved_alpha(TEST_NETUID), 0);
            assert_eq!(
                contract.get_reserved_alpha_for_hotkey(TEST_NETUID, accounts.bob),
                0
            );
            assert_eq!(
                contract.get_reserved_alpha_for_generation(TEST_NETUID, TEST_SUBNET_GENERATION),
                0
            );
            assert_eq!(contract.get_reserved_recovery_tao(), 0);

            let pool = contract
                .get_stale_listing_recovery_pool(TEST_NETUID, TEST_SUBNET_GENERATION)
                .expect("closed pool should remain queryable");
            assert!(pool.closed);
            assert_eq!(pool.tao_remaining, 0);
            assert_eq!(pool.alpha_remaining, 0);
        }

        #[ink::test]
        fn recover_stale_listing_tao_rejects_live_generation_without_mutation() {
            register_mock_extension(true, TEST_SUBNET_GENERATION);
            let accounts = default_accounts();
            let mut contract = create_test_contract();
            insert_test_listing(
                &mut contract,
                accounts.alice,
                accounts.bob,
                TEST_SUBNET_GENERATION,
            );

            assert_eq!(
                contract.recover_stale_listing_tao(TEST_NETUID, accounts.alice, TEST_LISTING_ID),
                Err(Error::RecoveryPoolNotStale)
            );
            assert_test_listing_state_unchanged(&contract, accounts.alice, accounts.bob);
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
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            // Initially zero
            assert_eq!(contract.get_reserved_alpha(1), 0);
            assert_eq!(contract.get_reserved_alpha_for_hotkey(1, accounts.bob), 0);
            assert_eq!(contract.get_reserved_alpha_for_generation(1, 1), 0);

            // Increase reserved alpha
            contract.increase_reserved_alpha(1, 1_000_000_000).unwrap();
            contract
                .increase_reserved_alpha_for_hotkey(1, accounts.bob, 1_000_000_000)
                .unwrap();
            contract
                .increase_reserved_alpha_for_generation(1, 1, 1_000_000_000)
                .unwrap();
            assert_eq!(contract.get_reserved_alpha(1), 1_000_000_000);
            assert_eq!(
                contract.get_reserved_alpha_for_hotkey(1, accounts.bob),
                1_000_000_000
            );
            assert_eq!(
                contract.get_reserved_alpha_for_generation(1, 1),
                1_000_000_000
            );

            // Increase again
            contract.increase_reserved_alpha(1, 500_000_000).unwrap();
            contract
                .increase_reserved_alpha_for_hotkey(1, accounts.bob, 500_000_000)
                .unwrap();
            contract
                .increase_reserved_alpha_for_generation(1, 1, 500_000_000)
                .unwrap();
            assert_eq!(contract.get_reserved_alpha(1), 1_500_000_000);
            assert_eq!(
                contract.get_reserved_alpha_for_hotkey(1, accounts.bob),
                1_500_000_000
            );
            assert_eq!(
                contract.get_reserved_alpha_for_generation(1, 1),
                1_500_000_000
            );

            // Decrease
            let result = contract.decrease_reserved_alpha(1, 300_000_000);
            assert!(result.is_ok());
            let result = contract.decrease_reserved_alpha_for_hotkey(1, accounts.bob, 300_000_000);
            assert!(result.is_ok());
            let result = contract.decrease_reserved_alpha_for_generation(1, 1, 300_000_000);
            assert!(result.is_ok());
            assert_eq!(contract.get_reserved_alpha(1), 1_200_000_000);
            assert_eq!(
                contract.get_reserved_alpha_for_hotkey(1, accounts.bob),
                1_200_000_000
            );
            assert_eq!(
                contract.get_reserved_alpha_for_generation(1, 1),
                1_200_000_000
            );
        }

        #[ink::test]
        fn decrease_reserved_alpha_prevents_underflow() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            contract.increase_reserved_alpha(1, 100).unwrap();
            contract
                .increase_reserved_alpha_for_hotkey(1, accounts.bob, 100)
                .unwrap();
            contract
                .increase_reserved_alpha_for_generation(1, 1, 100)
                .unwrap();

            let result = contract.decrease_reserved_alpha(1, 200);
            assert_eq!(result, Err(Error::Overflow));
            let result = contract.decrease_reserved_alpha_for_hotkey(1, accounts.bob, 200);
            assert_eq!(result, Err(Error::Overflow));
            let result = contract.decrease_reserved_alpha_for_generation(1, 1, 200);
            assert_eq!(result, Err(Error::Overflow));
        }

        #[ink::test]
        fn reserved_alpha_can_move_between_hotkeys() {
            let accounts = default_accounts();
            let mut contract = create_test_contract();

            contract
                .increase_reserved_alpha_for_hotkey(1, accounts.bob, 500)
                .unwrap();
            assert_eq!(contract.get_reserved_alpha_for_hotkey(1, accounts.bob), 500);
            assert_eq!(
                contract.get_reserved_alpha_for_hotkey(1, accounts.charlie),
                0
            );

            contract
                .move_reserved_alpha_between_hotkeys(1, accounts.bob, accounts.charlie, 300)
                .unwrap();

            assert_eq!(contract.get_reserved_alpha_for_hotkey(1, accounts.bob), 200);
            assert_eq!(
                contract.get_reserved_alpha_for_hotkey(1, accounts.charlie),
                300
            );
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
