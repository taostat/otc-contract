import { describe, it, expect, beforeAll, afterAll } from "vitest";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    type LockupListingsContext,
    LockupListingsSdk
} from "../setup";
import { percentageToFixedPoint, fixedPointToPercentage } from "../utils";

describe("Lockup Listings Contract - Deployment", () => {
    let context: LockupListingsContext;
    let contract: ReturnType<LockupListingsSdk["getContract"]>;

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);
    }, 180000); // 3 minute timeout for setup

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    it("should deploy the contract successfully", async () => {
        expect(context.contractAddress).toBeDefined();
        expect(context.escrowCodeHash).toBeDefined();

        const isCompatible = await contract.isCompatible();
        expect(isCompatible).toBe(true);
    });

    it("should have correct initial owner", async () => {
        const { accounts } = context;

        const ownerResult = await contract.query("get_owner", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(ownerResult.success).toBe(true);
        if (ownerResult.success) {
            expect(ownerResult.value.response).toBe(accounts.alice.address);
        }
    });

    it("should have correct initial hotkey", async () => {
        const { accounts } = context;

        const hotkeyResult = await contract.query("get_hotkey", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(hotkeyResult.success).toBe(true);
        if (hotkeyResult.success) {
            expect(hotkeyResult.value.response).toBe(accounts.alice.address);
        }
    });

    it("should have correct escrow code hash", async () => {
        const { accounts, escrowCodeHash } = context;

        const codeHashResult = await contract.query("get_escrow_code_hash", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(codeHashResult.success).toBe(true);
        if (codeHashResult.success) {
            const responseHex = codeHashResult.value.response.asHex();
            expect(responseHex).toBe(escrowCodeHash);
        }
    });

    it("should have correct fee rate", async () => {
        const { accounts } = context;

        const feeRateResult = await contract.query("get_fee_rate", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(feeRateResult.success).toBe(true);
        if (feeRateResult.success) {
            // We set 0.5% in the constructor
            const expectedFeeRate = percentageToFixedPoint(0.5);
            expect(Math.abs(Number(feeRateResult.value.response - expectedFeeRate))).toBeLessThanOrEqual(10);
            const feePercentage = fixedPointToPercentage(feeRateResult.value.response);
            expect(feePercentage).toBeCloseTo(0.5, 5);
        }
    });

    it("should have correct minimum listing amount", async () => {
        const { accounts } = context;

        const minListingAmountResult = await contract.query("get_min_listing_amount", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(minListingAmountResult.success).toBe(true);
        if (minListingAmountResult.success) {
            expect(minListingAmountResult.value.response).toBe(1_000_000_000n); // 1 Alpha
        }
    });

    it("should have correct minimum purchase amount", async () => {
        const { accounts } = context;

        const minPurchaseAmountResult = await contract.query("get_min_purchase_amount", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(minPurchaseAmountResult.success).toBe(true);
        if (minPurchaseAmountResult.success) {
            expect(minPurchaseAmountResult.value.response).toBe(100_000_000n); // 0.1 Alpha
        }
    });

    it("should have correct lockup duration limits", async () => {
        const { accounts } = context;

        const durationLimitsResult = await contract.query("get_lockup_duration_limits", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(durationLimitsResult.success).toBe(true);
        if (durationLimitsResult.success) {
            const [minDuration, maxDuration] = durationLimitsResult.value.response;
            expect(minDuration).toBe(10); // 10 blocks min
            expect(maxDuration).toBe(1_000_000); // ~46 days max
        }
    });

    it("should start with NotPaused state", async () => {
        const { accounts } = context;

        const pauseStateResult = await contract.query("get_pause_state", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(pauseStateResult.success).toBe(true);
        if (pauseStateResult.success) {
            expect(pauseStateResult.value.response.type).toBe("NotPaused");
        }
    });

    it("should return empty listings for new users", async () => {
        const { accounts } = context;

        const listingsResult = await contract.query("get_user_listings", {
            origin: accounts.alice.address,
            data: {
                seller: accounts.bob.address,
                netuid: 1
            }
        });

        expect(listingsResult.success).toBe(true);
        if (listingsResult.success) {
            expect(listingsResult.value.response).toEqual([]);
        }
    });

    it("should return undefined for non-existent listing", async () => {
        const { accounts } = context;

        const listingResult = await contract.query("get_listing", {
            origin: accounts.alice.address,
            data: {
                netuid: 1,
                seller: accounts.bob.address,
                listing_id: 999n
            }
        });

        expect(listingResult.success).toBe(true);
        if (listingResult.success) {
            expect(listingResult.value.response).toBeUndefined();
        }
    });

    it("should return zero reserved alpha for empty subnet", async () => {
        const { accounts } = context;

        const reservedResult = await contract.query("get_reserved_alpha", {
            origin: accounts.alice.address,
            data: {
                netuid: 1
            }
        });

        expect(reservedResult.success).toBe(true);
        if (reservedResult.success) {
            expect(reservedResult.value.response).toBe(0n);
        }
    });

    it("should return undefined for non-existent escrow", async () => {
        const { accounts } = context;

        const escrowResult = await contract.query("get_escrow", {
            origin: accounts.alice.address,
            data: {
                netuid: 1,
                listing_id: 1n,
                purchase_id: 1n
            }
        });

        expect(escrowResult.success).toBe(true);
        if (escrowResult.success) {
            expect(escrowResult.value.response).toBeUndefined();
        }
    });
});
