import { type TypedApi } from "polkadot-api";
import { getInkClient } from "polkadot-api/ink";
import { createInkSdk } from "@polkadot-api/sdk-ink";
import { AccountId, Binary, Blake2256, compactNumber } from "@polkadot-api/substrate-bindings";
import { contracts, type devnet } from "@polkadot-api/descriptors";
import { CODE_KINDS } from "./code-kinds";

type Api = TypedApi<typeof devnet>;

const accountId = AccountId();
const vaultClient = getInkClient(contracts.listing_vault);
const ADDRESS_PREFIX = new TextEncoder().encode("contract_addr_v1");

/** What a seller passes to `create_listing` */
export interface ListingTerms {
    netuid: number;
    hotkey: string;
    amount: bigint;
    price_offset_bps: number;
    min_price?: bigint;
    lockup_duration?: number;
}

/**
 * The address pallet-contracts gives a contract that `deployer` instantiates:
 * the BLAKE2-256 hash of the SCALE-encoded
 * `(b"contract_addr_v1", deployer, code_hash, input_data, salt)`, where
 * `input_data` is the constructor's selector followed by its arguments.
 */
export function contractAddress(deployer: string, codeHash: string, inputData: Uint8Array, salt: Uint8Array): string {
    return accountId.dec(Blake2256(concat([
        ADDRESS_PREFIX,
        accountId.enc(deployer),
        Binary.fromHex(codeHash).asBytes(),
        compactNumber.enc(inputData.length),
        inputData,
        compactNumber.enc(salt.length),
        salt,
    ])));
}

/** The factory's salt for a seller's vault: their account and nonce (u64, little-endian). */
export function vaultSalt(seller: string, nonce: bigint): Uint8Array {
    const salt = new Uint8Array(40);
    salt.set(accountId.enc(seller));
    new DataView(salt.buffer).setBigUint64(32, nonce, true);
    return salt;
}

/**
 * The vault the factory will create for `seller`'s next listing with `terms`,
 * from its settings, the seller's nonce and the registry's vault code as they
 * are now. A frontend can then fund and activate the vault in the same
 * `batch_all` as `create_listing`: if anything changed in between, the vault
 * lands elsewhere, the activation fails and the whole batch reverts. Only the
 * seller's own listings use their nonce, so the one race that doesn't revert is
 * the seller creating a listing with the same terms first: the batch then funds
 * that Pending vault, and leaves its own new one Pending.
 */
export async function predictVaultAddress(api: Api, factoryAddress: string, seller: string, terms: ListingTerms): Promise<string> {
    const factory = createInkSdk(api, contracts.lockup_listings).getContract(factoryAddress);
    const [config, nonce] = await Promise.all([
        factory.query("get_config", { origin: seller, data: {} }),
        factory.query("get_seller_nonce", { origin: seller, data: { seller } }),
    ]);
    if (!config.success || !nonce.success) {
        throw new Error("Querying the factory failed");
    }
    const settings = config.value.response;

    const registry = createInkSdk(api, contracts.code_registry).getContract(settings.registry);
    const current = await registry.query("current", { origin: seller, data: { kind: CODE_KINDS.listingVault } });
    const code = current.success ? current.value.response : undefined;
    if (!code) {
        throw new Error("The registry has no current vault code");
    }

    const inputData = vaultClient.constructor("new").encode({
        seller,
        netuid: terms.netuid,
        hotkey: terms.hotkey,
        amount: terms.amount,
        price_offset_bps: terms.price_offset_bps,
        min_price: terms.min_price ?? 0n,
        fee_rate: settings.fee_rate,
        min_purchase: settings.min_purchase_amount,
        lockup_duration: terms.lockup_duration,
        cancel_delay: terms.lockup_duration === undefined ? settings.min_listing_age : 0,
        registry: settings.registry,
    });
    return contractAddress(factoryAddress, code.code_hash.asHex(), inputData.asBytes(), vaultSalt(seller, nonce.value.response));
}

/**
 * The `Contracts.call` that activates a vault, built without querying it, so it
 * works for a vault that does not exist yet. The limits are explicit, since
 * there is nothing to dry-run against.
 */
export function activateCall(
    api: Api,
    vault: string,
    limits: { gasLimit: { ref_time: bigint; proof_size: bigint }; storageDepositLimit: bigint },
) {
    return api.tx.Contracts.call({
        dest: { type: "Id", value: vault },
        value: 0n,
        gas_limit: limits.gasLimit,
        storage_deposit_limit: limits.storageDepositLimit,
        data: vaultClient.message("activate").encode({}),
    });
}

function concat(parts: Uint8Array[]): Uint8Array {
    const bytes = new Uint8Array(parts.reduce((length, part) => length + part.length, 0));
    let offset = 0;
    for (const part of parts) {
        bytes.set(part, offset);
        offset += part.length;
    }
    return bytes;
}
