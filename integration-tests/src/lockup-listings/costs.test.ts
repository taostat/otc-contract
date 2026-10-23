import { describe, it, expect, beforeAll, afterAll } from "vitest";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    type LockupListingsContext,
    type TestAccount,
} from "../setup";
import {
    taoToRao,
    raoToTao,
    waitUntilBlock,
    MARKET_PRICE,
    SHORT_LOCKUP_DURATION,
    type Wallet,
} from "../utils";
import {
    expectEvent,
    listingBatch,
    queryOk,
    quoteVault,
    setupLockupSubnet,
    submitOk,
    takeLockupListing,
    withIntegrationGas,
} from "../test-helpers";
import type { ListingTerms } from "../vault-address";

const LISTING_AMOUNT = taoToRao(10);
const TAKE_AMOUNT = taoToRao(3);
const SELLER_ALPHA = LISTING_AMOUNT * 10n;
// The market price can move between a quote and the take; the excess is refunded.
const PAYMENT_BUFFER = taoToRao(10);
// About twice the heaviest step measured, so a large regression fails here
// before it nears the 250e9 limit the tests give every call.
const STEP_REF_TIME_BUDGET = 25_000_000_000n;
// The steps the two lifecycles below record.
const MEASURED_STEPS = 10;

/** What one transaction cost, from its events. */
interface Cost {
    step: string;
    signer: string;
    refTime: bigint;
    proofSize: bigint;
    fee: bigint;
    held: bigint;
    released: bigint;
}

/**
 * Measures what each step of a listing's life costs on localnet: the weight it
 * used, the fee it paid, and the storage deposit it held or released. The table
 * it prints is the README's "Costs" section.
 */
describe("Costs of a vault per listing", () => {
    let context: LockupListingsContext;
    let netuid: number;
    let sellerHotkey: Wallet;
    let alice: TestAccount;
    let bob: TestAccount;
    let charlie: TestAccount;
    const costs: Cost[] = [];
    // Net storage deposit held by each contract, by address.
    const deposits = new Map<string, bigint>();

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        ({ alice, bob, charlie } = context.accounts);
        ({ netuid, sellerHotkey } = await setupLockupSubnet(context, "costs", SELLER_ALPHA));
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    function terms(lockupDuration: number | undefined): ListingTerms {
        return {
            hotkey: sellerHotkey.address,
            netuid,
            amount: LISTING_AMOUNT,
            price_offset_bps: MARKET_PRICE,
            lockup_duration: lockupDuration,
        };
    }

    /** Record `result`'s weight, fee and storage deposits under `step`. */
    function record(step: string, signer: string, result: any) {
        const values = (pallet: string, name: string): any[] => result.events
            .filter((event: any) => event.type === pallet && event.value.type === name)
            .map((event: any) => event.value.value);
        const weight = values("System", "ExtrinsicSuccess")[0].dispatch_info.weight;
        const held = values("Contracts", "StorageDepositTransferredAndHeld");
        const released = values("Contracts", "StorageDepositTransferredAndReleased");
        for (const deposit of held) {
            deposits.set(deposit.to, (deposits.get(deposit.to) ?? 0n) + BigInt(deposit.amount));
        }
        for (const deposit of released) {
            deposits.set(deposit.from, (deposits.get(deposit.from) ?? 0n) - BigInt(deposit.amount));
        }
        const sum = (items: any[]) => items.reduce((total, item) => total + BigInt(item.amount), 0n);
        const cost = {
            step,
            signer,
            refTime: BigInt(weight.ref_time),
            proofSize: BigInt(weight.proof_size),
            fee: BigInt(values("TransactionPayment", "TransactionFeePaid")[0].actual_fee),
            held: sum(held),
            released: sum(released),
        };
        costs.push(cost);
        return cost;
    }

    async function buy(vault: any, amount: bigint) {
        const quote = await quoteVault(vault, charlie.address, amount);
        return takeLockupListing(vault, charlie, { amount, recipient: charlie.address }, quote.total + PAYMENT_BUFFER);
    }

    async function open(lockupDuration: number | undefined) {
        const { vaultAddress, batch } = await listingBatch(context, bob, terms(lockupDuration));
        const result = await submitOk(batch, bob.signer, "listing batch");
        const vault = context.vaultSdk.getContract(vaultAddress);
        const listed = BigInt(expectEvent(vault, result, "Activated").value.amount);
        return { vault, vaultAddress, listed, result };
    }

    it("measures a lockup listing from opening to closing, and its escrow", async () => {
        const { api } = context;
        const lockup = await open(SHORT_LOCKUP_DURATION);
        const opened = record("Open a lockup listing (create, fund, activate)", "seller", lockup.result);
        expect(opened.held).toBeGreaterThan(0n);

        const first = await buy(lockup.vault, TAKE_AMOUNT);
        const escrowTake = record("Take from a lockup listing (creates an escrow)", "buyer", first.result);
        expect(escrowTake.held).toBeGreaterThan(0n);

        const rest = await buy(lockup.vault, BigInt(first.event.value.remaining));
        record("Take the rest (creates an escrow, closes the listing)", "buyer", rest.result);
        expect(BigInt(rest.event.value.remaining)).toBe(0n);

        const closed = await submitOk(lockup.vault.send("close", withIntegrationGas({
            origin: bob.address,
            data: {},
        })), bob.signer, "close vault");
        record("Close the vault", "seller", closed);
        expect(deposits.get(lockup.vaultAddress)).toBe(0n);

        const escrow = context.alphaLockupSdk.getContract(first.escrowAccount!);
        await waitUntilBlock(api, queryOk<number>(await escrow.query("get_unlock_block", {
            origin: alice.address,
            data: {},
        }), "get_unlock_block"));
        const claimed = await submitOk(escrow.send("claim", withIntegrationGas({
            origin: charlie.address,
            data: {},
        })), charlie.signer, "claim");
        record("Claim an escrow", "anyone", claimed);
        const escrowClosed = await submitOk(escrow.send("close", withIntegrationGas({
            origin: charlie.address,
            data: {},
        })), charlie.signer, "close escrow");
        record("Close an escrow", "beneficiary", escrowClosed);
        expect(deposits.get(first.escrowAccount!)).toBe(0n);
    }, 600000);

    it("measures a spot listing that is taken from and then cancelled", async () => {
        const { api } = context;
        const spot = await open(undefined);
        record("Open a spot listing (create, fund, activate)", "seller", spot.result);

        const taken = await buy(spot.vault, TAKE_AMOUNT);
        const spotTake = record("Take from a spot listing", "buyer", taken.result);
        expect(spotTake.held).toBe(0n);

        const info = queryOk<any>(await spot.vault.query("get_info", { origin: alice.address, data: {} }), "get_info");
        await waitUntilBlock(api, Number(info.cancellable_after));
        const cancelled = await submitOk(spot.vault.send("cancel", withIntegrationGas({
            origin: bob.address,
            data: {},
        })), bob.signer, "cancel");
        record("Cancel a listing", "seller", cancelled);
        const closed = await submitOk(spot.vault.send("close", withIntegrationGas({
            origin: bob.address,
            data: {},
        })), bob.signer, "close vault");
        record("Close the vault after a cancel", "seller", closed);
        expect(deposits.get(spot.vaultAddress)).toBe(0n);
    }, 600000);

    it("keeps every step within its gas budget, and prints the table", () => {
        expect(costs.length).toBe(MEASURED_STEPS);
        for (const cost of costs) {
            expect(cost.refTime, cost.step).toBeLessThan(STEP_REF_TIME_BUDGET);
        }
        const tao = (rao: bigint) => (rao === 0n ? "–" : raoToTao(rao));
        const rows = costs.map((cost) => `| ${cost.step} | ${cost.signer} | ${cost.refTime} | ${cost.proofSize} | ${tao(cost.fee)} | ${tao(cost.held)} | ${tao(cost.released)} |`);
        console.log([
            "| Step | Signer | ref_time | proof_size | Fee (TAO) | Deposit held (TAO) | Deposit released (TAO) |",
            "|---|---|---:|---:|---:|---:|---:|",
            ...rows,
        ].join("\n"));
    });
});
