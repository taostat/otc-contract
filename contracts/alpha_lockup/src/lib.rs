#![cfg_attr(not(feature = "std"), no_std, no_main)]

pub use self::alpha_lockup::{AlphaLockup, AlphaLockupRef};

#[ink::contract(env = otc_shared::BittensorEnvironment)]
mod alpha_lockup {
    use otc_shared::{AlphaAmount, AlphaCurrency, NetUid};

    /// Error types for the Alpha Lockup escrow contract
    #[derive(Debug, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    pub enum Error {
        /// Caller is not the designated buyer
        Unauthorized,
        /// Lockup period has not expired yet
        LockupNotExpired,
        /// Alpha has already been claimed
        AlreadyClaimed,
        /// Failed to query stake
        StakeQueryFailed,
        /// Failed to transfer stake
        StakeTransferFailed,
    }

    /// Event emitted when Alpha is claimed
    #[ink::event]
    pub struct AlphaClaimed {
        #[ink(topic)]
        pub buyer: AccountId,
        #[ink(topic)]
        pub netuid: NetUid,
        pub amount_claimed: AlphaAmount,
        pub unlock_block: BlockNumber,
    }

    /// Information about a lockup escrow
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    pub struct LockupInfo {
        pub buyer: AccountId,
        pub netuid: NetUid,
        pub alpha_amount: AlphaAmount,
        pub unlock_block: BlockNumber,
        pub hotkey: AccountId,
        pub claimed: bool,
        pub current_stake: AlphaAmount,
    }

    #[ink(storage)]
    pub struct AlphaLockup {
        /// The buyer who can claim the locked Alpha
        buyer: AccountId,
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
                netuid,
                alpha_amount,
                unlock_block,
                hotkey,
                claimed: false,
            }
        }

        /// Claim the locked Alpha after the lockup period has expired
        /// Only the designated buyer can claim
        /// Transfers all stake (including dividends) to the buyer
        /// Terminates the contract after successful claim
        #[ink(message)]
        pub fn claim(&mut self) -> Result<(), Error> {
            // Access control: only buyer can claim
            if self.env().caller() != self.buyer {
                return Err(Error::Unauthorized);
            }

            // Time check: must be past unlock block
            if self.env().block_number() < self.unlock_block {
                return Err(Error::LockupNotExpired);
            }

            // Prevent double-claiming
            if self.claimed {
                return Err(Error::AlreadyClaimed);
            }

            // Mark as claimed first (effects before interactions)
            self.claimed = true;

            // Get current stake (original + dividends)
            let current_stake = self.get_current_stake()?;

            // Transfer all stake to buyer
            self.env()
                .extension()
                .transfer_stake(
                    self.buyer,
                    self.hotkey,
                    self.netuid,
                    self.netuid,
                    AlphaCurrency::from(current_stake),
                )
                .map_err(|_| Error::StakeTransferFailed)?;

            // Emit event before termination
            self.env().emit_event(AlphaClaimed {
                buyer: self.buyer,
                netuid: self.netuid,
                amount_claimed: current_stake,
                unlock_block: self.unlock_block,
            });

            // Terminate the contract, sending any remaining TAO balance to buyer
            self.env().terminate_contract(self.buyer)
        }

        /// Get information about this lockup escrow
        #[ink(message)]
        pub fn get_info(&self) -> Result<LockupInfo, Error> {
            let current_stake = self.get_current_stake()?;

            Ok(LockupInfo {
                buyer: self.buyer,
                netuid: self.netuid,
                alpha_amount: self.alpha_amount,
                unlock_block: self.unlock_block,
                hotkey: self.hotkey,
                claimed: self.claimed,
                current_stake,
            })
        }

        /// Get the buyer address
        #[ink(message)]
        pub fn get_buyer(&self) -> AccountId {
            self.buyer
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
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use ink::env::test;
        use otc_shared::BittensorEnvironment;

        fn default_accounts() -> test::DefaultAccounts<BittensorEnvironment> {
            test::default_accounts::<BittensorEnvironment>()
        }

        #[ink::test]
        fn constructor_works() {
            let accounts = default_accounts();
            let unlock_block = 1000u32;

            let contract = AlphaLockup::new(
                accounts.alice,
                1u16,
                1_000_000_000,
                unlock_block,
                accounts.bob,
            );

            assert_eq!(contract.get_buyer(), accounts.alice);
            assert_eq!(contract.get_unlock_block(), unlock_block);
            assert!(!contract.is_claimed());
        }

        #[ink::test]
        fn blocks_until_unlock_works() {
            let accounts = default_accounts();

            // Set current block to 500
            test::set_block_number::<BittensorEnvironment>(500);

            let contract = AlphaLockup::new(
                accounts.alice,
                1u16,
                1_000_000_000,
                1000, // Unlock at block 1000
                accounts.bob,
            );

            assert_eq!(contract.blocks_until_unlock(), 500);

            // Advance to block 1000
            test::set_block_number::<BittensorEnvironment>(1000);
            assert_eq!(contract.blocks_until_unlock(), 0);

            // Past unlock
            test::set_block_number::<BittensorEnvironment>(1500);
            assert_eq!(contract.blocks_until_unlock(), 0);
        }

        #[ink::test]
        fn claim_fails_if_not_buyer() {
            let accounts = default_accounts();

            // Set block past unlock
            test::set_block_number::<BittensorEnvironment>(2000);

            let mut contract = AlphaLockup::new(
                accounts.alice, // buyer is alice
                1u16,
                1_000_000_000,
                1000,
                accounts.bob,
            );

            // Set caller to bob (not the buyer)
            test::set_caller::<BittensorEnvironment>(accounts.bob);

            let result = contract.claim();
            assert_eq!(result, Err(Error::Unauthorized));
        }

        #[ink::test]
        fn claim_fails_if_lockup_not_expired() {
            let accounts = default_accounts();

            // Set block before unlock
            test::set_block_number::<BittensorEnvironment>(500);

            let mut contract = AlphaLockup::new(
                accounts.alice,
                1u16,
                1_000_000_000,
                1000, // Unlock at block 1000
                accounts.bob,
            );

            // Set caller to buyer
            test::set_caller::<BittensorEnvironment>(accounts.alice);

            let result = contract.claim();
            assert_eq!(result, Err(Error::LockupNotExpired));
        }
    }
}
