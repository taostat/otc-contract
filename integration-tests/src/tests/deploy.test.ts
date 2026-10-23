import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { spawn } from "node:child_process";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { type TypedApi } from "polkadot-api";
import { createInkSdk } from "@polkadot-api/sdk-ink";
import { contracts, type devnet } from "@polkadot-api/descriptors";
import { cleanupTestEnvironment, TestSetup, RELEASE_ARTIFACTS, type TestAccount } from "../setup";
import { buildCodeHash, codeExistsOnChain, verifyDeployment, type Deployment } from "../deploy";
import { queryOk } from "../test-helpers";

const integrationRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const LOCALNET_CONFIG = "scripts/deploy.localnet.json";

/** Run the deploy script and collect its output. */
function runDeploy(args: string[], env: Record<string, string | undefined>) {
    return new Promise<{ code: number | null; stdout: string; stderr: string }>((resolve, reject) => {
        const child = spawn(path.join(integrationRoot, "node_modules", ".bin", "tsx"), ["scripts/deploy.ts", ...args], {
            cwd: integrationRoot,
            env: { ...process.env, RPC_URL: process.env.CONTRACTS_NODE_URL ?? "ws://127.0.0.1:9944", ...env },
        });
        let stdout = "";
        let stderr = "";
        child.stdout.on("data", (chunk) => (stdout += chunk));
        child.stderr.on("data", (chunk) => (stderr += chunk));
        child.on("error", reject);
        child.on("close", (code) => resolve({ code, stdout, stderr }));
    });
}

describe("Deploy script", () => {
    let api: TypedApi<typeof devnet>;
    let alice: TestAccount;
    let eve: TestAccount;

    beforeAll(async () => {
        const setup = TestSetup.getInstance();
        api = await setup.getApi();
        ({ alice, eve } = setup.createTestAccounts());
    });

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    // Runs first, on a chain without contracts or uploaded code.
    it("leaves the chain untouched when a constructor would reject the config", async () => {
        const configDir = await fs.mkdtemp(path.join(os.tmpdir(), "otc-deploy-"));
        const configFile = path.join(configDir, "config.json");
        try {
            // A fee rate above MAX_FEE_RATE (5%), which only the factory and otc check.
            const config = JSON.parse(await fs.readFile(path.join(integrationRoot, LOCALNET_CONFIG), "utf-8"));
            await fs.writeFile(configFile, JSON.stringify({ ...config, feeRate: (1n << 63n).toString() }));
            const contractsBefore = (await api.query.Contracts.ContractInfoOf.getEntries()).length;

            const run = await runDeploy([configFile], { DEPLOYER_SURI: "//Alice" });
            expect(run.code).toBe(1);
            expect(run.stderr).toContain("Instantiating lockup_listings would fail");
            expect((await api.query.Contracts.ContractInfoOf.getEntries()).length).toBe(contractsBefore);
            expect(await codeExistsOnChain(api, await buildCodeHash(RELEASE_ARTIFACTS, "listing_vault"))).toBe(false);
        } finally {
            await fs.rm(configDir, { recursive: true, force: true });
        }
    }, 120000);

    it("deploys the registry, the factory and otc from a config file, wired together", async () => {
        const outDir = await fs.mkdtemp(path.join(os.tmpdir(), "otc-deploy-"));
        const outFile = path.join(outDir, "deployment.json");
        try {
            const run = await runDeploy([LOCALNET_CONFIG, "--out", outFile], { DEPLOYER_SURI: "//Alice" });
            expect(run.code, run.stderr).toBe(0);
            const deployment: Deployment = JSON.parse(run.stdout);
            expect(JSON.parse(await fs.readFile(outFile, "utf-8"))).toEqual(deployment);

            // The registry offers the builds' code for every kind.
            expect(deployment.codeHashes).toEqual({
                listingVault: await buildCodeHash(RELEASE_ARTIFACTS, "listing_vault"),
                escrow: await buildCodeHash(RELEASE_ARTIFACTS, "alpha_lockup"),
                lockupListings: await buildCodeHash(RELEASE_ARTIFACTS, "lockup_listings"),
                otc: await buildCodeHash(RELEASE_ARTIFACTS, "otc_contract"),
            });
            await verifyDeployment(api, alice, deployment);

            const registry = createInkSdk(api, contracts.code_registry).getContract(deployment.registry);
            expect(queryOk(await registry.query("owner", { origin: alice.address, data: {} }), "owner")).toBe(alice.address);
            expect(queryOk(await registry.query("guardian", { origin: alice.address, data: {} }), "guardian")).toBe(eve.address);
            expect(queryOk(await registry.query("delays", { origin: alice.address, data: {} }), "delays")).toEqual([10, 20]);

            // Both contracts take their settings from the config, and answer to the registry.
            const factory = createInkSdk(api, contracts.lockup_listings).getContract(deployment.lockupListings);
            const factoryConfig = queryOk<any>(await factory.query("get_config", { origin: alice.address, data: {} }), "factory config");
            expect(factoryConfig).toMatchObject({
                owner: alice.address,
                registry: deployment.registry,
                default_hotkey: alice.address,
                fee_rate: 92233720368547758n,
                fee_recipient: alice.address,
                min_listing_amount: 1_000_000_000n,
                min_purchase_amount: 100_000_000n,
                min_lockup_duration: 10,
                max_lockup_duration: 1_000_000,
                min_listing_age: 30,
            });

            const otc = createInkSdk(api, contracts.otc_contract).getContract(deployment.otc);
            const otcConfig = queryOk<any>(await otc.query("get_config", { origin: alice.address, data: {} }), "otc config");
            expect(otcConfig).toMatchObject({
                owner: alice.address,
                registry: deployment.registry,
                fee_rate: 92233720368547758n,
                fee_recipient: alice.address,
                min_offer_amount: 1_000_000_000n,
            });
        } finally {
            await fs.rm(outDir, { recursive: true, force: true });
        }
    }, 300000);

    it("refuses to run without a deployer secret", async () => {
        const run = await runDeploy([LOCALNET_CONFIG], { DEPLOYER_SURI: undefined });
        expect(run.code).toBe(1);
        expect(run.stderr).toContain("usage: DEPLOYER_SURI=");
        expect(run.stdout).toBe("");
    }, 60000);
});
