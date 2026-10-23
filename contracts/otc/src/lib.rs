#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub mod errors;
pub mod events;
pub mod types;

pub use otc_shared::BittensorEnvironment;

/// TAO offers: a buyer deposits TAO to buy Alpha on a subnet at the market price
/// moved by their offset. TAO isn't tied to a subnet, so offers stay pooled here;
/// spot and lockup listings are vaults created by `lockup_listings`.
///
/// A seller fills an offer with `take_tao_offer`, which moves their stake straight
/// to the buyer through a proxy. Frontends add the proxy and remove it again in
/// the seller's own batch, so it never outlives the fill. Buyers can cancel at
/// any time, even while trading is paused.
#[ink::contract(env = otc_shared::BittensorEnvironment)]
mod otc_contract {
    use crate::errors::Error;
    use crate::events::*;
    use crate::types::{Config, OfferKey, OfferQuote, TaoOffer, TaoOfferId, UserOffers};
    use ink::prelude::vec::Vec;
    use ink::storage::Mapping;
    use otc_shared::events::Upgraded;
    use otc_shared::pricing::{apply_price_offset, price_to_fixed_decimal};
    use otc_shared::{
        check_upgrade, stake_delta_verified, stake_of, AlphaAmount, BlockAge, FixedDecimal, NetUid,
        PriceOffsetBps, PricingError, TaoAmount, UpgradeError, UpgradePolicy, MAX_FEE_RATE,
        MAX_PAUSE,
    };

    /// Storage layout version this code expects; see `ensure_migrated`.
    pub const STORAGE_VERSION: u16 = 1;

    /// Maximum number of open offers a buyer can have per subnet
    pub const MAX_OFFERS_PER_USER_PER_NETUID: usize = 25;

    /// Root storage. Upgrades keep this struct exactly as it is; add new state in
    /// `Lazy` or `Mapping` fields with explicit storage keys instead.
    #[ink(storage)]
    pub struct OtcContract {
        owner: AccountId,
        /// Proposed new owner, who must accept before taking over
        pending_owner: Option<AccountId>,
        /// Code registry for this contract's upgrades
        registry: AccountId,
        /// Storage layout version, see `STORAGE_VERSION`
        version: u16,
        tao_offers: Mapping<OfferKey, TaoOffer>,
        user_offers: UserOffers,
        next_tao_offer_id: TaoOfferId,
        fee_rate: FixedDecimal,
        fee_recipient: AccountId,
        min_offer_amount: TaoAmount,
        /// New offers and fills are paused before this block
        paused_until: BlockNumber,
    }

    impl OtcContract {
        #[ink(constructor)]
        pub fn new(
            owner: AccountId,
            registry: AccountId,
            fee_rate: u128,
            fee_recipient: AccountId,
            min_offer_amount: TaoAmount,
        ) -> Result<Self, Error> {
            Self::validate_fee_rate(fee_rate)?;

            Ok(Self {
                owner,
                pending_owner: None,
                registry,
                version: STORAGE_VERSION,
                tao_offers: Mapping::default(),
                user_offers: Mapping::default(),
                next_tao_offer_id: 1,
                fee_rate: FixedDecimal::from_bits(fee_rate),
                fee_recipient,
                min_offer_amount,
                paused_until: 0,
            })
        }

        /// Deposit the transferred TAO as an offer to buy Alpha on `netuid` at the
        /// market price moved by `price_offset_bps`.
        ///
        /// `max_price` is the buyer's ceiling (TAO per Alpha, scaled by 1e9; 0 for
        /// none): fills above it fail, which limits spot-price manipulation by a
        /// seller who pushes the price up before filling.
        #[ink(message, payable)]
        pub fn create_tao_offer(
            &mut self,
            netuid: NetUid,
            price_offset_bps: PriceOffsetBps,
            max_price: u64,
        ) -> Result<TaoOfferId, Error> {
            self.ensure_migrated();

            if self.is_paused() {
                return Err(Error::TradingPaused);
            }

            let buyer = self.env().caller();
            let amount = self.env().transferred_value();
            if amount < self.min_offer_amount {
                return Err(Error::AmountTooSmall);
            }
            if price_offset_bps <= -10_000 {
                return Err(Error::InvalidPriceOffset);
            }

            let mut buyer_offers = self.user_offers.get((buyer, netuid)).unwrap_or_default();
            if buyer_offers.len() >= MAX_OFFERS_PER_USER_PER_NETUID {
                return Err(Error::TooManyOffers);
            }

            let offer_id = self.next_tao_offer_id;
            self.next_tao_offer_id = offer_id.checked_add(1).ok_or(Error::Overflow)?;

            self.tao_offers.insert(
                (netuid, buyer, offer_id),
                &TaoOffer {
                    id: offer_id,
                    netuid,
                    buyer,
                    amount,
                    price_offset_bps,
                    max_price,
                    fee_rate: self.fee_rate,
                    created_at: self.env().block_number(),
                },
            );
            buyer_offers.push(offer_id);
            self.user_offers.insert((buyer, netuid), &buyer_offers);

            self.env().emit_event(TaoOfferCreated {
                buyer,
                netuid,
                tao_offer_id: offer_id,
                amount,
                price_offset_bps,
                max_price,
            });

            Ok(offer_id)
        }

        /// Withdraw an offer and get the TAO back. Never paused.
        #[ink(message)]
        pub fn cancel_tao_offer(
            &mut self,
            netuid: NetUid,
            offer_id: TaoOfferId,
        ) -> Result<(), Error> {
            self.ensure_migrated();

            let buyer = self.env().caller();
            let offer = self
                .tao_offers
                .get((netuid, buyer, offer_id))
                .ok_or(Error::OfferNotFound)?;

            self.remove_offer(&offer);
            self.transfer_tao(buyer, offer.amount)?;

            self.env().emit_event(TaoOfferCancelled {
                buyer,
                netuid,
                tao_offer_id: offer_id,
                amount_returned: offer.amount,
            });

            Ok(())
        }

        /// Fill a whole offer: move the Alpha it buys at the current price from the
        /// caller's stake on `hotkey` to the buyer, and receive the offer's TAO
        /// minus the fee. Returns the Alpha moved.
        ///
        /// The transfer goes through a proxy, so the caller must have added this
        /// contract as a `Transfer` proxy; frontends add and remove it in the same
        /// batch as this call. `max_alpha` is the seller's cap: the fill fails if
        /// the price has fallen far enough to need more Alpha than that.
        #[ink(message)]
        pub fn take_tao_offer(
            &mut self,
            netuid: NetUid,
            buyer: AccountId,
            offer_id: TaoOfferId,
            hotkey: AccountId,
            max_alpha: AlphaAmount,
        ) -> Result<AlphaAmount, Error> {
            self.ensure_migrated();

            if self.is_paused() {
                return Err(Error::TradingPaused);
            }

            let seller = self.env().caller();
            let offer = self
                .tao_offers
                .get((netuid, buyer, offer_id))
                .ok_or(Error::OfferNotFound)?;

            let quote = self.quote_offer(&offer)?;
            if offer.max_price > 0 && quote.price > offer.max_price {
                return Err(Error::PriceAboveMaximum);
            }
            if quote.alpha == 0 {
                return Err(Error::AmountTooSmall);
            }
            if quote.alpha > max_alpha {
                return Err(Error::AlphaAboveMaximum);
            }

            let stake =
                |coldkey| stake_of(hotkey, coldkey, netuid).map_err(|_| Error::StakeQueryFailed);
            let seller_before = stake(seller)?;
            if seller_before < quote.alpha {
                return Err(Error::InsufficientStake);
            }
            let buyer_before = stake(buyer)?;

            self.transfer_stake_as(seller, buyer, hotkey, netuid, quote.alpha)?;

            let seller_decrease = seller_before.saturating_sub(stake(seller)?);
            let buyer_increase = stake(buyer)?.saturating_sub(buyer_before);
            if !stake_delta_verified(seller_decrease, quote.alpha)
                || !stake_delta_verified(buyer_increase, quote.alpha)
            {
                Self::trap_stake_transfer_not_verified();
            }

            self.remove_offer(&offer);
            self.transfer_tao(seller, quote.tao)?;
            if quote.fee > 0 {
                self.transfer_tao(self.fee_recipient, quote.fee)?;
            }

            self.env().emit_event(TaoOfferTaken {
                seller,
                buyer,
                netuid,
                tao_offer_id: offer_id,
                alpha_amount: quote.alpha,
                tao_amount: quote.tao,
                executed_price: quote.price,
                fee: quote.fee,
            });

            Ok(quote.alpha)
        }

        // ============ Views ============

        #[ink(message)]
        pub fn get_offer(
            &self,
            netuid: NetUid,
            buyer: AccountId,
            offer_id: TaoOfferId,
        ) -> Option<TaoOffer> {
            self.tao_offers.get((netuid, buyer, offer_id))
        }

        #[ink(message)]
        pub fn get_user_offers(&self, buyer: AccountId, netuid: NetUid) -> Vec<TaoOfferId> {
            self.user_offers.get((buyer, netuid)).unwrap_or_default()
        }

        /// What filling an offer pays at the current market price. Sellers should
        /// pass a `max_alpha` a little above `alpha`, since the price can move
        /// before the fill lands.
        #[ink(message)]
        pub fn quote_offer_fill(
            &self,
            netuid: NetUid,
            buyer: AccountId,
            offer_id: TaoOfferId,
        ) -> Result<OfferQuote, Error> {
            let offer = self
                .tao_offers
                .get((netuid, buyer, offer_id))
                .ok_or(Error::OfferNotFound)?;
            self.quote_offer(&offer)
        }

        #[ink(message)]
        pub fn get_config(&self) -> Config {
            Config {
                owner: self.owner,
                registry: self.registry,
                fee_rate: self.fee_rate,
                fee_recipient: self.fee_recipient,
                min_offer_amount: self.min_offer_amount,
                paused_until: self.paused_until,
            }
        }

        #[ink(message)]
        pub fn is_trading_paused(&self) -> bool {
            self.is_paused()
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

        /// Set the fee rate for new offers, as U64F64 bits, at most `MAX_FEE_RATE`.
        /// Open offers keep the rate they were created with.
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
        pub fn set_min_offer_amount(&mut self, new_amount: TaoAmount) -> Result<(), Error> {
            self.ensure_owner()?;

            let old_amount = self.min_offer_amount;
            self.min_offer_amount = new_amount;
            self.env().emit_event(MinOfferAmountChanged {
                old_amount,
                new_amount,
            });
            Ok(())
        }

        /// Pause new offers and fills for `blocks` blocks, at most `MAX_PAUSE`.
        /// Calling it again restarts the pause from now. Cancels are never paused.
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

        /// Install `code_hash`, which must be the registry's current otc code. Only
        /// the owner can upgrade. The new code migrates storage on its next call,
        /// or when anyone calls `migrate`.
        #[ink(message)]
        pub fn upgrade(&mut self, code_hash: Hash) -> Result<(), Error> {
            use otc_shared::upgrade::OTC;

            self.ensure_owner()?;
            let current = self.registry_current(OTC)?;
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

        /// The fee comes out of the offer, and the rest buys Alpha at the market
        /// price moved by the offer's offset.
        fn quote_offer(&self, offer: &TaoOffer) -> Result<OfferQuote, Error> {
            let market_price = self
                .env()
                .extension()
                .get_alpha_price(offer.netuid)
                .map_err(|_| Error::MarketPriceFetchFailed)?;
            let price =
                apply_price_offset(market_price, offer.price_offset_bps).map_err(|error| {
                    match error {
                        PricingError::InvalidPriceOffset => Error::InvalidPriceOffset,
                        PricingError::Overflow => Error::Overflow,
                    }
                })?;

            let fee = offer
                .fee_rate
                .mul(offer.amount)
                .map_err(|_| Error::Overflow)?;
            let tao = offer.amount.checked_sub(fee).ok_or(Error::Overflow)?;
            let alpha = price_to_fixed_decimal(price)
                .as_divisor_of(tao)
                .map_err(|_| Error::Overflow)?;

            Ok(OfferQuote {
                price,
                alpha,
                tao,
                fee,
            })
        }

        fn remove_offer(&mut self, offer: &TaoOffer) {
            let (netuid, buyer, offer_id) = (offer.netuid, offer.buyer, offer.id);
            self.tao_offers.remove((netuid, buyer, offer_id));

            let mut buyer_offers = self.user_offers.get((buyer, netuid)).unwrap_or_default();
            buyer_offers.retain(|&id| id != offer_id);
            if buyer_offers.is_empty() {
                self.user_offers.remove((buyer, netuid));
            } else {
                self.user_offers.insert((buyer, netuid), &buyer_offers);
            }
        }

        fn is_paused(&self) -> bool {
            self.env().block_number() < self.paused_until
        }

        fn validate_fee_rate(fee_rate: u128) -> Result<(), Error> {
            if fee_rate > MAX_FEE_RATE {
                return Err(Error::FeeRateTooHigh);
            }
            Ok(())
        }

        fn upgrade_error(error: UpgradeError) -> Error {
            match error {
                UpgradeError::NoCurrentCode => Error::NoCurrentCode,
                // The owner always has consent, so only a wrong hash remains.
                _ => Error::NotCurrentCode,
            }
        }

        /// A transfer that completed but moved the wrong amount must revert the
        /// whole call, including the transfer itself.
        fn trap_stake_transfer_not_verified() -> ! {
            panic!("post-transfer stake verification failed")
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

        /// Move `amount` of `seller`'s stake on `hotkey` to `buyer` through the
        /// seller's proxy. The one `call_runtime` left: if Subtensor renumbers its
        /// calls, an upgrade fixes the encoding in `otc-shared`.
        #[cfg(not(test))]
        fn transfer_stake_as(
            &self,
            seller: AccountId,
            buyer: AccountId,
            hotkey: AccountId,
            netuid: NetUid,
            amount: AlphaAmount,
        ) -> Result<(), Error> {
            use ink::prelude::boxed::Box;
            use otc_shared::runtime::NetUid as RuntimeNetUid;
            use otc_shared::{AlphaCurrency, ProxyCall, RuntimeCall, SubtensorCall};
            use sp_runtime::MultiAddress;

            let transfer = RuntimeCall::SubtensorModule(SubtensorCall::TransferStake {
                destination_coldkey: buyer,
                hotkey,
                origin_netuid: RuntimeNetUid::from(netuid),
                destination_netuid: RuntimeNetUid::from(netuid),
                alpha_amount: AlphaCurrency::from(amount),
            });
            let proxied = RuntimeCall::Proxy(ProxyCall::Proxy {
                real: MultiAddress::Id(seller),
                force_proxy_type: None,
                call: Box::new(transfer),
            });

            self.env()
                .call_runtime(&proxied)
                .map_err(|_| Error::RuntimeCallFailed)
        }

        #[cfg(not(test))]
        fn transfer_tao(&mut self, to: AccountId, amount: Balance) -> Result<(), Error> {
            self.env()
                .transfer(to, amount)
                .map_err(|_| Error::TransferFailed)
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
        fn install_code(&mut self, code_hash: Hash) -> Result<(), Error> {
            self.env()
                .set_code_hash(&code_hash)
                .map_err(|_| Error::CodeUpgradeFailed)
        }

        // ink!'s off-chain test environment cannot dispatch runtime calls, call
        // other contracts, replace code, or transfer a u64 balance, so unit tests
        // simulate the proxied transfer on the Subtensor mock, answer registry
        // queries and record upgrades and TAO transfers here; localnet integration
        // tests cover the real calls.
        #[cfg(test)]
        fn transfer_stake_as(
            &self,
            seller: AccountId,
            buyer: AccountId,
            hotkey: AccountId,
            netuid: NetUid,
            amount: AlphaAmount,
        ) -> Result<(), Error> {
            tests::proxied_transfer(seller, buyer, hotkey, netuid, amount)
        }

        #[cfg(test)]
        fn transfer_tao(&mut self, to: AccountId, amount: Balance) -> Result<(), Error> {
            tests::record_tao_transfer(to, amount);
            Ok(())
        }

        #[cfg(test)]
        fn registry_current(&self, kind: otc_shared::CodeKind) -> Result<Option<Hash>, Error> {
            Ok(tests::registry_current(kind))
        }

        #[cfg(test)]
        fn install_code(&mut self, code_hash: Hash) -> Result<(), Error> {
            tests::record_installed_code(code_hash);
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
        use ink::scale::Decode;
        use otc_shared::testing::{self, MockSubtensor};
        use otc_shared::upgrade::OTC;
        use otc_shared::{BittensorEnvironment, CodeKind, TRANSFER_TOLERANCE};
        use std::cell::RefCell;
        use std::collections::BTreeMap;

        type TestAccountId = <BittensorEnvironment as Environment>::AccountId;

        const NETUID: NetUid = 1;
        const NOW: BlockNumber = 100;
        const MIN_OFFER: TaoAmount = 1_000_000_000;
        // A 1/64 fee and prices of 0.5 and 0.625 TAO per Alpha are exact in fixed
        // point, so results are exact too.
        const MARKET_PRICE: u64 = 500_000_000;
        const OFFER: TaoAmount = 64_000_000_000;
        const OFFER_FEE: TaoAmount = 1_000_000_000;
        const SELLER_TAO: TaoAmount = OFFER - OFFER_FEE;
        // 63 TAO at 0.5 TAO per Alpha
        const ALPHA_AT_MARKET: AlphaAmount = 126_000_000_000;
        const SELLER_STAKE: AlphaAmount = 1_000_000_000_000;

        thread_local! {
            static REGISTRY: RefCell<BTreeMap<CodeKind, Hash>> = const { RefCell::new(BTreeMap::new()) };
            static INSTALLED_CODE: RefCell<Option<Hash>> = const { RefCell::new(None) };
            static TAO_TRANSFERS: RefCell<Vec<(TestAccountId, Balance)>> =
                const { RefCell::new(Vec::new()) };
            static PROXY_ADDED: RefCell<bool> = const { RefCell::new(true) };
            /// Rao the buyer loses on the proxied transfer
            static PROXY_LOSS: RefCell<AlphaAmount> = const { RefCell::new(0) };
        }

        /// Moves the stake on the mock like Subtensor's proxied `transfer_stake`,
        /// and fails like it when the seller has not added the proxy.
        pub(super) fn proxied_transfer(
            seller: TestAccountId,
            buyer: TestAccountId,
            hotkey: TestAccountId,
            netuid: NetUid,
            amount: AlphaAmount,
        ) -> Result<(), Error> {
            if !PROXY_ADDED.with(|added| *added.borrow()) {
                return Err(Error::RuntimeCallFailed);
            }
            let seller_stake = testing::stake(hotkey, seller, netuid);
            let buyer_stake = testing::stake(hotkey, buyer, netuid);
            let loss = PROXY_LOSS.with(|loss| *loss.borrow());
            testing::set_stake(hotkey, seller, netuid, seller_stake - amount);
            testing::set_stake(hotkey, buyer, netuid, buyer_stake + amount - loss);
            Ok(())
        }

        pub(super) fn record_tao_transfer(to: TestAccountId, amount: Balance) {
            TAO_TRANSFERS.with(|transfers| transfers.borrow_mut().push((to, amount)));
        }

        fn tao_transfers() -> Vec<(TestAccountId, Balance)> {
            TAO_TRANSFERS.with(|transfers| transfers.borrow().clone())
        }

        pub(super) fn registry_current(kind: CodeKind) -> Option<Hash> {
            REGISTRY.with(|registry| registry.borrow().get(&kind).copied())
        }

        pub(super) fn record_installed_code(code_hash: Hash) {
            INSTALLED_CODE.with(|installed| *installed.borrow_mut() = Some(code_hash));
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

        fn fee_rate() -> u128 {
            (U64F64::from_num(1) / 64).to_bits()
        }

        fn set_caller(account: TestAccountId) {
            test::set_caller::<BittensorEnvironment>(account);
        }

        /// ink!'s off-chain engine stores transferred values as u128 and only sets
        /// them for u128 environments; the contract reads them back as u64.
        fn pay(value: Balance) {
            test::set_value_transferred::<ink::env::DefaultEnvironment>(u128::from(value));
        }

        fn last_event<E: Decode>() -> E {
            let events = test::recorded_events().collect::<Vec<_>>();
            E::decode(&mut &events.last().expect("an event").data[..]).expect("decode event")
        }

        /// Roles: alice owns the contract, bob buys, charlie sells, django is a
        /// third party, eve is the hotkey, and frank is the contract itself.
        fn otc() -> OtcContract {
            let accounts = accounts();
            MockSubtensor::install(accounts.frank);
            testing::set_alpha_price(MARKET_PRICE);
            testing::set_stake(accounts.eve, accounts.charlie, NETUID, SELLER_STAKE);
            REGISTRY.with(|registry| registry.borrow_mut().clear());
            INSTALLED_CODE.with(|installed| *installed.borrow_mut() = None);
            TAO_TRANSFERS.with(|transfers| transfers.borrow_mut().clear());
            PROXY_ADDED.with(|added| *added.borrow_mut() = true);
            PROXY_LOSS.with(|loss| *loss.borrow_mut() = 0);
            test::set_block_number::<BittensorEnvironment>(NOW);
            test::set_callee::<BittensorEnvironment>(accounts.frank);
            set_caller(accounts.alice);

            OtcContract::new(
                accounts.alice,
                registry_account(),
                fee_rate(),
                fee_collector(),
                MIN_OFFER,
            )
            .expect("valid config")
        }

        /// Bob offers OFFER TAO at `offset` with a `max_price` ceiling.
        fn offer(otc: &mut OtcContract, offset: PriceOffsetBps, max_price: u64) -> TaoOfferId {
            set_caller(accounts().bob);
            pay(OFFER);
            otc.create_tao_offer(NETUID, offset, max_price)
                .expect("create offer")
        }

        fn fill(
            otc: &mut OtcContract,
            offer_id: TaoOfferId,
            max_alpha: AlphaAmount,
        ) -> Result<AlphaAmount, Error> {
            let accounts = accounts();
            set_caller(accounts.charlie);
            otc.take_tao_offer(NETUID, accounts.bob, offer_id, accounts.eve, max_alpha)
        }

        #[ink::test]
        fn constructor_caps_the_fee() {
            let accounts = accounts();
            assert!(matches!(
                OtcContract::new(
                    accounts.alice,
                    registry_account(),
                    MAX_FEE_RATE + 1,
                    fee_collector(),
                    MIN_OFFER
                ),
                Err(Error::FeeRateTooHigh)
            ));

            let otc = otc();
            let config = otc.get_config();
            assert_eq!(config.owner, accounts.alice);
            assert_eq!(config.registry, registry_account());
            assert_eq!(config.fee_recipient, fee_collector());
            assert_eq!(config.min_offer_amount, MIN_OFFER);
            assert_eq!(otc.get_version(), STORAGE_VERSION);
        }

        #[ink::test]
        fn buyers_create_offers_within_the_limits() {
            let accounts = accounts();
            let mut otc = otc();

            let offer_id = offer(&mut otc, 2_500, 700_000_000);
            let stored = otc
                .get_offer(NETUID, accounts.bob, offer_id)
                .expect("offer");
            assert_eq!(stored.amount, OFFER);
            assert_eq!(stored.max_price, 700_000_000);
            assert_eq!(stored.fee_rate.to_bits(), fee_rate());
            assert_eq!(otc.get_user_offers(accounts.bob, NETUID), vec![offer_id]);
            let created = last_event::<TaoOfferCreated>();
            assert_eq!(created.buyer, accounts.bob);
            assert_eq!(created.max_price, 700_000_000);

            pay(MIN_OFFER - 1);
            assert_eq!(
                otc.create_tao_offer(NETUID, 0, 0),
                Err(Error::AmountTooSmall)
            );
            pay(MIN_OFFER);
            assert_eq!(
                otc.create_tao_offer(NETUID, -10_000, 0),
                Err(Error::InvalidPriceOffset)
            );
            for _ in 1..MAX_OFFERS_PER_USER_PER_NETUID {
                otc.create_tao_offer(NETUID, 0, 0).expect("offer");
            }
            assert_eq!(
                otc.create_tao_offer(NETUID, 0, 0),
                Err(Error::TooManyOffers)
            );
        }

        #[ink::test]
        fn a_fill_moves_the_sellers_alpha_and_pays_them_the_offer_minus_the_fee() {
            let accounts = accounts();
            let mut otc = otc();
            let offer_id = offer(&mut otc, 0, 0);

            let quote = otc
                .quote_offer_fill(NETUID, accounts.bob, offer_id)
                .expect("quote");
            assert_eq!(quote.price, MARKET_PRICE);
            assert_eq!(quote.fee, OFFER_FEE);
            assert_eq!(quote.tao, SELLER_TAO);
            assert_eq!(quote.alpha, ALPHA_AT_MARKET);

            assert_eq!(
                fill(&mut otc, offer_id, ALPHA_AT_MARKET),
                Ok(ALPHA_AT_MARKET)
            );
            assert_eq!(
                testing::stake(accounts.eve, accounts.bob, NETUID),
                ALPHA_AT_MARKET
            );
            assert_eq!(
                testing::stake(accounts.eve, accounts.charlie, NETUID),
                SELLER_STAKE - ALPHA_AT_MARKET
            );
            assert_eq!(
                tao_transfers(),
                vec![(accounts.charlie, SELLER_TAO), (fee_collector(), OFFER_FEE)]
            );
            assert_eq!(otc.get_offer(NETUID, accounts.bob, offer_id), None);
            assert!(otc.get_user_offers(accounts.bob, NETUID).is_empty());

            let taken = last_event::<TaoOfferTaken>();
            assert_eq!(taken.seller, accounts.charlie);
            assert_eq!(taken.alpha_amount, ALPHA_AT_MARKET);
            assert_eq!(taken.tao_amount, SELLER_TAO);
            assert_eq!(taken.fee, OFFER_FEE);
        }

        #[ink::test]
        fn the_buyers_ceiling_and_the_sellers_cap_guard_the_price() {
            let mut otc = otc();
            // The buyer pays at most 0.55 TAO per Alpha.
            let offer_id = offer(&mut otc, 0, 550_000_000);

            // A seller who pushed the spot price up is refused.
            testing::set_alpha_price(600_000_000);
            assert_eq!(
                fill(&mut otc, offer_id, u64::MAX),
                Err(Error::PriceAboveMaximum)
            );

            // When the price falls, the seller's cap stops a fill that needs more
            // Alpha than they agreed to.
            testing::set_alpha_price(MARKET_PRICE / 2);
            assert_eq!(
                fill(&mut otc, offer_id, ALPHA_AT_MARKET),
                Err(Error::AlphaAboveMaximum)
            );
            assert_eq!(
                fill(&mut otc, offer_id, 2 * ALPHA_AT_MARKET),
                Ok(2 * ALPHA_AT_MARKET)
            );
        }

        #[ink::test]
        fn fills_need_the_proxy_and_the_stake() {
            let accounts = accounts();
            let mut otc = otc();
            let offer_id = offer(&mut otc, 0, 0);

            PROXY_ADDED.with(|added| *added.borrow_mut() = false);
            assert_eq!(
                fill(&mut otc, offer_id, u64::MAX),
                Err(Error::RuntimeCallFailed)
            );

            PROXY_ADDED.with(|added| *added.borrow_mut() = true);
            testing::set_stake(accounts.eve, accounts.charlie, NETUID, ALPHA_AT_MARKET - 1);
            assert_eq!(
                fill(&mut otc, offer_id, u64::MAX),
                Err(Error::InsufficientStake)
            );
            assert!(tao_transfers().is_empty());
            assert!(otc.get_offer(NETUID, accounts.bob, offer_id).is_some());
        }

        #[ink::test]
        fn a_fill_that_moves_the_wrong_amount_traps() {
            let accounts = accounts();
            let mut otc = otc();
            let offer_id = offer(&mut otc, 0, 0);
            PROXY_LOSS.with(|loss| *loss.borrow_mut() = TRANSFER_TOLERANCE + 1);

            let result =
                std::panic::catch_unwind(AssertUnwindSafe(|| fill(&mut otc, offer_id, u64::MAX)));
            let payload = result.expect_err("the fill should trap");
            assert_eq!(
                payload.downcast_ref::<&str>(),
                Some(&"post-transfer stake verification failed")
            );
            assert!(tao_transfers().is_empty());
            assert!(otc.get_offer(NETUID, accounts.bob, offer_id).is_some());
        }

        #[ink::test]
        fn buyers_cancel_at_any_time_even_while_paused() {
            let accounts = accounts();
            let mut otc = otc();
            let offer_id = offer(&mut otc, 0, 0);

            set_caller(accounts.alice);
            assert_eq!(otc.pause(10), Ok(()));

            set_caller(accounts.django);
            assert_eq!(
                otc.cancel_tao_offer(NETUID, offer_id),
                Err(Error::OfferNotFound)
            );

            set_caller(accounts.bob);
            assert_eq!(otc.cancel_tao_offer(NETUID, offer_id), Ok(()));
            assert_eq!(tao_transfers(), vec![(accounts.bob, OFFER)]);
            assert_eq!(last_event::<TaoOfferCancelled>().amount_returned, OFFER);
            assert_eq!(
                otc.cancel_tao_offer(NETUID, offer_id),
                Err(Error::OfferNotFound)
            );
        }

        #[ink::test]
        fn pauses_block_new_offers_and_fills_and_expire() {
            let accounts = accounts();
            let mut otc = otc();
            let offer_id = offer(&mut otc, 0, 0);

            set_caller(accounts.django);
            assert_eq!(otc.pause(10), Err(Error::Unauthorized));

            set_caller(accounts.alice);
            assert_eq!(otc.pause(0), Err(Error::InvalidPauseDuration));
            assert_eq!(otc.pause(MAX_PAUSE + 1), Err(Error::InvalidPauseDuration));
            assert_eq!(otc.pause(10), Ok(()));
            assert!(otc.is_trading_paused());

            set_caller(accounts.bob);
            pay(OFFER);
            assert_eq!(
                otc.create_tao_offer(NETUID, 0, 0),
                Err(Error::TradingPaused)
            );
            assert_eq!(
                fill(&mut otc, offer_id, u64::MAX),
                Err(Error::TradingPaused)
            );

            test::set_block_number::<BittensorEnvironment>(NOW + 10);
            assert!(!otc.is_trading_paused());
            assert!(fill(&mut otc, offer_id, u64::MAX).is_ok());

            set_caller(accounts.alice);
            assert_eq!(otc.pause(MAX_PAUSE), Ok(()));
            assert_eq!(otc.resume(), Ok(()));
            assert!(!otc.is_trading_paused());
        }

        #[ink::test]
        fn only_the_owner_changes_settings_and_the_fee_is_capped() {
            let accounts = accounts();
            let mut otc = otc();

            set_caller(accounts.django);
            assert_eq!(otc.set_fee_rate(0), Err(Error::Unauthorized));
            assert_eq!(
                otc.set_fee_recipient(accounts.django),
                Err(Error::Unauthorized)
            );
            assert_eq!(otc.set_min_offer_amount(1), Err(Error::Unauthorized));
            assert_eq!(
                otc.transfer_ownership(Some(accounts.django)),
                Err(Error::Unauthorized)
            );

            set_caller(accounts.alice);
            assert_eq!(
                otc.set_fee_rate(MAX_FEE_RATE + 1),
                Err(Error::FeeRateTooHigh)
            );
            assert_eq!(otc.set_fee_rate(MAX_FEE_RATE), Ok(()));
            assert_eq!(otc.set_fee_recipient(accounts.django), Ok(()));
            assert_eq!(otc.set_min_offer_amount(42), Ok(()));
            let config = otc.get_config();
            assert_eq!(config.fee_rate.to_bits(), MAX_FEE_RATE);
            assert_eq!(config.fee_recipient, accounts.django);
            assert_eq!(config.min_offer_amount, 42);
        }

        #[ink::test]
        fn open_offers_keep_their_fee_rate() {
            let accounts = accounts();
            let mut otc = otc();
            let offer_id = offer(&mut otc, 0, 0);

            set_caller(accounts.alice);
            otc.set_fee_rate(MAX_FEE_RATE).expect("owner");
            assert_eq!(
                otc.quote_offer_fill(NETUID, accounts.bob, offer_id)
                    .expect("quote")
                    .fee,
                OFFER_FEE
            );
        }

        #[ink::test]
        fn ownership_changes_only_when_the_new_owner_accepts() {
            let accounts = accounts();
            let mut otc = otc();

            set_caller(accounts.alice);
            assert_eq!(otc.transfer_ownership(Some(accounts.django)), Ok(()));
            set_caller(accounts.eve);
            assert_eq!(otc.accept_ownership(), Err(Error::Unauthorized));
            set_caller(accounts.django);
            assert_eq!(otc.accept_ownership(), Ok(()));
            assert_eq!(otc.get_owner(), accounts.django);
            assert_eq!(otc.get_pending_owner(), None);
            set_caller(accounts.alice);
            assert_eq!(otc.set_fee_rate(0), Err(Error::Unauthorized));
        }

        #[ink::test]
        fn only_the_owner_upgrades_to_the_registry_current_code() {
            let accounts = accounts();
            let mut otc = otc();

            set_caller(accounts.django);
            assert_eq!(otc.upgrade(hash(3)), Err(Error::Unauthorized));

            set_caller(accounts.alice);
            assert_eq!(otc.upgrade(hash(3)), Err(Error::NoCurrentCode));
            REGISTRY.with(|registry| registry.borrow_mut().insert(OTC, hash(3)));
            assert_eq!(otc.upgrade(hash(4)), Err(Error::NotCurrentCode));
            assert_eq!(otc.upgrade(hash(3)), Ok(()));
            assert_eq!(
                INSTALLED_CODE.with(|installed| *installed.borrow()),
                Some(hash(3))
            );
        }

        #[ink::test]
        fn state_changing_messages_migrate_first() {
            let mut otc = otc();
            otc.version = 0;
            offer(&mut otc, 0, 0);
            assert_eq!(otc.get_version(), STORAGE_VERSION);

            otc.version = 0;
            otc.migrate();
            assert_eq!(otc.get_version(), STORAGE_VERSION);
        }
    }

    /// Upgraded code must decode the storage this code writes; see
    /// `otc_shared::storage_layout`.
    #[cfg(test)]
    mod storage_layout {
        #[test]
        fn matches_its_snapshot() {
            otc_shared::storage_layout::check_snapshot::<super::OtcContract>(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/storage-layout.json"
            ));
        }
    }
}
