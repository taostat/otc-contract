import { expect } from "vitest";
import type { PolkadotSigner } from "polkadot-api/signer";

// Proxied `transfer_stake` pre-charges its worst-case weight (a full 256-entry
// `StakingHotkeys` walk, roughly 90e9 ref_time) before refunding, so contract calls
// that make it need far more gas than they end up using.
const INTEGRATION_GAS_LIMIT = {
    ref_time: 250_000_000_000n,
    proof_size: 3_000_000n,
};
const INTEGRATION_STORAGE_DEPOSIT_LIMIT = 10_000_000_000_000n;

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

export function expectEvent<T extends string>(contract: any, txResult: any, eventType: T): any {
    const event = contract.filterEvents(txResult.events).find((item: any) => item.type === eventType);
    if (!event) {
        throw new Error(`Missing ${eventType} event in: ${stringifyResult(contract.filterEvents(txResult.events))}`);
    }
    expect(event).toBeDefined();
    return event;
}

export async function createLockupListing(
    contract: any,
    signer: PolkadotSigner,
    origin: string,
    data: {
        hotkey: string;
        netuid: number;
        amount: bigint;
        price_offset_bps: number;
        lockup_duration: number;
    },
) {
    const result = await submitOk(contract.send("create_lockup_listing", withIntegrationGas({ origin, data })), signer, "create_lockup_listing");
    const event = expectEvent(contract, result, "LockupListingCreated");
    return {
        result,
        event,
        listingId: BigInt(event.value.listing_id),
        // Transfers can round down by a few rao; the listing holds what arrived.
        listedAmount: BigInt(event.value.amount),
    };
}

export async function takeLockupListing(
    contract: any,
    signer: PolkadotSigner,
    origin: string,
    data: {
        netuid: number;
        seller: string;
        listing_id: bigint;
        amount: bigint;
    },
    value: bigint,
) {
    const result = await submitOk(contract.send("take_lockup_listing", withIntegrationGas({ origin, data, value })), signer, "take_lockup_listing");
    const event = expectEvent(contract, result, "LockupListingTaken");
    return {
        result,
        event,
        purchaseId: BigInt(event.value.purchase_id),
        escrowAccount: event.value.escrow_account as string,
    };
}

export async function createAlphaListing(
    contract: any,
    signer: PolkadotSigner,
    origin: string,
    data: {
        hotkey: string;
        netuid: number;
        amount: bigint;
        price_offset_bps: number;
    },
) {
    const result = await submitOk(contract.send("list_alpha", withIntegrationGas({ origin, data })), signer, "list_alpha");
    const event = expectEvent(contract, result, "AlphaListed");
    return {
        result,
        event,
        listingId: BigInt(event.value.alpha_listing_id),
    };
}

export async function createTaoOffer(
    contract: any,
    signer: PolkadotSigner,
    origin: string,
    data: {
        netuid: number;
        price_offset_bps: number;
    },
    value: bigint,
) {
    const result = await submitOk(contract.send("create_tao_offer", withIntegrationGas({ origin, data, value })), signer, "create_tao_offer");
    const event = expectEvent(contract, result, "TaoOfferCreated");
    return {
        result,
        event,
        offerId: BigInt(event.value.tao_offer_id),
    };
}
