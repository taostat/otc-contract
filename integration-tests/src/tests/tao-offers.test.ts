import { describe, it, expect, beforeAll, afterAll, beforeEach } from "vitest";
import { setupTestEnvironment, cleanupTestEnvironment, type TestContext, ContractSdk, bigintReplacer } from "../setup";
import {
    percentageToFixedPoint,
    taoToRao,
    raoToTao,
    waitForBlocks,
    fundAccount,
    registerSubnet,
    registerValidator,
    addContractAsProxy,
    createHotkey,
    getBalance,
    calculateAlphaForOffer,
    type Wallet,
    elevateRegistrationLimits,
    MARKET_PRICE,
    INVALID_OFFSET,
    bpsToPercentage,
    getExecutedPriceFixed,
} from "../utils";
import {
    getStakeBalance,
    hasProxyPermission,
    formatStakeAmount,
} from "../utils/stake-helpers";
import {
    createTaoOffer,
    expectEvent,
    queryOk,
    submitOk,
    submitReverted,
} from "../test-helpers";

describe("TAO Offer Operations", () => {
    let context: TestContext;
    let contract: ReturnType<ContractSdk["getContract"]>;
    let netuid: number;
    let contractFeeRate: bigint;

    // Hotkeys for validators
    let bobHotkey: Wallet;
    let charlieHotkey: Wallet;
    let daveHotkey: Wallet;
    let eveHotkey: Wallet;

    beforeAll(async () => {
        context = await setupTestEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);

        // Get contract fee rate for calculations
        const feeRateResult = await contract.query("get_fee_rate", {
            origin: context.accounts.alice.address,
            data: {}
        });
        contractFeeRate = feeRateResult.success ?
            feeRateResult.value.response : percentageToFixedPoint(0.5);

        // Create a subnet for testing
        const aliceHotkey = createHotkey();
        await fundAccount(context.api, aliceHotkey.address, taoToRao(10), context.accounts.alice.signer);
        netuid = await registerSubnet(context.api, aliceHotkey.address, context.accounts.alice.signer);
        console.log(`Created test subnet with netuid: ${netuid}`);

        // Elevate registration limits so we can register multiple validators quickly
        await elevateRegistrationLimits(
            context.api,
            netuid,
            20, // target registrations per interval
            20, // max registrations per block
            context.accounts.alice.signer // sudo signer (alice)
        );
        // Small wait to ensure params applied in subsequent block
        await waitForBlocks(context.api, 1);


        // Get the contract's hotkey and register it as a validator
        const contractHotkeyResult = await contract.query("get_hotkey", {
            origin: context.accounts.alice.address,
            data: {}
        });

        if (contractHotkeyResult.success) {
            const contractHotkey = contractHotkeyResult.value.response;
            console.log(`Contract hotkey: ${contractHotkey}`);

            // Fund the contract's account for transaction fees
            await fundAccount(context.api, context.contractAddress!, taoToRao(10), context.accounts.alice.signer);

            // Register the contract's hotkey as a validator
            await registerValidator(
                context.api,
                netuid,
                contractHotkey,
                context.accounts.alice.signer,
                taoToRao(50) // Initial stake for the contract's hotkey
            );
            console.log(`Registered contract's hotkey as validator with 50 Alpha stake`);
        }

        // Create hotkeys for validators
        bobHotkey = createHotkey();
        charlieHotkey = createHotkey();
        daveHotkey = createHotkey();
        eveHotkey = createHotkey();

        // Fund hotkeys for transaction fees
        await fundAccount(context.api, bobHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, charlieHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, daveHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, eveHotkey.address, taoToRao(1), context.accounts.alice.signer);

        // Register validators with initial stake
        await registerValidator(
            context.api,
            netuid,
            bobHotkey.address,
            context.accounts.bob.signer,
            taoToRao(5000)
        );
        console.log(`✓ Bob's validator registered with 150 Alpha`);

        await registerValidator(
            context.api,
            netuid,
            charlieHotkey.address,
            context.accounts.charlie.signer,
            taoToRao(5000)
        );
        console.log(`✓ Charlie's validator registered with 200 Alpha`);

        await registerValidator(
            context.api,
            netuid,
            daveHotkey.address,
            context.accounts.dave.signer,
            taoToRao(5000)
        );
        console.log(`✓ Dave's validator registered with 100 Alpha`);

        await registerValidator(
            context.api,
            netuid,
            eveHotkey.address,
            context.accounts.eve.signer,
            taoToRao(30) // 30 Alpha initial stake for Eve (for insufficient stake test)
        );
        console.log(`✓ Eve's validator registered with 30 Alpha`);

        // Fund test accounts with TAO for creating offers
        await fundAccount(context.api, context.accounts.bob.address, taoToRao(300), context.accounts.alice.signer);
        await fundAccount(context.api, context.accounts.charlie.address, taoToRao(500), context.accounts.alice.signer);
        await fundAccount(context.api, context.accounts.dave.address, taoToRao(300), context.accounts.alice.signer);
        await fundAccount(context.api, context.accounts.eve.address, taoToRao(200), context.accounts.alice.signer);

        // Wait for all registrations and funds to be confirmed
        await waitForBlocks(context.api, 3);

        // Verify initial stakes
        const bobStake = await getStakeBalance(context.api, bobHotkey.address, netuid, context.accounts.bob.address);
        const charlieStake = await getStakeBalance(context.api, charlieHotkey.address, netuid, context.accounts.charlie.address);
        const daveStake = await getStakeBalance(context.api, daveHotkey.address, netuid, context.accounts.dave.address);
        const eveStake = await getStakeBalance(context.api, eveHotkey.address, netuid, context.accounts.eve.address);

        console.log(`\nInitial Stakes:`);
        console.log(`Bob's stake: ${formatStakeAmount(bobStake)}`);
        console.log(`Charlie's stake: ${formatStakeAmount(charlieStake)}`);
        console.log(`Dave's stake: ${formatStakeAmount(daveStake)}`);
        console.log(`Eve's stake: ${formatStakeAmount(eveStake)}`);

        // Verify TAO balances
        const bobBalance = await getBalance(context.api, context.accounts.bob.address);
        const charlieBalance = await getBalance(context.api, context.accounts.charlie.address);
        const daveBalance = await getBalance(context.api, context.accounts.dave.address);
        const eveBalance = await getBalance(context.api, context.accounts.eve.address);

        console.log(`\nInitial TAO Balances:`);
        console.log(`Bob's balance: ${raoToTao(bobBalance)} TAO`);
        console.log(`Charlie's balance: ${raoToTao(charlieBalance)} TAO`);
        console.log(`Dave's balance: ${raoToTao(daveBalance)} TAO`);
        console.log(`Eve's balance: ${raoToTao(eveBalance)} TAO`);
    }, 180000); // 3 minute timeout for setup

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    describe("Offer Creation", () => {
        it("should successfully create a TAO offer", async () => {
            const { accounts } = context;

            // Get Charlie's balance before creating offer
            const balanceBefore = await getBalance(context.api, accounts.charlie.address);
            console.log(`Charlie's balance before offer: ${raoToTao(balanceBefore)} TAO`);

            // Charlie creates an offer to buy Alpha at market price
            const offerAmount = taoToRao(150); // Total TAO to spend
            const priceOffsetBps = MARKET_PRICE; // 0% offset from market

            const { event, offerId } = await createTaoOffer(contract, accounts.charlie.signer, accounts.charlie.address, {
                netuid,
                price_offset_bps: priceOffsetBps,
            }, offerAmount);
            expect(event.value.buyer).toBe(accounts.charlie.address);
            expect(event.value.netuid).toBe(netuid);
            expect(BigInt(event.value.tao_offer_id)).toBe(offerId);
            expect(BigInt(event.value.amount)).toBe(offerAmount);
            expect(event.value.price_offset_bps).toBe(priceOffsetBps);

            // Get Charlie's balance after creating offer
            const balanceAfter = await getBalance(context.api, accounts.charlie.address);
            console.log(`Charlie's balance after offer: ${raoToTao(balanceAfter)} TAO`);

            // Verify TAO was transferred (balance reduced by offer amount + fees)
            expect(balanceAfter).toBeLessThan(balanceBefore - offerAmount);

            // Query Charlie's offers
            const offersResult = await contract.query("get_user_offers", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.charlie.address,
                    netuid
                }
            });

            const userOffers = queryOk<bigint[]>(offersResult, "get_user_offers");
            expect(userOffers).toContain(offerId);

            const offer = queryOk<any>(await contract.query("get_offer", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    buyer: accounts.charlie.address,
                    offer_id: offerId
                }
            }), "get_offer");

            expect(offer).toBeDefined();
            expect(offer.amount).toBe(offerAmount);
            expect(offer.price_offset_bps).toBe(priceOffsetBps);
            expect(offer.buyer).toBe(accounts.charlie.address);
            expect(offer.netuid).toBe(netuid);

            console.log(`Created offer ${offerId}: ${raoToTao(offerAmount)} TAO at ${bpsToPercentage(priceOffsetBps)}% offset from market`);
        });

        it("should create multiple offers from the same buyer", async () => {
            const { accounts } = context;

            // Dave creates multiple offers with different price offsets
            const offers = [
                { amount: taoToRao(50), priceOffsetBps: MARKET_PRICE },     // Market price
                { amount: taoToRao(75), priceOffsetBps: 500 },              // +5% above market
                { amount: taoToRao(100), priceOffsetBps: -500 }             // -5% below market
            ];
            const offerIds: bigint[] = [];

            for (const offer of offers) {
                const { offerId, event } = await createTaoOffer(contract, accounts.dave.signer, accounts.dave.address, {
                    netuid,
                    price_offset_bps: offer.priceOffsetBps,
                }, offer.amount);
                expect(BigInt(event.value.amount)).toBe(offer.amount);
                expect(event.value.price_offset_bps).toBe(offer.priceOffsetBps);
                offerIds.push(offerId);
            }

            // Query Dave's offers
            const offersResult = await contract.query("get_user_offers", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.dave.address,
                    netuid
                }
            });

            const userOfferIds = queryOk<bigint[]>(offersResult, "get_user_offers");
            for (const offerId of offerIds) {
                expect(userOfferIds).toContain(offerId);
                const offer = queryOk<any>(await contract.query("get_offer", {
                    origin: accounts.alice.address,
                    data: {
                        netuid,
                        buyer: accounts.dave.address,
                        offer_id: offerId
                    }
                }), "get_offer");

                expect(offer.buyer).toBe(accounts.dave.address);
                expect(offer.netuid).toBe(netuid);
            }

            console.log(`Dave created ${offerIds.length} offers: ${offerIds.join(", ")}`);
        });

        it("should reject offer with amount below minimum", async () => {
            const { accounts } = context;

            // Get minimum offer amount
            const minAmountResult = await contract.query("get_min_offer_amount", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(minAmountResult.success).toBe(true);
            const minAmount = minAmountResult.success ?
                minAmountResult.value.response : taoToRao(1);

            // Try to create offer below minimum - should fail at contract level
            const offerTx = contract.send("create_tao_offer", {
                origin: accounts.eve.address,
                value: minAmount - 1n, // Below minimum
                data: {
                    netuid,
                    price_offset_bps: MARKET_PRICE
                }
            });

            await submitReverted(offerTx, accounts.eve.signer, "create_tao_offer below minimum");
        });

        it("should reject offer with invalid price offset", async () => {
            const { accounts } = context;

            // Try to create offer with -100% offset (invalid) - should fail at contract level
            const offerTx = contract.send("create_tao_offer", {
                origin: accounts.eve.address,
                value: taoToRao(10),
                data: {
                    netuid,
                    price_offset_bps: INVALID_OFFSET // -10000 = -100% (invalid)
                }
            });

            await submitReverted(offerTx, accounts.eve.signer, "create_tao_offer invalid price offset");
        });
    });

    describe("Offer Cancellation", () => {
        let testOfferId: bigint;
        let offerAmount: bigint;

        beforeEach(async () => {
            const { accounts } = context;

            // Create a test offer from Eve
            offerAmount = taoToRao(25);
            const { offerId } = await createTaoOffer(contract, accounts.eve.signer, accounts.eve.address, {
                netuid,
                price_offset_bps: MARKET_PRICE,
            }, offerAmount);
            testOfferId = offerId;
        });

        it("should successfully cancel offer and return TAO", async () => {
            const { accounts } = context;

            // Get Eve's balance before cancellation
            const balanceBefore = await getBalance(context.api, accounts.eve.address);
            console.log(`Eve's balance before cancellation: ${raoToTao(balanceBefore)} TAO`);

            // Cancel the offer
            const cancelTx = contract.send("cancel_tao_offer", {
                origin: accounts.eve.address,
                data: {
                    netuid,
                    offer_id: testOfferId
                }
            });

            const result = await submitOk(cancelTx, accounts.eve.signer, "cancel_tao_offer");
            const event = expectEvent(contract, result, "TaoOfferCancelled");
            expect(BigInt(event.value.offer_id)).toBe(testOfferId);
            expect(BigInt(event.value.amount_returned)).toBe(offerAmount);

            // Get Eve's balance after cancellation
            const balanceAfter = await getBalance(context.api, accounts.eve.address);
            console.log(`Eve's balance after cancellation: ${raoToTao(balanceAfter)} TAO`);

            // Verify TAO was returned (balance should increase by close to offer amount)
            const refunded = balanceAfter - balanceBefore;
            console.log(`Refunded amount: ${raoToTao(refunded)} TAO`);

            // Should get back most of the TAO (minus gas fees)
            expect(refunded).toBeGreaterThan(offerAmount - taoToRao(0.1)); // Allow 0.1 TAO for fees

            // Verify offer was removed
            const offerResult = await contract.query("get_offer", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    buyer: accounts.eve.address,
                    offer_id: testOfferId
                }
            });

            expect(offerResult.success).toBe(true);
            if (offerResult.success) {
                expect(offerResult.value.response).toBeUndefined(); // Should be None
            }

            // Verify offer is removed from user's list
            const offersResult = await contract.query("get_user_offers", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.eve.address,
                    netuid
                }
            });

            expect(offersResult.success).toBe(true);
            if (offersResult.success) {
                expect(offersResult.value.response).not.toContain(testOfferId);
            }
        });

        it("should reject cancellation by non-owner", async () => {
            const { accounts } = context;

            // Charlie tries to cancel Eve's offer
            const cancelTx = contract.send("cancel_tao_offer", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    offer_id: testOfferId
                }
            });

            await submitReverted(cancelTx, accounts.charlie.signer, "cancel_tao_offer non-owner");

            // Verify offer still exists
            const offerResult = await contract.query("get_offer", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    buyer: accounts.eve.address,
                    offer_id: testOfferId
                }
            });

            expect(offerResult.success).toBe(true);
            if (offerResult.success) {
                expect(offerResult.value.response).toBeDefined();
            }
        });

        it("should reject cancellation of non-existent offer", async () => {
            const { accounts } = context;

            const cancelTx = contract.send("cancel_tao_offer", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    offer_id: 999999n // Non-existent offer
                }
            });

            await submitReverted(cancelTx, accounts.charlie.signer, "cancel_tao_offer missing offer");
        });
    });

    describe("Offer Queries", () => {
        it("should correctly query offers across multiple users", async () => {
            const { accounts } = context;

            // Create offers from different users with different price offsets
            const users = [
                { account: accounts.charlie, amount: taoToRao(10), priceOffsetBps: MARKET_PRICE },
                { account: accounts.dave, amount: taoToRao(20), priceOffsetBps: 200 },    // +2%
                { account: accounts.eve, amount: taoToRao(15), priceOffsetBps: -300 }     // -3%
            ];
            const createdOffers: Array<typeof users[number] & { offerId: bigint }> = [];

            for (const user of users) {
                const { offerId } = await createTaoOffer(contract, user.account.signer, user.account.address, {
                    netuid,
                    price_offset_bps: user.priceOffsetBps,
                }, user.amount);
                createdOffers.push({ ...user, offerId });
            }

            // Query each user's offers and verify the ID created by this test.
            for (const user of createdOffers) {
                const offersResult = await contract.query("get_user_offers", {
                    origin: accounts.alice.address,
                    data: {
                        buyer: user.account.address,
                        netuid
                    }
                });

                const userOfferIds = queryOk<bigint[]>(offersResult, "get_user_offers");
                expect(userOfferIds).toContain(user.offerId);
                console.log(`User ${user.account.address.slice(0, 8)}... includes offer ${user.offerId}`);
            }
        });

        it("should return empty array for users with no offers", async () => {
            const { accounts } = context;

            const unusedBuyer = createHotkey().address;
            const offersResult = await contract.query("get_user_offers", {
                origin: accounts.alice.address,
                data: {
                    buyer: unusedBuyer,
                    netuid
                }
            });

            expect(queryOk<bigint[]>(offersResult, "get_user_offers")).toEqual([]);
        });

        it("should correctly retrieve offer details", async () => {
            const { accounts } = context;

            // Create a specific offer to test
            const offerAmount = taoToRao(42);
            const priceOffsetBps = 1000; // +10% above market

            const { offerId } = await createTaoOffer(contract, accounts.charlie.signer, accounts.charlie.address, {
                netuid,
                price_offset_bps: priceOffsetBps,
            }, offerAmount);

            const offer = queryOk<any>(await contract.query("get_offer", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    buyer: accounts.charlie.address,
                    offer_id: offerId
                }
            }), "get_offer");

            expect(offer).toBeDefined();
            expect(offer.id).toBe(offerId);
            expect(offer.buyer).toBe(accounts.charlie.address);
            expect(offer.netuid).toBe(netuid);
            expect(offer.amount).toBe(offerAmount);
            expect(offer.price_offset_bps).toBe(priceOffsetBps);

            // Verify fee rate matches contract configuration
            const feeRateResult = await contract.query("get_fee_rate", {
                origin: accounts.alice.address,
                data: {}
            });

            if (feeRateResult.success) {
                expect(offer.fee_rate).toBe(feeRateResult.value.response);
            }

            console.log(`Offer ${offerId}: ${raoToTao(offer.amount)} TAO at ${bpsToPercentage(offer.price_offset_bps)}% offset from market`);
        });
    });

    describe("Taking TAO Offers", () => {
        it("should successfully take a TAO offer by providing Alpha", async () => {
            const { accounts } = context;

            // === Setup: Charlie creates offer to buy Alpha at market price ===
            const offerTaoAmount = taoToRao(90); // Total TAO to spend
            const priceOffsetBps = MARKET_PRICE; // 0% offset from market

            const { offerId } = await createTaoOffer(contract, accounts.charlie.signer, accounts.charlie.address, {
                netuid,
                price_offset_bps: priceOffsetBps,
            }, offerTaoAmount);

            // === Fetch market price and calculate actual price with offset ===
            const executedPriceFixed = await getExecutedPriceFixed(context.api, netuid, priceOffsetBps);

            // === Calculate Alpha amount needed ===
            const { alphaAmount, feeAmount, taoForSeller } = calculateAlphaForOffer(
                offerTaoAmount,
                executedPriceFixed,
                contractFeeRate
            );

            console.log(`\nOffer: ${raoToTao(offerTaoAmount)} TAO at ${bpsToPercentage(priceOffsetBps)}% offset from market`);
            console.log(`Alpha needed: ${formatStakeAmount(alphaAmount)}`);
            console.log(`Fee amount: ${raoToTao(feeAmount)} TAO`);
            console.log(`TAO for seller: ${raoToTao(taoForSeller)} TAO`);

            // === Bob takes the offer (provides Alpha, receives TAO) ===
            // Setup proxy for Bob to transfer Alpha
            await addContractAsProxy(context.api, context.contractAddress!, context.accounts.bob.signer);
            await waitForBlocks(context.api, 2);

            // Verify proxy was added
            const hasProxy = await hasProxyPermission(
                context.api,
                accounts.bob.address,
                context.contractAddress!
            );
            expect(hasProxy).toBe(true);

            // Get balances before trade
            const bobTaoBefore = await getBalance(context.api, accounts.bob.address);
            const bobAlphaBefore = await getStakeBalance(context.api, bobHotkey.address, netuid, accounts.bob.address);
            const charlieAlphaBefore = await getStakeBalance(context.api, charlieHotkey.address, netuid, accounts.charlie.address);
            const ownerTaoBefore = await getBalance(context.api, accounts.alice.address);

            console.log(`\nBefore trade:`);
            console.log(`Bob TAO: ${raoToTao(bobTaoBefore)}`);
            console.log(`Bob Alpha: ${formatStakeAmount(bobAlphaBefore)}`);
            console.log(`Charlie Alpha: ${formatStakeAmount(charlieAlphaBefore)}`);

            // Verify Bob has enough Alpha
            expect(bobAlphaBefore).toBeGreaterThanOrEqual(alphaAmount);

            // Take the offer
            const takeTx = contract.send("take_tao_offer", {
                origin: accounts.bob.address,
                data: {
                    netuid,
                    buyer: accounts.charlie.address,
                    offer_id: offerId,
                    hotkey: bobHotkey.address
                }
            });

            const result = await submitOk(takeTx, accounts.bob.signer, "take_tao_offer");

            console.log("Take TAO offer result:", {
                ok: result.ok,
                dispatchError: result.dispatchError,
            });

            // === Verify the TaoOfferTaken event ===
            const events = contract.filterEvents(result.events);
            console.log("Events emitted during transaction:", JSON.stringify(events, bigintReplacer, 2));

            const taoOfferTakenEvent = expectEvent(contract, result, "TaoOfferTaken");

            if (taoOfferTakenEvent) {
                // Verify the event contains expected values
                expect(BigInt(taoOfferTakenEvent.value.alpha_amount)).toBe(alphaAmount);
                expect(BigInt(taoOfferTakenEvent.value.tao_amount)).toBe(offerTaoAmount);
                expect(taoOfferTakenEvent.value.seller).toBe(accounts.bob.address);
                expect(taoOfferTakenEvent.value.buyer).toBe(accounts.charlie.address);
                expect(taoOfferTakenEvent.value.netuid).toBe(netuid);
                expect(BigInt(taoOfferTakenEvent.value.fee)).toBe(feeAmount);
                expect(BigInt(taoOfferTakenEvent.value.tao_offer_id || 0)).toBe(offerId);

                console.log(`\nTaoOfferTaken event verified:`);
                console.log(`Alpha transferred: ${formatStakeAmount(BigInt(taoOfferTakenEvent.value.alpha_amount))}`);
                console.log(`TAO amount: ${raoToTao(BigInt(taoOfferTakenEvent.value.tao_amount))}`);
                console.log(`Fee: ${raoToTao(BigInt(taoOfferTakenEvent.value.fee))}`);
            }

            await waitForBlocks(context.api, 3);

            // === Verify TAO balances (these are deterministic) ===
            // Check TAO balances after trade
            const bobTaoAfter = await getBalance(context.api, accounts.bob.address);
            const ownerTaoAfter = await getBalance(context.api, accounts.alice.address);

            // Bob should receive TAO (minus fee)
            const bobTaoReceived = bobTaoAfter - bobTaoBefore;
            console.log(`Bob TAO received: ${raoToTao(bobTaoReceived)}`);
            expect(bobTaoReceived).toBeGreaterThanOrEqual(taoForSeller - taoToRao(0.1)); // Allow variance for gas

            // Owner should receive fee
            const ownerFeeReceived = ownerTaoAfter - ownerTaoBefore;
            console.log(`Owner fee received: ${raoToTao(ownerFeeReceived)}`);
            expect(ownerFeeReceived).toBeGreaterThanOrEqual(feeAmount - taoToRao(0.01));

            // Verify offer was removed
            const offerResult = await contract.query("get_offer", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    buyer: accounts.charlie.address,
                    offer_id: offerId
                }
            });

            expect(offerResult.success).toBe(true);
            if (offerResult.success) {
                expect(offerResult.value.response).toBeUndefined(); // Should be None
            }
        });

        it("should reject taking offer without proxy setup", async () => {
            const { accounts } = context;

            // Create an offer from Dave
            const { offerId } = await createTaoOffer(contract, accounts.dave.signer, accounts.dave.address, {
                netuid,
                price_offset_bps: MARKET_PRICE,
            }, taoToRao(50));

            // Charlie tries to take without proxy (check if already has proxy first)
            const hasProxy = await hasProxyPermission(
                context.api,
                accounts.charlie.address,
                context.contractAddress!
            );

            if (!hasProxy) {
                const takeTx = contract.send("take_tao_offer", {
                    origin: accounts.charlie.address,
                    data: {
                        netuid,
                        buyer: accounts.dave.address,
                        offer_id: offerId,
                        hotkey: charlieHotkey.address
                    }
                });

                await submitReverted(takeTx, accounts.charlie.signer, "take_tao_offer without proxy");
            } else {
                console.log("Charlie already has proxy from previous test run, skipping test");
            }
        });

        it("should reject taking offer with insufficient Alpha stake", async () => {
            const { accounts } = context;

            // Create a large offer that Eve can't fulfill
            const largePriceOffsetBps = MARKET_PRICE;
            const { offerId } = await createTaoOffer(contract, accounts.charlie.signer, accounts.charlie.address, {
                netuid,
                price_offset_bps: largePriceOffsetBps,
            }, taoToRao(100));

            // Calculate required Alpha using executed price
            const executedPriceFixed = await getExecutedPriceFixed(context.api, netuid, largePriceOffsetBps);
            const { alphaAmount } = calculateAlphaForOffer(
                taoToRao(100),
                executedPriceFixed,
                contractFeeRate
            );

            // Check Eve's Alpha balance
            const eveAlpha = await getStakeBalance(context.api, eveHotkey.address, netuid, accounts.eve.address);
            console.log(`Required Alpha: ${formatStakeAmount(alphaAmount)}, Eve has: ${formatStakeAmount(eveAlpha)}`);

            // Eve should have insufficient Alpha (30 Alpha from setup)
            expect(eveAlpha).toBeLessThan(alphaAmount);

            // Setup proxy for Eve if not already done
            const hasProxy = await hasProxyPermission(
                context.api,
                accounts.eve.address,
                context.contractAddress!
            );

            if (!hasProxy) {
                await addContractAsProxy(context.api, context.contractAddress!, accounts.eve.signer);
                await waitForBlocks(context.api, 2);
            }

            // Try to take the offer
            const takeTx = contract.send("take_tao_offer", {
                origin: accounts.eve.address,
                data: {
                    netuid,
                    buyer: accounts.charlie.address,
                    offer_id: offerId,
                    hotkey: eveHotkey.address
                }
            });

            await submitReverted(takeTx, accounts.eve.signer, "take_tao_offer insufficient stake");
        });

        it("should reject taking non-existent offer", async () => {
            const { accounts } = context;

            // Setup proxy for Bob if not already done
            const hasProxy = await hasProxyPermission(
                context.api,
                accounts.bob.address,
                context.contractAddress!
            );

            if (!hasProxy) {
                await addContractAsProxy(context.api, context.contractAddress!, accounts.bob.signer);
                await waitForBlocks(context.api, 2);
            }

            const takeTx = contract.send("take_tao_offer", {
                origin: accounts.bob.address,
                data: {
                    netuid,
                    buyer: accounts.dave.address,
                    offer_id: 888888n, // Non-existent
                    hotkey: bobHotkey.address
                }
            });

            await submitReverted(takeTx, accounts.bob.signer, "take_tao_offer missing offer");
        });

        it("should verify stake transfer with tolerance", async () => {
            const { accounts } = context;

            // Create a TAO offer with a specific amount that might trigger rounding
            const offerTaoAmount = taoToRao(75.5); // Intentional decimal for potential rounding
            const priceOffsetBps = 250; // +2.5% above market

            const { offerId } = await createTaoOffer(contract, accounts.dave.signer, accounts.dave.address, {
                netuid,
                price_offset_bps: priceOffsetBps,
            }, offerTaoAmount);

            // Calculate expected Alpha amount using executed price
            const executedPriceFixed = await getExecutedPriceFixed(context.api, netuid, priceOffsetBps);
            const { alphaAmount } = calculateAlphaForOffer(
                offerTaoAmount,
                executedPriceFixed,
                contractFeeRate
            );

            console.log(`\nTolerance test - Offer: ${raoToTao(offerTaoAmount)} TAO`);
            console.log(`Expected Alpha transfer: ${formatStakeAmount(alphaAmount)}`);

            // Charlie takes the offer
            const hasProxy = await hasProxyPermission(
                context.api,
                accounts.charlie.address,
                context.contractAddress!
            );

            if (!hasProxy) {
                await addContractAsProxy(context.api, context.contractAddress!, accounts.charlie.signer);
                await waitForBlocks(context.api, 2);
            }

            // Take the offer
            const takeTx = contract.send("take_tao_offer", {
                origin: accounts.charlie.address,
                data: {
                    netuid,
                    buyer: accounts.dave.address,
                    offer_id: offerId,
                    hotkey: charlieHotkey.address
                }
            });

            const result = await submitOk(takeTx, accounts.charlie.signer, "take_tao_offer tolerance");

            // === Verify the TaoOfferTaken event for tolerance ===
            const taoOfferTakenEvent = expectEvent(contract, result, "TaoOfferTaken");

            if (taoOfferTakenEvent) {
                const actualAlphaAmount = BigInt(taoOfferTakenEvent.value.alpha_amount);
                const TRANSFER_TOLERANCE = 10n; // From contract

                console.log(`\nTolerance test - Event verification:`);
                console.log(`Expected Alpha transfer: ${formatStakeAmount(alphaAmount)}`);
                console.log(`Actual Alpha transfer (from event): ${formatStakeAmount(actualAlphaAmount)}`);
                console.log(`Difference: ${actualAlphaAmount - alphaAmount} rao`);
                console.log(`Tolerance: ${TRANSFER_TOLERANCE} rao`);

                // Verify the actual transfer is within tolerance of expected
                const difference = actualAlphaAmount > alphaAmount ?
                    actualAlphaAmount - alphaAmount :
                    alphaAmount - actualAlphaAmount;

                expect(difference).toBeLessThanOrEqual(TRANSFER_TOLERANCE);

                // Also verify other event fields
                expect(taoOfferTakenEvent.value.seller).toBe(accounts.charlie.address);
                expect(taoOfferTakenEvent.value.buyer).toBe(accounts.dave.address);
                expect(taoOfferTakenEvent.value.netuid).toBe(netuid);
                expect(BigInt(taoOfferTakenEvent.value.tao_amount)).toBe(offerTaoAmount);

                console.log(`✓ Transfer is within tolerance (${difference} ≤ ${TRANSFER_TOLERANCE})`);
            }
        });
    });
});
