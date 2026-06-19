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
    getBalance,
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
    queryOk,
    submitOk,
    submitReverted,
    takeLockupListing,
} from "../test-helpers";

const PURCHASE_UNIT = BITTENSOR_MIN_STAKE + 500_000n;
const TWO_PURCHASE_UNITS = PURCHASE_UNIT * 2n;
const THREE_PURCHASE_UNITS = PURCHASE_UNIT * 3n;
const REQUIRED_ALPHA = PURCHASE_UNIT * 15n;
// Overpayment is refunded by the contract. This buffer keeps happy-path
// purchases deterministic when localnet market price moves before finalization.
const PURCHASE_PAYMENT_DRIFT_BUFFER = taoToRao(10);
describe("Lockup Listings Contract - Take Listing", () => {
    let context: LockupListingsContext;
    let contract: ReturnType<LockupListingsSdk["getContract"]>;
    let netuid: number;

    let bobHotkey: Wallet;
    let charlieHotkey: Wallet;
    let daveHotkey: Wallet;

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);

        const updateMinListingTx = contract.send("update_min_listing_amount", {
            origin: context.accounts.alice.address,
            data: { new_amount: BITTENSOR_MIN_STAKE }
        });
        await submitOk(updateMinListingTx, context.accounts.alice.signer, "update_min_listing_amount");

        const updateMinPurchaseTx = contract.send("update_min_purchase_amount", {
            origin: context.accounts.alice.address,
            data: { new_amount: BITTENSOR_MIN_STAKE }
        });
        await submitOk(updateMinPurchaseTx, context.accounts.alice.signer, "update_min_purchase_amount");

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
                taoToRao(100) // Higher stake for more available Alpha
            );
        }

        // Create and register hotkeys
        bobHotkey = createHotkey("//Bob");
        charlieHotkey = createHotkey("//Charlie");
        daveHotkey = createHotkey("//Dave");

        await fundAccount(context.api, bobHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, charlieHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, daveHotkey.address, taoToRao(1), context.accounts.alice.signer);

        // Register validators with stake
        await registerValidator(context.api, netuid, bobHotkey.address, context.accounts.bob.signer, taoToRao(5000), REQUIRED_ALPHA);
        await registerValidator(context.api, netuid, charlieHotkey.address, context.accounts.charlie.signer, taoToRao(5000));
        await registerValidator(context.api, netuid, daveHotkey.address, context.accounts.dave.signer, taoToRao(5000));

        await waitForBlocks(context.api, 2);

        // Add contract as proxy for sellers
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.bob.signer);

        await waitForBlocks(context.api, 2);

        // Fund buyers with TAO
        await fundAccount(context.api, context.accounts.charlie.address, taoToRao(200), context.accounts.alice.signer);
        await fundAccount(context.api, context.accounts.dave.address, taoToRao(200), context.accounts.alice.signer);

        console.log("Test setup complete");
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    async function createBobListing(amount: bigint, lockupDuration = MEDIUM_LOCKUP_DURATION): Promise<bigint> {
        const { listingId } = await createLockupListing(contract, context.accounts.bob.signer, context.accounts.bob.address, {
            hotkey: bobHotkey.address,
            netuid,
            amount,
            price_offset_bps: MARKET_PRICE,
            lockup_duration: lockupDuration,
        });
        return listingId;
    }

    async function estimateListing(seller: string, listingId: bigint, amount: bigint): Promise<[bigint, bigint, bigint]> {
        return queryOk<[bigint, bigint, bigint]>(await contract.query("estimate_lockup_price", {
            origin: context.accounts.alice.address,
            data: { netuid, seller, listing_id: listingId, amount }
        }), "estimate_lockup_price");
    }

    describe("Happy Paths", () => {
        it("should successfully take a lockup listing and create escrow", async () => {
            const { accounts } = context;
            const listAmount = PURCHASE_UNIT;

            const listingId = await createBobListing(listAmount);

            // Estimate price
            const [, taoAmount, totalRequired] = await estimateListing(accounts.bob.address, listingId, listAmount);
            console.log(`Price estimate: taoAmount=${taoAmount}, totalRequired=${totalRequired}`);

            // Charlie takes the listing
            const { event: takenEvent, purchaseId, escrowAccount } = await takeLockupListing(contract, accounts.charlie.signer, accounts.charlie.address, {
                netuid,
                seller: accounts.bob.address,
                listing_id: listingId,
                amount: listAmount
            }, totalRequired + PURCHASE_PAYMENT_DRIFT_BUFFER);
            expect(takenEvent.value.seller).toBe(accounts.bob.address);
            expect(takenEvent.value.buyer).toBe(accounts.charlie.address);
            expect(BigInt(takenEvent.value.listing_id)).toBe(listingId);
            expect(escrowAccount).toBeDefined();
            console.log(`LockupListingTaken event - escrow: ${escrowAccount}`);

            // Verify escrow was created
            const escrow = queryOk<string | undefined>(await contract.query("get_escrow", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    listing_id: listingId,
                    purchase_id: purchaseId
                }
            }), "get_escrow");
            expect(escrow).toBe(escrowAccount);
            console.log(`Escrow created at: ${escrow}`);
        }, 180000);

        it("should pay seller and collect fee when taking listing", async () => {
            const { accounts } = context;
            const listAmount = PURCHASE_UNIT;

            const listingId = await createBobListing(listAmount);

            // Get balances before
            const sellerBalanceBefore = await getBalance(context.api, accounts.bob.address);
            const ownerBalanceBefore = await getBalance(context.api, accounts.alice.address);

            // Estimate price
            const [, , totalRequired] = await estimateListing(accounts.bob.address, listingId, listAmount);

            // Take listing
            const { event } = await takeLockupListing(contract, accounts.dave.signer, accounts.dave.address, {
                netuid,
                seller: accounts.bob.address,
                listing_id: listingId,
                amount: listAmount
            }, totalRequired + PURCHASE_PAYMENT_DRIFT_BUFFER);
            const executedTaoAmount = BigInt(event.value.tao_amount);
            const executedFeeAmount = BigInt(event.value.fee);
            expect(executedTaoAmount).toBeGreaterThan(0n);
            expect(executedFeeAmount).toBeGreaterThan(0n);

            // Get balances after
            const sellerBalanceAfter = await getBalance(context.api, accounts.bob.address);
            const ownerBalanceAfter = await getBalance(context.api, accounts.alice.address);

            // Verify seller received TAO (approximately taoAmount)
            const sellerIncrease = sellerBalanceAfter - sellerBalanceBefore;
            const tolerance = taoToRao(1);
            console.log(`Seller received: ${sellerIncrease}, expected: ${executedTaoAmount}`);
            expect(sellerIncrease).toBeGreaterThanOrEqual(executedTaoAmount - tolerance);
            expect(sellerIncrease).toBeLessThanOrEqual(executedTaoAmount + tolerance);

            // Verify owner received fee
            const ownerIncrease = ownerBalanceAfter - ownerBalanceBefore;
            console.log(`Owner received: ${ownerIncrease}, expected fee: ${executedFeeAmount}`);
            expect(ownerIncrease).toBeGreaterThanOrEqual(executedFeeAmount - tolerance);
        }, 180000);

        it("should update listing remaining amount on partial purchase", async () => {
            const { accounts } = context;
            const listAmount = TWO_PURCHASE_UNITS;
            const purchaseAmount = PURCHASE_UNIT;

            const listingId = await createBobListing(listAmount);

            // Estimate price for partial amount
            const [, , totalRequired] = await estimateListing(accounts.bob.address, listingId, purchaseAmount);

            // Take partial listing
            await takeLockupListing(contract, accounts.charlie.signer, accounts.charlie.address, {
                netuid,
                seller: accounts.bob.address,
                listing_id: listingId,
                amount: purchaseAmount
            }, totalRequired + PURCHASE_PAYMENT_DRIFT_BUFFER);

            // Verify listing still exists with reduced amount
            const listingAfter = await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId
                }
            });

            expect(listingAfter.success).toBe(true);
            if (listingAfter.success && listingAfter.value.response) {
                const expectedRemaining = listAmount - purchaseAmount;
                const actualRemaining = listingAfter.value.response.remaining_amount;
                expect(actualRemaining).toBe(expectedRemaining);
            }
        }, 180000);

        it("should allow multiple partial purchases from different buyers until fully filled", async () => {
            const { accounts } = context;
            const listAmount = THREE_PURCHASE_UNITS;
            const purchaseAmount = PURCHASE_UNIT;

            const listingId = await createBobListing(listAmount);

            // Store purchase info from events (purchase_id is a global counter)
            const purchases: Array<{ purchaseId: bigint; buyer: string }> = [];

            // --- First partial purchase by Charlie ---
            const [, , totalRequired1] = await estimateListing(accounts.bob.address, listingId, purchaseAmount);

            const purchase1 = await takeLockupListing(contract, accounts.charlie.signer, accounts.charlie.address, {
                netuid,
                seller: accounts.bob.address,
                listing_id: listingId,
                amount: purchaseAmount
            }, totalRequired1 + PURCHASE_PAYMENT_DRIFT_BUFFER);
            expect(purchase1.event.value.buyer).toBe(accounts.charlie.address);
            purchases.push({ purchaseId: purchase1.purchaseId, buyer: accounts.charlie.address });
            console.log(`Purchase 1: Charlie bought ${purchaseAmount} (purchase_id: ${purchase1.purchaseId})`);

            // Verify listing still exists with ~20 TAO remaining
            const listingAfter1 = await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId
                }
            });

            expect(listingAfter1.success).toBe(true);
            if (listingAfter1.success === true) {
                expect(listingAfter1.value.response).toBeDefined();
            }
            if (listingAfter1.success && listingAfter1.value.response) {
                const remaining1 = listingAfter1.value.response.remaining_amount;
                console.log(`After purchase 1, remaining: ${remaining1}`);
                expect(remaining1).toBe(TWO_PURCHASE_UNITS);
            }

            // --- Second partial purchase by Dave ---
            const [, , totalRequired2] = await estimateListing(accounts.bob.address, listingId, purchaseAmount);

            const purchase2 = await takeLockupListing(contract, accounts.dave.signer, accounts.dave.address, {
                netuid,
                seller: accounts.bob.address,
                listing_id: listingId,
                amount: purchaseAmount
            }, totalRequired2 + PURCHASE_PAYMENT_DRIFT_BUFFER);
            expect(purchase2.event.value.buyer).toBe(accounts.dave.address);
            purchases.push({ purchaseId: purchase2.purchaseId, buyer: accounts.dave.address });
            console.log(`Purchase 2: Dave bought ${purchaseAmount} (purchase_id: ${purchase2.purchaseId})`);

            // Verify listing still exists with ~10 TAO remaining
            const listingAfter2 = await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId
                }
            });

            expect(listingAfter2.success).toBe(true);
            if (listingAfter2.success === true) {
                expect(listingAfter2.value.response).toBeDefined();
            }
            if (listingAfter2.success && listingAfter2.value.response) {
                const remaining2 = listingAfter2.value.response.remaining_amount;
                console.log(`After purchase 2, remaining: ${remaining2}`);
                expect(remaining2).toBe(PURCHASE_UNIT);
            }

            // --- Third (final) partial purchase by Charlie to complete the listing ---
            const [, , totalRequired3] = await estimateListing(accounts.bob.address, listingId, purchaseAmount);

            const purchase3 = await takeLockupListing(contract, accounts.charlie.signer, accounts.charlie.address, {
                netuid,
                seller: accounts.bob.address,
                listing_id: listingId,
                amount: purchaseAmount
            }, totalRequired3 + PURCHASE_PAYMENT_DRIFT_BUFFER);
            expect(purchase3.event.value.buyer).toBe(accounts.charlie.address);
            purchases.push({ purchaseId: purchase3.purchaseId, buyer: accounts.charlie.address });
            console.log(`Purchase 3: Charlie bought ${purchaseAmount} (purchase_id: ${purchase3.purchaseId})`);

            const fullyFilledEvent = contract.filterEvents(purchase3.result.events).find(e => e.type === "LockupListingFullyFilled");
            expect(fullyFilledEvent).toBeDefined();
            if (fullyFilledEvent) {
                expect(fullyFilledEvent.value.seller).toBe(accounts.bob.address);
                expect(Number(fullyFilledEvent.value.listing_id)).toBe(Number(listingId));
                console.log(`LockupListingFullyFilled event emitted after 3 partial purchases`);
            }

            // Verify listing was removed
            const listingAfter3 = await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId
                }
            });

            expect(listingAfter3.success).toBe(true);
            if (listingAfter3.success) {
                expect(listingAfter3.value.response).toBeUndefined();
                console.log(`Listing ${listingId} was removed after being fully filled`);
            }

            // Verify all 3 escrows were created and have correct info
            expect(purchases.length).toBe(3);
            for (const purchase of purchases) {
                const escrowResult = await contract.query("get_escrow", {
                    origin: accounts.alice.address,
                    data: {
                        netuid,
                        listing_id: listingId,
                        purchase_id: purchase.purchaseId
                    }
                });

                expect(escrowResult.success).toBe(true);
                if (!escrowResult.success || !escrowResult.value.response) {
                    throw new Error(`Escrow not found for purchase_id ${purchase.purchaseId}`);
                }

                const escrowAddress = escrowResult.value.response;
                console.log(`Escrow for purchase_id ${purchase.purchaseId} exists at: ${escrowAddress}`);

                // Query escrow get_info and verify fields
                const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);
                const infoResult = await escrowContract.query("get_info", {
                    origin: accounts.alice.address,
                    data: {}
                });

                expect(infoResult.success).toBe(true);
                if (infoResult.success) {
                    const info = infoResult.value.response;
                    expect(info.buyer).toBe(purchase.buyer);
                    expect(info.netuid).toBe(netuid);
                    expect(info.claimed).toBe(false);
                    console.log(`  - buyer: ${info.buyer}, netuid: ${info.netuid}, claimed: ${info.claimed}, alpha_amount: ${info.alpha_amount}`);
                }
            }
        }, 300000);
    });

    describe("Error Cases", () => {
        it("should fail with AmountTooSmall when below minimum purchase", async () => {
            const { accounts } = context;
            const listAmount = PURCHASE_UNIT;

            const listingId = await createBobListing(listAmount);

            // Get minimum purchase amount
            const minAmount = queryOk<bigint>(await contract.query("get_min_purchase_amount", {
                origin: accounts.alice.address,
                data: {}
            }), "get_min_purchase_amount");
            const belowMin = minAmount - 1n;

            // Try to take below minimum
            const takeTx = contract.send("take_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: belowMin
                },
                value: taoToRao(10)
            });

            await submitReverted(takeTx, accounts.charlie.signer, "take_lockup_listing below minimum");
        }, 120000);

        it("should fail with ListingNotFound for invalid listing", async () => {
            const { accounts } = context;

            const takeTx = contract.send("take_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: 999999n,
                    amount: PURCHASE_UNIT
                },
                value: taoToRao(10)
            });

            await submitReverted(takeTx, accounts.charlie.signer, "take_lockup_listing invalid listing");
        }, 60000);

        it("should fail with AmountExceedsRemaining when taking more than available", async () => {
            const { accounts } = context;
            const listAmount = PURCHASE_UNIT;

            const listingId = await createBobListing(listAmount);

            // Try to take more than listing amount
            const takeTx = contract.send("take_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: listAmount + PURCHASE_UNIT // More than available
                },
                value: taoToRao(100)
            });

            await submitReverted(takeTx, accounts.charlie.signer, "take_lockup_listing amount exceeds remaining");
        }, 120000);

        it("should fail with InsufficientPayment when not enough TAO sent", async () => {
            const { accounts } = context;
            const listAmount = PURCHASE_UNIT;

            const listingId = await createBobListing(listAmount);

            // Estimate price
            const [, , totalRequired] = await estimateListing(accounts.bob.address, listingId, listAmount);

            // Send less than required
            const takeTx = contract.send("take_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: listAmount
                },
                value: totalRequired / 2n // Only half
            });

            await submitReverted(takeTx, accounts.charlie.signer, "take_lockup_listing insufficient payment");
        }, 120000);

        it("should fail when trading is paused", async () => {
            const { accounts } = context;

            const listingId = await createBobListing(PURCHASE_UNIT);

            // Pause trading
            const pauseTx = contract.send("pause_trading", {
                origin: accounts.alice.address,
                data: { reason: Binary.fromText("Test pause") }
            });
            await submitOk(pauseTx, accounts.alice.signer, "pause_trading");

            // Try to take
            const takeTx = contract.send("take_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: PURCHASE_UNIT
                },
                value: taoToRao(50)
            });

            await submitReverted(takeTx, accounts.charlie.signer, "take_lockup_listing while paused");

            // Resume
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await submitOk(resumeTx, accounts.alice.signer, "resume");
        }, 120000);
    });

    describe("Edge Cases", () => {
        it("should remove listing when fully filled", async () => {
            const { accounts } = context;
            const listAmount = PURCHASE_UNIT;

            const listingId = await createBobListing(listAmount);

            // Estimate for full amount
            const [, , totalRequired] = await estimateListing(accounts.bob.address, listingId, listAmount);

            // Take full listing
            const { result } = await takeLockupListing(contract, accounts.charlie.signer, accounts.charlie.address, {
                netuid,
                seller: accounts.bob.address,
                listing_id: listingId,
                amount: listAmount
            }, totalRequired + PURCHASE_PAYMENT_DRIFT_BUFFER);

            // The multi-partial-purchase test above asserts the fully-filled event.
            // This edge case focuses on storage removal after a single full fill.
            const events = contract.filterEvents(result.events);
            const fullyFilledEvent = events.find(e => e.type === "LockupListingFullyFilled");
            if (fullyFilledEvent) {
                expect(fullyFilledEvent.value.seller).toBe(accounts.bob.address);
                expect(Number(fullyFilledEvent.value.listing_id)).toBe(Number(listingId));
                console.log(`LockupListingFullyFilled event emitted for listing ${listingId}`);
            }

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
        }, 180000);
    });
});
