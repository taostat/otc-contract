import { Enum, type TypedApi } from "polkadot-api";
import { devnet, MultiAddress } from "@polkadot-api/descriptors";
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import { entropyToMiniSecret, ss58Address, type KeyPair } from "@polkadot-labs/hdkd-helpers";
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

const TARGETED_STAKE_TOP_UP = 50_000_000_000_000n; // 50,000 TAO
const MAX_TARGETED_STAKE_ATTEMPTS = 20;

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
export function createHotkey(): Wallet {
    return getRandomWallet();
}

/**
 * Convert a percentage to U64F64 fixed-point representation
 * Used for fee rates in the contract
 */
export function percentageToFixedPoint(percentage: number): bigint {
    // Convert percentage to decimal (e.g., 0.5% -> 0.005)
    const decimalStr = (percentage / 100).toString();
    const [integerPart, decimalPart = ''] = decimalStr.split('.');

    // Combine integer and decimal parts as a single number
    const numerator = BigInt(integerPart + decimalPart);
    const denominator = 10n ** BigInt(decimalPart.length || 0);

    // U64F64 has 64 fractional bits (2^64)
    const one = 1n << 64n;

    // Calculate the fixed-point representation
    return (numerator * one) / denominator;
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
    const duration = await api.query.SubtensorModule.StartCallDelay.getValue();
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
    stakeAmount: bigint,
    minimumAlphaAmount: bigint = 0n
): Promise<void> {
    // Register the hotkey on the subnet
    const registerTx = api.tx.SubtensorModule.burned_register({
        netuid,
        hotkey
    });

    await registerTx.signAndSubmit(coldkeySigner);
    console.log(`Registered validator with hotkey ${hotkey.slice(0, 10)}... on subnet ${netuid}`);

    // Add stake. Tests should pass a large enough TAO amount for the Alpha they
    // intend to move because dynamic subnet prices vary across localnet runs.
    const addStake = async (amount: bigint): Promise<void> => {
        const stakeTx = api.tx.SubtensorModule.add_stake({
            hotkey,
            netuid,
            amount_staked: amount
        });
        await stakeTx.signAndSubmit(coldkeySigner);
    };

    let currentStake = 0n;
    const coldkey = ss58Address(coldkeySigner.publicKey, TEST_CONFIG.ss58Prefix);

    if (stakeAmount > 0n) {
        await addStake(stakeAmount);
        currentStake = await getStakeForHotkeyColdkey(api, hotkey, coldkey, netuid);
        console.log(`Validator has ${raoToTao(currentStake)} Alpha on subnet ${netuid}`);
    }

    let attempts = 0;
    while (minimumAlphaAmount > 0n && currentStake < minimumAlphaAmount && attempts < MAX_TARGETED_STAKE_ATTEMPTS) {
        attempts += 1;
        await addStake(TARGETED_STAKE_TOP_UP);
        currentStake = await getStakeForHotkeyColdkey(api, hotkey, coldkey, netuid);
        console.log(
            `Validator top-up ${attempts}/${MAX_TARGETED_STAKE_ATTEMPTS}: ${raoToTao(currentStake)} Alpha on subnet ${netuid}`
        );
    }

    if (minimumAlphaAmount > 0n && currentStake < minimumAlphaAmount) {
        throw new Error(
            `Validator ${hotkey} only reached ${raoToTao(currentStake)} Alpha on subnet ${netuid}; required ${raoToTao(minimumAlphaAmount)}`
        );
    }
}

async function getStakeForHotkeyColdkey(
    api: TypedApi<typeof devnet>,
    hotkey: string,
    coldkey: string,
    netuid: number
): Promise<bigint> {
    const stakeInfo = await api.apis.StakeInfoRuntimeApi.get_stake_info_for_hotkey_coldkey_netuid(
        hotkey,
        coldkey,
        netuid
    );

    return stakeInfo?.stake ?? 0n;
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

    // Disable freeze window to allow immediate changes
    const setFreezeWindow = api.tx.AdminUtils.sudo_set_admin_freeze_window({ window: 0 });
    await api.tx.Sudo.sudo({ call: setFreezeWindow.decodedCall }).signAndSubmit(sudoSigner);

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
        proxy_type: Enum("Transfer"),
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
    // Convert to string to handle decimals precisely
    const priceStr = pricePerAlpha.toString();
    const [integerPart, decimalPart = ''] = priceStr.split('.');

    // Combine integer and decimal parts as a single number
    const numerator = BigInt(integerPart + decimalPart);
    const denominator = 10n ** BigInt(decimalPart.length || 0);

    // U64F64 has 64 fractional bits (2^64)
    const one = 1n << 64n;

    // Calculate the fixed-point representation
    return (numerator * one) / denominator;
}

// ============================================================================
// Dynamic Pricing Helpers (Basis Points)
// ============================================================================

/**
 * Common price offset constants for tests
 * Basis points: 1 bp = 0.01%, 100 bp = 1%, 10000 bp = 100%
 */
export const MARKET_PRICE = 0;           // 0% offset (exactly market price)
export const ABOVE_MARKET_5 = 500;       // +5% above market
export const ABOVE_MARKET_10 = 1000;     // +10% above market
export const BELOW_MARKET_5 = -500;      // -5% below market
export const BELOW_MARKET_10 = -1000;    // -10% below market
export const INVALID_OFFSET = -10000;    // -100% (invalid, would make price 0)

/**
 * Convert a percentage to basis points
 * e.g., 5 -> 500, -5 -> -500, 0.5 -> 50
 */
export function percentageToBps(percentage: number): number {
    return Math.round(percentage * 100);
}

/**
 * Convert basis points to percentage
 * e.g., 500 -> 5, -500 -> -5
 */
export function bpsToPercentage(bps: number): number {
    return bps / 100;
}

/**
 * Fetch current market price for a subnet from the chain
 * Returns price as TAO_per_Alpha * 1e9 (scaled for precision)
 */
export async function getCurrentAlphaPrice(
    api: TypedApi<typeof devnet>,
    netuid: number
): Promise<bigint> {
    const price = await api.apis.SwapRuntimeApi.current_alpha_price(netuid);
    return price;
}

/**
 * Apply a basis points offset to a market price
 * Formula: executed_price = market_price * (10000 + offset_bps) / 10000
 */
export function applyPriceOffset(marketPrice: bigint, offsetBps: number): bigint {
    const base = 10000n;
    const multiplier = base + BigInt(offsetBps);
    if (multiplier <= 0n) {
        throw new Error(`Invalid offset: ${offsetBps} would result in zero or negative price`);
    }
    return (marketPrice * multiplier) / base;
}

/**
 * Convert a scaled price (price * 1e9) to U64F64 fixed-point representation
 * This is the format used internally by the contract for calculations
 */
export function scaledPriceToFixedPoint(scaledPrice: bigint): bigint {
    // scaledPrice is TAO_per_Alpha * 1e9
    // We need to convert to U64F64 (multiply by 2^64, divide by 1e9)
    const one = 1n << 64n;
    const divisor = 1_000_000_000n;
    return (scaledPrice * one) / divisor;
}

/**
 * Get the executed price for a listing/offer given the market price and offset
 * Returns the price in U64F64 format ready for calculations
 */
export async function getExecutedPriceFixed(
    api: TypedApi<typeof devnet>,
    netuid: number,
    priceOffsetBps: number
): Promise<bigint> {
    const marketPrice = await getCurrentAlphaPrice(api, netuid);
    const executedPrice = applyPriceOffset(marketPrice, priceOffsetBps);
    return scaledPriceToFixedPoint(executedPrice);
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

// ============================================================================
// Lockup Listings Helpers
// ============================================================================

/**
 * Get the current block number
 */
export async function getCurrentBlock(api: TypedApi<typeof devnet>): Promise<number> {
    const blockNumber = await api.query.System.Number.getValue();
    return Number(blockNumber);
}

/**
 * Wait until a specific block is reached
 */
export async function waitUntilBlock(
    api: TypedApi<typeof devnet>,
    targetBlock: number
): Promise<void> {
    let currentBlock = await getCurrentBlock(api);

    while (currentBlock < targetBlock) {
        console.log(`Waiting for block ${targetBlock}, current: ${currentBlock}`);
        await new Promise(resolve => setTimeout(resolve, TEST_CONFIG.blockTime));
        currentBlock = await getCurrentBlock(api);
    }

    console.log(`Reached target block ${targetBlock}`);
}

/**
 * Bittensor minimum stake constant (2_000_000 rao = 0.002 TAO)
 */
export const BITTENSOR_MIN_STAKE = 2_000_000n;

/**
 * Default lockup duration constants for tests (in blocks)
 */
export const SHORT_LOCKUP_DURATION = 10;
export const MEDIUM_LOCKUP_DURATION = 50;
export const LONG_LOCKUP_DURATION = 100;

/**
 * Calculate TAO required for taking a lockup listing
 * Similar to calculateTotalTaoForListing but uses the dynamic pricing model
 */
export function calculateLockupListingTao(
    alphaAmount: bigint,
    executedPrice: bigint,
    feeRate: bigint
): { taoAmount: bigint; feeAmount: bigint; totalRequired: bigint } {
    // price is scaled by 1e9, need to apply properly
    const priceDecimal = scaledPriceToFixedPoint(executedPrice);
    const taoAmount = multiplyFixedByAmount(priceDecimal, alphaAmount);
    const feeAmount = multiplyFixedByAmount(feeRate, taoAmount);
    const totalRequired = taoAmount + feeAmount;

    return { taoAmount, feeAmount, totalRequired };
}
