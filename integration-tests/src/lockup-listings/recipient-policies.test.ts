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
    fundAccount,
    getRandomWallet,
    waitUntilBlock,
    MARKET_PRICE,
    SHORT_LOCKUP_DURATION,
    type Wallet,
} from "../utils";
import {
    createLockupListing,
    expectContractError,
    expectEvent,
    listingBatch,
    queryOk,
    quoteVault,
    setupLockupSubnet,
    submitOk,
    submitReverted,
    takeLockupListing,
    withIntegrationGas,
} from "../test-helpers";
import type { ListingTerms } from "../vault-address";

const LISTING_AMOUNT = taoToRao(10);
const TAKE_AMOUNT = taoToRao(3);
const SELLER_ALPHA = LISTING_AMOUNT * 10n;
// Covers a fresh seller's two listings, with room for transfer rounding.
const FRESH_SELLER_ALPHA = LISTING_AMOUNT * 2n + taoToRao(1);
// The market price can move between a quote and the take; the excess is refunded.
const PAYMENT_BUFFER = taoToRao(10);

// Variant indexes of the vault's and the escrow's Error enums.
const VAULT_STAKE_TRANSFER_FAILED = 18;
const VAULT_STAKE_REMAINING = 22;
const ESCROW_STAKE_TRANSFER_FAILED = 3;

describe("Payouts under Subtensor's recipient policies", () => {
    let context: LockupListingsContext;
    let factory: ReturnType<LockupListingsContext["contractSdk"]["getContract"]>;
    let netuid: number;
    let sellerHotkey: Wallet;
    let alice: TestAccount;
    let bob: TestAccount;
    let charlie: TestAccount;
    let eve: TestAccount;

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        factory = context.contractSdk.getContract(context.contractAddress!);
        ({ alice, bob, charlie, eve } = context.accounts);
        ({ netuid, sellerHotkey } = await setupLockupSubnet(context, "policies", SELLER_ALPHA));
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    function listingTerms(lockupDuration: number | undefined): ListingTerms {
        return {
            hotkey: sellerHotkey.address,
            netuid,
            amount: LISTING_AMOUNT,
            price_offset_bps: MARKET_PRICE,
            lockup_duration: lockupDuration,
        };
    }

    /**
     * A new coldkey funded with `tao`. The opt-out only applies to hotkeys a coldkey
     * doesn't stake on yet, on any subnet, so the opted-out accounts must be new
     * even when earlier suites ran on the same node.
     */
    async function freshAccount(tao: bigint): Promise<TestAccount> {
        const wallet = getRandomWallet();
        await fundAccount(context.api, wallet.address, tao, alice.signer);
        return { ...wallet, derivePath: "random" };
    }

    /** Opt `account` in or out of new staking hotkeys that others add to its list. */
    async function rejectStakingHotkeyGrowth(account: TestAccount, enabled: boolean) {
        await submitOk(
            context.api.tx.SubtensorModule.set_reject_staking_hotkey_growth({ enabled }),
            account.signer,
            "set_reject_staking_hotkey_growth",
        );
    }

    async function buy(vault: any, recipient: string) {
        const quote = await quoteVault(vault, charlie.address, TAKE_AMOUNT);
        return takeLockupListing(vault, charlie, { amount: TAKE_AMOUNT, recipient }, quote.total + PAYMENT_BUFFER);
    }

    it("holds payouts to a buyer, beneficiary or seller who rejects new staking hotkeys until they accept them", async () => {
        const { api } = context;

        // The seller stakes only on Bob's hotkey, and the recipient on nothing, so
        // neither holds stake on the vaults' validator hotkey.
        const seller = await freshAccount(taoToRao(100));
        const recipient = await freshAccount(taoToRao(10));
        await submitOk(api.tx.SubtensorModule.transfer_stake({
            destination_coldkey: seller.address,
            hotkey: sellerHotkey.address,
            origin_netuid: netuid,
            destination_netuid: netuid,
            alpha_amount: FRESH_SELLER_ALPHA,
        }), bob.signer, "transfer_stake to the seller");

        // The recipient opts out of others adding hotkeys to their list. A spot
        // take for them fails, refunding the buyer.
        await rejectStakingHotkeyGrowth(recipient, true);
        const spot = await createLockupListing(context, seller, listingTerms(undefined));
        const quote = await quoteVault(spot.vault, charlie.address, TAKE_AMOUNT);
        expectContractError(await spot.vault.query("take", {
            origin: charlie.address,
            data: { amount: TAKE_AMOUNT, recipient: recipient.address },
            value: quote.total + PAYMENT_BUFFER,
        }), VAULT_STAKE_TRANSFER_FAILED, "spot take for an opted-out recipient");

        // A lockup take for them still succeeds: the Alpha goes to a new escrow,
        // and only the claim waits for the recipient.
        const lockup = await createLockupListing(context, seller, listingTerms(SHORT_LOCKUP_DURATION));
        const { escrowAccount, event: lockupTaken } = await buy(lockup.vault, recipient.address);
        const escrow = context.alphaLockupSdk.getContract(escrowAccount!);
        await waitUntilBlock(api, queryOk<number>(await escrow.query("get_unlock_block", {
            origin: alice.address,
            data: {},
        }), "get_unlock_block"));
        expectContractError(await escrow.query("claim", {
            origin: eve.address,
            data: {},
        }), ESCROW_STAKE_TRANSFER_FAILED, "claim for an opted-out beneficiary");

        // Once the recipient accepts new hotkeys again, both pay out.
        await rejectStakingHotkeyGrowth(recipient, false);
        const claimed = await submitOk(escrow.send("claim", withIntegrationGas({
            origin: eve.address,
            data: {},
        })), eve.signer, "claim");
        expect(expectEvent(escrow, claimed, "AlphaClaimed").value.buyer).toBe(recipient.address);
        const { event: taken } = await buy(spot.vault, recipient.address);
        expect(taken.value.recipient).toBe(recipient.address);

        // A seller who opts out can still cancel: the listing closes, and the
        // stake it couldn't return waits in the vault.
        await rejectStakingHotkeyGrowth(seller, true);
        const cancelled = await submitOk(lockup.vault.send("cancel", withIntegrationGas({
            origin: seller.address,
            data: {},
        })), seller.signer, "cancel");
        expect(BigInt(expectEvent(lockup.vault, cancelled, "Cancelled").value.returned)).toBe(0n);
        expectEvent(factory, cancelled, "ListingClosed");
        expectContractError(await lockup.vault.query("sweep_stake", {
            origin: eve.address,
            data: {},
        }), VAULT_STAKE_TRANSFER_FAILED, "sweep to an opted-out seller");
        expectContractError(await lockup.vault.query("close", {
            origin: seller.address,
            data: {},
        }), VAULT_STAKE_REMAINING, "close with the stake held back");

        // When the seller accepts new hotkeys again, anyone sweeps the stake to
        // them, and they close the vault.
        await rejectStakingHotkeyGrowth(seller, false);
        const swept = await submitOk(lockup.vault.send("sweep_stake", withIntegrationGas({
            origin: eve.address,
            data: {},
        })), eve.signer, "sweep_stake");
        expect(BigInt(expectEvent(lockup.vault, swept, "StakeSwept").value.amount))
            .toBeGreaterThanOrEqual(BigInt(lockupTaken.value.remaining));
        await submitOk(lockup.vault.send("close", withIntegrationGas({
            origin: seller.address,
            data: {},
        })), seller.signer, "close");
        expect(await TestSetup.getInstance().contractExistsOnChain(api, lockup.vaultAddress)).toBe(false);
    }, 600000);

    it("blocks funding a vault address reserved as a coldkey-swap destination, and the seller lists at the next one", async () => {
        const { api } = context;
        const terms = listingTerms(SHORT_LOCKUP_DURATION);
        const { vaultAddress: reserved, batch } = await listingBatch(context, bob, terms);

        // A stranger announces a coldkey swap to the vault's predicted address,
        // which reserves it against stake transfers from anyone else.
        const stranger = getRandomWallet();
        await fundAccount(api, stranger.address, taoToRao(10), alice.signer);
        await submitOk(
            api.tx.SubtensorModule.announce_coldkey_swap_v2({ new_coldkey: reserved }),
            stranger.signer,
            "announce_coldkey_swap_v2",
        );

        const failed = await submitReverted(batch, bob.signer, "listing batch to a reserved address");
        expect(failed.dispatchError).toMatchObject({
            type: "Module",
            value: { type: "SubtensorModule", value: { type: "ColdkeySwapDestinationReserved" } },
        });

        // The reverted batch left Bob's nonce unused. Creating the vault on its
        // own still works, so he uses up the nonce by creating, cancelling and
        // closing it, and his next listing goes to a new address.
        const created = await submitOk(factory.send("create_listing", withIntegrationGas({
            origin: bob.address,
            data: { ...terms, min_price: 0n },
        })), bob.signer, "create_listing");
        const pendingAddress = expectEvent(factory, created, "ListingCreated").value.vault as string;
        expect(pendingAddress).toBe(reserved);
        const pending = context.vaultSdk.getContract(pendingAddress);
        await submitOk(pending.send("cancel", withIntegrationGas({ origin: bob.address, data: {} })), bob.signer, "cancel");
        await submitOk(pending.send("close", withIntegrationGas({ origin: bob.address, data: {} })), bob.signer, "close");

        const relisted = await createLockupListing(context, bob, terms);
        expect(relisted.vaultAddress).not.toBe(reserved);
        expect(queryOk<any>(await relisted.vault.query("get_info", {
            origin: alice.address,
            data: {},
        }), "get_info").status.type).toBe("Open");
    }, 300000);
});
