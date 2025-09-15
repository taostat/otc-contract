import { createClient, type PolkadotClient as Client, type TypedApi, Binary, TxEvent, TxFinalized, Enum } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider/web";
import { createInkSdk } from "@polkadot-api/sdk-ink";
import { devnet, contracts } from "@polkadot-api/descriptors";
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import { DEV_PHRASE, entropyToMiniSecret, mnemonicToEntropy, ss58Address } from "@polkadot-labs/hdkd-helpers";
import { getPolkadotSigner, type PolkadotSigner } from "polkadot-api/signer";
import * as fs from "fs/promises";
import * as fsSync from "fs";
import * as path from "path";
import { Observable } from "rxjs";

export type ContractSdk = ReturnType<typeof createInkSdk<TypedApi<typeof devnet>, typeof contracts.otc_contract>>;

// Contract address persistence file
const CONTRACT_ADDRESS_FILE = path.join(process.cwd(), ".contract-address");

// Load contract address from file if it exists
function loadContractAddress(): string | null {
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
    try {
        fsSync.writeFileSync(CONTRACT_ADDRESS_FILE, address, 'utf-8');
        console.log(`Saved contract address to file: ${address}`);
    } catch (error) {
        console.error("Failed to save contract address:", error);
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
        const contractPath = path.join(process.cwd(), "..", "target", "ink", "otc_contract.wasm");
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

// Helper functions for contract upgrade testing
export async function deployV2Contract(
    api: TypedApi<typeof devnet>,
    accounts: TestContext['accounts']
): Promise<{ codeHash: Uint8Array; wasmBlob: Uint8Array }> {
    try {
        // Read the V2 contract WASM file
        const v2WasmPath = path.join(process.cwd(), '..', 'target', 'ink', 'v2', 'otc_contract_v2.wasm');
        const wasmBlob = await fs.readFile(v2WasmPath);
        const wasmBytes = Binary.fromBytes(new Uint8Array(wasmBlob));


        console.log("Uploading V2 contract code...");

        // Upload the V2 code to the chain
        const uploadTx = api.tx.Contracts.upload_code({
            code: wasmBytes,
            determinism: Enum("Enforced"),
            storage_deposit_limit: undefined
        });

        // Submit and wait for finalization
        const result = uploadTx.signSubmitAndWatch(accounts.alice.signer);

        await new Promise<void>((resolve, reject) => {
            result.subscribe({
                next: (evt) => {
                    if (evt.type === "finalized") {
                        console.log("V2 code upload finalized");
                        resolve();
                    }
                },
                error: reject
            });
        });

        // Calculate code hash from WASM blob
        const { blake2AsU8a } = await import('@polkadot/util-crypto');
        const { u8aToHex } = await import('@polkadot/util');
        const codeHash = blake2AsU8a(wasmBlob, 256);
        const codeHashHex = u8aToHex(codeHash);

        console.log(`V2 contract code uploaded with hash: ${codeHashHex}`);

        return { codeHash, wasmBlob: new Uint8Array(wasmBlob) };
    } catch (error) {
        console.error("Failed to deploy V2 contract code:", error);
        throw error;
    }
}

export async function captureContractState(
    contract: ReturnType<ContractSdk["getContract"]>,
    accounts: TestContext['accounts']
): Promise<any> {
    const state: any = {};

    // Capture configuration
    const ownerResult = await contract.query("get_owner", {
        origin: accounts.alice.address,
        data: {}
    });
    if (ownerResult.success) {
        state.owner = ownerResult.value.response;
    }

    const hotkeyResult = await contract.query("get_hotkey", {
        origin: accounts.alice.address,
        data: {}
    });
    if (hotkeyResult.success) {
        state.hotkey = hotkeyResult.value.response;
    }

    const feeRateResult = await contract.query("get_fee_rate", {
        origin: accounts.alice.address,
        data: {}
    });
    if (feeRateResult.success) {
        state.feeRate = feeRateResult.value.response;
    }

    const minListingAmountResult = await contract.query("get_min_listing_amount", {
        origin: accounts.alice.address,
        data: {}
    });
    if (minListingAmountResult.success) {
        state.minListingAmount = minListingAmountResult.value.response;
    }

    const minOfferAmountResult = await contract.query("get_min_offer_amount", {
        origin: accounts.alice.address,
        data: {}
    });
    if (minOfferAmountResult.success) {
        state.minOfferAmount = minOfferAmountResult.value.response;
    }

    const minListingAgeResult = await contract.query("get_min_listing_age", {
        origin: accounts.alice.address,
        data: {}
    });
    if (minListingAgeResult.success) {
        state.minListingAge = minListingAgeResult.value.response;
    }

    return state;
}

export function compareContractStates(stateBefore: any, stateAfter: any): {
    isEqual: boolean;
    differences: string[];
} {
    const differences: string[] = [];

    // Compare each field
    const fields = ['owner', 'hotkey', 'feeRate', 'minListingAmount', 'minOfferAmount', 'minListingAge'];

    for (const field of fields) {
        if (stateBefore[field] !== stateAfter[field]) {
            differences.push(`${field}: ${stateBefore[field]} -> ${stateAfter[field]}`);
        }
    }

    return {
        isEqual: differences.length === 0,
        differences
    };
}
