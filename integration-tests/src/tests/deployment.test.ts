import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { setupTestEnvironment, cleanupTestEnvironment, type TestContext, ContractSdk } from "../setup";
import { percentageToFixedPoint, fixedPointToPercentage } from "../utils";

describe("OTC Contract Deployment", () => {
    let context: TestContext;
    let contract: ReturnType<ContractSdk["getContract"]>;

    beforeAll(async () => {
        context = await setupTestEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);
    }, 120000); // 2 minute timeout for setup

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    it("should deploy the contract successfully", async () => {
        expect(context.contractAddress).toBeDefined();

        const isCompatible = await contract.isCompatible();
        expect(isCompatible).toBe(true);
    });

    it("should have correct initial configuration", async () => {
        const { accounts } = context;

        // Test get_owner()
        const ownerResult = await contract.query("get_owner", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(ownerResult.success).toBe(true);

        if (ownerResult.success) {
            expect(ownerResult.value.response).toBe(accounts.alice.address);
        }

        // Test get_hotkey()
        const hotkeyResult = await contract.query("get_hotkey", {
            origin: accounts.alice.address,
            data: {}
        });
        expect(hotkeyResult.success).toBe(true);
        if (hotkeyResult.success) {
            expect(hotkeyResult.value.response).toBe(accounts.eve.address);
        }


        // Test get_fee_rate()
        const feeRateResult = await contract.query("get_fee_rate", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(feeRateResult.success).toBe(true);

        if (feeRateResult.success) {
            // We set 0.5% in the constructor
            const expectedFeeRate = percentageToFixedPoint(0.5);
            // Allow small precision difference (within 10 units for U64F64)
            expect(Math.abs(Number(feeRateResult.value.response - expectedFeeRate))).toBeLessThanOrEqual(10);
            // Verify the fee rate converts back correctly
            const feePercentage = fixedPointToPercentage(feeRateResult.value.response);
            expect(feePercentage).toBeCloseTo(0.5, 5);
        }

        // Test get_min_listing_amount()
        const minListingAmountResult = await contract.query("get_min_listing_amount", {
            origin: accounts.alice.address,
            data: {}
        });
        expect(minListingAmountResult.success).toBe(true);

        if (minListingAmountResult.success) {
            expect(minListingAmountResult.value.response).toBe(2_000_000n);
        }

        // Test get_min_offer_amount()
        const minOfferAmountResult = await contract.query("get_min_offer_amount", {
            origin: accounts.alice.address,
            data: {}
        });
        expect(minOfferAmountResult.success).toBe(true);

        if (minOfferAmountResult.success) {
            expect(minOfferAmountResult.value.response).toBe(1_000_000_000n); // 1 TAO in rao
        }

        // Test get_min_listing_age()
        const minListingAgeResult = await contract.query("get_min_listing_age", {
            origin: accounts.alice.address,
            data: {}
        });
        expect(minListingAgeResult.success).toBe(true);

        if (minListingAgeResult.success) {
            expect(minListingAgeResult.value.response).toBe(100); // 100 blocks
        }
    });

    it("should allow owner to update configuration", async () => {
        const { accounts } = context;

        // Test updating fee rate to 1%
        const newFeeRate = percentageToFixedPoint(1.0);
        const updateFeeTx = contract.send("update_fee_rate", {
            origin: accounts.alice.address,
            data: { new_rate: newFeeRate }
        });

        await updateFeeTx.signAndSubmit(accounts.alice.signer);

        // Verify the update
        const updatedFeeRateResult = await contract.query("get_fee_rate", {
            origin: accounts.alice.address,
            data: {}
        });
        expect(updatedFeeRateResult.success).toBe(true);

        if (updatedFeeRateResult.success) {
            expect(updatedFeeRateResult.value.response).toBe(newFeeRate);
            const updatedPercentage = fixedPointToPercentage(updatedFeeRateResult.value.response);
            expect(updatedPercentage).toBeCloseTo(1.0, 5);
        }

        // Reset back to 0.5% for other tests
        const resetFeeTx = contract.send("update_fee_rate", {
            origin: accounts.alice.address,
            data: { new_rate: percentageToFixedPoint(0.5) }
        });
        await resetFeeTx.signAndSubmit(accounts.alice.signer);
    });

    it("should prevent non-owner from updating configuration", async () => {
        const { accounts } = context;

        // Try to update fee rate as Bob (not owner)
        const newFeeRate = percentageToFixedPoint(2.0);
        const updateFeeTx = contract.send("update_fee_rate", {
            origin: accounts.bob.address,
            data: { new_rate: newFeeRate }
        });

        const result = await updateFeeTx.signAndSubmit(accounts.bob.signer);
        expect(result.dispatchError).toBeDefined();

        // Verify fee rate hasn't changed
        const currentFeeRateResult = await contract.query("get_fee_rate", {
            origin: accounts.alice.address,
            data: {}
        });
        expect(currentFeeRateResult.success).toBe(true);

        if (currentFeeRateResult.success) {
            const currentPercentage = fixedPointToPercentage(currentFeeRateResult.value.response);
            expect(currentPercentage).toBeCloseTo(0.5, 5);
        }
    });

    it("should correctly query listing for non-existent entries", async () => {
        const { accounts } = context;

        // Query non-existent listing
        const listingResult = await contract.query("get_listing", {
            origin: accounts.alice.address,
            data: {
                netuid: 1,
                seller: accounts.bob.address,
                listing_id: 999n
            }
        });

        // Check if the result is None variant
        expect(listingResult.success).toBe(true);

        if (listingResult.success) {
            expect(listingResult.value.response).toBeUndefined();
        }
    });

    it("should correctly query user listings when empty", async () => {
        const { accounts } = context;

        // Query user listings for user with no listings
        const listingsResult = await contract.query("get_user_listings", {
            origin: accounts.alice.address,
            data: {
                seller: accounts.charlie.address,
                netuid: 1
            }
        });

        expect(listingsResult.success).toBe(true);

        if (listingsResult.success) {
            expect(listingsResult.value.response).toEqual([]);
        }
    });

    it("should correctly query offers for non-existent entries", async () => {
        const { accounts } = context;

        // Query non-existent offer
        const offerResult = await contract.query("get_offer", {
            origin: accounts.alice.address,
            data: {
                netuid: 1,
                buyer: accounts.dave.address,
                offer_id: 999n
            }
        });

        // Check if the result is None variant
        expect(offerResult.success).toBe(true);

        if (offerResult.success) {
            expect(offerResult.value.response).toBeUndefined();
        }
    });

    it("should correctly query user offers when empty", async () => {
        const { accounts } = context;

        // Query user offers for user with no offers
        const offersResult = await contract.query("get_user_offers", {
            origin: accounts.alice.address,
            data: {
                buyer: accounts.eve.address,
                netuid: 1
            }
        });

        expect(offersResult.success).toBe(true);

        if (offersResult.success) {
            expect(offersResult.value.response).toEqual([]);
        }
    });
});
