import { Enum, type TypedApi } from "polkadot-api";
import { devnet, MultiAddress } from "@polkadot-api/descriptors";
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import { DEV_PHRASE, entropyToMiniSecret, mnemonicToEntropy, ss58Address, type KeyPair } from "@polkadot-labs/hdkd-helpers";
import { getPolkadotSigner, type PolkadotSigner } from "polkadot-api/signer";
import { randomBytes } from "crypto";

/**
 * Test configuration constants
 */
export const TEST_CONFIG = {
    wsUrl: process.env.CONTRACTS_NODE_URL || "ws://127.0.0.1:9944",
    ss58Prefix: 42,
    timeout: 60000, // 60 seconds for blockchain operations
    blockTime: 4000, // Expected block time in ms
};

function getWalletFromKeypair(keypair: KeyPair): Wallet {
    const signer = getPolkadotSigner(
        keypair.publicKey,
        "Sr25519",
        keypair.sign
    );
    const address = ss58Address(keypair.publicKey, TEST_CONFIG.ss58Prefix);

    return { address, signer };
}

function getRandomKeypair() {
    const seed = randomBytes(32);
    const miniSecret = entropyToMiniSecret(seed)
    const derive = sr25519CreateDerive(miniSecret)
    const hdkdKeyPair = derive("")

    return hdkdKeyPair
}

export type Wallet = {
    address: string;
    signer: PolkadotSigner;
};

export const getRandomWallet = (): Wallet => getWalletFromKeypair(getRandomKeypair());

/**
 * Create a hotkey for an account
 * Used for Subtensor operations
 */
export function createHotkey(coldkeyDerivePath: string): Wallet {
    const hotkeyDerivePath = `${coldkeyDerivePath}//hotkey`;
    const entropy = mnemonicToEntropy(DEV_PHRASE);
    const miniSecret = entropyToMiniSecret(entropy);
    const derive = sr25519CreateDerive(miniSecret);
    const keypair = derive(hotkeyDerivePath);

    const signer = getPolkadotSigner(
        keypair.publicKey,
        "Sr25519",
        keypair.sign
    );

    const address = ss58Address(keypair.publicKey, TEST_CONFIG.ss58Prefix);

    return { address, signer };
}

/**
 * Convert a percentage to U64F64 fixed-point representation
 * Used for fee rates in the contract
 */
export function percentageToFixedPoint(percentage: number): bigint {
    // U64F64 has 64 fractional bits
    // So 1.0 is represented as 2^64
    const one = 1n << 64n;
    const feeAsDecimal = percentage / 100;
    return BigInt(Math.floor(feeAsDecimal * Number(one)));
}

/**
 * Convert U64F64 fixed-point to a readable percentage
 */
export function fixedPointToPercentage(fixedPoint: bigint): number {
    const one = 1n << 64n;
    return (Number(fixedPoint) / Number(one)) * 100;
}

/**
 * Convert TAO amount to rao (smallest unit)
 * 1 TAO = 10^9 rao
 */
export function taoToRao(tao: number): bigint {
    return BigInt(Math.floor(tao * 1_000_000_000));
}

/**
 * Convert rao to TAO
 */
export function raoToTao(rao: bigint): number {
    return Number(rao) / 1_000_000_000;
}

/**
 * Wait for a specific number of blocks
 */
export async function waitForBlocks(api: TypedApi<typeof devnet>, blocks: number): Promise<void> {
    const startBlock = await api.query.System.Number.getValue();
    let currentBlock = startBlock;

    while (Number(currentBlock) < Number(startBlock) + blocks) {
        await new Promise(resolve => setTimeout(resolve, TEST_CONFIG.blockTime));
        currentBlock = await api.query.System.Number.getValue();
    }
}

/**
 * Fund an account with TAO from Alice (who has initial balance)
 */
export async function fundAccount(
    api: TypedApi<typeof devnet>,
    recipientAddress: string,
    amount: bigint,
    fundingSigner: PolkadotSigner
): Promise<void> {
    const tx = api.tx.Balances.transfer_keep_alive({
        dest: MultiAddress.Id(recipientAddress),
        value: amount
    });

    await tx.signAndSubmit(fundingSigner);
}

/**
 * Get the balance of an account
 */
export async function getBalance(
    api: TypedApi<typeof devnet>,
    address: string
): Promise<bigint> {
    const account = await api.query.System.Account.getValue(address);
    return account.data.free;
}

/**
 * Register a subnet for testing
 * Returns the netuid of the created subnet
 */
export async function registerSubnet(
    api: TypedApi<typeof devnet>,
    hotkey: string,
    signer: PolkadotSigner
): Promise<number> {
    const tx = api.tx.SubtensorModule.register_network({
        hotkey: hotkey
    });
    await tx.signAndSubmit(signer);

    // Get the new total networks to determine the netuid
    const afterNetworks = await api.query.SubtensorModule.TotalNetworks.getValue() || 0;
    const netuid = afterNetworks - 1; // Latest network has netuid = total - 1

    console.log(`Registered subnet with netuid: ${netuid}`);

    // Wait for network to be ready and start it
    await startSubnet(api, netuid, signer);

    return netuid;
}

/**
 * Start a subnet after registration
 */
async function startSubnet(
    api: TypedApi<typeof devnet>,
    netuid: number,
    signer: PolkadotSigner
): Promise<void> {
    console.log(`Waiting to start subnet ${netuid}...`);

    // Get the block when network was registered
    const registerBlock = await api.query.SubtensorModule.NetworkRegisteredAt.getValue(netuid);
    if (!registerBlock) {
        console.log(`Could not get registration block for netuid ${netuid}`);
        return;
    }

    // Get the required duration to wait
    const duration = await api.constants.SubtensorModule.DurationOfStartCall();
    const durationNumber = Number(duration);

    // Wait for the required duration
    let currentBlock = await api.query.System.Number.getValue();
    while (Number(currentBlock) - Number(registerBlock) <= durationNumber) {
        await new Promise((resolve) => setTimeout(resolve, 2000));
        currentBlock = await api.query.System.Number.getValue();
    }

    // Start the subnet
    const tx = api.tx.SubtensorModule.start_call({
        netuid: netuid
    });

    await tx.signAndSubmit(signer);
    console.log(`Started subnet ${netuid}`);
}

/**
 * Register a validator on a subnet and add stake
 */
export async function registerValidator(
    api: TypedApi<typeof devnet>,
    netuid: number,
    hotkey: string,
    coldkeySigner: PolkadotSigner,
    stakeAmount: bigint
): Promise<void> {
    // Register the hotkey on the subnet
    const registerTx = api.tx.SubtensorModule.burned_register({
        netuid,
        hotkey
    });

    await registerTx.signAndSubmit(coldkeySigner);
    console.log(`Registered validator with hotkey ${hotkey.slice(0, 10)}... on subnet ${netuid}`);

    // Add stake
    if (stakeAmount > 0n) {
        const stakeTx = api.tx.SubtensorModule.add_stake({
            hotkey,
            netuid,
            amount_staked: stakeAmount
        });
        await stakeTx.signAndSubmit(coldkeySigner);
        console.log(`Staked ${raoToTao(stakeAmount)} Alpha for validator on subnet ${netuid}`);
    }
}

/**
 * Elevate registration limits (target per interval & max per block) via sudo for test environments.
 */
export async function elevateRegistrationLimits(
    api: TypedApi<typeof devnet>,
    netuid: number,
    targetPerInterval: number,
    maxPerBlock: number,
    sudoSigner: PolkadotSigner
): Promise<void> {
    console.log(`Elevating registration limits on netuid ${netuid} (targetPerInterval=${targetPerInterval}, maxPerBlock=${maxPerBlock})`);
    // Set target registrations per interval
    const innerTarget = api.tx.AdminUtils.sudo_set_target_registrations_per_interval({
        netuid,
        target_registrations_per_interval: targetPerInterval,
    });
    const sudoTarget = api.tx.Sudo.sudo({ call: innerTarget.decodedCall });
    await sudoTarget.signAndSubmit(sudoSigner);

    const innerBlock = api.tx.AdminUtils.sudo_set_max_registrations_per_block({
        netuid,
        max_registrations_per_block: maxPerBlock,
    });
    const sudoBlock = api.tx.Sudo.sudo({ call: innerBlock.decodedCall });
    await sudoBlock.signAndSubmit(sudoSigner);

    const newTarget = await api.query.SubtensorModule.TargetRegistrationsPerInterval.getValue(netuid);
    const newMaxPerBlock = await api.query.SubtensorModule.MaxRegistrationsPerBlock.getValue(netuid).catch(() => undefined);
    console.log(`Updated registration params: targetPerInterval=${newTarget} maxPerBlock=${newMaxPerBlock}`);
}

/**
 * Add contract as proxy for an account
 * This is required for the contract to transfer stake on behalf of the user
 */
export async function addContractAsProxy(
    api: TypedApi<typeof devnet>,
    contractAddress: string,
    signer: PolkadotSigner
): Promise<void> {
    // Add the contract as a proxy with appropriate permissions
    const tx = api.tx.Proxy.add_proxy({
        delegate: MultiAddress.Id(contractAddress),
        proxy_type: Enum("Any"),
        delay: 0
    });

    await tx.signAndSubmit(signer);
    console.log(`Added contract ${contractAddress.slice(0, 10)}... as proxy`);
}

/**
 * Helper to format addresses for display
 */
export function formatAddress(address: string): string {
    if (address.length <= 16) return address;
    return `${address.slice(0, 8)}...${address.slice(-8)}`;
}

/**
 * Convert a price (TAO per Alpha) to U64F64 fixed-point representation
 */
export function priceToFixedPoint(pricePerAlpha: number): bigint {
    const one = 1n << 64n;
    return BigInt(Math.floor(pricePerAlpha * Number(one)));
}

/**
 * Convert U64F64 fixed-point to a readable price
 */
export function fixedPointToPrice(fixedPoint: bigint): number {
    const one = 1n << 64n;
    return Number(fixedPoint) / Number(one);
}

/**
 * Calculate total TAO required for taking an Alpha listing
 * Includes the fee
 */
export function calculateTotalTaoForListing(
    alphaAmount: bigint,
    pricePerAlpha: bigint,
    feeRate: bigint
): { taoAmount: bigint; feeAmount: bigint; totalRequired: bigint } {
    // price * amount
    const taoAmount = multiplyFixedByAmount(pricePerAlpha, alphaAmount);

    // fee_rate * tao_amount
    const feeAmount = multiplyFixedByAmount(feeRate, taoAmount);

    const totalRequired = taoAmount + feeAmount;

    return { taoAmount, feeAmount, totalRequired };
}

/**
 * Calculate Alpha amount needed for taking a TAO offer
 * Accounts for the fee that the seller pays
 */
export function calculateAlphaForOffer(
    taoAmount: bigint,
    pricePerAlpha: bigint,
    feeRate: bigint
): { alphaAmount: bigint; feeAmount: bigint; taoForSeller: bigint } {
    // fee_amount = fee_rate * tao_amount
    const feeAmount = multiplyFixedByAmount(feeRate, taoAmount);

    // tao_for_seller = tao_amount - fee_amount
    const taoForSeller = taoAmount - feeAmount;

    // alpha_amount = tao_for_seller / price
    const alphaAmount = divideByFixed(taoForSeller, pricePerAlpha);

    return { alphaAmount, feeAmount, taoForSeller };
}

/**
 * Multiply a U64F64 fixed-point number by an integer amount
 * Used for price * amount calculations
 */
function multiplyFixedByAmount(fixed: bigint, amount: bigint): bigint {
    const one = 1n << 64n;
    return (fixed * amount) / one;
}

/**
 * Divide an integer by a U64F64 fixed-point number
 * Used for amount / price calculations
 */
function divideByFixed(amount: bigint, fixed: bigint): bigint {
    const one = 1n << 64n;
    return (amount * one) / fixed;
}

/**
 * Check if a contract query resulted in an error
 */
export function hasContractError(result: any): boolean {
    // For query results
    if (!result.success && result.value?.value?.value) {
        return true;
    }
    // Legacy check
    return result?.value?.value?.type === "Err" ||
        result?.dispatchError !== undefined ||
        result?.success === false;
}

/**
 * Get the error message from a contract query result
 */
export function getContractError(result: any): string {
    // For query results with contract errors
    if (!result.success && result.value?.value?.value) {
        return result.value.value.value.type || "Unknown error";
    }
    if (result?.value?.value?.value) {
        return result.value.value.value.type || "Unknown error";
    }
    if (result?.dispatchError) {
        return JSON.stringify(result.dispatchError);
    }
    return "Contract call failed";
}
