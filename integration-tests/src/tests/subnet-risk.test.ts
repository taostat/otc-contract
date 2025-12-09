import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { stringToU8a } from "@polkadot/util";
import { Binary } from "polkadot-api";

import {
    setupTestEnvironment,
    cleanupTestEnvironment,
    type TestContext,
    type ContractSdk,
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
    type Wallet,
    MARKET_PRICE,
} from "../utils";

import {
    getStakeBalance,
    formatStakeAmount,
} from "../utils/stake-helpers";

const TRANSFER_TOLERANCE = 10n; // Matches contract tolerance (rao)

type ContractsError = {
    type: "Contracts";
    value: { type: "ContractReverted"; value: undefined };
};

describe("Subnet Risk Controls", () => {
    let context: TestContext;
    let contract: ReturnType<ContractSdk["getContract"]>;
    let netuid: number;
    let contractHotkey: string;
    let charlieHotkey: Wallet;

    beforeAll(async () => {
        context = await setupTestEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);

        const { api, accounts } = context;

        // Register a fresh subnet for tests
        const aliceHotkey = createHotkey("//Alice/subnet-risk");
        await fundAccount(api, aliceHotkey.address, taoToRao(10), accounts.alice.signer);
        netuid = await registerSubnet(api, aliceHotkey.address, accounts.alice.signer);

        // Relax registration limits so validators can join quickly
        await elevateRegistrationLimits(api, netuid, 20, 20, accounts.alice.signer);
        await waitForBlocks(api, 1);

        // Ensure the contract hotkey is registered on the subnet
        const hotkeyResult = await contract.query("get_hotkey", {
            origin: accounts.alice.address,
            data: {},
        });
        expect(hotkeyResult.success).toBe(true);
        if (!hotkeyResult.success) {
            throw new Error("Failed to read contract hotkey for subnet risk tests");
        }
        contractHotkey = hotkeyResult.value.response;

        // Fund contract for staking fees and register its validator position
        await fundAccount(api, context.contractAddress!, taoToRao(15), accounts.alice.signer);
        await registerValidator(api, netuid, contractHotkey, accounts.alice.signer, taoToRao(60));

        // Prepare Charlie as the primary seller in these tests
        charlieHotkey = createHotkey("//Charlie/subnet-risk");
        await fundAccount(api, charlieHotkey.address, taoToRao(2), accounts.alice.signer);
        await registerValidator(api, netuid, charlieHotkey.address, accounts.charlie.signer, taoToRao(150));

        await waitForBlocks(api, 2);

        // Allow the contract to act as Charlie's proxy for stake transfers
        await addContractAsProxy(api, context.contractAddress!, accounts.charlie.signer);
        await waitForBlocks(api, 1);
    }, 240000); // Allow up to 4 minutes for full setup

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    const fetchReservedAlpha = async (): Promise<bigint> => {
        const { accounts } = context;
        const result = await contract.query("get_reserved_alpha", {
            origin: accounts.alice.address,
            data: { netuid },
        });

        expect(result.success).toBe(true);
        if (!result.success) {
            throw new Error("Failed to query reserved alpha");
        }

        return BigInt(result.value.response);
    };

    const getUserListings = async (): Promise<number[]> => {
        const { accounts } = context;
        const listingsResult = await contract.query("get_user_listings", {
            origin: accounts.alice.address,
            data: {
                seller: accounts.charlie.address,
                netuid,
            },
        });

        expect(listingsResult.success).toBe(true);
        if (!listingsResult.success) {
            return [];
        }

        return (listingsResult.value.response as (number | bigint)[]).map((id) => Number(id));
    };

    const listAlphaForCharlie = async (amount: bigint, priceOffsetBps: number = MARKET_PRICE) => {
        const { accounts } = context;

        const listTx = contract.send("list_alpha", {
            origin: accounts.charlie.address,
            data: {
                hotkey: charlieHotkey.address,
                netuid,
                amount,
                price_offset_bps: priceOffsetBps,
            },
        });

        const result = await listTx.signAndSubmit(accounts.charlie.signer);
        if (!result.ok) {
            console.error("list_alpha failed", JSON.stringify(result.dispatchError, null, 2));
        }
        expect(result.ok).toBe(true);

        const events = contract.filterEvents(result.events);
        const listedEvent = events.find(event => event.type === "AlphaListed");
        const listingIdFromEvent = listedEvent ? Number(listedEvent.value.alpha_listing_id) : undefined;

        await waitForBlocks(context.api, 1);

        const listingIds = await getUserListings();
        let listingId = listingIdFromEvent ?? listingIds[listingIds.length - 1];

        if (listingId === undefined) {
            throw new Error("Listing ID could not be determined after list_alpha");
        }

        const listingIdNumber = Number(listingId);

        const listingDetails = await contract.query("get_listing", {
            origin: accounts.alice.address,
            data: {
                netuid,
                seller: accounts.charlie.address,
                listing_id: BigInt(listingIdNumber),
            },
        });

        expect(listingDetails.success).toBe(true);

        if (!listingDetails.success) {
            throw new Error("Failed to fetch listing details after creation");
        }

        const createdAt = listingDetails.value.response ? Number(listingDetails.value.response.created_at) : 0;

        return {
            listingId: listingIdNumber,
            createdAt,
        };
    };

    it("allows the owner to force cancel listings and return reserved stake", async () => {
        const { accounts, api } = context;

        const listingAmount = taoToRao(40);

        const stakeBefore = await getStakeBalance(api, charlieHotkey.address, netuid, accounts.charlie.address);
        console.log(`Charlie's stake before listing: ${formatStakeAmount(stakeBefore)}`);

        const { listingId } = await listAlphaForCharlie(listingAmount, MARKET_PRICE);

        const reservedAfterListing = await fetchReservedAlpha();
        console.log(`Reserved Alpha after listing: ${formatStakeAmount(reservedAfterListing)}`);
        expect(reservedAfterListing).toBeGreaterThanOrEqual(listingAmount - TRANSFER_TOLERANCE);
        expect(reservedAfterListing).toBeLessThanOrEqual(listingAmount + TRANSFER_TOLERANCE);

        const forceTx = contract.send("force_cancel_alpha_listing", {
            origin: accounts.alice.address,
            data: {
                netuid,
                seller: accounts.charlie.address,
                listing_id: BigInt(listingId),
            },
        });

        const forceResult = await forceTx.signAndSubmit(accounts.alice.signer);
        expect(forceResult.ok).toBe(true);

        const events = contract.filterEvents(forceResult.events);
        const cancelled = events.find(event => event.type === "AlphaListingForceCancelled");
        expect(cancelled).toBeDefined();

        if (cancelled) {
            expect(cancelled.value.seller).toBe(accounts.charlie.address);
            expect(cancelled.value.initiated_by).toBe(accounts.alice.address);
            expect(Number(cancelled.value.listing_id)).toBe(listingId);
            const amountReturned = BigInt(cancelled.value.amount_returned);
            expect(amountReturned).toBeGreaterThanOrEqual(listingAmount - TRANSFER_TOLERANCE);
            expect(amountReturned).toBeLessThanOrEqual(listingAmount);
        }

        await waitForBlocks(api, 1);

        const stakeAfter = await getStakeBalance(api, charlieHotkey.address, netuid, accounts.charlie.address);
        console.log(`Charlie's stake after force cancel: ${formatStakeAmount(stakeAfter)}`);

        const maxLoss = listingAmount + TRANSFER_TOLERANCE;
        const minExpected = stakeBefore - maxLoss;
        const maxExpected = stakeBefore + TRANSFER_TOLERANCE;
        expect(stakeAfter).toBeGreaterThanOrEqual(minExpected);
        expect(stakeAfter).toBeLessThanOrEqual(maxExpected);


        const reservedAfterCancel = await fetchReservedAlpha();
        console.log(`Reserved Alpha after cancel: ${formatStakeAmount(reservedAfterCancel)}`);
        expect(reservedAfterCancel).toBeLessThanOrEqual(TRANSFER_TOLERANCE);

        const remainingListings = await getUserListings();
        expect(remainingListings).not.toContain(listingId);
    }, 180000);

    it("rejects force cancellation attempts from non-owners", async () => {
        const { accounts } = context;

        const { listingId } = await listAlphaForCharlie(taoToRao(15), MARKET_PRICE);

        const unauthorizedTx = contract.send("force_cancel_alpha_listing", {
            origin: accounts.charlie.address,
            data: {
                netuid,
                seller: accounts.charlie.address,
                listing_id: BigInt(listingId),
            },
        });

        const unauthorizedResult = await unauthorizedTx.signAndSubmit(accounts.charlie.signer);
        expect(unauthorizedResult.ok).toBe(false);

        const contractsError = unauthorizedResult.dispatchError?.value as ContractsError | undefined;
        expect(contractsError?.type).toBe("Contracts");
        expect(contractsError?.value.type).toBe("ContractReverted");

        // Listing should still exist after failed cancellation
        const listingsAfterFailure = await getUserListings();
        expect(listingsAfterFailure).toContain(listingId);

        // Clean up by cancelling as owner
        const ownerCancelTx = contract.send("force_cancel_alpha_listing", {
            origin: accounts.alice.address,
            data: {
                netuid,
                seller: accounts.charlie.address,
                listing_id: BigInt(listingId),
            },
        });
        const ownerCancelResult = await ownerCancelTx.signAndSubmit(accounts.alice.signer);
        expect(ownerCancelResult.ok).toBe(true);
        await waitForBlocks(context.api, 1);
    }, 150000);

    it("allows force cancellation immediately after listing despite minimum age", async () => {
        const { accounts, api } = context;

        const blockBeforeListing = await api.query.System.Number.getValue();

        const { listingId, createdAt } = await listAlphaForCharlie(taoToRao(10), MARKET_PRICE);

        const cancelTx = contract.send("force_cancel_alpha_listing", {
            origin: accounts.alice.address,
            data: {
                netuid,
                seller: accounts.charlie.address,
                listing_id: BigInt(listingId),
            },
        });

        const cancelResult = await cancelTx.signAndSubmit(accounts.alice.signer);
        expect(cancelResult.ok).toBe(true);

        const blockAfterCancel = await api.query.System.Number.getValue();
        const blocksElapsed = Number(blockAfterCancel) - Number(blockBeforeListing);
        const listingAgeAtCancel = Number(blockAfterCancel) - createdAt;

        console.log(`Blocks elapsed from listing to cancel: ${blocksElapsed}, listing age on cancel: ${listingAgeAtCancel}`);

        expect(listingAgeAtCancel).toBeLessThan(100); // min_listing_age configured in constructor

        await waitForBlocks(api, 1);
    }, 120000);

    it("freezes and unfreezes subnet listings via admin controls", async () => {
        const { accounts } = context;

        const freezeReason = Binary.fromBytes(stringToU8a("subnet at risk"));
        const freezeTx = contract.send("set_subnet_listing_status", {
            origin: accounts.alice.address,
            data: {
                netuid,
                frozen: true,
                reason: freezeReason,
            },
        });

        const freezeResult = await freezeTx.signAndSubmit(accounts.alice.signer);
        expect(freezeResult.ok).toBe(true);

        const freezeEvents = contract.filterEvents(freezeResult.events);
        const freezeEvent = freezeEvents.find(event => event.type === "SubnetListingStatusChanged");
        expect(freezeEvent).toBeDefined();
        if (freezeEvent) {
            expect(freezeEvent.value.frozen).toBe(true);
            expect(freezeEvent.value.netuid).toBe(netuid);
            expect(freezeEvent.value.changed_by).toBe(accounts.alice.address);
        }

        await waitForBlocks(context.api, 1);

        const stakeBeforeAttempt = await getStakeBalance(
            context.api,
            charlieHotkey.address,
            netuid,
            accounts.charlie.address,
        );
        const listingsBefore = await getUserListings();
        const reservedBefore = await fetchReservedAlpha();

        const blockedTx = contract.send("list_alpha", {
            origin: accounts.charlie.address,
            data: {
                hotkey: charlieHotkey.address,
                netuid,
                amount: taoToRao(5),
                price_offset_bps: MARKET_PRICE,
            },
        });

        const blockedResult = await blockedTx.signAndSubmit(accounts.charlie.signer);
        expect(blockedResult.ok).toBe(false);
        const blockedError = blockedResult.dispatchError?.value as ContractsError | undefined;
        expect(blockedError?.type).toBe("Contracts");
        expect(blockedError?.value.type).toBe("ContractReverted");

        const stakeAfterAttempt = await getStakeBalance(
            context.api,
            charlieHotkey.address,
            netuid,
            accounts.charlie.address,
        );
        expect(stakeAfterAttempt).toBeGreaterThanOrEqual(stakeBeforeAttempt - TRANSFER_TOLERANCE);

        await waitForBlocks(context.api, 1);
        const listingsAfterBlocked = await getUserListings();
        expect(listingsAfterBlocked).toEqual(listingsBefore);

        const reservedAfterBlocked = await fetchReservedAlpha();
        expect(reservedAfterBlocked).toBeGreaterThanOrEqual(reservedBefore - TRANSFER_TOLERANCE);
        expect(reservedAfterBlocked).toBeLessThanOrEqual(reservedBefore + TRANSFER_TOLERANCE);

        const unfreezeTx = contract.send("set_subnet_listing_status", {
            origin: accounts.alice.address,
            data: {
                netuid,
                frozen: false,
                reason: Binary.fromBytes(stringToU8a("risk cleared")),
            },
        });

        const unfreezeResult = await unfreezeTx.signAndSubmit(accounts.alice.signer);
        expect(unfreezeResult.ok).toBe(true);
        await waitForBlocks(context.api, 1);

        const { listingId } = await listAlphaForCharlie(taoToRao(8), MARKET_PRICE);

        // Clean up listing so later tests start fresh
        const cleanupTx = contract.send("force_cancel_alpha_listing", {
            origin: accounts.alice.address,
            data: {
                netuid,
                seller: accounts.charlie.address,
                listing_id: BigInt(listingId),
            },
        });
        const cleanupResult = await cleanupTx.signAndSubmit(accounts.alice.signer);
        expect(cleanupResult.ok).toBe(true);
        await waitForBlocks(context.api, 1);
    }, 200000);

    it("supports end-to-end monitor workflow for risky subnets", async () => {
        const { accounts } = context;

        await listAlphaForCharlie(taoToRao(12), MARKET_PRICE);
        await listAlphaForCharlie(taoToRao(18), MARKET_PRICE);

        const listingsBefore = await getUserListings();
        expect(listingsBefore.length).toBeGreaterThanOrEqual(2);

        const freezeTx = contract.send("set_subnet_listing_status", {
            origin: accounts.alice.address,
            data: {
                netuid,
                frozen: true,
                reason: Binary.fromBytes(stringToU8a("monitor freeze")),
            },
        });
        const freezeResult = await freezeTx.signAndSubmit(accounts.alice.signer);
        expect(freezeResult.ok).toBe(true);
        await waitForBlocks(context.api, 1);

        // Forced unwind of all active listings
        for (const listingId of listingsBefore) {
            const cancelTx = contract.send("force_cancel_alpha_listing", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    seller: accounts.charlie.address,
                    listing_id: BigInt(listingId),
                },
            });
            const cancelResult = await cancelTx.signAndSubmit(accounts.alice.signer);
            expect(cancelResult.ok).toBe(true);
            await waitForBlocks(context.api, 1);
        }

        await waitForBlocks(context.api, 2);

        const reservedAfterUnwind = await fetchReservedAlpha();
        console.log(`Reserved Alpha after forced unwinds: ${formatStakeAmount(reservedAfterUnwind)}`);
        const maxResidualTolerance = TRANSFER_TOLERANCE * BigInt(listingsBefore.length + 1);
        expect(reservedAfterUnwind).toBeLessThanOrEqual(maxResidualTolerance);

        const listingsAfterUnwind = await getUserListings();
        expect(listingsAfterUnwind.length).toBe(0);

        const unfreezeTx = contract.send("set_subnet_listing_status", {
            origin: accounts.alice.address,
            data: {
                netuid,
                frozen: false,
                reason: Binary.fromBytes(stringToU8a("monitor clear")),
            },
        });
        const unfreezeResult = await unfreezeTx.signAndSubmit(accounts.alice.signer);
        expect(unfreezeResult.ok).toBe(true);
        await waitForBlocks(context.api, 2);

        const { listingId } = await listAlphaForCharlie(taoToRao(9), MARKET_PRICE);

        // Final cleanup
        const finalCancelTx = contract.send("force_cancel_alpha_listing", {
            origin: accounts.alice.address,
            data: {
                netuid,
                seller: accounts.charlie.address,
                listing_id: BigInt(listingId),
            },
        });
        const finalCancelResult = await finalCancelTx.signAndSubmit(accounts.alice.signer);
        expect(finalCancelResult.ok).toBe(true);
        await waitForBlocks(context.api, 1);
    }, 240000);
});
