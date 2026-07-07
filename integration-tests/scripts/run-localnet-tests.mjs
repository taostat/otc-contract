#!/usr/bin/env node

import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { createClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider/web";
import { devnet } from "@polkadot-api/descriptors";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const integrationRoot = path.resolve(__dirname, "..");
const repoRoot = path.resolve(integrationRoot, "..");
const defaultSubtensorDir = path.resolve(repoRoot, "..", "subtensor-fork");
const wsUrl = process.env.CONTRACTS_NODE_URL ?? "ws://127.0.0.1:9944";
const rpcHost = process.env.OTC_TEST_RPC_HOST ?? "127.0.0.1";
const rpcPort = Number(process.env.OTC_TEST_RPC_PORT ?? "9944");
const readinessTimeoutMs = Number(process.env.OTC_TEST_RPC_TIMEOUT_MS ?? "240000");
const shutdownTimeoutMs = Number(process.env.OTC_TEST_SHUTDOWN_TIMEOUT_MS ?? "30000");
const verboseLocalnet = process.env.OTC_TEST_VERBOSE_LOCALNET === "1";
const localnetLogTailBytes = Number(process.env.OTC_TEST_LOCALNET_LOG_TAIL_BYTES ?? "24000");

const DEFAULT_TEST_FILES = [
  "src/lockup-listings/admin.test.ts",
  "src/lockup-listings/cancel-listing.test.ts",
  "src/lockup-listings/claim.test.ts",
  "src/lockup-listings/create-listing.test.ts",
  "src/lockup-listings/deployment.test.ts",
  "src/lockup-listings/pause.test.ts",
  "src/lockup-listings/take-listing.test.ts",
  "src/tests/alpha-listing.test.ts",
  "src/tests/claim-dividends.test.ts",
  "src/tests/deployment.test.ts",
  "src/tests/pause.test.ts",
  "src/tests/subnet-risk.test.ts",
  "src/tests/tao-offers.test.ts",
  "src/tests/trading.test.ts",
];

function parseArgs(argv) {
  const options = {
    repeat: 1,
    restart: "file",
    reuseDeployments: false,
    buildOnly: false,
    files: [],
    subtensorDir: process.env.SUBTENSOR_FORK_DIR ?? defaultSubtensorDir,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--repeat") {
      options.repeat = Number(argv[++index] ?? "1");
    } else if (arg.startsWith("--repeat=")) {
      options.repeat = Number(arg.slice("--repeat=".length));
    } else if (arg === "--restart") {
      options.restart = argv[++index] ?? "file";
    } else if (arg.startsWith("--restart=")) {
      options.restart = arg.slice("--restart=".length);
    } else if (arg === "--reuse-deployments") {
      options.reuseDeployments = true;
    } else if (arg === "--build-only") {
      options.buildOnly = true;
    } else if (arg === "--subtensor-dir") {
      options.subtensorDir = path.resolve(argv[++index] ?? defaultSubtensorDir);
    } else if (arg.startsWith("--subtensor-dir=")) {
      options.subtensorDir = path.resolve(arg.slice("--subtensor-dir=".length));
    } else if (arg === "--help" || arg === "-h") {
      printHelp();
      process.exit(0);
    } else {
      options.files.push(arg);
    }
  }

  if (!Number.isInteger(options.repeat) || options.repeat < 1) {
    throw new Error("--repeat must be a positive integer");
  }

  if (!["file", "all"].includes(options.restart)) {
    throw new Error("--restart must be either 'file' or 'all'");
  }

  if (options.files.length === 0) {
    options.files = DEFAULT_TEST_FILES;
  }

  return options;
}

function printHelp() {
  console.log(`Usage: npm run test:localnet -- [options] [test files...]

Options:
  --repeat <n>             Repeat the selected files n times.
  --restart file|all       Restart localnet per file (default) or once per repeat.
  --reuse-deployments      Opt in to integration deployment cache reuse.
  --subtensor-dir <path>   Path to subtensor-fork. Defaults to ../subtensor-fork.
  --build-only             Run localnet.sh --build-only before tests.

Examples:
  npm run test:localnet -- src/tests/trading.test.ts --repeat 3
  npm run test:localnet -- --restart all src/tests/deployment.test.ts src/tests/pause.test.ts
`);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function createOutputTail() {
  let output = "";

  return {
    add(prefix, chunk, stream) {
      const text = `${prefix}${chunk.toString()}`;
      if (verboseLocalnet) {
        stream.write(text);
      }
      output += text;
      if (output.length > localnetLogTailBytes) {
        output = output.slice(output.length - localnetLogTailBytes);
      }
    },
    dump() {
      return output.trim();
    },
  };
}

async function clearDeploymentCache() {
  const entries = await fs.readdir(integrationRoot);
  const cacheFiles = entries.filter((entry) =>
    entry === ".contract-address" ||
    entry.startsWith(".lockup-") ||
    entry.startsWith(".alpha-")
  );

  for (const entry of cacheFiles) {
    const filePath = path.join(integrationRoot, entry);
    await fs.rm(filePath, { force: true });
    console.log(`[localnet-runner] removed cache ${entry}`);
  }
}

async function isPortOpen() {
  return new Promise((resolve) => {
    const socket = net.createConnection({ host: rpcHost, port: rpcPort });
    socket.setTimeout(1000);
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("timeout", () => {
      socket.destroy();
      resolve(false);
    });
    socket.once("error", () => resolve(false));
  });
}

async function waitForPortClosed() {
  const startedAt = Date.now();
  while (Date.now() - startedAt < shutdownTimeoutMs) {
    if (!(await isPortOpen())) {
      return;
    }
    await sleep(500);
  }
  throw new Error(`RPC port ${rpcPort} remained open after shutdown`);
}

async function queryCurrentBlock() {
  const provider = getWsProvider(wsUrl);
  const client = createClient(provider);
  try {
    const api = client.getTypedApi(devnet);
    return Number(await api.query.System.Number.getValue());
  } finally {
    client.destroy();
  }
}

async function waitForRpcReady() {
  const startedAt = Date.now();
  let lastError;

  while (Date.now() - startedAt < readinessTimeoutMs) {
    try {
      const blockNumber = await queryCurrentBlock();
      console.log(`[localnet-runner] RPC ready at block ${blockNumber}`);
      return;
    } catch (error) {
      lastError = error;
      await sleep(2000);
    }
  }

  throw new Error(`RPC did not become ready within ${readinessTimeoutMs}ms: ${lastError}`);
}

async function runCommand(command, args, options = {}) {
  await new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: integrationRoot,
      env: {
        ...process.env,
        CONTRACTS_NODE_URL: wsUrl,
        OTC_TEST_REUSE_DEPLOYMENTS: options.reuseDeployments ? "1" : "0",
      },
      stdio: "inherit",
    });

    child.on("error", reject);
    child.on("exit", (code, signal) => {
      if (code === 0) {
        resolve();
      } else {
        reject(new Error(`${command} ${args.join(" ")} exited with code=${code} signal=${signal}`));
      }
    });
  });
}

async function startLocalnet(subtensorDir) {
  const scriptPath = path.join(subtensorDir, "scripts", "localnet.sh");
  await fs.access(scriptPath);

  if (await isPortOpen()) {
    throw new Error(
      `RPC port ${rpcPort} is already open before localnet startup. Stop the existing node before running deterministic integration tests.`
    );
  }

  console.log(`[localnet-runner] starting localnet from ${scriptPath}`);
  const child = spawn(scriptPath, [], {
    cwd: subtensorDir,
    detached: true,
    env: process.env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const outputTail = createOutputTail();

  child.stdout.on("data", (chunk) => outputTail.add("[localnet] ", chunk, process.stdout));
  child.stderr.on("data", (chunk) => outputTail.add("[localnet] ", chunk, process.stderr));

  child.on("exit", (code, signal) => {
    if (code !== null || signal !== null) {
      console.log(`[localnet-runner] localnet exited code=${code} signal=${signal}`);
      if (!verboseLocalnet && code !== 0 && code !== 143) {
        const tail = outputTail.dump();
        if (tail.length > 0) {
          console.error(`[localnet-runner] localnet output tail:\n${tail}`);
        }
      }
    }
  });

  try {
    await waitForRpcReady();
  } catch (error) {
    if (!verboseLocalnet) {
      const tail = outputTail.dump();
      if (tail.length > 0) {
        console.error(`[localnet-runner] localnet output tail:\n${tail}`);
      }
    }
    throw error;
  }
  return child;
}

async function stopLocalnet(child) {
  if (!child || child.killed) {
    await waitForPortClosed();
    return;
  }

  console.log("[localnet-runner] stopping localnet");
  try {
    process.kill(-child.pid, "SIGTERM");
  } catch (error) {
    if (error.code !== "ESRCH") {
      throw error;
    }
  }

  const startedAt = Date.now();
  while (Date.now() - startedAt < shutdownTimeoutMs) {
    if (!(await isPortOpen())) {
      return;
    }
    await sleep(500);
  }

  console.log("[localnet-runner] SIGTERM timed out, sending SIGKILL");
  try {
    process.kill(-child.pid, "SIGKILL");
  } catch (error) {
    if (error.code !== "ESRCH") {
      throw error;
    }
  }
  await waitForPortClosed();
}

async function runLocalnetTestBatch(files, options) {
  if (!options.reuseDeployments) {
    await clearDeploymentCache();
  }

  let child;
  try {
    child = await startLocalnet(options.subtensorDir);
    await runCommand("npx", [
      "vitest",
      "run",
      "--no-file-parallelism",
      "--maxWorkers=1",
      ...files,
    ], options);
  } finally {
    await stopLocalnet(child);
  }
}

async function main() {
  const options = parseArgs(process.argv.slice(2));

  if (options.buildOnly) {
    await runCommand(path.join(options.subtensorDir, "scripts", "localnet.sh"), ["--build-only"], options);
  }

  for (let repeat = 1; repeat <= options.repeat; repeat += 1) {
    console.log(`[localnet-runner] repeat ${repeat}/${options.repeat}`);
    if (options.restart === "all") {
      await runLocalnetTestBatch(options.files, options);
      continue;
    }

    for (const file of options.files) {
      console.log(`[localnet-runner] running ${file}`);
      await runLocalnetTestBatch([file], options);
    }
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
