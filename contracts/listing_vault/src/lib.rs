#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub use self::listing_vault::{ListingVault, ListingVaultRef, Status};

/// One listing's Alpha, held on the vault's own coldkey.
///
/// The seller funds the vault without a proxy: one batch transfers the stake to
/// the vault and calls `activate`. Buyers take from the vault directly. Spot
/// listings send the Alpha to the buyer's chosen recipient, and lockup listings
/// create an escrow for the recipient. Alpha and TAO only ever leave a vault for
/// its seller, a buyer who paid in the same call, or an escrow created in the
/// same call.
///
/// The vault reads its factory only on takes (the trading pause and the fee
/// recipient) and to learn the validator hotkey. Cancelling, sweeping and closing
/// never depend on the factory. Upgrades work as for escrows, against the code
/// registry's `LISTING_VAULT` code, with the seller as the owner.
#[ink::contract(env = otc_shared::BittensorEnvironment)]
mod listing_vault {
    use otc_shared::events::{HotkeyResynced, TaoSwept, UpgradePolicyChanged, Upgraded};
    use otc_shared::{
        check_upgrade, move_stake_verified, quote, stake_delta_verified, stake_of,
        transfer_stake_verified, AlphaAmount, BlockAge, FixedDecimal, NetUid, PriceOffsetBps,
        PricingError, Quote, SubtensorError, TaoAmount, TransferError, UpgradeError, UpgradePolicy,
        BITTENSOR_MIN_STAKE,
    };

    /// Storage layout version this code expects; see `ensure_migrated`.
    #[cfg(not(feature = "upgrade-test"))]
    pub const STORAGE_VERSION: u16 = 1;
    #[cfg(feature = "upgrade-test")]
    pub const STORAGE_VERSION: u16 = 2;

    /// Storage key of the field the test-only version 2 adds. Defined in every
    /// build, because ink!'s storage macro names field types without their `cfg`.
    #[allow(dead_code)]
    type MigratedAtKey = ink::storage::traits::ManualKey<0x7631_6d32>;

    /// Gas a best-effort factory notification may use, in both dimensions, so a
    /// broken or hostile factory cannot make a cancel or the final take run out of
    /// gas.
    #[cfg(not(test))]
    const NOTIFY_REF_TIME_LIMIT: u64 = 10_000_000_000;
    #[cfg(not(test))]
    const NOTIFY_PROOF_SIZE_LIMIT: u64 = 128 * 1024;

    pub type PurchaseId = u64;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    #[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
    pub enum Status {
        /// Created; waiting for the seller to fund and activate it
        Pending,
        /// Open for takes
        Open,
        /// Filled or cancelled; only the sweeps and `close` remain
        Closed,
    }

    /// Error types for the listing vault
    #[derive(Debug, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    pub enum Error {
        /// Caller is not allowed to perform this action
        Unauthorized,
        /// The vault has already been activated
        NotPending,
        /// The vault has not been activated yet
        StillPending,
        /// The listing is not open for takes
        NotOpen,
        /// The listing is already closed
        AlreadyClosed,
        /// The listing is not closed yet
        NotClosed,
        /// The vault does not hold the listed amount, within the transfer tolerance
        FundingMismatch,
        /// The listing cannot be cancelled yet
        CancelTooEarly,
        /// The factory has paused trading
        TradingPaused,
        /// Amount is below the minimum purchase
        AmountTooSmall,
        /// Amount exceeds what is left in the listing
        AmountExceedsRemaining,
        /// The take would leave less than Subtensor's minimum stake
        AmountBelowMinimumStake,
        /// The executed price is below the seller's floor
        PriceBelowMinimum,
        /// The buyer sent less TAO than the price plus the fee
        InsufficientPayment,
        /// Failed to fetch the market price
        MarketPriceFetchFailed,
        /// The price offset prices Alpha at or below zero
        InvalidPriceOffset,
        /// Arithmetic overflow
        Overflow,
        /// Failed to query stake
        StakeQueryFailed,
        /// Subtensor rejected a stake transfer
        StakeTransferFailed,
        /// Failed to transfer TAO
        TaoTransferFailed,
        /// The vault holds no stake beyond what is listed
        NothingToSweep,
        /// The vault holds no TAO to sweep
        NoTaoToSweep,
        /// The vault still holds stake on its hotkey
        StakeRemaining,
        /// The new hotkey is the hotkey the vault already uses
        InvalidHotkey,
        /// The vault does not hold more stake on the new hotkey than on its current one
        InsufficientStakeOnHotkey,
        /// The stake already sits on the factory's validator hotkey
        AlreadyConsolidated,
        /// Failed to query the factory
        FactoryQueryFailed,
        /// Failed to create the buyer's escrow
        EscrowInstantiationFailed,
        /// The code registry has no current code for this kind
        NoCurrentCode,
        /// The requested code is not the registry's current vault code
        NotCurrentCode,
        /// Only the seller may upgrade this vault
        ConsentRequired,
        /// The current code has not been current for the force delay yet
        ForceDelayNotElapsed,
        /// Failed to query the code registry
        RegistryQueryFailed,
        /// Failed to replace the contract code
        CodeUpgradeFailed,
    }

    /// Event emitted when the seller activates the funded vault
    #[ink::event]
    pub struct Activated {
        #[ink(topic)]
        pub seller: AccountId,
        #[ink(topic)]
        pub hotkey: AccountId,
        pub amount: AlphaAmount,
    }

    /// Event emitted when a buyer takes Alpha from the listing
    #[ink::event]
    pub struct Taken {
        #[ink(topic)]
        pub buyer: AccountId,
        #[ink(topic)]
        pub recipient: AccountId,
        pub amount: AlphaAmount,
        pub price: u64,
        pub tao: TaoAmount,
        pub fee: TaoAmount,
        pub escrow: Option<AccountId>,
        pub remaining: AlphaAmount,
    }

    /// Event emitted when the last of the listing is taken
    #[ink::event]
    pub struct Filled {
        #[ink(topic)]
        pub seller: AccountId,
    }

    /// Event emitted when the seller cancels the listing
    #[ink::event]
    pub struct Cancelled {
        #[ink(topic)]
        pub seller: AccountId,
        pub returned: AlphaAmount,
    }

    /// Event emitted when stake beyond the listing is sent to the seller
    #[ink::event]
    pub struct StakeSwept {
        #[ink(topic)]
        pub seller: AccountId,
        #[ink(topic)]
        pub initiated_by: AccountId,
        pub amount: AlphaAmount,
    }

    /// Event emitted when the vault moves its stake to the factory's validator
    #[ink::event]
    pub struct Consolidated {
        #[ink(topic)]
        pub old_hotkey: AccountId,
        #[ink(topic)]
        pub new_hotkey: AccountId,
        #[ink(topic)]
        pub initiated_by: AccountId,
        pub amount: AlphaAmount,
    }

    /// Event emitted when the seller closes the vault
    #[ink::event]
    pub struct VaultClosed {
        #[ink(topic)]
        pub seller: AccountId,
        pub stake_returned: AlphaAmount,
        pub tao_balance: Balance,
    }

    /// Everything about a vault, for frontends
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    pub struct VaultInfo {
        pub factory: AccountId,
        pub registry: AccountId,
        pub seller: AccountId,
        pub netuid: NetUid,
        pub hotkey: AccountId,
        pub total: AlphaAmount,
        pub remaining: AlphaAmount,
        pub price_offset_bps: PriceOffsetBps,
        pub min_price: u64,
        pub fee_rate: FixedDecimal,
        pub min_purchase: AlphaAmount,
        pub lockup_duration: Option<BlockAge>,
        pub cancel_delay: BlockAge,
        pub cancellable_after: BlockNumber,
        pub status: Status,
        pub stake: AlphaAmount,
        pub tao_balance: Balance,
        pub upgrade_policy: UpgradePolicy,
        pub version: u16,
    }

    /// Root storage. Upgrades keep this struct exactly as it is; add new state in
    /// `Lazy` or `Mapping` fields with explicit storage keys instead.
    #[ink(storage)]
    pub struct ListingVault {
        /// The factory that created this vault
        factory: AccountId,
        /// Code registry that approves this vault's upgrades and escrow code
        registry: AccountId,
        /// Owner of the listed Alpha
        seller: AccountId,
        netuid: NetUid,
        /// Where the stake sits; changed only by activation, resync and consolidate
        hotkey: AccountId,
        /// The requested amount while Pending, then the stake activation measured
        total: AlphaAmount,
        /// What is left to sell
        remaining: AlphaAmount,
        price_offset_bps: PriceOffsetBps,
        /// The seller's floor, TAO per Alpha scaled by 1e9; 0 means none
        min_price: u64,
        /// Fee rate snapshot from the factory at creation
        fee_rate: FixedDecimal,
        min_purchase: AlphaAmount,
        /// `None` for spot listings; otherwise how long bought Alpha stays locked
        lockup_duration: Option<BlockAge>,
        /// How long after activation the seller must wait to cancel, so a listing
        /// can't vanish the moment it opens
        cancel_delay: BlockAge,
        /// First block at which the seller may cancel; 0 until activation
        cancellable_after: BlockNumber,
        status: Status,
        /// Counter for escrow salts
        next_purchase_id: PurchaseId,
        /// Who may upgrade this vault
        upgrade_policy: UpgradePolicy,
        /// Storage layout version, see `STORAGE_VERSION`
        version: u16,
        /// Test-only state version 2 adds, in its own storage cell as the layout
        /// rule requires: the block at which the vault migrated to version 2
        #[cfg(feature = "upgrade-test")]
        migrated_at: ink::storage::Lazy<BlockNumber, MigratedAtKey>,
    }

    impl ListingVault {
        /// Created by a factory, which becomes the vault's `factory`. The factory
        /// validates the listing terms and keeps the listing's id.
        ///
        /// The arguments are part of what the vault's address is derived from, so
        /// they hold only what the seller can know in advance: their terms and the
        /// factory's current settings.
        #[allow(clippy::too_many_arguments)]
        #[ink(constructor)]
        pub fn new(
            seller: AccountId,
            netuid: NetUid,
            hotkey: AccountId,
            amount: AlphaAmount,
            price_offset_bps: PriceOffsetBps,
            min_price: u64,
            fee_rate: FixedDecimal,
            min_purchase: AlphaAmount,
            lockup_duration: Option<BlockAge>,
            cancel_delay: BlockAge,
            registry: AccountId,
        ) -> Self {
            Self {
                factory: Self::env().caller(),
                registry,
                seller,
                netuid,
                hotkey,
                total: amount,
                remaining: 0,
                price_offset_bps,
                min_price,
                fee_rate,
                min_purchase,
                lockup_duration,
                cancel_delay,
                cancellable_after: 0,
                status: Status::Pending,
                next_purchase_id: 1,
                upgrade_policy: UpgradePolicy::default(),
                version: STORAGE_VERSION,
                #[cfg(feature = "upgrade-test")]
                migrated_at: Default::default(),
            }
        }

        /// Open the listing once the seller has transferred the stake here, usually
        /// in the same batch. The vault must hold the listed amount within the
        /// transfer tolerance, and lists what it actually holds. It then moves the
        /// stake to the factory's validator hotkey if Subtensor allows; otherwise
        /// the listing stays on the seller's hotkey.
        #[ink(message)]
        pub fn activate(&mut self) -> Result<(), Error> {
            self.ensure_migrated();
            self.ensure_seller()?;

            if self.status != Status::Pending {
                return Err(Error::NotPending);
            }

            let funded = self.own_stake(self.hotkey)?;
            if !stake_delta_verified(funded, self.total) {
                return Err(Error::FundingMismatch);
            }
            self.total = funded;
            self.remaining = funded;
            self.status = Status::Open;
            self.cancellable_after = self
                .env()
                .block_number()
                .checked_add(self.cancel_delay)
                .ok_or(Error::Overflow)?;

            if let Ok(validator) = self.factory_validator_hotkey() {
                if validator != self.hotkey {
                    match move_stake_verified(self.hotkey, validator, self.netuid, funded) {
                        Ok(moved) => {
                            self.hotkey = validator;
                            self.total = moved;
                            self.remaining = moved;
                        }
                        Err(TransferError::NotVerified) => Self::trap_not_verified(),
                        Err(_) => {}
                    }
                }
            }

            self.env().emit_event(Activated {
                seller: self.seller,
                hotkey: self.hotkey,
                amount: self.remaining,
            });

            Ok(())
        }

        /// Buy `amount` Alpha at the market price moved by the listing's offset,
        /// paying the price plus the fee; any excess TAO is refunded. Spot listings
        /// send the Alpha to `recipient`. Lockup listings create an escrow that
        /// locks it for `recipient` and return the escrow's account.
        #[ink(message, payable)]
        pub fn take(
            &mut self,
            amount: AlphaAmount,
            recipient: AccountId,
        ) -> Result<Option<AccountId>, Error> {
            self.ensure_migrated();

            if self.status != Status::Open {
                return Err(Error::NotOpen);
            }
            if self.factory_trading_paused()? {
                return Err(Error::TradingPaused);
            }

            // Transfers can round down by a rao, so the final remainder can always
            // be taken, even below the minimum purchase; otherwise it could never fill.
            if amount == 0 || (amount < self.min_purchase && amount != self.remaining) {
                return Err(Error::AmountTooSmall);
            }
            if amount > self.remaining {
                return Err(Error::AmountExceedsRemaining);
            }
            let left = self.remaining.checked_sub(amount).ok_or(Error::Overflow)?;
            if left > 0 && left < BITTENSOR_MIN_STAKE {
                return Err(Error::AmountBelowMinimumStake);
            }

            let quote = self.current_quote(amount)?;
            if quote.price < self.min_price {
                return Err(Error::PriceBelowMinimum);
            }
            let paid = self.env().transferred_value();
            let refund = paid
                .checked_sub(quote.total)
                .ok_or(Error::InsufficientPayment)?;

            let (escrow, held) = match self.lockup_duration {
                None => {
                    let held = transfer_stake_verified(recipient, self.hotkey, self.netuid, amount)
                        .map_err(Self::transfer_error)?;
                    (None, held)
                }
                Some(duration) => {
                    let unlock_block = self
                        .env()
                        .block_number()
                        .checked_add(duration)
                        .ok_or(Error::Overflow)?;
                    let escrow = self.create_escrow(recipient, amount, unlock_block)?;
                    let held = transfer_stake_verified(escrow, self.hotkey, self.netuid, amount)
                        .map_err(Self::transfer_error)?;
                    (Some(escrow), held)
                }
            };

            // Rounding can take a rao more than `amount` from the vault, so never
            // list more than it still holds.
            self.remaining = left.min(held);
            if self.remaining == 0 {
                self.status = Status::Closed;
            }

            let buyer = self.env().caller();
            self.transfer_tao(self.seller, quote.tao)?;
            if quote.fee > 0 {
                let fee_recipient = self.factory_fee_recipient()?;
                self.transfer_tao(fee_recipient, quote.fee)?;
            }
            if refund > 0 {
                self.transfer_tao(buyer, refund)?;
            }

            self.env().emit_event(Taken {
                buyer,
                recipient,
                amount,
                price: quote.price,
                tao: quote.tao,
                fee: quote.fee,
                escrow,
                remaining: self.remaining,
            });

            if self.status == Status::Closed {
                self.env().emit_event(Filled {
                    seller: self.seller,
                });
                self.notify_listing_closed();
            }

            Ok(escrow)
        }

        /// Close the listing and return its stake to the seller, once the cancel
        /// delay since activation has passed; a Pending vault can be cancelled at
        /// any time. If Subtensor
        /// refuses the transfer, for example while the subnet is being dissolved,
        /// the listing still closes; the sweeps return whatever is left later.
        /// Never pausable, and never depends on the factory.
        #[ink(message)]
        pub fn cancel(&mut self) -> Result<(), Error> {
            self.ensure_migrated();
            self.ensure_seller()?;

            if self.status == Status::Closed {
                return Err(Error::AlreadyClosed);
            }
            if self.env().block_number() < self.cancellable_after {
                return Err(Error::CancelTooEarly);
            }

            self.status = Status::Closed;
            self.remaining = 0;

            let returned = match self.own_stake(self.hotkey) {
                Ok(stake) if stake > 0 => {
                    match transfer_stake_verified(self.seller, self.hotkey, self.netuid, stake) {
                        Ok(_) => stake,
                        Err(TransferError::NotVerified) => Self::trap_not_verified(),
                        Err(_) => 0,
                    }
                }
                _ => 0,
            };

            self.env().emit_event(Cancelled {
                seller: self.seller,
                returned,
            });
            self.notify_listing_closed();

            Ok(())
        }

        /// Send the stake the vault holds beyond what is listed, such as staking
        /// rewards, to the seller; once the listing is closed, that is all of it.
        /// Anyone can call this.
        #[ink(message)]
        pub fn sweep_stake(&mut self) -> Result<(), Error> {
            self.ensure_migrated();

            if self.status == Status::Pending {
                return Err(Error::StillPending);
            }

            let held = self.own_stake(self.hotkey)?;
            let excess = held.saturating_sub(self.remaining);
            if excess == 0 {
                return Err(Error::NothingToSweep);
            }

            let held = transfer_stake_verified(self.seller, self.hotkey, self.netuid, excess)
                .map_err(Self::transfer_error)?;
            // Rounding can take a rao more than `excess`; never list more than is held.
            self.remaining = self.remaining.min(held);

            self.env().emit_event(StakeSwept {
                seller: self.seller,
                initiated_by: self.env().caller(),
                amount: excess,
            });

            Ok(())
        }

        /// Send the vault's whole TAO balance to the seller.
        ///
        /// TAO reaches a vault when its subnet is dissolved and Subtensor liquidates
        /// the stake, or when someone sends it. Anyone can sweep at any time.
        /// Sweeping never terminates the vault, because liquidation can credit TAO
        /// in several steps, and credits to a terminated vault would be lost.
        #[ink(message)]
        pub fn sweep_tao(&mut self) -> Result<(), Error> {
            self.ensure_migrated();

            let amount = self.env().balance();
            if amount == 0 {
                return Err(Error::NoTaoToSweep);
            }

            self.transfer_tao(self.seller, amount)?;

            self.env().emit_event(TaoSwept {
                owner: self.seller,
                initiated_by: self.env().caller(),
                amount,
            });

            Ok(())
        }

        /// Terminate the vault, sending its remaining stake, its TAO balance and
        /// the storage deposit to the seller.
        ///
        /// Only the seller can close, because only they can judge that nothing else
        /// is on its way: a terminated vault loses anything that arrives later, such
        /// as stake on another hotkey after a hotkey swap. It requires a closed
        /// listing. Stake still on the vault's hotkey, such as rewards that accrued
        /// after the final take, goes to the seller first; only dust worth less
        /// than Subtensor's minimum transfer, which it refuses to move, is left
        /// behind.
        #[ink(message)]
        pub fn close(&mut self) -> Result<(), Error> {
            self.ensure_seller()?;

            if self.status != Status::Closed {
                return Err(Error::NotClosed);
            }

            let stake = self.own_stake(self.hotkey)?;
            let mut stake_returned = 0;
            if stake > 0 {
                match transfer_stake_verified(self.seller, self.hotkey, self.netuid, stake) {
                    Ok(_) => stake_returned = stake,
                    Err(TransferError::NotVerified) => Self::trap_not_verified(),
                    Err(TransferError::Rejected(SubtensorError::AmountTooLow)) => {}
                    Err(_) => return Err(Error::StakeRemaining),
                }
            }

            self.env().emit_event(VaultClosed {
                seller: self.seller,
                stake_returned,
                tao_balance: self.env().balance(),
            });

            self.env().terminate_contract(self.seller)
        }

        /// Point the vault at `new_hotkey` after its stake moved there, for example
        /// through a validator hotkey swap. Moves nothing.
        ///
        /// Anyone can call this. It is accepted only when the vault holds more stake
        /// on `new_hotkey` than on its current hotkey, so pointing it elsewhere
        /// would take a donation larger than the vault's own stake, and that
        /// donation belongs to the seller.
        #[ink(message)]
        pub fn resync_hotkey(&mut self, new_hotkey: AccountId) -> Result<(), Error> {
            self.ensure_migrated();

            if new_hotkey == self.hotkey {
                return Err(Error::InvalidHotkey);
            }

            let current_stake = self.own_stake(self.hotkey)?;
            let new_hotkey_stake = self.own_stake(new_hotkey)?;
            if new_hotkey_stake <= current_stake {
                return Err(Error::InsufficientStakeOnHotkey);
            }

            let old_hotkey = self.hotkey;
            self.hotkey = new_hotkey;

            self.env().emit_event(HotkeyResynced {
                old_hotkey,
                new_hotkey,
                initiated_by: self.env().caller(),
                new_hotkey_stake,
            });

            Ok(())
        }

        /// Move the open listing's stake to the factory's current validator hotkey,
        /// so listings follow a validator change. Anyone can call this.
        #[ink(message)]
        pub fn consolidate(&mut self) -> Result<(), Error> {
            self.ensure_migrated();

            if self.status != Status::Open {
                return Err(Error::NotOpen);
            }

            let validator = self.factory_validator_hotkey()?;
            if validator == self.hotkey {
                return Err(Error::AlreadyConsolidated);
            }

            let held = self.own_stake(self.hotkey)?;
            let moved = move_stake_verified(self.hotkey, validator, self.netuid, held)
                .map_err(Self::transfer_error)?;

            let old_hotkey = self.hotkey;
            self.hotkey = validator;
            self.remaining = self.remaining.min(moved);

            self.env().emit_event(Consolidated {
                old_hotkey,
                new_hotkey: validator,
                initiated_by: self.env().caller(),
                amount: moved,
            });

            Ok(())
        }

        /// Install `code_hash`, which must be the registry's current vault code.
        ///
        /// The seller can upgrade at any time. Anyone else can upgrade once the code
        /// has been current for the registry's force delay, unless the seller set
        /// the policy to `ConsentOnly`. The new code takes effect from the next
        /// call, and migrates storage then if it needs to.
        #[ink(message)]
        pub fn upgrade(&mut self, code_hash: Hash) -> Result<(), Error> {
            use otc_shared::upgrade::LISTING_VAULT;

            let caller = self.env().caller();
            let current = self.registry_current(LISTING_VAULT)?;
            let force_target = self.registry_force_target(LISTING_VAULT)?;
            check_upgrade(
                caller == self.seller,
                self.upgrade_policy,
                code_hash,
                current,
                force_target,
            )
            .map_err(Self::upgrade_error)?;

            self.install_code(code_hash)?;

            self.env().emit_event(Upgraded {
                code_hash,
                initiated_by: caller,
            });

            Ok(())
        }

        /// Choose whether anyone may force the registry's current code onto this
        /// vault after the force delay (`Auto`), or only the seller may upgrade it
        /// (`ConsentOnly`).
        #[ink(message)]
        pub fn set_upgrade_policy(&mut self, policy: UpgradePolicy) -> Result<(), Error> {
            self.ensure_migrated();
            self.ensure_seller()?;

            self.upgrade_policy = policy;

            self.env().emit_event(UpgradePolicyChanged {
                owner: self.seller,
                policy,
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

        /// Everything about the vault
        #[ink(message)]
        pub fn get_info(&self) -> Result<VaultInfo, Error> {
            Ok(VaultInfo {
                factory: self.factory,
                registry: self.registry,
                seller: self.seller,
                netuid: self.netuid,
                hotkey: self.hotkey,
                total: self.total,
                remaining: self.remaining,
                price_offset_bps: self.price_offset_bps,
                min_price: self.min_price,
                fee_rate: self.fee_rate,
                min_purchase: self.min_purchase,
                lockup_duration: self.lockup_duration,
                cancel_delay: self.cancel_delay,
                cancellable_after: self.cancellable_after,
                status: self.status,
                stake: self.own_stake(self.hotkey)?,
                tao_balance: self.env().balance(),
                upgrade_policy: self.upgrade_policy,
                version: self.version,
            })
        }

        /// What taking `amount` costs at the current market price. Buyers should
        /// send a little more than `total`, since the price can move before the
        /// take lands; the excess is refunded.
        #[ink(message)]
        pub fn quote(&self, amount: AlphaAmount) -> Result<Quote, Error> {
            if amount > self.remaining {
                return Err(Error::AmountExceedsRemaining);
            }
            self.current_quote(amount)
        }

        #[ink(message)]
        pub fn get_status(&self) -> Status {
            self.status
        }

        #[ink(message)]
        pub fn get_seller(&self) -> AccountId {
            self.seller
        }

        #[ink(message)]
        pub fn get_factory(&self) -> AccountId {
            self.factory
        }

        #[ink(message)]
        pub fn get_registry(&self) -> AccountId {
            self.registry
        }

        #[ink(message)]
        pub fn get_hotkey(&self) -> AccountId {
            self.hotkey
        }

        #[ink(message)]
        pub fn get_remaining(&self) -> AlphaAmount {
            self.remaining
        }

        #[ink(message)]
        pub fn get_upgrade_policy(&self) -> UpgradePolicy {
            self.upgrade_policy
        }

        #[ink(message)]
        pub fn get_version(&self) -> u16 {
            self.version
        }

        /// Test-only: the block at which the vault migrated to version 2
        #[cfg(feature = "upgrade-test")]
        #[ink(message)]
        pub fn get_migrated_at(&self) -> Option<BlockNumber> {
            self.migrated_at.get()
        }

        fn own_stake(&self, hotkey: AccountId) -> Result<AlphaAmount, Error> {
            stake_of(hotkey, self.env().account_id(), self.netuid)
                .map_err(|_| Error::StakeQueryFailed)
        }

        fn current_quote(&self, amount: AlphaAmount) -> Result<Quote, Error> {
            let market_price = self
                .env()
                .extension()
                .get_alpha_price(self.netuid)
                .map_err(|_| Error::MarketPriceFetchFailed)?;
            quote(market_price, self.price_offset_bps, self.fee_rate, amount)
                .map_err(Self::pricing_error)
        }

        fn create_escrow(
            &mut self,
            beneficiary: AccountId,
            amount: AlphaAmount,
            unlock_block: BlockNumber,
        ) -> Result<AccountId, Error> {
            use otc_shared::upgrade::ESCROW;

            let code_hash = self.registry_current(ESCROW)?.ok_or(Error::NoCurrentCode)?;
            let purchase_id = self.next_purchase_id;
            self.next_purchase_id = purchase_id.checked_add(1).ok_or(Error::Overflow)?;

            self.instantiate_escrow(
                code_hash,
                beneficiary,
                amount,
                unlock_block,
                purchase_id.to_le_bytes(),
            )
        }

        fn transfer_error(error: TransferError) -> Error {
            match error {
                TransferError::Rejected(_) => Error::StakeTransferFailed,
                TransferError::QueryFailed => Error::StakeQueryFailed,
                TransferError::NotVerified => Self::trap_not_verified(),
            }
        }

        fn pricing_error(error: PricingError) -> Error {
            match error {
                PricingError::InvalidPriceOffset => Error::InvalidPriceOffset,
                PricingError::Overflow => Error::Overflow,
            }
        }

        fn upgrade_error(error: UpgradeError) -> Error {
            match error {
                UpgradeError::NoCurrentCode => Error::NoCurrentCode,
                UpgradeError::NotCurrentCode => Error::NotCurrentCode,
                UpgradeError::ConsentRequired => Error::ConsentRequired,
                UpgradeError::ForceDelayNotElapsed => Error::ForceDelayNotElapsed,
            }
        }

        /// A transfer that completed but moved the wrong amount must revert the
        /// whole call, including the transfer itself.
        fn trap_not_verified() -> ! {
            panic!("post-transfer stake verification failed")
        }

        /// Storage only migrates upwards. Older code runs on newer storage without
        /// knowing its added fields, so a rollback after a storage change must ship
        /// as new code with a higher version, never as the old code.
        fn ensure_migrated(&mut self) {
            if self.version < STORAGE_VERSION {
                // Versions newer than 1 initialise their added storage here.
                #[cfg(feature = "upgrade-test")]
                self.migrated_at.set(&self.env().block_number());
                self.version = STORAGE_VERSION;
            }
        }

        fn ensure_seller(&self) -> Result<(), Error> {
            if self.env().caller() != self.seller {
                return Err(Error::Unauthorized);
            }
            Ok(())
        }

        #[cfg(not(test))]
        fn factory(
            &self,
        ) -> ink::contract_ref!(otc_shared::VaultFactory, otc_shared::BittensorEnvironment)
        {
            self.factory.into()
        }

        #[cfg(not(test))]
        fn factory_trading_paused(&self) -> Result<bool, Error> {
            use ink::codegen::TraitCallBuilder;
            use otc_shared::VaultFactory as _;
            self.factory()
                .call()
                .trading_paused()
                .try_invoke()
                .map_err(|_| Error::FactoryQueryFailed)?
                .map_err(|_| Error::FactoryQueryFailed)
        }

        #[cfg(not(test))]
        fn factory_fee_recipient(&self) -> Result<AccountId, Error> {
            use ink::codegen::TraitCallBuilder;
            use otc_shared::VaultFactory as _;
            self.factory()
                .call()
                .fee_recipient()
                .try_invoke()
                .map_err(|_| Error::FactoryQueryFailed)?
                .map_err(|_| Error::FactoryQueryFailed)
        }

        #[cfg(not(test))]
        fn factory_validator_hotkey(&self) -> Result<AccountId, Error> {
            use ink::codegen::TraitCallBuilder;
            use otc_shared::VaultFactory as _;
            self.factory()
                .call()
                .validator_hotkey(self.netuid)
                .try_invoke()
                .map_err(|_| Error::FactoryQueryFailed)?
                .map_err(|_| Error::FactoryQueryFailed)
        }

        /// Tell the factory the listing closed, with bounded gas, ignoring failures.
        #[cfg(not(test))]
        fn notify_listing_closed(&self) {
            use ink::codegen::TraitCallBuilder;
            use otc_shared::VaultFactory as _;
            let mut factory = self.factory();
            let _ = factory
                .call_mut()
                .listing_closed()
                .ref_time_limit(NOTIFY_REF_TIME_LIMIT)
                .proof_size_limit(NOTIFY_PROOF_SIZE_LIMIT)
                .try_invoke();
        }

        #[cfg(not(test))]
        fn registry(&self) -> code_registry::CodeRegistryRef {
            use ink::env::call::FromAccountId;
            code_registry::CodeRegistryRef::from_account_id(self.registry)
        }

        #[cfg(not(test))]
        fn registry_current(&self, kind: otc_shared::CodeKind) -> Result<Option<Hash>, Error> {
            use ink::codegen::TraitCallBuilder;
            Ok(self
                .registry()
                .call()
                .current(kind)
                .try_invoke()
                .map_err(|_| Error::RegistryQueryFailed)?
                .map_err(|_| Error::RegistryQueryFailed)?
                .map(|code| code.code_hash))
        }

        #[cfg(not(test))]
        fn registry_force_target(&self, kind: otc_shared::CodeKind) -> Result<Option<Hash>, Error> {
            use ink::codegen::TraitCallBuilder;
            self.registry()
                .call()
                .force_target(kind)
                .try_invoke()
                .map_err(|_| Error::RegistryQueryFailed)?
                .map_err(|_| Error::RegistryQueryFailed)
        }

        #[cfg(not(test))]
        fn instantiate_escrow(
            &self,
            code_hash: Hash,
            beneficiary: AccountId,
            amount: AlphaAmount,
            unlock_block: BlockNumber,
            salt: [u8; 8],
        ) -> Result<AccountId, Error> {
            use alpha_lockup::AlphaLockupRef;
            use ink::ToAccountId;

            let escrow = AlphaLockupRef::new(
                beneficiary,
                self.netuid,
                amount,
                unlock_block,
                self.hotkey,
                self.registry,
            )
            .code_hash(code_hash)
            .endowment(0)
            .salt_bytes(salt)
            .try_instantiate()
            .map_err(|_| Error::EscrowInstantiationFailed)?
            .map_err(|_| Error::EscrowInstantiationFailed)?;

            Ok(escrow.to_account_id())
        }

        #[cfg(not(test))]
        fn install_code(&mut self, code_hash: Hash) -> Result<(), Error> {
            self.env()
                .set_code_hash(&code_hash)
                .map_err(|_| Error::CodeUpgradeFailed)
        }

        #[cfg(not(test))]
        fn transfer_tao(&mut self, to: AccountId, amount: Balance) -> Result<(), Error> {
            self.env()
                .transfer(to, amount)
                .map_err(|_| Error::TaoTransferFailed)
        }

        // ink!'s off-chain test environment cannot call other contracts,
        // instantiate them, replace code, or transfer a u64 balance, so unit tests
        // answer factory and registry queries and record escrows, upgrades and
        // TAO transfers here; localnet integration tests cover the real calls.
        #[cfg(test)]
        fn factory_trading_paused(&self) -> Result<bool, Error> {
            tests::factory().map(|factory| factory.paused)
        }

        #[cfg(test)]
        fn factory_fee_recipient(&self) -> Result<AccountId, Error> {
            tests::factory().map(|factory| factory.fee_recipient)
        }

        #[cfg(test)]
        fn factory_validator_hotkey(&self) -> Result<AccountId, Error> {
            tests::factory().map(|factory| factory.validator_hotkey)
        }

        #[cfg(test)]
        fn notify_listing_closed(&self) {
            tests::record_listing_closed();
        }

        #[cfg(test)]
        fn registry_current(&self, kind: otc_shared::CodeKind) -> Result<Option<Hash>, Error> {
            Ok(tests::registry_code(kind).0)
        }

        #[cfg(test)]
        fn registry_force_target(&self, kind: otc_shared::CodeKind) -> Result<Option<Hash>, Error> {
            Ok(tests::registry_code(kind).1)
        }

        #[cfg(test)]
        fn instantiate_escrow(
            &self,
            code_hash: Hash,
            beneficiary: AccountId,
            amount: AlphaAmount,
            unlock_block: BlockNumber,
            salt: [u8; 8],
        ) -> Result<AccountId, Error> {
            Ok(tests::record_escrow(tests::EscrowRecord {
                code_hash,
                beneficiary,
                amount,
                unlock_block,
                hotkey: self.hotkey,
                salt,
            }))
        }

        #[cfg(test)]
        fn install_code(&mut self, code_hash: Hash) -> Result<(), Error> {
            tests::record_installed_code(code_hash);
            Ok(())
        }

        #[cfg(test)]
        fn transfer_tao(&mut self, to: AccountId, amount: Balance) -> Result<(), Error> {
            tests::record_tao_transfer(to, amount);
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use core::panic::AssertUnwindSafe;
        use fixed::types::U64F64;
        use ink::env::test;
        use ink::env::Environment;
        use ink::scale::{Decode, Encode};
        use otc_shared::testing::{self, MockSubtensor};
        use otc_shared::upgrade::{ESCROW, LISTING_VAULT};
        use otc_shared::{BittensorEnvironment, CodeKind, TRANSFER_TOLERANCE};
        use std::cell::RefCell;
        use std::collections::BTreeMap;

        type TestAccountId = <BittensorEnvironment as Environment>::AccountId;

        const NETUID: NetUid = 1;
        const CREATED_AT: BlockNumber = 100;
        const LOCKUP: BlockAge = 50;
        const LISTED: AlphaAmount = 10_000_000_000;
        const MIN_PURCHASE: AlphaAmount = 1_000_000_000;
        const TWO_ALPHA: AlphaAmount = 2_000_000_000;
        // 0.5 TAO per Alpha and a 1/64 fee: both exact in fixed point.
        const MARKET_PRICE: u64 = 500_000_000;
        const TWO_ALPHA_TAO: TaoAmount = 1_000_000_000;
        const TWO_ALPHA_FEE: TaoAmount = 15_625_000;
        // Above the off-chain engine's existential deposit of 1_000_000.
        const VAULT_TAO_BALANCE: Balance = 5_000_000;

        #[derive(Clone, Copy)]
        pub(super) struct FactoryState {
            pub paused: bool,
            pub fee_recipient: TestAccountId,
            pub validator_hotkey: TestAccountId,
            pub reachable: bool,
        }

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(super) struct EscrowRecord {
            pub code_hash: Hash,
            pub beneficiary: TestAccountId,
            pub amount: AlphaAmount,
            pub unlock_block: BlockNumber,
            pub hotkey: TestAccountId,
            pub salt: [u8; 8],
        }

        /// The registry's `(current, force_target)` code per kind.
        type RegistryCodes = BTreeMap<CodeKind, (Option<Hash>, Option<Hash>)>;

        thread_local! {
            static FACTORY: RefCell<FactoryState> = RefCell::new(FactoryState {
                paused: false,
                fee_recipient: fee_collector(),
                validator_hotkey: validator(),
                reachable: true,
            });
            static LISTINGS_CLOSED: RefCell<u32> = const { RefCell::new(0) };
            static REGISTRY: RefCell<RegistryCodes> = const { RefCell::new(BTreeMap::new()) };
            static ESCROWS: RefCell<Vec<EscrowRecord>> = const { RefCell::new(Vec::new()) };
            static INSTALLED_CODE: RefCell<Option<Hash>> = const { RefCell::new(None) };
            static TAO_TRANSFERS: RefCell<Vec<(TestAccountId, Balance)>> =
                const { RefCell::new(Vec::new()) };
        }

        pub(super) fn factory() -> Result<FactoryState, Error> {
            let factory = FACTORY.with(|factory| *factory.borrow());
            if factory.reachable {
                Ok(factory)
            } else {
                Err(Error::FactoryQueryFailed)
            }
        }

        fn set_factory(update: impl FnOnce(&mut FactoryState)) {
            FACTORY.with(|factory| update(&mut factory.borrow_mut()));
        }

        pub(super) fn record_listing_closed() {
            LISTINGS_CLOSED.with(|count| *count.borrow_mut() += 1);
        }

        fn listings_closed() -> u32 {
            LISTINGS_CLOSED.with(|count| *count.borrow())
        }

        pub(super) fn registry_code(kind: CodeKind) -> (Option<Hash>, Option<Hash>) {
            REGISTRY.with(|registry| registry.borrow().get(&kind).copied().unwrap_or_default())
        }

        fn set_registry_code(kind: CodeKind, current: Option<Hash>, force_target: Option<Hash>) {
            REGISTRY.with(|registry| registry.borrow_mut().insert(kind, (current, force_target)));
        }

        /// Records the escrow and returns a distinct account for it.
        pub(super) fn record_escrow(record: EscrowRecord) -> TestAccountId {
            ESCROWS.with(|escrows| {
                let mut escrows = escrows.borrow_mut();
                escrows.push(record);
                let mut account = [0xEE; 32];
                account[31] = u8::try_from(escrows.len()).expect("few escrows");
                TestAccountId::from(account)
            })
        }

        fn escrows() -> Vec<EscrowRecord> {
            ESCROWS.with(|escrows| escrows.borrow().clone())
        }

        pub(super) fn record_installed_code(code_hash: Hash) {
            INSTALLED_CODE.with(|installed| *installed.borrow_mut() = Some(code_hash));
        }

        fn installed_code() -> Option<Hash> {
            INSTALLED_CODE.with(|installed| *installed.borrow())
        }

        pub(super) fn record_tao_transfer(to: TestAccountId, amount: Balance) {
            TAO_TRANSFERS.with(|transfers| transfers.borrow_mut().push((to, amount)));
            // Mirror the transfer so the vault's balance reads as spent.
            set_vault_balance(vault_balance().saturating_sub(amount));
        }

        fn tao_transfers() -> Vec<(TestAccountId, Balance)> {
            TAO_TRANSFERS.with(|transfers| transfers.borrow().clone())
        }

        /// ink!'s off-chain engine stores balances and transferred values as u128
        /// and only exposes them for u128 environments, so set them through the
        /// default environment; the vault reads them back as u64.
        fn set_vault_balance(amount: Balance) {
            test::set_account_balance::<ink::env::DefaultEnvironment>(
                accounts().frank,
                u128::from(amount),
            );
        }

        fn vault_balance() -> Balance {
            let balance =
                test::get_account_balance::<ink::env::DefaultEnvironment>(accounts().frank)
                    .expect("vault balance");
            Balance::try_from(balance).expect("balance fits in u64")
        }

        fn pay(value: Balance) {
            test::set_value_transferred::<ink::env::DefaultEnvironment>(u128::from(value));
        }

        fn accounts() -> test::DefaultAccounts<BittensorEnvironment> {
            test::default_accounts::<BittensorEnvironment>()
        }

        fn registry_account() -> TestAccountId {
            TestAccountId::from([0x55; 32])
        }

        fn fee_collector() -> TestAccountId {
            TestAccountId::from([0x66; 32])
        }

        fn validator() -> TestAccountId {
            TestAccountId::from([0x77; 32])
        }

        fn hash(byte: u8) -> Hash {
            Hash::from([byte; 32])
        }

        fn fee_rate() -> FixedDecimal {
            FixedDecimal::from_bits((U64F64::from_num(1) / 64).to_bits())
        }

        fn set_caller(account: TestAccountId) {
            test::set_caller::<BittensorEnvironment>(account);
        }

        #[derive(Default)]
        struct Terms {
            price_offset_bps: PriceOffsetBps,
            min_price: u64,
            lockup_duration: Option<BlockAge>,
            cancel_delay: BlockAge,
        }

        /// Roles in every test: alice sells, bob is the factory, charlie buys,
        /// django is a recipient or a third party, eve is the seller's hotkey, and
        /// frank is the vault itself.
        fn pending_vault(terms: Terms) -> ListingVault {
            let accounts = accounts();
            MockSubtensor::install(accounts.frank);
            testing::set_alpha_price(MARKET_PRICE);
            set_factory(|factory| {
                *factory = FactoryState {
                    paused: false,
                    fee_recipient: fee_collector(),
                    validator_hotkey: validator(),
                    reachable: true,
                }
            });
            LISTINGS_CLOSED.with(|count| *count.borrow_mut() = 0);
            REGISTRY.with(|registry| registry.borrow_mut().clear());
            set_registry_code(ESCROW, Some(hash(0xE5)), None);
            ESCROWS.with(|escrows| escrows.borrow_mut().clear());
            INSTALLED_CODE.with(|installed| *installed.borrow_mut() = None);
            TAO_TRANSFERS.with(|transfers| transfers.borrow_mut().clear());

            test::set_block_number::<BittensorEnvironment>(CREATED_AT);
            test::set_callee::<BittensorEnvironment>(accounts.frank);
            set_vault_balance(VAULT_TAO_BALANCE);
            // The factory instantiates the vault.
            set_caller(accounts.bob);
            ListingVault::new(
                accounts.alice,
                NETUID,
                accounts.eve,
                LISTED,
                terms.price_offset_bps,
                terms.min_price,
                fee_rate(),
                MIN_PURCHASE,
                terms.lockup_duration,
                terms.cancel_delay,
                registry_account(),
            )
        }

        /// The seller transfers `amount` to the vault on their hotkey.
        fn fund(amount: AlphaAmount) {
            let accounts = accounts();
            testing::set_stake(accounts.eve, accounts.frank, NETUID, amount);
        }

        /// A funded, activated vault, delegated to the validator hotkey.
        fn open_vault(terms: Terms) -> ListingVault {
            let mut vault = pending_vault(terms);
            fund(LISTED);
            set_caller(accounts().alice);
            assert_eq!(vault.activate(), Ok(()));
            vault
        }

        fn vault_stake(hotkey: TestAccountId) -> AlphaAmount {
            testing::stake(hotkey, accounts().frank, NETUID)
        }

        fn assert_terminated_to(
            result: std::thread::Result<Result<(), Error>>,
            beneficiary: TestAccountId,
            amount: Balance,
        ) {
            let payload = result.expect_err("terminate_contract should panic in ink! unit tests");
            let encoded = payload
                .downcast_ref::<Vec<u8>>()
                .expect("termination panic should contain SCALE encoded payload");
            let (terminated_amount, encoded_beneficiary): (u128, Vec<u8>) =
                Decode::decode(&mut &encoded[..]).expect("decode termination payload");

            assert_eq!(terminated_amount, u128::from(amount));
            assert_eq!(encoded_beneficiary, Encode::encode(&beneficiary));
        }

        fn last_event<E: Decode>() -> E {
            let events = test::recorded_events().collect::<Vec<_>>();
            let event = events.last().expect("an event");
            E::decode(&mut &event.data[..]).expect("decode event")
        }

        #[ink::test]
        fn constructor_records_the_terms_and_the_factory() {
            let accounts = accounts();
            let vault = pending_vault(Terms::default());

            let info = vault.get_info().expect("info");
            assert_eq!(info.factory, accounts.bob);
            assert_eq!(info.registry, registry_account());
            assert_eq!(info.seller, accounts.alice);
            assert_eq!(info.hotkey, accounts.eve);
            assert_eq!(info.total, LISTED);
            assert_eq!(info.remaining, 0);
            assert_eq!(info.status, Status::Pending);
            assert_eq!(info.upgrade_policy, UpgradePolicy::Auto);
            assert_eq!(info.version, STORAGE_VERSION);
        }

        #[ink::test]
        fn activation_lists_the_measured_stake_on_the_validator() {
            let accounts = accounts();
            let mut vault = pending_vault(Terms::default());
            // The transfer into the vault rounded down by a rao.
            fund(LISTED - 1);

            set_caller(accounts.alice);
            assert_eq!(vault.activate(), Ok(()));

            assert_eq!(vault.get_status(), Status::Open);
            assert_eq!(vault.get_hotkey(), validator());
            assert_eq!(vault.get_remaining(), LISTED - 1);
            assert_eq!(vault_stake(validator()), LISTED - 1);
            assert_eq!(vault_stake(accounts.eve), 0);

            let activated = last_event::<Activated>();
            assert_eq!(activated.seller, accounts.alice);
            assert_eq!(activated.hotkey, validator());
            assert_eq!(activated.amount, LISTED - 1);
        }

        #[ink::test]
        fn activation_stays_on_the_seller_hotkey_when_delegation_fails() {
            let accounts = accounts();
            let mut vault = pending_vault(Terms::default());
            fund(LISTED);
            testing::set_reject_transfers(true);

            set_caller(accounts.alice);
            assert_eq!(vault.activate(), Ok(()));
            assert_eq!(vault.get_hotkey(), accounts.eve);
            assert_eq!(vault.get_remaining(), LISTED);

            // An unreachable factory is not fatal either.
            let mut vault = pending_vault(Terms::default());
            fund(LISTED);
            set_factory(|factory| factory.reachable = false);
            set_caller(accounts.alice);
            assert_eq!(vault.activate(), Ok(()));
            assert_eq!(vault.get_hotkey(), accounts.eve);
        }

        #[ink::test]
        fn activation_requires_the_seller_and_the_listed_stake() {
            let accounts = accounts();
            let mut vault = pending_vault(Terms::default());
            fund(LISTED);

            set_caller(accounts.django);
            assert_eq!(vault.activate(), Err(Error::Unauthorized));

            set_caller(accounts.alice);
            fund(LISTED - TRANSFER_TOLERANCE - 1);
            assert_eq!(vault.activate(), Err(Error::FundingMismatch));
            fund(LISTED + TRANSFER_TOLERANCE + 1);
            assert_eq!(vault.activate(), Err(Error::FundingMismatch));

            fund(LISTED);
            assert_eq!(vault.activate(), Ok(()));
            assert_eq!(vault.activate(), Err(Error::NotPending));
        }

        #[ink::test]
        fn spot_take_pays_the_seller_the_fee_and_refunds_the_rest() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());

            set_caller(accounts.charlie);
            pay(TWO_ALPHA_TAO + TWO_ALPHA_FEE + 1_000);
            assert_eq!(vault.take(TWO_ALPHA, accounts.django), Ok(None));

            assert_eq!(
                testing::stake(validator(), accounts.django, NETUID),
                TWO_ALPHA
            );
            assert_eq!(vault.get_remaining(), LISTED - TWO_ALPHA);
            assert_eq!(vault.get_status(), Status::Open);
            assert_eq!(
                tao_transfers(),
                vec![
                    (accounts.alice, TWO_ALPHA_TAO),
                    (fee_collector(), TWO_ALPHA_FEE),
                    (accounts.charlie, 1_000),
                ]
            );

            let taken = last_event::<Taken>();
            assert_eq!(taken.buyer, accounts.charlie);
            assert_eq!(taken.recipient, accounts.django);
            assert_eq!(taken.amount, TWO_ALPHA);
            assert_eq!(taken.price, MARKET_PRICE);
            assert_eq!(taken.tao, TWO_ALPHA_TAO);
            assert_eq!(taken.fee, TWO_ALPHA_FEE);
            assert_eq!(taken.escrow, None);
            assert_eq!(taken.remaining, LISTED - TWO_ALPHA);
        }

        #[ink::test]
        fn lockup_take_creates_an_escrow_for_the_recipient() {
            let accounts = accounts();
            let mut vault = open_vault(Terms {
                lockup_duration: Some(LOCKUP),
                ..Terms::default()
            });

            set_caller(accounts.charlie);
            pay(TWO_ALPHA_TAO + TWO_ALPHA_FEE);
            let escrow = vault
                .take(TWO_ALPHA, accounts.django)
                .expect("take")
                .expect("an escrow");

            assert_eq!(
                escrows(),
                vec![EscrowRecord {
                    code_hash: hash(0xE5),
                    beneficiary: accounts.django,
                    amount: TWO_ALPHA,
                    unlock_block: CREATED_AT + LOCKUP,
                    hotkey: validator(),
                    salt: 1u64.to_le_bytes(),
                }]
            );
            assert_eq!(testing::stake(validator(), escrow, NETUID), TWO_ALPHA);
            assert_eq!(testing::stake(validator(), accounts.django, NETUID), 0);
            assert_eq!(last_event::<Taken>().escrow, Some(escrow));

            // Every purchase gets its own salt.
            pay(TWO_ALPHA_TAO + TWO_ALPHA_FEE);
            let second = vault.take(TWO_ALPHA, accounts.django).expect("take");
            assert_ne!(second, Some(escrow));
            assert_eq!(escrows()[1].salt, 2u64.to_le_bytes());
        }

        #[ink::test]
        fn lockup_take_needs_escrow_code_in_the_registry() {
            let accounts = accounts();
            let mut vault = open_vault(Terms {
                lockup_duration: Some(LOCKUP),
                ..Terms::default()
            });
            set_registry_code(ESCROW, None, None);

            set_caller(accounts.charlie);
            pay(TWO_ALPHA_TAO + TWO_ALPHA_FEE);
            assert_eq!(
                vault.take(TWO_ALPHA, accounts.charlie),
                Err(Error::NoCurrentCode)
            );
        }

        #[ink::test]
        fn take_rejects_amounts_outside_the_listing() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());
            set_caller(accounts.charlie);
            pay(u64::MAX / 2);

            assert_eq!(vault.take(0, accounts.charlie), Err(Error::AmountTooSmall));
            assert_eq!(
                vault.take(MIN_PURCHASE - 1, accounts.charlie),
                Err(Error::AmountTooSmall)
            );
            assert_eq!(
                vault.take(LISTED + 1, accounts.charlie),
                Err(Error::AmountExceedsRemaining)
            );
            assert_eq!(
                vault.take(LISTED - 1, accounts.charlie),
                Err(Error::AmountBelowMinimumStake)
            );
        }

        #[ink::test]
        fn the_final_remainder_can_be_taken_below_the_minimum_purchase() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());
            let remainder = MIN_PURCHASE / 2;

            set_caller(accounts.charlie);
            pay(u64::MAX / 2);
            assert_eq!(vault.take(LISTED - remainder, accounts.charlie), Ok(None));
            assert_eq!(vault.get_remaining(), remainder);

            assert_eq!(vault.take(remainder, accounts.charlie), Ok(None));
            assert_eq!(vault.get_remaining(), 0);
        }

        #[ink::test]
        fn the_final_take_closes_the_listing_and_tells_the_factory() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());

            set_caller(accounts.charlie);
            pay(u64::MAX / 2);
            assert_eq!(vault.take(LISTED, accounts.charlie), Ok(None));

            assert_eq!(vault.get_status(), Status::Closed);
            assert_eq!(listings_closed(), 1);
            assert_eq!(last_event::<Filled>().seller, accounts.alice);
            assert_eq!(
                vault.take(MIN_PURCHASE, accounts.charlie),
                Err(Error::NotOpen)
            );
        }

        #[ink::test]
        fn take_enforces_the_floor_the_payment_and_the_pause() {
            let accounts = accounts();
            // The seller's floor sits just above the market.
            let mut vault = open_vault(Terms {
                min_price: MARKET_PRICE + 1,
                ..Terms::default()
            });
            set_caller(accounts.charlie);
            pay(u64::MAX / 2);
            assert_eq!(
                vault.take(TWO_ALPHA, accounts.charlie),
                Err(Error::PriceBelowMinimum)
            );
            testing::set_alpha_price(MARKET_PRICE + 2);
            assert!(vault.take(TWO_ALPHA, accounts.charlie).is_ok());

            let mut vault = open_vault(Terms::default());
            set_caller(accounts.charlie);
            pay(TWO_ALPHA_TAO + TWO_ALPHA_FEE - 1);
            assert_eq!(
                vault.take(TWO_ALPHA, accounts.charlie),
                Err(Error::InsufficientPayment)
            );

            pay(u64::MAX / 2);
            set_factory(|factory| factory.paused = true);
            assert_eq!(
                vault.take(TWO_ALPHA, accounts.charlie),
                Err(Error::TradingPaused)
            );
            set_factory(|factory| factory.reachable = false);
            assert_eq!(
                vault.take(TWO_ALPHA, accounts.charlie),
                Err(Error::FactoryQueryFailed)
            );

            let mut pending = pending_vault(Terms::default());
            set_caller(accounts.charlie);
            assert_eq!(
                pending.take(TWO_ALPHA, accounts.charlie),
                Err(Error::NotOpen)
            );
        }

        #[ink::test]
        fn quote_matches_what_a_take_charges() {
            let vault = open_vault(Terms {
                price_offset_bps: 2_500,
                ..Terms::default()
            });

            let quote = vault.quote(TWO_ALPHA).expect("quote");
            assert_eq!(quote.price, 625_000_000);
            assert_eq!(quote.tao, 1_250_000_000);
            assert_eq!(quote.fee, 19_531_250);
            assert_eq!(quote.total, 1_269_531_250);
            assert_eq!(vault.quote(LISTED + 1), Err(Error::AmountExceedsRemaining));
        }

        #[ink::test]
        fn cancel_returns_the_stake_and_closes_the_listing() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());

            set_caller(accounts.django);
            assert_eq!(vault.cancel(), Err(Error::Unauthorized));

            set_caller(accounts.alice);
            assert_eq!(vault.cancel(), Ok(()));
            assert_eq!(vault.get_status(), Status::Closed);
            assert_eq!(vault.get_remaining(), 0);
            assert_eq!(testing::stake(validator(), accounts.alice, NETUID), LISTED);
            assert_eq!(vault_stake(validator()), 0);
            assert_eq!(listings_closed(), 1);
            assert_eq!(last_event::<Cancelled>().returned, LISTED);

            assert_eq!(vault.cancel(), Err(Error::AlreadyClosed));
        }

        #[ink::test]
        fn cancel_closes_even_when_the_stake_cannot_move() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());
            // For example, while the subnet is being dissolved. The factory being
            // paused or unreachable does not matter either.
            testing::set_reject_transfers(true);
            set_factory(|factory| {
                factory.paused = true;
                factory.reachable = false;
            });

            set_caller(accounts.alice);
            assert_eq!(vault.cancel(), Ok(()));
            assert_eq!(vault.get_status(), Status::Closed);
            assert_eq!(last_event::<Cancelled>().returned, 0);

            // Once Subtensor allows it, anyone sweeps the stake to the seller.
            testing::set_reject_transfers(false);
            set_caller(accounts.django);
            assert_eq!(vault.sweep_stake(), Ok(()));
            assert_eq!(testing::stake(validator(), accounts.alice, NETUID), LISTED);
        }

        #[ink::test]
        fn cancel_waits_for_the_delay_after_activation_but_not_while_pending() {
            let accounts = accounts();
            let mut vault = open_vault(Terms {
                cancel_delay: 10,
                ..Terms::default()
            });
            assert_eq!(
                vault.get_info().expect("info").cancellable_after,
                CREATED_AT + 10
            );
            set_caller(accounts.alice);
            assert_eq!(vault.cancel(), Err(Error::CancelTooEarly));
            test::set_block_number::<BittensorEnvironment>(CREATED_AT + 10);
            assert_eq!(vault.cancel(), Ok(()));

            // A funded vault the seller never activated returns its stake at once.
            let mut vault = pending_vault(Terms {
                cancel_delay: 10,
                ..Terms::default()
            });
            fund(LISTED);
            set_caller(accounts.alice);
            assert_eq!(vault.cancel(), Ok(()));
            assert_eq!(testing::stake(accounts.eve, accounts.alice, NETUID), LISTED);
        }

        #[ink::test]
        fn anyone_sweeps_staking_rewards_to_the_seller() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());
            set_caller(accounts.django);
            assert_eq!(vault.sweep_stake(), Err(Error::NothingToSweep));

            // Emissions accrue on the listed stake.
            testing::set_stake(validator(), accounts.frank, NETUID, LISTED + 5_000_000);
            assert_eq!(vault.sweep_stake(), Ok(()));
            assert_eq!(
                testing::stake(validator(), accounts.alice, NETUID),
                5_000_000
            );
            assert_eq!(vault.get_remaining(), LISTED);
            assert_eq!(vault_stake(validator()), LISTED);

            let swept = last_event::<StakeSwept>();
            assert_eq!(swept.seller, accounts.alice);
            assert_eq!(swept.initiated_by, accounts.django);
            assert_eq!(swept.amount, 5_000_000);

            let mut pending = pending_vault(Terms::default());
            fund(LISTED);
            assert_eq!(pending.sweep_stake(), Err(Error::StillPending));
        }

        #[ink::test]
        fn anyone_sweeps_tao_to_the_seller_without_closing() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());

            set_caller(accounts.django);
            assert_eq!(vault.sweep_tao(), Ok(()));
            assert_eq!(tao_transfers(), vec![(accounts.alice, VAULT_TAO_BALANCE)]);
            assert_eq!(vault.sweep_tao(), Err(Error::NoTaoToSweep));
            assert_eq!(vault.get_status(), Status::Open);

            let swept = last_event::<TaoSwept>();
            assert_eq!(swept.owner, accounts.alice);
            assert_eq!(swept.initiated_by, accounts.django);
        }

        #[ink::test]
        fn the_seller_closes_a_closed_vault_and_takes_what_is_left() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());

            set_caller(accounts.alice);
            assert_eq!(vault.close(), Err(Error::NotClosed));

            // A cancel that could not move the stake leaves it in the vault, and
            // closing cannot move it either.
            testing::set_reject_transfers(true);
            assert_eq!(vault.cancel(), Ok(()));
            assert_eq!(vault.close(), Err(Error::StakeRemaining));

            testing::set_reject_transfers(false);
            set_caller(accounts.django);
            assert_eq!(vault.close(), Err(Error::Unauthorized));

            set_caller(accounts.alice);
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| vault.close()));
            assert_terminated_to(result, accounts.alice, VAULT_TAO_BALANCE);
            assert_eq!(testing::stake(validator(), accounts.alice, NETUID), LISTED);
            assert_eq!(vault_stake(validator()), 0);
        }

        #[ink::test]
        fn closing_leaves_behind_only_dust_subtensor_will_not_move() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());
            set_caller(accounts.alice);
            assert_eq!(vault.cancel(), Ok(()));

            // Rewards that accrued after the cancel are worth less than Subtensor's
            // minimum transfer, even though they exceed BITTENSOR_MIN_STAKE in rao.
            let dust = BITTENSOR_MIN_STAKE * 2;
            testing::set_stake(validator(), accounts.frank, NETUID, dust);
            testing::set_min_transfer(dust + 1);
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| vault.close()));
            assert_terminated_to(result, accounts.alice, VAULT_TAO_BALANCE);
            assert_eq!(vault_stake(validator()), dust);
        }

        #[ink::test]
        fn closing_keeps_small_stake_that_failed_for_another_reason() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());
            set_caller(accounts.alice);
            assert_eq!(vault.cancel(), Ok(()));

            testing::set_stake(validator(), accounts.frank, NETUID, BITTENSOR_MIN_STAKE - 1);
            testing::set_reject_transfers(true);
            assert_eq!(vault.close(), Err(Error::StakeRemaining));
        }

        #[ink::test]
        fn anyone_can_resync_after_a_hotkey_swap() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());
            set_caller(accounts.django);

            assert_eq!(vault.resync_hotkey(validator()), Err(Error::InvalidHotkey));

            // A smaller or equal donation elsewhere cannot redirect the vault.
            testing::set_stake(accounts.charlie, accounts.frank, NETUID, LISTED);
            assert_eq!(
                vault.resync_hotkey(accounts.charlie),
                Err(Error::InsufficientStakeOnHotkey)
            );

            // A swap moves the vault's stake from the validator to charlie.
            testing::set_stake(validator(), accounts.frank, NETUID, 0);
            assert_eq!(vault.resync_hotkey(accounts.charlie), Ok(()));
            assert_eq!(vault.get_hotkey(), accounts.charlie);

            let resynced = last_event::<HotkeyResynced>();
            assert_eq!(resynced.old_hotkey, validator());
            assert_eq!(resynced.new_hotkey, accounts.charlie);
            assert_eq!(resynced.new_hotkey_stake, LISTED);
        }

        #[ink::test]
        fn anyone_consolidates_an_open_listing_to_the_new_validator() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());
            set_caller(accounts.django);
            assert_eq!(vault.consolidate(), Err(Error::AlreadyConsolidated));

            set_factory(|factory| factory.validator_hotkey = accounts.charlie);
            testing::set_transfer_loss(1);
            assert_eq!(vault.consolidate(), Ok(()));

            assert_eq!(vault.get_hotkey(), accounts.charlie);
            assert_eq!(vault_stake(accounts.charlie), LISTED - 1);
            assert_eq!(vault.get_remaining(), LISTED - 1);
            let consolidated = last_event::<Consolidated>();
            assert_eq!(consolidated.old_hotkey, validator());
            assert_eq!(consolidated.amount, LISTED - 1);

            let mut pending = pending_vault(Terms::default());
            assert_eq!(pending.consolidate(), Err(Error::NotOpen));
        }

        #[ink::test]
        fn alpha_only_reaches_the_seller_a_recipient_or_a_new_escrow() {
            let accounts = accounts();
            let mut vault = open_vault(Terms {
                lockup_duration: Some(LOCKUP),
                ..Terms::default()
            });

            set_caller(accounts.charlie);
            pay(u64::MAX / 2);
            let escrow = vault
                .take(TWO_ALPHA, accounts.django)
                .expect("take")
                .expect("escrow");
            testing::set_stake(validator(), accounts.frank, NETUID, LISTED);
            set_caller(accounts.django);
            vault.sweep_stake().expect("sweep");
            set_caller(accounts.alice);
            vault.cancel().expect("cancel");

            let destinations = testing::movements()
                .into_iter()
                .map(|movement| movement.destination_coldkey)
                .collect::<Vec<_>>();
            for destination in destinations {
                assert!(
                    [accounts.frank, accounts.alice, escrow].contains(&destination),
                    "unexpected destination {destination:?}"
                );
            }
            for (to, _) in tao_transfers() {
                assert!([accounts.alice, fee_collector(), accounts.charlie].contains(&to));
            }
        }

        #[ink::test]
        fn the_seller_adopts_current_code_and_others_wait_for_the_force_delay() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());

            set_caller(accounts.alice);
            assert_eq!(vault.upgrade(hash(2)), Err(Error::NoCurrentCode));
            set_registry_code(LISTING_VAULT, Some(hash(2)), None);
            assert_eq!(vault.upgrade(hash(3)), Err(Error::NotCurrentCode));

            set_caller(accounts.django);
            assert_eq!(vault.upgrade(hash(2)), Err(Error::ForceDelayNotElapsed));
            assert_eq!(installed_code(), None);

            set_caller(accounts.alice);
            assert_eq!(vault.upgrade(hash(2)), Ok(()));
            assert_eq!(installed_code(), Some(hash(2)));
            assert_eq!(last_event::<Upgraded>().initiated_by, accounts.alice);
        }

        #[ink::test]
        fn consent_only_blocks_forced_upgrades() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());
            set_registry_code(LISTING_VAULT, Some(hash(2)), Some(hash(2)));

            set_caller(accounts.django);
            assert_eq!(
                vault.set_upgrade_policy(UpgradePolicy::ConsentOnly),
                Err(Error::Unauthorized)
            );
            assert_eq!(vault.upgrade(hash(2)), Ok(()));

            set_caller(accounts.alice);
            assert_eq!(vault.set_upgrade_policy(UpgradePolicy::ConsentOnly), Ok(()));
            set_caller(accounts.django);
            assert_eq!(vault.upgrade(hash(2)), Err(Error::ConsentRequired));
        }

        #[ink::test]
        fn state_changing_messages_migrate_first() {
            let accounts = accounts();
            let mut vault = open_vault(Terms::default());
            vault.version = 0;

            set_caller(accounts.django);
            assert_eq!(vault.sweep_tao(), Ok(()));
            assert_eq!(vault.get_version(), STORAGE_VERSION);

            vault.version = 0;
            vault.migrate();
            assert_eq!(vault.get_version(), STORAGE_VERSION);
        }
    }
}
