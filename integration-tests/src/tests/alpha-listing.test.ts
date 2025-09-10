import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { setupTestEnvironment, cleanupTestEnvironment, type TestContext, ContractSdk } from "../setup";
import {
    priceToFixedPoint,
    fixedPointToPrice,
    taoToRao,
    waitForBlocks,
    fundAccount,
    registerSubnet,
    registerValidator,
    addContractAsProxy,
    createHotkey,
    elevateRegistrationLimits,
    type Wallet,
} from "../utils";
import {
    getStakeBalance,
    hasProxyPermission,
    formatStakeAmount,
} from "../utils/stake-helpers";

describe("Alpha Listing Operations", () => {
    let context: TestContext;
    let contract: ReturnType<ContractSdk["getContract"]>;
    let netuid: number;

    let bobHotkey: Wallet;
    let charlieWallet: Wallet;
    let daveHotkey: Wallet;

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

        await registerValidator(
            context.api,
            netuid,
            charlieWallet.address,
            context.accounts.charlie.signer,
            taoToRao(150) // 150 Alpha initial stake
        );

        await registerValidator(
            context.api,
            netuid,
            daveHotkey.address,
            context.accounts.dave.signer,
            taoToRao(100) // 100 Alpha initial stake for Dave
        );

        // Wait for registrations to be processed
        await waitForBlocks(context.api, 2);

        // Verify initial stakes (pass coldkey for accurate balance)
        const bobStake = await getStakeBalance(context.api, bobHotkey.address, netuid, context.accounts.bob.address);
        const charlieStake = await getStakeBalance(context.api, charlieWallet.address, netuid, context.accounts.charlie.address);
        const daveStake = await getStakeBalance(context.api, daveHotkey.address, netuid, context.accounts.dave.address);

        console.log(`Bob's initial stake: ${formatStakeAmount(bobStake)}`);
        console.log(`Charlie's initial stake: ${formatStakeAmount(charlieStake)}`);
        console.log(`Dave's initial stake: ${formatStakeAmount(daveStake)}`);
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

            // Bob lists 50 Alpha at 2 TAO per Alpha
            const listAmount = taoToRao(50);
            const listPrice = priceToFixedPoint(2.0);

            console.log(`Attempting to list ${formatStakeAmount(listAmount)} at price ${fixedPointToPrice(listPrice)} TAO per Alpha`);

            const listTx = contract.send("list_alpha", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: listAmount,
                    price: listPrice
                }
            });

            const result = await listTx.signAndSubmit(accounts.bob.signer);

            console.log("Listing transaction result:", {
                ok: result.ok,
                dispatchError: result.dispatchError,
                events: result.events
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
                    expect(listing.value.response.price).toBe(listPrice);
                    expect(listing.value.response.seller).toBe(accounts.bob.address);
                    expect(listing.value.response.netuid).toBe(netuid);
                }
            }
        });

        it("should fail to list Alpha without proxy setup", async () => {
            const { accounts } = context;

            // Dave tries to list without setting up proxy (Dave has never set up a proxy)
            const listTx = contract.send("list_alpha", {
                origin: accounts.dave.address,
                data: {
                    hotkey: daveHotkey.address,
                    netuid,
                    amount: taoToRao(50),
                    price: priceToFixedPoint(2.0) // 2 TAO per Alpha
                }
            });

            const result = await listTx.signAndSubmit(accounts.dave.signer);

            console.log("Transaction result for Dave (no proxy):", result);

            // Should fail because Dave has no proxy set up
            expect(result.ok).toBe(false);
            expect(result.dispatchError?.type).toContain("Module");
        });

    });
});
