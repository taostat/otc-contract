import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { Binary } from "polkadot-api";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    TestSetup,
    RELEASE_ARTIFACTS,
    FACTORY_FEE_RATE,
    LISTING_VAULT_CODE_KIND,
    ESCROW_CODE_KIND,
    LOCKUP_FACTORY_CODE_KIND,
    type LockupListingsContext,
} from "../setup";
import { waitForBlocks } from "../utils";
import { expectContractError, expectEvent, queryOk, submitOk } from "../test-helpers";

// Variant indexes of the factory's Error enum.
const UNAUTHORIZED = 0;
const FEE_RATE_TOO_HIGH = 8;
const INVALID_LOCKUP_DURATION_LIMITS = 9;
const INVALID_PAUSE_DURATION = 10;
const NOT_CURRENT_CODE = 13;

// 5%, the most the owner can charge, as U64F64 bits
const MAX_FEE_RATE = 922_337_203_685_477_580n;

// papi decodes the single-field FixedDecimal struct as its bits.
const feeRateBits = (rate: any): bigint => BigInt(rate?.value ?? rate);

describe("Lockup listings v2 - factory", () => {
    let context: LockupListingsContext;
    let factory: ReturnType<LockupListingsContext["contractSdk"]["getContract"]>;

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        factory = context.contractSdk.getContract(context.contractAddress!);
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    async function config() {
        return queryOk<any>(await factory.query("get_config", {
            origin: context.accounts.alice.address,
            data: {},
        }), "get_config");
    }

    it("deploys with its config and a registry offering vault, escrow and factory code", async () => {
        const { accounts } = context;
        expect(await factory.isCompatible()).toBe(true);

        const current = await config();
        expect(current.owner).toBe(accounts.alice.address);
        expect(current.registry).toBe(context.registryAddress);
        expect(current.default_hotkey).toBe(accounts.alice.address);
        expect(current.fee_recipient).toBe(accounts.alice.address);
        expect(feeRateBits(current.fee_rate)).toBe(FACTORY_FEE_RATE);
        expect(current.paused_until).toBe(0);
        expect(queryOk<number>(await factory.query("get_version", {
            origin: accounts.alice.address,
            data: {},
        }), "get_version")).toBe(1);

        const registry = context.registrySdk.getContract(context.registryAddress);
        const currentCode = async (kind: number) => queryOk<any>(await registry.query("current", {
            origin: accounts.alice.address,
            data: { kind },
        }), "current").code_hash.asHex();
        const setup = TestSetup.getInstance();
        expect(await currentCode(LISTING_VAULT_CODE_KIND)).toBe(context.vaultCodeHash);
        expect(await currentCode(ESCROW_CODE_KIND)).toBe(context.escrowCodeHash);
        expect(await currentCode(LOCKUP_FACTORY_CODE_KIND))
            .toBe(await setup.buildCodeHash(RELEASE_ARTIFACTS, "lockup_listings"));
    });

    it("lets only the owner change settings, and caps the fee", async () => {
        const { accounts } = context;

        expectContractError(await factory.query("set_fee_rate", {
            origin: accounts.bob.address,
            data: { new_rate: 0n },
        }), UNAUTHORIZED, "non-owner set_fee_rate");
        expectContractError(await factory.query("set_fee_rate", {
            origin: accounts.alice.address,
            data: { new_rate: MAX_FEE_RATE + 1n },
        }), FEE_RATE_TOO_HIGH, "fee above the cap");
        expectContractError(await factory.query("set_lockup_duration_limits", {
            origin: accounts.alice.address,
            data: { min: 20, max: 10 },
        }), INVALID_LOCKUP_DURATION_LIMITS, "inverted lockup limits");
        expectContractError(await factory.query("transfer_ownership", {
            origin: accounts.bob.address,
            data: { new_owner: accounts.bob.address },
        }), UNAUTHORIZED, "non-owner transfer_ownership");

        await submitOk(factory.send("set_fee_rate", {
            origin: accounts.alice.address,
            data: { new_rate: MAX_FEE_RATE },
        }), accounts.alice.signer, "set_fee_rate");
        await submitOk(factory.send("set_fee_recipient", {
            origin: accounts.alice.address,
            data: { new_recipient: accounts.dave.address },
        }), accounts.alice.signer, "set_fee_recipient");
        const updated = await config();
        expect(feeRateBits(updated.fee_rate)).toBe(MAX_FEE_RATE);
        expect(queryOk<string>(await factory.query("VaultFactory::fee_recipient", {
            origin: accounts.bob.address,
            data: {},
        }), "fee_recipient")).toBe(accounts.dave.address);

        // Restore the defaults for the tests that follow.
        await submitOk(factory.send("set_fee_rate", {
            origin: accounts.alice.address,
            data: { new_rate: FACTORY_FEE_RATE },
        }), accounts.alice.signer, "restore fee rate");
        await submitOk(factory.send("set_fee_recipient", {
            origin: accounts.alice.address,
            data: { new_recipient: accounts.alice.address },
        }), accounts.alice.signer, "restore fee recipient");
    });

    it("overrides the validator hotkey per subnet", async () => {
        const { accounts } = context;
        const validatorHotkey = async (netuid: number) => queryOk<string>(await factory.query("VaultFactory::validator_hotkey", {
            origin: accounts.bob.address,
            data: { netuid },
        }), "validator_hotkey");

        expect(await validatorHotkey(7)).toBe(accounts.alice.address);
        await submitOk(factory.send("set_validator_hotkey", {
            origin: accounts.alice.address,
            data: { netuid: 7, hotkey: accounts.eve.address },
        }), accounts.alice.signer, "set_validator_hotkey");
        expect(await validatorHotkey(7)).toBe(accounts.eve.address);
        expect(await validatorHotkey(8)).toBe(accounts.alice.address);

        await submitOk(factory.send("set_validator_hotkey", {
            origin: accounts.alice.address,
            data: { netuid: 7, hotkey: undefined },
        }), accounts.alice.signer, "clear validator hotkey");
        expect(await validatorHotkey(7)).toBe(accounts.alice.address);
    });

    it("pauses trading for a bounded number of blocks", async () => {
        const { accounts, api } = context;
        const paused = async () => queryOk<boolean>(await factory.query("VaultFactory::trading_paused", {
            origin: accounts.bob.address,
            data: {},
        }), "trading_paused");

        expectContractError(await factory.query("pause", {
            origin: accounts.alice.address,
            data: { blocks: 0 },
        }), INVALID_PAUSE_DURATION, "zero-length pause");
        expectContractError(await factory.query("pause", {
            origin: accounts.bob.address,
            data: { blocks: 5 },
        }), UNAUTHORIZED, "non-owner pause");

        // Long enough to outlast finalization, short enough to wait out.
        const result = await submitOk(factory.send("pause", {
            origin: accounts.alice.address,
            data: { blocks: 20 },
        }), accounts.alice.signer, "pause");
        const until = Number(expectEvent(factory, result, "TradingPaused").value.until);
        expect(await paused()).toBe(true);

        // The pause expires on its own.
        while (Number(await api.query.System.Number.getValue()) < until) {
            await waitForBlocks(api, 1);
        }
        expect(await paused()).toBe(false);

        await submitOk(factory.send("pause", {
            origin: accounts.alice.address,
            data: { blocks: 100 },
        }), accounts.alice.signer, "pause again");
        expect(await paused()).toBe(true);
        await submitOk(factory.send("resume", {
            origin: accounts.alice.address,
            data: {},
        }), accounts.alice.signer, "resume");
        expect(await paused()).toBe(false);
    });

    it("upgrades only to the registry's current factory code, and only by the owner", async () => {
        const { accounts } = context;
        const current = await TestSetup.getInstance().buildCodeHash(RELEASE_ARTIFACTS, "lockup_listings");

        expectContractError(await factory.query("upgrade", {
            origin: accounts.bob.address,
            data: { code_hash: Binary.fromHex(current) },
        }), UNAUTHORIZED, "non-owner upgrade");
        expectContractError(await factory.query("upgrade", {
            origin: accounts.alice.address,
            data: { code_hash: Binary.fromHex(context.vaultCodeHash) },
        }), NOT_CURRENT_CODE, "upgrade to other code");
        queryOk(await factory.query("upgrade", {
            origin: accounts.alice.address,
            data: { code_hash: Binary.fromHex(current) },
        }), "upgrade to the current code");
    });
});
