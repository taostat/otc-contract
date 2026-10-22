import { expect } from "vitest";
import type { PolkadotSigner } from "polkadot-api/signer";
import type { ListingVault, LockupListingsContext, TestAccount } from "./setup";
import {
    createHotkey,
    elevateRegistrationLimits,
    fundAccount,
    registerSubnet,
    registerValidator,
    taoToRao,
    waitForBlocks,
    BITTENSOR_MIN_STAKE,
    type Wallet,
} from "./utils";

// Proxied `transfer_stake` pre-charges its worst-case weight (a full 256-entry
// `StakingHotkeys` walk, roughly 90e9 ref_time) before refunding, so contract calls
// that make it need far more gas than they end up using.
const INTEGRATION_GAS_LIMIT = {
    ref_time: 250_000_000_000n,
    proof_size: 3_000_000n,
};
// pallet-contracts requires the caller to hold the full limit up front. Contract storage
// deposits on Subtensor are tiny, so 10 TAO leaves a wide margin.
const INTEGRATION_STORAGE_DEPOSIT_LIMIT = 10_000_000_000n;

export function withIntegrationGas<T extends Record<string, unknown>>(args: T): T & {
    gasLimit: typeof INTEGRATION_GAS_LIMIT;
    storageDepositLimit: bigint;
} {
    return {
        ...args,
        gasLimit: INTEGRATION_GAS_LIMIT,
        storageDepositLimit: INTEGRATION_STORAGE_DEPOSIT_LIMIT,
    };
}

export function stringifyResult(value: unknown): string {
    return JSON.stringify(value, (_key, item) => {
        if (typeof item === "bigint") {
            return item.toString();
        }
        if (item && typeof item === "object" && "asHex" in item && typeof item.asHex === "function") {
            return item.asHex();
        }
        return item;
    }, 2);
}

export async function submitOk<Tx extends { signAndSubmit: (signer: PolkadotSigner) => Promise<any> }>(
    tx: Tx,
    signer: PolkadotSigner,
    label = "transaction",
): Promise<any> {
    const result = await tx.signAndSubmit(signer);
    if (!result.ok) {
        throw new Error(`${label} failed unexpectedly: ${stringifyResult(result)}`);
    }
    expect(result.ok).toBe(true);
    return result;
}

export async function submitReverted<Tx extends { signAndSubmit: (signer: PolkadotSigner) => Promise<any> }>(
    tx: Tx,
    signer: PolkadotSigner,
    label = "transaction",
): Promise<any> {
    const result = await tx.signAndSubmit(signer);
    if (result.ok) {
        throw new Error(`${label} succeeded unexpectedly: ${stringifyResult(result)}`);
    }
    expect(result.ok).toBe(false);
    return result;
}

export function queryOk<T = any>(result: any, label = "query"): T {
    if (!result.success) {
        throw new Error(`${label} failed unexpectedly: ${stringifyResult(result)}`);
    }
    expect(result.success).toBe(true);
    return result.value.response as T;
}

/**
 * Assert that a dry run reverted with a specific contract error. ink! encodes a
 * message result as Result<Result<T, Error>, LangError>, so Ok(Err(variant)) is
 * 0x00 0x01 followed by the variant's index in the contract's Error enum.
 */
export function expectContractError(result: any, errorIndex: number, label = "query"): void {
    const expected = `0x0001${errorIndex.toString(16).padStart(2, "0")}`;
    const message = result.value?.value?.message;
    const actual = typeof message === "string" ? message : message?.asHex?.();
    if (result.success || result.value?.type !== "FlagReverted" || actual !== expected) {
        throw new Error(`${label}: expected contract error ${expected}, got ${stringifyResult(result)}`);
    }
}

export function expectEvent<T extends string>(contract: any, txResult: any, eventType: T): any {
    const event = contract.filterEvents(txResult.events).find((item: any) => item.type === eventType);
    if (!event) {
        throw new Error(`Missing ${eventType} event in: ${stringifyResult(contract.filterEvents(txResult.events))}`);
    }
    expect(event).toBeDefined();
    return event;
}

export interface LockupListingTerms {
    hotkey: string;
    netuid: number;
    amount: bigint;
    price_offset_bps: number;
    /** Leave out for a spot listing */
    lockup_duration?: number;
    /** TAO per Alpha scaled by 1e9; 0 for no floor */
    min_price?: bigint;
}

/**
 * Create a lockup listing and fund it without a proxy: one batch transfers the
 * seller's stake to the new vault and activates it. Returns the vault and the
 * stake it listed, which can be a rao or two below `amount`.
 */
export interface CreatedListing {
    event: any;
    listingId: bigint;
    vaultAddress: string;
    vault: ListingVault;
    /** What the vault holds and lists, within the transfer tolerance of the amount */
    listedAmount: bigint;
}

export async function createLockupListing(
    context: LockupListingsContext,
    seller: TestAccount,
    terms: LockupListingTerms,
): Promise<CreatedListing> {
    const factory = context.contractSdk.getContract(context.contractAddress!);
    const created = await submitOk(factory.send("create_listing", withIntegrationGas({
        origin: seller.address,
        data: {
            netuid: terms.netuid,
            hotkey: terms.hotkey,
            amount: terms.amount,
            price_offset_bps: terms.price_offset_bps,
            min_price: terms.min_price ?? 0n,
            lockup_duration: terms.lockup_duration,
        },
    })), seller.signer, "create_listing");
    const event = expectEvent(factory, created, "ListingCreated");
    const vaultAddress = event.value.vault as string;
    const vault = context.vaultSdk.getContract(vaultAddress);

    const activated = await fundAndActivate(context, seller, vaultAddress, terms);
    return {
        event,
        listingId: BigInt(event.value.listing_id),
        vaultAddress,
        vault,
        listedAmount: BigInt(activated.value.amount),
    };
}

/** Transfer `amount` to a Pending vault and activate it, in one batch. */
export async function fundAndActivate(
    context: LockupListingsContext,
    seller: TestAccount,
    vaultAddress: string,
    terms: { hotkey: string; netuid: number; amount: bigint },
) {
    const vault = context.vaultSdk.getContract(vaultAddress);
    const result = await submitOk(await fundingBatch(context, seller, vaultAddress, terms), seller.signer, "fund and activate");
    return expectEvent(vault, result, "Activated");
}

/** The `batch_all([transfer_stake, activate])` a seller signs to fund a vault. */
export async function fundingBatch(
    context: LockupListingsContext,
    seller: TestAccount,
    vaultAddress: string,
    terms: { hotkey: string; netuid: number; amount: bigint },
) {
    const transfer = context.api.tx.SubtensorModule.transfer_stake({
        destination_coldkey: vaultAddress,
        hotkey: terms.hotkey,
        origin_netuid: terms.netuid,
        destination_netuid: terms.netuid,
        alpha_amount: terms.amount,
    });
    const activate = context.vaultSdk.getContract(vaultAddress)
        .send("activate", withIntegrationGas({ origin: seller.address, data: {} }));
    return context.api.tx.Utility.batch_all({
        calls: [transfer.decodedCall, await activate.decodedCall],
    });
}

/**
 * Take `amount` from a vault for `recipient`, paying `value`; any excess is
 * refunded. Returns the `Taken` event and, for lockup listings, the escrow.
 */
export async function takeLockupListing(
    vault: any,
    buyer: TestAccount,
    data: { amount: bigint; recipient: string },
    value: bigint,
) {
    const result = await submitOk(vault.send("take", withIntegrationGas({ origin: buyer.address, data, value })), buyer.signer, "take");
    const event = expectEvent(vault, result, "Taken");
    return {
        result,
        event,
        escrowAccount: event.value.escrow as string | undefined,
    };
}

/** What taking `amount` from a vault costs right now */
export async function quoteVault(vault: any, origin: string, amount: bigint) {
    return queryOk<{ price: bigint; tao: bigint; fee: bigint; total: bigint }>(
        await vault.query("quote", { origin, data: { amount } }),
        "quote",
    );
}


/**
 * A fresh subnet for lockup tests: alice, the factory's default validator hotkey,
 * is registered on it, and bob owns `sellerAlpha` on his own registered hotkey to
 * list. Also lowers the factory's minimums to Subtensor's minimum stake.
 */
export async function setupLockupSubnet(
    context: LockupListingsContext,
    label: string,
    sellerAlpha: bigint,
): Promise<{ netuid: number; sellerHotkey: Wallet }> {
    const { api, accounts } = context;
    const factory = context.contractSdk.getContract(context.contractAddress!);
    await submitOk(factory.send("set_min_listing_amount", {
        origin: accounts.alice.address,
        data: { new_amount: BITTENSOR_MIN_STAKE },
    }), accounts.alice.signer, "set_min_listing_amount");
    await submitOk(factory.send("set_min_purchase_amount", {
        origin: accounts.alice.address,
        data: { new_amount: BITTENSOR_MIN_STAKE },
    }), accounts.alice.signer, "set_min_purchase_amount");

    const subnetOwner = createHotkey(`//Alice/${label}`);
    await fundAccount(api, subnetOwner.address, taoToRao(10), accounts.alice.signer);
    const netuid = await registerSubnet(api, subnetOwner.address, accounts.alice.signer);
    await elevateRegistrationLimits(api, netuid, 20, 20, accounts.alice.signer);
    await waitForBlocks(api, 1);

    await registerValidator(api, netuid, accounts.alice.address, accounts.alice.signer, taoToRao(100));

    const sellerHotkey = createHotkey(`//Bob/${label}`);
    await fundAccount(api, sellerHotkey.address, taoToRao(1), accounts.alice.signer);
    await registerValidator(api, netuid, sellerHotkey.address, accounts.bob.signer, taoToRao(5000), sellerAlpha);
    await waitForBlocks(api, 2);

    return { netuid, sellerHotkey };
}
