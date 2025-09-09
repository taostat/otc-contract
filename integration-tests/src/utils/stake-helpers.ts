import { type TypedApi } from "polkadot-api";
import { devnet } from "@polkadot-api/descriptors";
import { type PolkadotSigner } from "polkadot-api/signer";

/**
 * Check the stake balance for a validator on a subnet
 * Uses the runtime API to get stake info
 */
export async function getStakeBalance(
    api: TypedApi<typeof devnet>,
    hotkey: string,
    netuid: number,
    coldkey: string
): Promise<bigint> {
    try {
        const stakeInfo = await api.apis.StakeInfoRuntimeApi.get_stake_info_for_hotkey_coldkey_netuid(
            hotkey,
            coldkey,
            netuid
        );

        if (stakeInfo) {
            return stakeInfo.stake;
        }

        return 0n;
    } catch (error) {
        console.error(`Error getting stake balance for ${hotkey} and coldkey: ${coldkey}) on netuid ${netuid}:`, error);
        return 0n;
    }
}

/**
 * Transfer stake from one coldkey to another via Subtensor
 * This simulates what the contract does internally
 */
export async function transferStake(
    api: TypedApi<typeof devnet>,
    destinationColdkey: string,
    hotkey: string,
    netuid: number,
    amount: bigint,
    signer: PolkadotSigner
): Promise<void> {
    const tx = api.tx.SubtensorModule.transfer_stake({
        destination_coldkey: destinationColdkey,
        hotkey,
        origin_netuid: netuid,
        destination_netuid: netuid,
        alpha_amount: amount
    });

    await tx.signAndSubmit(signer);
    console.log(`Transferred ${amount} Alpha from hotkey ${hotkey.slice(0, 10)}... to ${destinationColdkey.slice(0, 10)}...`);
}

/**
 * Move stake between hotkeys (consolidation)
 * Used when the contract needs to consolidate stake under its own hotkey
 */
export async function moveStake(
    api: TypedApi<typeof devnet>,
    originHotkey: string,
    destinationHotkey: string,
    netuid: number,
    amount: bigint,
    signer: PolkadotSigner
): Promise<void> {
    const tx = api.tx.SubtensorModule.move_stake({
        origin_hotkey: originHotkey,
        destination_hotkey: destinationHotkey,
        origin_netuid: netuid,
        destination_netuid: netuid,
        alpha_amount: amount
    });

    await tx.signAndSubmit(signer);
    console.log(`Moved ${amount} Alpha from ${originHotkey.slice(0, 10)}... to ${destinationHotkey.slice(0, 10)}...`);
}

/**
 * Check if a user has set up proxy permissions for the contract
 */
export async function hasProxyPermission(
    api: TypedApi<typeof devnet>,
    delegator: string,
    delegate: string
): Promise<boolean> {
    try {
        const proxies = await api.query.Proxy.Proxies.getValue(delegator);

        if (!proxies || !proxies[0]) {
            return false;
        }

        // Check if the delegate is in the proxy list with appropriate permissions
        return proxies[0].some(proxy => {
            return proxy.delegate === delegate &&
                (proxy.proxy_type.type === "Staking" || proxy.proxy_type.type === "Any");
        });
    } catch (error) {
        console.error(`Error checking proxy permissions:`, error);
        return false;
    }
}

/**
 * Helper to format stake amounts for display
 */
export function formatStakeAmount(amount: bigint): string {
    const alpha = Number(amount) / 1_000_000_000;
    return `${alpha.toFixed(4)} Alpha`;
}
