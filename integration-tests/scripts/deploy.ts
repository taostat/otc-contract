/**
 * Deploy the contracts: the code registry first, offering every kind's code,
 * then the listing factory and otc.
 *
 *   DEPLOYER_SURI='<mnemonic>//<path>' npm run deploy -- <config.json> [--out deployment.json]
 *
 * `DEPLOYER_SURI` is a secret URI: a mnemonic, optionally followed by derivation
 * paths and a `///password`. A bare path such as `//Alice` uses the development
 * phrase, for localnet only. It is read from the environment so the secret never
 * sits in a file. `RPC_URL` defaults to ws://127.0.0.1:9944. Build first with
 * `npm run build:contracts`.
 *
 * The config is a JSON `DeployConfig` (see src/deploy.ts) with amounts as
 * strings; `scripts/deploy.localnet.json` is an example. The deployment, with
 * every address and code hash, is printed as JSON.
 */
import { createClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider/node";
import { devnet } from "@polkadot-api/descriptors";
import * as fs from "fs/promises";
import * as path from "path";
import { fileURLToPath } from "url";
import { deployAll, deployerFromSuri, type DeployConfig } from "../src/deploy";

const USAGE = "usage: DEPLOYER_SURI=... npm run deploy -- <config.json> [--out deployment.json]";

async function readConfig(file: string): Promise<DeployConfig> {
    const raw = JSON.parse(await fs.readFile(file, "utf-8"));
    // JSON numbers lose precision above 2^53, so amounts must be strings.
    const amount = (key: string): bigint => {
        if (typeof raw[key] !== "string") {
            throw new Error(`${key} must be a string of digits`);
        }
        return BigInt(raw[key]);
    };
    // Artifact paths in the config are relative to the repository root.
    const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
    return {
        ...raw,
        artifactsDir: path.resolve(repoRoot, raw.artifactsDir ?? "target/ink"),
        registryArtifactsDir: raw.registryArtifactsDir && path.resolve(repoRoot, raw.registryArtifactsDir),
        feeRate: amount("feeRate"),
        minListingAmount: amount("minListingAmount"),
        minPurchaseAmount: amount("minPurchaseAmount"),
        minOfferAmount: amount("minOfferAmount"),
    };
}

function parseArgs(args: string[]): { configFile: string; outFile?: string } {
    let configFile: string | undefined;
    let outFile: string | undefined;
    for (let index = 0; index < args.length; index++) {
        if (args[index] === "--out" && index + 1 < args.length) {
            outFile = args[++index];
        } else if (!args[index].startsWith("--") && configFile === undefined) {
            configFile = args[index];
        } else {
            throw new Error(USAGE);
        }
    }
    if (!configFile) {
        throw new Error(USAGE);
    }
    return { configFile, outFile };
}

async function main() {
    const { configFile, outFile } = parseArgs(process.argv.slice(2));
    const suri = process.env.DEPLOYER_SURI;
    if (!suri) {
        throw new Error(USAGE);
    }

    const config = await readConfig(configFile);
    const deployer = deployerFromSuri(suri);
    const client = createClient(getWsProvider(process.env.RPC_URL ?? "ws://127.0.0.1:9944"));
    try {
        console.error(`Deploying from ${deployer.address}...`);
        const deployment = await deployAll(client.getTypedApi(devnet), deployer, config);
        const output = JSON.stringify(deployment, null, 2);
        if (outFile) {
            await fs.writeFile(outFile, `${output}\n`, "utf-8");
        }
        console.log(output);
    } finally {
        client.destroy();
    }
}

main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
});
