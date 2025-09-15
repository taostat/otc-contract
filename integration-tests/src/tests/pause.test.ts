import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { setupTestEnvironment, cleanupTestEnvironment, type TestContext, ContractSdk } from "../setup";
import { taoToRao, priceToFixedPoint } from "../utils";
import { stringToU8a } from "@polkadot/util";
import { Binary } from "polkadot-api";

describe("Pause Functionality", () => {
    let context: TestContext;
    let contract: ReturnType<ContractSdk["getContract"]>;

    beforeAll(async () => {
        context = await setupTestEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);
    }, 120000); // 2 minute timeout for setup

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    describe("Initial State", () => {
        it("should initialize in NotPaused state", async () => {
            const { accounts } = context;

            const result = await contract.query("get_pause_state", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(result.success).toBe(true);
            if (result.success) {
                expect(result.value.response.type).toBe("NotPaused");
            }
        });
    });

    describe("Pause Controls", () => {
        it("should allow owner to pause trading", async () => {
            const { accounts } = context;

            const pauseTx = contract.send("pause_trading", {
                origin: accounts.alice.address,
                data: {
                    reason: Binary.fromBytes(stringToU8a("maintenance"))
                }
            });

            const result = await pauseTx.signAndSubmit(accounts.alice.signer);
            expect(result.ok).toBe(true);

            // Verify state changed
            const stateResult = await contract.query("get_pause_state", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(stateResult.success).toBe(true);
            if (stateResult.success) {
                expect(stateResult.value.response.type).toBe("TradingPaused");
            }
        });

        it("should not allow non-owner to pause", async () => {
            const { accounts } = context;

            // First resume to reset state
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await resumeTx.signAndSubmit(accounts.alice.signer);

            // Bob tries to pause
            const pauseTx = contract.send("pause_trading", {
                origin: accounts.bob.address,
                data: {
                    reason: Binary.fromBytes(stringToU8a("unauthorized"))
                }
            });

            const result = await pauseTx.signAndSubmit(accounts.bob.signer);
            expect(result.ok).toBe(false);
            expect(result.dispatchError).toBeDefined();

            // Verify state is still NotPaused
            const stateResult = await contract.query("get_pause_state", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(stateResult.success).toBe(true);
            if (stateResult.success) {
                expect(stateResult.value.response.type).toBe("NotPaused");
            }
        });

        it("should allow owner to fully pause", async () => {
            const { accounts } = context;

            const pauseTx = contract.send("pause_fully", {
                origin: accounts.alice.address,
                data: {
                    reason: Binary.fromBytes(stringToU8a("emergency"))
                }
            });

            const result = await pauseTx.signAndSubmit(accounts.alice.signer);
            expect(result.ok).toBe(true);

            // Verify state changed
            const stateResult = await contract.query("get_pause_state", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(stateResult.success).toBe(true);
            if (stateResult.success) {
                expect(stateResult.value.response.type).toBe("FullyPaused");
            }
        });

        it("should allow owner to resume", async () => {
            const { accounts } = context;

            // Ensure it's paused first
            const pauseTx = contract.send("pause_fully", {
                origin: accounts.alice.address,
                data: {
                    reason: Binary.fromBytes(stringToU8a("test"))
                }
            });
            await pauseTx.signAndSubmit(accounts.alice.signer);

            // Now resume
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });

            const result = await resumeTx.signAndSubmit(accounts.alice.signer);
            expect(result.ok).toBe(true);

            // Verify state is back to NotPaused
            const stateResult = await contract.query("get_pause_state", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(stateResult.success).toBe(true);
            if (stateResult.success) {
                expect(stateResult.value.response.type).toBe("NotPaused");
            }
        });

        it("should not allow non-owner to resume", async () => {
            const { accounts } = context;

            // First pause as owner
            const pauseTx = contract.send("pause_trading", {
                origin: accounts.alice.address,
                data: {
                    reason: Binary.fromBytes(stringToU8a("test"))
                }
            });
            await pauseTx.signAndSubmit(accounts.alice.signer);

            // Bob tries to resume
            const resumeTx = contract.send("resume", {
                origin: accounts.bob.address,
                data: {}
            });

            const result = await resumeTx.signAndSubmit(accounts.bob.signer);
            expect(result.ok).toBe(false);
            expect(result.dispatchError).toBeDefined();

            // Verify state is still paused
            const stateResult = await contract.query("get_pause_state", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(stateResult.success).toBe(true);
            if (stateResult.success) {
                expect(stateResult.value.response.type).toBe("TradingPaused");
            }

            // Clean up - resume as owner
            const cleanupTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await cleanupTx.signAndSubmit(accounts.alice.signer);
        });
    });

    describe("Trading Paused Behavior", () => {
        beforeAll(async () => {
            const { accounts } = context;

            // Ensure contract is in TradingPaused state
            const pauseTx = contract.send("pause_trading", {
                origin: accounts.alice.address,
                data: {
                    reason: Binary.fromBytes(stringToU8a("test trading pause"))
                }
            });
            await pauseTx.signAndSubmit(accounts.alice.signer);
        });

        afterAll(async () => {
            const { accounts } = context;

            // Resume after tests
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await resumeTx.signAndSubmit(accounts.alice.signer);
        });

        it("should block creating TAO offers when trading paused", async () => {
            const { accounts } = context;

            // Try to create a TAO offer with TAO value
            const offerAmount = taoToRao(10); // 10 TAO
            const offerTx = contract.send("create_tao_offer", {
                origin: accounts.bob.address,
                value: offerAmount, // Send TAO with the transaction
                data: {
                    netuid: 1,
                    price: priceToFixedPoint(2.0)
                }
            });

            // Execute the transaction - should fail due to trading pause
            const result = await offerTx.signAndSubmit(accounts.bob.signer);

            // Transaction should fail due to trading being paused
            expect(result.ok).toBe(false);
            expect(result.dispatchError).toBeDefined();

            // The error should be a contract revert due to trading pause
            const contractsError = result.dispatchError?.value as any;
            expect(contractsError.type).toBe("Contracts");
            expect(contractsError.value.type).toBe("ContractReverted");
        });

        it("should allow canceling TAO offers when trading paused", async () => {
            const { accounts } = context;

            // Step 1: First ensure contract is NOT paused and create an offer
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await resumeTx.signAndSubmit(accounts.alice.signer);

            // Create a real TAO offer from Bob
            const offerAmount = taoToRao(20); // 20 TAO
            const createOfferTx = contract.send("create_tao_offer", {
                origin: accounts.bob.address,
                value: offerAmount,
                data: {
                    netuid: 1,
                    price: priceToFixedPoint(2.5)
                }
            });

            const createResult = await createOfferTx.signAndSubmit(accounts.bob.signer);
            expect(createResult.ok).toBe(true);

            // Get the offer ID
            const offersResult = await contract.query("get_user_offers", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.bob.address,
                    netuid: 1
                }
            });

            expect(offersResult.success).toBe(true);
            let offerId: bigint = 0n;
            if (offersResult.success) {
                expect(offersResult.value.response.length).toBeGreaterThan(0);
                offerId = offersResult.value.response[0];
            }

            // Step 2: Now pause trading
            const pauseTx = contract.send("pause_trading", {
                origin: accounts.alice.address,
                data: {
                    reason: Binary.fromBytes(stringToU8a("testing cancellation"))
                }
            });
            await pauseTx.signAndSubmit(accounts.alice.signer);

            // Verify contract is in TradingPaused state
            const stateResult = await contract.query("get_pause_state", {
                origin: accounts.alice.address,
                data: {}
            });
            expect(stateResult.success).toBe(true);
            if (stateResult.success) {
                expect(stateResult.value.response.type).toBe("TradingPaused");
            }

            // Step 3: Cancel the offer while trading is paused - this SHOULD work
            const cancelTx = contract.send("cancel_tao_offer", {
                origin: accounts.bob.address,
                data: {
                    netuid: 1,
                    offer_id: offerId
                }
            });

            const cancelResult = await cancelTx.signAndSubmit(accounts.bob.signer);

            // Cancellation should succeed even during trading pause
            expect(cancelResult.ok).toBe(true);

            // Verify offer was removed
            const offerAfterCancel = await contract.query("get_offer", {
                origin: accounts.alice.address,
                data: {
                    netuid: 1,
                    buyer: accounts.bob.address,
                    offer_id: offerId
                }
            });
            expect(offerAfterCancel.success).toBe(true);
            if (offerAfterCancel.success) {
                expect(offerAfterCancel.value.response).toBeUndefined(); // Offer should be gone
            }

            // Clean up - resume for next tests
            const cleanupResumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await cleanupResumeTx.signAndSubmit(accounts.alice.signer);
        });
    });

    describe("Fully Paused Behavior", () => {
        beforeAll(async () => {
            const { accounts } = context;

            // Ensure contract is in FullyPaused state
            const pauseTx = contract.send("pause_fully", {
                origin: accounts.alice.address,
                data: {
                    reason: Binary.fromBytes(stringToU8a("test full pause"))
                }
            });
            await pauseTx.signAndSubmit(accounts.alice.signer);
        });

        afterAll(async () => {
            const { accounts } = context;

            // Resume after tests
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await resumeTx.signAndSubmit(accounts.alice.signer);
        });

        it("should block all trading operations when fully paused", async () => {
            const { accounts } = context;

            // Try to create TAO offer
            const offerResult = await contract.query("create_tao_offer", {
                origin: accounts.bob.address,
                data: {
                    netuid: 1,
                    price: priceToFixedPoint(2.0)
                }
            });
            expect(offerResult.success).toBe(false);

            // Try to cancel (should also be blocked)
            const cancelResult = await contract.query("cancel_tao_offer", {
                origin: accounts.bob.address,
                data: {
                    netuid: 1,
                    offer_id: 1n
                }
            });
            expect(cancelResult.success).toBe(false);
        });

        it("should allow admin functions when fully paused", async () => {
            const { accounts } = context;

            // Test updating fee rate
            const newFeeRate = priceToFixedPoint(0.75); // Using priceToFixedPoint for consistency
            const feeResult = await contract.query("update_fee_rate", {
                origin: accounts.alice.address,
                data: { new_rate: newFeeRate }
            });
            expect(feeResult.success).toBe(true);

            // Test updating min listing amount
            const minResult = await contract.query("update_min_listing_amount", {
                origin: accounts.alice.address,
                data: { new_amount: taoToRao(2) }
            });
            expect(minResult.success).toBe(true);
        });
    });

    describe("State Transitions", () => {
        it("should transition between all pause states correctly", async () => {
            const { accounts } = context;

            // Start from NotPaused (resume if needed)
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await resumeTx.signAndSubmit(accounts.alice.signer);

            // Verify NotPaused
            let stateResult = await contract.query("get_pause_state", {
                origin: accounts.alice.address,
                data: {}
            });
            expect(stateResult.success).toBe(true);
            if (stateResult.success) {
                expect(stateResult.value.response.type).toBe("NotPaused");
            }

            // Transition to TradingPaused
            const tradingPauseTx = contract.send("pause_trading", {
                origin: accounts.alice.address,
                data: {
                    reason: Binary.fromBytes(stringToU8a("transition test 1"))
                }
            });
            await tradingPauseTx.signAndSubmit(accounts.alice.signer);

            stateResult = await contract.query("get_pause_state", {
                origin: accounts.alice.address,
                data: {}
            });
            expect(stateResult.success).toBe(true);
            if (stateResult.success) {
                expect(stateResult.value.response.type).toBe("TradingPaused");
            }

            // Transition to FullyPaused
            const fullyPauseTx = contract.send("pause_fully", {
                origin: accounts.alice.address,
                data: {
                    reason: Binary.fromBytes(stringToU8a("transition test 2"))
                }
            });
            await fullyPauseTx.signAndSubmit(accounts.alice.signer);

            stateResult = await contract.query("get_pause_state", {
                origin: accounts.alice.address,
                data: {}
            });
            expect(stateResult.success).toBe(true);
            if (stateResult.success) {
                expect(stateResult.value.response.type).toBe("FullyPaused");
            }

            // Transition back to NotPaused
            const finalResumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await finalResumeTx.signAndSubmit(accounts.alice.signer);

            stateResult = await contract.query("get_pause_state", {
                origin: accounts.alice.address,
                data: {}
            });
            expect(stateResult.success).toBe(true);
            if (stateResult.success) {
                expect(stateResult.value.response.type).toBe("NotPaused");
            }
        });

        it("should allow direct transition from FullyPaused to TradingPaused", async () => {
            const { accounts } = context;

            // Start in FullyPaused
            const fullyPauseTx = contract.send("pause_fully", {
                origin: accounts.alice.address,
                data: {
                    reason: Binary.fromBytes(stringToU8a("full pause"))
                }
            });
            await fullyPauseTx.signAndSubmit(accounts.alice.signer);

            // Transition directly to TradingPaused
            const tradingPauseTx = contract.send("pause_trading", {
                origin: accounts.alice.address,
                data: {
                    reason: Binary.fromBytes(stringToU8a("downgrade pause"))
                }
            });
            await tradingPauseTx.signAndSubmit(accounts.alice.signer);

            const stateResult = await contract.query("get_pause_state", {
                origin: accounts.alice.address,
                data: {}
            });
            expect(stateResult.success).toBe(true);
            if (stateResult.success) {
                expect(stateResult.value.response.type).toBe("TradingPaused");
            }

            // Clean up
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await resumeTx.signAndSubmit(accounts.alice.signer);
        });
    });
});
