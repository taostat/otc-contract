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
    type Wallet,
} from "../utils";

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
        await registerValidator(context.api, netuid, bobHotkey.address, context.accounts.bob.signer, taoToRao(100));
        await registerValidator(context.api, netuid, charlieHotkey.address, context.accounts.charlie.signer, taoToRao(100));
        await registerValidator(context.api, netuid, daveHotkey.address, context.accounts.dave.signer, taoToRao(100));

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

    describe("Happy Paths", () => {
        it("should successfully take a lockup listing and create escrow", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(10);

            // Bob creates a listing
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

            expect(estimateResult.success).toBe(true);
            if (!estimateResult.success) {
                throw new Error("Failed to estimate price");
            }

            const [, taoAmount, totalRequired] = estimateResult.value.response;
            console.log(`Price estimate: taoAmount=${taoAmount}, totalRequired=${totalRequired}`);

            // Charlie takes the listing
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

            const result = await takeTx.signAndSubmit(accounts.charlie.signer);
            expect(result.ok).toBe(true);

            // Verify LockupListingTaken event
            const events = contract.filterEvents(result.events);
            const takenEvent = events.find(e => e.type === "LockupListingTaken");
            expect(takenEvent).toBeDefined();
            if (takenEvent) {
                expect(takenEvent.value.seller).toBe(accounts.bob.address);
                expect(takenEvent.value.buyer).toBe(accounts.charlie.address);
                expect(Number(takenEvent.value.listing_id)).toBe(Number(listingId));
                expect(takenEvent.value.escrow_account).toBeDefined();
                console.log(`LockupListingTaken event - escrow: ${takenEvent.value.escrow_account}`);
            }

            await waitForBlocks(context.api, 2);

            // Verify escrow was created
            const escrowResult = await contract.query("get_escrow", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    listing_id: listingId,
                    purchase_id: 1n
                }
            });

            expect(escrowResult.success).toBe(true);
            if (escrowResult.success) {
                expect(escrowResult.value.response).toBeDefined();
                console.log(`Escrow created at: ${escrowResult.value.response}`);
            }
        }, 180000);

        it("should pay seller and collect fee when taking listing", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(5);

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

            // Get balances before
            const sellerBalanceBefore = await getBalance(context.api, accounts.bob.address);
            const ownerBalanceBefore = await getBalance(context.api, accounts.alice.address);

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

            const [, taoAmount, totalRequired] = estimateResult.value.response;
            const feeAmount = totalRequired - taoAmount;

            // Take listing
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

            // Get balances after
            const sellerBalanceAfter = await getBalance(context.api, accounts.bob.address);
            const ownerBalanceAfter = await getBalance(context.api, accounts.alice.address);

            // Verify seller received TAO (approximately taoAmount)
            const sellerIncrease = sellerBalanceAfter - sellerBalanceBefore;
            const tolerance = taoToRao(1);
            console.log(`Seller received: ${sellerIncrease}, expected: ${taoAmount}`);
            expect(sellerIncrease).toBeGreaterThanOrEqual(taoAmount - tolerance);
            expect(sellerIncrease).toBeLessThanOrEqual(taoAmount + tolerance);

            // Verify owner received fee
            const ownerIncrease = ownerBalanceAfter - ownerBalanceBefore;
            console.log(`Owner received: ${ownerIncrease}, expected fee: ${feeAmount}`);
            expect(ownerIncrease).toBeGreaterThanOrEqual(feeAmount - tolerance);
        }, 180000);

        it("should update listing remaining amount on partial purchase", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(20);
            const purchaseAmount = taoToRao(10);

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

            // Estimate price for partial amount
            const estimateResult = await contract.query("estimate_lockup_price", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: purchaseAmount
                }
            });

            if (!estimateResult.success) {
                throw new Error("Failed to estimate price");
            }

            const [, , totalRequired] = estimateResult.value.response;

            // Take partial listing
            const takeTx = contract.send("take_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: purchaseAmount
                },
                value: totalRequired
            });

            await takeTx.signAndSubmit(accounts.charlie.signer);
            await waitForBlocks(context.api, 2);

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
                const tolerance = taoToRao(1);
                expect(actualRemaining).toBeGreaterThanOrEqual(expectedRemaining - tolerance);
                expect(actualRemaining).toBeLessThanOrEqual(expectedRemaining + tolerance);
            }
        }, 180000);

        it("should allow multiple partial purchases from different buyers until fully filled", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(30);
            const purchaseAmount = taoToRao(10);

            // Create a larger listing
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

            // Get listing ID
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

            // Store purchase info from events (purchase_id is a global counter)
            const purchases: Array<{ purchaseId: bigint; buyer: string }> = [];

            // --- First partial purchase by Charlie ---
            const estimate1 = await contract.query("estimate_lockup_price", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: purchaseAmount
                }
            });

            if (!estimate1.success) throw new Error("Failed to estimate price 1");
            const [, , totalRequired1] = estimate1.value.response;

            const take1Tx = contract.send("take_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: purchaseAmount
                },
                value: totalRequired1
            });

            const result1 = await take1Tx.signAndSubmit(accounts.charlie.signer);
            expect(result1.ok).toBe(true);

            // Verify first purchase event and capture purchase info
            const events1 = contract.filterEvents(result1.events);
            const takenEvent1 = events1.find(e => e.type === "LockupListingTaken");
            expect(takenEvent1).toBeDefined();
            expect(takenEvent1?.value.buyer).toBe(accounts.charlie.address);
            if (takenEvent1) {
                purchases.push({
                    purchaseId: takenEvent1.value.purchase_id,
                    buyer: accounts.charlie.address
                });
                console.log(`Purchase 1: Charlie bought ${purchaseAmount} (purchase_id: ${takenEvent1.value.purchase_id})`);
            }

            await waitForBlocks(context.api, 2);

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
                const tolerance = taoToRao(2);
                expect(remaining1).toBeGreaterThanOrEqual(taoToRao(20) - tolerance);
                expect(remaining1).toBeLessThanOrEqual(taoToRao(20) + tolerance);
            }

            // --- Second partial purchase by Dave ---
            const estimate2 = await contract.query("estimate_lockup_price", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: purchaseAmount
                }
            });

            if (!estimate2.success) throw new Error("Failed to estimate price 2");
            const [, , totalRequired2] = estimate2.value.response;

            const take2Tx = contract.send("take_lockup_listing", {
                origin: accounts.dave.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: purchaseAmount
                },
                value: totalRequired2
            });

            const result2 = await take2Tx.signAndSubmit(accounts.dave.signer);
            expect(result2.ok).toBe(true);

            // Verify second purchase event and capture purchase info
            const events2 = contract.filterEvents(result2.events);
            const takenEvent2 = events2.find(e => e.type === "LockupListingTaken");
            expect(takenEvent2).toBeDefined();
            expect(takenEvent2?.value.buyer).toBe(accounts.dave.address);
            if (takenEvent2) {
                purchases.push({
                    purchaseId: takenEvent2.value.purchase_id,
                    buyer: accounts.dave.address
                });
                console.log(`Purchase 2: Dave bought ${purchaseAmount} (purchase_id: ${takenEvent2.value.purchase_id})`);
            }

            await waitForBlocks(context.api, 2);

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
                const tolerance = taoToRao(2);
                expect(remaining2).toBeGreaterThanOrEqual(taoToRao(10) - tolerance);
                expect(remaining2).toBeLessThanOrEqual(taoToRao(10) + tolerance);
            }

            // --- Third (final) partial purchase by Charlie to complete the listing ---
            const estimate3 = await contract.query("estimate_lockup_price", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: purchaseAmount
                }
            });

            if (!estimate3.success) throw new Error("Failed to estimate price 3");
            const [, , totalRequired3] = estimate3.value.response;

            const take3Tx = contract.send("take_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: purchaseAmount
                },
                value: totalRequired3
            });

            const result3 = await take3Tx.signAndSubmit(accounts.charlie.signer);
            expect(result3.ok).toBe(true);

            // Verify third purchase event, capture purchase info, and check fully filled event
            const events3 = contract.filterEvents(result3.events);
            const takenEvent3 = events3.find(e => e.type === "LockupListingTaken");
            expect(takenEvent3).toBeDefined();
            expect(takenEvent3?.value.buyer).toBe(accounts.charlie.address);
            if (takenEvent3) {
                purchases.push({
                    purchaseId: takenEvent3.value.purchase_id,
                    buyer: accounts.charlie.address
                });
                console.log(`Purchase 3: Charlie bought ${purchaseAmount} (purchase_id: ${takenEvent3.value.purchase_id})`);
            }

            const fullyFilledEvent = events3.find(e => e.type === "LockupListingFullyFilled");
            expect(fullyFilledEvent).toBeDefined();
            if (fullyFilledEvent) {
                expect(fullyFilledEvent.value.seller).toBe(accounts.bob.address);
                expect(Number(fullyFilledEvent.value.listing_id)).toBe(Number(listingId));
                console.log(`LockupListingFullyFilled event emitted after 3 partial purchases`);
            }

            await waitForBlocks(context.api, 2);

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
            const listAmount = taoToRao(10);

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

            // Get minimum purchase amount
            const minAmountResult = await contract.query("get_min_purchase_amount", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(minAmountResult.success).toBe(true);
            if (!minAmountResult.success) return;

            const minAmount = minAmountResult.value.response;
            const belowMin = minAmount - 1n;

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

            const result = await takeTx.signAndSubmit(accounts.charlie.signer);
            expect(result.ok).toBe(false);
        }, 120000);

        it("should fail with ListingNotFound for invalid listing", async () => {
            const { accounts } = context;

            const takeTx = contract.send("take_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: 999999n,
                    amount: taoToRao(5)
                },
                value: taoToRao(10)
            });

            const result = await takeTx.signAndSubmit(accounts.charlie.signer);
            expect(result.ok).toBe(false);
        }, 60000);

        it("should fail with AmountExceedsRemaining when taking more than available", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(5);

            // Create small listing
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

            // Try to take more than listing amount
            const takeTx = contract.send("take_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: listAmount + taoToRao(10) // More than available
                },
                value: taoToRao(100)
            });

            const result = await takeTx.signAndSubmit(accounts.charlie.signer);
            expect(result.ok).toBe(false);
        }, 120000);

        it("should fail with InsufficientPayment when not enough TAO sent", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(5);

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

            const result = await takeTx.signAndSubmit(accounts.charlie.signer);
            expect(result.ok).toBe(false);
        }, 120000);

        it("should fail when trading is paused", async () => {
            const { accounts } = context;

            // Create listing first
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

            // Pause trading
            const pauseTx = contract.send("pause_trading", {
                origin: accounts.alice.address,
                data: { reason: Binary.fromText("Test pause") }
            });
            await pauseTx.signAndSubmit(accounts.alice.signer);

            // Try to take
            const takeTx = contract.send("take_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId,
                    amount: taoToRao(5)
                },
                value: taoToRao(50)
            });

            const result = await takeTx.signAndSubmit(accounts.charlie.signer);
            expect(result.ok).toBe(false);

            // Resume
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await resumeTx.signAndSubmit(accounts.alice.signer);
        }, 120000);
    });

    describe("Edge Cases", () => {
        it("should remove listing when fully filled", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(5);

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

            // Estimate for full amount
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

            // Take full listing
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

            const result = await takeTx.signAndSubmit(accounts.charlie.signer);
            expect(result.ok).toBe(true);

            // Verify LockupListingFullyFilled event
            const events = contract.filterEvents(result.events);
            const fullyFilledEvent = events.find(e => e.type === "LockupListingFullyFilled");
            expect(fullyFilledEvent).toBeDefined();
            if (fullyFilledEvent) {
                expect(fullyFilledEvent.value.seller).toBe(accounts.bob.address);
                expect(Number(fullyFilledEvent.value.listing_id)).toBe(Number(listingId));
                console.log(`LockupListingFullyFilled event emitted for listing ${listingId}`);
            }

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
        }, 180000);
    });
});
