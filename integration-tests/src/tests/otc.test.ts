import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { Binary } from "polkadot-api";
import { createInkSdk } from "@polkadot-api/sdk-ink";
import { contracts } from "@polkadot-api/descriptors";
import {
    setupTestEnvironment,
    cleanupTestEnvironment,
    TestSetup,
    RELEASE_ARTIFACTS,
    FACTORY_FEE_RATE,
    OTC_CODE_KIND,
    type TestContext,
} from "../setup";
import { taoToRao, waitForBlocks } from "../utils";
import { expectContractError, expectEvent, queryOk, submitOk } from "../test-helpers";

// Variant indexes of otc's Error enum.
const UNAUTHORIZED = 0;
const FEE_RATE_TOO_HIGH = 14;
const INVALID_PAUSE_DURATION = 15;
const NOT_CURRENT_CODE = 17;

// 5%, the most the owner can charge, as U64F64 bits
const MAX_FEE_RATE = 922_337_203_685_477_580n;

// papi decodes the single-field FixedDecimal struct as its bits.
const feeRateBits = (rate: any): bigint => BigInt(rate?.value ?? rate);

describe("otc v2 - configuration", () => {
    let context: TestContext;
    let otc: ReturnType<TestContext["contractSdk"]["getContract"]>;

    beforeAll(async () => {
        context = await setupTestEnvironment();
        otc = context.contractSdk.getContract(context.contractAddress!);
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    async function config() {
        return queryOk<any>(await otc.query("get_config", {
            origin: context.accounts.alice.address,
            data: {},
        }), "get_config");
    }

    it("deploys with its config and a registry offering its code", async () => {
        const { accounts } = context;
        expect(await otc.isCompatible()).toBe(true);

        const current = await config();
        expect(current.owner).toBe(accounts.alice.address);
        expect(current.registry).toBe(context.registryAddress);
        expect(current.fee_recipient).toBe(accounts.alice.address);
        expect(feeRateBits(current.fee_rate)).toBe(FACTORY_FEE_RATE);
        expect(BigInt(current.min_offer_amount)).toBe(taoToRao(1));
        expect(current.paused_until).toBe(0);

        const registry = createInkSdk(context.api, contracts.code_registry).getContract(context.registryAddress!);
        const otcCode = queryOk<any>(await registry.query("current", {
            origin: accounts.alice.address,
            data: { kind: OTC_CODE_KIND },
        }), "current").code_hash.asHex();
        expect(otcCode).toBe(await TestSetup.getInstance().buildCodeHash(RELEASE_ARTIFACTS, "otc_contract"));
    });

    it("lets only the owner change settings, and caps the fee", async () => {
        const { accounts } = context;

        expectContractError(await otc.query("set_fee_rate", {
            origin: accounts.bob.address,
            data: { new_rate: 0n },
        }), UNAUTHORIZED, "non-owner set_fee_rate");
        expectContractError(await otc.query("set_fee_rate", {
            origin: accounts.alice.address,
            data: { new_rate: MAX_FEE_RATE + 1n },
        }), FEE_RATE_TOO_HIGH, "fee above the cap");

        await submitOk(otc.send("set_fee_rate", {
            origin: accounts.alice.address,
            data: { new_rate: MAX_FEE_RATE },
        }), accounts.alice.signer, "set_fee_rate");
        await submitOk(otc.send("set_min_offer_amount", {
            origin: accounts.alice.address,
            data: { new_amount: taoToRao(2) },
        }), accounts.alice.signer, "set_min_offer_amount");
        const updated = await config();
        expect(feeRateBits(updated.fee_rate)).toBe(MAX_FEE_RATE);
        expect(BigInt(updated.min_offer_amount)).toBe(taoToRao(2));

        await submitOk(otc.send("set_fee_rate", {
            origin: accounts.alice.address,
            data: { new_rate: FACTORY_FEE_RATE },
        }), accounts.alice.signer, "restore fee rate");
        await submitOk(otc.send("set_min_offer_amount", {
            origin: accounts.alice.address,
            data: { new_amount: taoToRao(1) },
        }), accounts.alice.signer, "restore min offer");
    });

    it("moves ownership only when the new owner accepts", async () => {
        const { accounts } = context;
        const owner = async () => queryOk<string>(await otc.query("get_owner", {
            origin: accounts.alice.address,
            data: {},
        }), "get_owner");

        await submitOk(otc.send("transfer_ownership", {
            origin: accounts.alice.address,
            data: { new_owner: accounts.bob.address },
        }), accounts.alice.signer, "transfer_ownership");
        expect(await owner()).toBe(accounts.alice.address);
        expectContractError(await otc.query("accept_ownership", {
            origin: accounts.charlie.address,
            data: {},
        }), UNAUTHORIZED, "accept by someone else");

        await submitOk(otc.send("accept_ownership", {
            origin: accounts.bob.address,
            data: {},
        }), accounts.bob.signer, "accept_ownership");
        expect(await owner()).toBe(accounts.bob.address);

        // Hand it back for the tests that follow.
        await submitOk(otc.send("transfer_ownership", {
            origin: accounts.bob.address,
            data: { new_owner: accounts.alice.address },
        }), accounts.bob.signer, "transfer back");
        await submitOk(otc.send("accept_ownership", {
            origin: accounts.alice.address,
            data: {},
        }), accounts.alice.signer, "accept back");
        expect(await owner()).toBe(accounts.alice.address);
    });

    it("pauses trading for a bounded number of blocks", async () => {
        const { accounts, api } = context;
        const paused = async () => queryOk<boolean>(await otc.query("is_trading_paused", {
            origin: accounts.bob.address,
            data: {},
        }), "is_trading_paused");

        expectContractError(await otc.query("pause", {
            origin: accounts.alice.address,
            data: { blocks: 0 },
        }), INVALID_PAUSE_DURATION, "zero-length pause");
        expectContractError(await otc.query("pause", {
            origin: accounts.bob.address,
            data: { blocks: 5 },
        }), UNAUTHORIZED, "non-owner pause");

        // Long enough to outlast finalization, short enough to wait out.
        const result = await submitOk(otc.send("pause", {
            origin: accounts.alice.address,
            data: { blocks: 20 },
        }), accounts.alice.signer, "pause");
        const until = Number(expectEvent(otc, result, "TradingPaused").value.until);
        expect(await paused()).toBe(true);

        while (Number(await api.query.System.Number.getValue()) < until) {
            await waitForBlocks(api, 1);
        }
        expect(await paused()).toBe(false);

        await submitOk(otc.send("pause", {
            origin: accounts.alice.address,
            data: { blocks: 100 },
        }), accounts.alice.signer, "pause again");
        expect(await paused()).toBe(true);
        await submitOk(otc.send("resume", {
            origin: accounts.alice.address,
            data: {},
        }), accounts.alice.signer, "resume");
        expect(await paused()).toBe(false);
    });

    it("upgrades only to the registry's current otc code, and only by the owner", async () => {
        const { accounts } = context;
        const current = await TestSetup.getInstance().buildCodeHash(RELEASE_ARTIFACTS, "otc_contract");
        const other = await TestSetup.getInstance().buildCodeHash(RELEASE_ARTIFACTS, "code_registry");

        expectContractError(await otc.query("upgrade", {
            origin: accounts.bob.address,
            data: { code_hash: Binary.fromHex(current) },
        }), UNAUTHORIZED, "non-owner upgrade");
        expectContractError(await otc.query("upgrade", {
            origin: accounts.alice.address,
            data: { code_hash: Binary.fromHex(other) },
        }), NOT_CURRENT_CODE, "upgrade to other code");
        queryOk(await otc.query("upgrade", {
            origin: accounts.alice.address,
            data: { code_hash: Binary.fromHex(current) },
        }), "upgrade to the current code");
    });
});
