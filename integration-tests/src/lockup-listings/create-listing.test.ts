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
    BITTENSOR_MIN_STAKE,
    type Wallet,
} from "../utils";
import {
    createLockupListing,
    expectEvent,
    queryOk,
    submitOk,
    submitReverted,
} from "../test-helpers";

const VALID_LISTING_AMOUNT = BITTENSOR_MIN_STAKE;
const LARGER_LISTING_AMOUNT = BITTENSOR_MIN_STAKE + 1_000_000n;
const BOB_REQUIRED_ALPHA = BITTENSOR_MIN_STAKE * 4n;
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

        const updateMinTx = contract.send("update_min_listing_amount", {
            origin: context.accounts.alice.address,
            data: { new_amount: BITTENSOR_MIN_STAKE }
        });
        await submitOk(updateMinTx, context.accounts.alice.signer, "update_min_listing_amount");

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
        await registerValidator(context.api, netuid, bobHotkey.address, context.accounts.bob.signer, taoToRao(5000), BOB_REQUIRED_ALPHA);
        await registerValidator(context.api, netuid, charlieHotkey.address, context.accounts.charlie.signer, taoToRao(5000), LARGER_LISTING_AMOUNT);
        await registerValidator(context.api, netuid, daveHotkey.address, context.accounts.dave.signer, taoToRao(5000), LARGER_LISTING_AMOUNT);
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
            const listAmount = VALID_LISTING_AMOUNT;
            const lockupDuration = MEDIUM_LOCKUP_DURATION;

            const { listingId, event } = await createLockupListing(contract, accounts.bob.signer, accounts.bob.address, {
                hotkey: bobHotkey.address,
                netuid,
                amount: listAmount,
                price_offset_bps: MARKET_PRICE,
                lockup_duration: lockupDuration
            });

            expect(event.value.seller).toBe(accounts.bob.address);
            expect(event.value.hotkey).toBe(bobHotkey.address);
            expect(event.value.custody_hotkey).toBe(bobHotkey.address);
            expect(event.value.netuid).toBe(netuid);
            expect(BigInt(event.value.amount)).toBe(listAmount);

            // Verify listing was created
            const listings = queryOk<bigint[]>(await contract.query("get_user_listings", {
                origin: accounts.alice.address,
                data: {
                    seller: accounts.bob.address,
                    netuid
                }
            }), "get_user_listings");
            expect(listings).toContain(listingId);

            const listing = queryOk<any>(await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: { netuid, seller: accounts.bob.address, listing_id: listingId }
            }), "get_listing");
            expect(listing).toBeDefined();
            expect(listing.total_amount).toBe(listAmount);
            expect(listing.remaining_amount).toBe(listAmount);
        }, 120000);

        it("should create listing with positive price offset", async () => {
            const { accounts } = context;
            const listAmount = VALID_LISTING_AMOUNT;
            const lockupDuration = MEDIUM_LOCKUP_DURATION;

            const { listingId } = await createLockupListing(contract, accounts.charlie.signer, accounts.charlie.address, {
                hotkey: charlieHotkey.address,
                netuid,
                amount: listAmount,
                price_offset_bps: ABOVE_MARKET_5, // +5%
                lockup_duration: lockupDuration
            });

            // Verify the listing has correct price offset
            const listing = queryOk<any>(await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: { netuid, seller: accounts.charlie.address, listing_id: listingId }
            }), "get_listing");
            expect(listing.price_offset_bps).toBe(ABOVE_MARKET_5);
        }, 120000);

        it("should create listing with negative price offset", async () => {
            const { accounts } = context;
            const listAmount = VALID_LISTING_AMOUNT;
            const lockupDuration = MEDIUM_LOCKUP_DURATION;

            const { listingId } = await createLockupListing(contract, accounts.dave.signer, accounts.dave.address, {
                hotkey: daveHotkey.address,
                netuid,
                amount: listAmount,
                price_offset_bps: BELOW_MARKET_5, // -5%
                lockup_duration: lockupDuration
            });

            // Verify the listing
            const listing = queryOk<any>(await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: { netuid, seller: accounts.dave.address, listing_id: listingId }
            }), "get_listing");
            expect(listing.price_offset_bps).toBe(BELOW_MARKET_5);
        }, 120000);

        it("should increment reserved alpha after listing", async () => {
            const { accounts } = context;

            const reservedBefore = await contract.query("get_reserved_alpha", {
                origin: accounts.alice.address,
                data: { netuid }
            });

            const listAmount = VALID_LISTING_AMOUNT;
            await createLockupListing(contract, accounts.bob.signer, accounts.bob.address, {
                hotkey: bobHotkey.address,
                netuid,
                amount: listAmount,
                price_offset_bps: MARKET_PRICE,
                lockup_duration: MEDIUM_LOCKUP_DURATION
            });

            const reservedAfter = await contract.query("get_reserved_alpha", {
                origin: accounts.alice.address,
                data: { netuid }
            });

            expect(reservedBefore.success).toBe(true);
            expect(reservedAfter.success).toBe(true);

            if (reservedBefore.success && reservedAfter.success) {
                const increase = reservedAfter.value.response - reservedBefore.value.response;
                expect(increase).toBe(listAmount);
            }
        }, 120000);

        it("should keep listing active when company hotkey consolidation fails", async () => {
            const { accounts } = context;
            const listAmount = VALID_LISTING_AMOUNT;

            const updateTx = contract.send("update_hotkey_for_subnet", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    new_hotkey: accounts.eve.address // coldkey, intentionally not registered as a subnet hotkey
                }
            });
            await submitOk(updateTx, accounts.alice.signer, "update_hotkey_for_subnet");

            const reservedByHotkeyBefore = queryOk<bigint>(await contract.query("get_reserved_alpha_for_hotkey", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    hotkey: bobHotkey.address
                }
            }), "get_reserved_alpha_for_hotkey");

            const { result, listingId } = await createLockupListing(contract, accounts.bob.signer, accounts.bob.address, {
                hotkey: bobHotkey.address,
                netuid,
                amount: listAmount,
                price_offset_bps: MARKET_PRICE,
                lockup_duration: MEDIUM_LOCKUP_DURATION
            });

            expectEvent(contract, result, "ListingHotkeyConsolidationFailed");

            const listing = queryOk<any>(await contract.query("get_listing", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.bob.address,
                    listing_id: listingId
                }
            }), "get_listing");
            expect(listing.custody_hotkey).toBe(bobHotkey.address);
            expect(listing.remaining_amount).toBe(listAmount);

            const reservedByHotkey = queryOk<bigint>(await contract.query("get_reserved_alpha_for_hotkey", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    hotkey: bobHotkey.address
                }
            }), "get_reserved_alpha_for_hotkey");
            expect(reservedByHotkey - reservedByHotkeyBefore).toBe(listAmount);

            const cancelTx = contract.send("cancel_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    netuid,
                    listing_id: listingId
                }
            });
            await submitOk(cancelTx, accounts.bob.signer, "cancel_lockup_listing");

            const revertTx = contract.send("update_hotkey_for_subnet", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    new_hotkey: accounts.alice.address
                }
            });
            await submitOk(revertTx, accounts.alice.signer, "update_hotkey_for_subnet");
        }, 180000);
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

            await submitReverted(listTx, accounts.bob.signer, "create_lockup_listing below minimum");
        }, 60000);

        it("should fail with InvalidPriceOffset when offset is -100% or below", async () => {
            const { accounts } = context;

            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: VALID_LISTING_AMOUNT,
                    price_offset_bps: INVALID_OFFSET, // -100%
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            await submitReverted(listTx, accounts.bob.signer, "create_lockup_listing invalid offset");
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
                    amount: VALID_LISTING_AMOUNT,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: tooShort
                }
            });

            await submitReverted(listTx, accounts.bob.signer, "create_lockup_listing short duration");
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
                    amount: VALID_LISTING_AMOUNT,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: tooLong
                }
            });

            await submitReverted(listTx, accounts.bob.signer, "create_lockup_listing long duration");
        }, 60000);

        it("should fail with InsufficientStake when seller lacks Alpha", async () => {
            const { accounts } = context;

            // Eve has only 30 Alpha, try to list 50
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.eve.address,
                data: {
                    hotkey: eveHotkey.address,
                    netuid,
                    amount: taoToRao(1),
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            await submitReverted(listTx, accounts.eve.signer, "create_lockup_listing insufficient stake");
        }, 60000);

        it("should fail when trading is paused", async () => {
            const { accounts } = context;

            // Pause trading
            const pauseTx = contract.send("pause_trading", {
                origin: accounts.alice.address,
                data: { reason: Binary.fromText("Testing pause") }
            });
            await submitOk(pauseTx, accounts.alice.signer, "pause_trading");

            // Try to create listing
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: VALID_LISTING_AMOUNT,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            await submitReverted(listTx, accounts.bob.signer, "create_lockup_listing while trading paused");

            // Resume
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await submitOk(resumeTx, accounts.alice.signer, "resume");
        }, 120000);

        it("should fail when contract is fully paused", async () => {
            const { accounts } = context;

            // Pause fully
            const pauseTx = contract.send("pause_fully", {
                origin: accounts.alice.address,
                data: { reason: Binary.fromText("Testing full pause") }
            });
            await submitOk(pauseTx, accounts.alice.signer, "pause_fully");

            // Try to create listing
            const listTx = contract.send("create_lockup_listing", {
                origin: accounts.bob.address,
                data: {
                    hotkey: bobHotkey.address,
                    netuid,
                    amount: VALID_LISTING_AMOUNT,
                    price_offset_bps: MARKET_PRICE,
                    lockup_duration: MEDIUM_LOCKUP_DURATION
                }
            });

            await submitReverted(listTx, accounts.bob.signer, "create_lockup_listing while fully paused");

            // Resume
            const resumeTx = contract.send("resume", {
                origin: accounts.alice.address,
                data: {}
            });
            await submitOk(resumeTx, accounts.alice.signer, "resume");
        }, 120000);
    });
});
