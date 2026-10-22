import { createClient, type PolkadotClient as Client, type TypedApi, Binary, TxEvent, TxFinalized } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider/web";
import { createInkSdk } from "@polkadot-api/sdk-ink";
import { devnet, contracts, MultiAddress } from "@polkadot-api/descriptors";
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import { DEV_PHRASE, entropyToMiniSecret, mnemonicToEntropy, ss58Address } from "@polkadot-labs/hdkd-helpers";
import { getPolkadotSigner, type PolkadotSigner } from "polkadot-api/signer";
import { randomBytes } from "crypto";
import * as fs from "fs/promises";
import * as fsSync from "fs";
import * as path from "path";
import { Observable } from "rxjs";

export type ContractSdk = ReturnType<typeof createInkSdk<TypedApi<typeof devnet>, typeof contracts.otc_contract>>;
export type LockupListingsSdk = ReturnType<typeof createInkSdk<TypedApi<typeof devnet>, typeof contracts.lockup_listings>>;
export type AlphaLockupSdk = ReturnType<typeof createInkSdk<TypedApi<typeof devnet>, typeof contracts.alpha_lockup>>;
export type CodeRegistrySdk = ReturnType<typeof createInkSdk<TypedApi<typeof devnet>, typeof contracts.code_registry>>;
export type ListingVaultSdk = ReturnType<typeof createInkSdk<TypedApi<typeof devnet>, typeof contracts.listing_vault>>;
export type ListingVault = ReturnType<ListingVaultSdk["getContract"]>;

// Code kind ids from otc-shared/src/upgrade.rs
export const LISTING_VAULT_CODE_KIND = 1;
export const ESCROW_CODE_KIND = 2;
export const LOCKUP_FACTORY_CODE_KIND = 3;
// 0.5%, as U64F64 bits
export const FACTORY_FEE_RATE = 92233720368547758n;
// The registry's minimum delay (MIN_DELAY) in a production build: about a day.
export const REGISTRY_DELAY = 7_200;

/** Which code registry build to deploy, and its delays */
export interface RegistryBuild {
    artifactsDir: string;
    upgradeDelay: number;
    forceDelay: number;
}

export const RELEASE_ARTIFACTS = path.join(process.cwd(), "..", "target", "ink");
// Built by `npm run build:contracts`: an escrow with a newer storage version, so
// tests have real code to upgrade escrows to, and a code registry built with
// `short-delays`, so tests can activate and force upgrades within a few blocks.
export const UPGRADE_TEST_ARTIFACTS = path.join(process.cwd(), "..", "target", "upgrade-test", "ink");

const RELEASE_REGISTRY: RegistryBuild = {
    artifactsDir: RELEASE_ARTIFACTS,
    upgradeDelay: REGISTRY_DELAY,
    forceDelay: REGISTRY_DELAY,
};

// Contract address persistence files
const CONTRACT_ADDRESS_FILE = path.join(process.cwd(), ".contract-address");
const LOCKUP_LISTINGS_ADDRESS_FILE = path.join(process.cwd(), ".lockup-listings-address");
// Deployment caching is opt-in so every run gets fresh contracts by default.
const REUSE_DEPLOYMENT_CACHE = process.env.OTC_TEST_REUSE_DEPLOYMENTS === "1";
// Tests move TAO amounts that depend on dynamic subnet prices, so every test account
// starts well funded rather than failing on balance when prices move.
const TEST_ACCOUNT_TOP_UP = 1_000_000_000_000_000n; // 1,000,000 TAO

// A random salt lets the same code and constructor arguments deploy again on one chain.
function randomDeploymentSalt(): any {
    return Binary.fromBytes(randomBytes(32)) as any;
}

// Load contract address from file if it exists
function loadContractAddress(): string | null {
    if (!REUSE_DEPLOYMENT_CACHE) {
        return null;
    }

    try {
        if (fsSync.existsSync(CONTRACT_ADDRESS_FILE)) {
            const address = fsSync.readFileSync(CONTRACT_ADDRESS_FILE, 'utf-8').trim();
            console.log(`Loaded contract address from file: ${address}`);
            return address;
        }
    } catch (error) {
        console.error("Failed to load contract address:", error);
    }
    return null;
}

// Save contract address to file
function saveContractAddress(address: string): void {
    if (!REUSE_DEPLOYMENT_CACHE) {
        return;
    }

    try {
        fsSync.writeFileSync(CONTRACT_ADDRESS_FILE, address, 'utf-8');
        console.log(`Saved contract address to file: ${address}`);
    } catch (error) {
        console.error("Failed to save contract address:", error);
    }
}

// Load lockup listings address from file
function loadLockupListingsAddress(): string | null {
    if (!REUSE_DEPLOYMENT_CACHE) {
        return null;
    }

    try {
        if (fsSync.existsSync(LOCKUP_LISTINGS_ADDRESS_FILE)) {
            const address = fsSync.readFileSync(LOCKUP_LISTINGS_ADDRESS_FILE, 'utf-8').trim();
            console.log(`Loaded lockup listings address from file: ${address}`);
            return address;
        }
    } catch (error) {
        console.error("Failed to load lockup listings address:", error);
    }
    return null;
}

// Save lockup listings address to file
function saveLockupListingsAddress(address: string): void {
    if (!REUSE_DEPLOYMENT_CACHE) {
        return;
    }

    try {
        fsSync.writeFileSync(LOCKUP_LISTINGS_ADDRESS_FILE, address, 'utf-8');
        console.log(`Saved lockup listings address to file: ${address}`);
    } catch (error) {
        console.error("Failed to save lockup listings address:", error);
    }
}

export interface TestContext {
    api: TypedApi<typeof devnet>;
    contractSdk: ContractSdk;
    accounts: {
        alice: TestAccount;
        bob: TestAccount;
        charlie: TestAccount;
        dave: TestAccount;
        eve: TestAccount;
    };
    contractAddress?: string;
}

export interface LockupListingsContext {
    api: TypedApi<typeof devnet>;
    contractSdk: LockupListingsSdk;
    alphaLockupSdk: AlphaLockupSdk;
    accounts: {
        alice: TestAccount;
        bob: TestAccount;
        charlie: TestAccount;
        dave: TestAccount;
        eve: TestAccount;
    };
    vaultSdk: ListingVaultSdk;
    /** The lockup factory */
    contractAddress?: string;
    escrowCodeHash?: string;
    vaultCodeHash: string;
    registrySdk: CodeRegistrySdk;
    registryAddress: string;
}

export interface TestAccount {
    address: string;
    signer: PolkadotSigner;
    derivePath: string;
}

export class TestSetup {
    private static instance: TestSetup | null = null;
    private client: Client | null = null;
    private api: TypedApi<typeof devnet> | null = null;

    private constructor() { }

    static getInstance(): TestSetup {
        if (!TestSetup.instance) {
            TestSetup.instance = new TestSetup();
        }
        return TestSetup.instance;
    }

    async getApi(): Promise<TypedApi<typeof devnet>> {
        if (this.client && this.api) {
            return this.api;
        }

        const wsUrl = process.env.CONTRACTS_NODE_URL || "ws://127.0.0.1:9944";
        console.log(`Connecting to node at ${wsUrl}...`);

        const provider = getWsProvider(wsUrl);
        this.client = createClient(provider);
        this.api = this.client.getTypedApi(devnet);

        return this.api;
    }

    /**
     * The untyped API, for runtime calls newer than the generated descriptors
     */
    async getUnsafeApi(): Promise<any> {
        await this.getApi();
        return this.client!.getUnsafeApi();
    }

    createTestAccounts(): TestContext['accounts'] {
        const accounts = {
            alice: this.createAccount("//Alice"),
            bob: this.createAccount("//Bob"),
            charlie: this.createAccount("//Charlie"),
            dave: this.createAccount("//Dave"),
            eve: this.createAccount("//Eve"),
        };

        console.log("Test accounts created:");
        Object.entries(accounts).forEach(([name, account]) => {
            console.log(`  ${name}: ${account.address}`);
        });

        return accounts;
    }

    private createAccount(derivePath: string): TestAccount {
        const entropy = mnemonicToEntropy(DEV_PHRASE);
        const miniSecret = entropyToMiniSecret(entropy);
        const derive = sr25519CreateDerive(miniSecret);
        const keypair = derive(derivePath);

        const signer = getPolkadotSigner(
            keypair.publicKey,
            "Sr25519",
            keypair.sign
        );

        const address = ss58Address(keypair.publicKey, 42);

        return {
            address,
            signer,
            derivePath
        };
    }

    /**
     * Deploy the OTC contract
     */
    async deployContract(api: TypedApi<typeof devnet>, accounts: TestContext['accounts']): Promise<string> {
        const contractPath = path.join(process.cwd(), "..", "target", "ink", "otc_contract", "otc_contract.wasm");
        const wasmFile = await fs.readFile(contractPath);
        const wasmBytes = Binary.fromBytes(new Uint8Array(wasmFile));

        const contractSdk = createInkSdk(api, contracts.otc_contract);
        const deployer = contractSdk.getDeployer(wasmBytes);

        const constructorArgs = {
            owner: accounts.alice.address,
            hotkey: accounts.eve.address,
            fee_rate: 92233720368547758n, // 0.5% as U64F64 bits (0.005 * 2^64)
            min_listing_amount: 1_000_000_000n, // 1 Alpha
            min_offer_amount: 1_000_000_000n, // 1 TAO
            min_listing_age: 100, // 100 blocks
        };

        const cachedAddress = loadContractAddress();
        if (cachedAddress && await this.contractExistsOnChain(api, cachedAddress)) {
            console.log(`Using existing contract at ${cachedAddress}`);
            return cachedAddress;
        }

        try {
            const dryRunResult = await deployer.dryRun("new", {
                origin: accounts.alice.address,
                data: constructorArgs,
                options: { salt: randomDeploymentSalt() },
            });

            if (!dryRunResult.success) {
                const errorType = dryRunResult.value?.value?.value?.type;
                console.log("Dry run failed with error:", errorType);
                console.log("Full dry run result:", JSON.stringify(dryRunResult, null, 2));
                console.log("Dry run did not succeed", dryRunResult.value)
                return Promise.reject(new Error(`Dry run failed: ${errorType || 'Unknown'}`));
            }

            // Deploy the contract
            console.log(`Deploying new contract...`)
            const fin = await this.trackTx(
                dryRunResult.value.deploy().signSubmitAndWatch(accounts.alice.signer),
            )
            console.log(`Deployed to address ${dryRunResult.value.address}`)
            console.log(contractSdk.readDeploymentEvents(fin.events))

            const contractAddress = dryRunResult.value.address;
            saveContractAddress(contractAddress);
            return contractAddress;
        } catch (error) {
            console.error("Deployment error:", error);
            throw error;
        }
    }


    async trackTx(obs: Observable<TxEvent>): Promise<TxFinalized> {
        return new Promise<TxFinalized>((resolve, reject) =>
            obs.subscribe({
                next: (evt) => {
                    console.log(evt.type)
                    if (evt.type === "finalized") {
                        resolve(evt)
                    }
                },
                error: (err) => reject(err),
            }),
        )
    }

    /**
     * Check if contract code exists on-chain
     */
    async contractExistsOnChain(
        api: TypedApi<typeof devnet>,
        address: string
    ): Promise<boolean> {
        try {
            const contractInfo = await api.query.Contracts.ContractInfoOf.getValue(address);
            return contractInfo !== undefined;
        } catch {
            return false;
        }
    }

    async codeExistsOnChain(
        api: TypedApi<typeof devnet>,
        codeHash: string
    ): Promise<boolean> {
        try {
            const codeHashBinary = Binary.fromHex(codeHash);
            const codeInfo = await api.query.Contracts.CodeInfoOf.getValue(codeHashBinary);
            return codeInfo !== undefined;
        } catch {
            return false;
        }
    }

    /**
     * Upload a contract's code unless it is already on chain, and return its code hash.
     * The hash comes from the build's metadata, which pallet-contracts reproduces.
     */
    async uploadCode(
        api: TypedApi<typeof devnet>,
        accounts: TestContext['accounts'],
        artifactsDir: string,
        name: string,
    ): Promise<string> {
        const wasmPath = path.join(artifactsDir, name, `${name}.wasm`);
        const metadataPath = path.join(artifactsDir, name, `${name}.json`);
        const metadata = JSON.parse(await fs.readFile(metadataPath, "utf-8"));
        const codeHash: string = metadata.source.hash;

        if (await this.codeExistsOnChain(api, codeHash)) {
            console.log(`${name} code already on chain: ${codeHash}`);
            return codeHash;
        }

        console.log(`Uploading ${name} code from ${wasmPath}...`);
        const wasmBytes = Binary.fromBytes(new Uint8Array(await fs.readFile(wasmPath)));
        await this.trackTx(
            api.tx.Contracts.upload_code({
                code: wasmBytes,
                storage_deposit_limit: undefined,
                determinism: { type: "Enforced", value: undefined },
            }).signSubmitAndWatch(accounts.alice.signer)
        );

        if (!(await this.codeExistsOnChain(api, codeHash))) {
            throw new Error(`Uploaded ${name} code but ${codeHash} is not on chain`);
        }
        return codeHash;
    }

    /**
     * The code hash a contract build produces, from its metadata
     */
    async buildCodeHash(artifactsDir: string, name: string): Promise<string> {
        const metadataPath = path.join(artifactsDir, name, `${name}.json`);
        return JSON.parse(await fs.readFile(metadataPath, "utf-8")).source.hash;
    }

    /**
     * Deploy a code registry. Alice is the owner and Eve the guardian.
     */
    async deployCodeRegistry(
        api: TypedApi<typeof devnet>,
        accounts: TestContext['accounts'],
        initialCodes: Array<[number, string]>,
        build: RegistryBuild = RELEASE_REGISTRY,
    ): Promise<string> {
        const wasmPath = path.join(build.artifactsDir, "code_registry", "code_registry.wasm");
        const wasmBytes = Binary.fromBytes(new Uint8Array(await fs.readFile(wasmPath)));
        const deployer = createInkSdk(api, contracts.code_registry).getDeployer(wasmBytes);

        const dryRunResult = await deployer.dryRun("new", {
            origin: accounts.alice.address,
            data: {
                owner: accounts.alice.address,
                guardian: accounts.eve.address,
                upgrade_delay: build.upgradeDelay,
                force_delay: build.forceDelay,
                initial_codes: initialCodes.map(([kind, codeHash]) => [kind, Binary.fromHex(codeHash)]),
            } as any,
            options: { salt: randomDeploymentSalt() },
        });
        if (!dryRunResult.success) {
            throw new Error(`Code registry dry run failed: ${JSON.stringify(dryRunResult.value, (_key, value) =>
                typeof value === "bigint" ? value.toString() : value)}`);
        }

        await this.trackTx(dryRunResult.value.deploy().signSubmitAndWatch(accounts.alice.signer));
        console.log(`Code registry deployed to address ${dryRunResult.value.address}`);
        return dryRunResult.value.address;
    }

    /**
     * Deploy the lockup factory. Alice owns it, receives its fees and is the
     * default validator hotkey.
     */
    async deployLockupListingsContract(
        api: TypedApi<typeof devnet>,
        accounts: TestContext['accounts'],
        registryAddress: string,
    ): Promise<string> {
        const wasmPath = path.join(RELEASE_ARTIFACTS, "lockup_listings", "lockup_listings.wasm");
        const wasmBytes = Binary.fromBytes(new Uint8Array(await fs.readFile(wasmPath)));
        const deployer = createInkSdk(api, contracts.lockup_listings).getDeployer(wasmBytes);

        const dryRunResult = await deployer.dryRun("new", {
            origin: accounts.alice.address,
            data: {
                owner: accounts.alice.address,
                registry: registryAddress,
                default_hotkey: accounts.alice.address,
                fee_rate: FACTORY_FEE_RATE,
                fee_recipient: accounts.alice.address,
                min_listing_amount: 1_000_000_000n, // 1 Alpha
                min_purchase_amount: 100_000_000n, // 0.1 Alpha
                min_lockup_duration: 10, // 10 blocks for testing (short lockup)
                max_lockup_duration: 1_000_000, // ~46 days
            },
            options: { salt: randomDeploymentSalt() },
        });
        if (!dryRunResult.success) {
            throw new Error(`Lockup factory dry run failed: ${JSON.stringify(dryRunResult.value, (_key, value) =>
                typeof value === "bigint" ? value.toString() : value)}`);
        }

        await this.trackTx(dryRunResult.value.deploy().signSubmitAndWatch(accounts.alice.signer));
        console.log(`Lockup factory deployed to address ${dryRunResult.value.address}`);
        return dryRunResult.value.address;
    }

    /**
     * Deploy the lockup factory with a code registry offering the released vault,
     * escrow and factory code. `registry` picks a test registry build, such as one
     * with short delays for upgrade tests.
     */
    async createLockupListingsContext(options: { registry?: RegistryBuild } = {}): Promise<LockupListingsContext> {
        const api = await this.getApi();
        const accounts = this.createTestAccounts();
        await this.fundTestAccounts(api, accounts);

        // Vaults and escrows are instantiated from uploaded code.
        const escrowCodeHash = await this.uploadCode(api, accounts, RELEASE_ARTIFACTS, "alpha_lockup");
        let vaultCodeHash = await this.uploadCode(api, accounts, RELEASE_ARTIFACTS, "listing_vault");
        const contractSdk = createInkSdk(api, contracts.lockup_listings);
        const registrySdk = createInkSdk(api, contracts.code_registry);

        // A custom registry gets its own deployment, outside the shared cache.
        const useCache = options.registry === undefined;
        const cachedAddress = useCache ? loadLockupListingsAddress() : null;
        let contractAddress: string;
        let registryAddress: string;
        if (cachedAddress && await this.contractExistsOnChain(api, cachedAddress)) {
            console.log(`Using existing lockup listings contract at ${cachedAddress}`);
            contractAddress = cachedAddress;
            const registryResult = await contractSdk.getContract(contractAddress).query("get_registry", {
                origin: accounts.alice.address,
                data: {},
            });
            if (!registryResult.success) {
                throw new Error("Failed to read the cached lockup listings contract's registry");
            }
            registryAddress = registryResult.value.response;

            // New vaults come from the cached registry, which may predate this build.
            const offered = await registrySdk.getContract(registryAddress).query("current", {
                origin: accounts.alice.address,
                data: { kind: LISTING_VAULT_CODE_KIND },
            });
            const offeredHash = offered.success ? offered.value.response?.code_hash.asHex() : undefined;
            if (offeredHash && offeredHash !== vaultCodeHash) {
                console.warn(`Cached registry offers vault code ${offeredHash}, not this build's ${vaultCodeHash}`);
                vaultCodeHash = offeredHash;
            }
        } else {
            registryAddress = await this.deployCodeRegistry(api, accounts, [
                [LISTING_VAULT_CODE_KIND, vaultCodeHash],
                [ESCROW_CODE_KIND, escrowCodeHash],
                [LOCKUP_FACTORY_CODE_KIND, await this.buildCodeHash(RELEASE_ARTIFACTS, "lockup_listings")],
            ], options.registry);
            contractAddress = await this.deployLockupListingsContract(api, accounts, registryAddress);
            if (useCache) {
                saveLockupListingsAddress(contractAddress);
            }
        }

        return {
            api,
            contractSdk,
            alphaLockupSdk: createInkSdk(api, contracts.alpha_lockup),
            vaultSdk: createInkSdk(api, contracts.listing_vault),
            accounts,
            contractAddress,
            escrowCodeHash,
            vaultCodeHash,
            registrySdk,
            registryAddress,
        };
    }

    async cleanup(): Promise<void> {
        if (this.client) {
            this.client.destroy();
            this.client = null;
            this.api = null;
        }
    }

    async createTestContext(): Promise<TestContext> {
        const api = await this.getApi();
        const accounts = this.createTestAccounts();
        await this.fundTestAccounts(api, accounts);
        const contractSdk = createInkSdk(api, contracts.otc_contract);

        const context: TestContext = {
            api,
            contractSdk,
            accounts,
        };

        const contractAddress = await this.deployContract(api, accounts);
        context.contractAddress = contractAddress;

        return context;
    }

    private async fundTestAccounts(
        api: TypedApi<typeof devnet>,
        accounts: TestContext['accounts']
    ): Promise<void> {
        const calls = [accounts.bob, accounts.charlie, accounts.dave, accounts.eve].map((account) =>
            api.tx.Balances.force_set_balance({
                who: MultiAddress.Id(account.address),
                new_free: TEST_ACCOUNT_TOP_UP,
            }).decodedCall
        );
        // One sudo batch: batch_all reverts every top-up if any of them fails.
        const batch = api.tx.Utility.batch_all({ calls });
        const result = await api.tx.Sudo.sudo({ call: batch.decodedCall }).signAndSubmit(accounts.alice.signer);
        const sudid = result.events.find((event: any) => event.type === "Sudo" && event.value.type === "Sudid");
        if (!result.ok || !sudid || sudid.value.value.success === false) {
            throw new Error("Failed to top up test accounts");
        }
    }
}

export function bigintReplacer(_key: any, value: any) {
    console.log(JSON.stringify(value, (_, value) =>
        typeof value === 'bigint' ? value.toString() : value
        , 2));
}

export async function setupTestEnvironment(): Promise<TestContext> {
    const setup = TestSetup.getInstance();
    return await setup.createTestContext();
}

export async function cleanupTestEnvironment(): Promise<void> {
    const setup = TestSetup.getInstance();
    await setup.cleanup();
}

export async function setupLockupListingsEnvironment(
    options: { registry?: RegistryBuild } = {}
): Promise<LockupListingsContext> {
    const setup = TestSetup.getInstance();
    return await setup.createLockupListingsContext(options);
}
