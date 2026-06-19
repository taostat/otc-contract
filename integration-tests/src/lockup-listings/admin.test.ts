import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { Binary } from "polkadot-api";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    type LockupListingsContext,
    LockupListingsSdk
} from "../setup";
import { percentageToFixedPoint, fixedPointToPercentage } from "../utils";

describe("Lockup Listings Contract - Admin Functions", () => {
    let context: LockupListingsContext;
    let contract: ReturnType<LockupListingsSdk["getContract"]>;

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);
    }, 180000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    describe("update_owner", () => {
        it("should allow owner to transfer ownership", async () => {
            const { accounts } = context;

            // Transfer ownership to bob
            const updateTx = contract.send("update_owner", {
                origin: accounts.alice.address,
                data: { new_owner: accounts.bob.address }
            });

            await updateTx.signAndSubmit(accounts.alice.signer);

            // Verify new owner
            const ownerResult = await contract.query("get_owner", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(ownerResult.success).toBe(true);
            if (ownerResult.success) {
                expect(ownerResult.value.response).toBe(accounts.bob.address);
            }

            // Transfer ownership back to alice
            const revertTx = contract.send("update_owner", {
                origin: accounts.bob.address,
                data: { new_owner: accounts.alice.address }
            });

            await revertTx.signAndSubmit(accounts.bob.signer);
        }, 120000);

        it("should prevent non-owner from changing ownership", async () => {
            const { accounts } = context;

            const updateTx = contract.send("update_owner", {
                origin: accounts.bob.address,
                data: { new_owner: accounts.charlie.address }
            });

            const result = await updateTx.signAndSubmit(accounts.bob.signer);
            expect(result.dispatchError).toBeDefined();

            // Verify owner unchanged
            const ownerResult = await contract.query("get_owner", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(ownerResult.success).toBe(true);
            if (ownerResult.success) {
                expect(ownerResult.value.response).toBe(accounts.alice.address);
            }
        }, 60000);
    });

    describe("update_hotkey", () => {
        it("should allow owner to change hotkey", async () => {
            const { accounts } = context;

            const updateTx = contract.send("update_hotkey", {
                origin: accounts.alice.address,
                data: { new_hotkey: accounts.bob.address }
            });

            await updateTx.signAndSubmit(accounts.alice.signer);

            const hotkeyResult = await contract.query("get_hotkey", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(hotkeyResult.success).toBe(true);
            if (hotkeyResult.success) {
                expect(hotkeyResult.value.response).toBe(accounts.bob.address);
            }

            // Revert to original hotkey
            const revertTx = contract.send("update_hotkey", {
                origin: accounts.alice.address,
                data: { new_hotkey: accounts.alice.address }
            });

            await revertTx.signAndSubmit(accounts.alice.signer);
        }, 120000);

        it("should allow owner to change subnet-specific active hotkey", async () => {
            const { accounts } = context;

            const updateTx = contract.send("update_hotkey_for_subnet", {
                origin: accounts.alice.address,
                data: { netuid: 1, new_hotkey: accounts.bob.address }
            });

            await updateTx.signAndSubmit(accounts.alice.signer);

            const activeHotkeyResult = await contract.query("get_active_hotkey_for_subnet", {
                origin: accounts.alice.address,
                data: { netuid: 1 }
            });

            expect(activeHotkeyResult.success).toBe(true);
            if (activeHotkeyResult.success) {
                expect(activeHotkeyResult.value.response).toBe(accounts.bob.address);
            }

            const revertTx = contract.send("update_hotkey_for_subnet", {
                origin: accounts.alice.address,
                data: { netuid: 1, new_hotkey: accounts.alice.address }
            });

            await revertTx.signAndSubmit(accounts.alice.signer);
        }, 120000);

        it("should prevent non-owner from changing hotkey", async () => {
            const { accounts } = context;

            const updateTx = contract.send("update_hotkey", {
                origin: accounts.bob.address,
                data: { new_hotkey: accounts.charlie.address }
            });

            const result = await updateTx.signAndSubmit(accounts.bob.signer);
            expect(result.dispatchError).toBeDefined();
        }, 60000);
    });

    describe("update_fee_rate", () => {
        it("should allow owner to change fee rate", async () => {
            const { accounts } = context;

            const newFeeRate = percentageToFixedPoint(1.0); // 1%

            const updateTx = contract.send("update_fee_rate", {
                origin: accounts.alice.address,
                data: { new_rate: newFeeRate }
            });

            await updateTx.signAndSubmit(accounts.alice.signer);

            const feeRateResult = await contract.query("get_fee_rate", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(feeRateResult.success).toBe(true);
            if (feeRateResult.success) {
                const feePercentage = fixedPointToPercentage(feeRateResult.value.response);
                expect(feePercentage).toBeCloseTo(1.0, 5);
            }

            // Reset to original
            const resetTx = contract.send("update_fee_rate", {
                origin: accounts.alice.address,
                data: { new_rate: percentageToFixedPoint(0.5) }
            });

            await resetTx.signAndSubmit(accounts.alice.signer);
        }, 120000);

        it("should prevent non-owner from changing fee rate", async () => {
            const { accounts } = context;

            const updateTx = contract.send("update_fee_rate", {
                origin: accounts.bob.address,
                data: { new_rate: percentageToFixedPoint(5.0) }
            });

            const result = await updateTx.signAndSubmit(accounts.bob.signer);
            expect(result.dispatchError).toBeDefined();
        }, 60000);
    });

    describe("update_escrow_code_hash", () => {
        it("should allow owner to change escrow code hash", async () => {
            const { accounts, escrowCodeHash } = context;

            // Create a different hash for testing
            const newHash = Binary.fromHex("0x" + "ab".repeat(32));

            const updateTx = contract.send("update_escrow_code_hash", {
                origin: accounts.alice.address,
                data: { new_hash: newHash }
            });

            await updateTx.signAndSubmit(accounts.alice.signer);

            const codeHashResult = await contract.query("get_escrow_code_hash", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(codeHashResult.success).toBe(true);
            if (codeHashResult.success) {
                expect(codeHashResult.value.response.asHex()).toBe(newHash.asHex());
            }

            // Revert to original
            const revertTx = contract.send("update_escrow_code_hash", {
                origin: accounts.alice.address,
                data: { new_hash: Binary.fromHex(escrowCodeHash!) }
            });

            await revertTx.signAndSubmit(accounts.alice.signer);
        }, 120000);

        it("should prevent non-owner from changing escrow code hash", async () => {
            const { accounts } = context;

            const newHash = Binary.fromHex("0x" + "cd".repeat(32));

            const updateTx = contract.send("update_escrow_code_hash", {
                origin: accounts.bob.address,
                data: { new_hash: newHash }
            });

            const result = await updateTx.signAndSubmit(accounts.bob.signer);
            expect(result.dispatchError).toBeDefined();
        }, 60000);
    });

    describe("update_min_listing_amount", () => {
        it("should allow owner to change minimum listing amount", async () => {
            const { accounts } = context;

            const newAmount = 2_000_000_000n; // 2 Alpha

            const updateTx = contract.send("update_min_listing_amount", {
                origin: accounts.alice.address,
                data: { new_amount: newAmount }
            });

            await updateTx.signAndSubmit(accounts.alice.signer);

            const minAmountResult = await contract.query("get_min_listing_amount", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(minAmountResult.success).toBe(true);
            if (minAmountResult.success) {
                expect(minAmountResult.value.response).toBe(newAmount);
            }

            // Reset to original
            const resetTx = contract.send("update_min_listing_amount", {
                origin: accounts.alice.address,
                data: { new_amount: 1_000_000_000n }
            });

            await resetTx.signAndSubmit(accounts.alice.signer);
        }, 120000);

        it("should prevent non-owner from changing minimum listing amount", async () => {
            const { accounts } = context;

            const updateTx = contract.send("update_min_listing_amount", {
                origin: accounts.bob.address,
                data: { new_amount: 5_000_000_000n }
            });

            const result = await updateTx.signAndSubmit(accounts.bob.signer);
            expect(result.dispatchError).toBeDefined();
        }, 60000);
    });

    describe("update_min_purchase_amount", () => {
        it("should allow owner to change minimum purchase amount", async () => {
            const { accounts } = context;

            const newAmount = 500_000_000n; // 0.5 Alpha

            const updateTx = contract.send("update_min_purchase_amount", {
                origin: accounts.alice.address,
                data: { new_amount: newAmount }
            });

            await updateTx.signAndSubmit(accounts.alice.signer);

            const minAmountResult = await contract.query("get_min_purchase_amount", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(minAmountResult.success).toBe(true);
            if (minAmountResult.success) {
                expect(minAmountResult.value.response).toBe(newAmount);
            }

            // Reset to original
            const resetTx = contract.send("update_min_purchase_amount", {
                origin: accounts.alice.address,
                data: { new_amount: 100_000_000n }
            });

            await resetTx.signAndSubmit(accounts.alice.signer);
        }, 120000);

        it("should prevent non-owner from changing minimum purchase amount", async () => {
            const { accounts } = context;

            const updateTx = contract.send("update_min_purchase_amount", {
                origin: accounts.bob.address,
                data: { new_amount: 1_000_000_000n }
            });

            const result = await updateTx.signAndSubmit(accounts.bob.signer);
            expect(result.dispatchError).toBeDefined();
        }, 60000);
    });

    describe("update_lockup_duration_limits", () => {
        it("should allow owner to change lockup duration limits", async () => {
            const { accounts } = context;

            const newMin = 5;
            const newMax = 2_000_000;

            const updateTx = contract.send("update_lockup_duration_limits", {
                origin: accounts.alice.address,
                data: { new_min: newMin, new_max: newMax }
            });

            await updateTx.signAndSubmit(accounts.alice.signer);

            const limitsResult = await contract.query("get_lockup_duration_limits", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(limitsResult.success).toBe(true);
            if (limitsResult.success) {
                const [minDuration, maxDuration] = limitsResult.value.response;
                expect(minDuration).toBe(newMin);
                expect(maxDuration).toBe(newMax);
            }

            // Reset to original
            const resetTx = contract.send("update_lockup_duration_limits", {
                origin: accounts.alice.address,
                data: { new_min: 10, new_max: 1_000_000 }
            });

            await resetTx.signAndSubmit(accounts.alice.signer);
        }, 120000);

        it("should prevent non-owner from changing lockup duration limits", async () => {
            const { accounts } = context;

            const updateTx = contract.send("update_lockup_duration_limits", {
                origin: accounts.bob.address,
                data: { new_min: 1, new_max: 10_000_000 }
            });

            const result = await updateTx.signAndSubmit(accounts.bob.signer);
            expect(result.dispatchError).toBeDefined();
        }, 60000);
    });
});
