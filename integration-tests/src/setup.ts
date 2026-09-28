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

// Contract address persistence files
const CONTRACT_ADDRESS_FILE = path.join(process.cwd(), ".contract-address");
const LOCKUP_LISTINGS_ADDRESS_FILE = path.join(process.cwd(), ".lockup-listings-address");
const ALPHA_LOCKUP_CODE_HASH_FILE = path.join(process.cwd(), ".alpha-lockup-code-hash");
// Deployment caching is opt-in so every run gets fresh contracts by default.
const REUSE_DEPLOYMENT_CACHE = process.env.OTC_TEST_REUSE_DEPLOYMENTS === "1";
// pallet-contracts requires the caller to hold the full storage deposit limit, and the
// test helpers use a generous limit, so every test account starts well funded.
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

// Load alpha lockup code hash from file
function loadAlphaLockupCodeHash(): string | null {
    if (!REUSE_DEPLOYMENT_CACHE) {
        return null;
    }

    try {
        if (fsSync.existsSync(ALPHA_LOCKUP_CODE_HASH_FILE)) {
            const codeHash = fsSync.readFileSync(ALPHA_LOCKUP_CODE_HASH_FILE, 'utf-8').trim();
            console.log(`Loaded alpha lockup code hash from file: ${codeHash}`);
            return codeHash;
        }
    } catch (error) {
        console.error("Failed to load alpha lockup code hash:", error);
    }
    return null;
}

// Save alpha lockup code hash to file
function saveAlphaLockupCodeHash(codeHash: string): void {
    if (!REUSE_DEPLOYMENT_CACHE) {
        return;
    }

    try {
        fsSync.writeFileSync(ALPHA_LOCKUP_CODE_HASH_FILE, codeHash, 'utf-8');
        console.log(`Saved alpha lockup code hash to file: ${codeHash}`);
    } catch (error) {
        console.error("Failed to save alpha lockup code hash:", error);
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
    contractAddress?: string;
    escrowCodeHash?: string;
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

        try {
            const dryRunResult = await deployer.dryRun("new", {
                origin: accounts.alice.address,
                data: constructorArgs,
                options: { salt: randomDeploymentSalt() },
            });

            if (!dryRunResult.success) {
                // Check if it's a DuplicateContract error
                const errorType = dryRunResult.value?.value?.value?.type;
                console.log("Dry run failed with error:", errorType);
                console.log("Full dry run result:", JSON.stringify(dryRunResult, null, 2));

                if (errorType === 'DuplicateContract') {
                    console.log("Contract already deployed, attempting to find and use it");
                    const contractAddress = loadContractAddress();
                    if (contractAddress) {
                        console.log(`Using existing contract at ${contractAddress}`);
                        return contractAddress;
                    } else {
                        return Promise.reject(new Error("Contract already deployed but address not found"));
                    }
                }

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
     * Upload alpha_lockup code and return its code hash
     */
    async uploadAlphaLockupCode(
        api: TypedApi<typeof devnet>,
        accounts: TestContext['accounts']
    ): Promise<string> {
        const existingCodeHash = loadAlphaLockupCodeHash();
        if (existingCodeHash) {
            const exists = await this.codeExistsOnChain(api, existingCodeHash);
            if (exists) {
                console.log(`Using existing alpha_lockup code hash: ${existingCodeHash}`);
                return existingCodeHash;
            }
            console.log(`Cached code hash invalid (code not on chain), re-uploading...`);
        }

        const contractPath = path.join(process.cwd(), "..", "target", "ink", "alpha_lockup", "alpha_lockup.wasm");
        const wasmFile = await fs.readFile(contractPath);
        const wasmBytes = Binary.fromBytes(new Uint8Array(wasmFile));

        console.log("Uploading alpha_lockup code...");

        const uploadTx = api.tx.Contracts.upload_code({
            code: wasmBytes,
            storage_deposit_limit: undefined,
            determinism: { type: "Enforced", value: undefined },
        });

        const fin = await this.trackTx(
            uploadTx.signSubmitAndWatch(accounts.alice.signer)
        );

        // Extract code hash from events
        let codeHash: string | null = null;
        for (const event of fin.events) {
            if (event.type === "Contracts" && event.value.type === "CodeStored") {
                codeHash = event.value.value.code_hash.asHex();
                break;
            }
        }

        if (!codeHash) {
            // Try to get the code hash from the metadata
            const alphaLockupSdk = createInkSdk(api, contracts.alpha_lockup);
            const deployer = alphaLockupSdk.getDeployer(wasmBytes);

            // The code hash is deterministic based on the WASM code
            const dryRunResult = await deployer.dryRun("new", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.alice.address,
                    netuid: 1,
                    alpha_amount: 1_000_000_000n,
                    unlock_block: 1000,
                    hotkey: accounts.bob.address,
                },
            });

            if (dryRunResult.success) {
                // The code_hash should be available from the contract metadata
                const metadata = contracts.alpha_lockup.metadata;
                codeHash = (metadata as any).source?.hash;
            }
        }

        if (!codeHash) {
            throw new Error("Failed to get alpha_lockup code hash");
        }

        console.log(`Alpha lockup code hash: ${codeHash}`);
        saveAlphaLockupCodeHash(codeHash);
        return codeHash;
    }

    /**
     * Deploy the lockup_listings contract
     */
    async deployLockupListingsContract(
        api: TypedApi<typeof devnet>,
        accounts: TestContext['accounts'],
        escrowCodeHash: string
    ): Promise<string> {
        const contractPath = path.join(process.cwd(), "..", "target", "ink", "lockup_listings", "lockup_listings.wasm");
        const wasmFile = await fs.readFile(contractPath);
        const wasmBytes = Binary.fromBytes(new Uint8Array(wasmFile));

        const contractSdk = createInkSdk(api, contracts.lockup_listings);
        const deployer = contractSdk.getDeployer(wasmBytes);

        // Convert hex code hash to proper format
        const codeHashBytes = Binary.fromHex(escrowCodeHash);

        const constructorArgs = {
            owner: accounts.alice.address,
            hotkey: accounts.alice.address, // Use alice's hotkey for testing
            escrow_code_hash: codeHashBytes,
            fee_rate: 92233720368547758n, // 0.5% as U64F64 bits
            min_listing_amount: 1_000_000_000n, // 1 Alpha
            min_purchase_amount: 100_000_000n, // 0.1 Alpha
            min_lockup_duration: 10, // 10 blocks for testing (short lockup)
            max_lockup_duration: 1_000_000, // ~46 days
        };

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

                if (errorType === 'DuplicateContract') {
                    console.log("Lockup listings contract already deployed");
                    const contractAddress = loadLockupListingsAddress();
                    if (contractAddress) {
                        console.log(`Using existing lockup listings contract at ${contractAddress}`);
                        return contractAddress;
                    } else {
                        return Promise.reject(new Error("Lockup listings contract already deployed but address not found"));
                    }
                }

                console.log("Dry run did not succeed", dryRunResult.value);
                return Promise.reject(new Error(`Dry run failed: ${errorType || 'Unknown'}`));
            }

            console.log("Deploying lockup_listings contract...");
            const fin = await this.trackTx(
                dryRunResult.value.deploy().signSubmitAndWatch(accounts.alice.signer),
            );
            console.log(`Lockup listings deployed to address ${dryRunResult.value.address}`);
            console.log(contractSdk.readDeploymentEvents(fin.events));

            const contractAddress = dryRunResult.value.address;
            saveLockupListingsAddress(contractAddress);
            return contractAddress;
        } catch (error) {
            console.error("Lockup listings deployment error:", error);
            throw error;
        }
    }

    /**
     * Create test context for lockup_listings tests
     */
    async createLockupListingsContext(): Promise<LockupListingsContext> {
        const api = await this.getApi();
        const accounts = this.createTestAccounts();
        await this.fundTestAccounts(api, accounts);

        // First upload the alpha_lockup code to get its code hash
        const escrowCodeHash = await this.uploadAlphaLockupCode(api, accounts);

        // Deploy lockup_listings contract
        const contractAddress = await this.deployLockupListingsContract(api, accounts, escrowCodeHash);

        const contractSdk = createInkSdk(api, contracts.lockup_listings);
        const alphaLockupSdk = createInkSdk(api, contracts.alpha_lockup);

        return {
            api,
            contractSdk,
            alphaLockupSdk,
            accounts,
            contractAddress,
            escrowCodeHash,
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
        for (const account of [accounts.bob, accounts.charlie, accounts.dave, accounts.eve]) {
            const setBalance = api.tx.Balances.force_set_balance({
                who: MultiAddress.Id(account.address),
                new_free: TEST_ACCOUNT_TOP_UP,
            });
            const result = await api.tx.Sudo.sudo({ call: setBalance.decodedCall }).signAndSubmit(accounts.alice.signer);
            const sudid = result.events.find((event: any) => event.type === "Sudo" && event.value.type === "Sudid");
            if (!result.ok || !sudid || sudid.value.value.success === false) {
                throw new Error(`Failed to top up test account ${account.address}`);
            }
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

export async function setupLockupListingsEnvironment(): Promise<LockupListingsContext> {
    const setup = TestSetup.getInstance();
    return await setup.createLockupListingsContext();
}
