#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub mod errors;
pub mod events;
pub mod types;

pub use otc_shared::BittensorEnvironment;

/// Factory for spot and lockup listings. Each listing is its own `listing_vault`,
/// created from the code registry's current vault code; the factory holds no
/// Alpha. (TAO offers, which are pooled, live in `otc`.)
///
/// The factory keeps the `(netuid, seller, listing_id) -> vault` index and the
/// settings vaults read: the validator hotkey per subnet, the fee recipient and
/// the trading pause. A compromised factory could block takes or redirect future
/// fees, but never touch a seller's Alpha: cancelling and sweeping a vault never
/// consult the factory.
#[ink::contract(env = otc_shared::BittensorEnvironment)]
mod lockup_listings {
    use crate::errors::Error;
    use crate::events::*;
    use crate::types::{Config, ListingId, ListingKey, UserListings};
    use ink::prelude::vec::Vec;
    use ink::storage::Mapping;
    use otc_shared::events::Upgraded;
    use otc_shared::{
        check_upgrade, AlphaAmount, BlockAge, FixedDecimal, NetUid, PriceOffsetBps, UpgradeError,
        UpgradePolicy, VaultFactory, MAX_FEE_RATE, MAX_PAUSE,
    };

    /// Storage layout version this code expects; see `ensure_migrated`.
    pub const STORAGE_VERSION: u16 = 1;

    /// Maximum number of open listings, spot and lockup together, a seller can
    /// have per subnet
    pub const MAX_LISTINGS_PER_USER_PER_NETUID: usize = 25;

    /// Longest the owner can make spot listings wait before their seller can
    /// cancel: about a week of 12-second blocks. The seller doesn't choose this
    /// delay, so it is bounded like a pause.
    pub const MAX_LISTING_AGE: BlockAge = 50_400;

    /// Root storage. Upgrades keep this struct exactly as it is; add new state in
    /// `Lazy` or `Mapping` fields with explicit storage keys instead.
    #[ink(storage)]
    pub struct LockupListings {
        owner: AccountId,
        /// Proposed new owner, who must accept before taking over
        pending_owner: Option<AccountId>,
        /// Code registry for vault code and this factory's upgrades
        registry: AccountId,
        /// Storage layout version, see `STORAGE_VERSION`
        version: u16,
        next_listing_id: ListingId,
        /// Per-seller counter that salts vault addresses, so they are predictable
        seller_nonces: Mapping<AccountId, u64>,
        listings: Mapping<ListingKey, AccountId>,
        /// The reverse index, so a vault can report its listing closed
        vault_listings: Mapping<AccountId, ListingKey>,
        user_listings: UserListings,
        /// Validator hotkey for subnets without an override
        default_hotkey: AccountId,
        validator_hotkeys: Mapping<NetUid, AccountId>,
        fee_rate: FixedDecimal,
        fee_recipient: AccountId,
        min_listing_amount: AlphaAmount,
        min_purchase_amount: AlphaAmount,
        min_lockup_duration: BlockAge,
        max_lockup_duration: BlockAge,
        /// How long a spot listing must stay open after activation before the
        /// seller can cancel it
        min_listing_age: BlockAge,
        /// Subnets where new listings are frozen
        frozen_subnets: Mapping<NetUid, ()>,
        /// New listings and takes are paused before this block
        paused_until: BlockNumber,
    }

    /// What a new vault is created with, besides the factory's settings
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct VaultTerms {
        seller: AccountId,
        listing_id: ListingId,
        netuid: NetUid,
        hotkey: AccountId,
        amount: AlphaAmount,
        price_offset_bps: PriceOffsetBps,
        min_price: u64,
        lockup_duration: Option<BlockAge>,
        cancel_delay: BlockAge,
    }

    impl LockupListings {
        #[allow(clippy::too_many_arguments)]
        #[ink(constructor)]
        pub fn new(
            owner: AccountId,
            registry: AccountId,
            default_hotkey: AccountId,
            fee_rate: u128,
            fee_recipient: AccountId,
            min_listing_amount: AlphaAmount,
            min_purchase_amount: AlphaAmount,
            min_lockup_duration: BlockAge,
            max_lockup_duration: BlockAge,
            min_listing_age: BlockAge,
        ) -> Result<Self, Error> {
            Self::validate_fee_rate(fee_rate)?;
            Self::validate_lockup_limits(min_lockup_duration, max_lockup_duration)?;
            Self::validate_listing_age(min_listing_age)?;

            Ok(Self {
                owner,
                pending_owner: None,
                registry,
                version: STORAGE_VERSION,
                next_listing_id: 1,
                seller_nonces: Mapping::default(),
                listings: Mapping::default(),
                vault_listings: Mapping::default(),
                user_listings: Mapping::default(),
                default_hotkey,
                validator_hotkeys: Mapping::default(),
                fee_rate: FixedDecimal::from_bits(fee_rate),
                fee_recipient,
                min_listing_amount,
                min_purchase_amount,
                min_lockup_duration,
                max_lockup_duration,
                min_listing_age,
                frozen_subnets: Mapping::default(),
                paused_until: 0,
            })
        }

        /// Create a Pending listing for `amount` Alpha on `hotkey` and return its id
        /// and vault. The seller pays the vault's storage deposit, then funds it by
        /// transferring the stake to the vault and calling `activate`, in one batch.
        ///
        /// With a `lockup_duration`, buyers receive the Alpha in an escrow that
        /// locks it for that many blocks; within the factory's limits, and the
        /// seller can cancel at any time. Without one it is a spot listing: buyers
        /// receive the Alpha directly, and the seller can cancel only once the
        /// listing has been open for `min_listing_age`.
        ///
        /// `min_price` is the seller's floor (TAO per Alpha, scaled by 1e9; 0 for
        /// none): takes below it fail, which limits spot-price manipulation.
        #[ink(message)]
        pub fn create_listing(
            &mut self,
            netuid: NetUid,
            hotkey: AccountId,
            amount: AlphaAmount,
            price_offset_bps: PriceOffsetBps,
            min_price: u64,
            lockup_duration: Option<BlockAge>,
        ) -> Result<(ListingId, AccountId), Error> {
            use otc_shared::upgrade::LISTING_VAULT;

            self.ensure_migrated();

            if self.is_paused() {
                return Err(Error::TradingPaused);
            }
            if amount < self.min_listing_amount {
                return Err(Error::AmountTooSmall);
            }
            if price_offset_bps <= -10_000 {
                return Err(Error::InvalidPriceOffset);
            }
            if self.frozen_subnets.contains(netuid) {
                return Err(Error::SubnetFrozen);
            }
            let cancel_delay = match lockup_duration {
                Some(duration) => {
                    if duration < self.min_lockup_duration {
                        return Err(Error::LockupDurationTooShort);
                    }
                    if duration > self.max_lockup_duration {
                        return Err(Error::LockupDurationTooLong);
                    }
                    0
                }
                None => self.min_listing_age,
            };

            let seller = self.env().caller();
            let mut seller_listings = self.user_listings.get((seller, netuid)).unwrap_or_default();
            if seller_listings.len() >= MAX_LISTINGS_PER_USER_PER_NETUID {
                return Err(Error::TooManyListings);
            }

            let listing_id = self.next_listing_id;
            self.next_listing_id = listing_id.checked_add(1).ok_or(Error::Overflow)?;
            let nonce = self.seller_nonces.get(seller).unwrap_or(0);
            self.seller_nonces
                .insert(seller, &nonce.checked_add(1).ok_or(Error::Overflow)?);

            let code_hash = self
                .registry_current(LISTING_VAULT)?
                .ok_or(Error::NoCurrentCode)?;
            let vault = self.instantiate_vault(
                code_hash,
                VaultTerms {
                    seller,
                    listing_id,
                    netuid,
                    hotkey,
                    amount,
                    price_offset_bps,
                    min_price,
                    lockup_duration,
                    cancel_delay,
                },
                Self::vault_salt(seller, nonce),
            )?;

            let key = (netuid, seller, listing_id);
            self.listings.insert(key, &vault);
            self.vault_listings.insert(vault, &key);
            seller_listings.push(listing_id);
            self.user_listings
                .insert((seller, netuid), &seller_listings);

            self.env().emit_event(ListingCreated {
                seller,
                netuid,
                listing_id,
                vault,
                hotkey,
                amount,
                price_offset_bps,
                min_price,
                lockup_duration,
            });

            Ok((listing_id, vault))
        }

        /// Drop a closed listing from the index when its vault's own report did not
        /// arrive, for example because the report ran out of gas. Anyone can call
        /// this; it fails while the vault is still pending or open.
        #[ink(message)]
        pub fn prune_listing(
            &mut self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: ListingId,
        ) -> Result<(), Error> {
            self.ensure_migrated();

            let key = (netuid, seller, listing_id);
            let vault = self.listings.get(key).ok_or(Error::ListingNotFound)?;
            if self.vault_is_open(vault)? {
                return Err(Error::ListingStillOpen);
            }

            self.remove_listing(vault, key);
            Ok(())
        }

        // ============ Views ============

        /// The vault of an open listing
        #[ink(message)]
        pub fn get_listing(
            &self,
            netuid: NetUid,
            seller: AccountId,
            listing_id: ListingId,
        ) -> Option<AccountId> {
            self.listings.get((netuid, seller, listing_id))
        }

        /// Which open listing a vault holds
        #[ink(message)]
        pub fn get_vault_listing(&self, vault: AccountId) -> Option<ListingKey> {
            self.vault_listings.get(vault)
        }

        /// A seller's open listings on a subnet
        #[ink(message)]
        pub fn get_user_listings(&self, seller: AccountId, netuid: NetUid) -> Vec<ListingId> {
            self.user_listings.get((seller, netuid)).unwrap_or_default()
        }

        /// The nonce that will salt the seller's next vault, for predicting its
        /// address
        #[ink(message)]
        pub fn get_seller_nonce(&self, seller: AccountId) -> u64 {
            self.seller_nonces.get(seller).unwrap_or(0)
        }

        #[ink(message)]
        pub fn get_config(&self) -> Config {
            Config {
                owner: self.owner,
                registry: self.registry,
                default_hotkey: self.default_hotkey,
                fee_rate: self.fee_rate,
                fee_recipient: self.fee_recipient,
                min_listing_amount: self.min_listing_amount,
                min_purchase_amount: self.min_purchase_amount,
                min_lockup_duration: self.min_lockup_duration,
                max_lockup_duration: self.max_lockup_duration,
                min_listing_age: self.min_listing_age,
                paused_until: self.paused_until,
            }
        }

        #[ink(message)]
        pub fn is_subnet_frozen(&self, netuid: NetUid) -> bool {
            self.frozen_subnets.contains(netuid)
        }

        #[ink(message)]
        pub fn get_owner(&self) -> AccountId {
            self.owner
        }

        #[ink(message)]
        pub fn get_pending_owner(&self) -> Option<AccountId> {
            self.pending_owner
        }

        #[ink(message)]
        pub fn get_registry(&self) -> AccountId {
            self.registry
        }

        #[ink(message)]
        pub fn get_version(&self) -> u16 {
            self.version
        }

        // ============ Admin ============

        /// Propose `new_owner`, who takes over by calling `accept_ownership`. A new
        /// proposal replaces the previous one; `None` withdraws it.
        #[ink(message)]
        pub fn transfer_ownership(&mut self, new_owner: Option<AccountId>) -> Result<(), Error> {
            self.ensure_owner()?;

            self.pending_owner = new_owner;
            self.env()
                .emit_event(OwnershipTransferProposed { new_owner });
            Ok(())
        }

        #[ink(message)]
        pub fn accept_ownership(&mut self) -> Result<(), Error> {
            let caller = self.env().caller();
            if self.pending_owner != Some(caller) {
                return Err(Error::Unauthorized);
            }

            let old_owner = self.owner;
            self.owner = caller;
            self.pending_owner = None;
            self.env().emit_event(OwnerChanged {
                old_owner,
                new_owner: caller,
            });
            Ok(())
        }

        /// Set the validator hotkey for subnets without an override. Open listings
        /// follow through `consolidate()` on their vaults.
        #[ink(message)]
        pub fn set_default_hotkey(&mut self, new_hotkey: AccountId) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_hotkey = self.default_hotkey;
            self.default_hotkey = new_hotkey;
            self.env().emit_event(DefaultHotkeyChanged {
                old_hotkey,
                new_hotkey,
            });
            Ok(())
        }

        /// Override the validator hotkey for `netuid`, or remove the override with
        /// `None`
        #[ink(message)]
        pub fn set_validator_hotkey(
            &mut self,
            netuid: NetUid,
            hotkey: Option<AccountId>,
        ) -> Result<(), Error> {
            self.ensure_owner()?;

            match hotkey {
                Some(hotkey) => {
                    self.validator_hotkeys.insert(netuid, &hotkey);
                }
                None => self.validator_hotkeys.remove(netuid),
            }
            self.env()
                .emit_event(ValidatorHotkeyChanged { netuid, hotkey });
            Ok(())
        }

        /// Set the fee rate for new listings, as U64F64 bits, at most `MAX_FEE_RATE`.
        /// Existing listings keep the rate they were created with.
        #[ink(message)]
        pub fn set_fee_rate(&mut self, new_rate: u128) -> Result<(), Error> {
            self.ensure_owner()?;
            Self::validate_fee_rate(new_rate)?;

            let old_rate = self.fee_rate;
            self.fee_rate = FixedDecimal::from_bits(new_rate);
            self.env().emit_event(FeeRateChanged {
                old_rate,
                new_rate: self.fee_rate,
            });
            Ok(())
        }

        #[ink(message)]
        pub fn set_fee_recipient(&mut self, new_recipient: AccountId) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_recipient = self.fee_recipient;
            self.fee_recipient = new_recipient;
            self.env().emit_event(FeeRecipientChanged {
                old_recipient,
                new_recipient,
            });
            Ok(())
        }

        #[ink(message)]
        pub fn set_min_listing_amount(&mut self, new_amount: AlphaAmount) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_amount = self.min_listing_amount;
            self.min_listing_amount = new_amount;
            self.env().emit_event(MinListingAmountChanged {
                old_amount,
                new_amount,
            });
            Ok(())
        }

        /// Set the minimum purchase for new listings
        #[ink(message)]
        pub fn set_min_purchase_amount(&mut self, new_amount: AlphaAmount) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_amount = self.min_purchase_amount;
            self.min_purchase_amount = new_amount;
            self.env().emit_event(MinPurchaseAmountChanged {
                old_amount,
                new_amount,
            });
            Ok(())
        }

        #[ink(message)]
        pub fn set_lockup_duration_limits(
            &mut self,
            min: BlockAge,
            max: BlockAge,
        ) -> Result<(), Error> {
            self.ensure_owner()?;
            Self::validate_lockup_limits(min, max)?;

            self.min_lockup_duration = min;
            self.max_lockup_duration = max;
            self.env()
                .emit_event(LockupDurationLimitsChanged { min, max });
            Ok(())
        }

        /// Set how long new spot listings must stay open before the seller can
        /// cancel them, at most `MAX_LISTING_AGE`
        #[ink(message)]
        pub fn set_min_listing_age(&mut self, new_age: BlockAge) -> Result<(), Error> {
            self.ensure_owner()?;
            Self::validate_listing_age(new_age)?;

            let old_age = self.min_listing_age;
            self.min_listing_age = new_age;
            self.env()
                .emit_event(MinListingAgeChanged { old_age, new_age });
            Ok(())
        }

        /// Freeze or unfreeze new listings on `netuid`, for example while the subnet
        /// is at risk of deregistration. Open listings are not affected.
        #[ink(message)]
        pub fn set_subnet_frozen(
            &mut self,
            netuid: NetUid,
            frozen: bool,
            reason: Vec<u8>,
        ) -> Result<(), Error> {
            self.ensure_owner()?;

            if frozen {
                self.frozen_subnets.insert(netuid, &());
            } else {
                self.frozen_subnets.remove(netuid);
            }
            self.env().emit_event(SubnetFrozenChanged {
                netuid,
                frozen,
                reason,
            });
            Ok(())
        }

        /// Pause new listings and takes for `blocks` blocks, at most `MAX_PAUSE`.
        /// Calling it again restarts the pause from now. Cancels, sweeps and claims
        /// are never paused.
        #[ink(message)]
        pub fn pause(&mut self, blocks: BlockAge) -> Result<(), Error> {
            self.ensure_owner()?;

            if blocks == 0 || blocks > MAX_PAUSE {
                return Err(Error::InvalidPauseDuration);
            }
            let until = self
                .env()
                .block_number()
                .checked_add(blocks)
                .ok_or(Error::Overflow)?;
            self.paused_until = until;
            self.env().emit_event(TradingPaused {
                until,
                paused_by: self.env().caller(),
            });
            Ok(())
        }

        #[ink(message)]
        pub fn resume(&mut self) -> Result<(), Error> {
            self.ensure_owner()?;

            self.paused_until = 0;
            self.env().emit_event(TradingResumed {
                resumed_by: self.env().caller(),
            });
            Ok(())
        }

        /// Install `code_hash`, which must be the registry's current factory code.
        /// Only the owner can upgrade. The new code migrates storage on its next
        /// call, or when anyone calls `migrate`.
        #[ink(message)]
        pub fn upgrade(&mut self, code_hash: Hash) -> Result<(), Error> {
            use otc_shared::upgrade::LOCKUP_FACTORY;

            self.ensure_owner()?;
            let current = self.registry_current(LOCKUP_FACTORY)?;
            check_upgrade(true, UpgradePolicy::ConsentOnly, code_hash, current, None)
                .map_err(Self::upgrade_error)?;

            self.install_code(code_hash)?;

            self.env().emit_event(Upgraded {
                code_hash,
                initiated_by: self.env().caller(),
            });
            Ok(())
        }

        /// Bring storage up to this code's `STORAGE_VERSION`. Every state-changing
        /// message does this first; anyone can also call it directly after an
        /// upgrade.
        #[ink(message)]
        pub fn migrate(&mut self) {
            self.ensure_migrated();
        }

        // ============ Internal ============

        fn is_paused(&self) -> bool {
            self.env().block_number() < self.paused_until
        }

        fn remove_listing(&mut self, vault: AccountId, key: ListingKey) {
            let (netuid, seller, listing_id) = key;

            self.vault_listings.remove(vault);
            self.listings.remove(key);
            let mut seller_listings = self.user_listings.get((seller, netuid)).unwrap_or_default();
            seller_listings.retain(|&id| id != listing_id);
            if seller_listings.is_empty() {
                self.user_listings.remove((seller, netuid));
            } else {
                self.user_listings
                    .insert((seller, netuid), &seller_listings);
            }

            self.env().emit_event(ListingClosed {
                seller,
                netuid,
                listing_id,
                vault,
            });
        }

        fn validate_fee_rate(fee_rate: u128) -> Result<(), Error> {
            if fee_rate > MAX_FEE_RATE {
                return Err(Error::FeeRateTooHigh);
            }
            Ok(())
        }

        fn validate_listing_age(age: BlockAge) -> Result<(), Error> {
            if age > MAX_LISTING_AGE {
                return Err(Error::ListingAgeTooLong);
            }
            Ok(())
        }

        fn validate_lockup_limits(min: BlockAge, max: BlockAge) -> Result<(), Error> {
            if min > max {
                return Err(Error::InvalidLockupDurationLimits);
            }
            Ok(())
        }

        /// The seller and their nonce: unique per vault, and predictable, so a
        /// frontend can fund the vault in the same batch that creates it.
        fn vault_salt(seller: AccountId, nonce: u64) -> [u8; 40] {
            let mut salt = [0u8; 40];
            salt[..32].copy_from_slice(seller.as_ref());
            salt[32..].copy_from_slice(&nonce.to_le_bytes());
            salt
        }

        fn upgrade_error(error: UpgradeError) -> Error {
            match error {
                UpgradeError::NoCurrentCode => Error::NoCurrentCode,
                // The owner always has consent, so only a wrong hash remains.
                _ => Error::NotCurrentCode,
            }
        }

        /// Storage only migrates upwards. Older code runs on newer storage without
        /// knowing its added fields, so a rollback after a storage change must ship
        /// as new code with a higher version, never as the old code.
        fn ensure_migrated(&mut self) {
            if self.version < STORAGE_VERSION {
                // Versions newer than 1 initialise their added storage here.
                self.version = STORAGE_VERSION;
            }
        }

        fn ensure_owner(&self) -> Result<(), Error> {
            if self.env().caller() != self.owner {
                return Err(Error::Unauthorized);
            }
            Ok(())
        }

        #[cfg(not(test))]
        fn registry_current(&self, kind: otc_shared::CodeKind) -> Result<Option<Hash>, Error> {
            use code_registry::CodeRegistryRef;
            use ink::codegen::TraitCallBuilder;
            use ink::env::call::FromAccountId;

            Ok(CodeRegistryRef::from_account_id(self.registry)
                .call()
                .current(kind)
                .try_invoke()
                .map_err(|_| Error::RegistryQueryFailed)?
                .map_err(|_| Error::RegistryQueryFailed)?
                .map(|code| code.code_hash))
        }

        #[cfg(not(test))]
        fn instantiate_vault(
            &self,
            code_hash: Hash,
            terms: VaultTerms,
            salt: [u8; 40],
        ) -> Result<AccountId, Error> {
            use ink::ToAccountId;
            use listing_vault::ListingVaultRef;

            // The arguments are part of the vault's address, so they hold nothing
            // the seller can't predict: no listing id, and no block number.
            let vault = ListingVaultRef::new(
                terms.seller,
                terms.netuid,
                terms.hotkey,
                terms.amount,
                terms.price_offset_bps,
                terms.min_price,
                self.fee_rate,
                self.min_purchase_amount,
                terms.lockup_duration,
                terms.cancel_delay,
                self.registry,
            )
            .code_hash(code_hash)
            .endowment(0)
            .salt_bytes(salt)
            .try_instantiate()
            .map_err(|_| Error::VaultInstantiationFailed)?
            .map_err(|_| Error::VaultInstantiationFailed)?;

            Ok(vault.to_account_id())
        }

        #[cfg(not(test))]
        fn install_code(&mut self, code_hash: Hash) -> Result<(), Error> {
            self.env()
                .set_code_hash(&code_hash)
                .map_err(|_| Error::CodeUpgradeFailed)
        }

        /// Whether the vault still has a pending or open listing. A vault that no
        /// longer exists has been closed for good.
        #[cfg(not(test))]
        fn vault_is_open(&self, vault: AccountId) -> Result<bool, Error> {
            use ink::codegen::TraitCallBuilder;
            use ink::env::call::FromAccountId;
            use listing_vault::{ListingVaultRef, Status};

            if !self.env().is_contract(&vault) {
                return Ok(false);
            }
            let status = ListingVaultRef::from_account_id(vault)
                .call()
                .get_status()
                .try_invoke()
                .map_err(|_| Error::VaultQueryFailed)?
                .map_err(|_| Error::VaultQueryFailed)?;
            Ok(status != Status::Closed)
        }

        // ink!'s off-chain test environment cannot call or instantiate other
        // contracts or replace code, so unit tests answer registry queries and
        // record vaults and upgrades here; localnet integration tests cover the
        // real calls.
        #[cfg(test)]
        fn registry_current(&self, kind: otc_shared::CodeKind) -> Result<Option<Hash>, Error> {
            Ok(tests::registry_current(kind))
        }

        #[cfg(test)]
        fn instantiate_vault(
            &self,
            code_hash: Hash,
            terms: VaultTerms,
            salt: [u8; 40],
        ) -> Result<AccountId, Error> {
            Ok(tests::record_vault(code_hash, terms, salt))
        }

        #[cfg(test)]
        fn install_code(&mut self, code_hash: Hash) -> Result<(), Error> {
            tests::record_installed_code(code_hash);
            Ok(())
        }

        #[cfg(test)]
        fn vault_is_open(&self, vault: AccountId) -> Result<bool, Error> {
            Ok(tests::vault_is_open(vault))
        }
    }

    impl VaultFactory for LockupListings {
        #[ink(message)]
        fn trading_paused(&self) -> bool {
            self.is_paused()
        }

        #[ink(message)]
        fn fee_recipient(&self) -> AccountId {
            self.fee_recipient
        }

        #[ink(message)]
        fn validator_hotkey(&self, netuid: NetUid) -> AccountId {
            self.validator_hotkeys
                .get(netuid)
                .unwrap_or(self.default_hotkey)
        }

        #[ink(message)]
        fn listing_closed(&mut self) {
            let vault = self.env().caller();
            if let Some(key) = self.vault_listings.get(vault) {
                self.remove_listing(vault, key);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use ink::env::test;
        use ink::env::Environment;
        use ink::scale::Decode;
        use otc_shared::upgrade::{LISTING_VAULT, LOCKUP_FACTORY};
        use otc_shared::{BittensorEnvironment, CodeKind};
        use std::cell::RefCell;
        use std::collections::BTreeMap;

        type TestAccountId = <BittensorEnvironment as Environment>::AccountId;

        const NETUID: NetUid = 1;
        const MIN_LISTING: AlphaAmount = 1_000_000_000;
        const MIN_PURCHASE: AlphaAmount = 100_000_000;
        const MIN_LOCKUP: BlockAge = 10;
        const MAX_LOCKUP: BlockAge = 1_000;
        const MIN_AGE: BlockAge = 30;
        /// 0.5%, as U64F64 bits
        const FEE_RATE: u128 = 92_233_720_368_547_758;
        const NOW: BlockNumber = 100;

        thread_local! {
            static REGISTRY: RefCell<BTreeMap<CodeKind, Hash>> = const { RefCell::new(BTreeMap::new()) };
            static VAULTS: RefCell<Vec<(Hash, VaultTerms, [u8; 40])>> = const { RefCell::new(Vec::new()) };
            static INSTALLED_CODE: RefCell<Option<Hash>> = const { RefCell::new(None) };
            /// Vaults that report `Closed` or no longer exist
            static CLOSED_VAULTS: RefCell<Vec<TestAccountId>> = const { RefCell::new(Vec::new()) };
        }

        pub(super) fn vault_is_open(vault: TestAccountId) -> bool {
            CLOSED_VAULTS.with(|closed| !closed.borrow().contains(&vault))
        }

        fn close_vault(vault: TestAccountId) {
            CLOSED_VAULTS.with(|closed| closed.borrow_mut().push(vault));
        }

        pub(super) fn registry_current(kind: CodeKind) -> Option<Hash> {
            REGISTRY.with(|registry| registry.borrow().get(&kind).copied())
        }

        fn set_registry_current(kind: CodeKind, code: Option<Hash>) {
            REGISTRY.with(|registry| match code {
                Some(code) => registry.borrow_mut().insert(kind, code),
                None => registry.borrow_mut().remove(&kind),
            });
        }

        /// Records the vault and returns a distinct account for it.
        pub(super) fn record_vault(
            code_hash: Hash,
            terms: VaultTerms,
            salt: [u8; 40],
        ) -> TestAccountId {
            VAULTS.with(|vaults| {
                let mut vaults = vaults.borrow_mut();
                vaults.push((code_hash, terms, salt));
                let mut account = [0xAA; 32];
                account[31] = u8::try_from(vaults.len()).expect("few vaults");
                TestAccountId::from(account)
            })
        }

        fn vaults() -> Vec<(Hash, VaultTerms, [u8; 40])> {
            VAULTS.with(|vaults| vaults.borrow().clone())
        }

        pub(super) fn record_installed_code(code_hash: Hash) {
            INSTALLED_CODE.with(|installed| *installed.borrow_mut() = Some(code_hash));
        }

        fn installed_code() -> Option<Hash> {
            INSTALLED_CODE.with(|installed| *installed.borrow())
        }

        fn accounts() -> test::DefaultAccounts<BittensorEnvironment> {
            test::default_accounts::<BittensorEnvironment>()
        }

        fn hash(byte: u8) -> Hash {
            Hash::from([byte; 32])
        }

        fn registry_account() -> TestAccountId {
            TestAccountId::from([0x55; 32])
        }

        fn fee_collector() -> TestAccountId {
            TestAccountId::from([0x66; 32])
        }

        fn set_caller(account: TestAccountId) {
            test::set_caller::<BittensorEnvironment>(account);
        }

        fn last_event<E: Decode>() -> E {
            let events = test::recorded_events().collect::<Vec<_>>();
            E::decode(&mut &events.last().expect("an event").data[..]).expect("decode event")
        }

        fn new_factory(
            fee_rate: u128,
            min_lockup: BlockAge,
            max_lockup: BlockAge,
        ) -> Result<LockupListings, Error> {
            let accounts = accounts();
            LockupListings::new(
                accounts.alice,
                registry_account(),
                accounts.charlie,
                fee_rate,
                fee_collector(),
                MIN_LISTING,
                MIN_PURCHASE,
                min_lockup,
                max_lockup,
                MIN_AGE,
            )
        }

        /// Roles: alice owns the factory, bob sells, charlie is the default
        /// validator hotkey, django a third party, and eve the seller's hotkey.
        fn factory() -> LockupListings {
            REGISTRY.with(|registry| registry.borrow_mut().clear());
            set_registry_current(LISTING_VAULT, Some(hash(0xA1)));
            VAULTS.with(|vaults| vaults.borrow_mut().clear());
            INSTALLED_CODE.with(|installed| *installed.borrow_mut() = None);
            CLOSED_VAULTS.with(|closed| closed.borrow_mut().clear());
            test::set_block_number::<BittensorEnvironment>(NOW);
            set_caller(accounts().alice);

            new_factory(FEE_RATE, MIN_LOCKUP, MAX_LOCKUP).expect("valid config")
        }

        fn create(factory: &mut LockupListings) -> Result<(ListingId, AccountId), Error> {
            let accounts = accounts();
            set_caller(accounts.bob);
            factory.create_listing(NETUID, accounts.eve, MIN_LISTING, 100, 7, Some(20))
        }

        #[ink::test]
        fn constructor_validates_the_config() {
            let accounts = accounts();

            assert!(matches!(
                new_factory(MAX_FEE_RATE + 1, MIN_LOCKUP, MAX_LOCKUP),
                Err(Error::FeeRateTooHigh)
            ));
            assert!(matches!(
                new_factory(FEE_RATE, MAX_LOCKUP + 1, MAX_LOCKUP),
                Err(Error::InvalidLockupDurationLimits)
            ));
            assert!(matches!(
                LockupListings::new(
                    accounts.alice,
                    registry_account(),
                    accounts.charlie,
                    FEE_RATE,
                    fee_collector(),
                    MIN_LISTING,
                    MIN_PURCHASE,
                    MIN_LOCKUP,
                    MAX_LOCKUP,
                    MAX_LISTING_AGE + 1,
                ),
                Err(Error::ListingAgeTooLong)
            ));

            let factory = new_factory(MAX_FEE_RATE, MIN_LOCKUP, MAX_LOCKUP).expect("valid");
            let config = factory.get_config();
            assert_eq!(config.owner, accounts.alice);
            assert_eq!(config.registry, registry_account());
            assert_eq!(config.default_hotkey, accounts.charlie);
            assert_eq!(config.fee_rate.to_bits(), MAX_FEE_RATE);
            assert_eq!(config.fee_recipient, fee_collector());
            assert_eq!(config.paused_until, 0);
            assert_eq!(factory.get_version(), STORAGE_VERSION);
        }

        #[ink::test]
        fn create_listing_makes_a_vault_from_the_registry_and_indexes_it() {
            let accounts = accounts();
            let mut factory = factory();

            let (listing_id, vault) = create(&mut factory).expect("create");
            assert_eq!(listing_id, 1);

            let (code_hash, terms, salt) = vaults()[0];
            assert_eq!(code_hash, hash(0xA1));
            assert_eq!(
                terms,
                VaultTerms {
                    seller: accounts.bob,
                    listing_id: 1,
                    netuid: NETUID,
                    hotkey: accounts.eve,
                    amount: MIN_LISTING,
                    price_offset_bps: 100,
                    min_price: 7,
                    lockup_duration: Some(20),
                    cancel_delay: 0,
                }
            );
            assert_eq!(&salt[..32], AsRef::<[u8]>::as_ref(&accounts.bob));
            assert_eq!(salt[32..], 0u64.to_le_bytes());

            assert_eq!(factory.get_listing(NETUID, accounts.bob, 1), Some(vault));
            assert_eq!(
                factory.get_vault_listing(vault),
                Some((NETUID, accounts.bob, 1))
            );
            assert_eq!(factory.get_user_listings(accounts.bob, NETUID), vec![1]);
            assert_eq!(factory.get_seller_nonce(accounts.bob), 1);

            let created = last_event::<ListingCreated>();
            assert_eq!(created.seller, accounts.bob);
            assert_eq!(created.vault, vault);
            assert_eq!(created.min_price, 7);

            // The next listing gets a new id and a new salt.
            let (second_id, second_vault) = create(&mut factory).expect("create");
            assert_eq!(second_id, 2);
            assert_ne!(second_vault, vault);
            assert_eq!(vaults()[1].2[32..], 1u64.to_le_bytes());
        }

        #[ink::test]
        fn create_listing_checks_the_terms() {
            let accounts = accounts();
            let mut factory = factory();
            set_caller(accounts.bob);

            assert_eq!(
                factory.create_listing(NETUID, accounts.eve, MIN_LISTING - 1, 0, 0, Some(20)),
                Err(Error::AmountTooSmall)
            );
            assert_eq!(
                factory.create_listing(NETUID, accounts.eve, MIN_LISTING, -10_000, 0, Some(20)),
                Err(Error::InvalidPriceOffset)
            );
            assert_eq!(
                factory.create_listing(
                    NETUID,
                    accounts.eve,
                    MIN_LISTING,
                    0,
                    0,
                    Some(MIN_LOCKUP - 1)
                ),
                Err(Error::LockupDurationTooShort)
            );
            assert_eq!(
                factory.create_listing(
                    NETUID,
                    accounts.eve,
                    MIN_LISTING,
                    0,
                    0,
                    Some(MAX_LOCKUP + 1)
                ),
                Err(Error::LockupDurationTooLong)
            );

            set_registry_current(LISTING_VAULT, None);
            assert_eq!(create(&mut factory), Err(Error::NoCurrentCode));
            assert!(vaults().is_empty());
        }

        #[ink::test]
        fn spot_listings_wait_the_minimum_age_before_cancelling() {
            let accounts = accounts();
            let mut factory = factory();
            set_caller(accounts.bob);

            // Spot listings take no lockup, so the lockup limits don't apply.
            let (listing_id, _) = factory
                .create_listing(NETUID, accounts.eve, MIN_LISTING, 0, 0, None)
                .expect("create");
            let (_, terms, _) = vaults()[0];
            assert_eq!(terms.lockup_duration, None);
            assert_eq!(terms.cancel_delay, MIN_AGE);
            assert_eq!(last_event::<ListingCreated>().lockup_duration, None);
            assert_eq!(listing_id, 1);

            set_caller(accounts.alice);
            assert_eq!(
                factory.set_min_listing_age(MAX_LISTING_AGE + 1),
                Err(Error::ListingAgeTooLong)
            );
            assert_eq!(factory.set_min_listing_age(5), Ok(()));
            assert_eq!(factory.get_config().min_listing_age, 5);
            set_caller(accounts.bob);
            factory
                .create_listing(NETUID, accounts.eve, MIN_LISTING, 0, 0, None)
                .expect("create");
            assert_eq!(vaults()[1].1.cancel_delay, 5);
        }

        #[ink::test]
        fn frozen_subnets_take_no_new_listings() {
            let accounts = accounts();
            let mut factory = factory();

            set_caller(accounts.django);
            assert_eq!(
                factory.set_subnet_frozen(NETUID, true, Vec::new()),
                Err(Error::Unauthorized)
            );

            set_caller(accounts.alice);
            assert_eq!(
                factory.set_subnet_frozen(NETUID, true, b"at risk".to_vec()),
                Ok(())
            );
            assert!(factory.is_subnet_frozen(NETUID));
            assert_eq!(create(&mut factory), Err(Error::SubnetFrozen));
            set_caller(accounts.bob);
            assert_eq!(
                factory.create_listing(NETUID, accounts.eve, MIN_LISTING, 0, 0, None),
                Err(Error::SubnetFrozen)
            );
            // Other subnets are unaffected.
            assert!(factory
                .create_listing(NETUID + 1, accounts.eve, MIN_LISTING, 0, 0, None)
                .is_ok());

            set_caller(accounts.alice);
            assert_eq!(factory.set_subnet_frozen(NETUID, false, Vec::new()), Ok(()));
            assert!(!factory.is_subnet_frozen(NETUID));
            assert!(create(&mut factory).is_ok());
        }

        #[ink::test]
        fn sellers_are_limited_per_subnet() {
            let accounts = accounts();
            let mut factory = factory();
            for _ in 0..MAX_LISTINGS_PER_USER_PER_NETUID {
                create(&mut factory).expect("create");
            }
            assert_eq!(create(&mut factory), Err(Error::TooManyListings));

            // Closing a listing frees a slot.
            let vault = factory
                .get_listing(NETUID, accounts.bob, 3)
                .expect("listing 3");
            set_caller(vault);
            factory.listing_closed();
            assert!(create(&mut factory).is_ok());
        }

        #[ink::test]
        fn only_a_listing_vault_can_close_its_listing() {
            let accounts = accounts();
            let mut factory = factory();
            let (_, vault) = create(&mut factory).expect("create");
            let (_, other) = create(&mut factory).expect("create");

            // Anyone else is ignored.
            set_caller(accounts.django);
            factory.listing_closed();
            assert_eq!(factory.get_user_listings(accounts.bob, NETUID), vec![1, 2]);

            set_caller(vault);
            factory.listing_closed();
            assert_eq!(factory.get_listing(NETUID, accounts.bob, 1), None);
            assert_eq!(factory.get_vault_listing(vault), None);
            assert_eq!(factory.get_user_listings(accounts.bob, NETUID), vec![2]);
            let closed = last_event::<ListingClosed>();
            assert_eq!(closed.vault, vault);
            assert_eq!(closed.listing_id, 1);

            // A second report changes nothing.
            factory.listing_closed();
            assert_eq!(
                factory.get_vault_listing(other),
                Some((NETUID, accounts.bob, 2))
            );
        }

        #[ink::test]
        fn anyone_prunes_a_closed_listing_whose_report_never_arrived() {
            let accounts = accounts();
            let mut factory = factory();
            let (listing_id, vault) = create(&mut factory).expect("create");

            set_caller(accounts.django);
            assert_eq!(
                factory.prune_listing(NETUID, accounts.bob, listing_id),
                Err(Error::ListingStillOpen)
            );

            // The vault closed, but its report to the factory failed.
            close_vault(vault);
            assert_eq!(
                factory.prune_listing(NETUID, accounts.bob, listing_id),
                Ok(())
            );
            assert_eq!(factory.get_listing(NETUID, accounts.bob, listing_id), None);
            assert_eq!(factory.get_vault_listing(vault), None);
            assert!(factory.get_user_listings(accounts.bob, NETUID).is_empty());
            assert_eq!(last_event::<ListingClosed>().vault, vault);

            assert_eq!(
                factory.prune_listing(NETUID, accounts.bob, listing_id),
                Err(Error::ListingNotFound)
            );
        }

        #[ink::test]
        fn pauses_expire_and_can_be_renewed_or_lifted() {
            let accounts = accounts();
            let mut factory = factory();

            set_caller(accounts.django);
            assert_eq!(factory.pause(10), Err(Error::Unauthorized));

            set_caller(accounts.alice);
            assert_eq!(factory.pause(0), Err(Error::InvalidPauseDuration));
            assert_eq!(
                factory.pause(MAX_PAUSE + 1),
                Err(Error::InvalidPauseDuration)
            );
            assert_eq!(factory.pause(10), Ok(()));
            assert!(factory.trading_paused());
            assert_eq!(last_event::<TradingPaused>().until, NOW + 10);
            assert_eq!(create(&mut factory), Err(Error::TradingPaused));

            test::set_block_number::<BittensorEnvironment>(NOW + 10);
            assert!(!factory.trading_paused());
            assert!(create(&mut factory).is_ok());

            set_caller(accounts.alice);
            assert_eq!(factory.pause(MAX_PAUSE), Ok(()));
            assert!(factory.trading_paused());
            assert_eq!(factory.resume(), Ok(()));
            assert!(!factory.trading_paused());
        }

        #[ink::test]
        fn validator_hotkeys_default_and_override_per_subnet() {
            let accounts = accounts();
            let mut factory = factory();
            assert_eq!(factory.validator_hotkey(NETUID), accounts.charlie);

            set_caller(accounts.alice);
            assert_eq!(
                factory.set_validator_hotkey(NETUID, Some(accounts.django)),
                Ok(())
            );
            assert_eq!(factory.validator_hotkey(NETUID), accounts.django);
            assert_eq!(factory.validator_hotkey(NETUID + 1), accounts.charlie);

            assert_eq!(factory.set_default_hotkey(accounts.eve), Ok(()));
            assert_eq!(factory.validator_hotkey(NETUID + 1), accounts.eve);

            assert_eq!(factory.set_validator_hotkey(NETUID, None), Ok(()));
            assert_eq!(factory.validator_hotkey(NETUID), accounts.eve);
        }

        #[ink::test]
        fn only_the_owner_changes_settings_and_the_fee_is_capped() {
            let accounts = accounts();
            let mut factory = factory();

            set_caller(accounts.django);
            assert_eq!(factory.set_fee_rate(0), Err(Error::Unauthorized));
            assert_eq!(
                factory.set_fee_recipient(accounts.django),
                Err(Error::Unauthorized)
            );
            assert_eq!(factory.set_min_listing_amount(1), Err(Error::Unauthorized));
            assert_eq!(factory.set_min_purchase_amount(1), Err(Error::Unauthorized));
            assert_eq!(
                factory.set_lockup_duration_limits(1, 2),
                Err(Error::Unauthorized)
            );
            assert_eq!(
                factory.set_default_hotkey(accounts.django),
                Err(Error::Unauthorized)
            );
            assert_eq!(
                factory.set_validator_hotkey(NETUID, Some(accounts.django)),
                Err(Error::Unauthorized)
            );
            assert_eq!(
                factory.transfer_ownership(Some(accounts.django)),
                Err(Error::Unauthorized)
            );

            set_caller(accounts.alice);
            assert_eq!(
                factory.set_fee_rate(MAX_FEE_RATE + 1),
                Err(Error::FeeRateTooHigh)
            );
            assert_eq!(factory.set_fee_rate(MAX_FEE_RATE), Ok(()));
            assert_eq!(factory.get_config().fee_rate.to_bits(), MAX_FEE_RATE);
            assert_eq!(
                factory.set_lockup_duration_limits(5, 4),
                Err(Error::InvalidLockupDurationLimits)
            );
            assert_eq!(factory.set_lockup_duration_limits(5, 5), Ok(()));
            assert_eq!(factory.set_fee_recipient(accounts.django), Ok(()));
            assert_eq!(factory.fee_recipient(), accounts.django);
            assert_eq!(factory.set_min_listing_amount(42), Ok(()));
            assert_eq!(factory.set_min_purchase_amount(43), Ok(()));
            let config = factory.get_config();
            assert_eq!(config.min_listing_amount, 42);
            assert_eq!(config.min_purchase_amount, 43);
        }

        #[ink::test]
        fn ownership_changes_only_when_the_new_owner_accepts() {
            let accounts = accounts();
            let mut factory = factory();

            set_caller(accounts.alice);
            assert_eq!(factory.transfer_ownership(Some(accounts.django)), Ok(()));
            assert_eq!(factory.get_pending_owner(), Some(accounts.django));
            // Until django accepts, alice is still the owner.
            assert_eq!(factory.set_fee_rate(0), Ok(()));

            set_caller(accounts.eve);
            assert_eq!(factory.accept_ownership(), Err(Error::Unauthorized));

            set_caller(accounts.django);
            assert_eq!(factory.accept_ownership(), Ok(()));
            assert_eq!(factory.get_owner(), accounts.django);
            assert_eq!(factory.get_pending_owner(), None);
            assert_eq!(last_event::<OwnerChanged>().old_owner, accounts.alice);

            set_caller(accounts.alice);
            assert_eq!(factory.set_fee_rate(0), Err(Error::Unauthorized));

            // A proposal can be withdrawn.
            set_caller(accounts.django);
            assert_eq!(factory.transfer_ownership(Some(accounts.eve)), Ok(()));
            assert_eq!(factory.transfer_ownership(None), Ok(()));
            set_caller(accounts.eve);
            assert_eq!(factory.accept_ownership(), Err(Error::Unauthorized));
        }

        #[ink::test]
        fn only_the_owner_upgrades_to_the_registry_current_code() {
            let accounts = accounts();
            let mut factory = factory();

            set_caller(accounts.django);
            assert_eq!(factory.upgrade(hash(3)), Err(Error::Unauthorized));

            set_caller(accounts.alice);
            assert_eq!(factory.upgrade(hash(3)), Err(Error::NoCurrentCode));
            set_registry_current(LOCKUP_FACTORY, Some(hash(3)));
            assert_eq!(factory.upgrade(hash(4)), Err(Error::NotCurrentCode));
            assert_eq!(installed_code(), None);
            assert_eq!(factory.upgrade(hash(3)), Ok(()));
            assert_eq!(installed_code(), Some(hash(3)));
        }

        #[ink::test]
        fn state_changing_messages_migrate_first() {
            let mut factory = factory();
            factory.version = 0;
            create(&mut factory).expect("create");
            assert_eq!(factory.get_version(), STORAGE_VERSION);

            factory.version = 0;
            factory.migrate();
            assert_eq!(factory.get_version(), STORAGE_VERSION);
        }
    }
}
