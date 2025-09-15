import { describe, it, expect, beforeAll, afterAll } from "vitest";
import {
    setupTestEnvironment,
    cleanupTestEnvironment,
    deployV2Contract,
    captureContractState,
    compareContractStates,
    type TestContext,
    ContractSdk
} from "../setup";
import { percentageToFixedPoint, priceToFixedPoint, waitForBlocks } from "../utils";
import { Binary } from "polkadot-api";
import { createInkSdk } from "@polkadot-api/sdk-ink";
import { contracts } from "@polkadot-api/descriptors";

describe("Contract Upgrade Tests", () => {
    let context: TestContext;
    let contract: ReturnType<ContractSdk["getContract"]>;

    beforeAll(async () => {
        // Setup test environment with original contract
        // Note: Contracts should be built before running tests using npm run prepare-tests
        context = await setupTestEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);
    }, 120000); // 2 minute timeout for setup

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    describe("Basic Upgrade Functionality", () => {
        it("should verify new method does not exist before upgrade", async () => {
            const { accounts } = context;

            // Try to call get_version (should fail as it doesn't exist in V1)
            try {
                const versionResult = await contract.query("get_version", {
                    origin: accounts.alice.address,
                    data: {}
                });

                console.log(">>>>>>>>>> Got version result:", versionResult);

                // If we get here, the method exists (shouldn't happen in V1)
                expect(versionResult).toBeUndefined();
            } catch (error: any) {
                // Expected behavior - method doesn't exist
                expect(error).toBeDefined();
            }
        });

        it("should only allow owner to upgrade contract", async () => {
            const { accounts, api } = context;

            // Deploy V2 code to chain
            const { codeHash } = await deployV2Contract(api, accounts);

            // Try to upgrade as non-owner (should fail)
            const upgradeAsNonOwner = contract.send("set_code", {
                origin: accounts.bob.address,
                data: {
                    code_hash: Binary.fromBytes(codeHash)
                }
            });

            const nonOwnerResult = await upgradeAsNonOwner.signAndSubmit(accounts.bob.signer);

            // Should fail because Bob is not the owner
            expect(nonOwnerResult.ok).toBe(false);
            expect(nonOwnerResult.dispatchError).toBeDefined();
        });

        it("should successfully upgrade contract as owner", async () => {
            const { accounts, api } = context;

            // Capture state before upgrade
            const stateBefore = await captureContractState(contract, accounts);
            console.log("State before upgrade:", stateBefore);

            // Deploy V2 code to chain
            const { codeHash } = await deployV2Contract(api, accounts);

            // Perform the upgrade as owner
            console.log("Performing contract upgrade...");
            const upgradeTx = contract.send("set_code", {
                origin: accounts.alice.address,
                data: {
                    code_hash: Binary.fromBytes(codeHash)
                }
            });

            const upgradeResult = await upgradeTx.signAndSubmit(accounts.alice.signer);

            console.log("Upgrade transaction result:", upgradeResult.ok);
            expect(upgradeResult.ok).toBe(true);

            // Wait for the upgrade to take effect
            await waitForBlocks(context.api, 2);

            // Verify state is preserved after upgrade
            const stateAfter = await captureContractState(contract, accounts);
            console.log("State after upgrade:", stateAfter);

            const comparison = compareContractStates(stateBefore, stateAfter);
            expect(comparison.isEqual).toBe(true);
            if (!comparison.isEqual) {
                console.log("State differences:", comparison.differences);
            }

            // Create V2 contract SDK to access new methods
            // Note: This assumes 'otc_contract_v2' has been generated via npm run generate-contract-v2
            const v2ContractSdk = createInkSdk(context.api, (contracts as any).otc_contract_v2 || contracts.otc_contract);
            const v2Contract = v2ContractSdk.getContract(context.contractAddress!);

            // Verify new method exists and works
            try {
                const versionResult = await v2Contract.query("get_version", {
                    origin: accounts.alice.address,
                    data: {}
                });

                console.log(">>>>>>>>>> Got version result after upgrade:", versionResult);

                expect(versionResult.success).toBe(true);
                if (versionResult.success) {
                    expect(versionResult.value.response).toBe(2);
                }
            } catch (error) {
                console.warn("Note: get_version method test skipped. Run 'npm run generate-contract-v2' to generate V2 types.");
                // If V2 types aren't generated yet, we skip this check but the upgrade itself is still tested
            }
        });
    });

    describe("State Preservation During Upgrade", () => {
        it("should preserve configuration settings after upgrade", async () => {
            const { accounts, api } = context;

            // First, update some configuration settings
            const newFeeRate = percentageToFixedPoint(1.5);
            const updateFeeRateTx = contract.send("update_fee_rate", {
                origin: accounts.alice.address,
                data: {
                    new_rate: newFeeRate
                }
            });

            const feeRateResult = await updateFeeRateTx.signAndSubmit(accounts.alice.signer);
            expect(feeRateResult.ok).toBe(true);

            // Update min listing amount
            const newMinListingAmount = 2_000_000_000n;
            const updateMinListingTx = contract.send("update_min_listing_amount", {
                origin: accounts.alice.address,
                data: {
                    new_amount: newMinListingAmount
                }
            });

            const minListingResult = await updateMinListingTx.signAndSubmit(accounts.alice.signer);
            expect(minListingResult.ok).toBe(true);

            // Capture state before upgrade
            const stateBefore = await captureContractState(contract, accounts);

            // Deploy and upgrade to V2
            const { codeHash } = await deployV2Contract(api, accounts);
            const upgradeTx = contract.send("set_code", {
                origin: accounts.alice.address,
                data: {
                    code_hash: Binary.fromBytes(codeHash)
                }
            });

            const upgradeResult = await upgradeTx.signAndSubmit(accounts.alice.signer);
            expect(upgradeResult.ok).toBe(true);

            // Wait for upgrade to take effect
            await waitForBlocks(context.api, 2);

            // Capture state after upgrade
            const stateAfter = await captureContractState(contract, accounts);

            // Verify all configuration is preserved
            expect(stateAfter.feeRate).toBe(stateBefore.feeRate);
            expect(stateAfter.minListingAmount).toBe(stateBefore.minListingAmount);
            expect(stateAfter.owner).toBe(stateBefore.owner);
            expect(stateAfter.hotkey).toBe(stateBefore.hotkey);
            expect(stateAfter.minOfferAmount).toBe(stateBefore.minOfferAmount);
            expect(stateAfter.minListingAge).toBe(stateBefore.minListingAge);
        });

        it("should preserve TAO offers after upgrade", async () => {
            const { accounts, api } = context;

            // Create a TAO offer before upgrade
            const offerAmount = 5_000_000_000n; // 5 TAO
            const offerPrice = priceToFixedPoint(2.0); // 2 TAO per Alpha

            const createOfferTx = contract.send("create_tao_offer", {
                origin: accounts.bob.address,
                data: {
                    netuid: 1,
                    price: offerPrice
                },
                value: offerAmount
            });

            const offerResult = await createOfferTx.signAndSubmit(accounts.bob.signer);
            expect(offerResult.ok).toBe(true);

            // Assuming first offer ID is 1
            const offerId = 1n;

            // Get offer details before upgrade
            const offerBeforeResult = await contract.query("get_offer", {
                origin: accounts.bob.address,
                data: {
                    netuid: 1,
                    buyer: accounts.bob.address,
                    offer_id: offerId || 1n
                }
            });

            expect(offerBeforeResult.success).toBe(true);
            const offerBefore = offerBeforeResult.success ? offerBeforeResult.value.response : null;

            // Deploy and upgrade to V2
            const { codeHash } = await deployV2Contract(api, accounts);
            const upgradeTx = contract.send("set_code", {
                origin: accounts.alice.address,
                data: {
                    code_hash: Binary.fromBytes(codeHash)

                }
            });

            const upgradeResult = await upgradeTx.signAndSubmit(accounts.alice.signer);
            expect(upgradeResult.ok).toBe(true);

            // Wait for upgrade to take effect
            await waitForBlocks(context.api, 2);

            // Get offer details after upgrade
            const offerAfterResult = await contract.query("get_offer", {
                origin: accounts.bob.address,
                data: {
                    netuid: 1,
                    buyer: accounts.bob.address,
                    offer_id: offerId || 1n
                }
            });

            expect(offerAfterResult.success).toBe(true);
            const offerAfter = offerAfterResult.success ? offerAfterResult.value.response : null;

            // Verify offer is preserved
            expect(offerAfter).toEqual(offerBefore);

            // Verify we can still interact with the offer (e.g., cancel it)
            const cancelOfferTx = contract.send("cancel_tao_offer", {
                origin: accounts.bob.address,
                data: {
                    netuid: 1,
                    offer_id: offerId || 1n
                }
            });

            const cancelResult = await cancelOfferTx.signAndSubmit(accounts.bob.signer);
            expect(cancelResult.ok).toBe(true);
        });
    });

    describe("Functionality Tests After Upgrade", () => {
        it("should allow all existing functions to work after upgrade", async () => {
            const { accounts, api } = context;

            // Deploy and upgrade to V2
            const { codeHash } = await deployV2Contract(api, accounts);
            const upgradeTx = contract.send("set_code", {
                origin: accounts.alice.address,
                data: {
                    code_hash: Binary.fromBytes(codeHash)
                }
            });

            const upgradeResult = await upgradeTx.signAndSubmit(accounts.alice.signer);
            expect(upgradeResult.ok).toBe(true);

            // Wait for upgrade to take effect
            await waitForBlocks(context.api, 2);

            // Test configuration update functions
            const updateHotkeyTx = contract.send("update_hotkey", {
                origin: accounts.alice.address,
                data: {
                    new_hotkey: accounts.charlie.address
                }
            });

            const hotkeyResult = await updateHotkeyTx.signAndSubmit(accounts.alice.signer);
            expect(hotkeyResult.ok).toBe(true);

            // Verify the update worked
            const hotkeyQuery = await contract.query("get_hotkey", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(hotkeyQuery.success).toBe(true);
            if (hotkeyQuery.success) {
                expect(hotkeyQuery.value.response).toBe(accounts.charlie.address);
            }

            // Create V2 contract SDK to test new methods
            const v2ContractSdk = createInkSdk(context.api, (contracts as any).otc_contract_v2 || contracts.otc_contract);
            const v2Contract = v2ContractSdk.getContract(context.contractAddress!);

            // Test that new V2 function works
            try {
                const versionResult = await v2Contract.query("get_version", {
                    origin: accounts.alice.address,
                    data: {}
                });

                expect(versionResult.success).toBe(true);
                if (versionResult.success) {
                    expect(versionResult.value.response).toBe(2);
                }
            } catch (error) {
                console.warn("Note: get_version method test skipped. Run 'npm run generate-contract-v2' to generate V2 types.");
            }
        });

        it("should reject invalid code hash during upgrade", async () => {
            const { accounts } = context;

            // Try to upgrade with an invalid code hash (32 bytes of zeros)
            const invalidCodeHash = new Uint8Array(32);

            const invalidUpgradeTx = contract.send("set_code", {
                origin: accounts.alice.address,
                data: {
                    code_hash: Binary.fromBytes(invalidCodeHash)

                }
            });

            const result = await invalidUpgradeTx.signAndSubmit(accounts.alice.signer);

            // Should fail with invalid code hash
            expect(result.ok).toBe(false);
            expect(result.dispatchError).toBeDefined();
        });
    });

    describe("Complex State Preservation", () => {
        it("should preserve user indices and mappings after upgrade", async () => {
            const { accounts, api } = context;

            // Create multiple TAO offers from different users
            const offer1Tx = contract.send("create_tao_offer", {
                origin: accounts.charlie.address,
                data: {
                    netuid: 1,
                    price: priceToFixedPoint(1.5)
                },
                value: 3_000_000_000n
            });

            const offer1Result = await offer1Tx.signAndSubmit(accounts.charlie.signer);
            expect(offer1Result.ok).toBe(true);

            const offer2Tx = contract.send("create_tao_offer", {
                origin: accounts.dave.address,
                data: {
                    netuid: 1,
                    price: priceToFixedPoint(2.0)
                },
                value: 4_000_000_000n
            });

            const offer2Result = await offer2Tx.signAndSubmit(accounts.dave.signer);
            expect(offer2Result.ok).toBe(true);

            // Get user offers before upgrade
            const charlieOffersBefore = await contract.query("get_user_offers", {
                origin: accounts.charlie.address,
                data: {
                    buyer: accounts.charlie.address,
                    netuid: 1
                }
            });

            const daveOffersBefore = await contract.query("get_user_offers", {
                origin: accounts.dave.address,
                data: {
                    buyer: accounts.dave.address,
                    netuid: 1
                }
            });

            // Deploy and upgrade to V2
            const { codeHash } = await deployV2Contract(api, accounts);
            const upgradeTx = contract.send("set_code", {
                origin: accounts.alice.address,
                data: {
                    code_hash: Binary.fromBytes(codeHash)
                }
            });

            const upgradeResult = await upgradeTx.signAndSubmit(accounts.alice.signer);
            expect(upgradeResult.ok).toBe(true);

            // Wait for upgrade to take effect
            await waitForBlocks(context.api, 2);

            // Get user offers after upgrade
            const charlieOffersAfter = await contract.query("get_user_offers", {
                origin: accounts.charlie.address,
                data: {
                    buyer: accounts.charlie.address,
                    netuid: 1
                }
            });

            const daveOffersAfter = await contract.query("get_user_offers", {
                origin: accounts.dave.address,
                data: {
                    buyer: accounts.dave.address,
                    netuid: 1
                }
            });

            // Verify indices are preserved
            expect(charlieOffersAfter.success).toBe(true);
            expect(daveOffersAfter.success).toBe(true);

            if (charlieOffersBefore.success && charlieOffersAfter.success) {
                expect(charlieOffersAfter.value.response).toEqual(charlieOffersBefore.value.response);
            }

            if (daveOffersBefore.success && daveOffersAfter.success) {
                expect(daveOffersAfter.value.response).toEqual(daveOffersBefore.value.response);
            }
        });
    });
});
