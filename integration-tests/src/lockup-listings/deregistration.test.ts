import { describe, it, expect, beforeAll, afterAll } from "vitest";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    TestSetup,
    type LockupListingsContext,
    type TestAccount,
} from "../setup";
import {
    taoToRao,
    getBalance,
    waitForBlocks,
    waitUntilBlock,
    MARKET_PRICE,
    SHORT_LOCKUP_DURATION,
    type Wallet,
} from "../utils";
import {
    createLockupListing,
    expectContractError,
    expectEvent,
    queryOk,
    quoteVault,
    setupLockupSubnet,
    submitOk,
    takeLockupListing,
    withIntegrationGas,
} from "../test-helpers";

const LISTING_AMOUNT = taoToRao(10);
const TAKE_AMOUNT = taoToRao(3);
const SELLER_ALPHA = LISTING_AMOUNT * 10n;
// The market price can move between a quote and the take; the excess is refunded.
const PAYMENT_BUFFER = taoToRao(10);
// Subtensor clears a dissolved subnet over several blocks, in on_idle.
const MAX_CLEANUP_BLOCKS = 300;

// Variant indexes of the vault's and the escrow's Error enums.
const VAULT_STAKE_TRANSFER_FAILED = 18;
const ESCROW_NO_ALPHA_TO_CLAIM = 6;

describe("Listings and escrows on a dissolved subnet", () => {
    let context: LockupListingsContext;
    let factory: ReturnType<LockupListingsContext["contractSdk"]["getContract"]>;
    let netuid: number;
    let sellerHotkey: Wallet;
    let alice: TestAccount;
    let bob: TestAccount;
    let charlie: TestAccount;
    let dave: TestAccount;

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        factory = context.contractSdk.getContract(context.contractAddress!);
        ({ alice, bob, charlie, dave } = context.accounts);
        ({ netuid, sellerHotkey } = await setupLockupSubnet(context, "dissolved", SELLER_ALPHA));
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    function listBobs(lockupDuration: number | undefined) {
        return createLockupListing(context, bob, {
            hotkey: sellerHotkey.address,
            netuid,
            amount: LISTING_AMOUNT,
            price_offset_bps: MARKET_PRICE,
            lockup_duration: lockupDuration,
        });
    }

    async function userListings(seller: string, subnet: number) {
        return queryOk<bigint[]>(await factory.query("get_user_listings", {
            origin: alice.address,
            data: { seller, netuid: subnet },
        }), "get_user_listings");
    }

    /** Sweep a vault's or escrow's TAO as a third party, and check it reached `owner`. */
    async function sweepTo(contract: any, owner: string) {
        const ownerBalanceBefore = await getBalance(context.api, owner);
        const result = await submitOk(contract.send("sweep_tao", withIntegrationGas({
            origin: dave.address,
            data: {},
        })), dave.signer, "sweep_tao");
        const swept = expectEvent(contract, result, "TaoSwept");
        expect(swept.value.owner).toBe(owner);
        const amount = BigInt(swept.value.amount);
        expect(await getBalance(context.api, owner) - ownerBalanceBefore).toBe(amount);
        return amount;
    }

    async function waitForCleanup(subnet: number) {
        const unsafeApi = await TestSetup.getInstance().getUnsafeApi();
        for (let block = 0; block < MAX_CLEANUP_BLOCKS; block++) {
            const queue: number[] = await unsafeApi.query.SubtensorModule.DissolveCleanupQueue.getValue();
            if (!queue.includes(subnet)) {
                return;
            }
            await waitForBlocks(context.api, 1);
        }
        throw new Error(`Subnet ${subnet} was not cleaned up within ${MAX_CLEANUP_BLOCKS} blocks`);
    }

    it("pays the liquidated stake of open vaults and expired escrows to their owners, who then close them", async () => {
        const { api } = context;

        // An open lockup listing with a purchase in an escrow, and an open spot listing.
        const lockup = await listBobs(SHORT_LOCKUP_DURATION);
        const spot = await listBobs(undefined);
        const quote = await quoteVault(lockup.vault, charlie.address, TAKE_AMOUNT);
        const { escrowAccount } = await takeLockupListing(lockup.vault, charlie, {
            amount: TAKE_AMOUNT,
            recipient: charlie.address,
        }, quote.total + PAYMENT_BUFFER);
        const escrow = context.alphaLockupSdk.getContract(escrowAccount!);
        await waitUntilBlock(api, queryOk<number>(await escrow.query("get_unlock_block", {
            origin: alice.address,
            data: {},
        }), "get_unlock_block"));

        // Root dissolves the subnet. Subtensor queues its cleanup, which sells
        // every position for TAO and credits each coldkey's share.
        const dissolve = api.tx.SubtensorModule.root_dissolve_network({ netuid });
        await submitOk(api.tx.Sudo.sudo({ call: dissolve.decodedCall }), alice.signer, "root_dissolve_network");

        // Nothing can be bought from the subnet's listings any more: Subtensor
        // refuses to move stake on a dissolved subnet.
        expectContractError(await spot.vault.query("take", {
            origin: charlie.address,
            data: { amount: TAKE_AMOUNT, recipient: charlie.address },
            value: quote.total + PAYMENT_BUFFER,
        }), VAULT_STAKE_TRANSFER_FAILED, "take on a dissolved subnet");

        // Once the cleanup is done, the liquidation TAO waits in each contract, and
        // anyone sweeps it to the seller or the escrow's beneficiary.
        await waitForCleanup(netuid);
        expect(await sweepTo(lockup.vault, bob.address)).toBeGreaterThan(0n);
        expect(await sweepTo(spot.vault, bob.address)).toBeGreaterThan(0n);
        expect(await sweepTo(escrow, charlie.address)).toBeGreaterThan(0n);

        // No Alpha is left to claim. The beneficiary closes the escrow.
        expectContractError(await escrow.query("claim", { origin: dave.address, data: {} }), ESCROW_NO_ALPHA_TO_CLAIM, "claim");
        await submitOk(escrow.send("close", withIntegrationGas({ origin: charlie.address, data: {} })), charlie.signer, "close escrow");
        expect(await TestSetup.getInstance().contractExistsOnChain(api, escrowAccount!)).toBe(false);

        // The seller cancels both listings, with no stake to return, and closes them.
        const spotInfo = queryOk<any>(await spot.vault.query("get_info", { origin: alice.address, data: {} }), "get_info");
        await waitUntilBlock(api, Number(spotInfo.cancellable_after));
        for (const listing of [lockup, spot]) {
            const cancelled = await submitOk(listing.vault.send("cancel", withIntegrationGas({
                origin: bob.address,
                data: {},
            })), bob.signer, "cancel");
            expect(BigInt(expectEvent(listing.vault, cancelled, "Cancelled").value.returned)).toBe(0n);
            expectEvent(factory, cancelled, "ListingClosed");
            await submitOk(listing.vault.send("close", withIntegrationGas({
                origin: bob.address,
                data: {},
            })), bob.signer, "close vault");
            expect(await TestSetup.getInstance().contractExistsOnChain(api, listing.vaultAddress)).toBe(false);
        }
        expect(await userListings(bob.address, netuid)).toEqual([]);
    }, 900000);

    it("lists and trades as usual on a subnet registered under the same netuid", async () => {
        const { api } = context;
        const setRateLimit = api.tx.AdminUtils.sudo_set_network_rate_limit({ rate_limit: 0n });
        await submitOk(api.tx.Sudo.sudo({ call: setRateLimit.decodedCall }), alice.signer, "sudo_set_network_rate_limit");

        // Subtensor gives the new subnet the lowest free netuid: the dissolved one.
        const previous = netuid;
        ({ netuid, sellerHotkey } = await setupLockupSubnet(context, "reregistered", SELLER_ALPHA));
        expect(netuid).toBe(previous);

        const { vault, listingId, listedAmount } = await listBobs(SHORT_LOCKUP_DURATION);
        expect(await userListings(bob.address, netuid)).toEqual([listingId]);
        const quote = await quoteVault(vault, charlie.address, TAKE_AMOUNT);
        const { event, escrowAccount } = await takeLockupListing(vault, charlie, {
            amount: TAKE_AMOUNT,
            recipient: charlie.address,
        }, quote.total + PAYMENT_BUFFER);
        expect(BigInt(event.value.remaining)).toBe(listedAmount - TAKE_AMOUNT);
        expect(await TestSetup.getInstance().contractExistsOnChain(api, escrowAccount!)).toBe(true);
    }, 900000);
});
