import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { Binary } from "polkadot-api";
import { createInkSdk } from "@polkadot-api/sdk-ink";
import { contracts } from "@polkadot-api/descriptors";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    TestSetup,
    LISTING_VAULT_CODE_KIND,
    UPGRADE_TEST_ARTIFACTS,
    type LockupListingsContext,
} from "../setup";
import { contractCodeHash } from "../deploy";
import { taoToRao, waitForBlocks, waitUntilBlock, MARKET_PRICE, type Wallet } from "../utils";
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

const LISTING_AMOUNT = taoToRao(2);
const TAKE_AMOUNT = taoToRao(1);
const SELLER_ALPHA = LISTING_AMOUNT * 10n;
// The market price can move between a quote and the take; the excess is refunded.
const PAYMENT_BUFFER = taoToRao(10);

// Variant indexes of the vault's and the registry's Error enums.
const VAULT_NOT_CURRENT_CODE = 29;
const VAULT_CONSENT_REQUIRED = 30;
const VAULT_FORCE_DELAY_NOT_ELAPSED = 31;
const REGISTRY_UNAUTHORIZED = 0;
const REGISTRY_PROPOSAL_NOT_FOUND = 4;

// The test registry's delays, in blocks.
const UPGRADE_DELAY = 5;
const FORCE_DELAY = 30;

describe("Listing vaults - upgrades through the registry", () => {
    let context: LockupListingsContext;
    let netuid: number;
    let sellerHotkey: Wallet;
    // Vault code built with a newer storage version, for the registry to adopt.
    let upgradeCodeHash: string;

    beforeAll(async () => {
        const setup = TestSetup.getInstance();
        upgradeCodeHash = await setup.uploadCode(
            await setup.getApi(),
            setup.createTestAccounts(),
            UPGRADE_TEST_ARTIFACTS,
            "listing_vault",
        );
        context = await setupLockupListingsEnvironment({
            registry: { artifactsDir: UPGRADE_TEST_ARTIFACTS, upgradeDelay: UPGRADE_DELAY, forceDelay: FORCE_DELAY },
        });
        expect(upgradeCodeHash).not.toBe(context.vaultCodeHash);
        ({ netuid, sellerHotkey } = await setupLockupSubnet(context, "vault-upgrade", SELLER_ALPHA));
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    const upgradeTo = (codeHash: string) => ({ code_hash: Binary.fromHex(codeHash) });

    function listBobs() {
        return createLockupListing(context, context.accounts.bob, {
            hotkey: sellerHotkey.address,
            netuid,
            amount: LISTING_AMOUNT,
            price_offset_bps: MARKET_PRICE,
        });
    }

    async function version(vault: any) {
        return queryOk<number>(await vault.query("get_version", {
            origin: context.accounts.alice.address,
            data: {},
        }), "get_version");
    }

    async function currentVaultCode() {
        const registry = context.registrySdk.getContract(context.registryAddress);
        return queryOk<any>(await registry.query("current", {
            origin: context.accounts.alice.address,
            data: { kind: LISTING_VAULT_CODE_KIND },
        }), "current");
    }

    it("lets the guardian veto a proposed vault code before it activates", async () => {
        const { accounts } = context;
        const registry = context.registrySdk.getContract(context.registryAddress);
        const proposal = { kind: LISTING_VAULT_CODE_KIND, ...upgradeTo(upgradeCodeHash) };

        // Only the owner proposes, and only the guardian vetoes.
        expectContractError(await registry.query("propose", {
            origin: accounts.bob.address,
            data: proposal,
        }), REGISTRY_UNAUTHORIZED, "propose as a stranger");
        await submitOk(registry.send("propose", {
            origin: accounts.alice.address,
            data: proposal,
        }), accounts.alice.signer, "propose");
        expectContractError(await registry.query("veto", {
            origin: accounts.alice.address,
            data: proposal,
        }), REGISTRY_UNAUTHORIZED, "veto as the owner");
        const vetoed = await submitOk(registry.send("veto", {
            origin: accounts.eve.address,
            data: proposal,
        }), accounts.eve.signer, "veto");
        expectEvent(registry, vetoed, "CodeVetoed");

        // A vetoed proposal never activates, and new vaults keep the released code.
        await waitForBlocks(context.api, UPGRADE_DELAY + 1);
        expectContractError(await registry.query("activate", {
            origin: accounts.dave.address,
            data: proposal,
        }), REGISTRY_PROPOSAL_NOT_FOUND, "activate a vetoed proposal");
        expect((await currentVaultCode()).code_hash.asHex()).toBe(context.vaultCodeHash);
        const { vaultAddress } = await listBobs();
        expect(await contractCodeHash(context.api, vaultAddress)).toBe(context.vaultCodeHash);
    }, 300000);

    it("upgrades vaults: the seller at once, anyone after the force delay unless ConsentOnly, and they keep trading", async () => {
        const { accounts, api } = context;
        const registry = context.registrySdk.getContract(context.registryAddress);

        // Open listings run the released code.
        const adopter = await listBobs();
        const forced = await listBobs();
        const optedOut = await listBobs();
        expect(await version(adopter.vault)).toBe(1);
        await submitOk(optedOut.vault.send("set_upgrade_policy", withIntegrationGas({
            origin: accounts.bob.address,
            data: { policy: { type: "ConsentOnly", value: undefined } },
        })), accounts.bob.signer, "set_upgrade_policy");

        // A veto doesn't block the code for good: the owner proposes it again, and
        // after the delay anyone activates it.
        await submitOk(registry.send("propose", {
            origin: accounts.alice.address,
            data: { kind: LISTING_VAULT_CODE_KIND, ...upgradeTo(upgradeCodeHash) },
        }), accounts.alice.signer, "propose");
        await waitForBlocks(api, UPGRADE_DELAY + 1);
        await submitOk(registry.send("activate", {
            origin: accounts.dave.address,
            data: { kind: LISTING_VAULT_CODE_KIND, ...upgradeTo(upgradeCodeHash) },
        }), accounts.dave.signer, "activate");
        const current = await currentVaultCode();
        expect(current.code_hash.asHex()).toBe(upgradeCodeHash);

        // New listings get the new code, at the address predicted from it.
        const fresh = await listBobs();
        expect(await contractCodeHash(api, fresh.vaultAddress)).toBe(upgradeCodeHash);
        expect(await version(fresh.vault)).toBe(2);

        // Only the current code can be installed, and others wait for the force delay.
        expectContractError(await adopter.vault.query("upgrade", {
            origin: accounts.bob.address,
            data: upgradeTo(context.vaultCodeHash),
        }), VAULT_NOT_CURRENT_CODE, "upgrade to the released code");
        expectContractError(await forced.vault.query("upgrade", {
            origin: accounts.dave.address,
            data: upgradeTo(upgradeCodeHash),
        }), VAULT_FORCE_DELAY_NOT_ELAPSED, "third-party upgrade before the force delay");

        // The seller adopts it at once. Storage migrates on the next state change,
        // here a take, which works as before.
        const upgraded = await submitOk(adopter.vault.send("upgrade", withIntegrationGas({
            origin: accounts.bob.address,
            data: upgradeTo(upgradeCodeHash),
        })), accounts.bob.signer, "seller upgrade");
        expect(expectEvent(adopter.vault, upgraded, "Upgraded").value.code_hash.asHex()).toBe(upgradeCodeHash);
        expect(await contractCodeHash(api, adopter.vaultAddress)).toBe(upgradeCodeHash);
        expect(await version(adopter.vault)).toBe(1);

        const quote = await quoteVault(adopter.vault, accounts.charlie.address, TAKE_AMOUNT);
        const { event: taken } = await takeLockupListing(adopter.vault, accounts.charlie, {
            amount: TAKE_AMOUNT,
            recipient: accounts.charlie.address,
        }, quote.total + PAYMENT_BUFFER);
        expect(BigInt(taken.value.amount)).toBe(TAKE_AMOUNT);
        expect(await version(adopter.vault)).toBe(2);
        const adopterV2 = createInkSdk(api, contracts.listing_vault_v2).getContract(adopter.vaultAddress);
        const migratedAt = queryOk<number | undefined>(await adopterV2.query("get_migrated_at", {
            origin: accounts.alice.address,
            data: {},
        }), "get_migrated_at");
        expect(migratedAt).toBeGreaterThan(Number(current.since));
        const info = queryOk<any>(await adopter.vault.query("get_info", { origin: accounts.alice.address, data: {} }), "get_info");
        expect(info.status.type).toBe("Open");
        expect(BigInt(info.remaining)).toBe(adopter.listedAmount - TAKE_AMOUNT);

        // Once the code has been current for the force delay, anyone applies it,
        // except to vaults whose seller chose ConsentOnly; that seller still can.
        await waitUntilBlock(api, Number(current.since) + FORCE_DELAY);
        await submitOk(forced.vault.send("upgrade", withIntegrationGas({
            origin: accounts.dave.address,
            data: upgradeTo(upgradeCodeHash),
        })), accounts.dave.signer, "forced upgrade");
        await submitOk(forced.vault.send("migrate", withIntegrationGas({
            origin: accounts.dave.address,
            data: {},
        })), accounts.dave.signer, "migrate after forced upgrade");
        expect(await version(forced.vault)).toBe(2);

        expectContractError(await optedOut.vault.query("upgrade", {
            origin: accounts.dave.address,
            data: upgradeTo(upgradeCodeHash),
        }), VAULT_CONSENT_REQUIRED, "forced upgrade under ConsentOnly");
        expect(await contractCodeHash(api, optedOut.vaultAddress)).toBe(context.vaultCodeHash);
        await submitOk(optedOut.vault.send("upgrade", withIntegrationGas({
            origin: accounts.bob.address,
            data: upgradeTo(upgradeCodeHash),
        })), accounts.bob.signer, "seller upgrade under ConsentOnly");
        expect(await contractCodeHash(api, optedOut.vaultAddress)).toBe(upgradeCodeHash);
    }, 900000);
});
