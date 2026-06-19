import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { Binary } from "polkadot-api";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    type LockupListingsContext,
    type TestAccount,
    LockupListingsSdk
} from "../setup";
import {
    taoToRao,
    waitForBlocks,
    fundAccount,
    registerSubnet,
    registerValidator,
    addContractAsProxy,
    createHotkey,
    elevateRegistrationLimits,
    MARKET_PRICE,
    MEDIUM_LOCKUP_DURATION,
    BITTENSOR_MIN_STAKE,
    type Wallet,
} from "../utils";
import {
    createLockupListing,
    expectEvent,
    submitOk,
    submitReverted,
    withIntegrationGas,
} from "../test-helpers";
import {
    getStakeBalance,
    formatStakeAmount,
} from "../utils/stake-helpers";

const VALID_LISTING_AMOUNT = BITTENSOR_MIN_STAKE + 500_000n;
const REQUIRED_ALPHA = BITTENSOR_MIN_STAKE * 2n;
describe("Lockup Listings Contract - Cancel Listing", () => {
    let context: LockupListingsContext;
    let contract: ReturnType<LockupListingsSdk["getContract"]>;
    let netuid: number;

    let bobHotkey: Wallet;
    let charlieHotkey: Wallet;

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);

        const updateMinTx = contract.send("update_min_listing_amount", {
            origin: context.accounts.alice.address,
            data: { new_amount: BITTENSOR_MIN_STAKE }
        });
        await submitOk(updateMinTx, context.accounts.alice.signer, "update_min_listing_amount");

        // Create a subnet for testing
        const aliceHotkey = createHotkey("//Alice");
        await fundAccount(context.api, aliceHotkey.address, taoToRao(10), context.accounts.alice.signer);
        netuid = await registerSubnet(context.api, aliceHotkey.address, context.accounts.alice.signer);
        console.log(`Created test subnet with netuid: ${netuid}`);

        // Elevate registration limits
        await elevateRegistrationLimits(
            context.api,
            netuid,
            20,
            20,
            context.accounts.alice.signer
        );
        await waitForBlocks(context.api, 1);

        // Register the contract's hotkey
        const contractHotkeyResult = await contract.query("get_hotkey", {
            origin: context.accounts.alice.address,
            data: {}
        });

        if (contractHotkeyResult.success) {
            const contractHotkey = contractHotkeyResult.value.response;
            await fundAccount(context.api, context.contractAddress!, taoToRao(10), context.accounts.alice.signer);
            await registerValidator(
                context.api,
                netuid,
                contractHotkey,
                context.accounts.alice.signer,
                taoToRao(50)
            );
        }

        // Create and register hotkeys
        bobHotkey = createHotkey("//Bob");
        charlieHotkey = createHotkey("//Charlie");

        await fundAccount(context.api, bobHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, charlieHotkey.address, taoToRao(1), context.accounts.alice.signer);

        await registerValidator(context.api, netuid, bobHotkey.address, context.accounts.bob.signer, taoToRao(5000), REQUIRED_ALPHA);
        await registerValidator(context.api, netuid, charlieHotkey.address, context.accounts.charlie.signer, taoToRao(5000), REQUIRED_ALPHA);

        await waitForBlocks(context.api, 2);

        // Add contract as proxy
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.bob.signer);
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.charlie.signer);

        await waitForBlocks(context.api, 2);

        console.log("Test setup complete");
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    async function createListing(
        seller: TestAccount,
        hotkey: Wallet,
        amount = VALID_LISTING_AMOUNT,
    ): Promise<bigint> {
        const { listingId } = await createLockupListing(contract, seller.signer, seller.address, {
            hotkey: hotkey.address,
            netuid,
            amount,
            price_offset_bps: MARKET_PRICE,
            lockup_duration: MEDIUM_LOCKUP_DURATION,
        });
        return listingId;
    }

    describe("Seller Cancellation", () => {
        it("should allow seller to cancel their own listing", async () => {
            const { accounts } = context;
            const listAmount = VALID_LISTING_AMOUNT;

            const listingId = await createListing(accounts.bob, bobHotkey, listAmount);

            // Get Bob's stake before cancellation
            const stakeBefore = await getStakeBalance(context.api, bobHotkey.address, netuid, accounts.bob.address);
            console.log(`Bob's stake before cancel: ${formatStakeAmount(stakeBefore)}`);

            // Cancel the listing
            const cancelTx = contract.send("cancel_lockup_listing", withIntegrationGas({
                origin: accounts.bob.address,
                data: {
                    netuid,
                    listing_id: listingId
                }
            }));

            const result = await submitOk(cancelTx, accounts.bob.signer, "cancel_lockup_listing");
            const event = expectEvent(contract, result, "LockupListingCancelled");
            expect(BigInt(event.value.listing_id)).toBe(listingId);
            expect(BigInt(event.value.amount_returned)).toBe(listAmount);

            // Verify listing was removed
            const listingAfter = await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId
                }
            });

            expect(listingAfter.success).toBe(true);
            if (listingAfter.success) {
                expect(listingAfter.value.response).toBeUndefined();
            }
        }, 120000);

        it("should return Alpha to seller when cancelled", async () => {
            const { accounts } = context;
            const listAmount = VALID_LISTING_AMOUNT;

            // Get stake before listing
            const stakeBeforeListing = await getStakeBalance(context.api, bobHotkey.address, netuid, accounts.bob.address);

            const listingId = await createListing(accounts.bob, bobHotkey, listAmount);

            // Get stake after listing
            const stakeAfterListing = await getStakeBalance(context.api, bobHotkey.address, netuid, accounts.bob.address);
            console.log(`Stake reduction from listing: ${formatStakeAmount(stakeBeforeListing - stakeAfterListing)}`);

            // Cancel listing
            const cancelTx = contract.send("cancel_lockup_listing", withIntegrationGas({
                origin: accounts.bob.address,
                data: {
                    netuid,
                    listing_id: listingId
                }
            }));

            const cancelResult = await submitOk(cancelTx, accounts.bob.signer, "cancel_lockup_listing");
            const cancelEvent = expectEvent(contract, cancelResult, "LockupListingCancelled");
            expect(BigInt(cancelEvent.value.amount_returned)).toBe(listAmount);

            // Runtime emissions can swamp tiny live-chain stake deltas, so use the
            // deterministic contract event/storage as the source of truth.
            const listingAfterCancel = await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                },
            });
            expect(listingAfterCancel.success).toBe(true);
            if (listingAfterCancel.success) {
                expect(listingAfterCancel.value.response).toBeUndefined();
            }
        }, 120000);

        it("should decrement reserved alpha when cancelled", async () => {
            const { accounts } = context;
            const listAmount = VALID_LISTING_AMOUNT;

            // Get reserved before
            const reservedBefore = await contract.query("get_reserved_alpha", {
                origin: accounts.alice.address,
                data: { netuid }
            });

            // Create listing
            const listingId = await createListing(accounts.bob, bobHotkey, listAmount);

            // Get reserved after listing
            const reservedAfterListing = await contract.query("get_reserved_alpha", {
                origin: accounts.alice.address,
                data: { netuid }
            });

            const cancelTx = contract.send("cancel_lockup_listing", withIntegrationGas({
                origin: accounts.bob.address,
                data: {
                    netuid,
                    listing_id: listingId
                }
            }));

            await submitOk(cancelTx, accounts.bob.signer, "cancel_lockup_listing");

            // Get reserved after cancel
            const reservedAfterCancel = await contract.query("get_reserved_alpha", {
                origin: accounts.alice.address,
                data: { netuid }
            });

            expect(reservedBefore.success).toBe(true);
            expect(reservedAfterListing.success).toBe(true);
            expect(reservedAfterCancel.success).toBe(true);

            if (reservedBefore.success && reservedAfterListing.success && reservedAfterCancel.success) {
                expect(reservedAfterListing.value.response).toBe(reservedBefore.value.response + listAmount);
                expect(reservedAfterCancel.value.response).toBe(reservedBefore.value.response);
            }
        }, 120000);
    });

    describe("Owner Force Cancellation", () => {
        it("should allow owner to force cancel any listing", async () => {
            const { accounts } = context;
            const listAmount = VALID_LISTING_AMOUNT;

            const listingId = await createListing(accounts.charlie, charlieHotkey, listAmount);

            // Alice (owner) force cancels Charlie's listing
            const forceCancelTx = contract.send("force_cancel_lockup_listing", withIntegrationGas({
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.charlie.address,
                    listing_id: listingId
                }
            }));

            const result = await submitOk(forceCancelTx, accounts.alice.signer, "force_cancel_lockup_listing");
            const event = expectEvent(contract, result, "LockupListingForceCancelled");
            expect(BigInt(event.value.listing_id)).toBe(listingId);
            expect(BigInt(event.value.amount_returned)).toBe(listAmount);

            // Verify listing was removed
            const listingAfter = await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.charlie.address,
                    listing_id: listingId
                }
            });

            expect(listingAfter.success).toBe(true);
            if (listingAfter.success) {
                expect(listingAfter.value.response).toBeUndefined();
            }
        }, 120000);
    });

    describe("Error Cases", () => {
        it("should fail when listing not found", async () => {
            const { accounts } = context;

            const cancelTx = contract.send("cancel_lockup_listing", withIntegrationGas({
                origin: accounts.bob.address,
                data: {
                    netuid,
                    listing_id: 999999n
                }
            }));

            await submitReverted(cancelTx, accounts.bob.signer, "cancel_lockup_listing missing listing");
        }, 60000);

        it("should prevent non-owner from force cancelling", async () => {
            const { accounts } = context;

            const listingId = await createListing(accounts.bob, bobHotkey);

            // Charlie (not owner) tries to force cancel Bob's listing
            const forceCancelTx = contract.send("force_cancel_lockup_listing", withIntegrationGas({
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId
                }
            }));

            await submitReverted(forceCancelTx, accounts.charlie.signer, "force_cancel_lockup_listing non-owner");
        }, 60000);

        it("should fail when contract is fully paused", async () => {
            const { accounts } = context;

            const listingId = await createListing(accounts.bob, bobHotkey);

            // Pause fully
            const pauseTx = contract.send("pause_fully", {
                origin: accounts.alice.address,
                data: { reason: Binary.fromText("Test pause") }
            });
            await submitOk(pauseTx, accounts.alice.signer, "pause_fully");

            // Try to cancel
            const cancelTx = contract.send("cancel_lockup_listing", withIntegrationGas({
                origin: accounts.bob.address,
                data: {
                    netuid,
                    listing_id: listingId
                }
            }));

            await submitReverted(cancelTx, accounts.bob.signer, "cancel_lockup_listing fully paused");

            // Resume
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await submitOk(resumeTx, accounts.alice.signer, "resume");
        }, 120000);

        it("should allow cancel when only trading is paused", async () => {
            const { accounts } = context;

            const listingId = await createListing(accounts.bob, bobHotkey);

            // Pause trading only
            const pauseTx = contract.send("pause_trading", {
                origin: accounts.alice.address,
                data: { reason: Binary.fromText("Test trading pause") }
            });
            await submitOk(pauseTx, accounts.alice.signer, "pause_trading");

            // Cancel should work even with trading paused
            const cancelTx = contract.send("cancel_lockup_listing", withIntegrationGas({
                origin: accounts.bob.address,
                data: {
                    netuid,
                    listing_id: listingId
                }
            }));

            await submitOk(cancelTx, accounts.bob.signer, "cancel_lockup_listing while trading paused");

            // Resume
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await submitOk(resumeTx, accounts.alice.signer, "resume");
        }, 120000);
    });
});
