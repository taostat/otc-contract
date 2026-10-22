import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { Enum } from "polkadot-api";
import { MultiAddress } from "@polkadot-api/descriptors";
import {
    setupTestEnvironment,
    cleanupTestEnvironment,
    type TestAccount,
    type TestContext,
} from "../setup";
import {
    createHotkey,
    elevateRegistrationLimits,
    fundAccount,
    getBalance,
    registerSubnet,
    registerValidator,
    taoToRao,
    waitForBlocks,
    MARKET_PRICE,
    type Wallet,
} from "../utils";
import {
    expectContractError,
    expectEvent,
    queryOk,
    submitOk,
    submitReverted,
    withIntegrationGas,
} from "../test-helpers";
import { getStakeBalance, hasProxyPermission } from "../utils/stake-helpers";

const OFFER_AMOUNT = taoToRao(10);
const TRANSFER_TOLERANCE = 10n;

// Variant indexes of otc's Error enum.
const TRADING_PAUSED = 1;
const AMOUNT_TOO_SMALL = 2;
const INVALID_PRICE_OFFSET = 3;
const OFFER_NOT_FOUND = 5;
const PRICE_ABOVE_MAXIMUM = 8;
const ALPHA_ABOVE_MAXIMUM = 9;
const RUNTIME_CALL_FAILED = 12;

describe("otc v2 - TAO offers", () => {
    let context: TestContext;
    let otc: ReturnType<TestContext["contractSdk"]["getContract"]>;
    let netuid: number;
    let sellerHotkey: Wallet;
    let alice: TestAccount;
    let bob: TestAccount;
    let charlie: TestAccount;
    let dave: TestAccount;

    beforeAll(async () => {
        context = await setupTestEnvironment();
        otc = context.contractSdk.getContract(context.contractAddress!);
        ({ alice, bob, charlie, dave } = context.accounts);

        const subnetOwner = createHotkey("//Alice/offers");
        await fundAccount(context.api, subnetOwner.address, taoToRao(10), alice.signer);
        netuid = await registerSubnet(context.api, subnetOwner.address, alice.signer);
        await elevateRegistrationLimits(context.api, netuid, 20, 20, alice.signer);
        await waitForBlocks(context.api, 1);

        // Charlie sells Alpha staked on his own validator hotkey.
        sellerHotkey = createHotkey("//Charlie/offers");
        await fundAccount(context.api, sellerHotkey.address, taoToRao(1), alice.signer);
        await registerValidator(context.api, netuid, sellerHotkey.address, charlie.signer, taoToRao(1000));
        await waitForBlocks(context.api, 2);
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    async function createOffer(buyer: TestAccount, maxPrice = 0n) {
        const result = await submitOk(otc.send("create_tao_offer", withIntegrationGas({
            origin: buyer.address,
            data: { netuid, price_offset_bps: MARKET_PRICE, max_price: maxPrice },
            value: OFFER_AMOUNT,
        })), buyer.signer, "create_tao_offer");
        return BigInt(expectEvent(otc, result, "TaoOfferCreated").value.tao_offer_id);
    }

    async function quoteFill(buyer: TestAccount, offerId: bigint) {
        return queryOk<{ price: bigint; alpha: bigint; tao: bigint; fee: bigint }>(await otc.query("quote_offer_fill", {
            origin: charlie.address,
            data: { netuid, buyer: buyer.address, offer_id: offerId },
        }), "quote_offer_fill");
    }

    /** The take, as the seller would send it, with `max_alpha` as their cap */
    function takeOffer(buyer: TestAccount, offerId: bigint, maxAlpha: bigint) {
        return otc.send("take_tao_offer", withIntegrationGas({
            origin: charlie.address,
            data: { netuid, buyer: buyer.address, offer_id: offerId, hotkey: sellerHotkey.address, max_alpha: maxAlpha },
        }));
    }

    function proxyCall(name: "add_proxy" | "remove_proxy") {
        return context.api.tx.Proxy[name]({
            delegate: MultiAddress.Id(context.contractAddress!),
            proxy_type: Enum("Transfer"),
            delay: 0,
        });
    }

    it("creates offers within the limits and lists them", async () => {
        const offerId = await createOffer(bob, 1_000_000_000_000n);

        const offer = queryOk<any>(await otc.query("get_offer", {
            origin: alice.address,
            data: { netuid, buyer: bob.address, offer_id: offerId },
        }), "get_offer");
        expect(BigInt(offer.amount)).toBe(OFFER_AMOUNT);
        expect(BigInt(offer.max_price)).toBe(1_000_000_000_000n);
        expect(queryOk<bigint[]>(await otc.query("get_user_offers", {
            origin: alice.address,
            data: { buyer: bob.address, netuid },
        }), "get_user_offers")).toContain(offerId);

        expectContractError(await otc.query("create_tao_offer", {
            origin: bob.address,
            data: { netuid, price_offset_bps: MARKET_PRICE, max_price: 0n },
            value: taoToRao(1) - 1n,
        }), AMOUNT_TOO_SMALL, "offer below the minimum");
        expectContractError(await otc.query("create_tao_offer", {
            origin: bob.address,
            data: { netuid, price_offset_bps: -10_000, max_price: 0n },
            value: OFFER_AMOUNT,
        }), INVALID_PRICE_OFFSET, "offer at a zero price");
    }, 300000);

    it("fills an offer with a proxy that exists only inside the seller's batch", async () => {
        const offerId = await createOffer(bob);
        const quote = await quoteFill(bob, offerId);
        const bobStakeBefore = await getStakeBalance(context.api, sellerHotkey.address, netuid, bob.address);
        const aliceBalanceBefore = await getBalance(context.api, alice.address);

        // Charlie never added the proxy before, and it is gone afterwards.
        expect(await hasProxyPermission(context.api, charlie.address, context.contractAddress!)).toBe(false);
        const batch = context.api.tx.Utility.batch_all({
            calls: [
                proxyCall("add_proxy").decodedCall,
                await takeOffer(bob, offerId, quote.alpha * 101n / 100n).decodedCall,
                proxyCall("remove_proxy").decodedCall,
            ],
        });
        const result = await submitOk(batch, charlie.signer, "add proxy, take, remove proxy");
        expect(await hasProxyPermission(context.api, charlie.address, context.contractAddress!)).toBe(false);

        const taken = expectEvent(otc, result, "TaoOfferTaken");
        expect(taken.value.seller).toBe(charlie.address);
        expect(taken.value.buyer).toBe(bob.address);
        const alpha = BigInt(taken.value.alpha_amount);
        expect(BigInt(taken.value.tao_amount) + BigInt(taken.value.fee)).toBe(OFFER_AMOUNT);

        // The buyer gets the Alpha; the fee recipient gets the fee.
        expect(await getStakeBalance(context.api, sellerHotkey.address, netuid, bob.address) - bobStakeBefore)
            .toBeGreaterThanOrEqual(alpha - TRANSFER_TOLERANCE);
        expect(await getBalance(context.api, alice.address) - aliceBalanceBefore).toBe(BigInt(taken.value.fee));
        expect(queryOk<any>(await otc.query("get_offer", {
            origin: alice.address,
            data: { netuid, buyer: bob.address, offer_id: offerId },
        }), "get_offer")).toBeUndefined();
    }, 300000);

    it("refuses a fill without the seller's proxy", async () => {
        const offerId = await createOffer(bob);
        const quote = await quoteFill(bob, offerId);

        expectContractError(await otc.query("take_tao_offer", {
            origin: charlie.address,
            data: { netuid, buyer: bob.address, offer_id: offerId, hotkey: sellerHotkey.address, max_alpha: quote.alpha * 2n },
        }), RUNTIME_CALL_FAILED, "take without a proxy");
        await submitReverted(takeOffer(bob, offerId, quote.alpha * 2n), charlie.signer, "take without a proxy");
    }, 300000);

    it("guards the price for both sides", async () => {
        // The buyer's ceiling sits below the market.
        const cappedId = await createOffer(dave, 1n);
        expectContractError(await otc.query("take_tao_offer", {
            origin: charlie.address,
            data: { netuid, buyer: dave.address, offer_id: cappedId, hotkey: sellerHotkey.address, max_alpha: taoToRao(1_000_000) },
        }), PRICE_ABOVE_MAXIMUM, "fill above the buyer's ceiling");

        // The seller's cap sits below what the fill needs.
        const offerId = await createOffer(dave);
        const quote = await quoteFill(dave, offerId);
        expectContractError(await otc.query("take_tao_offer", {
            origin: charlie.address,
            data: { netuid, buyer: dave.address, offer_id: offerId, hotkey: sellerHotkey.address, max_alpha: quote.alpha / 2n },
        }), ALPHA_ABOVE_MAXIMUM, "fill above the seller's cap");
    }, 300000);

    it("blocks new offers and fills while paused, but never cancels", async () => {
        const offerId = await createOffer(bob);
        await submitOk(otc.send("pause", { origin: alice.address, data: { blocks: 100 } }), alice.signer, "pause");

        expectContractError(await otc.query("create_tao_offer", {
            origin: bob.address,
            data: { netuid, price_offset_bps: MARKET_PRICE, max_price: 0n },
            value: OFFER_AMOUNT,
        }), TRADING_PAUSED, "offer while paused");
        expectContractError(await otc.query("take_tao_offer", {
            origin: charlie.address,
            data: { netuid, buyer: bob.address, offer_id: offerId, hotkey: sellerHotkey.address, max_alpha: taoToRao(1_000_000) },
        }), TRADING_PAUSED, "fill while paused");

        // Only the buyer can cancel, and paused or not, the TAO comes back.
        expectContractError(await otc.query("cancel_tao_offer", {
            origin: dave.address,
            data: { netuid, offer_id: offerId },
        }), OFFER_NOT_FOUND, "cancel by someone else");
        const result = await submitOk(otc.send("cancel_tao_offer", {
            origin: bob.address,
            data: { netuid, offer_id: offerId },
        }), bob.signer, "cancel while paused");
        expect(BigInt(expectEvent(otc, result, "TaoOfferCancelled").value.amount_returned)).toBe(OFFER_AMOUNT);

        await submitOk(otc.send("resume", { origin: alice.address, data: {} }), alice.signer, "resume");
    }, 300000);
});
