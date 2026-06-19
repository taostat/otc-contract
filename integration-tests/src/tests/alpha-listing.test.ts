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
import {
    createAlphaListing,
    queryOk,
    submitReverted,
} from "../test-helpers";

type EventWithTopics = TxEventsPayload["events"][number];

type ContractsError = {
    type: 'Contracts',
    value: { type: 'ContractReverted', value: undefined }
}

const PRIMARY_LISTING_AMOUNT = taoToRao(0.02);
const SECONDARY_LISTING_AMOUNT = taoToRao(0.015);
const INSUFFICIENT_LISTING_AMOUNT = taoToRao(0.5);
const PRIMARY_REQUIRED_ALPHA = taoToRao(0.05);
const SECONDARY_REQUIRED_ALPHA = taoToRao(0.03);

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
            taoToRao(5000),
            PRIMARY_REQUIRED_ALPHA
        );
        console.log(`✓ Bob's validator registered successfully`);

        console.log(`Registering Charlie's validator...`);
        await registerValidator(
            context.api,
            netuid,
            charlieWallet.address,
            context.accounts.charlie.signer,
            taoToRao(5000),
            PRIMARY_REQUIRED_ALPHA
        );
        console.log(`✓ Charlie's validator registered successfully`);

        console.log(`Registering Dave's validator...`);
        await registerValidator(
            context.api,
            netuid,
            daveHotkey.address,
            context.accounts.dave.signer,
            taoToRao(5000),
            PRIMARY_REQUIRED_ALPHA
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

        // Charlie: plenty of Alpha on special hotkey (for consolidation test)
        await registerValidator(
            context.api,
            netuid,
            charlieSpecialHotkey.address,
            context.accounts.charlie.signer,
            taoToRao(5000),
            SECONDARY_REQUIRED_ALPHA
        );

        // Dave: plenty of Alpha on second hotkey (for minimum amount test)
        await registerValidator(
            context.api,
            netuid,
            daveHotkey2.address,
            context.accounts.dave.signer,
            taoToRao(5000),
            SECONDARY_REQUIRED_ALPHA
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

            // Bob lists a small but valid amount at market price (0% offset).
            const listAmount = PRIMARY_LISTING_AMOUNT;
            const priceOffsetBps = MARKET_PRICE; // 0% offset = market price
            const reservedBeforeResult = await contract.query("get_reserved_alpha", {
                origin: accounts.alice.address,
                data: { netuid },
            });
            expect(reservedBeforeResult.success).toBe(true);
            const reservedBefore = reservedBeforeResult.success
                ? BigInt(reservedBeforeResult.value.response)
                : 0n;

            console.log(`Attempting to list ${formatStakeAmount(listAmount)} at ${bpsToPercentage(priceOffsetBps)}% offset from market price`);

            const { result, listingId, event } = await createAlphaListing(contract, accounts.bob.signer, accounts.bob.address, {
                hotkey: bobHotkey.address,
                netuid,
                amount: listAmount,
                price_offset_bps: priceOffsetBps
            });
            console.log("Events emitted during transaction:", {
                events: JSON.stringify(contract.filterEvents(result.events), bigintReplacer, 2),
            });
            expect(event.value.seller).toBe(accounts.bob.address);
            expect(event.value.netuid).toBe(netuid);
            expect(BigInt(event.value.amount)).toBe(listAmount);

            // Get Bob's stake after listing
            const stakeAfter = await getStakeBalance(context.api, bobHotkey.address, netuid, accounts.bob.address);
            console.log(`Bob's stake after listing: ${formatStakeAmount(stakeAfter)}`);
            console.log(`Seller stake delta after listing: ${formatStakeAmount(stakeAfter - stakeBefore)} (runtime emissions may offset transfer)`);

            const reservedAfterResult = await contract.query("get_reserved_alpha", {
                origin: accounts.alice.address,
                data: { netuid },
            });
            expect(reservedAfterResult.success).toBe(true);
            const reservedAfter = reservedAfterResult.success
                ? BigInt(reservedAfterResult.value.response)
                : 0n;
            expect(reservedAfter - reservedBefore).toBe(listAmount);

            // Query the listing to verify it was created
            const userListingIds = queryOk<bigint[]>(await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.bob.address,
                    netuid
                }
            }), "get_user_listings");
            expect(userListingIds).toContain(listingId);

            const listing = queryOk<any>(await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: { netuid, seller: accounts.bob.address, listing_id: listingId }
            }), "get_listing");
            expect(listing.amount).toBe(listAmount);
            expect(listing.price_offset_bps).toBe(priceOffsetBps);
            expect(listing.seller).toBe(accounts.bob.address);
            expect(listing.netuid).toBe(netuid);
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
                    amount: PRIMARY_LISTING_AMOUNT,
                    price_offset_bps: MARKET_PRICE // Market price
                }
            });

            const result = await submitReverted(listTx, accounts.dave.signer, "list_alpha without proxy");
            expect(result.dispatchError?.type).toContain("Module");
        });
    });

    describe("Verification Logic", () => {
        it("should fail when seller has insufficient stake", async () => {
            const { accounts } = context;
            // Eve has intentionally low stake and tries to list more than she owns.
            const listAmount = INSUFFICIENT_LISTING_AMOUNT;
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

            const result = await submitReverted(listTx, accounts.eve.signer, "list_alpha insufficient stake");

            const eveStakeAfter = await getStakeBalance(context.api, eveHotkey.address, netuid, accounts.eve.address);

            // Should fail with ContractReverted error
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

            const listAmount = SECONDARY_LISTING_AMOUNT;

            // List with special hotkey (different from contract's hotkey)
            const { result } = await createAlphaListing(contract, accounts.charlie.signer, accounts.charlie.address, {
                hotkey: charlieSpecialHotkey.address, // Different from contract's hotkey
                netuid,
                amount: listAmount,
                price_offset_bps: MARKET_PRICE
            });

            console.log("Consolidation result:", result.ok);

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

            const failResult = await submitReverted(failTx, accounts.dave.signer, "list_alpha below minimum");

            const contractsError = failResult.dispatchError?.value as ContractsError;
            expect(contractsError.type).toBe("Contracts");
            expect(contractsError.value.type).toBe("ContractReverted");

            // Test 2: Exactly minimum succeeds
            console.log(`Attempting to list exactly minimum: ${formatStakeAmount(minAmount)}`);

            const exact = await createAlphaListing(contract, accounts.dave.signer, accounts.dave.address, {
                hotkey: daveHotkey2.address,
                netuid,
                amount: minAmount,
                price_offset_bps: MARKET_PRICE
            });

            // Verify Dave has at least one listing (may have more from previous test runs)
            const listings = queryOk<bigint[]>(await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.dave.address,
                    netuid
                }
            }), "get_user_listings");
            expect(listings).toContain(exact.listingId);

            const listing = queryOk<any>(await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.dave.address,
                    listing_id: exact.listingId
                }
            }), "get_listing");
            expect(listing.amount).toBe(minAmount);
        });
    });
});
