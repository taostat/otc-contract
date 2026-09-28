#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub use self::alpha_lockup::{AlphaLockup, AlphaLockupRef};

#[ink::contract(env = otc_shared::BittensorEnvironment)]
mod alpha_lockup {
    use otc_shared::{AlphaAmount, AlphaCurrency, NetUid, TRANSFER_TOLERANCE};

    /// Error types for the Alpha Lockup escrow contract
    #[derive(Debug, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    pub enum Error {
        /// Caller is not allowed to perform this action
        Unauthorized,
        /// Lockup period has not expired yet
        LockupNotExpired,
        /// Alpha has already been claimed
        AlreadyClaimed,
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
    }

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
        /// Whether the Alpha has been claimed
        claimed: bool,
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
        ) -> Self {
            Self {
                buyer,
                pending_beneficiary: None,
                netuid,
                alpha_amount,
                unlock_block,
                hotkey,
                claimed: false,
            }
        }

        /// Claim the locked Alpha after the lockup period has expired
        /// Anyone can trigger claim after unlock.
        /// Transfers all stake (including dividends) to the current beneficiary
        /// Terminates the contract after successful claim
        #[ink(message)]
        pub fn claim(&mut self) -> Result<(), Error> {
            // Time check: must be past unlock block
            if self.env().block_number() < self.unlock_block {
                return Err(Error::LockupNotExpired);
            }

            // Prevent double-claiming
            if self.claimed {
                return Err(Error::AlreadyClaimed);
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

            // Transfer all stake to beneficiary
            self.env()
                .extension()
                .transfer_stake(
                    beneficiary,
                    self.hotkey,
                    self.netuid,
                    self.netuid,
                    AlphaCurrency::from(current_stake),
                )
                .map_err(|_| Error::StakeTransferFailed)?;

            let remaining_stake = self.get_current_stake()?;
            if remaining_stake > TRANSFER_TOLERANCE {
                Self::trap_stake_transfer_not_verified();
            }

            self.claimed = true;

            // Emit event before termination
            self.env().emit_event(AlphaClaimed {
                buyer: beneficiary,
                initiated_by: self.env().caller(),
                netuid: self.netuid,
                amount_claimed: current_stake,
                unlock_block: self.unlock_block,
            });

            // Terminate the contract, sending any remaining TAO balance to beneficiary
            self.env().terminate_contract(beneficiary)
        }

        /// Propose a new beneficiary for coldkey swaps
        #[ink(message)]
        pub fn propose_beneficiary(&mut self, new_beneficiary: AccountId) -> Result<(), Error> {
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

        /// Get the unlock block
        #[ink(message)]
        pub fn get_unlock_block(&self) -> BlockNumber {
            self.unlock_block
        }

        /// Check if the lockup has been claimed
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
            let contract_account = self.env().account_id();

            match self
                .env()
                .extension()
                .get_stake_info(self.hotkey, contract_account, self.netuid)
            {
                Ok(Some(info)) => Ok(info.stake_amount()),
                Ok(None) => Ok(0),
                Err(_) => Err(Error::StakeQueryFailed),
            }
        }

        /// A transfer that completed but left stake behind must revert the whole
        /// call, including the transfer itself.
        fn trap_stake_transfer_not_verified() -> ! {
            panic!("post-transfer stake verification failed")
        }

        fn ensure_beneficiary(&self) -> Result<(), Error> {
            if self.env().caller() != self.buyer {
                return Err(Error::Unauthorized);
            }

            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use core::panic::AssertUnwindSafe;
        use ink::env::test;
        use ink::env::Environment;
        use ink::scale::{Compact, Decode, Encode};
        use otc_shared::runtime::NetUid as RuntimeNetUid;
        use otc_shared::BittensorEnvironment;
        use otc_shared::{StakeInfo, TaoCurrency};
        use std::cell::RefCell;

        const SUBTENSOR_EXTENSION_ID: u16 = 0;
        const GET_STAKE_INFO_FN_ID: u16 = 0;
        const TRANSFER_STAKE_FN_ID: u16 = 6;
        const UNLOCK_BLOCK: BlockNumber = 1000;
        const LOCKED_ALPHA: AlphaAmount = 1_000_000_000;
        // Default off-chain balance of `bob`, who plays the escrow contract below.
        const ESCROW_TAO_BALANCE: Balance = 1_000;

        type TestAccountId = <BittensorEnvironment as Environment>::AccountId;

        #[derive(Clone, Copy)]
        enum StakeResponse {
            None,
            Stake(AlphaAmount),
        }

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        struct TransferRecord {
            destination_coldkey: TestAccountId,
            hotkey: TestAccountId,
            origin_netuid: NetUid,
            destination_netuid: NetUid,
            amount: AlphaAmount,
        }

        #[derive(Clone)]
        struct MockState {
            stake_response: StakeResponse,
            stake_response_queue: Vec<StakeResponse>,
            last_transfer: Option<TransferRecord>,
        }

        impl Default for MockState {
            fn default() -> Self {
                Self {
                    stake_response: StakeResponse::None,
                    stake_response_queue: Vec::new(),
                    last_transfer: None,
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
                    GET_STAKE_INFO_FN_ID => {
                        let payload = decode_input(input);
                        let mut input = &payload[..];
                        let hotkey = TestAccountId::decode(&mut input).expect("mock decode hotkey");
                        let coldkey =
                            TestAccountId::decode(&mut input).expect("mock decode coldkey");
                        let netuid = NetUid::decode(&mut input).expect("mock decode netuid");

                        MOCK_STATE.with(|state| {
                            let mut state = state.borrow_mut();
                            let response = if !state.stake_response_queue.is_empty() {
                                state.stake_response_queue.remove(0)
                            } else {
                                state.stake_response
                            };

                            match response {
                                StakeResponse::None => {
                                    output.extend(Encode::encode(&Option::<StakeInfo>::None));
                                }
                                StakeResponse::Stake(amount) => {
                                    encode_some_stake(output, hotkey, coldkey, netuid, amount);
                                }
                            }
                        });

                        0
                    }
                    TRANSFER_STAKE_FN_ID => {
                        let payload = decode_input(input);
                        let mut input = &payload[..];
                        let destination_coldkey = TestAccountId::decode(&mut input)
                            .expect("mock decode destination coldkey");
                        let hotkey = TestAccountId::decode(&mut input).expect("mock decode hotkey");
                        let origin_netuid =
                            NetUid::decode(&mut input).expect("mock decode origin netuid");
                        let destination_netuid =
                            NetUid::decode(&mut input).expect("mock decode destination netuid");
                        let alpha_amount =
                            AlphaCurrency::decode(&mut input).expect("mock decode alpha amount");

                        MOCK_STATE.with(|state| {
                            let mut state = state.borrow_mut();
                            state.last_transfer = Some(TransferRecord {
                                destination_coldkey,
                                hotkey,
                                origin_netuid,
                                destination_netuid,
                                amount: alpha_amount.as_u64(),
                            });
                            state.stake_response = StakeResponse::None;
                        });

                        0
                    }
                    _ => 1,
                }
            }
        }

        /// The off-chain engine hands chain extensions their arguments as a
        /// length-prefixed byte vector.
        fn decode_input(input: &[u8]) -> Vec<u8> {
            Vec::<u8>::decode(&mut &input[..]).expect("mock decode input bytes")
        }

        fn encode_some_stake(
            output: &mut Vec<u8>,
            hotkey: TestAccountId,
            coldkey: TestAccountId,
            netuid: NetUid,
            stake_amount: AlphaAmount,
        ) {
            output.push(1);
            output.extend(Encode::encode(&hotkey));
            output.extend(Encode::encode(&coldkey));
            output.extend(Encode::encode(&Compact(RuntimeNetUid::from(netuid))));
            output.extend(Encode::encode(&Compact(AlphaCurrency::from(stake_amount))));
            output.extend(Encode::encode(&Compact(0u64)));
            output.extend(Encode::encode(&Compact(AlphaCurrency::from(0))));
            output.extend(Encode::encode(&Compact(TaoCurrency::from(0))));
            output.extend(Encode::encode(&Compact(0u64)));
            output.extend(Encode::encode(&true));
        }

        fn default_accounts() -> test::DefaultAccounts<BittensorEnvironment> {
            test::default_accounts::<BittensorEnvironment>()
        }

        fn register_mock_extension(stake_response: StakeResponse) {
            MOCK_STATE.with(|state| {
                *state.borrow_mut() = MockState {
                    stake_response,
                    ..MockState::default()
                };
            });
            test::register_chain_extension(MockSubtensorExtension);
        }

        fn set_stake_response_queue(stake_responses: Vec<StakeResponse>) {
            MOCK_STATE.with(|state| {
                state.borrow_mut().stake_response_queue = stake_responses;
            });
        }

        fn last_transfer() -> Option<TransferRecord> {
            MOCK_STATE.with(|state| state.borrow().last_transfer)
        }

        fn new_contract(beneficiary: TestAccountId, hotkey: TestAccountId) -> AlphaLockup {
            AlphaLockup::new(beneficiary, 1u16, LOCKED_ALPHA, UNLOCK_BLOCK, hotkey)
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
            let accounts = default_accounts();

            let contract = new_contract(accounts.alice, accounts.bob);

            assert_eq!(contract.get_buyer(), accounts.alice);
            assert_eq!(contract.get_beneficiary(), accounts.alice);
            assert_eq!(contract.get_pending_beneficiary(), None);
            assert_eq!(contract.get_hotkey(), accounts.bob);
            assert_eq!(contract.get_unlock_block(), UNLOCK_BLOCK);
            assert!(!contract.is_claimed());
        }

        #[ink::test]
        fn blocks_until_unlock_works() {
            let accounts = default_accounts();

            test::set_block_number::<BittensorEnvironment>(500);
            let contract = new_contract(accounts.alice, accounts.bob);
            assert_eq!(contract.blocks_until_unlock(), 500);

            test::set_block_number::<BittensorEnvironment>(UNLOCK_BLOCK);
            assert_eq!(contract.blocks_until_unlock(), 0);

            test::set_block_number::<BittensorEnvironment>(1500);
            assert_eq!(contract.blocks_until_unlock(), 0);
        }

        #[ink::test]
        fn beneficiary_can_propose_and_pending_beneficiary_can_accept() {
            let accounts = default_accounts();

            let mut contract = new_contract(accounts.alice, accounts.bob);

            test::set_caller::<BittensorEnvironment>(accounts.alice);
            assert_eq!(contract.propose_beneficiary(accounts.charlie), Ok(()));
            assert_eq!(contract.get_pending_beneficiary(), Some(accounts.charlie));

            test::set_caller::<BittensorEnvironment>(accounts.charlie);
            assert_eq!(contract.accept_beneficiary(), Ok(()));
            assert_eq!(contract.get_beneficiary(), accounts.charlie);
            assert_eq!(contract.get_buyer(), accounts.charlie);
            assert_eq!(contract.get_pending_beneficiary(), None);
        }

        #[ink::test]
        fn non_beneficiary_cannot_propose_beneficiary() {
            let accounts = default_accounts();

            let mut contract = new_contract(accounts.alice, accounts.bob);

            test::set_caller::<BittensorEnvironment>(accounts.bob);
            assert_eq!(
                contract.propose_beneficiary(accounts.charlie),
                Err(Error::Unauthorized)
            );
            assert_eq!(contract.get_beneficiary(), accounts.alice);
            assert_eq!(contract.get_pending_beneficiary(), None);
        }

        #[ink::test]
        fn non_pending_account_cannot_accept_beneficiary() {
            let accounts = default_accounts();

            let mut contract = new_contract(accounts.alice, accounts.bob);

            test::set_caller::<BittensorEnvironment>(accounts.alice);
            assert_eq!(contract.propose_beneficiary(accounts.charlie), Ok(()));

            test::set_caller::<BittensorEnvironment>(accounts.django);
            assert_eq!(contract.accept_beneficiary(), Err(Error::Unauthorized));
            assert_eq!(contract.get_beneficiary(), accounts.alice);
            assert_eq!(contract.get_pending_beneficiary(), Some(accounts.charlie));
        }

        #[ink::test]
        fn beneficiary_can_replace_and_cancel_pending_proposal() {
            let accounts = default_accounts();

            let mut contract = new_contract(accounts.alice, accounts.bob);

            test::set_caller::<BittensorEnvironment>(accounts.alice);
            assert_eq!(contract.propose_beneficiary(accounts.charlie), Ok(()));
            assert_eq!(contract.propose_beneficiary(accounts.django), Ok(()));
            assert_eq!(contract.get_pending_beneficiary(), Some(accounts.django));

            assert_eq!(contract.cancel_beneficiary_proposal(), Ok(()));
            assert_eq!(contract.get_pending_beneficiary(), None);
            assert_eq!(
                contract.cancel_beneficiary_proposal(),
                Err(Error::NoPendingBeneficiary)
            );
        }

        #[ink::test]
        fn cannot_propose_current_beneficiary() {
            let accounts = default_accounts();

            let mut contract = new_contract(accounts.alice, accounts.bob);

            test::set_caller::<BittensorEnvironment>(accounts.alice);
            assert_eq!(
                contract.propose_beneficiary(accounts.alice),
                Err(Error::InvalidBeneficiary)
            );
        }

        #[ink::test]
        fn claim_fails_if_lockup_not_expired() {
            let accounts = default_accounts();

            test::set_block_number::<BittensorEnvironment>(500);
            let mut contract = new_contract(accounts.alice, accounts.bob);

            test::set_caller::<BittensorEnvironment>(accounts.alice);
            assert_eq!(contract.claim(), Err(Error::LockupNotExpired));
        }

        #[ink::test]
        fn third_party_can_claim_after_unlock_to_current_beneficiary() {
            let accounts = default_accounts();
            register_mock_extension(StakeResponse::Stake(LOCKED_ALPHA));

            let mut contract = new_contract(accounts.alice, accounts.bob);

            test::set_callee::<BittensorEnvironment>(accounts.bob);
            test::set_block_number::<BittensorEnvironment>(UNLOCK_BLOCK);
            test::set_caller::<BittensorEnvironment>(accounts.django);

            let result = std::panic::catch_unwind(AssertUnwindSafe(|| contract.claim()));
            assert_terminated_to(result, accounts.alice, ESCROW_TAO_BALANCE);

            assert_eq!(
                last_transfer(),
                Some(TransferRecord {
                    destination_coldkey: accounts.alice,
                    hotkey: accounts.bob,
                    origin_netuid: 1,
                    destination_netuid: 1,
                    amount: LOCKED_ALPHA,
                })
            );

            let events = test::recorded_events().collect::<Vec<_>>();
            let claimed = <AlphaClaimed as Decode>::decode(&mut &events[0].data[..])
                .expect("decode AlphaClaimed event");
            assert_eq!(claimed.buyer, accounts.alice);
            assert_eq!(claimed.initiated_by, accounts.django);
            assert_eq!(claimed.amount_claimed, LOCKED_ALPHA);
        }

        #[ink::test]
        fn claim_rejects_while_beneficiary_transfer_pending() {
            let accounts = default_accounts();
            register_mock_extension(StakeResponse::Stake(LOCKED_ALPHA));

            let mut contract = new_contract(accounts.alice, accounts.bob);
            test::set_caller::<BittensorEnvironment>(accounts.alice);
            assert_eq!(contract.propose_beneficiary(accounts.charlie), Ok(()));

            test::set_callee::<BittensorEnvironment>(accounts.bob);
            test::set_block_number::<BittensorEnvironment>(UNLOCK_BLOCK);
            test::set_caller::<BittensorEnvironment>(accounts.django);

            assert_eq!(contract.claim(), Err(Error::BeneficiaryTransferPending));
            assert!(!contract.is_claimed());
            assert_eq!(last_transfer(), None);
        }

        #[ink::test]
        fn claim_succeeds_after_beneficiary_proposal_cancelled() {
            let accounts = default_accounts();
            register_mock_extension(StakeResponse::Stake(LOCKED_ALPHA));

            let mut contract = new_contract(accounts.alice, accounts.bob);
            test::set_caller::<BittensorEnvironment>(accounts.alice);
            assert_eq!(contract.propose_beneficiary(accounts.charlie), Ok(()));
            assert_eq!(contract.cancel_beneficiary_proposal(), Ok(()));

            test::set_callee::<BittensorEnvironment>(accounts.bob);
            test::set_block_number::<BittensorEnvironment>(UNLOCK_BLOCK);
            test::set_caller::<BittensorEnvironment>(accounts.django);

            let result = std::panic::catch_unwind(AssertUnwindSafe(|| contract.claim()));
            assert_terminated_to(result, accounts.alice, ESCROW_TAO_BALANCE);
            assert!(last_transfer().is_some());
        }

        #[ink::test]
        fn claim_fails_with_no_alpha_and_does_not_mark_claimed() {
            let accounts = default_accounts();
            register_mock_extension(StakeResponse::None);

            let mut contract = new_contract(accounts.alice, accounts.bob);

            test::set_callee::<BittensorEnvironment>(accounts.bob);
            test::set_block_number::<BittensorEnvironment>(UNLOCK_BLOCK);
            test::set_caller::<BittensorEnvironment>(accounts.django);

            assert_eq!(contract.claim(), Err(Error::NoAlphaToClaim));
            assert!(!contract.is_claimed());
            assert_eq!(last_transfer(), None);
        }

        #[ink::test]
        fn claim_traps_when_post_transfer_stake_verification_fails() {
            let accounts = default_accounts();
            register_mock_extension(StakeResponse::Stake(LOCKED_ALPHA));
            // The escrow still reports its full stake after the transfer.
            set_stake_response_queue(vec![
                StakeResponse::Stake(LOCKED_ALPHA),
                StakeResponse::Stake(LOCKED_ALPHA),
            ]);

            let mut contract = new_contract(accounts.alice, accounts.bob);

            test::set_callee::<BittensorEnvironment>(accounts.bob);
            test::set_block_number::<BittensorEnvironment>(UNLOCK_BLOCK);
            test::set_caller::<BittensorEnvironment>(accounts.django);

            let result = std::panic::catch_unwind(AssertUnwindSafe(|| contract.claim()));

            assert!(result.is_err());
            assert!(!contract.is_claimed());
            assert!(last_transfer().is_some());
        }

        #[ink::test]
        fn claim_uses_accepted_beneficiary_and_old_beneficiary_loses_control() {
            let accounts = default_accounts();
            register_mock_extension(StakeResponse::Stake(LOCKED_ALPHA));

            let mut contract = new_contract(accounts.alice, accounts.bob);

            test::set_caller::<BittensorEnvironment>(accounts.alice);
            assert_eq!(contract.propose_beneficiary(accounts.charlie), Ok(()));

            test::set_caller::<BittensorEnvironment>(accounts.charlie);
            assert_eq!(contract.accept_beneficiary(), Ok(()));

            test::set_caller::<BittensorEnvironment>(accounts.alice);
            assert_eq!(
                contract.propose_beneficiary(accounts.django),
                Err(Error::Unauthorized)
            );

            test::set_callee::<BittensorEnvironment>(accounts.bob);
            test::set_block_number::<BittensorEnvironment>(UNLOCK_BLOCK);
            test::set_caller::<BittensorEnvironment>(accounts.django);

            let result = std::panic::catch_unwind(AssertUnwindSafe(|| contract.claim()));
            assert_terminated_to(result, accounts.charlie, ESCROW_TAO_BALANCE);

            assert_eq!(
                last_transfer().map(|transfer| transfer.destination_coldkey),
                Some(accounts.charlie)
            );
        }
    }
}
