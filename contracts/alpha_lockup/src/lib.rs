#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub use self::alpha_lockup::{AlphaLockup, AlphaLockupRef};

/// Per-purchase escrow: holds the buyer's Alpha on its own coldkey until
/// `unlock_block`, then anyone can pay it out to the current beneficiary.
///
/// The escrow upgrades itself, but only to the code registry's current escrow
/// code: the beneficiary can adopt it at any time, and anyone can apply it once it
/// has been current for the registry's force delay, unless the beneficiary chose
/// `ConsentOnly`.
#[ink::contract(env = otc_shared::BittensorEnvironment)]
mod alpha_lockup {
    use otc_shared::events::{HotkeyResynced, TaoSwept, UpgradePolicyChanged, Upgraded};
    use otc_shared::{
        check_upgrade, stake_of, transfer_stake_verified, AlphaAmount, NetUid, TransferError,
        UpgradeError, UpgradePolicy,
    };

    /// Storage layout version this code expects. When an upgrade adds storage,
    /// bump it and initialise the new storage in `ensure_migrated`. New fields go
    /// in `Lazy` or `Mapping` storage; the root struct below never changes shape.
    #[cfg(not(feature = "upgrade-test"))]
    pub const STORAGE_VERSION: u16 = 1;
    #[cfg(feature = "upgrade-test")]
    pub const STORAGE_VERSION: u16 = 2;

    /// Error types for the Alpha Lockup escrow contract
    #[derive(Debug, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    pub enum Error {
        /// Caller is not allowed to perform this action
        Unauthorized,
        /// Lockup period has not expired yet
        LockupNotExpired,
        /// Failed to query stake
        StakeQueryFailed,
        /// Failed to transfer stake
        StakeTransferFailed,
        /// Proposed beneficiary is invalid
        InvalidBeneficiary,
        /// There is no pending beneficiary transfer
        NoPendingBeneficiary,
        /// No Alpha is available on the escrow hotkey
        NoAlphaToClaim,
        /// A beneficiary transfer is pending; resolve it before payout
        BeneficiaryTransferPending,
        /// The escrow holds no TAO to sweep
        NoTaoToSweep,
        /// Failed to transfer TAO
        TaoTransferFailed,
        /// The new hotkey is the hotkey the escrow already uses
        InvalidHotkey,
        /// The escrow does not hold more stake on the new hotkey than on its current one
        InsufficientStakeOnHotkey,
        /// The code registry has no current escrow code
        NoCurrentCode,
        /// The requested code is not the registry's current escrow code
        NotCurrentCode,
        /// Only the beneficiary may upgrade this escrow
        ConsentRequired,
        /// The current code has not been current for the force delay yet
        ForceDelayNotElapsed,
        /// Failed to query the code registry
        RegistryQueryFailed,
        /// Failed to replace the contract code
        CodeUpgradeFailed,
        /// The escrow still holds stake on its hotkey; claim it before closing
        StakeRemaining,
    }

    /// Event emitted when Alpha is claimed
    #[ink::event]
    pub struct AlphaClaimed {
        #[ink(topic)]
        pub buyer: AccountId,
        #[ink(topic)]
        pub initiated_by: AccountId,
        #[ink(topic)]
        pub netuid: NetUid,
        pub amount_claimed: AlphaAmount,
        pub unlock_block: BlockNumber,
    }

    /// Event emitted when a beneficiary transfer is proposed
    #[ink::event]
    pub struct BeneficiaryTransferProposed {
        #[ink(topic)]
        pub current_beneficiary: AccountId,
        #[ink(topic)]
        pub proposed_beneficiary: AccountId,
    }

    /// Event emitted when a beneficiary transfer proposal is cancelled
    #[ink::event]
    pub struct BeneficiaryTransferCancelled {
        #[ink(topic)]
        pub beneficiary: AccountId,
        #[ink(topic)]
        pub cancelled_beneficiary: AccountId,
    }

    /// Event emitted when a beneficiary transfer is accepted
    #[ink::event]
    pub struct BeneficiaryTransferred {
        #[ink(topic)]
        pub old_beneficiary: AccountId,
        #[ink(topic)]
        pub new_beneficiary: AccountId,
    }

    /// Event emitted when the beneficiary closes the escrow
    #[ink::event]
    pub struct EscrowClosed {
        #[ink(topic)]
        pub beneficiary: AccountId,
        pub tao_balance: Balance,
    }

    /// Information about a lockup escrow
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    pub struct LockupInfo {
        pub buyer: AccountId,
        pub pending_beneficiary: Option<AccountId>,
        pub netuid: NetUid,
        pub alpha_amount: AlphaAmount,
        pub unlock_block: BlockNumber,
        pub hotkey: AccountId,
        pub claimed: bool,
        pub current_stake: AlphaAmount,
        pub tao_balance: Balance,
        pub upgrade_policy: UpgradePolicy,
        pub version: u16,
    }

    /// Root storage. Upgrades keep this struct exactly as it is; add new state in
    /// `Lazy` or `Mapping` fields with explicit storage keys instead.
    #[ink(storage)]
    pub struct AlphaLockup {
        /// The current beneficiary, who receives the locked Alpha
        buyer: AccountId,
        /// Pending beneficiary for two-step coldkey swaps
        pending_beneficiary: Option<AccountId>,
        /// The subnet ID
        netuid: NetUid,
        /// Original Alpha amount locked (for reference)
        alpha_amount: AlphaAmount,
        /// Block number when Alpha can be claimed
        unlock_block: BlockNumber,
        /// The hotkey the stake is held on
        hotkey: AccountId,
        /// Whether a claim has paid out
        claimed: bool,
        /// Code registry that approves this escrow's upgrades
        registry: AccountId,
        /// Who may upgrade this escrow
        upgrade_policy: UpgradePolicy,
        /// Storage layout version, see `STORAGE_VERSION`
        version: u16,
    }

    impl AlphaLockup {
        /// Create a new lockup escrow
        /// Called by the lockup_listings contract when a buyer takes a portion
        #[ink(constructor)]
        pub fn new(
            buyer: AccountId,
            netuid: NetUid,
            alpha_amount: AlphaAmount,
            unlock_block: BlockNumber,
            hotkey: AccountId,
            registry: AccountId,
        ) -> Self {
            Self {
                buyer,
                pending_beneficiary: None,
                netuid,
                alpha_amount,
                unlock_block,
                hotkey,
                claimed: false,
                registry,
                upgrade_policy: UpgradePolicy::default(),
                version: STORAGE_VERSION,
            }
        }

        /// Pay the escrow's stake on its hotkey, including staking rewards, to the
        /// current beneficiary once the lockup has expired. Anyone can call it.
        ///
        /// Claiming never closes the escrow. Stake can still sit on another hotkey,
        /// for example after a validator hotkey swap, and anyone can claim it after
        /// `resync_hotkey`. If claiming terminated the escrow, anyone could strand
        /// that stake by donating a little on the old hotkey and claiming first.
        /// The beneficiary closes the escrow with `close`.
        #[ink(message)]
        pub fn claim(&mut self) -> Result<(), Error> {
            self.ensure_migrated();

            // Time check: must be past unlock block
            if self.env().block_number() < self.unlock_block {
                return Err(Error::LockupNotExpired);
            }

            // Claim is permissionless, but while a beneficiary handoff is pending the
            // rightful recipient is ambiguous (the current buyer key may be being
            // abandoned or compromised). Block payout until the proposal is resolved.
            if self.pending_beneficiary.is_some() {
                return Err(Error::BeneficiaryTransferPending);
            }

            // Get current stake (original + dividends)
            let current_stake = self.get_current_stake()?;
            if current_stake == 0 {
                return Err(Error::NoAlphaToClaim);
            }

            let beneficiary = self.buyer;
            transfer_stake_verified(beneficiary, self.hotkey, self.netuid, current_stake)
                .map_err(Self::transfer_error)?;

            self.claimed = true;

            self.env().emit_event(AlphaClaimed {
                buyer: beneficiary,
                initiated_by: self.env().caller(),
                netuid: self.netuid,
                amount_claimed: current_stake,
                unlock_block: self.unlock_block,
            });

            Ok(())
        }

        /// Terminate the escrow, sending its TAO balance and the storage deposit
        /// to the beneficiary.
        ///
        /// Only the beneficiary can close, because only they can judge that no
        /// stake or liquidation TAO is still on its way: a terminated escrow
        /// loses anything that arrives later. It fails while the escrow holds
        /// stake on its hotkey.
        #[ink(message)]
        pub fn close(&mut self) -> Result<(), Error> {
            self.ensure_beneficiary()?;

            if self.pending_beneficiary.is_some() {
                return Err(Error::BeneficiaryTransferPending);
            }

            if self.get_current_stake()? > 0 {
                return Err(Error::StakeRemaining);
            }

            let beneficiary = self.buyer;
            self.env().emit_event(EscrowClosed {
                beneficiary,
                tao_balance: self.env().balance(),
            });

            self.env().terminate_contract(beneficiary)
        }

        /// Send the escrow's whole TAO balance to the beneficiary.
        ///
        /// TAO only reaches an escrow when its subnet is dissolved and Subtensor
        /// liquidates the stake, or when someone sends it. Anyone can sweep at any
        /// time. Sweeping never terminates the escrow, because liquidation can credit
        /// TAO in several steps, and credits to a terminated escrow would be lost.
        #[ink(message)]
        pub fn sweep_tao(&mut self) -> Result<(), Error> {
            self.ensure_migrated();

            if self.pending_beneficiary.is_some() {
                return Err(Error::BeneficiaryTransferPending);
            }

            let amount = self.env().balance();
            if amount == 0 {
                return Err(Error::NoTaoToSweep);
            }

            let beneficiary = self.buyer;
            self.transfer_tao(beneficiary, amount)?;

            self.env().emit_event(TaoSwept {
                owner: beneficiary,
                initiated_by: self.env().caller(),
                amount,
            });

            Ok(())
        }

        /// Point the escrow at `new_hotkey` after its stake moved there, for
        /// example through a validator hotkey swap. Moves nothing.
        ///
        /// Anyone can call this. It is accepted only when the escrow holds more
        /// stake on `new_hotkey` than on its current hotkey, so pointing it
        /// elsewhere would take a donation larger than the escrow's own stake, and
        /// that donation belongs to the beneficiary.
        #[ink(message)]
        pub fn resync_hotkey(&mut self, new_hotkey: AccountId) -> Result<(), Error> {
            self.ensure_migrated();

            if new_hotkey == self.hotkey {
                return Err(Error::InvalidHotkey);
            }

            let escrow = self.env().account_id();
            let current_stake =
                stake_of(self.hotkey, escrow, self.netuid).map_err(|_| Error::StakeQueryFailed)?;
            let new_hotkey_stake =
                stake_of(new_hotkey, escrow, self.netuid).map_err(|_| Error::StakeQueryFailed)?;
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

        /// Propose a new beneficiary for coldkey swaps
        #[ink(message)]
        pub fn propose_beneficiary(&mut self, new_beneficiary: AccountId) -> Result<(), Error> {
            self.ensure_migrated();
            self.ensure_beneficiary()?;

            if new_beneficiary == self.buyer {
                return Err(Error::InvalidBeneficiary);
            }

            self.pending_beneficiary = Some(new_beneficiary);

            self.env().emit_event(BeneficiaryTransferProposed {
                current_beneficiary: self.buyer,
                proposed_beneficiary: new_beneficiary,
            });

            Ok(())
        }

        /// Accept a pending beneficiary transfer
        #[ink(message)]
        pub fn accept_beneficiary(&mut self) -> Result<(), Error> {
            self.ensure_migrated();

            let caller = self.env().caller();
            let pending = self
                .pending_beneficiary
                .ok_or(Error::NoPendingBeneficiary)?;

            if caller != pending {
                return Err(Error::Unauthorized);
            }

            let old_beneficiary = self.buyer;
            self.buyer = pending;
            self.pending_beneficiary = None;

            self.env().emit_event(BeneficiaryTransferred {
                old_beneficiary,
                new_beneficiary: pending,
            });

            Ok(())
        }

        /// Cancel a pending beneficiary transfer
        #[ink(message)]
        pub fn cancel_beneficiary_proposal(&mut self) -> Result<(), Error> {
            self.ensure_migrated();
            self.ensure_beneficiary()?;

            let cancelled_beneficiary = self
                .pending_beneficiary
                .ok_or(Error::NoPendingBeneficiary)?;
            self.pending_beneficiary = None;

            self.env().emit_event(BeneficiaryTransferCancelled {
                beneficiary: self.buyer,
                cancelled_beneficiary,
            });

            Ok(())
        }

        /// Install `code_hash`, which must be the registry's current escrow code.
        ///
        /// The beneficiary can upgrade at any time. Anyone else can upgrade once
        /// the code has been current for the registry's force delay, unless the
        /// beneficiary set the policy to `ConsentOnly`. The new code takes effect
        /// from the next call, and migrates storage then if it needs to.
        #[ink(message)]
        pub fn upgrade(&mut self, code_hash: Hash) -> Result<(), Error> {
            let caller = self.env().caller();
            let (current, force_target) = self.registry_code()?;
            check_upgrade(
                caller == self.buyer,
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
        /// escrow after the force delay (`Auto`), or only the beneficiary may
        /// upgrade it (`ConsentOnly`).
        #[ink(message)]
        pub fn set_upgrade_policy(&mut self, policy: UpgradePolicy) -> Result<(), Error> {
            self.ensure_migrated();
            self.ensure_beneficiary()?;

            self.upgrade_policy = policy;

            self.env().emit_event(UpgradePolicyChanged {
                owner: self.buyer,
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

        /// Get information about this lockup escrow
        #[ink(message)]
        pub fn get_info(&self) -> Result<LockupInfo, Error> {
            let current_stake = self.get_current_stake()?;

            Ok(LockupInfo {
                buyer: self.buyer,
                pending_beneficiary: self.pending_beneficiary,
                netuid: self.netuid,
                alpha_amount: self.alpha_amount,
                unlock_block: self.unlock_block,
                hotkey: self.hotkey,
                claimed: self.claimed,
                current_stake,
                tao_balance: self.env().balance(),
                upgrade_policy: self.upgrade_policy,
                version: self.version,
            })
        }

        /// Get the buyer address
        #[ink(message)]
        pub fn get_buyer(&self) -> AccountId {
            self.buyer
        }

        /// Get the current beneficiary address
        #[ink(message)]
        pub fn get_beneficiary(&self) -> AccountId {
            self.buyer
        }

        /// Get the pending beneficiary address
        #[ink(message)]
        pub fn get_pending_beneficiary(&self) -> Option<AccountId> {
            self.pending_beneficiary
        }

        /// Get the escrow's TAO balance
        #[ink(message)]
        pub fn get_tao_balance(&self) -> Balance {
            self.env().balance()
        }

        /// Get the hotkey holding escrow stake
        #[ink(message)]
        pub fn get_hotkey(&self) -> AccountId {
            self.hotkey
        }

        /// Get the code registry that approves this escrow's upgrades
        #[ink(message)]
        pub fn get_registry(&self) -> AccountId {
            self.registry
        }

        /// Get who may upgrade this escrow
        #[ink(message)]
        pub fn get_upgrade_policy(&self) -> UpgradePolicy {
            self.upgrade_policy
        }

        /// Get the storage layout version
        #[ink(message)]
        pub fn get_version(&self) -> u16 {
            self.version
        }

        /// Get the unlock block
        #[ink(message)]
        pub fn get_unlock_block(&self) -> BlockNumber {
            self.unlock_block
        }

        /// Whether a claim has paid out
        #[ink(message)]
        pub fn is_claimed(&self) -> bool {
            self.claimed
        }

        /// Get blocks remaining until unlock (0 if already unlocked)
        #[ink(message)]
        pub fn blocks_until_unlock(&self) -> BlockNumber {
            self.unlock_block.saturating_sub(self.env().block_number())
        }

        /// Get the contract's account ID (useful for the parent contract)
        #[ink(message)]
        pub fn account_id(&self) -> AccountId {
            self.env().account_id()
        }

        /// Helper function to get current stake amount
        fn get_current_stake(&self) -> Result<AlphaAmount, Error> {
            stake_of(self.hotkey, self.env().account_id(), self.netuid)
                .map_err(|_| Error::StakeQueryFailed)
        }

        fn transfer_error(error: TransferError) -> Error {
            match error {
                TransferError::Rejected(_) => Error::StakeTransferFailed,
                TransferError::QueryFailed => Error::StakeQueryFailed,
                // A transfer that completed but left stake behind must revert the
                // whole call, including the transfer itself.
                TransferError::NotVerified => panic!("post-transfer stake verification failed"),
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

        /// Storage only migrates upwards. Older code runs on newer storage without
        /// knowing its added fields, so a rollback after a storage change must ship
        /// as new code with a higher version, never as the old code.
        fn ensure_migrated(&mut self) {
            if self.version < STORAGE_VERSION {
                // Versions newer than 1 initialise their added storage here.
                self.version = STORAGE_VERSION;
            }
        }

        fn ensure_beneficiary(&self) -> Result<(), Error> {
            if self.env().caller() != self.buyer {
                return Err(Error::Unauthorized);
            }

            Ok(())
        }

        /// The registry's current escrow code, and that code once it may be
        /// forced onto escrows.
        #[cfg(not(test))]
        fn registry_code(&self) -> Result<(Option<Hash>, Option<Hash>), Error> {
            use code_registry::CodeRegistryRef;
            use ink::codegen::TraitCallBuilder;
            use ink::env::call::FromAccountId;
            use otc_shared::upgrade::ESCROW;

            let registry = CodeRegistryRef::from_account_id(self.registry);
            let current = registry
                .call()
                .current(ESCROW)
                .try_invoke()
                .map_err(|_| Error::RegistryQueryFailed)?
                .map_err(|_| Error::RegistryQueryFailed)?
                .map(|code| code.code_hash);
            let force_target = registry
                .call()
                .force_target(ESCROW)
                .try_invoke()
                .map_err(|_| Error::RegistryQueryFailed)?
                .map_err(|_| Error::RegistryQueryFailed)?;
            Ok((current, force_target))
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

        // ink!'s off-chain test environment cannot call other contracts, replace
        // code, or transfer a u64 balance, so unit tests answer registry queries
        // and record upgrades and TAO transfers here; localnet integration tests
        // cover the real calls.
        #[cfg(test)]
        fn registry_code(&self) -> Result<(Option<Hash>, Option<Hash>), Error> {
            Ok(tests::registry_code())
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
        use ink::env::test;
        use ink::env::Environment;
        use ink::scale::{Decode, Encode};
        use otc_shared::testing::{self, MockSubtensor};
        use otc_shared::{BittensorEnvironment, TRANSFER_TOLERANCE};
        use std::cell::RefCell;

        const NETUID: NetUid = 1;
        const UNLOCK_BLOCK: BlockNumber = 1000;
        const LOCKED_ALPHA: AlphaAmount = 1_000_000_000;
        // Above the off-chain engine's existential deposit of 1_000_000.
        const ESCROW_TAO_BALANCE: Balance = 5_000_000;

        type TestAccountId = <BittensorEnvironment as Environment>::AccountId;

        thread_local! {
            static REGISTRY_CODE: RefCell<(Option<Hash>, Option<Hash>)> = const { RefCell::new((None, None)) };
            static INSTALLED_CODE: RefCell<Option<Hash>> = const { RefCell::new(None) };
            static TAO_TRANSFERS: RefCell<Vec<(TestAccountId, Balance)>> = const { RefCell::new(Vec::new()) };
        }

        pub(super) fn record_tao_transfer(to: TestAccountId, amount: Balance) {
            TAO_TRANSFERS.with(|transfers| transfers.borrow_mut().push((to, amount)));
            // Mirror the transfer so the escrow's balance reads as swept.
            set_escrow_balance(0);
        }

        fn tao_transfers() -> Vec<(TestAccountId, Balance)> {
            TAO_TRANSFERS.with(|transfers| transfers.borrow().clone())
        }

        /// ink!'s off-chain engine stores balances as u128 and only exposes them for
        /// u128 environments, so set them through the default environment; the
        /// escrow reads them back as u64.
        fn set_escrow_balance(amount: Balance) {
            test::set_account_balance::<ink::env::DefaultEnvironment>(
                accounts().frank,
                u128::from(amount),
            );
        }

        pub(super) fn registry_code() -> (Option<Hash>, Option<Hash>) {
            REGISTRY_CODE.with(|code| *code.borrow())
        }

        pub(super) fn record_installed_code(code_hash: Hash) {
            INSTALLED_CODE.with(|installed| *installed.borrow_mut() = Some(code_hash));
        }

        fn set_registry_code(current: Option<Hash>, force_target: Option<Hash>) {
            REGISTRY_CODE.with(|code| *code.borrow_mut() = (current, force_target));
        }

        fn installed_code() -> Option<Hash> {
            INSTALLED_CODE.with(|installed| *installed.borrow())
        }

        fn hash(byte: u8) -> Hash {
            Hash::from([byte; 32])
        }

        /// Roles in every test: alice is the beneficiary, django a third party,
        /// eve the validator hotkey, and frank the escrow contract itself.
        fn accounts() -> test::DefaultAccounts<BittensorEnvironment> {
            test::default_accounts::<BittensorEnvironment>()
        }

        /// An escrow holding `LOCKED_ALPHA` on eve, with `ESCROW_TAO_BALANCE` TAO.
        fn escrow() -> AlphaLockup {
            let accounts = accounts();
            MockSubtensor::install(accounts.frank);
            set_registry_code(None, None);
            INSTALLED_CODE.with(|installed| *installed.borrow_mut() = None);
            TAO_TRANSFERS.with(|transfers| transfers.borrow_mut().clear());
            test::set_callee::<BittensorEnvironment>(accounts.frank);
            set_escrow_balance(ESCROW_TAO_BALANCE);
            testing::set_stake(accounts.eve, accounts.frank, NETUID, LOCKED_ALPHA);
            AlphaLockup::new(
                accounts.alice,
                NETUID,
                LOCKED_ALPHA,
                UNLOCK_BLOCK,
                accounts.eve,
                accounts.bob,
            )
        }

        fn set_caller(account: TestAccountId) {
            test::set_caller::<BittensorEnvironment>(account);
        }

        fn unlock() {
            test::set_block_number::<BittensorEnvironment>(UNLOCK_BLOCK);
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

        #[ink::test]
        fn constructor_works() {
            let accounts = accounts();
            let escrow = escrow();

            assert_eq!(escrow.get_buyer(), accounts.alice);
            assert_eq!(escrow.get_beneficiary(), accounts.alice);
            assert_eq!(escrow.get_pending_beneficiary(), None);
            assert_eq!(escrow.get_hotkey(), accounts.eve);
            assert_eq!(escrow.get_registry(), accounts.bob);
            assert_eq!(escrow.get_upgrade_policy(), UpgradePolicy::Auto);
            assert_eq!(escrow.get_version(), STORAGE_VERSION);
            assert_eq!(escrow.get_unlock_block(), UNLOCK_BLOCK);
            assert!(!escrow.is_claimed());
        }

        #[ink::test]
        fn blocks_until_unlock_works() {
            test::set_block_number::<BittensorEnvironment>(500);
            let escrow = escrow();
            assert_eq!(escrow.blocks_until_unlock(), 500);

            unlock();
            assert_eq!(escrow.blocks_until_unlock(), 0);

            test::set_block_number::<BittensorEnvironment>(1500);
            assert_eq!(escrow.blocks_until_unlock(), 0);
        }

        #[ink::test]
        fn beneficiary_can_propose_and_pending_beneficiary_can_accept() {
            let accounts = accounts();
            let mut escrow = escrow();

            set_caller(accounts.alice);
            assert_eq!(escrow.propose_beneficiary(accounts.charlie), Ok(()));
            assert_eq!(escrow.get_pending_beneficiary(), Some(accounts.charlie));

            set_caller(accounts.charlie);
            assert_eq!(escrow.accept_beneficiary(), Ok(()));
            assert_eq!(escrow.get_beneficiary(), accounts.charlie);
            assert_eq!(escrow.get_pending_beneficiary(), None);
        }

        #[ink::test]
        fn non_beneficiary_cannot_propose_beneficiary() {
            let accounts = accounts();
            let mut escrow = escrow();

            set_caller(accounts.django);
            assert_eq!(
                escrow.propose_beneficiary(accounts.charlie),
                Err(Error::Unauthorized)
            );
            assert_eq!(escrow.get_pending_beneficiary(), None);
        }

        #[ink::test]
        fn non_pending_account_cannot_accept_beneficiary() {
            let accounts = accounts();
            let mut escrow = escrow();

            set_caller(accounts.alice);
            assert_eq!(escrow.propose_beneficiary(accounts.charlie), Ok(()));

            set_caller(accounts.django);
            assert_eq!(escrow.accept_beneficiary(), Err(Error::Unauthorized));
            assert_eq!(escrow.get_beneficiary(), accounts.alice);
        }

        #[ink::test]
        fn beneficiary_can_replace_and_cancel_pending_proposal() {
            let accounts = accounts();
            let mut escrow = escrow();

            set_caller(accounts.alice);
            assert_eq!(escrow.propose_beneficiary(accounts.charlie), Ok(()));
            assert_eq!(escrow.propose_beneficiary(accounts.django), Ok(()));
            assert_eq!(escrow.get_pending_beneficiary(), Some(accounts.django));

            assert_eq!(escrow.cancel_beneficiary_proposal(), Ok(()));
            assert_eq!(escrow.get_pending_beneficiary(), None);
            assert_eq!(
                escrow.cancel_beneficiary_proposal(),
                Err(Error::NoPendingBeneficiary)
            );
        }

        #[ink::test]
        fn cannot_propose_current_beneficiary() {
            let accounts = accounts();
            let mut escrow = escrow();

            set_caller(accounts.alice);
            assert_eq!(
                escrow.propose_beneficiary(accounts.alice),
                Err(Error::InvalidBeneficiary)
            );
        }

        #[ink::test]
        fn claim_fails_if_lockup_not_expired() {
            let accounts = accounts();
            test::set_block_number::<BittensorEnvironment>(500);
            let mut escrow = escrow();

            set_caller(accounts.alice);
            assert_eq!(escrow.claim(), Err(Error::LockupNotExpired));
        }

        #[ink::test]
        fn third_party_can_claim_after_unlock_to_current_beneficiary() {
            let accounts = accounts();
            let mut escrow = escrow();

            unlock();
            set_caller(accounts.django);
            assert_eq!(escrow.claim(), Ok(()));
            assert!(escrow.is_claimed());

            assert_eq!(testing::stake(accounts.eve, accounts.frank, NETUID), 0);
            assert_eq!(
                testing::stake(accounts.eve, accounts.alice, NETUID),
                LOCKED_ALPHA
            );
            // Claiming leaves the escrow and its TAO in place.
            assert!(tao_transfers().is_empty());
            assert_eq!(escrow.claim(), Err(Error::NoAlphaToClaim));

            let events = test::recorded_events().collect::<Vec<_>>();
            let claimed = <AlphaClaimed as Decode>::decode(&mut &events[0].data[..])
                .expect("decode AlphaClaimed event");
            assert_eq!(claimed.buyer, accounts.alice);
            assert_eq!(claimed.initiated_by, accounts.django);
            assert_eq!(claimed.amount_claimed, LOCKED_ALPHA);
        }

        #[ink::test]
        fn claim_tolerates_transfer_rounding() {
            let accounts = accounts();
            let mut escrow = escrow();
            testing::set_transfer_loss(1);

            unlock();
            set_caller(accounts.django);
            assert_eq!(escrow.claim(), Ok(()));
            assert_eq!(
                testing::stake(accounts.eve, accounts.alice, NETUID),
                LOCKED_ALPHA - 1
            );
        }

        #[ink::test]
        fn claim_rejects_while_beneficiary_transfer_pending() {
            let accounts = accounts();
            let mut escrow = escrow();
            set_caller(accounts.alice);
            assert_eq!(escrow.propose_beneficiary(accounts.charlie), Ok(()));

            unlock();
            set_caller(accounts.django);
            assert_eq!(escrow.claim(), Err(Error::BeneficiaryTransferPending));
            assert!(!escrow.is_claimed());
            assert!(testing::movements().is_empty());
        }

        #[ink::test]
        fn claim_succeeds_after_beneficiary_proposal_cancelled() {
            let accounts = accounts();
            let mut escrow = escrow();
            set_caller(accounts.alice);
            assert_eq!(escrow.propose_beneficiary(accounts.charlie), Ok(()));
            assert_eq!(escrow.cancel_beneficiary_proposal(), Ok(()));

            unlock();
            set_caller(accounts.django);
            assert_eq!(escrow.claim(), Ok(()));
        }

        #[ink::test]
        fn claim_fails_with_no_alpha_and_does_not_mark_claimed() {
            let accounts = accounts();
            let mut escrow = escrow();
            testing::set_stake(accounts.eve, accounts.frank, NETUID, 0);

            unlock();
            set_caller(accounts.django);
            assert_eq!(escrow.claim(), Err(Error::NoAlphaToClaim));
            assert!(!escrow.is_claimed());
        }

        #[ink::test]
        fn claim_reports_a_rejected_transfer() {
            let accounts = accounts();
            let mut escrow = escrow();
            testing::set_reject_transfers(true);

            unlock();
            set_caller(accounts.django);
            assert_eq!(escrow.claim(), Err(Error::StakeTransferFailed));
            assert!(!escrow.is_claimed());
        }

        #[ink::test]
        fn claim_traps_when_the_transfer_moves_the_wrong_amount() {
            let accounts = accounts();
            let mut escrow = escrow();
            testing::set_transfer_loss(TRANSFER_TOLERANCE + 1);

            unlock();
            set_caller(accounts.django);
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| escrow.claim()));
            let payload = result.expect_err("claim should trap");
            assert_eq!(
                payload.downcast_ref::<&str>(),
                Some(&"post-transfer stake verification failed")
            );
            assert!(!escrow.is_claimed());
        }

        #[ink::test]
        fn claim_uses_accepted_beneficiary_and_old_beneficiary_loses_control() {
            let accounts = accounts();
            let mut escrow = escrow();

            set_caller(accounts.alice);
            assert_eq!(escrow.propose_beneficiary(accounts.charlie), Ok(()));
            set_caller(accounts.charlie);
            assert_eq!(escrow.accept_beneficiary(), Ok(()));
            set_caller(accounts.alice);
            assert_eq!(
                escrow.propose_beneficiary(accounts.django),
                Err(Error::Unauthorized)
            );

            unlock();
            set_caller(accounts.django);
            assert_eq!(escrow.claim(), Ok(()));
            assert_eq!(
                testing::stake(accounts.eve, accounts.charlie, NETUID),
                LOCKED_ALPHA
            );
        }

        #[ink::test]
        fn anyone_can_sweep_tao_to_the_beneficiary_without_terminating() {
            let accounts = accounts();
            let mut escrow = escrow();

            // Sweeping works before unlock: TAO is not the locked Alpha.
            set_caller(accounts.django);
            assert_eq!(escrow.sweep_tao(), Ok(()));
            assert_eq!(tao_transfers(), vec![(accounts.alice, ESCROW_TAO_BALANCE)]);
            assert_eq!(escrow.get_tao_balance(), 0);
            assert!(!escrow.is_claimed());

            // A later credit can be swept again.
            assert_eq!(escrow.sweep_tao(), Err(Error::NoTaoToSweep));
            set_escrow_balance(2_000_000);
            assert_eq!(escrow.sweep_tao(), Ok(()));
            assert_eq!(
                tao_transfers(),
                vec![
                    (accounts.alice, ESCROW_TAO_BALANCE),
                    (accounts.alice, 2_000_000)
                ]
            );

            let events = test::recorded_events().collect::<Vec<_>>();
            let swept = <TaoSwept as Decode>::decode(&mut &events[0].data[..])
                .expect("decode TaoSwept event");
            assert_eq!(swept.owner, accounts.alice);
            assert_eq!(swept.initiated_by, accounts.django);
            assert_eq!(swept.amount, ESCROW_TAO_BALANCE);
        }

        #[ink::test]
        fn sweep_tao_waits_for_a_pending_beneficiary_transfer() {
            let accounts = accounts();
            let mut escrow = escrow();
            set_caller(accounts.alice);
            assert_eq!(escrow.propose_beneficiary(accounts.charlie), Ok(()));

            set_caller(accounts.django);
            assert_eq!(escrow.sweep_tao(), Err(Error::BeneficiaryTransferPending));
            assert!(tao_transfers().is_empty());
        }

        #[ink::test]
        fn anyone_can_resync_after_a_hotkey_swap_and_claim_follows() {
            let accounts = accounts();
            let mut escrow = escrow();

            // A validator hotkey swap moves the escrow's stake from eve to charlie.
            testing::set_stake(accounts.eve, accounts.frank, NETUID, 0);
            testing::set_stake(accounts.charlie, accounts.frank, NETUID, LOCKED_ALPHA);

            unlock();
            set_caller(accounts.django);
            assert_eq!(escrow.claim(), Err(Error::NoAlphaToClaim));
            assert_eq!(escrow.resync_hotkey(accounts.charlie), Ok(()));
            assert_eq!(escrow.get_hotkey(), accounts.charlie);

            assert_eq!(escrow.claim(), Ok(()));
            assert_eq!(
                testing::stake(accounts.charlie, accounts.alice, NETUID),
                LOCKED_ALPHA
            );
        }

        #[ink::test]
        fn a_donation_on_the_old_hotkey_cannot_strand_swapped_stake() {
            let accounts = accounts();
            let mut escrow = escrow();
            let donation = 2_000_000;

            // After a hotkey swap moves the escrow's stake to charlie, someone
            // donates a little on the old hotkey and claims before anyone resyncs.
            testing::set_stake(accounts.eve, accounts.frank, NETUID, donation);
            testing::set_stake(accounts.charlie, accounts.frank, NETUID, LOCKED_ALPHA);

            unlock();
            set_caller(accounts.django);
            assert_eq!(escrow.claim(), Ok(()));
            assert_eq!(
                testing::stake(accounts.eve, accounts.alice, NETUID),
                donation
            );

            // The escrow is still there, so the swapped stake can follow.
            assert_eq!(escrow.resync_hotkey(accounts.charlie), Ok(()));
            assert_eq!(escrow.claim(), Ok(()));
            assert_eq!(
                testing::stake(accounts.charlie, accounts.alice, NETUID),
                LOCKED_ALPHA
            );
        }

        #[ink::test]
        fn beneficiary_closes_an_empty_escrow() {
            let accounts = accounts();
            let mut escrow = escrow();

            unlock();
            set_caller(accounts.alice);
            assert_eq!(escrow.close(), Err(Error::StakeRemaining));

            set_caller(accounts.django);
            assert_eq!(escrow.claim(), Ok(()));
            assert_eq!(escrow.close(), Err(Error::Unauthorized));

            set_caller(accounts.alice);
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| escrow.close()));
            assert_terminated_to(result, accounts.alice, ESCROW_TAO_BALANCE);
        }

        #[ink::test]
        fn close_waits_for_a_pending_beneficiary_transfer() {
            let accounts = accounts();
            let mut escrow = escrow();
            testing::set_stake(accounts.eve, accounts.frank, NETUID, 0);

            set_caller(accounts.alice);
            assert_eq!(escrow.propose_beneficiary(accounts.charlie), Ok(()));
            assert_eq!(escrow.close(), Err(Error::BeneficiaryTransferPending));
        }

        #[ink::test]
        fn resync_requires_more_stake_on_the_new_hotkey() {
            let accounts = accounts();
            let mut escrow = escrow();
            set_caller(accounts.django);

            assert_eq!(
                escrow.resync_hotkey(accounts.eve),
                Err(Error::InvalidHotkey)
            );

            // A smaller donation on another hotkey cannot redirect the escrow.
            testing::set_stake(accounts.charlie, accounts.frank, NETUID, LOCKED_ALPHA - 1);
            assert_eq!(
                escrow.resync_hotkey(accounts.charlie),
                Err(Error::InsufficientStakeOnHotkey)
            );
            assert_eq!(escrow.get_hotkey(), accounts.eve);

            // Equal stake is not enough either.
            testing::set_stake(accounts.charlie, accounts.frank, NETUID, LOCKED_ALPHA);
            assert_eq!(
                escrow.resync_hotkey(accounts.charlie),
                Err(Error::InsufficientStakeOnHotkey)
            );
        }

        #[ink::test]
        fn beneficiary_can_adopt_current_code_immediately() {
            let accounts = accounts();
            let mut escrow = escrow();
            set_registry_code(Some(hash(2)), None);

            set_caller(accounts.alice);
            assert_eq!(escrow.upgrade(hash(2)), Ok(()));
            assert_eq!(installed_code(), Some(hash(2)));
        }

        #[ink::test]
        fn third_party_upgrade_waits_for_the_force_delay() {
            let accounts = accounts();
            let mut escrow = escrow();
            set_caller(accounts.django);

            set_registry_code(Some(hash(2)), None);
            assert_eq!(escrow.upgrade(hash(2)), Err(Error::ForceDelayNotElapsed));
            assert_eq!(installed_code(), None);

            set_registry_code(Some(hash(2)), Some(hash(2)));
            assert_eq!(escrow.upgrade(hash(2)), Ok(()));
            assert_eq!(installed_code(), Some(hash(2)));
        }

        #[ink::test]
        fn consent_only_blocks_forced_upgrades() {
            let accounts = accounts();
            let mut escrow = escrow();
            set_registry_code(Some(hash(2)), Some(hash(2)));

            set_caller(accounts.django);
            assert_eq!(
                escrow.set_upgrade_policy(UpgradePolicy::ConsentOnly),
                Err(Error::Unauthorized)
            );

            set_caller(accounts.alice);
            assert_eq!(
                escrow.set_upgrade_policy(UpgradePolicy::ConsentOnly),
                Ok(())
            );
            assert_eq!(escrow.get_upgrade_policy(), UpgradePolicy::ConsentOnly);

            set_caller(accounts.django);
            assert_eq!(escrow.upgrade(hash(2)), Err(Error::ConsentRequired));

            // The beneficiary can still upgrade.
            set_caller(accounts.alice);
            assert_eq!(escrow.upgrade(hash(2)), Ok(()));
            assert_eq!(installed_code(), Some(hash(2)));
        }

        #[ink::test]
        fn upgrade_only_installs_the_registry_current_code() {
            let accounts = accounts();
            let mut escrow = escrow();
            set_caller(accounts.alice);

            assert_eq!(escrow.upgrade(hash(2)), Err(Error::NoCurrentCode));

            set_registry_code(Some(hash(2)), Some(hash(2)));
            assert_eq!(escrow.upgrade(hash(3)), Err(Error::NotCurrentCode));
            assert_eq!(installed_code(), None);
        }

        #[ink::test]
        fn migrate_brings_storage_to_the_current_version() {
            let mut escrow = escrow();
            escrow.version = 0;

            escrow.migrate();
            assert_eq!(escrow.get_version(), STORAGE_VERSION);
        }

        #[ink::test]
        fn state_changing_messages_migrate_first() {
            let accounts = accounts();
            let mut escrow = escrow();
            escrow.version = 0;

            set_caller(accounts.django);
            assert_eq!(escrow.sweep_tao(), Ok(()));
            assert_eq!(escrow.get_version(), STORAGE_VERSION);
        }
    }

    /// Upgraded code must decode the storage this code writes; see
    /// `otc_shared::storage_layout`.
    #[cfg(test)]
    mod storage_layout {
        const SNAPSHOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/storage-layout.json");

        #[cfg(not(feature = "upgrade-test"))]
        #[test]
        fn matches_its_snapshot() {
            otc_shared::storage_layout::check_snapshot::<super::AlphaLockup>(SNAPSHOT);
        }

        /// The test-only version 2 only bumps the version, and still decodes the released storage.
        #[cfg(feature = "upgrade-test")]
        #[test]
        fn decodes_the_released_snapshot() {
            otc_shared::storage_layout::assert_decodes_snapshot::<super::AlphaLockup>(SNAPSHOT);
        }
    }
}
