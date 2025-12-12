import { describe, it, expect, beforeAll, afterAll } from "vitest";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    type LockupListingsContext,
    LockupListingsSdk,
    bigintReplacer,
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
    SHORT_LOCKUP_DURATION,
    getCurrentBlock,
    waitUntilBlock,
    type Wallet,
} from "../utils";
import {
    getStakeBalance,
    formatStakeAmount,
} from "../utils/stake-helpers";

describe("Lockup Listings Contract - Claim", () => {
    let context: LockupListingsContext;
    let contract: ReturnType<LockupListingsSdk["getContract"]>;
    let netuid: number;

    let bobHotkey: Wallet;

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
                taoToRao(100)
            );
        }

        // Create and register Bob's hotkey
        bobHotkey = createHotkey("//Bob");
        await fundAccount(context.api, bobHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await registerValidator(context.api, netuid, bobHotkey.address, context.accounts.bob.signer, taoToRao(100));

        await waitForBlocks(context.api, 2);

        // Add contract as proxy for Bob
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.bob.signer);

        await waitForBlocks(context.api, 2);

        // Fund Charlie (buyer) with TAO
        await fundAccount(context.api, context.accounts.charlie.address, taoToRao(200), context.accounts.alice.signer);

        console.log("Test setup complete");
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    describe("Full Lifecycle", () => {
        it("should complete full lifecycle: create -> take -> claim", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(5);
            const lockupDuration = SHORT_LOCKUP_DURATION; // 10 blocks

            // Get Charlie's stake before (should be 0 or minimal)
            const buyerStakeBefore = await getStakeBalance(
                context.api,
                context.accounts.alice.address, // contract's hotkey
                netuid,
                accounts.charlie.address
            );
            console.log(`Charlie's stake before: ${formatStakeAmount(buyerStakeBefore)}`);

            // Step 1: Bob creates a lockup listing
            console.log("Step 1: Creating lockup listing...");
            const currentBlock = await getCurrentBlock(context.api);
            console.log(`Current block: ${currentBlock}`);

            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: lockupDuration
                }
            });

            await listTx.signAndSubmit(accounts.bob.signer);
            await waitForBlocks(context.api, 2);

            // Get the listing
            const listingsResult = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.bob.address,
                    netuid
                }
            });

            expect(listingsResult.success).toBe(true);
            if (!listingsResult.success || listingsResult.value.response.length === 0) {
                throw new Error("Failed to create listing");
            }

            const listingId = listingsResult.value.response[listingsResult.value.response.length - 1];
            console.log(`Listing created with ID: ${listingId}`);

            // Step 2: Estimate price and Charlie takes the listing
            console.log("Step 2: Taking lockup listing...");
            const estimateResult = await contract.query("estimate_lockup_price", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: listAmount
                }
            });

            expect(estimateResult.success).toBe(true);
            if (!estimateResult.success) {
                throw new Error("Failed to estimate price");
            }

            const [, , totalRequired] = estimateResult.value.response;
            console.log(`Total TAO required: ${totalRequired}`);

            const blockBeforeTake = await getCurrentBlock(context.api);
            console.log(`Block before take: ${blockBeforeTake}`);

            const takeTx = contract.send("take_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: listAmount
                },
                value: totalRequired
            });

            const takeResult = await takeTx.signAndSubmit(accounts.charlie.signer);
            console.log("Take result:", {
                ok: takeResult.ok,
                dispatchError: JSON.stringify(takeResult.dispatchError, bigintReplacer, 2),
                valueSent: totalRequired.toString()
            });
            expect(takeResult.ok).toBe(true);

            // Verify LockupListingTaken event
            const takeEvents = contract.filterEvents(takeResult.events);
            const takenEvent = takeEvents.find(e => e.type === "LockupListingTaken");
            expect(takenEvent).toBeDefined();
            if (takenEvent) {
                expect(takenEvent.value.seller).toBe(accounts.bob.address);
                expect(takenEvent.value.buyer).toBe(accounts.charlie.address);
                expect(takenEvent.value.escrow_account).toBeDefined();
                console.log(`LockupListingTaken event - escrow: ${takenEvent.value.escrow_account}`);
            }

            await waitForBlocks(context.api, 2);

            // Get escrow address
            const escrowResult = await contract.query("get_escrow", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    listing_id: listingId,
                    purchase_id: 1n
                }
            });

            expect(escrowResult.success).toBe(true);
            if (!escrowResult.success || !escrowResult.value.response) {
                throw new Error("Failed to get escrow address");
            }

            const escrowAddress = escrowResult.value.response;
            console.log(`Escrow created at: ${escrowAddress}`);

            // Get the escrow contract and query its info
            const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);

            // Query escrow info
            const infoResult = await escrowContract.query("get_info", {
                origin: accounts.charlie.address,
                data: {}
            });

            expect(infoResult.success).toBe(true);
            if (infoResult.success) {
                const info = infoResult.value.response;
                console.log(`Escrow info: buyer=${info.buyer}, unlock_block=${info.unlock_block}, claimed=${info.claimed}`);
                expect(info.buyer).toBe(accounts.charlie.address);
                expect(info.claimed).toBe(false);
            }

            // Step 3: Wait for lockup to expire
            console.log("Step 3: Waiting for lockup to expire...");
            const unlockBlockResult = await escrowContract.query("get_unlock_block", {
                origin: accounts.charlie.address,
                data: {}
            });

            if (!unlockBlockResult.success) {
                throw new Error("Failed to get unlock block");
            }

            const unlockBlock = unlockBlockResult.value.response;
            console.log(`Unlock block: ${unlockBlock}`);

            // Wait until unlock block
            await waitUntilBlock(context.api, unlockBlock);
            console.log("Lockup period expired!");

            // Step 4: Charlie claims the escrow
            console.log("Step 4: Claiming escrow...");
            const claimTx = escrowContract.send("claim", {
                origin: accounts.charlie.address,
                data: {}
            });

            const claimResult = await claimTx.signAndSubmit(accounts.charlie.signer);
            console.log(`Claim result: ${claimResult.ok}`);
            expect(claimResult.ok).toBe(true);

            // Verify AlphaClaimed event from escrow contract
            const claimEvents = escrowContract.filterEvents(claimResult.events);
            const claimedEvent = claimEvents.find(e => e.type === "AlphaClaimed");
            expect(claimedEvent).toBeDefined();
            if (claimedEvent) {
                expect(claimedEvent.value.buyer).toBe(accounts.charlie.address);
                expect(claimedEvent.value.netuid).toBe(netuid);
                console.log(`AlphaClaimed event - amount_claimed: ${claimedEvent.value.amount_claimed}`);
            }

            await waitForBlocks(context.api, 2);

            // Verify Charlie received the Alpha
            const buyerStakeAfter = await getStakeBalance(
                context.api,
                context.accounts.alice.address, // contract's hotkey
                netuid,
                accounts.charlie.address
            );
            console.log(`Charlie's stake after claim: ${formatStakeAmount(buyerStakeAfter)}`);

            // Charlie should have received approximately the listing amount
            const stakeIncrease = buyerStakeAfter - buyerStakeBefore;
            const tolerance = taoToRao(1);
            expect(stakeIncrease).toBeGreaterThanOrEqual(listAmount - tolerance);
        }, 600000); // 10 minute timeout for full lifecycle test
    });

    describe("Claim Error Cases", () => {
        let escrowAddress: string;

        beforeAll(async () => {
            const { accounts } = context;
            const listAmount = taoToRao(5);
            const lockupDuration = 100; // Longer lockup for error tests

            // Create listing
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: lockupDuration
                }
            });

            await listTx.signAndSubmit(accounts.bob.signer);
            await waitForBlocks(context.api, 2);

            // Get listing
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

            // Estimate price
            const estimateResult = await contract.query("estimate_lockup_price", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: listAmount
                }
            });

            if (!estimateResult.success) {
                throw new Error("Failed to estimate price");
            }

            const [, , totalRequired] = estimateResult.value.response;

            // Take listing
            const takeTx = contract.send("take_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: listAmount
                },
                value: totalRequired
            });

            await takeTx.signAndSubmit(accounts.charlie.signer);
            await waitForBlocks(context.api, 2);

            // Get escrow
            const escrowResult = await contract.query("get_escrow", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    listing_id: listingId,
                    purchase_id: 2n // Increment from previous test
                }
            });

            if (!escrowResult.success || !escrowResult.value.response) {
                // Try with purchase_id 1 if this is first escrow
                const escrowResult2 = await contract.query("get_escrow", {
                    origin: accounts.alice.address,
                    data: {
                        netuid,
                        listing_id: listingId,
                        purchase_id: 1n
                    }
                });

                if (!escrowResult2.success || !escrowResult2.value.response) {
                    throw new Error("Failed to get escrow");
                }
                escrowAddress = escrowResult2.value.response;
            } else {
                escrowAddress = escrowResult.value.response;
            }
        }, 300000);

        it("should fail to claim before unlock block", async () => {
            const { accounts } = context;

            const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);

            // Check blocks remaining
            const blocksRemainingResult = await escrowContract.query("blocks_until_unlock", {
                origin: accounts.charlie.address,
                data: {}
            });

            if (blocksRemainingResult.success) {
                console.log(`Blocks until unlock: ${blocksRemainingResult.value.response}`);
            }

            // Try to claim before unlock
            const claimTx = escrowContract.send("claim", {
                origin: accounts.charlie.address,
                data: {}
            });

            const result = await claimTx.signAndSubmit(accounts.charlie.signer);
            expect(result.ok).toBe(false);
        }, 60000);

        it("should fail if caller is not the buyer", async () => {
            const { accounts } = context;

            const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);

            // Dave (not the buyer) tries to claim
            const claimTx = escrowContract.send("claim", {
                origin: accounts.dave.address,
                data: {}
            });

            const result = await claimTx.signAndSubmit(accounts.dave.signer);
            expect(result.ok).toBe(false);
        }, 60000);
    });

    describe("Escrow Query Functions", () => {
        it("should return correct buyer from escrow", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(3);

            // Create and take a listing
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: SHORT_LOCKUP_DURATION
                }
            });

            await listTx.signAndSubmit(accounts.bob.signer);
            await waitForBlocks(context.api, 2);

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

            const estimateResult = await contract.query("estimate_lockup_price", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: listAmount
                }
            });

            if (!estimateResult.success) {
                throw new Error("Failed to estimate price");
            }

            const [, , totalRequired] = estimateResult.value.response;

            // Fund Dave and have him take the listing
            await fundAccount(context.api, accounts.dave.address, taoToRao(100), accounts.alice.signer);

            const takeTx = contract.send("take_lockup_listing", {
                origin: accounts.dave.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: listAmount
                },
                value: totalRequired
            });

            await takeTx.signAndSubmit(accounts.dave.signer);
            await waitForBlocks(context.api, 2);

            // Get escrow - find the latest purchase_id
            let escrowAddress: string | null = null;
            for (let i = 10n; i >= 1n; i--) {
                const escrowResult = await contract.query("get_escrow", {
                    origin: accounts.alice.address,
                    data: {
                        netuid,
                        listing_id: listingId,
                        purchase_id: i
                    }
                });

                if (escrowResult.success && escrowResult.value.response) {
                    escrowAddress = escrowResult.value.response;
                    break;
                }
            }

            if (!escrowAddress) {
                throw new Error("Failed to find escrow");
            }

            const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);

            // Query buyer
            const buyerResult = await escrowContract.query("get_buyer", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(buyerResult.success).toBe(true);
            if (buyerResult.success) {
                expect(buyerResult.value.response).toBe(accounts.dave.address);
            }
        }, 180000);
    });
});
