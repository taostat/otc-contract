import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { Binary } from "polkadot-api";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    TestSetup,
    ESCROW_CODE_KIND,
    UPGRADE_TEST_ARTIFACTS,
    type LockupListingsContext,
    type TestAccount,
} from "../setup";
import {
    taoToRao,
    waitForBlocks,
    fundAccount,
    getBalance,
    createHotkey,
    MARKET_PRICE,
    SHORT_LOCKUP_DURATION,
    LONG_LOCKUP_DURATION,
    waitUntilBlock,
    BITTENSOR_MIN_STAKE,
    type Wallet,
} from "../utils";
import {
    createLockupListing,
    expectContractError,
    expectEvent,
    queryOk,
    quoteVault,
    setupLockupSubnet,
    submitOk,
    takeLockupListing,
    withIntegrationGas,
} from "../test-helpers";
import { getStakeBalance } from "../utils/stake-helpers";

const LISTING_AMOUNT = BITTENSOR_MIN_STAKE + 500_000n;
const SELLER_ALPHA = LISTING_AMOUNT * 10n;
const PURCHASE_PAYMENT_DRIFT_BUFFER = taoToRao(10);
const TRANSFER_TOLERANCE = 10n;

// Variant indexes of the escrow's Error enum (contracts/alpha_lockup/src/lib.rs).
const NO_ALPHA_TO_CLAIM = 6;
const NOT_CURRENT_CODE = 13;
const CONSENT_REQUIRED = 14;
const FORCE_DELAY_NOT_ELAPSED = 15;
const STAKE_REMAINING = 18;

// The test registry's delays, in blocks.
const UPGRADE_DELAY = 5;
const FORCE_DELAY = 30;

describe("Alpha Lockup Escrow - sweep, resync and upgrade", () => {
    let context: LockupListingsContext;
    let netuid: number;
    let bobHotkey: Wallet;
    // Escrow code built with a newer storage version, for the registry to adopt.
    let upgradeCodeHash: string;

    beforeAll(async () => {
        const setup = TestSetup.getInstance();
        upgradeCodeHash = await setup.uploadCode(
            await setup.getApi(),
            setup.createTestAccounts(),
            UPGRADE_TEST_ARTIFACTS,
            "alpha_lockup",
        );
        context = await setupLockupListingsEnvironment({
            registry: { artifactsDir: UPGRADE_TEST_ARTIFACTS, upgradeDelay: UPGRADE_DELAY, forceDelay: FORCE_DELAY },
        });
        expect(upgradeCodeHash).not.toBe(context.escrowCodeHash);
        ({ netuid, sellerHotkey: bobHotkey } = await setupLockupSubnet(context, "escrow", SELLER_ALPHA));
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    /**
     * Bob lists, and `buyer` takes the whole listing into an escrow. Vaults create
     * escrows from the registry's current escrow code.
     */
    async function purchaseEscrow(buyer: TestAccount, lockupDuration: number) {
        const { vault, listedAmount } = await createLockupListing(context, context.accounts.bob, {
            hotkey: bobHotkey.address,
            netuid,
            amount: LISTING_AMOUNT,
            price_offset_bps: MARKET_PRICE,
            lockup_duration: lockupDuration,
        });
        const quote = await quoteVault(vault, buyer.address, listedAmount);
        const { escrowAccount } = await takeLockupListing(vault, buyer, {
            amount: listedAmount,
            recipient: buyer.address,
        }, quote.total + PURCHASE_PAYMENT_DRIFT_BUFFER);
        return { escrow: context.alphaLockupSdk.getContract(escrowAccount!), escrowAccount: escrowAccount! };
    }

    async function waitForUnlock(escrow: Awaited<ReturnType<typeof purchaseEscrow>>["escrow"]) {
        const unlockBlock = queryOk<number>(await escrow.query("get_unlock_block", {
            origin: context.accounts.alice.address,
            data: {},
        }), "get_unlock_block");
        await waitUntilBlock(context.api, unlockBlock);
    }

    it("lets anyone sweep TAO sent to an escrow to its beneficiary, without closing it", async () => {
        const { accounts } = context;
        const { escrow, escrowAccount } = await purchaseEscrow(accounts.charlie, LONG_LOCKUP_DURATION);

        // TAO reaches an escrow when its subnet is liquidated or someone sends it.
        await fundAccount(context.api, escrowAccount, taoToRao(3), accounts.dave.signer);
        const charlieBefore = await getBalance(context.api, accounts.charlie.address);

        const result = await submitOk(escrow.send("sweep_tao", withIntegrationGas({
            origin: accounts.eve.address,
            data: {},
        })), accounts.eve.signer, "sweep_tao");
        const swept = expectEvent(escrow, result, "TaoSwept");
        expect(swept.value.owner).toBe(accounts.charlie.address);
        expect(swept.value.initiated_by).toBe(accounts.eve.address);
        const sweptAmount = BigInt(swept.value.amount);
        expect(sweptAmount).toBeGreaterThanOrEqual(taoToRao(3));

        const charlieAfter = await getBalance(context.api, accounts.charlie.address);
        expect(charlieAfter - charlieBefore).toBe(sweptAmount);

        // The escrow is still there, still locked and unclaimed.
        const info = queryOk<any>(await escrow.query("get_info", {
            origin: accounts.alice.address,
            data: {},
        }), "get_info");
        expect(info.claimed).toBe(false);
        expect(BigInt(info.tao_balance)).toBe(0n);
    }, 600000);

    it("upgrades escrows through the registry: the beneficiary at once, anyone after the force delay, unless ConsentOnly", async () => {
        const { accounts, api } = context;
        const registry = context.registrySdk.getContract(context.registryAddress);
        const version = async (escrow: any) => queryOk<number>(await escrow.query("get_version", {
            origin: accounts.alice.address,
            data: {},
        }), "get_version");
        const upgradeTo = (codeHash: string) => ({ code_hash: Binary.fromHex(codeHash) });

        // Escrows bought now run the released code, from the factory's registry.
        const adopter = await purchaseEscrow(accounts.charlie, SHORT_LOCKUP_DURATION);
        const forced = await purchaseEscrow(accounts.charlie, LONG_LOCKUP_DURATION);
        const optedOut = await purchaseEscrow(accounts.charlie, LONG_LOCKUP_DURATION);
        expect(await version(adopter.escrow)).toBe(1);
        expect(queryOk<string>(await adopter.escrow.query("get_registry", {
            origin: accounts.alice.address,
            data: {},
        }), "get_registry")).toBe(context.registryAddress);
        await submitOk(optedOut.escrow.send("set_upgrade_policy", {
            origin: accounts.charlie.address,
            data: { policy: { type: "ConsentOnly", value: undefined } },
        }), accounts.charlie.signer, "set_upgrade_policy");

        // The registry owner proposes the new code; after the delay anyone activates it.
        await submitOk(registry.send("propose", {
            origin: accounts.alice.address,
            data: { kind: ESCROW_CODE_KIND, ...upgradeTo(upgradeCodeHash) },
        }), accounts.alice.signer, "propose");
        await waitForBlocks(api, UPGRADE_DELAY + 1);
        await submitOk(registry.send("activate", {
            origin: accounts.dave.address,
            data: { kind: ESCROW_CODE_KIND, ...upgradeTo(upgradeCodeHash) },
        }), accounts.dave.signer, "activate");
        const current = queryOk<any>(await registry.query("current", {
            origin: accounts.alice.address,
            data: { kind: ESCROW_CODE_KIND },
        }), "current");
        expect(current.code_hash.asHex()).toBe(upgradeCodeHash);

        // Only the current code can be installed, and others wait for the force delay.
        expectContractError(await adopter.escrow.query("upgrade", {
            origin: accounts.charlie.address,
            data: upgradeTo(context.escrowCodeHash!),
        }), NOT_CURRENT_CODE, "upgrade to the released code");
        expectContractError(await forced.escrow.query("upgrade", {
            origin: accounts.dave.address,
            data: upgradeTo(upgradeCodeHash),
        }), FORCE_DELAY_NOT_ELAPSED, "third-party upgrade before the force delay");

        // The beneficiary adopts it at once; the new code migrates storage.
        const upgradeResult = await submitOk(adopter.escrow.send("upgrade", withIntegrationGas({
            origin: accounts.charlie.address,
            data: upgradeTo(upgradeCodeHash),
        })), accounts.charlie.signer, "beneficiary upgrade");
        expect(expectEvent(adopter.escrow, upgradeResult, "Upgraded").value.code_hash.asHex()).toBe(upgradeCodeHash);
        await submitOk(adopter.escrow.send("migrate", withIntegrationGas({
            origin: accounts.dave.address,
            data: {},
        })), accounts.dave.signer, "migrate");
        expect(await version(adopter.escrow)).toBe(2);

        // The upgraded escrow still pays out to the beneficiary.
        await waitForUnlock(adopter.escrow);
        const charlieStakeBefore = await getStakeBalance(api, accounts.alice.address, netuid, accounts.charlie.address);
        const claimResult = await submitOk(adopter.escrow.send("claim", withIntegrationGas({
            origin: accounts.dave.address,
            data: {},
        })), accounts.dave.signer, "claim after upgrade");
        const claimed = expectEvent(adopter.escrow, claimResult, "AlphaClaimed");
        expect(claimed.value.buyer).toBe(accounts.charlie.address);
        const charlieStakeAfter = await getStakeBalance(api, accounts.alice.address, netuid, accounts.charlie.address);
        expect(charlieStakeAfter - charlieStakeBefore).toBeGreaterThanOrEqual(BigInt(claimed.value.amount_claimed) - TRANSFER_TOLERANCE);

        // Once the code has been current for the force delay, anyone applies it,
        // except to escrows whose beneficiary chose ConsentOnly.
        await waitUntilBlock(api, Number(current.since) + FORCE_DELAY);
        await submitOk(forced.escrow.send("upgrade", withIntegrationGas({
            origin: accounts.dave.address,
            data: upgradeTo(upgradeCodeHash),
        })), accounts.dave.signer, "forced upgrade");
        await submitOk(forced.escrow.send("migrate", withIntegrationGas({
            origin: accounts.dave.address,
            data: {},
        })), accounts.dave.signer, "migrate after forced upgrade");
        expect(await version(forced.escrow)).toBe(2);
        expectContractError(await optedOut.escrow.query("upgrade", {
            origin: accounts.dave.address,
            data: upgradeTo(upgradeCodeHash),
        }), CONSENT_REQUIRED, "forced upgrade under ConsentOnly");
        expect(await version(optedOut.escrow)).toBe(1);
    }, 900000);

    // Runs last: the swap moves every stake on alice's hotkey in this subnet,
    // including the open listings' vaults.
    it("lets anyone resync an escrow after a validator hotkey swap, and claim follows", async () => {
        const { accounts } = context;
        const { escrow, escrowAccount } = await purchaseEscrow(accounts.charlie, SHORT_LOCKUP_DURATION);

        const newHotkey = createHotkey("//Alice/escrow/swapped");
        await submitOk(context.api.tx.SubtensorModule.swap_hotkey({
            hotkey: accounts.alice.address,
            new_hotkey: newHotkey.address,
            netuid,
        }), accounts.alice.signer, "swap_hotkey");

        const escrowStake = await getStakeBalance(context.api, newHotkey.address, netuid, escrowAccount);
        expect(escrowStake).toBeGreaterThan(0n);

        await waitForUnlock(escrow);
        expectContractError(await escrow.query("claim", {
            origin: accounts.dave.address,
            data: {},
        }), NO_ALPHA_TO_CLAIM, "claim on the old hotkey");

        const resyncResult = await submitOk(escrow.send("resync_hotkey", withIntegrationGas({
            origin: accounts.dave.address,
            data: { new_hotkey: newHotkey.address },
        })), accounts.dave.signer, "resync_hotkey");
        const resynced = expectEvent(escrow, resyncResult, "HotkeyResynced");
        expect(resynced.value.old_hotkey).toBe(accounts.alice.address);
        expect(resynced.value.new_hotkey).toBe(newHotkey.address);

        // The beneficiary cannot close the escrow while it holds stake.
        expectContractError(await escrow.query("close", {
            origin: accounts.charlie.address,
            data: {},
        }), STAKE_REMAINING, "close before claiming");

        const charlieStakeBefore = await getStakeBalance(context.api, newHotkey.address, netuid, accounts.charlie.address);
        const claimResult = await submitOk(escrow.send("claim", withIntegrationGas({
            origin: accounts.dave.address,
            data: {},
        })), accounts.dave.signer, "claim after resync");
        const claimed = expectEvent(escrow, claimResult, "AlphaClaimed");
        expect(claimed.value.buyer).toBe(accounts.charlie.address);
        const charlieStakeAfter = await getStakeBalance(context.api, newHotkey.address, netuid, accounts.charlie.address);
        expect(charlieStakeAfter - charlieStakeBefore).toBeGreaterThanOrEqual(BigInt(claimed.value.amount_claimed) - TRANSFER_TOLERANCE);

        // Now empty, the escrow can be closed by its beneficiary.
        const closeResult = await submitOk(escrow.send("close", withIntegrationGas({
            origin: accounts.charlie.address,
            data: {},
        })), accounts.charlie.signer, "close");
        expect(expectEvent(escrow, closeResult, "EscrowClosed").value.beneficiary).toBe(accounts.charlie.address);
        expect(await TestSetup.getInstance().contractExistsOnChain(context.api, escrowAccount)).toBe(false);
    }, 600000);
});
