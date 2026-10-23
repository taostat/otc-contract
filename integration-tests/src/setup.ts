import { createClient, type PolkadotClient as Client, type TypedApi } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider/web";
import { createInkSdk } from "@polkadot-api/sdk-ink";
import { devnet, contracts, MultiAddress } from "@polkadot-api/descriptors";
import type { PolkadotSigner } from "polkadot-api/signer";
import * as fsSync from "fs";
import * as path from "path";
import { CODE_KINDS } from "./code-kinds";
import {
    buildCodeHash,
    codeExistsOnChain,
    contractCodeHash,
    deployAll,
    deployerFromSuri,
    uploadCode,
    type DeployConfig,
    type Deployment,
} from "./deploy";

export type ContractSdk = ReturnType<typeof createInkSdk<TypedApi<typeof devnet>, typeof contracts.otc_contract>>;
export type LockupListingsSdk = ReturnType<typeof createInkSdk<TypedApi<typeof devnet>, typeof contracts.lockup_listings>>;
export type AlphaLockupSdk = ReturnType<typeof createInkSdk<TypedApi<typeof devnet>, typeof contracts.alpha_lockup>>;
export type CodeRegistrySdk = ReturnType<typeof createInkSdk<TypedApi<typeof devnet>, typeof contracts.code_registry>>;
export type ListingVaultSdk = ReturnType<typeof createInkSdk<TypedApi<typeof devnet>, typeof contracts.listing_vault>>;
export type ListingVault = ReturnType<ListingVaultSdk["getContract"]>;

export const LISTING_VAULT_CODE_KIND = CODE_KINDS.listingVault;
export const ESCROW_CODE_KIND = CODE_KINDS.escrow;
export const LOCKUP_FACTORY_CODE_KIND = CODE_KINDS.lockupFactory;
export const OTC_CODE_KIND = CODE_KINDS.otc;
// How long spot listings must stay open before the seller can cancel, in blocks:
// long enough to check a cancel is refused right after funding.
export const SPOT_LISTING_AGE = 30;
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
// Built by `npm run build:contracts`: an escrow and a vault with a newer storage
// version, so tests have real code to upgrade positions to, and a code registry
// built with `short-delays`, so tests can activate and force upgrades within a
// few blocks.
export const UPGRADE_TEST_ARTIFACTS = path.join(process.cwd(), "..", "target", "upgrade-test", "ink");

// Contract address persistence files
const CONTRACT_ADDRESS_FILE = path.join(process.cwd(), ".contract-address");
const LOCKUP_LISTINGS_ADDRESS_FILE = path.join(process.cwd(), ".lockup-listings-address");
// Deployment caching is opt-in so every run gets fresh contracts by default.
const REUSE_DEPLOYMENT_CACHE = process.env.OTC_TEST_REUSE_DEPLOYMENTS === "1";
// Tests move TAO amounts that depend on dynamic subnet prices, so every test account
// starts well funded rather than failing on balance when prices move.
const TEST_ACCOUNT_TOP_UP = 1_000_000_000_000_000n; // 1,000,000 TAO

function loadCachedAddress(file: string): string | null {
    if (!REUSE_DEPLOYMENT_CACHE || !fsSync.existsSync(file)) {
        return null;
    }
    const address = fsSync.readFileSync(file, "utf-8").trim();
    console.log(`Loaded cached address ${address} from ${path.basename(file)}`);
    return address;
}

function saveCachedAddress(file: string, address: string): void {
    if (REUSE_DEPLOYMENT_CACHE) {
        fsSync.writeFileSync(file, address, "utf-8");
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
    registryAddress?: string;
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
    /** The listing factory */
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
        return { ...deployerFromSuri(derivePath), derivePath };
    }

    async contractExistsOnChain(api: TypedApi<typeof devnet>, address: string): Promise<boolean> {
        return (await contractCodeHash(api, address)) !== undefined;
    }

    async codeExistsOnChain(api: TypedApi<typeof devnet>, codeHash: string): Promise<boolean> {
        return codeExistsOnChain(api, codeHash);
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
        return uploadCode(api, accounts.alice, artifactsDir, name);
    }

    /**
     * The code hash a contract build produces, from its metadata
     */
    async buildCodeHash(artifactsDir: string, name: string): Promise<string> {
        return buildCodeHash(artifactsDir, name);
    }

    /**
     * Deploy everything the way `scripts/deploy.ts` does, with test settings:
     * Alice owns the contracts, receives the fees and is the default validator
     * hotkey; Eve is the registry's guardian. `registry` picks a test registry
     * build, such as one with short delays for upgrade tests.
     */
    async deploy(
        api: TypedApi<typeof devnet>,
        accounts: TestContext['accounts'],
        registry?: RegistryBuild,
    ): Promise<Deployment> {
        const config: DeployConfig = {
            artifactsDir: RELEASE_ARTIFACTS,
            registryArtifactsDir: registry?.artifactsDir,
            owner: accounts.alice.address,
            guardian: accounts.eve.address,
            upgradeDelay: registry?.upgradeDelay ?? REGISTRY_DELAY,
            forceDelay: registry?.forceDelay ?? REGISTRY_DELAY,
            feeRate: FACTORY_FEE_RATE,
            feeRecipient: accounts.alice.address,
            defaultHotkey: accounts.alice.address,
            minListingAmount: 1_000_000_000n, // 1 Alpha
            minPurchaseAmount: 100_000_000n, // 0.1 Alpha
            minLockupDuration: 10, // 10 blocks for testing (short lockup)
            maxLockupDuration: 1_000_000, // ~46 days
            minListingAge: SPOT_LISTING_AGE,
            minOfferAmount: 1_000_000_000n, // 1 TAO
        };
        const deployment = await deployAll(api, accounts.alice, config);
        console.log(`Deployed: ${JSON.stringify(deployment)}`);
        return deployment;
    }

    /**
     * The listing factory, with its code registry. `registry` picks a test
     * registry build, such as one with short delays for upgrade tests; it always
     * gets a fresh deployment, outside the shared cache.
     */
    async createLockupListingsContext(options: { registry?: RegistryBuild } = {}): Promise<LockupListingsContext> {
        const api = await this.getApi();
        const accounts = this.createTestAccounts();
        await this.fundTestAccounts(api, accounts);
        const contractSdk = createInkSdk(api, contracts.lockup_listings);
        const registrySdk = createInkSdk(api, contracts.code_registry);

        const useCache = options.registry === undefined;
        const cachedAddress = useCache ? loadCachedAddress(LOCKUP_LISTINGS_ADDRESS_FILE) : null;
        let contractAddress: string;
        let registryAddress: string;
        let escrowCodeHash: string;
        let vaultCodeHash: string;
        if (cachedAddress && await this.contractExistsOnChain(api, cachedAddress)) {
            contractAddress = cachedAddress;
            const registry = await contractSdk.getContract(contractAddress).query("get_registry", {
                origin: accounts.alice.address,
                data: {},
            });
            if (!registry.success) {
                throw new Error("Failed to read the cached listing factory's registry");
            }
            registryAddress = registry.value.response;
            // New vaults and escrows come from the cached registry, which may
            // predate this build, so report the code it actually offers.
            const offered = async (kind: number) => {
                const current = await registrySdk.getContract(registryAddress).query("current", {
                    origin: accounts.alice.address,
                    data: { kind },
                });
                return current.success ? current.value.response?.code_hash.asHex() : undefined;
            };
            vaultCodeHash = (await offered(LISTING_VAULT_CODE_KIND))!;
            escrowCodeHash = (await offered(ESCROW_CODE_KIND))!;
        } else {
            const deployment = await this.deploy(api, accounts, options.registry);
            contractAddress = deployment.lockupListings;
            registryAddress = deployment.registry;
            escrowCodeHash = deployment.codeHashes.escrow;
            vaultCodeHash = deployment.codeHashes.listingVault;
            if (useCache) {
                saveCachedAddress(LOCKUP_LISTINGS_ADDRESS_FILE, contractAddress);
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

    /** otc, with its code registry */
    async createTestContext(): Promise<TestContext> {
        const api = await this.getApi();
        const accounts = this.createTestAccounts();
        await this.fundTestAccounts(api, accounts);
        const contractSdk = createInkSdk(api, contracts.otc_contract);

        const cachedAddress = loadCachedAddress(CONTRACT_ADDRESS_FILE);
        if (cachedAddress && await this.contractExistsOnChain(api, cachedAddress)) {
            const registry = await contractSdk.getContract(cachedAddress).query("get_registry", {
                origin: accounts.alice.address,
                data: {},
            });
            if (!registry.success) {
                throw new Error("Failed to read the cached otc contract's registry");
            }
            return { api, contractSdk, accounts, contractAddress: cachedAddress, registryAddress: registry.value.response };
        }

        const deployment = await this.deploy(api, accounts);
        saveCachedAddress(CONTRACT_ADDRESS_FILE, deployment.otc);
        return { api, contractSdk, accounts, contractAddress: deployment.otc, registryAddress: deployment.registry };
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
