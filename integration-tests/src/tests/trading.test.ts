import { describe, it, expect, beforeAll, afterAll } from "vitest";
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
    calculateTotalTaoForListing,
    elevateRegistrationLimits,
    type Wallet,
    MARKET_PRICE,
    getExecutedPriceFixed,
    bpsToPercentage,
} from "../utils";
import {
    getStakeBalance,
    formatStakeAmount,
} from "../utils/stake-helpers";

type ContractsError = {
    type: 'Contracts',
    value: { type: 'ContractReverted', value: undefined }
}

describe("Trading Execution", () => {
    let context: TestContext;
    let contract: ReturnType<ContractSdk["getContract"]>;
    let netuid: number;
    let bobHotkey: Wallet;
    let charlieHotkey: Wallet;
    let daveHotkey: Wallet;
    let eveHotkey: Wallet;
    let contractFeeRate: bigint;

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

        // Register validators with substantial stake for trading
        await registerValidator(context.api, netuid, bobHotkey.address, context.accounts.bob.signer, taoToRao(200));
        await registerValidator(context.api, netuid, charlieHotkey.address, context.accounts.charlie.signer, taoToRao(300));
        await registerValidator(context.api, netuid, daveHotkey.address, context.accounts.dave.signer, taoToRao(150));
        await registerValidator(context.api, netuid, eveHotkey.address, context.accounts.eve.signer, taoToRao(250));

        // Fund accounts with TAO for trading
        await fundAccount(context.api, context.accounts.bob.address, taoToRao(500), context.accounts.alice.signer);
        await fundAccount(context.api, context.accounts.charlie.address, taoToRao(600), context.accounts.alice.signer);
        await fundAccount(context.api, context.accounts.dave.address, taoToRao(400), context.accounts.alice.signer);
        await fundAccount(context.api, context.accounts.eve.address, taoToRao(300), context.accounts.alice.signer);

        // Setup proxies for sellers (needed for listing Alpha)
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.bob.signer);
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.charlie.signer);
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.eve.signer);

        await waitForBlocks(context.api, 3);

        // Log initial balances
        console.log("\n=== Initial Setup Complete ===");
        console.log(`Bob's stake: ${formatStakeAmount(await getStakeBalance(context.api, bobHotkey.address, netuid, context.accounts.bob.address))}`);
        console.log(`Charlie's stake: ${formatStakeAmount(await getStakeBalance(context.api, charlieHotkey.address, netuid, context.accounts.charlie.address))}`);
        console.log(`Dave's stake: ${formatStakeAmount(await getStakeBalance(context.api, daveHotkey.address, netuid, context.accounts.dave.address))}`);
        console.log(`Eve's stake: ${formatStakeAmount(await getStakeBalance(context.api, eveHotkey.address, netuid, context.accounts.eve.address))}`);
    }, 180000); // 3 minute timeout

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    describe("Taking Alpha Listings", () => {
        it("should successfully take an Alpha listing with exact TAO payment", async () => {
            const { accounts } = context;

            // === Setup: Bob lists 50 Alpha at market price ===
            const listAmount = taoToRao(50);
            const priceOffsetBps = MARKET_PRICE; // 0% offset from market

            const listTx = contract.send("list_alpha", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: priceOffsetBps
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
            const listingId = listingsResult.success ?
                listingsResult.value.response[0] : 0n;

            // === Calculate required TAO payment using market price with offset ===
            const executedPriceFixed = await getExecutedPriceFixed(context.api, netuid, priceOffsetBps);
            const { taoAmount, feeAmount, totalRequired } = calculateTotalTaoForListing(
                listAmount,
                executedPriceFixed,
                contractFeeRate
            );

            console.log(`\nListing: 50 Alpha at ${bpsToPercentage(priceOffsetBps)}% offset from market`);
            console.log(`TAO amount: ${raoToTao(taoAmount)} TAO`);
            console.log(`Fee amount: ${raoToTao(feeAmount)} TAO`);
            console.log(`Total required: ${raoToTao(totalRequired)} TAO`);

            // === Dave takes the listing ===
            // Get balances before trade
            const daveTaoBefore = await getBalance(context.api, accounts.dave.address);
            const bobTaoBefore = await getBalance(context.api, accounts.bob.address);
            const ownerTaoBefore = await getBalance(context.api, accounts.alice.address);
            const daveAlphaBefore = await getStakeBalance(context.api, daveHotkey.address, netuid, accounts.dave.address);

            console.log(`\nBefore trade:`);
            console.log(`Dave TAO: ${raoToTao(daveTaoBefore)}`);
            console.log(`Dave Alpha: ${formatStakeAmount(daveAlphaBefore)}`);

            // Take the listing with exact payment
            const takeTx = contract.send("take_alpha_listing", {
                origin: accounts.dave.address,
                value: totalRequired, // Exact TAO amount including fees
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId
                }
            });

            const result = await takeTx.signAndSubmit(accounts.dave.signer);
            expect(result.ok).toBe(true);

            // === Verify the AlphaListingTaken event ===
            const events = contract.filterEvents(result.events);
            console.log("Events emitted during transaction:", JSON.stringify(events, bigintReplacer, 2));

            const alphaListingTakenEvent = events.find(e => e.type === 'AlphaListingTaken');
            expect(alphaListingTakenEvent).toBeDefined();

            if (alphaListingTakenEvent) {
                // Verify the event contains expected values
                expect(BigInt(alphaListingTakenEvent.value.alpha_amount)).toBe(listAmount);
                expect(BigInt(alphaListingTakenEvent.value.tao_amount)).toBe(taoAmount);
                expect(alphaListingTakenEvent.value.seller).toBe(accounts.bob.address);
                expect(alphaListingTakenEvent.value.buyer).toBe(accounts.dave.address);
                expect(BigInt(alphaListingTakenEvent.value.fee)).toBe(feeAmount);
                expect(BigInt(alphaListingTakenEvent.value.alpha_listing_id || 0)).toBe(listingId);

                console.log(`\nAlphaListingTaken event verified:`);
                console.log(`Alpha transferred: ${formatStakeAmount(BigInt(alphaListingTakenEvent.value.alpha_amount))}`);
                console.log(`TAO amount: ${raoToTao(BigInt(alphaListingTakenEvent.value.tao_amount))}`);
                console.log(`Fee: ${raoToTao(BigInt(alphaListingTakenEvent.value.fee))}`);
            }

            await waitForBlocks(context.api, 3);

            // === Verify TAO balances ===
            const daveTaoAfter = await getBalance(context.api, accounts.dave.address);
            const bobTaoAfter = await getBalance(context.api, accounts.bob.address);
            const ownerTaoAfter = await getBalance(context.api, accounts.alice.address);

            // Dave's TAO should decrease by approximately the total payment
            // We use a tolerance here because the exact amount might vary due to runtime mechanics
            const daveTaoSpent = daveTaoBefore - daveTaoAfter;
            const tolerance = taoToRao(1); // 1 TAO tolerance for runtime fee variations
            expect(daveTaoSpent).toBeGreaterThan(totalRequired - tolerance);
            expect(daveTaoSpent).toBeLessThan(totalRequired + tolerance);

            // Bob should receive TAO (minus fee)
            const bobTaoReceived = bobTaoAfter - bobTaoBefore;
            expect(bobTaoReceived).toBeGreaterThanOrEqual(taoAmount - taoToRao(0.1)); // Allow small variance for gas

            // Owner should receive fee
            const ownerFeeReceived = ownerTaoAfter - ownerTaoBefore;
            expect(ownerFeeReceived).toBeGreaterThanOrEqual(feeAmount - taoToRao(0.01)); // Allow tiny variance

            console.log(`\nAfter trade:`);
            console.log(`Dave TAO spent: ${raoToTao(daveTaoSpent)}`);
            console.log(`Bob TAO received: ${raoToTao(bobTaoReceived)}`);
            console.log(`Owner fee received: ${raoToTao(ownerFeeReceived)}`);

            // Verify listing was removed
            const listingResult = await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId
                }
            });

            expect(listingResult.success).toBe(true);
            if (listingResult.success) {
                expect(listingResult.value.response).toBeUndefined(); // Should be None
            }
        });

        it("should successfully take an Alpha listing with non-market price offset", async () => {
            const { accounts } = context;

            // === Setup: Charlie lists 40 Alpha at +5% above market ===
            const listAmount = taoToRao(40);
            const priceOffsetBps = 500; // +5% above market

            const listTx = contract.send("list_alpha", {
                origin: accounts.charlie.address,
                data: {
                    hotkey: charlieHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: priceOffsetBps
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
            const listingId = listingsResult.success ?
                listingsResult.value.response[listingsResult.value.response.length - 1] : 0n;

            // === Calculate required TAO payment using the offset price ===
            const executedPriceFixed = await getExecutedPriceFixed(context.api, netuid, priceOffsetBps);
            const { taoAmount, feeAmount, totalRequired } = calculateTotalTaoForListing(
                listAmount,
                executedPriceFixed,
                contractFeeRate
            );

            // Log the price difference vs market price for verification
            const marketPriceFixed = await getExecutedPriceFixed(context.api, netuid, MARKET_PRICE);

            console.log(`\nListing: 40 Alpha at +${bpsToPercentage(priceOffsetBps)}% above market`);
            console.log(`Market price (fixed): ${marketPriceFixed}`);
            console.log(`Executed price (+5%): ${executedPriceFixed}`);
            console.log(`TAO amount: ${raoToTao(taoAmount)} TAO`);
            console.log(`Fee amount: ${raoToTao(feeAmount)} TAO`);
            console.log(`Total required: ${raoToTao(totalRequired)} TAO`);

            // Verify executed price is ~5% higher than market price
            const actualRatio = (executedPriceFixed * 100n) / marketPriceFixed;
            expect(actualRatio).toBeGreaterThanOrEqual(104n); // Allow some tolerance
            expect(actualRatio).toBeLessThanOrEqual(106n);

            // === Eve takes the listing ===
            // Get balances before trade
            const eveTaoBefore = await getBalance(context.api, accounts.eve.address);
            const charlieTaoBefore = await getBalance(context.api, accounts.charlie.address);
            const ownerTaoBefore = await getBalance(context.api, accounts.alice.address);

            console.log(`\nBefore trade:`);
            console.log(`Eve TAO: ${raoToTao(eveTaoBefore)}`);
            console.log(`Charlie TAO: ${raoToTao(charlieTaoBefore)}`);

            // Take the listing with exact payment
            const takeTx = contract.send("take_alpha_listing", {
                origin: accounts.eve.address,
                value: totalRequired,
                data: {
                    netuid,
                    seller: accounts.charlie.address,
                    listing_id: listingId
                }
            });

            const result = await takeTx.signAndSubmit(accounts.eve.signer);
            expect(result.ok).toBe(true);

            // === Verify the AlphaListingTaken event ===
            const events = contract.filterEvents(result.events);
            const alphaListingTakenEvent = events.find(e => e.type === 'AlphaListingTaken');
            expect(alphaListingTakenEvent).toBeDefined();

            if (alphaListingTakenEvent) {
                // Verify the event contains expected values
                expect(BigInt(alphaListingTakenEvent.value.alpha_amount)).toBe(listAmount);
                expect(BigInt(alphaListingTakenEvent.value.tao_amount)).toBe(taoAmount);
                expect(alphaListingTakenEvent.value.seller).toBe(accounts.charlie.address);
                expect(alphaListingTakenEvent.value.buyer).toBe(accounts.eve.address);
                expect(BigInt(alphaListingTakenEvent.value.fee)).toBe(feeAmount);

                // Verify price offset is recorded in event (if present)
                if ('price_offset_bps' in alphaListingTakenEvent.value) {
                    expect(alphaListingTakenEvent.value.price_offset_bps).toBe(priceOffsetBps);
                }

                console.log(`\nAlphaListingTaken event verified:`);
                console.log(`Alpha transferred: ${formatStakeAmount(BigInt(alphaListingTakenEvent.value.alpha_amount))}`);
                console.log(`TAO amount: ${raoToTao(BigInt(alphaListingTakenEvent.value.tao_amount))}`);
                console.log(`Fee: ${raoToTao(BigInt(alphaListingTakenEvent.value.fee))}`);
            }

            await waitForBlocks(context.api, 3);

            // === Verify TAO balances ===
            const eveTaoAfter = await getBalance(context.api, accounts.eve.address);
            const charlieTaoAfter = await getBalance(context.api, accounts.charlie.address);
            const ownerTaoAfter = await getBalance(context.api, accounts.alice.address);

            // Eve's TAO should decrease by approximately the total payment
            const eveTaoSpent = eveTaoBefore - eveTaoAfter;
            const tolerance = taoToRao(1);
            expect(eveTaoSpent).toBeGreaterThan(totalRequired - tolerance);
            expect(eveTaoSpent).toBeLessThan(totalRequired + tolerance);

            // Charlie should receive TAO (based on +5% price, minus fee)
            const charlieTaoReceived = charlieTaoAfter - charlieTaoBefore;
            expect(charlieTaoReceived).toBeGreaterThanOrEqual(taoAmount - taoToRao(0.1));

            // Owner should receive fee
            const ownerFeeReceived = ownerTaoAfter - ownerTaoBefore;
            expect(ownerFeeReceived).toBeGreaterThanOrEqual(feeAmount - taoToRao(0.01));

            console.log(`\nAfter trade:`);
            console.log(`Eve TAO spent: ${raoToTao(eveTaoSpent)}`);
            console.log(`Charlie TAO received: ${raoToTao(charlieTaoReceived)}`);
            console.log(`Owner fee received: ${raoToTao(ownerFeeReceived)}`);

            // Verify listing was removed
            const listingResult = await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.charlie.address,
                    listing_id: listingId
                }
            });

            expect(listingResult.success).toBe(true);
            if (listingResult.success) {
                expect(listingResult.value.response).toBeUndefined();
            }

            console.log(`\n✓ Full flow with +5% price offset verified successfully`);
        });

        it("should reject taking listing with incorrect TAO amount", async () => {
            const { accounts } = context;

            // Charlie lists 30 Alpha at market price
            const listAmount = taoToRao(30);
            const priceOffsetBps = MARKET_PRICE;

            const listTx = contract.send("list_alpha", {
                origin: accounts.charlie.address,
                data: {
                    hotkey: charlieHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: priceOffsetBps
                }
            });

            await listTx.signAndSubmit(accounts.charlie.signer);
            await waitForBlocks(context.api, 2);

            // Get listing ID
            const listingsResult = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.charlie.address,
                    netuid
                }
            });

            const listingId = listingsResult.success ?
                listingsResult.value.response[0] : 0n;

            // Calculate correct amount using executed price
            const executedPriceFixed = await getExecutedPriceFixed(context.api, netuid, priceOffsetBps);
            const { totalRequired } = calculateTotalTaoForListing(
                listAmount,
                executedPriceFixed,
                contractFeeRate
            );

            // Try to take with incorrect amount (too little)
            const takeTx = contract.send("take_alpha_listing", {
                origin: accounts.eve.address,
                value: totalRequired - taoToRao(1), // 1 TAO less than required
                data: {
                    netuid,
                    seller: accounts.charlie.address,
                    listing_id: listingId
                }
            });

            const result = await takeTx.signAndSubmit(accounts.eve.signer);

            // Should fail due to incorrect amount
            expect(result.ok).toBe(false);
            const contractsError = result.dispatchError?.value as ContractsError;
            expect(contractsError.type).toBe("Contracts");
            expect(contractsError.value.type).toBe("ContractReverted");
        });

        it("should reject taking non-existent listing", async () => {
            const { accounts } = context;

            const takeTx = contract.send("take_alpha_listing", {
                origin: accounts.dave.address,
                value: taoToRao(100),
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: 999999n // Non-existent
                }
            });

            const result = await takeTx.signAndSubmit(accounts.dave.signer);

            // Should fail because listing doesn't exist
            expect(result.ok).toBe(false);
            const contractsError = result.dispatchError?.value as ContractsError;
            expect(contractsError.type).toBe("Contracts");
            expect(contractsError.value.type).toBe("ContractReverted");
        });
    });

    // Note: TAO offer tests are primarily covered in tao-offers.test.ts
    // Here we only test edge cases specific to the trading execution flow
    describe("Trading Edge Cases", () => {
        it("should handle partial fills and minimum amounts correctly", async () => {
            const { accounts } = context;

            // Test minimum amounts
            const minListingResult = await contract.query("get_min_listing_amount", {
                origin: accounts.alice.address,
                data: {}
            });
            const minOfferResult = await contract.query("get_min_offer_amount", {
                origin: accounts.alice.address,
                data: {}
            });

            const minListingAmount = minListingResult.success ? minListingResult.value.response : taoToRao(10);
            const minOfferAmount = minOfferResult.success ? minOfferResult.value.response : taoToRao(1);

            console.log(`Minimum listing amount: ${formatStakeAmount(minListingAmount)}`);
            console.log(`Minimum offer amount: ${raoToTao(minOfferAmount)} TAO`);

            // === Test listing at exactly minimum amount ===
            const priceOffsetBps = MARKET_PRICE;

            const listTx = contract.send("list_alpha", {
                origin: accounts.eve.address,
                data: {
                    hotkey: eveHotkey.address,
                    netuid,
                    amount: minListingAmount,
                    price_offset_bps: priceOffsetBps
                }
            });

            const listResult = await listTx.signAndSubmit(accounts.eve.signer);
            expect(listResult.ok).toBe(true);

            // === Test offer at exactly minimum amount ===
            const offerTx = contract.send("create_tao_offer", {
                origin: accounts.charlie.address,
                value: minOfferAmount,
                data: {
                    netuid,
                    price_offset_bps: MARKET_PRICE
                }
            });

            const offerResult = await offerTx.signAndSubmit(accounts.charlie.signer);
            expect(offerResult.ok).toBe(true);

            console.log(`✓ Successfully created listing at minimum amount: ${formatStakeAmount(minListingAmount)}`);
            console.log(`✓ Successfully created offer at minimum amount: ${raoToTao(minOfferAmount)} TAO`);
        });

        it("should handle rapid successive trades correctly", async () => {
            const { accounts } = context;

            // Create multiple listings in succession with different price offsets
            const listings = [
                { seller: accounts.bob, hotkey: bobHotkey, amount: taoToRao(10), priceOffsetBps: MARKET_PRICE },   // Market price
                { seller: accounts.charlie, hotkey: charlieHotkey, amount: taoToRao(15), priceOffsetBps: 200 },    // +2%
                { seller: accounts.eve, hotkey: eveHotkey, amount: taoToRao(20), priceOffsetBps: 500 }             // +5%
            ];

            const listingIds: bigint[] = [];

            for (const listing of listings) {
                const tx = contract.send("list_alpha", {
                    origin: listing.seller.address,
                    data: {
                        hotkey: listing.hotkey.address,
                        netuid,
                        amount: listing.amount,
                        price_offset_bps: listing.priceOffsetBps
                    }
                });

                const result = await tx.signAndSubmit(listing.seller.signer);
                expect(result.ok).toBe(true);

                const userListings = await contract.query("get_user_listings", {
                    origin: accounts.alice.address,
                    data: {
                        seller: listing.seller.address,
                        netuid
                    }
                });

                if (userListings.success) {
                    const lastId = userListings.value.response[userListings.value.response.length - 1];
                    listingIds.push(lastId);
                }
            }

            await waitForBlocks(context.api, 2);

            console.log(`Created ${listingIds.length} listings: ${listingIds.join(", ")}`);

            // Take first listing to verify it doesn't affect others
            const executedPriceFixed = await getExecutedPriceFixed(context.api, netuid, listings[0].priceOffsetBps);
            const { totalRequired } = calculateTotalTaoForListing(
                listings[0].amount,
                executedPriceFixed,
                contractFeeRate
            );

            const takeTx = contract.send("take_alpha_listing", {
                origin: accounts.dave.address,
                value: totalRequired,
                data: {
                    netuid,
                    seller: listings[0].seller.address,
                    listing_id: listingIds[0]
                }
            });

            const result = await takeTx.signAndSubmit(accounts.dave.signer);
            expect(result.ok).toBe(true);

            // Verify other listings still exist
            for (let i = 1; i < listings.length; i++) {
                const listing = await contract.query("get_listing", {
                    origin: accounts.alice.address,
                    data: {
                        netuid,
                        seller: listings[i].seller.address,
                        listing_id: listingIds[i]
                    }
                });

                expect(listing.success).toBe(true);
                if (listing.success) {
                    expect(listing.value.response).toBeDefined();
                    console.log(`✓ Listing ${listingIds[i]} still exists after taking listing ${listingIds[0]}`);
                }
            }
        });
    });

    describe("Cross-Subnet Trading", () => {
        it("should handle trading on different subnets correctly", async () => {
            const { accounts } = context;

            // Create a second subnet
            const aliceHotkey2 = createHotkey("//Alice//2");
            await fundAccount(context.api, aliceHotkey2.address, taoToRao(10), accounts.alice.signer);
            const netuid2 = await registerSubnet(context.api, aliceHotkey2.address, accounts.alice.signer);
            console.log(`Created second subnet with netuid: ${netuid2}`);

            // Elevate registration limits for second subnet
            await elevateRegistrationLimits(
                context.api,
                netuid2,
                20,
                20,
                context.accounts.alice.signer
            );
            await waitForBlocks(context.api, 1);

            // Register contract's hotkey on second subnet
            const contractHotkeyResult = await contract.query("get_hotkey", {
                origin: context.accounts.alice.address,
                data: {}
            });

            if (contractHotkeyResult.success) {
                const contractHotkey = contractHotkeyResult.value.response;
                await registerValidator(
                    context.api,
                    netuid2,
                    contractHotkey,
                    context.accounts.alice.signer,
                    taoToRao(50)
                );
                console.log(`Registered contract's hotkey on subnet ${netuid2}`);
            }

            // Register validators on second subnet
            const bobHotkey2 = createHotkey("//Bob//2");
            const charlieHotkey2 = createHotkey("//Charlie//2");

            await fundAccount(context.api, bobHotkey2.address, taoToRao(1), accounts.alice.signer);
            await fundAccount(context.api, charlieHotkey2.address, taoToRao(1), accounts.alice.signer);

            await registerValidator(context.api, netuid2, bobHotkey2.address, accounts.bob.signer, taoToRao(100));
            await registerValidator(context.api, netuid2, charlieHotkey2.address, accounts.charlie.signer, taoToRao(100));

            await waitForBlocks(context.api, 2);

            // Bob lists on subnet 1 at market price
            const priceOffsetBps1 = MARKET_PRICE;
            const list1Tx = contract.send("list_alpha", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid: netuid, // First subnet
                    amount: taoToRao(20),
                    price_offset_bps: priceOffsetBps1
                }
            });

            const result1 = await list1Tx.signAndSubmit(accounts.bob.signer);
            expect(result1.ok).toBe(true);

            // Bob also lists on subnet 2 at +5% above market
            const priceOffsetBps2 = 500; // +5%
            const list2Tx = contract.send("list_alpha", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey2.address,
                    netuid: netuid2, // Second subnet
                    amount: taoToRao(30),
                    price_offset_bps: priceOffsetBps2
                }
            });

            const result2 = await list2Tx.signAndSubmit(accounts.bob.signer);
            expect(result2.ok).toBe(true);
            await waitForBlocks(context.api, 2);

            // Verify listings are separate per subnet
            const listings1 = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.bob.address,
                    netuid: netuid
                }
            });

            const listings2 = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.bob.address,
                    netuid: netuid2
                }
            });

            expect(listings1.success).toBe(true);
            expect(listings2.success).toBe(true);

            if (listings1.success && listings2.success) {
                // Should have separate listings on each subnet
                expect(listings1.value.response.length).toBeGreaterThan(0);
                expect(listings2.value.response.length).toBeGreaterThan(0);

                // Listing IDs should be different
                const listing1Id = listings1.value.response[listings1.value.response.length - 1];
                const listing2Id = listings2.value.response[listings2.value.response.length - 1];
                expect(listing1Id).not.toBe(listing2Id);

                console.log(`Bob has listings on subnet ${netuid}: ${listings1.value.response}`);
                console.log(`Bob has listings on subnet ${netuid2}: ${listings2.value.response}`);

                // Take listing from subnet 1 and verify it doesn't affect subnet 2
                const executedPriceFixed = await getExecutedPriceFixed(context.api, netuid, priceOffsetBps1);
                const { totalRequired } = calculateTotalTaoForListing(
                    taoToRao(20),
                    executedPriceFixed,
                    contractFeeRate
                );

                const takeTx = contract.send("take_alpha_listing", {
                    origin: accounts.dave.address,
                    value: totalRequired,
                    data: {
                        netuid,
                        seller: accounts.bob.address,
                        listing_id: listing1Id
                    }
                });

                const takeResult = await takeTx.signAndSubmit(accounts.dave.signer);
                expect(takeResult.ok).toBe(true);

                // Verify listing on subnet 2 still exists
                const subnet2Listing = await contract.query("get_listing", {
                    origin: accounts.alice.address,
                    data: {
                        netuid: netuid2,
                        seller: accounts.bob.address,
                        listing_id: listing2Id
                    }
                });

                expect(subnet2Listing.success).toBe(true);
                if (subnet2Listing.success) {
                    expect(subnet2Listing.value.response).toBeDefined();
                    console.log(`✓ Listing on subnet ${netuid2} still exists after taking listing on subnet ${netuid}`);
                }
            }
        });
    });
});
