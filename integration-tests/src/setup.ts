import { createClient, type PolkadotClient as Client, type TypedApi, Binary, TxEvent, TxFinalized } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider/web";
import { createInkSdk } from "@polkadot-api/sdk-ink";
import { devnet, contracts } from "@polkadot-api/descriptors";
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import { DEV_PHRASE, entropyToMiniSecret, mnemonicToEntropy, ss58Address } from "@polkadot-labs/hdkd-helpers";
import { getPolkadotSigner, type PolkadotSigner } from "polkadot-api/signer";
import * as fs from "fs/promises";
import * as path from "path";
import { Observable } from "rxjs";

export type ContractSdk = ReturnType<typeof createInkSdk<TypedApi<typeof devnet>, typeof contracts.otc_contract>>;

export const CONTRACT_ADDRESS = "5FBn4jtSHJPoAwVrGx1UDLsjzxBH8KFvYnPUfU1jDWEUay25";

export interface TestContext {
    client: Client;
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

    async connect(): Promise<{ client: Client; api: TypedApi<typeof devnet> }> {
        if (this.client && this.api) {
            return { client: this.client, api: this.api };
        }

        const wsUrl = process.env.CONTRACTS_NODE_URL || "ws://127.0.0.1:9944";
        console.log(`Connecting to node at ${wsUrl}...`);

        const provider = getWsProvider(wsUrl);
        this.client = createClient(provider);
        this.api = this.client.getTypedApi(devnet);

        // Wait for connection
        await new Promise((resolve) => {
            this.client!.finalizedBlock$.subscribe((block) => {
                console.log(`Connected! Current block: ${block.number}`);
                resolve(undefined);
            });
        });

        return { client: this.client, api: this.api };
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
            hotkey: accounts.alice.address, // Using alice as hotkey for simplicity
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
                if (dryRunResult.value.value?.value?.type === "DuplicateContract") {
                    console.log("Contract already exists at predicted address (using existing)")
                    // Contract already exists, just use the known address
                    // This is deterministic based on the code and salt
                    return CONTRACT_ADDRESS;
                } else {
                    console.log("Dry run did not succeed", dryRunResult.value)
                    return Promise.reject(new Error(`Dry run failed: ${JSON.stringify(dryRunResult, bigintReplacer)}`));
                }
            }

            if (dryRunResult.value.address !== CONTRACT_ADDRESS) {
                console.warn(`Warning: Predicted contract address ${dryRunResult.value.address} does not match expected ${CONTRACT_ADDRESS}`);
            }

            const fin = await this.trackTx(
                dryRunResult.value.deploy().signSubmitAndWatch(accounts.alice.signer),
            )
            console.log(`Deployed to address ${dryRunResult.value.address}`)
            console.log(contractSdk.readDeploymentEvents(fin.events))

            const contractAddress = dryRunResult.value.address;
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
        const { client, api } = await this.connect();
        const accounts = this.createTestAccounts();
        const contractSdk = createInkSdk(api, contracts.otc_contract);

        const context: TestContext = {
            client,
            api,
            contractSdk,
            accounts,
        };

        const contractAddress = await this.deployContract(api, accounts);
        context.contractAddress = contractAddress;

        return context;
    }
}

function bigintReplacer(_key: any, value: any) {
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
