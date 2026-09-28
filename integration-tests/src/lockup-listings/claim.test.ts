import { describe, it, expect, beforeAll, afterAll } from "vitest";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    type LockupListingsContext,
    type TestAccount,
    LockupListingsSdk,
} from "../setup";
import {
    taoToRao,
    waitForBlocks,
    fundAccount,
    registerSubnet,
    registerValidator,
    addContractAsProxy,
    createHotkey,
    elevateRegistrationLimits,
    MARKET_PRICE,
    SHORT_LOCKUP_DURATION,
    getCurrentBlock,
    waitUntilBlock,
    BITTENSOR_MIN_STAKE,
    type Wallet,
} from "../utils";
import {
    createLockupListing,
    queryOk,
    submitOk,
    submitReverted,
    takeLockupListing,
    withIntegrationGas,
} from "../test-helpers";
import {
    getStakeBalance,
    formatStakeAmount,
} from "../utils/stake-helpers";

const VALID_LISTING_AMOUNT = BITTENSOR_MIN_STAKE + 500_000n;
const REQUIRED_ALPHA = VALID_LISTING_AMOUNT * 5n;
const SMALL_ACCOUNTING_TOLERANCE = 1_000n;
// Each stake transfer can round down by up to this many rao.
const TRANSFER_TOLERANCE = 10n;
// Claim tests care about escrow settlement, not exact purchase pricing. The
// market price can move between the estimate query and the finalized purchase;
// the contract refunds any overpayment.
const CLAIM_PURCHASE_PAYMENT_DRIFT_BUFFER = taoToRao(10);
// A finalized claim can include emissions accrued between our pre-claim stake
// snapshot and the block that executes the escrow transfer.
const CLAIM_FINALIZATION_EMISSION_TOLERANCE = 200_000n;

describe("Lockup Listings Contract - Claim", () => {
    let context: LockupListingsContext;
    let contract: ReturnType<LockupListingsSdk["getContract"]>;
    let netuid: number;

    let bobHotkey: Wallet;

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);

        await submitOk(contract.send("update_min_listing_amount", {
            origin: context.accounts.alice.address,
            data: { new_amount: BITTENSOR_MIN_STAKE }
        }), context.accounts.alice.signer, "update_min_listing_amount");

        await submitOk(contract.send("update_min_purchase_amount", {
            origin: context.accounts.alice.address,
            data: { new_amount: BITTENSOR_MIN_STAKE }
        }), context.accounts.alice.signer, "update_min_purchase_amount");

        // Create a subnet for testing
        const aliceHotkey = createHotkey("//Alice/claim");
        await fundAccount(context.api, aliceHotkey.address, taoToRao(10), context.accounts.alice.signer);
        netuid = await registerSubnet(context.api, aliceHotkey.address, context.accounts.alice.signer);
        console.log(`Created test subnet with netuid: ${netuid}`);

        // Elevate registration limits
        await elevateRegistrationLimits(
            context.api,
            netuid,
            20,
            20,
            context.accounts.alice.signer
        );
        await waitForBlocks(context.api, 1);

        // Register the contract's hotkey
        const contractHotkey = queryOk<string>(await contract.query("get_hotkey", {
            origin: context.accounts.alice.address,
            data: {}
        }), "get_hotkey");
        await fundAccount(context.api, context.contractAddress!, taoToRao(10), context.accounts.alice.signer);
        await registerValidator(
            context.api,
            netuid,
            contractHotkey,
            context.accounts.alice.signer,
            taoToRao(100)
        );

        // Create and register Bob's hotkey
        bobHotkey = createHotkey("//Bob/claim");
        await fundAccount(context.api, bobHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await registerValidator(context.api, netuid, bobHotkey.address, context.accounts.bob.signer, taoToRao(5000), REQUIRED_ALPHA);

        await waitForBlocks(context.api, 2);

        // Add contract as proxy for Bob
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.bob.signer);

        await waitForBlocks(context.api, 2);

        // Fund the buyer and the third parties that trigger claims
        await fundAccount(context.api, context.accounts.charlie.address, taoToRao(200), context.accounts.alice.signer);
        await fundAccount(context.api, context.accounts.dave.address, taoToRao(100), context.accounts.alice.signer);
        await fundAccount(context.api, context.accounts.eve.address, taoToRao(20), context.accounts.alice.signer);

        console.log("Test setup complete");
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    async function createBobListing(amount: bigint, lockupDuration: number): Promise<{ listingId: bigint; listedAmount: bigint }> {
        const { listingId, listedAmount } = await createLockupListing(contract, context.accounts.bob.signer, context.accounts.bob.address, {
            hotkey: bobHotkey.address,
            netuid,
            amount,
            price_offset_bps: MARKET_PRICE,
            lockup_duration: lockupDuration,
        });
        expect(listedAmount).toBeLessThanOrEqual(amount);
        expect(amount - listedAmount).toBeLessThanOrEqual(TRANSFER_TOLERANCE * 2n);
        return { listingId, listedAmount };
    }

    async function estimateListing(listingId: bigint, amount: bigint): Promise<[bigint, bigint, bigint]> {
        return queryOk<[bigint, bigint, bigint]>(await contract.query("estimate_lockup_price", {
            origin: context.accounts.alice.address,
            data: {
                netuid,
                seller: context.accounts.bob.address,
                listing_id: listingId,
                amount,
            },
        }), "estimate_lockup_price");
    }

    async function createPurchasedEscrow(
        buyer: TestAccount,
        amount: bigint,
        lockupDuration: number,
    ) {
        const { listingId, listedAmount } = await createBobListing(amount, lockupDuration);
        const [, , totalRequired] = await estimateListing(listingId, listedAmount);
        return takeLockupListing(contract, buyer.signer, buyer.address, {
            netuid,
            seller: context.accounts.bob.address,
            listing_id: listingId,
            amount: listedAmount,
        }, totalRequired + CLAIM_PURCHASE_PAYMENT_DRIFT_BUFFER);
    }

    async function escrowStakeAfterUnlock(escrowAddress: string): Promise<bigint> {
        const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);
        const unlockBlock = queryOk<number>(await escrowContract.query("get_unlock_block", {
            origin: context.accounts.alice.address,
            data: {}
        }), "get_unlock_block");
        await waitUntilBlock(context.api, unlockBlock);

        const escrowHotkey = queryOk<string>(await escrowContract.query("get_hotkey", {
            origin: context.accounts.alice.address,
            data: {}
        }), "get_hotkey");
        return getStakeBalance(context.api, escrowHotkey, netuid, escrowAddress);
    }

    describe("Full Lifecycle", () => {
        it("should complete full lifecycle: create -> take -> third-party claim", async () => {
            const { accounts } = context;
            const listAmount = VALID_LISTING_AMOUNT;

            const buyerStakeBefore = await getStakeBalance(
                context.api,
                context.accounts.alice.address, // contract's hotkey
                netuid,
                accounts.charlie.address
            );
            console.log(`Charlie's stake before: ${formatStakeAmount(buyerStakeBefore)}`);
            console.log(`Current block: ${await getCurrentBlock(context.api)}`);

            const { listingId, listedAmount } = await createBobListing(listAmount, SHORT_LOCKUP_DURATION);
            const [, , totalRequired] = await estimateListing(listingId, listedAmount);

            const { event: takenEvent, purchaseId, escrowAccount } = await takeLockupListing(contract, accounts.charlie.signer, accounts.charlie.address, {
                netuid,
                seller: accounts.bob.address,
                listing_id: listingId,
                amount: listedAmount
            }, totalRequired + CLAIM_PURCHASE_PAYMENT_DRIFT_BUFFER);
            expect(takenEvent.value.seller).toBe(accounts.bob.address);
            expect(takenEvent.value.buyer).toBe(accounts.charlie.address);

            // The escrow lookup matches the purchase id emitted by the contract.
            const escrowLookup = queryOk<string | undefined>(await contract.query("get_escrow", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    listing_id: listingId,
                    purchase_id: purchaseId
                }
            }), "get_escrow");
            expect(escrowLookup).toBe(escrowAccount);

            const escrowContract = context.alphaLockupSdk.getContract(escrowAccount);
            const info = queryOk<any>(await escrowContract.query("get_info", {
                origin: accounts.charlie.address,
                data: {}
            }), "get_info");
            expect(info.buyer).toBe(accounts.charlie.address);
            expect(info.pending_beneficiary).toBeUndefined();
            expect(info.claimed).toBe(false);

            const escrowStakeBeforeClaim = await escrowStakeAfterUnlock(escrowAccount);
            expect(escrowStakeBeforeClaim).toBeGreaterThanOrEqual(listedAmount - TRANSFER_TOLERANCE);

            // Dave, who is not the buyer, triggers the claim; the Alpha still goes to Charlie.
            const claimResult = await submitOk(escrowContract.send("claim", withIntegrationGas({
                origin: accounts.dave.address,
                data: {}
            })), accounts.dave.signer, "claim");

            const claimedEvent = escrowContract.filterEvents(claimResult.events)
                .find(e => e.type === "AlphaClaimed");
            expect(claimedEvent).toBeDefined();
            expect(claimedEvent!.value.buyer).toBe(accounts.charlie.address);
            expect(claimedEvent!.value.initiated_by).toBe(accounts.dave.address);
            expect(claimedEvent!.value.netuid).toBe(netuid);
            const claimedAmount = BigInt(claimedEvent!.value.amount_claimed);
            expect(claimedAmount).toBeGreaterThanOrEqual(escrowStakeBeforeClaim);
            expect(claimedAmount - escrowStakeBeforeClaim).toBeLessThanOrEqual(CLAIM_FINALIZATION_EMISSION_TOLERANCE);

            const buyerStakeAfter = await getStakeBalance(
                context.api,
                context.accounts.alice.address, // contract's hotkey
                netuid,
                accounts.charlie.address
            );
            console.log(`Charlie's stake after claim: ${formatStakeAmount(buyerStakeAfter)}`);
            expect(buyerStakeAfter - buyerStakeBefore).toBeGreaterThanOrEqual(claimedAmount - SMALL_ACCOUNTING_TOLERANCE);
        }, 600000);
    });

    describe("Claim Error Cases", () => {
        let escrowAddress: string;

        beforeAll(async () => {
            const purchase = await createPurchasedEscrow(context.accounts.charlie, VALID_LISTING_AMOUNT, 100);
            escrowAddress = purchase.escrowAccount;
        }, 300000);

        it("should fail to claim before unlock block", async () => {
            const { accounts } = context;
            const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);

            const blocksRemaining = queryOk<number>(await escrowContract.query("blocks_until_unlock", {
                origin: accounts.charlie.address,
                data: {}
            }), "blocks_until_unlock");
            console.log(`Blocks until unlock: ${blocksRemaining}`);

            await submitReverted(escrowContract.send("claim", {
                origin: accounts.charlie.address,
                data: {}
            }), accounts.charlie.signer, "claim before unlock");
        }, 60000);

        it("should fail before unlock even when triggered by a third party", async () => {
            const { accounts } = context;
            const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);

            await submitReverted(escrowContract.send("claim", {
                origin: accounts.dave.address,
                data: {}
            }), accounts.dave.signer, "third-party claim before unlock");
        }, 60000);
    });

    describe("Beneficiary Transfer", () => {
        it("should reject third-party claim while a beneficiary transfer is pending", async () => {
            const { accounts } = context;
            const purchase = await createPurchasedEscrow(
                accounts.charlie,
                VALID_LISTING_AMOUNT,
                SHORT_LOCKUP_DURATION,
            );
            const escrowContract = context.alphaLockupSdk.getContract(purchase.escrowAccount);

            await submitOk(escrowContract.send("propose_beneficiary", {
                origin: accounts.charlie.address,
                data: { new_beneficiary: accounts.dave.address }
            }), accounts.charlie.signer, "propose_beneficiary");

            await escrowStakeAfterUnlock(purchase.escrowAccount);

            const pendingClaimQuery = await escrowContract.query("claim", {
                origin: accounts.eve.address,
                data: {}
            });
            expect(pendingClaimQuery.success).toBe(false);
            if (!pendingClaimQuery.success) {
                expect(pendingClaimQuery.value.type).toBe("FlagReverted");
            }

            await submitReverted(escrowContract.send("claim", withIntegrationGas({
                origin: accounts.eve.address,
                data: {}
            })), accounts.eve.signer, "claim with pending beneficiary");

            await submitOk(escrowContract.send("cancel_beneficiary_proposal", {
                origin: accounts.charlie.address,
                data: {}
            }), accounts.charlie.signer, "cancel_beneficiary_proposal");

            const claimResult = await submitOk(escrowContract.send("claim", withIntegrationGas({
                origin: accounts.eve.address,
                data: {}
            })), accounts.eve.signer, "claim after beneficiary proposal cancellation");
            const claimedEvent = escrowContract.filterEvents(claimResult.events)
                .find(event => event.type === "AlphaClaimed");
            expect(claimedEvent).toBeDefined();
            expect(claimedEvent!.value.buyer).toBe(accounts.charlie.address);
        }, 600000);

        it("should pay the new beneficiary after a coldkey handoff", async () => {
            const { accounts } = context;
            const listAmount = VALID_LISTING_AMOUNT;

            const daveStakeBefore = await getStakeBalance(
                context.api,
                context.accounts.alice.address,
                netuid,
                accounts.dave.address
            );

            const purchase = await createPurchasedEscrow(accounts.charlie, listAmount, SHORT_LOCKUP_DURATION);
            const escrowContract = context.alphaLockupSdk.getContract(purchase.escrowAccount);

            await submitOk(escrowContract.send("propose_beneficiary", {
                origin: accounts.charlie.address,
                data: { new_beneficiary: accounts.dave.address }
            }), accounts.charlie.signer, "propose_beneficiary");

            await submitOk(escrowContract.send("accept_beneficiary", {
                origin: accounts.dave.address,
                data: {}
            }), accounts.dave.signer, "accept_beneficiary");

            const beneficiary = queryOk<string>(await escrowContract.query("get_beneficiary", {
                origin: accounts.alice.address,
                data: {}
            }), "get_beneficiary");
            expect(beneficiary).toBe(accounts.dave.address);

            const escrowStakeBeforeClaim = await escrowStakeAfterUnlock(purchase.escrowAccount);
            expect(escrowStakeBeforeClaim).toBeGreaterThanOrEqual(listAmount - 3n * TRANSFER_TOLERANCE);

            const claimResult = await submitOk(escrowContract.send("claim", withIntegrationGas({
                origin: accounts.eve.address,
                data: {}
            })), accounts.eve.signer, "claim after beneficiary handoff");
            const claimedEvent = escrowContract.filterEvents(claimResult.events)
                .find(e => e.type === "AlphaClaimed");
            expect(claimedEvent).toBeDefined();
            expect(claimedEvent!.value.buyer).toBe(accounts.dave.address);
            expect(claimedEvent!.value.initiated_by).toBe(accounts.eve.address);
            const claimedAmount = BigInt(claimedEvent!.value.amount_claimed);
            expect(claimedAmount).toBeGreaterThanOrEqual(escrowStakeBeforeClaim);
            expect(claimedAmount - escrowStakeBeforeClaim).toBeLessThanOrEqual(CLAIM_FINALIZATION_EMISSION_TOLERANCE);

            const daveStakeAfter = await getStakeBalance(
                context.api,
                context.accounts.alice.address,
                netuid,
                accounts.dave.address
            );
            expect(daveStakeAfter - daveStakeBefore).toBeGreaterThanOrEqual(claimedAmount - SMALL_ACCOUNTING_TOLERANCE);
        }, 600000);
    });

    describe("Escrow Query Functions", () => {
        it("should return correct buyer from escrow", async () => {
            const { accounts } = context;

            const { escrowAccount } = await createPurchasedEscrow(accounts.dave, VALID_LISTING_AMOUNT, SHORT_LOCKUP_DURATION);
            const escrowContract = context.alphaLockupSdk.getContract(escrowAccount);

            const buyer = queryOk<string>(await escrowContract.query("get_buyer", {
                origin: accounts.alice.address,
                data: {}
            }), "get_buyer");
            expect(buyer).toBe(accounts.dave.address);
        }, 180000);
    });
});
