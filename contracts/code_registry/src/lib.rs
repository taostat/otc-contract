#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub use self::code_registry::{CodeRegistry, CodeRegistryRef};

/// Timelocked allowlist of the code every other contract may upgrade to.
///
/// This is the only contract in the system that is never upgraded. It makes no
/// chain-extension or runtime calls, so Subtensor changes cannot break it.
///
/// The owner proposes a code hash for a contract kind. After `upgrade_delay`
/// blocks anyone can activate it, making it that kind's current code, unless the
/// guardian vetoed it first. Position contracts may adopt the current code; once
/// it has been current for `force_delay` blocks, anyone may apply it to positions
/// that allow forced upgrades. Owner and guardian changes go through the same
/// delay and veto.
#[ink::contract(env = otc_shared::BittensorEnvironment)]
mod code_registry {
    use ink::prelude::vec::Vec;
    use ink::storage::Mapping;
    use otc_shared::CodeKind;

    #[derive(Debug, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    pub enum Error {
        /// Caller is not allowed to perform this action
        Unauthorized,
        /// This code hash is already proposed for this kind
        AlreadyProposed,
        /// This code hash is already the current code for this kind
        AlreadyCurrent,
        /// This code hash was revoked for this kind
        CodeRevoked,
        /// No pending proposal exists for this kind and code hash
        ProposalNotFound,
        /// The upgrade delay has not elapsed yet
        DelayNotElapsed,
        /// No owner or guardian change is pending
        NoPendingRoleChange,
        /// The owner and the guardian must be different accounts
        RolesMustDiffer,
        /// Block number arithmetic overflowed
        Overflow,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    #[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
    pub enum Role {
        Owner,
        Guardian,
    }

    /// A code hash waiting for its upgrade delay to elapse.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    #[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
    pub struct Proposal {
        pub proposed_at: BlockNumber,
    }

    /// The code a kind's contracts may currently adopt, and since when.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    #[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
    pub struct CurrentCode {
        pub code_hash: Hash,
        pub since: BlockNumber,
    }

    /// An owner or guardian change waiting for its delay to elapse.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    #[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
    pub struct RoleChange {
        pub role: Role,
        pub account: AccountId,
        pub proposed_at: BlockNumber,
    }

    #[ink::event]
    pub struct CodeProposed {
        #[ink(topic)]
        pub kind: CodeKind,
        #[ink(topic)]
        pub code_hash: Hash,
        pub activates_at: BlockNumber,
    }

    #[ink::event]
    pub struct CodeVetoed {
        #[ink(topic)]
        pub kind: CodeKind,
        #[ink(topic)]
        pub code_hash: Hash,
    }

    #[ink::event]
    pub struct CodeActivated {
        #[ink(topic)]
        pub kind: CodeKind,
        #[ink(topic)]
        pub code_hash: Hash,
        pub previous: Option<Hash>,
        #[ink(topic)]
        pub activated_by: AccountId,
    }

    #[ink::event]
    pub struct CodeRevoked {
        #[ink(topic)]
        pub kind: CodeKind,
        #[ink(topic)]
        pub code_hash: Hash,
        #[ink(topic)]
        pub revoked_by: AccountId,
        pub was_current: bool,
    }

    #[ink::event]
    pub struct RoleChangeProposed {
        pub role: Role,
        #[ink(topic)]
        pub account: AccountId,
        pub executable_at: BlockNumber,
    }

    #[ink::event]
    pub struct RoleChangeVetoed {
        pub role: Role,
        #[ink(topic)]
        pub account: AccountId,
    }

    #[ink::event]
    pub struct RoleChanged {
        pub role: Role,
        #[ink(topic)]
        pub old_account: AccountId,
        #[ink(topic)]
        pub new_account: AccountId,
    }

    #[ink(storage)]
    pub struct CodeRegistry {
        owner: AccountId,
        guardian: AccountId,
        /// Blocks a proposal or role change must wait before it can take effect
        upgrade_delay: BlockNumber,
        /// Blocks code must be current before anyone may force it onto positions
        force_delay: BlockNumber,
        proposals: Mapping<(CodeKind, Hash), Proposal>,
        current: Mapping<CodeKind, CurrentCode>,
        revoked: Mapping<(CodeKind, Hash), bool>,
        pending_role_change: Option<RoleChange>,
    }

    impl CodeRegistry {
        /// Create the registry with each kind's initial code already current.
        ///
        /// The delays cannot be changed later. Production deployments should use
        /// delays long enough for users to react; tests may use short ones.
        #[ink(constructor)]
        pub fn new(
            owner: AccountId,
            guardian: AccountId,
            upgrade_delay: BlockNumber,
            force_delay: BlockNumber,
            initial_codes: Vec<(CodeKind, Hash)>,
        ) -> Self {
            assert!(owner != guardian, "owner and guardian must differ");

            let now = Self::env().block_number();
            let mut current = Mapping::default();
            for (kind, code_hash) in initial_codes {
                current.insert(
                    kind,
                    &CurrentCode {
                        code_hash,
                        since: now,
                    },
                );
            }

            Self {
                owner,
                guardian,
                upgrade_delay,
                force_delay,
                proposals: Mapping::default(),
                current,
                revoked: Mapping::default(),
                pending_role_change: None,
            }
        }

        /// Propose `code_hash` as the next code for `kind`. Starts the upgrade delay.
        #[ink(message)]
        pub fn propose(&mut self, kind: CodeKind, code_hash: Hash) -> Result<(), Error> {
            self.ensure_owner()?;

            if self.is_revoked(kind, code_hash) {
                return Err(Error::CodeRevoked);
            }
            if self.is_current(kind, code_hash) {
                return Err(Error::AlreadyCurrent);
            }
            if self.proposals.contains((kind, code_hash)) {
                return Err(Error::AlreadyProposed);
            }

            let proposed_at = self.env().block_number();
            let activates_at = self.after_delay(proposed_at, self.upgrade_delay)?;
            self.proposals
                .insert((kind, code_hash), &Proposal { proposed_at });

            self.env().emit_event(CodeProposed {
                kind,
                code_hash,
                activates_at,
            });

            Ok(())
        }

        /// Cancel a pending proposal. Only the guardian can veto.
        #[ink(message)]
        pub fn veto(&mut self, kind: CodeKind, code_hash: Hash) -> Result<(), Error> {
            self.ensure_guardian()?;

            if !self.proposals.contains((kind, code_hash)) {
                return Err(Error::ProposalNotFound);
            }
            self.proposals.remove((kind, code_hash));

            self.env().emit_event(CodeVetoed { kind, code_hash });

            Ok(())
        }

        /// Make a matured, unvetoed proposal the current code for its kind.
        /// Anyone can call this once the upgrade delay has elapsed.
        #[ink(message)]
        pub fn activate(&mut self, kind: CodeKind, code_hash: Hash) -> Result<(), Error> {
            if self.is_revoked(kind, code_hash) {
                return Err(Error::CodeRevoked);
            }

            let proposal = self
                .proposals
                .get((kind, code_hash))
                .ok_or(Error::ProposalNotFound)?;
            let now = self.env().block_number();
            if now < self.after_delay(proposal.proposed_at, self.upgrade_delay)? {
                return Err(Error::DelayNotElapsed);
            }

            let previous = self.current.get(kind).map(|code| code.code_hash);
            self.current.insert(
                kind,
                &CurrentCode {
                    code_hash,
                    since: now,
                },
            );
            self.proposals.remove((kind, code_hash));

            self.env().emit_event(CodeActivated {
                kind,
                code_hash,
                previous,
                activated_by: self.env().caller(),
            });

            Ok(())
        }

        /// Block `code_hash` from ever being adopted for `kind`. Clears it if it is
        /// current, and drops any pending proposal for it. The owner or the
        /// guardian can revoke.
        #[ink(message)]
        pub fn revoke(&mut self, kind: CodeKind, code_hash: Hash) -> Result<(), Error> {
            let caller = self.env().caller();
            if caller != self.owner && caller != self.guardian {
                return Err(Error::Unauthorized);
            }

            let was_current = self.is_current(kind, code_hash);
            if was_current {
                self.current.remove(kind);
            }
            self.proposals.remove((kind, code_hash));
            self.revoked.insert((kind, code_hash), &true);

            self.env().emit_event(CodeRevoked {
                kind,
                code_hash,
                revoked_by: caller,
                was_current,
            });

            Ok(())
        }

        /// Propose a new owner or guardian. Starts the upgrade delay and replaces
        /// any change already pending.
        #[ink(message)]
        pub fn propose_role_change(&mut self, role: Role, account: AccountId) -> Result<(), Error> {
            self.ensure_owner()?;

            let other = match role {
                Role::Owner => self.guardian,
                Role::Guardian => self.owner,
            };
            if account == other {
                return Err(Error::RolesMustDiffer);
            }

            let proposed_at = self.env().block_number();
            let executable_at = self.after_delay(proposed_at, self.upgrade_delay)?;
            self.pending_role_change = Some(RoleChange {
                role,
                account,
                proposed_at,
            });

            self.env().emit_event(RoleChangeProposed {
                role,
                account,
                executable_at,
            });

            Ok(())
        }

        /// Cancel the pending owner or guardian change. Only the guardian can veto.
        #[ink(message)]
        pub fn veto_role_change(&mut self) -> Result<(), Error> {
            self.ensure_guardian()?;

            let change = self
                .pending_role_change
                .take()
                .ok_or(Error::NoPendingRoleChange)?;

            self.env().emit_event(RoleChangeVetoed {
                role: change.role,
                account: change.account,
            });

            Ok(())
        }

        /// Apply the pending owner or guardian change once its delay has elapsed.
        /// Anyone can call this.
        #[ink(message)]
        pub fn execute_role_change(&mut self) -> Result<(), Error> {
            let change = self.pending_role_change.ok_or(Error::NoPendingRoleChange)?;
            if self.env().block_number()
                < self.after_delay(change.proposed_at, self.upgrade_delay)?
            {
                return Err(Error::DelayNotElapsed);
            }

            let old_account = match change.role {
                Role::Owner => core::mem::replace(&mut self.owner, change.account),
                Role::Guardian => core::mem::replace(&mut self.guardian, change.account),
            };
            self.pending_role_change = None;

            self.env().emit_event(RoleChanged {
                role: change.role,
                old_account,
                new_account: change.account,
            });

            Ok(())
        }

        /// The current code for `kind`, if any.
        #[ink(message)]
        pub fn current(&self, kind: CodeKind) -> Option<CurrentCode> {
            self.current.get(kind)
        }

        /// Whether `code_hash` is the current code for `kind`.
        #[ink(message)]
        pub fn is_current(&self, kind: CodeKind, code_hash: Hash) -> bool {
            self.current
                .get(kind)
                .is_some_and(|code| code.code_hash == code_hash)
        }

        /// The current code for `kind` once it has been current for the force
        /// delay, so anyone may apply it to positions that allow forced upgrades.
        #[ink(message)]
        pub fn force_target(&self, kind: CodeKind) -> Option<Hash> {
            let code = self.current.get(kind)?;
            let forceable_at = code.since.saturating_add(self.force_delay);
            (self.env().block_number() >= forceable_at).then_some(code.code_hash)
        }

        /// The pending proposal for `kind` and `code_hash`, if any.
        #[ink(message)]
        pub fn proposal(&self, kind: CodeKind, code_hash: Hash) -> Option<Proposal> {
            self.proposals.get((kind, code_hash))
        }

        /// Whether `code_hash` was revoked for `kind`.
        #[ink(message)]
        pub fn is_revoked(&self, kind: CodeKind, code_hash: Hash) -> bool {
            self.revoked.contains((kind, code_hash))
        }

        /// The upgrade delay and the force delay, in blocks.
        #[ink(message)]
        pub fn delays(&self) -> (BlockNumber, BlockNumber) {
            (self.upgrade_delay, self.force_delay)
        }

        #[ink(message)]
        pub fn owner(&self) -> AccountId {
            self.owner
        }

        #[ink(message)]
        pub fn guardian(&self) -> AccountId {
            self.guardian
        }

        #[ink(message)]
        pub fn pending_role_change(&self) -> Option<RoleChange> {
            self.pending_role_change
        }

        fn after_delay(&self, from: BlockNumber, delay: BlockNumber) -> Result<BlockNumber, Error> {
            from.checked_add(delay).ok_or(Error::Overflow)
        }

        fn ensure_owner(&self) -> Result<(), Error> {
            if self.env().caller() != self.owner {
                return Err(Error::Unauthorized);
            }
            Ok(())
        }

        fn ensure_guardian(&self) -> Result<(), Error> {
            if self.env().caller() != self.guardian {
                return Err(Error::Unauthorized);
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use ink::env::test;
        use otc_shared::upgrade::{ESCROW, LISTING_VAULT};
        use otc_shared::BittensorEnvironment;

        const UPGRADE_DELAY: BlockNumber = 100;
        const FORCE_DELAY: BlockNumber = 300;

        fn accounts() -> test::DefaultAccounts<BittensorEnvironment> {
            test::default_accounts::<BittensorEnvironment>()
        }

        fn hash(byte: u8) -> Hash {
            Hash::from([byte; 32])
        }

        fn set_caller(account: AccountId) {
            test::set_caller::<BittensorEnvironment>(account);
        }

        fn set_block(block: BlockNumber) {
            test::set_block_number::<BittensorEnvironment>(block);
        }

        /// Owner is alice, guardian is bob, and the vault kind starts on code 1.
        fn registry() -> CodeRegistry {
            let accounts = accounts();
            set_block(10);
            set_caller(accounts.alice);
            CodeRegistry::new(
                accounts.alice,
                accounts.bob,
                UPGRADE_DELAY,
                FORCE_DELAY,
                ink::prelude::vec![(LISTING_VAULT, hash(1))],
            )
        }

        #[ink::test]
        fn constructor_makes_initial_codes_current() {
            let registry = registry();

            assert_eq!(
                registry.current(LISTING_VAULT),
                Some(CurrentCode {
                    code_hash: hash(1),
                    since: 10,
                })
            );
            assert!(registry.is_current(LISTING_VAULT, hash(1)));
            assert_eq!(registry.current(ESCROW), None);
            assert_eq!(registry.delays(), (UPGRADE_DELAY, FORCE_DELAY));
            assert_eq!(registry.owner(), accounts().alice);
            assert_eq!(registry.guardian(), accounts().bob);
        }

        #[ink::test]
        #[should_panic(expected = "owner and guardian must differ")]
        fn constructor_rejects_same_owner_and_guardian() {
            let accounts = accounts();
            CodeRegistry::new(
                accounts.alice,
                accounts.alice,
                UPGRADE_DELAY,
                FORCE_DELAY,
                Vec::new(),
            );
        }

        #[ink::test]
        fn only_owner_can_propose() {
            let mut registry = registry();

            set_caller(accounts().bob);
            assert_eq!(
                registry.propose(LISTING_VAULT, hash(2)),
                Err(Error::Unauthorized)
            );

            set_caller(accounts().alice);
            assert_eq!(registry.propose(LISTING_VAULT, hash(2)), Ok(()));
            assert_eq!(
                registry.proposal(LISTING_VAULT, hash(2)),
                Some(Proposal { proposed_at: 10 })
            );
        }

        #[ink::test]
        fn propose_rejects_current_duplicate_and_revoked_code() {
            let mut registry = registry();
            set_caller(accounts().alice);

            assert_eq!(
                registry.propose(LISTING_VAULT, hash(1)),
                Err(Error::AlreadyCurrent)
            );

            assert_eq!(registry.propose(LISTING_VAULT, hash(2)), Ok(()));
            assert_eq!(
                registry.propose(LISTING_VAULT, hash(2)),
                Err(Error::AlreadyProposed)
            );

            assert_eq!(registry.revoke(LISTING_VAULT, hash(3)), Ok(()));
            assert_eq!(
                registry.propose(LISTING_VAULT, hash(3)),
                Err(Error::CodeRevoked)
            );
        }

        #[ink::test]
        fn activate_waits_for_the_upgrade_delay() {
            let mut registry = registry();
            set_caller(accounts().alice);
            assert_eq!(registry.propose(LISTING_VAULT, hash(2)), Ok(()));

            // Anyone may activate, but not before the delay.
            set_caller(accounts().django);
            set_block(10 + UPGRADE_DELAY - 1);
            assert_eq!(
                registry.activate(LISTING_VAULT, hash(2)),
                Err(Error::DelayNotElapsed)
            );

            set_block(10 + UPGRADE_DELAY);
            assert_eq!(registry.activate(LISTING_VAULT, hash(2)), Ok(()));
            assert_eq!(
                registry.current(LISTING_VAULT),
                Some(CurrentCode {
                    code_hash: hash(2),
                    since: 10 + UPGRADE_DELAY,
                })
            );
            assert_eq!(registry.proposal(LISTING_VAULT, hash(2)), None);
            assert!(!registry.is_current(LISTING_VAULT, hash(1)));
        }

        #[ink::test]
        fn activate_requires_a_proposal() {
            let mut registry = registry();
            set_block(10_000);
            assert_eq!(
                registry.activate(LISTING_VAULT, hash(2)),
                Err(Error::ProposalNotFound)
            );
        }

        #[ink::test]
        fn guardian_veto_stops_a_proposal() {
            let mut registry = registry();
            set_caller(accounts().alice);
            assert_eq!(registry.propose(LISTING_VAULT, hash(2)), Ok(()));

            // The owner cannot veto its own proposal; only the guardian can.
            assert_eq!(
                registry.veto(LISTING_VAULT, hash(2)),
                Err(Error::Unauthorized)
            );

            set_caller(accounts().bob);
            assert_eq!(registry.veto(LISTING_VAULT, hash(2)), Ok(()));
            assert_eq!(registry.proposal(LISTING_VAULT, hash(2)), None);

            set_block(10 + UPGRADE_DELAY);
            assert_eq!(
                registry.activate(LISTING_VAULT, hash(2)),
                Err(Error::ProposalNotFound)
            );
            assert!(registry.is_current(LISTING_VAULT, hash(1)));

            // A vetoed hash can be proposed again, restarting the delay.
            set_caller(accounts().alice);
            assert_eq!(registry.propose(LISTING_VAULT, hash(2)), Ok(()));
            assert_eq!(
                registry.proposal(LISTING_VAULT, hash(2)),
                Some(Proposal {
                    proposed_at: 10 + UPGRADE_DELAY
                })
            );
        }

        #[ink::test]
        fn veto_requires_a_pending_proposal() {
            let mut registry = registry();
            set_caller(accounts().bob);
            assert_eq!(
                registry.veto(LISTING_VAULT, hash(2)),
                Err(Error::ProposalNotFound)
            );
        }

        #[ink::test]
        fn revoking_current_code_clears_it() {
            let mut registry = registry();

            set_caller(accounts().django);
            assert_eq!(
                registry.revoke(LISTING_VAULT, hash(1)),
                Err(Error::Unauthorized)
            );

            set_caller(accounts().bob);
            assert_eq!(registry.revoke(LISTING_VAULT, hash(1)), Ok(()));
            assert_eq!(registry.current(LISTING_VAULT), None);
            assert!(registry.is_revoked(LISTING_VAULT, hash(1)));
            assert_eq!(registry.force_target(LISTING_VAULT), None);
        }

        #[ink::test]
        fn revoking_a_pending_proposal_blocks_activation() {
            let mut registry = registry();
            set_caller(accounts().alice);
            assert_eq!(registry.propose(LISTING_VAULT, hash(2)), Ok(()));
            assert_eq!(registry.revoke(LISTING_VAULT, hash(2)), Ok(()));

            set_block(10 + UPGRADE_DELAY);
            assert_eq!(
                registry.activate(LISTING_VAULT, hash(2)),
                Err(Error::CodeRevoked)
            );
            assert!(registry.is_current(LISTING_VAULT, hash(1)));
        }

        #[ink::test]
        fn revocation_is_per_kind() {
            let mut registry = registry();
            set_caller(accounts().alice);
            assert_eq!(registry.revoke(ESCROW, hash(2)), Ok(()));
            assert_eq!(registry.propose(LISTING_VAULT, hash(2)), Ok(()));
        }

        #[ink::test]
        fn force_target_waits_for_the_force_delay() {
            let mut registry = registry();

            set_block(10 + FORCE_DELAY - 1);
            assert_eq!(registry.force_target(LISTING_VAULT), None);

            set_block(10 + FORCE_DELAY);
            assert_eq!(registry.force_target(LISTING_VAULT), Some(hash(1)));

            // Activating new code restarts the force delay.
            set_caller(accounts().alice);
            assert_eq!(registry.propose(LISTING_VAULT, hash(2)), Ok(()));
            let activated_at = 10 + FORCE_DELAY + UPGRADE_DELAY;
            set_block(activated_at);
            assert_eq!(registry.activate(LISTING_VAULT, hash(2)), Ok(()));
            assert_eq!(registry.force_target(LISTING_VAULT), None);

            set_block(activated_at + FORCE_DELAY);
            assert_eq!(registry.force_target(LISTING_VAULT), Some(hash(2)));
            assert_eq!(registry.force_target(ESCROW), None);
        }

        #[ink::test]
        fn role_change_waits_for_the_delay() {
            let mut registry = registry();
            let accounts = accounts();

            set_caller(accounts.bob);
            assert_eq!(
                registry.propose_role_change(Role::Owner, accounts.charlie),
                Err(Error::Unauthorized)
            );

            set_caller(accounts.alice);
            assert_eq!(
                registry.propose_role_change(Role::Owner, accounts.charlie),
                Ok(())
            );
            assert_eq!(
                registry.pending_role_change(),
                Some(RoleChange {
                    role: Role::Owner,
                    account: accounts.charlie,
                    proposed_at: 10,
                })
            );

            set_caller(accounts.django);
            set_block(10 + UPGRADE_DELAY - 1);
            assert_eq!(registry.execute_role_change(), Err(Error::DelayNotElapsed));

            set_block(10 + UPGRADE_DELAY);
            assert_eq!(registry.execute_role_change(), Ok(()));
            assert_eq!(registry.owner(), accounts.charlie);
            assert_eq!(registry.pending_role_change(), None);

            // The old owner has lost its powers.
            set_caller(accounts.alice);
            assert_eq!(
                registry.propose(LISTING_VAULT, hash(2)),
                Err(Error::Unauthorized)
            );
        }

        #[ink::test]
        fn guardian_can_veto_a_role_change() {
            let mut registry = registry();
            let accounts = accounts();

            set_caller(accounts.alice);
            assert_eq!(
                registry.propose_role_change(Role::Guardian, accounts.charlie),
                Ok(())
            );

            set_caller(accounts.bob);
            assert_eq!(registry.veto_role_change(), Ok(()));
            assert_eq!(registry.pending_role_change(), None);
            assert_eq!(registry.veto_role_change(), Err(Error::NoPendingRoleChange));

            set_block(10 + UPGRADE_DELAY);
            assert_eq!(
                registry.execute_role_change(),
                Err(Error::NoPendingRoleChange)
            );
            assert_eq!(registry.guardian(), accounts.bob);
        }

        #[ink::test]
        fn role_change_keeps_owner_and_guardian_separate() {
            let mut registry = registry();
            let accounts = accounts();
            set_caller(accounts.alice);

            assert_eq!(
                registry.propose_role_change(Role::Owner, accounts.bob),
                Err(Error::RolesMustDiffer)
            );
            assert_eq!(
                registry.propose_role_change(Role::Guardian, accounts.alice),
                Err(Error::RolesMustDiffer)
            );
        }

        #[ink::test]
        fn new_role_change_replaces_the_pending_one() {
            let mut registry = registry();
            let accounts = accounts();
            set_caller(accounts.alice);

            assert_eq!(
                registry.propose_role_change(Role::Guardian, accounts.charlie),
                Ok(())
            );
            set_block(50);
            assert_eq!(
                registry.propose_role_change(Role::Guardian, accounts.django),
                Ok(())
            );

            // The delay restarted with the replacement.
            set_block(10 + UPGRADE_DELAY);
            assert_eq!(registry.execute_role_change(), Err(Error::DelayNotElapsed));
            set_block(50 + UPGRADE_DELAY);
            assert_eq!(registry.execute_role_change(), Ok(()));
            assert_eq!(registry.guardian(), accounts.django);
        }

        #[ink::test]
        fn delay_overflow_is_reported() {
            let accounts = accounts();
            set_caller(accounts.alice);
            set_block(10);
            let mut registry = CodeRegistry::new(
                accounts.alice,
                accounts.bob,
                BlockNumber::MAX,
                FORCE_DELAY,
                Vec::new(),
            );
            assert_eq!(
                registry.propose(LISTING_VAULT, hash(2)),
                Err(Error::Overflow)
            );
        }

        #[ink::test]
        fn events_record_proposals_and_activations() {
            let mut registry = registry();
            set_caller(accounts().alice);
            assert_eq!(registry.propose(LISTING_VAULT, hash(2)), Ok(()));
            set_block(10 + UPGRADE_DELAY);
            assert_eq!(registry.activate(LISTING_VAULT, hash(2)), Ok(()));

            let events = test::recorded_events().collect::<Vec<_>>();
            assert_eq!(events.len(), 2);
        }
    }
}
