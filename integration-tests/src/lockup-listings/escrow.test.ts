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
    LockupListingsSdk,
} from "../setup";
import {
    taoToRao,
    waitForBlocks,
    fundAccount,
    getBalance,
    registerSubnet,
    registerValidator,
    addContractAsProxy,
    createHotkey,
    elevateRegistrationLimits,
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
    submitOk,
    takeLockupListing,
    withIntegrationGas,
} from "../test-helpers";
import { getStakeBalance } from "../utils/stake-helpers";

const LISTING_AMOUNT = BITTENSOR_MIN_STAKE + 500_000n;
const REQUIRED_ALPHA = LISTING_AMOUNT * 10n;
const PURCHASE_PAYMENT_DRIFT_BUFFER = taoToRao(10);
const TRANSFER_TOLERANCE = 10n;

// Variant indexes of the escrow's Error enum (contracts/alpha_lockup/src/lib.rs).
const NO_ALPHA_TO_CLAIM = 6;
const NOT_CURRENT_CODE = 13;
const CONSENT_REQUIRED = 14;
const FORCE_DELAY_NOT_ELAPSED = 15;
const STAKE_REMAINING = 18;

describe("Alpha Lockup Escrow - sweep, resync and upgrade", () => {
    let context: LockupListingsContext;
    let contract: ReturnType<LockupListingsSdk["getContract"]>;
    let netuid: number;
    let bobHotkey: Wallet;
    // Escrow code built with a newer storage version, offered by the registry as
    // the current escrow code, while new escrows still run the released code.
    let upgradeCodeHash: string;

    beforeAll(async () => {
        const setup = TestSetup.getInstance();
        upgradeCodeHash = await setup.uploadCode(
            await setup.getApi(),
            setup.createTestAccounts(),
            UPGRADE_TEST_ARTIFACTS,
            "alpha_lockup",
        );
        context = await setupLockupListingsEnvironment({ registryEscrowCode: upgradeCodeHash });
        expect(upgradeCodeHash).not.toBe(context.escrowCodeHash);
        contract = context.contractSdk.getContract(context.contractAddress!);
        const { accounts } = context;

        await submitOk(contract.send("update_min_listing_amount", {
            origin: accounts.alice.address,
            data: { new_amount: BITTENSOR_MIN_STAKE },
        }), accounts.alice.signer, "update_min_listing_amount");
        await submitOk(contract.send("update_min_purchase_amount", {
            origin: accounts.alice.address,
            data: { new_amount: BITTENSOR_MIN_STAKE },
        }), accounts.alice.signer, "update_min_purchase_amount");

        const subnetOwnerHotkey = createHotkey("//Alice/escrow");
        await fundAccount(context.api, subnetOwnerHotkey.address, taoToRao(10), accounts.alice.signer);
        netuid = await registerSubnet(context.api, subnetOwnerHotkey.address, accounts.alice.signer);
        await elevateRegistrationLimits(context.api, netuid, 20, 20, accounts.alice.signer);
        await waitForBlocks(context.api, 1);

        // The lockup contract's hotkey is alice; escrows hold stake on it.
        await fundAccount(context.api, context.contractAddress!, taoToRao(10), accounts.alice.signer);
        await registerValidator(context.api, netuid, accounts.alice.address, accounts.alice.signer, taoToRao(100));

        bobHotkey = createHotkey("//Bob/escrow");
        await fundAccount(context.api, bobHotkey.address, taoToRao(1), accounts.alice.signer);
        await registerValidator(context.api, netuid, bobHotkey.address, accounts.bob.signer, taoToRao(5000), REQUIRED_ALPHA);
        await waitForBlocks(context.api, 2);
        await addContractAsProxy(context.api, context.contractAddress!, accounts.bob.signer);
        await waitForBlocks(context.api, 2);

        // Fund the buyer and the third parties that sweep, upgrade and claim.
        await fundAccount(context.api, accounts.charlie.address, taoToRao(200), accounts.alice.signer);
        await fundAccount(context.api, accounts.dave.address, taoToRao(100), accounts.alice.signer);
        await fundAccount(context.api, accounts.eve.address, taoToRao(20), accounts.alice.signer);
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    async function purchaseEscrow(buyer: TestAccount, lockupDuration: number) {
        const { listingId, listedAmount } = await createLockupListing(contract, context.accounts.bob.signer, context.accounts.bob.address, {
            hotkey: bobHotkey.address,
            netuid,
            amount: LISTING_AMOUNT,
            price_offset_bps: MARKET_PRICE,
            lockup_duration: lockupDuration,
        });
        const [, , totalRequired] = queryOk<[bigint, bigint, bigint]>(await contract.query("estimate_lockup_price", {
            origin: context.accounts.alice.address,
            data: { netuid, seller: context.accounts.bob.address, listing_id: listingId, amount: listedAmount },
        }), "estimate_lockup_price");
        const { escrowAccount } = await takeLockupListing(contract, buyer.signer, buyer.address, {
            netuid,
            seller: context.accounts.bob.address,
            listing_id: listingId,
            amount: listedAmount,
        }, totalRequired + PURCHASE_PAYMENT_DRIFT_BUFFER);
        return { escrow: context.alphaLockupSdk.getContract(escrowAccount), escrowAccount };
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
        expect(swept.value.beneficiary).toBe(accounts.charlie.address);
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

    it("refuses a third-party upgrade before the force delay, and under ConsentOnly", async () => {
        const { accounts } = context;
        const { escrow } = await purchaseEscrow(accounts.charlie, LONG_LOCKUP_DURATION);
        const upgradeArgs = { code_hash: Binary.fromHex(upgradeCodeHash) };

        expectContractError(await escrow.query("upgrade", {
            origin: accounts.dave.address,
            data: upgradeArgs,
        }), FORCE_DELAY_NOT_ELAPSED, "third-party upgrade");

        await submitOk(escrow.send("set_upgrade_policy", {
            origin: accounts.charlie.address,
            data: { policy: { type: "ConsentOnly", value: undefined } },
        }), accounts.charlie.signer, "set_upgrade_policy");

        expectContractError(await escrow.query("upgrade", {
            origin: accounts.dave.address,
            data: upgradeArgs,
        }), CONSENT_REQUIRED, "third-party upgrade under ConsentOnly");
    }, 600000);

    it("lets the beneficiary upgrade to the registry's current code, and the escrow keeps working", async () => {
        const { accounts } = context;
        const { escrow } = await purchaseEscrow(accounts.charlie, SHORT_LOCKUP_DURATION);

        expect(queryOk<number>(await escrow.query("get_version", {
            origin: accounts.alice.address,
            data: {},
        }), "get_version")).toBe(1);

        // The escrow answers to the lockup's registry, which offers the upgrade code.
        expect(queryOk<string>(await escrow.query("get_registry", {
            origin: accounts.alice.address,
            data: {},
        }), "get_registry")).toBe(context.registryAddress);
        const registry = context.registrySdk.getContract(context.registryAddress);
        const current = queryOk<any>(await registry.query("current", {
            origin: accounts.alice.address,
            data: { kind: ESCROW_CODE_KIND },
        }), "current");
        expect(current.code_hash.asHex()).toBe(upgradeCodeHash);

        // Only the registry's current code can be installed.
        expectContractError(await escrow.query("upgrade", {
            origin: accounts.charlie.address,
            data: { code_hash: Binary.fromHex(context.escrowCodeHash!) },
        }), NOT_CURRENT_CODE, "upgrade to the released code");

        const upgradeResult = await submitOk(escrow.send("upgrade", withIntegrationGas({
            origin: accounts.charlie.address,
            data: { code_hash: Binary.fromHex(upgradeCodeHash) },
        })), accounts.charlie.signer, "beneficiary upgrade");
        const upgraded = expectEvent(escrow, upgradeResult, "Upgraded");
        expect(upgraded.value.code_hash.asHex()).toBe(upgradeCodeHash);

        // The new code migrates storage to its version on the next call.
        await submitOk(escrow.send("migrate", withIntegrationGas({
            origin: accounts.dave.address,
            data: {},
        })), accounts.dave.signer, "migrate");
        expect(queryOk<number>(await escrow.query("get_version", {
            origin: accounts.alice.address,
            data: {},
        }), "get_version after upgrade")).toBe(2);

        // The upgraded escrow still pays out to the beneficiary.
        await waitForUnlock(escrow);
        const charlieStakeBefore = await getStakeBalance(context.api, accounts.alice.address, netuid, accounts.charlie.address);
        const claimResult = await submitOk(escrow.send("claim", withIntegrationGas({
            origin: accounts.dave.address,
            data: {},
        })), accounts.dave.signer, "claim after upgrade");
        const claimed = expectEvent(escrow, claimResult, "AlphaClaimed");
        expect(claimed.value.buyer).toBe(accounts.charlie.address);
        const charlieStakeAfter = await getStakeBalance(context.api, accounts.alice.address, netuid, accounts.charlie.address);
        expect(charlieStakeAfter - charlieStakeBefore).toBeGreaterThanOrEqual(BigInt(claimed.value.amount_claimed) - TRANSFER_TOLERANCE);
    }, 600000);

    // Runs last: the swap moves every stake on alice's hotkey in this subnet,
    // including the lockup contract's own.
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
