import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { TxEventsPayload } from "polkadot-api";
import { setupTestEnvironment, cleanupTestEnvironment, type TestContext, ContractSdk, bigintReplacer } from "../setup";
import {
    taoToRao,
    waitForBlocks,
    fundAccount,
    registerSubnet,
    registerValidator,
    addContractAsProxy,
    createHotkey,
    elevateRegistrationLimits,
    MARKET_PRICE,
    bpsToPercentage,
    type Wallet,
} from "../utils";
import {
    getStakeBalance,
    hasProxyPermission,
    formatStakeAmount,
} from "../utils/stake-helpers";

type EventWithTopics = TxEventsPayload["events"][number];

type ContractsError = {
    type: 'Contracts',
    value: { type: 'ContractReverted', value: undefined }
}

describe("Alpha Listing Operations", () => {
    let context: TestContext;
    let contract: ReturnType<ContractSdk["getContract"]>;
    let netuid: number;

    let bobHotkey: Wallet;
    let charlieWallet: Wallet;
    let daveHotkey: Wallet;

    // Additional hotkeys for verification tests
    let eveHotkey: Wallet;
    let charlieHotkey: Wallet;
    let charlieSpecialHotkey: Wallet;
    let daveHotkey2: Wallet;

    beforeAll(async () => {
        context = await setupTestEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);

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
        // The contract uses Alice as its hotkey, so we need to register it on the subnet
        const contractHotkeyResult = await contract.query("get_hotkey", {
            origin: context.accounts.alice.address,
            data: {}
        });

        if (contractHotkeyResult.success) {
            const contractHotkey = contractHotkeyResult.value.response;
            console.log(`Contract hotkey: ${contractHotkey}`);

            // Fund the contract's account for transaction fees if needed
            await fundAccount(context.api, context.contractAddress!, taoToRao(10), context.accounts.alice.signer);

            // Register the contract's hotkey as a validator with some initial stake
            // This is necessary for the contract to be able to receive stake transfers
            await registerValidator(
                context.api,
                netuid,
                contractHotkey,
                context.accounts.alice.signer,
                taoToRao(50) // Initial stake for the contract's hotkey
            );
            console.log(`Registered contract's hotkey as validator with 50 Alpha stake`);
        }

        // Register Bob, Charlie, and Dave as validators with initial stake
        bobHotkey = createHotkey("//Bob");
        charlieWallet = createHotkey("//Charlie");
        daveHotkey = createHotkey("//Dave");

        // Fund hotkeys for transaction fees
        await fundAccount(context.api, bobHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, charlieWallet.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, daveHotkey.address, taoToRao(1), context.accounts.alice.signer);

        // Register validators and add initial stake
        await registerValidator(
            context.api,
            netuid,
            bobHotkey.address,
            context.accounts.bob.signer,
            taoToRao(100) // 100 Alpha initial stake
        );
        console.log(`✓ Bob's validator registered successfully`);

        console.log(`Registering Charlie's validator...`);
        await registerValidator(
            context.api,
            netuid,
            charlieWallet.address,
            context.accounts.charlie.signer,
            taoToRao(150) // 150 Alpha initial stake
        );
        console.log(`✓ Charlie's validator registered successfully`);

        console.log(`Registering Dave's validator...`);
        await registerValidator(
            context.api,
            netuid,
            daveHotkey.address,
            context.accounts.dave.signer,
            taoToRao(100) // 100 Alpha initial stake for Dave
        );
        console.log(`✓ Dave's validator registered successfully`);

        // Wait for registrations to be processed
        await waitForBlocks(context.api, 2);

        // Verify initial stakes (pass coldkey for accurate balance)
        const bobStake = await getStakeBalance(context.api, bobHotkey.address, netuid, context.accounts.bob.address);
        const charlieStake = await getStakeBalance(context.api, charlieWallet.address, netuid, context.accounts.charlie.address);
        const daveStake = await getStakeBalance(context.api, daveHotkey.address, netuid, context.accounts.dave.address);

        console.log(`Bob's initial stake: ${formatStakeAmount(bobStake)}`);
        console.log(`Charlie's initial stake: ${formatStakeAmount(charlieStake)}`);
        console.log(`Dave's initial stake: ${formatStakeAmount(daveStake)}`);

        // Set up additional hotkeys for dual verification tests
        // Create additional hotkeys for test accounts
        eveHotkey = createHotkey("//Eve");
        charlieHotkey = createHotkey("//Charlie/hotkey2");
        charlieSpecialHotkey = createHotkey("//Charlie/special");
        daveHotkey2 = createHotkey("//Dave/hotkey2");

        // Fund hotkeys for transaction fees
        await fundAccount(context.api, eveHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, charlieHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, charlieSpecialHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, daveHotkey2.address, taoToRao(1), context.accounts.alice.signer);

        // Register validators with specific stake amounts for testing
        // Eve: 30 Alpha (for insufficient stake test)
        await registerValidator(
            context.api,
            netuid,
            eveHotkey.address,
            context.accounts.eve.signer,
            taoToRao(30)
        );

        // Charlie: 100 Alpha on special hotkey (for consolidation test)
        // Note: Charlie already has 150 Alpha on charlieWallet from initial setup
        await registerValidator(
            context.api,
            netuid,
            charlieSpecialHotkey.address,
            context.accounts.charlie.signer,
            taoToRao(100)
        );

        // Dave: 100 Alpha on second hotkey (for minimum amount test)
        // Note: Dave already has 100 Alpha on daveHotkey from initial setup
        await registerValidator(
            context.api,
            netuid,
            daveHotkey2.address,
            context.accounts.dave.signer,
            taoToRao(100)
        );

        // Wait for all registrations to be processed
        await waitForBlocks(context.api, 2);

        // Verify the new test accounts' stakes
        const eveStake = await getStakeBalance(context.api, eveHotkey.address, netuid, context.accounts.eve.address);
        const charlieSpecialStake = await getStakeBalance(context.api, charlieSpecialHotkey.address, netuid, context.accounts.charlie.address);
        const daveStake2 = await getStakeBalance(context.api, daveHotkey2.address, netuid, context.accounts.dave.address);

        console.log(`Eve's initial stake: ${formatStakeAmount(eveStake)}`);
        console.log(`Charlie's stake on special hotkey: ${formatStakeAmount(charlieSpecialStake)}`);
        console.log(`Dave's stake on second hotkey: ${formatStakeAmount(daveStake2)}`);
    }, 180000); // 3 minute timeout for setup

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    describe("Listing Creation", () => {
        it("should successfully list Alpha after proxy setup", async () => {
            const { accounts } = context;

            // Bob adds contract as proxy
            await addContractAsProxy(context.api, context.contractAddress!, context.accounts.bob.signer);

            // Wait for proxy to be registered
            await waitForBlocks(context.api, 2);

            // Verify proxy was added
            const hasProxy = await hasProxyPermission(
                context.api,
                accounts.bob.address,
                context.contractAddress!
            );
            console.log(`Bob has proxy permission: ${hasProxy}`);
            expect(hasProxy).toBe(true);

            // Additional debug: check proxy details
            const proxies = await context.api.query.Proxy.Proxies.getValue(accounts.bob.address);
            console.log(`Bob's proxies:`, proxies);
            if (proxies && proxies[0]) {
                proxies[0].forEach((proxy: any) => {
                    console.log(`  - Delegate: ${proxy.delegate}, Type: ${JSON.stringify(proxy.proxy_type)}, Delay: ${proxy.delay}`);
                });
            }

            // Get Bob's stake before listing
            const stakeBefore = await getStakeBalance(context.api, bobHotkey.address, netuid, accounts.bob.address);
            console.log(`Bob's stake before listing: ${formatStakeAmount(stakeBefore)}`);

            // Bob lists 50 Alpha at market price (0% offset)
            const listAmount = taoToRao(50);
            const priceOffsetBps = MARKET_PRICE; // 0% offset = market price

            console.log(`Attempting to list ${formatStakeAmount(listAmount)} at ${bpsToPercentage(priceOffsetBps)}% offset from market price`);

            const listTx = contract.send("list_alpha", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: priceOffsetBps
                }
            });

            const result = await listTx.signAndSubmit(accounts.bob.signer);

            console.log("Listing transaction result:", {
                ok: result.ok,
                dispatchError: result.dispatchError,
            });

            console.log("Events emitted during transaction:", {
                events: JSON.stringify(contract.filterEvents(result.events), bigintReplacer, 2),
            });

            if (!result.ok) {
                console.log("Full error details:", JSON.stringify(result.dispatchError, null, 2));
                if (result.dispatchError?.value) {
                    console.log("Contract error type:", result.dispatchError.value);
                }
            }

            expect(result.ok).toBe(true);

            // Wait for transaction to be processed
            await waitForBlocks(context.api, 2);

            // Get Bob's stake after listing
            const stakeAfter = await getStakeBalance(context.api, bobHotkey.address, netuid, accounts.bob.address);
            console.log(`Bob's stake after listing: ${formatStakeAmount(stakeAfter)}`);

            // Verify stake was transferred (should be reduced by listing amount)
            // Account for potential staking rewards by checking the stake decreased by at least the listing amount
            const stakeReduction = stakeBefore - stakeAfter;
            const tolerance = taoToRao(5); // Allow up to 5 Alpha variance for staking rewards

            console.log(`Stake reduction: ${formatStakeAmount(stakeReduction)} (expected: ${formatStakeAmount(listAmount)})`);

            // The stake should have decreased by approximately the listing amount
            expect(stakeReduction).toBeGreaterThanOrEqual(listAmount - tolerance);
            expect(stakeReduction).toBeLessThanOrEqual(listAmount + tolerance);

            // Query the listing to verify it was created
            const listingResult = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.bob.address,
                    netuid
                }
            });

            expect(listingResult.success).toBe(true);

            if (listingResult.success) {
                expect(listingResult.value.response).toBeDefined();
                expect(listingResult.value.response.length).toBeGreaterThan(0);

                // Get the listing details
                const listingId = listingResult.value.response[0];
                const listing = await contract.query("get_listing", {
                    origin: accounts.alice.address,
                    data: {
                        netuid,
                        seller: accounts.bob.address,
                        listing_id: listingId
                    }
                });

                expect(listing.success).toBe(true);

                if (listing.success && listing.value.response) {
                    expect(listing.value.response.amount).toBe(listAmount);
                    expect(listing.value.response.price_offset_bps).toBe(priceOffsetBps);
                    expect(listing.value.response.seller).toBe(accounts.bob.address);
                    expect(listing.value.response.netuid).toBe(netuid);
                }
            }
        });

        it("should fail to list Alpha without proxy setup", async () => {
            const { accounts } = context;

            // First check if Dave already has proxy (from previous test runs)
            const hasDaveProxy = await hasProxyPermission(
                context.api,
                accounts.dave.address,
                context.contractAddress!
            );

            if (hasDaveProxy) {
                console.log("Dave already has proxy from previous test run, skipping test");
                return; // Skip test if Dave already has proxy
            }

            // Dave tries to list without setting up proxy
            const listTx = contract.send("list_alpha", {
                origin: accounts.dave.address,
                data: {
                    hotkey: daveHotkey.address,
                    netuid,
                    amount: taoToRao(50),
                    price_offset_bps: MARKET_PRICE // Market price
                }
            });

            const result = await listTx.signAndSubmit(accounts.dave.signer);

            console.log("Transaction result for Dave (no proxy):", result);

            // Should fail because Dave has no proxy set up
            expect(result.ok).toBe(false);
            expect(result.dispatchError?.type).toContain("Module");
        });
    });

    describe("Verification Logic", () => {
        it("should fail when seller has insufficient stake", async () => {
            const { accounts } = context;
            // Eve has 30 or less Alpha, tries to list 50 Alpha
            const listAmount = taoToRao(50);
            const priceOffsetBps = MARKET_PRICE;
            // First add Eve as proxy (required for the call to be made)
            await addContractAsProxy(context.api, context.contractAddress!, accounts.eve.signer);
            await waitForBlocks(context.api, 2);

            const eveStakeBefore = await getStakeBalance(context.api, eveHotkey.address, netuid, accounts.eve.address);
            console.log(`Eve attempting to list ${formatStakeAmount(listAmount)} with only ${formatStakeAmount(eveStakeBefore)} available`);

            const listTx = contract.send("list_alpha", {
                origin: accounts.eve.address,
                data: {
                    hotkey: eveHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: priceOffsetBps
                }
            });

            console.log(`Eve's stake before insufficient stake attempt: ${formatStakeAmount(eveStakeBefore)}`);

            const result = await listTx.signAndSubmit(accounts.eve.signer);

            const eveStakeAfter = await getStakeBalance(context.api, eveHotkey.address, netuid, accounts.eve.address);

            // Should fail with ContractReverted error
            expect(result.ok).toBe(false);
            const contractsError = result.dispatchError?.value as ContractsError;
            expect(contractsError.type).toBe("Contracts");
            expect(contractsError.value.type).toBe("ContractReverted");

            // Verify Eve's balance was not reduced
            expect(eveStakeAfter).toBeGreaterThanOrEqual(eveStakeBefore);
        });

        it("should verify stake consolidation when hotkey differs", async () => {
            const { accounts } = context;

            // Charlie uses a different hotkey than the contract's
            await addContractAsProxy(context.api, context.contractAddress!, accounts.charlie.signer);
            await waitForBlocks(context.api, 2);

            // Get contract's main hotkey for comparison
            const contractHotkeyResult = await contract.query("get_hotkey", {
                origin: accounts.alice.address,
                data: {}
            });
            if (contractHotkeyResult.success === false) {
                throw new Error("Failed to get contract hotkey");
            }
            const contractHotkey = contractHotkeyResult.value.response;

            console.log(`Contract's main hotkey: ${contractHotkey}`);
            console.log(`Charlie's special hotkey: ${charlieSpecialHotkey.address}`);

            const listAmount = taoToRao(40);

            // List with special hotkey (different from contract's hotkey)
            const listTx = contract.send("list_alpha", {
                origin: accounts.charlie.address,
                data: {
                    hotkey: charlieSpecialHotkey.address, // Different from contract's hotkey
                    netuid,
                    amount: listAmount,
                    price_offset_bps: MARKET_PRICE
                }
            });

            const result = await listTx.signAndSubmit(accounts.charlie.signer);

            console.log("Consolidation result:", result.ok);
            expect(result.ok).toBe(true);

            // Find and verify the StakeMoved event
            const stakeMovedEvent = result.events.find((event: EventWithTopics) =>
                event.type === "SubtensorModule" &&
                event.value.type === "StakeMoved"
            );

            expect(stakeMovedEvent).toBeDefined();

            if (stakeMovedEvent) {
                const [coldkey, originHotkey, originNetuid, destHotkey, destNetuid, _amount] = stakeMovedEvent.value.value;

                console.log("StakeMoved event details:", {
                    coldkey,
                    originHotkey,
                    originNetuid,
                    destHotkey,
                    destNetuid
                });

                // Verify the stake was moved from Charlie's special hotkey to contract's main hotkey
                expect(coldkey).toBe(context.contractAddress);
                expect(originHotkey).toBe(charlieSpecialHotkey.address);
                expect(originNetuid).toBe(netuid);
                expect(destHotkey).toBe(contractHotkey);
                expect(destNetuid).toBe(netuid);
            }
        });

        it("should enforce minimum listing amount", async () => {
            const { accounts } = context;

            // Dave uses his second hotkey for this test
            // Check if Dave already has proxy from previous test runs
            const hasDaveProxy = await hasProxyPermission(
                context.api,
                accounts.dave.address,
                context.contractAddress!
            );

            if (!hasDaveProxy) {
                await addContractAsProxy(context.api, context.contractAddress!, accounts.dave.signer);
                await waitForBlocks(context.api, 2);
            }

            // Get minimum amount from contract
            const minAmountResult = await contract.query("get_min_listing_amount", {
                origin: accounts.alice.address,
                data: {}
            });
            if (!minAmountResult.success) {
                throw new Error("Failed to get minimum listing amount from contract");
            }
            const minAmount = minAmountResult.value.response;

            console.log(`Minimum listing amount: ${formatStakeAmount(minAmount)}`);

            // Test 1: Below minimum fails
            const belowMin = minAmount - 1n;
            console.log(`Attempting to list below minimum: ${formatStakeAmount(belowMin)}`);

            const failTx = contract.send("list_alpha", {
                origin: accounts.dave.address,
                data: {
                    hotkey: daveHotkey2.address,
                    netuid,
                    amount: belowMin,
                    price_offset_bps: MARKET_PRICE
                }
            });

            const failResult = await failTx.signAndSubmit(accounts.dave.signer);

            expect(failResult.ok).toBe(false);
            const contractsError = failResult.dispatchError?.value as ContractsError;
            expect(contractsError.type).toBe("Contracts");
            expect(contractsError.value.type).toBe("ContractReverted");

            // Test 2: Exactly minimum succeeds
            console.log(`Attempting to list exactly minimum: ${formatStakeAmount(minAmount)}`);

            const exactTx = contract.send("list_alpha", {
                origin: accounts.dave.address,
                data: {
                    hotkey: daveHotkey2.address,
                    netuid,
                    amount: minAmount,
                    price_offset_bps: MARKET_PRICE
                }
            });

            const exactResult = await exactTx.signAndSubmit(accounts.dave.signer);

            expect(exactResult.ok).toBe(true);

            // Verify Dave has at least one listing (may have more from previous test runs)
            const listings = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.dave.address,
                    netuid
                }
            });

            expect(listings.success).toBe(true);
            if (listings.success) {
                expect(listings.value.response.length).toBeGreaterThanOrEqual(1);

                // Find the listing we just created (should be the last one)
                const lastListingId = listings.value.response[listings.value.response.length - 1];
                const listing = await contract.query("get_listing", {
                    origin: accounts.alice.address,
                    data: {
                        netuid,
                        seller: accounts.dave.address,
                        listing_id: lastListingId
                    }
                });

                // Verify it's the minimum amount listing we just created
                expect(listing.success).toBe(true);
                if (listing.success && listing.value.response) {
                    const minAmountResult = await contract.query("get_min_listing_amount", {
                        origin: accounts.alice.address,
                        data: {}
                    });
                    if (minAmountResult.success) {
                        expect(listing.value.response.amount).toBe(minAmountResult.value.response);
                    }
                }
            }
        });
    });
});
