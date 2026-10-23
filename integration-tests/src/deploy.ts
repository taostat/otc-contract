import { Binary, type TypedApi } from "polkadot-api";
import { getPolkadotSigner, type PolkadotSigner } from "polkadot-api/signer";
import { createInkSdk } from "@polkadot-api/sdk-ink";
import { contracts, type devnet } from "@polkadot-api/descriptors";
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import { DEV_PHRASE, entropyToMiniSecret, mnemonicToEntropy, parseSuri, ss58Address } from "@polkadot-labs/hdkd-helpers";
import { randomBytes } from "crypto";
import * as fs from "fs/promises";
import * as path from "path";
import { CODE_KINDS } from "./code-kinds";

type Api = TypedApi<typeof devnet>;

/** The account that signs the deployment */
export interface Deployer {
    address: string;
    signer: PolkadotSigner;
}

/**
 * The sr25519 account for a secret URI, `<mnemonic>[//hard | /soft ...][///password]`.
 * A URI that starts with a path, such as `//Alice`, uses the development phrase.
 */
export function deployerFromSuri(suri: string): Deployer {
    const { phrase, paths, password } = parseSuri(suri.startsWith("/") ? `${DEV_PHRASE}${suri}` : suri);
    if (!phrase) {
        // Never echo the URI: it holds the secret.
        throw new Error("Not a valid secret URI");
    }
    const keypair = sr25519CreateDerive(entropyToMiniSecret(mnemonicToEntropy(phrase), password))(paths ?? "");
    return {
        address: ss58Address(keypair.publicKey, 42),
        signer: getPolkadotSigner(keypair.publicKey, "Sr25519", keypair.sign),
    };
}

export interface DeployConfig {
    /** Where `cargo contract build --release` wrote the artifacts (`target/ink`) */
    artifactsDir: string;
    /** A different code registry build, such as the tests' short-delays one */
    registryArtifactsDir?: string;
    /** Owns the registry and both contracts */
    owner: string;
    /** Can veto registry proposals; must differ from the owner */
    guardian: string;
    /** Registry delays in blocks; at least 7,200 unless built with short-delays */
    upgradeDelay: number;
    forceDelay: number;
    /** U64F64 bits, at most 5% */
    feeRate: bigint;
    feeRecipient: string;
    /** Validator hotkey vaults delegate to, unless a subnet has an override */
    defaultHotkey: string;
    minListingAmount: bigint;
    minPurchaseAmount: bigint;
    minLockupDuration: number;
    maxLockupDuration: number;
    /** Blocks a spot listing stays open before its seller can cancel */
    minListingAge: number;
    minOfferAmount: bigint;
}

export interface Deployment {
    registry: string;
    lockupListings: string;
    otc: string;
    codeHashes: {
        listingVault: string;
        escrow: string;
        lockupListings: string;
        otc: string;
    };
}

/** The code hash a build produces, from its metadata */
export async function buildCodeHash(artifactsDir: string, name: string): Promise<string> {
    const metadata = JSON.parse(await fs.readFile(path.join(artifactsDir, name, `${name}.json`), "utf-8"));
    return metadata.source.hash;
}

export async function codeExistsOnChain(api: Api, codeHash: string): Promise<boolean> {
    return (await api.query.Contracts.CodeInfoOf.getValue(Binary.fromHex(codeHash))) !== undefined;
}

/** The code hash a contract runs, or `undefined` if there is no contract */
export async function contractCodeHash(api: Api, address: string): Promise<string | undefined> {
    const info = await api.query.Contracts.ContractInfoOf.getValue(address);
    return info?.code_hash.asHex();
}

/** Upload a contract's code unless it is already on chain, and return its hash. */
export async function uploadCode(api: Api, deployer: Deployer, artifactsDir: string, name: string): Promise<string> {
    const codeHash = await buildCodeHash(artifactsDir, name);
    if (await codeExistsOnChain(api, codeHash)) {
        return codeHash;
    }

    const wasm = await fs.readFile(path.join(artifactsDir, name, `${name}.wasm`));
    const result = await api.tx.Contracts.upload_code({
        code: Binary.fromBytes(new Uint8Array(wasm)),
        storage_deposit_limit: undefined,
        determinism: { type: "Enforced", value: undefined },
    }).signAndSubmit(deployer.signer);
    if (!result.ok || !(await codeExistsOnChain(api, codeHash))) {
        throw new Error(`Uploading ${name} failed: ${stringify(result)}`);
    }
    return codeHash;
}

/** Dry-run constructor `new(data)` of `name`'s build under a random salt; throws if it fails. */
async function dryRunNew(
    api: Api,
    deployer: Deployer,
    descriptor: any,
    artifactsDir: string,
    name: string,
    data: Record<string, unknown>,
) {
    const wasm = await fs.readFile(path.join(artifactsDir, name, `${name}.wasm`));
    const sdk = createInkSdk(api, descriptor);
    const dryRun = await sdk.getDeployer(Binary.fromBytes(new Uint8Array(wasm))).dryRun("new", {
        origin: deployer.address,
        data,
        options: { salt: Binary.fromBytes(randomBytes(32)) },
    } as any);
    if (!dryRun.success) {
        throw new Error(`Instantiating ${name} would fail: ${stringify(dryRun.value)}`);
    }
    return dryRun.value;
}

/**
 * Instantiate `name` from its build with constructor `new(data)`, under a random
 * salt, and return its address. A constructor that fails its dry run never
 * reaches the chain.
 */
async function instantiate(
    api: Api,
    deployer: Deployer,
    descriptor: any,
    artifactsDir: string,
    name: string,
    data: Record<string, unknown>,
): Promise<string> {
    const dryRun = await dryRunNew(api, deployer, descriptor, artifactsDir, name, data);
    const result = await dryRun.deploy().signAndSubmit(deployer.signer);
    if (!result.ok) {
        throw new Error(`Instantiating ${name} failed: ${stringify(result)}`);
    }
    return dryRun.address;
}

/**
 * Deploy everything, in order: upload the vault and escrow code, deploy the code
 * registry offering every kind's code, then the listing factory and otc, which
 * both answer to that registry. Every constructor is dry-run with the config
 * first, so a config the contracts reject changes nothing on chain. Checks the
 * wiring before returning.
 */
export async function deployAll(api: Api, deployer: Deployer, config: DeployConfig): Promise<Deployment> {
    const codeHashes = {
        listingVault: await buildCodeHash(config.artifactsDir, "listing_vault"),
        escrow: await buildCodeHash(config.artifactsDir, "alpha_lockup"),
        lockupListings: await buildCodeHash(config.artifactsDir, "lockup_listings"),
        otc: await buildCodeHash(config.artifactsDir, "otc_contract"),
    };
    const registryArtifactsDir = config.registryArtifactsDir ?? config.artifactsDir;
    const registryData = {
        owner: config.owner,
        guardian: config.guardian,
        upgrade_delay: config.upgradeDelay,
        force_delay: config.forceDelay,
        initial_codes: registryCodes(codeHashes).map(([kind, codeHash]) => [kind, Binary.fromHex(codeHash)]),
    };
    const factoryData = (registry: string) => ({
        owner: config.owner,
        registry,
        default_hotkey: config.defaultHotkey,
        fee_rate: config.feeRate,
        fee_recipient: config.feeRecipient,
        min_listing_amount: config.minListingAmount,
        min_purchase_amount: config.minPurchaseAmount,
        min_lockup_duration: config.minLockupDuration,
        max_lockup_duration: config.maxLockupDuration,
        min_listing_age: config.minListingAge,
    });
    const otcData = (registry: string) => ({
        owner: config.owner,
        registry,
        fee_rate: config.feeRate,
        fee_recipient: config.feeRecipient,
        min_offer_amount: config.minOfferAmount,
    });

    // Neither constructor calls the registry, so any address checks the rest.
    await dryRunNew(api, deployer, contracts.code_registry, registryArtifactsDir, "code_registry", registryData);
    await dryRunNew(api, deployer, contracts.lockup_listings, config.artifactsDir, "lockup_listings", factoryData(deployer.address));
    await dryRunNew(api, deployer, contracts.otc_contract, config.artifactsDir, "otc_contract", otcData(deployer.address));

    await uploadCode(api, deployer, config.artifactsDir, "listing_vault");
    await uploadCode(api, deployer, config.artifactsDir, "alpha_lockup");
    const registry = await instantiate(api, deployer, contracts.code_registry, registryArtifactsDir, "code_registry", registryData);
    const lockupListings = await instantiate(
        api, deployer, contracts.lockup_listings, config.artifactsDir, "lockup_listings", factoryData(registry),
    );
    const otc = await instantiate(api, deployer, contracts.otc_contract, config.artifactsDir, "otc_contract", otcData(registry));

    const deployment = { registry, lockupListings, otc, codeHashes };
    await verifyDeployment(api, deployer, deployment);
    return deployment;
}

/**
 * Check that the registry offers each kind's code, and that the factory and otc
 * answer to that registry and run the code it offers for them.
 */
export async function verifyDeployment(api: Api, deployer: Deployer, deployment: Deployment): Promise<void> {
    const registry = createInkSdk(api, contracts.code_registry).getContract(deployment.registry);
    for (const [kind, codeHash] of registryCodes(deployment.codeHashes)) {
        const current = await registry.query("current", { origin: deployer.address, data: { kind } });
        const offered = current.success ? current.value.response?.code_hash.asHex() : undefined;
        if (offered !== codeHash) {
            throw new Error(`Registry offers ${offered} for kind ${kind}, expected ${codeHash}`);
        }
    }
    for (const [descriptor, address, codeHash] of [
        [contracts.lockup_listings, deployment.lockupListings, deployment.codeHashes.lockupListings],
        [contracts.otc_contract, deployment.otc, deployment.codeHashes.otc],
    ] as const) {
        const running = await contractCodeHash(api, address);
        if (running !== codeHash) {
            throw new Error(`${address} runs ${running}, but the registry offers ${codeHash}`);
        }
        const answersTo = await createInkSdk(api, descriptor as any).getContract(address).query("get_registry", {
            origin: deployer.address,
            data: {},
        });
        if (!answersTo.success || answersTo.value.response !== deployment.registry) {
            throw new Error(`${address} does not answer to the registry ${deployment.registry}`);
        }
    }
}

/** The code the registry starts with: each kind's build */
function registryCodes(codeHashes: Deployment["codeHashes"]): Array<[number, string]> {
    return [
        [CODE_KINDS.listingVault, codeHashes.listingVault],
        [CODE_KINDS.escrow, codeHashes.escrow],
        [CODE_KINDS.lockupFactory, codeHashes.lockupListings],
        [CODE_KINDS.otc, codeHashes.otc],
    ];
}

function stringify(value: unknown): string {
    return JSON.stringify(value, (_key, item) => (typeof item === "bigint" ? item.toString() : item));
}
