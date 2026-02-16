import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { Binary } from "polkadot-api";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    type LockupListingsContext,
    LockupListingsSdk
} from "../setup";
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
    ABOVE_MARKET_5,
    BELOW_MARKET_5,
    INVALID_OFFSET,
    MEDIUM_LOCKUP_DURATION,
    type Wallet,
} from "../utils";
import {
    getStakeBalance,
    formatStakeAmount,
} from "../utils/stake-helpers";

describe("Lockup Listings Contract - Create Listing", () => {
    let context: LockupListingsContext;
    let contract: ReturnType<LockupListingsSdk["getContract"]>;
    let netuid: number;

    let bobHotkey: Wallet;
    let charlieHotkey: Wallet;
    let daveHotkey: Wallet;
    let eveHotkey: Wallet;

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);

        // Create a subnet for testing
        const aliceHotkey = createHotkey("//Alice");
        await fundAccount(context.api, aliceHotkey.address, taoToRao(10), context.accounts.alice.signer);
        netuid = await registerSubnet(context.api, aliceHotkey.address, context.accounts.alice.signer);
        console.log(`Created test subnet with netuid: ${netuid}`);

        // Elevate registration limits
        await elevateRegistrationLimits(
            context.api,
            netuid,
            20,
            20,
            context.accounts.alice.signer
        );
        await waitForBlocks(context.api, 1);

        // Register the contract's hotkey as a validator
        const contractHotkeyResult = await contract.query("get_hotkey", {
            origin: context.accounts.alice.address,
            data: {}
        });

        if (contractHotkeyResult.success) {
            const contractHotkey = contractHotkeyResult.value.response;
            console.log(`Contract hotkey: ${contractHotkey}`);

            await fundAccount(context.api, context.contractAddress!, taoToRao(10), context.accounts.alice.signer);
            await registerValidator(
                context.api,
                netuid,
                contractHotkey,
                context.accounts.alice.signer,
                taoToRao(50)
            );
            console.log(`Registered contract's hotkey as validator`);
        }

        // Create and register hotkeys for test accounts
        bobHotkey = createHotkey("//Bob");
        charlieHotkey = createHotkey("//Charlie");
        daveHotkey = createHotkey("//Dave");
        eveHotkey = createHotkey("//Eve");

        // Fund hotkeys
        await fundAccount(context.api, bobHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, charlieHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, daveHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await fundAccount(context.api, eveHotkey.address, taoToRao(1), context.accounts.alice.signer);

        // Register validators with initial stake
        await registerValidator(context.api, netuid, bobHotkey.address, context.accounts.bob.signer, taoToRao(100));
        await registerValidator(context.api, netuid, charlieHotkey.address, context.accounts.charlie.signer, taoToRao(100));
        await registerValidator(context.api, netuid, daveHotkey.address, context.accounts.dave.signer, taoToRao(100));
        await registerValidator(context.api, netuid, eveHotkey.address, context.accounts.eve.signer, taoToRao(30)); // Eve has less stake

        await waitForBlocks(context.api, 2);

        // Add contract as proxy for test accounts
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.bob.signer);
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.charlie.signer);
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.dave.signer);
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.eve.signer);

        await waitForBlocks(context.api, 2);

        console.log("Test setup complete");
    }, 300000); // 5 minute timeout for setup

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    describe("Happy Paths", () => {
        it("should create a lockup listing with valid parameters", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(10);
            const lockupDuration = MEDIUM_LOCKUP_DURATION;

            const stakeBefore = await getStakeBalance(context.api, bobHotkey.address, netuid, accounts.bob.address);
            console.log(`Bob's stake before: ${formatStakeAmount(stakeBefore)}`);

            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: lockupDuration
                }
            });

            const result = await listTx.signAndSubmit(accounts.bob.signer);
            expect(result.ok).toBe(true);

            await waitForBlocks(context.api, 2);

            // Verify listing was created
            const listingsResult = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.bob.address,
                    netuid
                }
            });

            expect(listingsResult.success).toBe(true);
            if (listingsResult.success) {
                expect(listingsResult.value.response.length).toBeGreaterThan(0);
            }
        }, 120000);

        it("should create listing with positive price offset", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(5);
            const lockupDuration = MEDIUM_LOCKUP_DURATION;

            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.charlie.address,
                data: {
                    hotkey: charlieHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: ABOVE_MARKET_5, // +5%
                    lockup_duration: lockupDuration
                }
            });

            const result = await listTx.signAndSubmit(accounts.charlie.signer);
            expect(result.ok).toBe(true);

            // Verify the listing has correct price offset
            const listingsResult = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.charlie.address,
                    netuid
                }
            });

            expect(listingsResult.success).toBe(true);
            if (listingsResult.success && listingsResult.value.response.length > 0) {
                const listingId = listingsResult.value.response[listingsResult.value.response.length - 1];
                const listing = await contract.query("get_listing", {
                    origin: accounts.alice.address,
                    data: {
                        netuid,
                        seller: accounts.charlie.address,
                        listing_id: listingId
                    }
                });

                expect(listing.success).toBe(true);
                if (listing.success && listing.value.response) {
                    expect(listing.value.response.price_offset_bps).toBe(ABOVE_MARKET_5);
                }
            }
        }, 120000);

        it("should create listing with negative price offset", async () => {
            const { accounts } = context;
            const listAmount = taoToRao(5);
            const lockupDuration = MEDIUM_LOCKUP_DURATION;

            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.dave.address,
                data: {
                    hotkey: daveHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: BELOW_MARKET_5, // -5%
                    lockup_duration: lockupDuration
                }
            });

            const result = await listTx.signAndSubmit(accounts.dave.signer);
            expect(result.ok).toBe(true);

            // Verify the listing
            const listingsResult = await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.dave.address,
                    netuid
                }
            });

            expect(listingsResult.success).toBe(true);
            if (listingsResult.success && listingsResult.value.response.length > 0) {
                const listingId = listingsResult.value.response[listingsResult.value.response.length - 1];
                const listing = await contract.query("get_listing", {
                    origin: accounts.alice.address,
                    data: {
                        netuid,
                        seller: accounts.dave.address,
                        listing_id: listingId
                    }
                });

                expect(listing.success).toBe(true);
                if (listing.success && listing.value.response) {
                    expect(listing.value.response.price_offset_bps).toBe(BELOW_MARKET_5);
                }
            }
        }, 120000);

        it("should increment reserved alpha after listing", async () => {
            const { accounts } = context;

            const reservedBefore = await contract.query("get_reserved_alpha", {
                origin: accounts.alice.address,
                data: { netuid }
            });

            const listAmount = taoToRao(5);
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: listAmount,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            await listTx.signAndSubmit(accounts.bob.signer);
            await waitForBlocks(context.api, 1);

            const reservedAfter = await contract.query("get_reserved_alpha", {
                origin: accounts.alice.address,
                data: { netuid }
            });

            expect(reservedBefore.success).toBe(true);
            expect(reservedAfter.success).toBe(true);

            if (reservedBefore.success && reservedAfter.success) {
                const increase = reservedAfter.value.response - reservedBefore.value.response;
                // Allow some tolerance for potential staking rewards adjustments
                expect(increase).toBeGreaterThanOrEqual(listAmount - taoToRao(1));
            }
        }, 120000);
    });

    describe("Error Cases", () => {
        it("should fail with AmountTooSmall when below minimum", async () => {
            const { accounts } = context;

            // Get minimum listing amount
            const minAmountResult = await contract.query("get_min_listing_amount", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(minAmountResult.success).toBe(true);
            if (!minAmountResult.success) return;

            const minAmount = minAmountResult.value.response;
            const belowMin = minAmount - 1n;

            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: belowMin,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            const result = await listTx.signAndSubmit(accounts.bob.signer);
            expect(result.ok).toBe(false);
        }, 60000);

        it("should fail with InvalidPriceOffset when offset is -100% or below", async () => {
            const { accounts } = context;

            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: taoToRao(5),
                    price_offset_bps: INVALID_OFFSET, // -100%
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            const result = await listTx.signAndSubmit(accounts.bob.signer);
            expect(result.ok).toBe(false);
        }, 60000);

        it("should fail with LockupDurationTooShort when below minimum", async () => {
            const { accounts } = context;

            // Get lockup duration limits
            const limitsResult = await contract.query("get_lockup_duration_limits", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(limitsResult.success).toBe(true);
            if (!limitsResult.success) return;

            const [minDuration] = limitsResult.value.response;
            const tooShort = minDuration - 1;

            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: taoToRao(5),
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: tooShort
                }
            });

            const result = await listTx.signAndSubmit(accounts.bob.signer);
            expect(result.ok).toBe(false);
        }, 60000);

        it("should fail with LockupDurationTooLong when above maximum", async () => {
            const { accounts } = context;

            // Get lockup duration limits
            const limitsResult = await contract.query("get_lockup_duration_limits", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(limitsResult.success).toBe(true);
            if (!limitsResult.success) return;

            const [, maxDuration] = limitsResult.value.response;
            const tooLong = maxDuration + 1;

            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: taoToRao(5),
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: tooLong
                }
            });

            const result = await listTx.signAndSubmit(accounts.bob.signer);
            expect(result.ok).toBe(false);
        }, 60000);

        it("should fail with InsufficientStake when seller lacks Alpha", async () => {
            const { accounts } = context;

            // Eve has only 30 Alpha, try to list 50
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.eve.address,
                data: {
                    hotkey: eveHotkey.address,
                    netuid,
                    amount: taoToRao(50),
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            const result = await listTx.signAndSubmit(accounts.eve.signer);
            expect(result.ok).toBe(false);
        }, 60000);

        it("should fail when trading is paused", async () => {
            const { accounts } = context;

            // Pause trading
            const pauseTx = contract.send("pause_trading", {
                origin: accounts.alice.address,
                data: { reason: Binary.fromText("Testing pause") }
            });
            await pauseTx.signAndSubmit(accounts.alice.signer);

            // Try to create listing
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: taoToRao(5),
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            const result = await listTx.signAndSubmit(accounts.bob.signer);
            expect(result.ok).toBe(false);

            // Resume
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await resumeTx.signAndSubmit(accounts.alice.signer);
        }, 120000);

        it("should fail when contract is fully paused", async () => {
            const { accounts } = context;

            // Pause fully
            const pauseTx = contract.send("pause_fully", {
                origin: accounts.alice.address,
                data: { reason: Binary.fromText("Testing full pause") }
            });
            await pauseTx.signAndSubmit(accounts.alice.signer);

            // Try to create listing
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: taoToRao(5),
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            const result = await listTx.signAndSubmit(accounts.bob.signer);
            expect(result.ok).toBe(false);

            // Resume
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await resumeTx.signAndSubmit(accounts.alice.signer);
        }, 120000);
    });
});
