import { createClient, type PolkadotClient as Client, type TypedApi, Binary, TxEvent, TxFinalized, FixedSizeBinary } from "polkadot-api";
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
