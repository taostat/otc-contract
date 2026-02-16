import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { Binary } from "polkadot-api";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    type LockupListingsContext,
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
    type Wallet,
} from "../utils";
import {
    getStakeBalance,
    formatStakeAmount,
} from "../utils/stake-helpers";

describe("Lockup Listings Contract - Cancel Listing", () => {
    let context: LockupListingsContext;
    let contract: ReturnType<LockupListingsSdk["getContract"]>;
    let netuid: number;

    let bobHotkey: Wallet;
    let charlieHotkey: Wallet;

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);

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

        await registerValidator(context.api, netuid, bobHotkey.address, context.accounts.bob.signer, taoToRao(100));
        await registerValidator(context.api, netuid, charlieHotkey.address, context.accounts.charlie.signer, taoToRao(100));

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

    describe("Seller Cancellation", () => {
        it("should allow seller to cancel their own listing", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(10);

            // Create a listing first
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            await listTx.signAndSubmit(accounts.bob.signer);
            await waitForBlocks(context.api, 2);

            // Get the listing ID
            const listingsResult = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.bob.address,
                    netuid
                }
            });

            expect(listingsResult.success).toBe(true);
            if (!listingsResult.success || listingsResult.value.response.length === 0) {
                throw new Error("Failed to create listing for cancellation test");
            }

            const listingId = listingsResult.value.response[listingsResult.value.response.length - 1];

            // Get Bob's stake before cancellation
            const stakeBefore = await getStakeBalance(context.api, bobHotkey.address, netuid, accounts.bob.address);
            console.log(`Bob's stake before cancel: ${formatStakeAmount(stakeBefore)}`);

            // Cancel the listing
            const cancelTx = contract.send("cancel_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    netuid,
                    listing_id: listingId
                }
            });

            const result = await cancelTx.signAndSubmit(accounts.bob.signer);
            expect(result.ok).toBe(true);

            await waitForBlocks(context.api, 2);

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
            const listAmount = taoToRao(10);

            // Get stake before listing
            const stakeBeforeListing = await getStakeBalance(context.api, bobHotkey.address, netuid, accounts.bob.address);

            // Create listing
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            await listTx.signAndSubmit(accounts.bob.signer);
            await waitForBlocks(context.api, 2);

            // Get stake after listing
            const stakeAfterListing = await getStakeBalance(context.api, bobHotkey.address, netuid, accounts.bob.address);
            console.log(`Stake reduction from listing: ${formatStakeAmount(stakeBeforeListing - stakeAfterListing)}`);

            // Get listing ID
            const listingsResult = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.bob.address,
                    netuid
                }
            });

            if (!listingsResult.success || listingsResult.value.response.length === 0) {
                throw new Error("Failed to get listing");
            }

            const listingId = listingsResult.value.response[listingsResult.value.response.length - 1];

            // Cancel listing
            const cancelTx = contract.send("cancel_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    netuid,
                    listing_id: listingId
                }
            });

            await cancelTx.signAndSubmit(accounts.bob.signer);
            await waitForBlocks(context.api, 2);

            // Verify stake was returned
            const stakeAfterCancel = await getStakeBalance(context.api, bobHotkey.address, netuid, accounts.bob.address);
            console.log(`Stake after cancel: ${formatStakeAmount(stakeAfterCancel)}`);

            // Compare what was returned to what was actually deducted
            const stakeDeducted = stakeBeforeListing - stakeAfterListing;
            const stakeReturned = stakeAfterCancel - stakeAfterListing;
            console.log(`Stake deducted from listing: ${formatStakeAmount(stakeDeducted)}`);
            console.log(`Stake returned from cancel: ${formatStakeAmount(stakeReturned)}`);

            // Verify that cancel returns some stake (there's slippage in both directions)
            expect(stakeReturned).toBeGreaterThan(0n);
            // Verify stake after cancel is higher than when listing was active
            expect(stakeAfterCancel).toBeGreaterThan(stakeAfterListing);
        }, 120000);

        it("should decrement reserved alpha when cancelled", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(10);

            // Get reserved before
            const reservedBefore = await contract.query("get_reserved_alpha", {
                origin: accounts.alice.address,
                data: { netuid }
            });

            // Create listing
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            await listTx.signAndSubmit(accounts.bob.signer);
            await waitForBlocks(context.api, 1);

            // Get reserved after listing
            const reservedAfterListing = await contract.query("get_reserved_alpha", {
                origin: accounts.alice.address,
                data: { netuid }
            });

            // Get listing ID and cancel
            const listingsResult = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.bob.address,
                    netuid
                }
            });

            if (!listingsResult.success || listingsResult.value.response.length === 0) {
                throw new Error("Failed to get listing");
            }

            const listingId = listingsResult.value.response[listingsResult.value.response.length - 1];

            const cancelTx = contract.send("cancel_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    netuid,
                    listing_id: listingId
                }
            });

            await cancelTx.signAndSubmit(accounts.bob.signer);
            await waitForBlocks(context.api, 1);

            // Get reserved after cancel
            const reservedAfterCancel = await contract.query("get_reserved_alpha", {
                origin: accounts.alice.address,
                data: { netuid }
            });

            expect(reservedBefore.success).toBe(true);
            expect(reservedAfterListing.success).toBe(true);
            expect(reservedAfterCancel.success).toBe(true);

            if (reservedBefore.success && reservedAfterListing.success && reservedAfterCancel.success) {
                // Reserved should have increased after listing
                expect(reservedAfterListing.value.response).toBeGreaterThan(reservedBefore.value.response);
                // Reserved should decrease after cancel (approximately back to original)
                const tolerance = taoToRao(1);
                expect(reservedAfterCancel.value.response).toBeLessThanOrEqual(reservedBefore.value.response + tolerance);
            }
        }, 120000);
    });

    describe("Owner Force Cancellation", () => {
        it("should allow owner to force cancel any listing", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(10);

            // Charlie creates a listing
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    hotkey: charlieHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            await listTx.signAndSubmit(accounts.charlie.signer);
            await waitForBlocks(context.api, 2);

            // Get the listing ID
            const listingsResult = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.charlie.address,
                    netuid
                }
            });

            expect(listingsResult.success).toBe(true);
            if (!listingsResult.success || listingsResult.value.response.length === 0) {
                throw new Error("Failed to create listing");
            }

            const listingId = listingsResult.value.response[listingsResult.value.response.length - 1];

            // Alice (owner) force cancels Charlie's listing
            const forceCancelTx = contract.send("force_cancel_lockup_listing", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.charlie.address,
                    listing_id: listingId
                }
            });

            const result = await forceCancelTx.signAndSubmit(accounts.alice.signer);
            expect(result.ok).toBe(true);

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

            const cancelTx = contract.send("cancel_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    netuid,
                    listing_id: 999999n
                }
            });

            const result = await cancelTx.signAndSubmit(accounts.bob.signer);
            expect(result.ok).toBe(false);
        }, 60000);

        it("should prevent non-owner from force cancelling", async () => {
            const { accounts } = context;

            // Create a listing first
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: taoToRao(5),
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            await listTx.signAndSubmit(accounts.bob.signer);
            await waitForBlocks(context.api, 1);

            const listingsResult = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.bob.address,
                    netuid
                }
            });

            if (!listingsResult.success || listingsResult.value.response.length === 0) {
                throw new Error("Failed to create listing");
            }

            const listingId = listingsResult.value.response[listingsResult.value.response.length - 1];

            // Charlie (not owner) tries to force cancel Bob's listing
            const forceCancelTx = contract.send("force_cancel_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId
                }
            });

            const result = await forceCancelTx.signAndSubmit(accounts.charlie.signer);
            expect(result.ok).toBe(false);
        }, 60000);

        it("should fail when contract is fully paused", async () => {
            const { accounts } = context;

            // Create a listing first
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: taoToRao(5),
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            await listTx.signAndSubmit(accounts.bob.signer);
            await waitForBlocks(context.api, 1);

            const listingsResult = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.bob.address,
                    netuid
                }
            });

            if (!listingsResult.success || listingsResult.value.response.length === 0) {
                throw new Error("Failed to create listing");
            }

            const listingId = listingsResult.value.response[listingsResult.value.response.length - 1];

            // Pause fully
            const pauseTx = contract.send("pause_fully", {
                origin: accounts.alice.address,
                data: { reason: Binary.fromText("Test pause") }
            });
            await pauseTx.signAndSubmit(accounts.alice.signer);

            // Try to cancel
            const cancelTx = contract.send("cancel_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    netuid,
                    listing_id: listingId
                }
            });

            const result = await cancelTx.signAndSubmit(accounts.bob.signer);
            expect(result.ok).toBe(false);

            // Resume
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await resumeTx.signAndSubmit(accounts.alice.signer);
        }, 120000);

        it("should allow cancel when only trading is paused", async () => {
            const { accounts } = context;

            // Create a listing first
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: taoToRao(5),
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            await listTx.signAndSubmit(accounts.bob.signer);
            await waitForBlocks(context.api, 1);

            const listingsResult = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.bob.address,
                    netuid
                }
            });

            if (!listingsResult.success || listingsResult.value.response.length === 0) {
                throw new Error("Failed to create listing");
            }

            const listingId = listingsResult.value.response[listingsResult.value.response.length - 1];

            // Pause trading only
            const pauseTx = contract.send("pause_trading", {
                origin: accounts.alice.address,
                data: { reason: Binary.fromText("Test trading pause") }
            });
            await pauseTx.signAndSubmit(accounts.alice.signer);

            // Cancel should work even with trading paused
            const cancelTx = contract.send("cancel_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    netuid,
                    listing_id: listingId
                }
            });

            const result = await cancelTx.signAndSubmit(accounts.bob.signer);
            expect(result.ok).toBe(true);

            // Resume
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await resumeTx.signAndSubmit(accounts.alice.signer);
        }, 120000);
    });
});
