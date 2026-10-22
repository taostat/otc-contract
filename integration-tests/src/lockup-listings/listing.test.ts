import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { Binary } from "polkadot-api";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    TestSetup,
    SPOT_LISTING_AGE,
    type LockupListingsContext,
    type TestAccount,
} from "../setup";
import {
    taoToRao,
    createHotkey,
    fundAccount,
    waitForBlocks,
    waitUntilBlock,
    getBalance,
    getCurrentAlphaPrice,
    MARKET_PRICE,
    SHORT_LOCKUP_DURATION,
    LONG_LOCKUP_DURATION,
    type Wallet,
} from "../utils";
import {
    createLockupListing,
    expectContractError,
    expectEvent,
    fundingBatch,
    queryOk,
    quoteVault,
    setupLockupSubnet,
    stringifyResult,
    submitOk,
    submitReverted,
    takeLockupListing,
    withIntegrationGas,
} from "../test-helpers";
import { getStakeBalance } from "../utils/stake-helpers";

const LISTING_AMOUNT = taoToRao(10);
const TAKE_AMOUNT = taoToRao(3);
const SELLER_ALPHA = LISTING_AMOUNT * 20n;
const TRANSFER_TOLERANCE = 10n;
// The market price can move between a quote and the take; the excess is refunded.
const PAYMENT_BUFFER = taoToRao(10);

// Variant indexes of the vault's and the factory's Error enums.
const VAULT_CANCEL_TOO_EARLY = 7;
const VAULT_TRADING_PAUSED = 8;
const VAULT_PRICE_BELOW_MINIMUM = 12;
const FACTORY_TRADING_PAUSED = 1;
const FACTORY_LISTING_NOT_FOUND = 16;
const FACTORY_LISTING_STILL_OPEN = 17;
const FACTORY_SUBNET_FROZEN = 19;

describe("Lockup listings v2 - vaults", () => {
    let context: LockupListingsContext;
    let factory: ReturnType<LockupListingsContext["contractSdk"]["getContract"]>;
    let netuid: number;
    let sellerHotkey: Wallet;
    let alice: TestAccount;
    let bob: TestAccount;
    let charlie: TestAccount;
    let dave: TestAccount;
    let eve: TestAccount;

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        factory = context.contractSdk.getContract(context.contractAddress!);
        ({ alice, bob, charlie, dave, eve } = context.accounts);
        ({ netuid, sellerHotkey } = await setupLockupSubnet(context, "vaults", SELLER_ALPHA));
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    function listBobs(terms: { amount?: bigint; lockup_duration?: number; spot?: boolean; min_price?: bigint } = {}) {
        return createLockupListing(context, bob, {
            hotkey: sellerHotkey.address,
            netuid,
            amount: terms.amount ?? LISTING_AMOUNT,
            price_offset_bps: MARKET_PRICE,
            lockup_duration: terms.spot ? undefined : terms.lockup_duration ?? SHORT_LOCKUP_DURATION,
            min_price: terms.min_price,
        });
    }

    async function vaultInfo(vault: any) {
        return queryOk<any>(await vault.query("get_info", { origin: alice.address, data: {} }), "get_info");
    }

    async function stakeOn(hotkey: string, coldkey: string) {
        return getStakeBalance(context.api, hotkey, netuid, coldkey);
    }

    it("creates a listing and funds it in one batch, without a proxy", async () => {
        // Bob never authorized a proxy; his own batch moves the stake.
        const { listingId, vaultAddress, vault, listedAmount } = await listBobs();
        expect(LISTING_AMOUNT - listedAmount).toBeLessThanOrEqual(TRANSFER_TOLERANCE);

        // The vault holds the stake on the factory's validator hotkey.
        const info = await vaultInfo(vault);
        expect(info.status.type).toBe("Open");
        expect(info.seller).toBe(bob.address);
        expect(info.factory).toBe(context.contractAddress);
        expect(info.registry).toBe(context.registryAddress);
        expect(info.hotkey).toBe(alice.address);
        expect(BigInt(info.remaining)).toBe(listedAmount);
        expect(await stakeOn(alice.address, vaultAddress)).toBeGreaterThanOrEqual(listedAmount);

        expect(queryOk<string | undefined>(await factory.query("get_listing", {
            origin: alice.address,
            data: { netuid, seller: bob.address, listing_id: listingId },
        }), "get_listing")).toBe(vaultAddress);
        expect(queryOk<any>(await factory.query("get_vault_listing", {
            origin: alice.address,
            data: { vault: vaultAddress },
        }), "get_vault_listing")).toEqual([netuid, bob.address, listingId]);

        // An open listing cannot be pruned from the index.
        expectContractError(await factory.query("prune_listing", {
            origin: eve.address,
            data: { netuid, seller: bob.address, listing_id: listingId },
        }), FACTORY_LISTING_STILL_OPEN, "prune an open listing");
    }, 300000);

    it("reverts the whole funding batch when activation fails", async () => {
        const created = await submitOk(factory.send("create_listing", withIntegrationGas({
            origin: bob.address,
            data: {
                netuid,
                hotkey: sellerHotkey.address,
                amount: LISTING_AMOUNT,
                price_offset_bps: MARKET_PRICE,
                min_price: 0n,
                lockup_duration: SHORT_LOCKUP_DURATION,
            },
        })), bob.signer, "create_listing");
        const vaultAddress = expectEvent(factory, created, "ListingCreated").value.vault as string;
        const vault = context.vaultSdk.getContract(vaultAddress);
        const bobStakeBefore = await stakeOn(sellerHotkey.address, bob.address);

        // Funding half the listed amount fails activation, which undoes the transfer.
        await submitReverted(await fundingBatch(context, bob, vaultAddress, {
            hotkey: sellerHotkey.address,
            netuid,
            amount: LISTING_AMOUNT / 2n,
        }), bob.signer, "underfunded batch");
        // Rewards may have added to bob's position, but none of it left.
        expect(await stakeOn(sellerHotkey.address, bob.address)).toBeGreaterThanOrEqual(bobStakeBefore);
        expect((await vaultInfo(vault)).status.type).toBe("Pending");

        // The seller cancels the unfunded vault and closes it.
        const cancelled = await submitOk(vault.send("cancel", withIntegrationGas({
            origin: bob.address,
            data: {},
        })), bob.signer, "cancel");
        expectEvent(factory, cancelled, "ListingClosed");
        await submitOk(vault.send("close", withIntegrationGas({ origin: bob.address, data: {} })), bob.signer, "close");
        expect(await TestSetup.getInstance().contractExistsOnChain(context.api, vaultAddress)).toBe(false);
    }, 300000);

    it("takes part of a lockup listing for a recipient, who claims after unlock", async () => {
        const { vault, listedAmount } = await listBobs();
        const quote = await quoteVault(vault, charlie.address, TAKE_AMOUNT);
        const bobBalanceBefore = await getBalance(context.api, bob.address);
        const aliceBalanceBefore = await getBalance(context.api, alice.address);

        const { event, escrowAccount } = await takeLockupListing(vault, charlie, {
            amount: TAKE_AMOUNT,
            recipient: dave.address,
        }, quote.total + PAYMENT_BUFFER);

        expect(event.value.buyer).toBe(charlie.address);
        expect(event.value.recipient).toBe(dave.address);
        expect(BigInt(event.value.amount)).toBe(TAKE_AMOUNT);
        expect(BigInt(event.value.remaining)).toBe(listedAmount - TAKE_AMOUNT);
        expect(escrowAccount).toBeDefined();

        // The seller receives the price and the fee recipient the fee.
        const tao = BigInt(event.value.tao);
        const fee = BigInt(event.value.fee);
        expect(fee).toBeGreaterThan(0n);
        expect(await getBalance(context.api, bob.address) - bobBalanceBefore).toBe(tao);
        expect(await getBalance(context.api, alice.address) - aliceBalanceBefore).toBe(fee);

        // The escrow locks the Alpha for the recipient, on the vault's hotkey.
        const escrow = context.alphaLockupSdk.getContract(escrowAccount!);
        const escrowInfo = queryOk<any>(await escrow.query("get_info", { origin: alice.address, data: {} }), "get_info");
        expect(escrowInfo.buyer).toBe(dave.address);
        expect(escrowInfo.hotkey).toBe(alice.address);
        expect(BigInt(escrowInfo.alpha_amount)).toBe(TAKE_AMOUNT);
        expect(await stakeOn(alice.address, escrowAccount!)).toBeGreaterThanOrEqual(TAKE_AMOUNT - TRANSFER_TOLERANCE);

        await waitUntilBlock(context.api, Number(escrowInfo.unlock_block));
        const daveStakeBefore = await stakeOn(alice.address, dave.address);
        const claimed = await submitOk(escrow.send("claim", withIntegrationGas({
            origin: eve.address,
            data: {},
        })), eve.signer, "claim");
        const claimedAmount = BigInt(expectEvent(escrow, claimed, "AlphaClaimed").value.amount_claimed);
        expect(claimedAmount).toBeGreaterThanOrEqual(TAKE_AMOUNT - TRANSFER_TOLERANCE);
        expect(await stakeOn(alice.address, dave.address) - daveStakeBefore)
            .toBeGreaterThanOrEqual(claimedAmount - TRANSFER_TOLERANCE);
    }, 600000);

    it("sells a spot listing straight to the buyer's recipient", async () => {
        const { vault, listedAmount } = await listBobs({ spot: true });
        expect((await vaultInfo(vault)).lockup_duration).toBeUndefined();
        const quote = await quoteVault(vault, charlie.address, TAKE_AMOUNT);
        const daveStakeBefore = await stakeOn(alice.address, dave.address);

        const { event, escrowAccount } = await takeLockupListing(vault, charlie, {
            amount: TAKE_AMOUNT,
            recipient: dave.address,
        }, quote.total + PAYMENT_BUFFER);

        expect(escrowAccount).toBeUndefined();
        expect(event.value.recipient).toBe(dave.address);
        expect(BigInt(event.value.remaining)).toBe(listedAmount - TAKE_AMOUNT);
        expect(await stakeOn(alice.address, dave.address) - daveStakeBefore)
            .toBeGreaterThanOrEqual(TAKE_AMOUNT - TRANSFER_TOLERANCE);
    }, 300000);

    it("keeps a spot listing open for the minimum age before the seller can cancel", async () => {
        const { vault } = await listBobs({ spot: true });
        const info = await vaultInfo(vault);
        expect(info.cancel_delay).toBe(SPOT_LISTING_AGE);
        const cancellableAfter = Number(info.cancellable_after);
        expect(cancellableAfter).toBeGreaterThan(0);

        expect(Number(await context.api.query.System.Number.getValue())).toBeLessThan(cancellableAfter);
        expectContractError(await vault.query("cancel", {
            origin: bob.address,
            data: {},
        }), VAULT_CANCEL_TOO_EARLY, "cancel before the minimum age");
        await waitUntilBlock(context.api, cancellableAfter);
        const result = await submitOk(vault.send("cancel", withIntegrationGas({
            origin: bob.address,
            data: {},
        })), bob.signer, "cancel after the minimum age");
        expectEvent(vault, result, "Cancelled");
    }, 300000);

    it("lets the owner freeze new listings on a subnet", async () => {
        await submitOk(factory.send("set_subnet_frozen", {
            origin: alice.address,
            data: { netuid, frozen: true, reason: Binary.fromText("subnet at risk") },
        }), alice.signer, "freeze");
        expect(queryOk<boolean>(await factory.query("is_subnet_frozen", {
            origin: alice.address,
            data: { netuid },
        }), "is_subnet_frozen")).toBe(true);

        expectContractError(await factory.query("create_listing", {
            origin: bob.address,
            data: {
                netuid,
                hotkey: sellerHotkey.address,
                amount: LISTING_AMOUNT,
                price_offset_bps: MARKET_PRICE,
                min_price: 0n,
                lockup_duration: undefined,
            },
        }), FACTORY_SUBNET_FROZEN, "create on a frozen subnet");

        await submitOk(factory.send("set_subnet_frozen", {
            origin: alice.address,
            data: { netuid, frozen: false, reason: Binary.fromText("") },
        }), alice.signer, "unfreeze");
    }, 300000);

    it("closes the listing on the final take, and the seller closes the empty vault", async () => {
        const { listingId, vaultAddress, vault, listedAmount } = await listBobs();
        const quote = await quoteVault(vault, charlie.address, listedAmount);

        const { result } = await takeLockupListing(vault, charlie, {
            amount: listedAmount,
            recipient: charlie.address,
        }, quote.total + PAYMENT_BUFFER);

        expectEvent(vault, result, "Filled");
        expectEvent(factory, result, "ListingClosed");
        expect((await vaultInfo(vault)).status.type).toBe("Closed");
        expect(queryOk<string | undefined>(await factory.query("get_listing", {
            origin: alice.address,
            data: { netuid, seller: bob.address, listing_id: listingId },
        }), "get_listing")).toBeUndefined();

        const closed = await submitOk(vault.send("close", withIntegrationGas({
            origin: bob.address,
            data: {},
        })), bob.signer, "close");
        expect(expectEvent(vault, closed, "VaultClosed").value.seller).toBe(bob.address);
        expect(await TestSetup.getInstance().contractExistsOnChain(context.api, vaultAddress)).toBe(false);

        // The vault's report already removed it, so there is nothing to prune.
        expectContractError(await factory.query("prune_listing", {
            origin: eve.address,
            data: { netuid, seller: bob.address, listing_id: listingId },
        }), FACTORY_LISTING_NOT_FOUND, "prune a removed listing");
    }, 300000);

    it("returns the stake with its staking rewards on cancel", async () => {
        const { vaultAddress, vault, listedAmount } = await listBobs({ lockup_duration: LONG_LOCKUP_DURATION });
        await waitForBlocks(context.api, 20);
        const vaultStake = await stakeOn(alice.address, vaultAddress);
        expect(vaultStake).toBeGreaterThanOrEqual(listedAmount);
        const bobStakeBefore = await stakeOn(alice.address, bob.address);

        const result = await submitOk(vault.send("cancel", withIntegrationGas({
            origin: bob.address,
            data: {},
        })), bob.signer, "cancel");
        const returned = BigInt(expectEvent(vault, result, "Cancelled").value.returned);
        expect(returned).toBeGreaterThanOrEqual(vaultStake);
        expect(await stakeOn(alice.address, bob.address) - bobStakeBefore)
            .toBeGreaterThanOrEqual(returned - TRANSFER_TOLERANCE);
        expect(await stakeOn(alice.address, vaultAddress)).toBe(0n);
        expectEvent(factory, result, "ListingClosed");
    }, 300000);

    it("lets anyone sweep TAO sent to a vault to the seller, without closing it", async () => {
        const { vaultAddress, vault } = await listBobs();
        // TAO reaches a vault when its subnet is liquidated, or when someone sends it.
        await fundAccount(context.api, vaultAddress, taoToRao(2), dave.signer);
        const bobBalanceBefore = await getBalance(context.api, bob.address);

        const result = await submitOk(vault.send("sweep_tao", withIntegrationGas({
            origin: eve.address,
            data: {},
        })), eve.signer, "sweep_tao");
        const swept = expectEvent(vault, result, "TaoSwept");
        expect(swept.value.owner).toBe(bob.address);
        expect(BigInt(swept.value.amount)).toBeGreaterThanOrEqual(taoToRao(2));
        expect(await getBalance(context.api, bob.address) - bobBalanceBefore).toBe(BigInt(swept.value.amount));
        expect((await vaultInfo(vault)).status.type).toBe("Open");
    }, 300000);

    it("keeps takes paused while the factory is paused, but never cancels", async () => {
        const { vault } = await listBobs();
        await submitOk(factory.send("pause", { origin: alice.address, data: { blocks: 50 } }), alice.signer, "pause");

        expectContractError(await vault.query("take", {
            origin: charlie.address,
            data: { amount: TAKE_AMOUNT, recipient: charlie.address },
            value: taoToRao(100),
        }), VAULT_TRADING_PAUSED, "take while paused");
        expectContractError(await factory.query("create_listing", {
            origin: bob.address,
            data: {
                netuid,
                hotkey: sellerHotkey.address,
                amount: LISTING_AMOUNT,
                price_offset_bps: MARKET_PRICE,
                min_price: 0n,
                lockup_duration: SHORT_LOCKUP_DURATION,
            },
        }), FACTORY_TRADING_PAUSED, "create while paused");

        await submitOk(vault.send("cancel", withIntegrationGas({ origin: bob.address, data: {} })), bob.signer, "cancel while paused");
        await submitOk(factory.send("resume", { origin: alice.address, data: {} }), alice.signer, "resume");
    }, 300000);

    it("stops a batch that pushes the spot price below the seller's floor before taking", async () => {
        // Charlie builds an Alpha position to sell.
        await submitOk(context.api.tx.SubtensorModule.add_stake({
            hotkey: alice.address,
            netuid,
            amount_staked: taoToRao(500),
        }), charlie.signer, "add_stake");
        await waitForBlocks(context.api, 1);

        // The chain extension and this runtime API report the same spot price.
        const floor = await getCurrentAlphaPrice(context.api, netuid) * 98n / 100n;
        const { vault, listedAmount } = await listBobs({ min_price: floor });
        const charlieAlpha = await stakeOn(alice.address, charlie.address);
        const value = (await quoteVault(vault, charlie.address, TAKE_AMOUNT)).total + PAYMENT_BUFFER;

        // Selling Alpha and taking in one batch fails as a whole.
        const take = vault.send("take", withIntegrationGas({
            origin: charlie.address,
            data: { amount: TAKE_AMOUNT, recipient: charlie.address },
            value,
        }));
        const sell = context.api.tx.SubtensorModule.remove_stake({
            hotkey: alice.address,
            netuid,
            amount_unstaked: charlieAlpha,
        });
        const batch = context.api.tx.Utility.batch_all({ calls: [sell.decodedCall, await take.decodedCall] });
        await submitReverted(batch, charlie.signer, "sell then take");
        // Nothing was sold; rewards may have added to the position since.
        expect(await stakeOn(alice.address, charlie.address)).toBeGreaterThanOrEqual(charlieAlpha);
        expect(BigInt((await vaultInfo(vault)).remaining)).toBe(listedAmount);

        // It was the floor: after the same sale on its own, the take is refused.
        await submitOk(context.api.tx.SubtensorModule.remove_stake({
            hotkey: alice.address,
            netuid,
            amount_unstaked: charlieAlpha,
        }), charlie.signer, "remove_stake");
        expect((await quoteVault(vault, charlie.address, TAKE_AMOUNT)).price).toBeLessThan(floor);
        expectContractError(await vault.query("take", {
            origin: charlie.address,
            data: { amount: TAKE_AMOUNT, recipient: charlie.address },
            value,
        }), VAULT_PRICE_BELOW_MINIMUM, "take below the floor");
    }, 600000);

    it("refuses locked Alpha when funding a vault", async () => {
        // Eve, who holds nothing else on this subnet, stakes on the seller's hotkey,
        // locks all of it and lists all of it. A lock covers a coldkey's whole
        // position on the subnet, and a transfer only carries locked Alpha once it
        // exceeds the unlocked part; contracts cannot accept locked Alpha.
        await submitOk(context.api.tx.SubtensorModule.add_stake({
            hotkey: sellerHotkey.address,
            netuid,
            amount_staked: taoToRao(50),
        }), eve.signer, "add_stake");
        await waitForBlocks(context.api, 1);
        const eveAlpha = await stakeOn(sellerHotkey.address, eve.address);
        const unsafeApi = await TestSetup.getInstance().getUnsafeApi();
        await submitOk(unsafeApi.tx.SubtensorModule.lock_stake({
            hotkey: sellerHotkey.address,
            netuid,
            amount: eveAlpha,
        }), eve.signer, "lock_stake");

        const eveStake = await stakeOn(sellerHotkey.address, eve.address);
        const created = await submitOk(factory.send("create_listing", withIntegrationGas({
            origin: eve.address,
            data: {
                netuid,
                hotkey: sellerHotkey.address,
                amount: eveStake,
                price_offset_bps: MARKET_PRICE,
                min_price: 0n,
                lockup_duration: SHORT_LOCKUP_DURATION,
            },
        })), eve.signer, "create_listing");
        const vaultAddress = expectEvent(factory, created, "ListingCreated").value.vault as string;

        const result = await submitReverted(await fundingBatch(context, eve, vaultAddress, {
            hotkey: sellerHotkey.address,
            netuid,
            amount: eveStake,
        }), eve.signer, "fund with locked Alpha");
        expect(stringifyResult(result.dispatchError)).toContain("AccountRejectsLockedAlpha");
        expect(await stakeOn(sellerHotkey.address, eve.address)).toBeGreaterThanOrEqual(eveStake);
    }, 300000);

    // Runs last: it changes the subnet's validator hotkey, then swaps it.
    it("lets anyone move an open listing to the factory's new validator, and follow it through a hotkey swap", async () => {
        const { vaultAddress, vault, listedAmount } = await listBobs();
        await submitOk(factory.send("set_validator_hotkey", {
            origin: alice.address,
            data: { netuid, hotkey: sellerHotkey.address },
        }), alice.signer, "set_validator_hotkey");

        const result = await submitOk(vault.send("consolidate", withIntegrationGas({
            origin: eve.address,
            data: {},
        })), eve.signer, "consolidate");
        const consolidated = expectEvent(vault, result, "Consolidated");
        expect(consolidated.value.old_hotkey).toBe(alice.address);
        expect(consolidated.value.new_hotkey).toBe(sellerHotkey.address);

        const info = await vaultInfo(vault);
        expect(info.hotkey).toBe(sellerHotkey.address);
        expect(LISTING_AMOUNT - BigInt(info.remaining)).toBeLessThanOrEqual(2n * TRANSFER_TOLERANCE);
        expect(BigInt(info.remaining)).toBeLessThanOrEqual(listedAmount);
        expect(await stakeOn(alice.address, vaultAddress)).toBe(0n);
        expect(await stakeOn(sellerHotkey.address, vaultAddress)).toBeGreaterThanOrEqual(BigInt(info.remaining));

        // The validator swaps its hotkey: the stake moves, and anyone points the
        // vault at it.
        const swappedHotkey = createHotkey("//Bob/vaults/swapped");
        await submitOk(context.api.tx.SubtensorModule.swap_hotkey({
            hotkey: sellerHotkey.address,
            new_hotkey: swappedHotkey.address,
            netuid,
        }), bob.signer, "swap_hotkey");
        expect(await stakeOn(swappedHotkey.address, vaultAddress)).toBeGreaterThan(0n);

        const resynced = await submitOk(vault.send("resync_hotkey", withIntegrationGas({
            origin: eve.address,
            data: { new_hotkey: swappedHotkey.address },
        })), eve.signer, "resync_hotkey");
        expect(expectEvent(vault, resynced, "HotkeyResynced").value.new_hotkey).toBe(swappedHotkey.address);

        // The seller's cancel returns the stake from the new hotkey.
        const bobStakeBefore = await stakeOn(swappedHotkey.address, bob.address);
        const cancelled = await submitOk(vault.send("cancel", withIntegrationGas({
            origin: bob.address,
            data: {},
        })), bob.signer, "cancel after resync");
        const returned = BigInt(expectEvent(vault, cancelled, "Cancelled").value.returned);
        expect(returned).toBeGreaterThanOrEqual(BigInt(info.remaining));
        expect(await stakeOn(swappedHotkey.address, bob.address) - bobStakeBefore)
            .toBeGreaterThanOrEqual(returned - TRANSFER_TOLERANCE);
    }, 300000);
});
