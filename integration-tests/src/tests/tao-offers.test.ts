import { describe, it, expect, beforeAll, afterAll, beforeEach } from "vitest";
import { setupTestEnvironment, cleanupTestEnvironment, type TestContext, ContractSdk, bigintReplacer } from "../setup";
import {
    percentageToFixedPoint,
    priceToFixedPoint,
    fixedPointToPrice,
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
    elevateRegistrationLimits
} from "../utils";
import {
    getStakeBalance,
    hasProxyPermission,
    formatStakeAmount,
} from "../utils/stake-helpers";

type ContractsError = {
    type: 'Contracts',
    value: { type: 'ContractReverted', value: undefined }
}

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
        const aliceHotkey = createHotkey("//Alice");
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
        bobHotkey = createHotkey("//Bob");
        charlieHotkey = createHotkey("//Charlie");
        daveHotkey = createHotkey("//Dave");
        eveHotkey = createHotkey("//Eve");

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
            taoToRao(150) // 150 Alpha initial stake for Bob
        );
        console.log(`✓ Bob's validator registered with 150 Alpha`);

        await registerValidator(
            context.api,
            netuid,
            charlieHotkey.address,
            context.accounts.charlie.signer,
            taoToRao(200) // 200 Alpha initial stake for Charlie
        );
        console.log(`✓ Charlie's validator registered with 200 Alpha`);

        await registerValidator(
            context.api,
            netuid,
            daveHotkey.address,
            context.accounts.dave.signer,
            taoToRao(100) // 100 Alpha initial stake for Dave
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

            // Charlie creates an offer to buy 100 Alpha at 1.5 TAO per Alpha
            const offerAmount = taoToRao(150); // Total TAO to spend
            const offerPrice = priceToFixedPoint(1.5); // Price per Alpha

            const offerTx = contract.send("create_tao_offer", {
                origin: accounts.charlie.address,
                value: offerAmount, // TAO sent with transaction
                data: {
                    netuid,
                    price: offerPrice
                }
            });

            const result = await offerTx.signAndSubmit(accounts.charlie.signer);
            expect(result.ok).toBe(true);

            // Wait for transaction to be processed
            await waitForBlocks(context.api, 2);

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

            expect(offersResult.success).toBe(true);
            if (offersResult.success) {
                expect(offersResult.value.response.length).toBeGreaterThan(0);

                // Get the offer details
                const offerId = offersResult.value.response[0];
                const offer = await contract.query("get_offer", {
                    origin: accounts.alice.address,
                    data: {
                        netuid,
                        buyer: accounts.charlie.address,
                        offer_id: offerId
                    }
                });

                expect(offer.success).toBe(true);
                if (offer.success && offer.value.response) {
                    expect(offer.value.response.amount).toBe(offerAmount);
                    expect(offer.value.response.price).toBe(offerPrice);
                    expect(offer.value.response.buyer).toBe(accounts.charlie.address);
                    expect(offer.value.response.netuid).toBe(netuid);

                    console.log(`Created offer ${offerId}: ${raoToTao(offerAmount)} TAO at ${fixedPointToPrice(offerPrice)} TAO/Alpha`);
                }
            }
        });

        it("should create multiple offers from the same buyer", async () => {
            const { accounts } = context;

            // Dave creates multiple offers
            const offers = [
                { amount: taoToRao(50), price: priceToFixedPoint(1.0) },
                { amount: taoToRao(75), price: priceToFixedPoint(1.25) },
                { amount: taoToRao(100), price: priceToFixedPoint(1.75) }
            ];

            const offerIds: bigint[] = [];

            for (const offer of offers) {
                const tx = contract.send("create_tao_offer", {
                    origin: accounts.dave.address,
                    value: offer.amount,
                    data: {
                        netuid,
                        price: offer.price
                    }
                });

                const result = await tx.signAndSubmit(accounts.dave.signer);
                expect(result.ok).toBe(true);

                await waitForBlocks(context.api, 1);
            }

            // Query Dave's offers
            const offersResult = await contract.query("get_user_offers", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.dave.address,
                    netuid
                }
            });

            expect(offersResult.success).toBe(true);
            if (offersResult.success) {
                expect(offersResult.value.response.length).toBeGreaterThanOrEqual(offers.length);

                // Verify each offer
                for (const offerId of offersResult.value.response) {
                    const offer = await contract.query("get_offer", {
                        origin: accounts.alice.address,
                        data: {
                            netuid,
                            buyer: accounts.dave.address,
                            offer_id: offerId
                        }
                    });

                    expect(offer.success).toBe(true);
                    if (offer.success && offer.value.response) {
                        expect(offer.value.response.buyer).toBe(accounts.dave.address);
                        expect(offer.value.response.netuid).toBe(netuid);
                        offerIds.push(offerId);
                    }
                }
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
                    price: priceToFixedPoint(1.5)
                }
            });

            const result = await offerTx.signAndSubmit(accounts.eve.signer);

            // Should fail with contract error
            expect(result.ok).toBe(false);
            const contractsError = result.dispatchError?.value as ContractsError;
            expect(contractsError.type).toBe("Contracts");
            expect(contractsError.value.type).toBe("ContractReverted");
        });

        it("should reject offer with zero price", async () => {
            const { accounts } = context;

            // Try to create offer with zero price - should fail at contract level
            const offerTx = contract.send("create_tao_offer", {
                origin: accounts.eve.address,
                value: taoToRao(10),
                data: {
                    netuid,
                    price: 0n // Zero price
                }
            });

            const result = await offerTx.signAndSubmit(accounts.eve.signer);

            // Should fail with contract error
            expect(result.ok).toBe(false);
            const contractsError = result.dispatchError?.value as ContractsError;
            expect(contractsError.type).toBe("Contracts");
            expect(contractsError.value.type).toBe("ContractReverted");
        });
    });

    describe("Offer Cancellation", () => {
        let testOfferId: bigint;
        let offerAmount: bigint;

        beforeEach(async () => {
            const { accounts } = context;

            // Create a test offer from Eve
            offerAmount = taoToRao(25);
            const tx = contract.send("create_tao_offer", {
                origin: accounts.eve.address,
                value: offerAmount,
                data: {
                    netuid,
                    price: priceToFixedPoint(2.0)
                }
            });

            await tx.signAndSubmit(accounts.eve.signer);
            await waitForBlocks(context.api, 2);

            // Get the offer ID
            const offersResult = await contract.query("get_user_offers", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.eve.address,
                    netuid
                }
            });

            if (offersResult.success && offersResult.value.response.length > 0) {
                testOfferId = offersResult.value.response[offersResult.value.response.length - 1];
            }
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

            const result = await cancelTx.signAndSubmit(accounts.eve.signer);
            expect(result.ok).toBe(true);

            // Wait for TAO to be returned
            await waitForBlocks(context.api, 2);

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

            const result = await cancelTx.signAndSubmit(accounts.charlie.signer);

            // Should fail because Charlie is not the owner
            expect(result.ok).toBe(false);
            const contractsError = result.dispatchError?.value as ContractsError;
            expect(contractsError.type).toBe("Contracts");
            expect(contractsError.value.type).toBe("ContractReverted");

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

            const result = await cancelTx.signAndSubmit(accounts.charlie.signer);

            // Should fail because offer doesn't exist
            expect(result.ok).toBe(false);
            const contractsError = result.dispatchError?.value as ContractsError;
            expect(contractsError.type).toBe("Contracts");
            expect(contractsError.value.type).toBe("ContractReverted");
        });
    });

    describe("Offer Queries", () => {
        it("should correctly query offers across multiple users", async () => {
            const { accounts } = context;

            // Create offers from different users
            const users = [
                { account: accounts.charlie, amount: taoToRao(10), price: priceToFixedPoint(1.1) },
                { account: accounts.dave, amount: taoToRao(20), price: priceToFixedPoint(1.2) },
                { account: accounts.eve, amount: taoToRao(15), price: priceToFixedPoint(1.3) }
            ];

            for (const user of users) {
                const tx = contract.send("create_tao_offer", {
                    origin: user.account.address,
                    value: user.amount,
                    data: {
                        netuid,
                        price: user.price
                    }
                });
                await tx.signAndSubmit(user.account.signer);
                await waitForBlocks(context.api, 1);
            }

            // Query each user's offers
            for (const user of users) {
                const offersResult = await contract.query("get_user_offers", {
                    origin: accounts.alice.address,
                    data: {
                        buyer: user.account.address,
                        netuid
                    }
                });

                expect(offersResult.success).toBe(true);
                if (offersResult.success) {
                    expect(offersResult.value.response.length).toBeGreaterThan(0);
                    console.log(`User ${user.account.address.slice(0, 8)}... has ${offersResult.value.response.length} offers`);
                }
            }
        });

        it("should return empty array for users with no offers", async () => {
            const { accounts } = context;

            // Bob has created no offers
            const offersResult = await contract.query("get_user_offers", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.bob.address,
                    netuid
                }
            });

            expect(offersResult.success).toBe(true);
            if (offersResult.success) {
                expect(offersResult.value.response).toEqual([]);
            }
        });

        it("should correctly retrieve offer details", async () => {
            const { accounts } = context;

            // Create a specific offer to test
            const offerAmount = taoToRao(42);
            const offerPrice = priceToFixedPoint(3.14);

            const tx = contract.send("create_tao_offer", {
                origin: accounts.charlie.address,
                value: offerAmount,
                data: {
                    netuid,
                    price: offerPrice
                }
            });

            await tx.signAndSubmit(accounts.charlie.signer);
            await waitForBlocks(context.api, 2);

            // Get the offer ID
            const offersResult = await contract.query("get_user_offers", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.charlie.address,
                    netuid
                }
            });

            expect(offersResult.success).toBe(true);
            if (offersResult.success && offersResult.value.response.length > 0) {
                const offerId = offersResult.value.response[offersResult.value.response.length - 1];

                // Get full offer details
                const offerResult = await contract.query("get_offer", {
                    origin: accounts.alice.address,
                    data: {
                        netuid,
                        buyer: accounts.charlie.address,
                        offer_id: offerId
                    }
                });

                expect(offerResult.success).toBe(true);
                if (offerResult.success && offerResult.value.response) {
                    const offer = offerResult.value.response;

                    expect(offer.id).toBe(offerId);
                    expect(offer.buyer).toBe(accounts.charlie.address);
                    expect(offer.netuid).toBe(netuid);
                    expect(offer.amount).toBe(offerAmount);
                    expect(offer.price).toBe(offerPrice);

                    // Verify fee rate matches contract configuration
                    const feeRateResult = await contract.query("get_fee_rate", {
                        origin: accounts.alice.address,
                        data: {}
                    });

                    if (feeRateResult.success) {
                        expect(offer.fee_rate).toBe(feeRateResult.value.response);
                    }

                    console.log(`Offer ${offerId}: ${raoToTao(offer.amount)} TAO at ${fixedPointToPrice(offer.price)} TAO/Alpha`);
                }
            }
        });
    });

    describe("Taking TAO Offers", () => {
        it("should successfully take a TAO offer by providing Alpha", async () => {
            const { accounts } = context;

            // === Setup: Charlie creates offer to buy Alpha at 1.8 TAO per Alpha ===
            const offerTaoAmount = taoToRao(90); // Total TAO to spend
            const offerPrice = priceToFixedPoint(1.8); // Price per Alpha

            const offerTx = contract.send("create_tao_offer", {
                origin: accounts.charlie.address,
                value: offerTaoAmount,
                data: {
                    netuid,
                    price: offerPrice
                }
            });

            await offerTx.signAndSubmit(accounts.charlie.signer);
            await waitForBlocks(context.api, 2);

            // Get the offer ID
            const offersResult = await contract.query("get_user_offers", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.charlie.address,
                    netuid
                }
            });

            expect(offersResult.success).toBe(true);
            const offerId = offersResult.success ?
                offersResult.value.response[offersResult.value.response.length - 1] : 0n;

            // === Calculate Alpha amount needed ===
            const { alphaAmount, feeAmount, taoForSeller } = calculateAlphaForOffer(
                offerTaoAmount,
                offerPrice,
                contractFeeRate
            );

            console.log(`\nOffer: ${raoToTao(offerTaoAmount)} TAO at ${fixedPointToPrice(offerPrice)} TAO/Alpha`);
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

            const result = await takeTx.signAndSubmit(accounts.bob.signer);

            console.log("Take TAO offer result:", {
                ok: result.ok,
                dispatchError: result.dispatchError,
            });

            expect(result.ok).toBe(true);

            // === Verify the TaoOfferTaken event ===
            const events = contract.filterEvents(result.events);
            console.log("Events emitted during transaction:", JSON.stringify(events, bigintReplacer, 2));

            const taoOfferTakenEvent = events.find(e => e.type === 'TaoOfferTaken');
            expect(taoOfferTakenEvent).toBeDefined();

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
            const offerTx = contract.send("create_tao_offer", {
                origin: accounts.dave.address,
                value: taoToRao(50),
                data: {
                    netuid,
                    price: priceToFixedPoint(2.0)
                }
            });

            await offerTx.signAndSubmit(accounts.dave.signer);
            await waitForBlocks(context.api, 2);

            // Get offer ID
            const offersResult = await contract.query("get_user_offers", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.dave.address,
                    netuid
                }
            });

            const offerId = offersResult.success ?
                offersResult.value.response[offersResult.value.response.length - 1] : 0n;

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

                const result = await takeTx.signAndSubmit(accounts.charlie.signer);

                // Should fail because no proxy is set up
                expect(result.ok).toBe(false);
                expect(result.dispatchError?.type).toContain("Module");
            } else {
                console.log("Charlie already has proxy from previous test run, skipping test");
            }
        });

        it("should reject taking offer with insufficient Alpha stake", async () => {
            const { accounts } = context;

            // Create a large offer that Eve can't fulfill
            const offerTx = contract.send("create_tao_offer", {
                origin: accounts.charlie.address,
                value: taoToRao(100), // Large offer
                data: {
                    netuid,
                    price: priceToFixedPoint(2.0)
                }
            });

            await offerTx.signAndSubmit(accounts.charlie.signer);
            await waitForBlocks(context.api, 2);

            // Get offer ID
            const offersResult = await contract.query("get_user_offers", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.charlie.address,
                    netuid
                }
            });

            const offerId = offersResult.success ?
                offersResult.value.response[offersResult.value.response.length - 1] : 0n;

            // Calculate required Alpha
            const { alphaAmount } = calculateAlphaForOffer(
                taoToRao(100),
                priceToFixedPoint(2.0),
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

            const result = await takeTx.signAndSubmit(accounts.eve.signer);

            // Should fail with ContractReverted error due to insufficient stake
            expect(result.ok).toBe(false);
            const contractsError = result.dispatchError?.value as ContractsError;
            expect(contractsError.type).toBe("Contracts");
            expect(contractsError.value.type).toBe("ContractReverted");
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

            const result = await takeTx.signAndSubmit(accounts.bob.signer);

            // Should fail with ContractReverted because offer doesn't exist
            expect(result.ok).toBe(false);
            const contractsError = result.dispatchError?.value as ContractsError;
            expect(contractsError.type).toBe("Contracts");
            expect(contractsError.value.type).toBe("ContractReverted");
        });

        it("should verify stake transfer with tolerance", async () => {
            const { accounts } = context;

            // Create a TAO offer with a specific amount that might trigger rounding
            const offerTaoAmount = taoToRao(75.5); // Intentional decimal for potential rounding
            const offerPrice = priceToFixedPoint(1.25);

            const offerTx = contract.send("create_tao_offer", {
                origin: accounts.dave.address,
                value: offerTaoAmount,
                data: {
                    netuid,
                    price: offerPrice
                }
            });

            await offerTx.signAndSubmit(accounts.dave.signer);
            await waitForBlocks(context.api, 2);

            // Get offer ID
            const offersResult = await contract.query("get_user_offers", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.dave.address,
                    netuid
                }
            });

            const offerId = offersResult.success ?
                offersResult.value.response[offersResult.value.response.length - 1] : 0n;

            // Calculate expected Alpha amount
            const { alphaAmount } = calculateAlphaForOffer(
                offerTaoAmount,
                offerPrice,
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

            const result = await takeTx.signAndSubmit(accounts.charlie.signer);
            expect(result.ok).toBe(true);

            // === Verify the TaoOfferTaken event for tolerance ===
            const events = contract.filterEvents(result.events);
            const taoOfferTakenEvent = events.find(e => e.type === 'TaoOfferTaken');

            expect(taoOfferTakenEvent).toBeDefined();

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
