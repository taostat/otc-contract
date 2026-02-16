import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { Binary } from "polkadot-api";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    type LockupListingsContext,
    LockupListingsSdk
} from "../setup";

describe("Lockup Listings Contract - Pause/Resume", () => {
    let context: LockupListingsContext;
    let contract: ReturnType<LockupListingsSdk["getContract"]>;

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);
    }, 180000);

    afterAll(async () => {
        await cleanupTestEnvironment();
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

    it("should allow owner to pause trading", async () => {
        const { accounts } = context;

        const pauseTx = contract.send("pause_trading", {
            origin: accounts.alice.address,
            data: { reason: Binary.fromText("Testing pause") }
        });

        await pauseTx.signAndSubmit(accounts.alice.signer);

        const pauseStateResult = await contract.query("get_pause_state", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(pauseStateResult.success).toBe(true);
        if (pauseStateResult.success) {
            expect(pauseStateResult.value.response.type).toBe("TradingPaused");
        }
    }, 60000);

    it("should allow owner to resume from trading paused", async () => {
        const { accounts } = context;

        const resumeTx = contract.send("resume", {
            origin: accounts.alice.address,
            data: {}
        });

        await resumeTx.signAndSubmit(accounts.alice.signer);

        const pauseStateResult = await contract.query("get_pause_state", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(pauseStateResult.success).toBe(true);
        if (pauseStateResult.success) {
            expect(pauseStateResult.value.response.type).toBe("NotPaused");
        }
    }, 60000);

    it("should allow owner to pause fully", async () => {
        const { accounts } = context;

        const pauseTx = contract.send("pause_fully", {
            origin: accounts.alice.address,
            data: { reason: Binary.fromText("Emergency pause test") }
        });

        await pauseTx.signAndSubmit(accounts.alice.signer);

        const pauseStateResult = await contract.query("get_pause_state", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(pauseStateResult.success).toBe(true);
        if (pauseStateResult.success) {
            expect(pauseStateResult.value.response.type).toBe("FullyPaused");
        }
    }, 60000);

    it("should allow owner to resume from fully paused", async () => {
        const { accounts } = context;

        const resumeTx = contract.send("resume", {
            origin: accounts.alice.address,
            data: {}
        });

        await resumeTx.signAndSubmit(accounts.alice.signer);

        const pauseStateResult = await contract.query("get_pause_state", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(pauseStateResult.success).toBe(true);
        if (pauseStateResult.success) {
            expect(pauseStateResult.value.response.type).toBe("NotPaused");
        }
    }, 60000);

    it("should prevent non-owner from pausing trading", async () => {
        const { accounts } = context;

        const pauseTx = contract.send("pause_trading", {
            origin: accounts.bob.address,
            data: { reason: Binary.fromText("Unauthorized pause attempt") }
        });

        const result = await pauseTx.signAndSubmit(accounts.bob.signer);
        expect(result.dispatchError).toBeDefined();

        // Verify state hasn't changed
        const pauseStateResult = await contract.query("get_pause_state", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(pauseStateResult.success).toBe(true);
        if (pauseStateResult.success) {
            expect(pauseStateResult.value.response.type).toBe("NotPaused");
        }
    }, 60000);

    it("should prevent non-owner from pausing fully", async () => {
        const { accounts } = context;

        const pauseTx = contract.send("pause_fully", {
            origin: accounts.bob.address,
            data: { reason: Binary.fromText("Unauthorized full pause attempt") }
        });

        const result = await pauseTx.signAndSubmit(accounts.bob.signer);
        expect(result.dispatchError).toBeDefined();

        // Verify state hasn't changed
        const pauseStateResult = await contract.query("get_pause_state", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(pauseStateResult.success).toBe(true);
        if (pauseStateResult.success) {
            expect(pauseStateResult.value.response.type).toBe("NotPaused");
        }
    }, 60000);

    it("should prevent non-owner from resuming", async () => {
        const { accounts } = context;

        // First pause as owner
        const pauseTx = contract.send("pause_trading", {
            origin: accounts.alice.address,
            data: { reason: Binary.fromText("Pause for resume test") }
        });
        await pauseTx.signAndSubmit(accounts.alice.signer);

        // Try to resume as non-owner
        const resumeTx = contract.send("resume", {
            origin: accounts.bob.address,
            data: {}
        });

        const result = await resumeTx.signAndSubmit(accounts.bob.signer);
        expect(result.dispatchError).toBeDefined();

        // Verify state is still paused
        const pauseStateResult = await contract.query("get_pause_state", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(pauseStateResult.success).toBe(true);
        if (pauseStateResult.success) {
            expect(pauseStateResult.value.response.type).toBe("TradingPaused");
        }

        // Cleanup: resume as owner
        const cleanupResumeTx = contract.send("resume", {
            origin: accounts.alice.address,
            data: {}
        });
        await cleanupResumeTx.signAndSubmit(accounts.alice.signer);
    }, 60000);

    it("should allow admin functions when paused", async () => {
        const { accounts } = context;

        // Pause fully
        const pauseTx = contract.send("pause_fully", {
            origin: accounts.alice.address,
            data: { reason: Binary.fromText("Admin function test") }
        });
        await pauseTx.signAndSubmit(accounts.alice.signer);

        // Try to update min listing amount (should work)
        const updateTx = contract.send("update_min_listing_amount", {
            origin: accounts.alice.address,
            data: { new_amount: 2_000_000_000n }
        });

        await updateTx.signAndSubmit(accounts.alice.signer);

        // Verify update worked
        const minListingResult = await contract.query("get_min_listing_amount", {
            origin: accounts.alice.address,
            data: {}
        });

        expect(minListingResult.success).toBe(true);
        if (minListingResult.success) {
            expect(minListingResult.value.response).toBe(2_000_000_000n);
        }

        // Reset min listing amount
        const resetTx = contract.send("update_min_listing_amount", {
            origin: accounts.alice.address,
            data: { new_amount: 1_000_000_000n }
        });
        await resetTx.signAndSubmit(accounts.alice.signer);

        // Resume
        const resumeTx = contract.send("resume", {
            origin: accounts.alice.address,
            data: {}
        });
        await resumeTx.signAndSubmit(accounts.alice.signer);
    }, 120000);
});
