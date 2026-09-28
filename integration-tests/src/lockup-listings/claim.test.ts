import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { Binary, type TxEvent, type TxFinalized } from "polkadot-api";
import { Observable } from "rxjs";
import * as fs from "fs/promises";
import * as path from "path";
import {
    setupLockupListingsEnvironment,
    cleanupTestEnvironment,
    type LockupListingsContext,
    type TestAccount,
    LockupListingsSdk,
    bigintReplacer,
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
    getBalance,
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
// Claim tests care about escrow settlement, not exact purchase pricing. On thin
// later subnets in a shared localnet, the market price can move between the
// estimate query and finalized purchase; the contract safely refunds overpay.
const CLAIM_PURCHASE_PAYMENT_DRIFT_BUFFER = taoToRao(10);
// A finalized claim can include emissions accrued between our pre-claim stake
// snapshot and the block that executes the escrow transfer. Keep this named and
// isolated: contract-owned state is asserted exactly elsewhere.
const CLAIM_FINALIZATION_EMISSION_TOLERANCE = 200_000n;

describe("Lockup Listings Contract - Claim", () => {
    let context: LockupListingsContext;
    let contract: ReturnType<LockupListingsSdk["getContract"]>;
    let netuid: number;

    let bobHotkey: Wallet;

    beforeAll(async () => {
        context = await setupLockupListingsEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);

        const updateMinListingTx = contract.send("update_min_listing_amount", {
            origin: context.accounts.alice.address,
            data: { new_amount: BITTENSOR_MIN_STAKE }
        });
        await submitOk(updateMinListingTx, context.accounts.alice.signer, "update_min_listing_amount");

        const updateMinPurchaseTx = contract.send("update_min_purchase_amount", {
            origin: context.accounts.alice.address,
            data: { new_amount: BITTENSOR_MIN_STAKE }
        });
        await submitOk(updateMinPurchaseTx, context.accounts.alice.signer, "update_min_purchase_amount");

        // Create a subnet for testing
        const aliceHotkey = createHotkey();
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
        const contractHotkeyResult = await contract.query("get_hotkey", {
            origin: context.accounts.alice.address,
            data: {}
        });

        if (contractHotkeyResult.success) {
            const contractHotkey = contractHotkeyResult.value.response;
            await fundAccount(context.api, context.contractAddress!, taoToRao(10), context.accounts.alice.signer);
            await registerValidator(
                context.api,
                netuid,
                contractHotkey,
                context.accounts.alice.signer,
                taoToRao(100)
            );
        }

        // Create and register Bob's hotkey
        bobHotkey = createHotkey();
        await fundAccount(context.api, bobHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await registerValidator(context.api, netuid, bobHotkey.address, context.accounts.bob.signer, taoToRao(5000), REQUIRED_ALPHA);

        await waitForBlocks(context.api, 2);

        // Add contract as proxy for Bob
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.bob.signer);

        await waitForBlocks(context.api, 2);

        // Fund Charlie (buyer) with TAO
        await fundAccount(context.api, context.accounts.charlie.address, taoToRao(200), context.accounts.alice.signer);
        await fundAccount(context.api, context.accounts.dave.address, taoToRao(20), context.accounts.alice.signer);
        await fundAccount(context.api, context.accounts.eve.address, taoToRao(20), context.accounts.alice.signer);

        console.log("Test setup complete");
    }, 300000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    const trackTx = async (obs: Observable<TxEvent>): Promise<TxFinalized> =>
        new Promise<TxFinalized>((resolve, reject) =>
            obs.subscribe({
                next: (evt) => {
                    if (evt.type === "finalized") {
                        resolve(evt);
                    }
                },
                error: (err) => reject(err),
            })
        );

    async function createBobListing(amount: bigint, lockupDuration: number): Promise<bigint> {
        const { listingId } = await createLockupListing(contract, context.accounts.bob.signer, context.accounts.bob.address, {
            hotkey: bobHotkey.address,
            netuid,
            amount,
            price_offset_bps: MARKET_PRICE,
            lockup_duration: lockupDuration,
        });
        return listingId;
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
        const listingId = await createBobListing(amount, lockupDuration);
        const [, , totalRequired] = await estimateListing(listingId, amount);
        return takeLockupListing(contract, buyer.signer, buyer.address, {
            netuid,
            seller: context.accounts.bob.address,
            listing_id: listingId,
            amount,
        }, totalRequired + CLAIM_PURCHASE_PAYMENT_DRIFT_BUFFER);
    }

    describe("Full Lifecycle", () => {
        it("should complete full lifecycle: create -> take -> claim", async () => {
            const { accounts } = context;
            const listAmount = VALID_LISTING_AMOUNT;
            const lockupDuration = SHORT_LOCKUP_DURATION; // 10 blocks

            // Get Charlie's stake before (should be 0 or minimal)
            const buyerStakeBefore = await getStakeBalance(
                context.api,
                context.accounts.alice.address, // contract's hotkey
                netuid,
                accounts.charlie.address
            );
            console.log(`Charlie's stake before: ${formatStakeAmount(buyerStakeBefore)}`);

            // Step 1: Bob creates a lockup listing
            console.log("Step 1: Creating lockup listing...");
            const currentBlock = await getCurrentBlock(context.api);
            console.log(`Current block: ${currentBlock}`);

            const listingId = await createBobListing(listAmount, lockupDuration);
            console.log(`Listing created with ID: ${listingId}`);

            // Step 2: Estimate price and Charlie takes the listing
            console.log("Step 2: Taking lockup listing...");
            const [, , totalRequired] = await estimateListing(listingId, listAmount);
            console.log(`Total TAO required: ${totalRequired}`);

            const blockBeforeTake = await getCurrentBlock(context.api);
            console.log(`Block before take: ${blockBeforeTake}`);

            const { event: takenEvent, purchaseId, escrowAccount } = await takeLockupListing(contract, accounts.charlie.signer, accounts.charlie.address, {
                netuid,
                seller: accounts.bob.address,
                listing_id: listingId,
                amount: listAmount
            }, totalRequired + CLAIM_PURCHASE_PAYMENT_DRIFT_BUFFER);
            expect(takenEvent.value.seller).toBe(accounts.bob.address);
            expect(takenEvent.value.buyer).toBe(accounts.charlie.address);
            expect(takenEvent.value.escrow_account).toBeDefined();
            console.log(`LockupListingTaken event - escrow: ${takenEvent.value.escrow_account}`);

            // Verify the escrow lookup with the purchase id emitted by the contract.
            const escrowLookup = queryOk<string | undefined>(await contract.query("get_escrow", {
                origin: accounts.alice.address,
                data: {
                    netuid,
                    listing_id: listingId,
                    purchase_id: purchaseId
                }
            }), "get_escrow");
            expect(escrowLookup).toBe(escrowAccount);

            const escrowAddress = escrowAccount;
            console.log(`Escrow created at: ${escrowAddress}`);

            // Get the escrow contract and query its info
            const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);

            // Query escrow info
            const infoResult = await escrowContract.query("get_info", {
                origin: accounts.charlie.address,
                data: {}
            });

            expect(infoResult.success).toBe(true);
            if (infoResult.success) {
                const info = infoResult.value.response;
                console.log(`Escrow info: buyer=${info.buyer}, unlock_block=${info.unlock_block}, claimed=${info.claimed}`);
                expect(info.buyer).toBe(accounts.charlie.address);
                expect(info.claimed).toBe(false);
            }

            // Step 3: Wait for lockup to expire
            console.log("Step 3: Waiting for lockup to expire...");
            const unlockBlockResult = await escrowContract.query("get_unlock_block", {
                origin: accounts.charlie.address,
                data: {}
            });

            if (!unlockBlockResult.success) {
                throw new Error("Failed to get unlock block");
            }

            const unlockBlock = unlockBlockResult.value.response;
            console.log(`Unlock block: ${unlockBlock}`);

            // Wait until unlock block
            await waitUntilBlock(context.api, unlockBlock);
            console.log("Lockup period expired!");

            const escrowHotkey = await queryOk<string>(await escrowContract.query("get_hotkey", {
                origin: accounts.alice.address,
                data: {}
            }), "get_hotkey");
            const escrowStakeBeforeClaim = await getStakeBalance(
                context.api,
                escrowHotkey,
                netuid,
                escrowAddress
            );
            expect(escrowStakeBeforeClaim).toBeGreaterThanOrEqual(listAmount);

            // Step 4: Dave triggers the claim for Charlie, simulating backend settlement
            console.log("Step 4: Claiming escrow via third-party trigger...");
            const claimTx = escrowContract.send("claim", withIntegrationGas({
                origin: accounts.dave.address,
                data: {}
            }));

            const claimResult = await submitOk(claimTx, accounts.dave.signer, "claim");
            console.log(`Claim result: ${claimResult.ok}`);

            // Verify AlphaClaimed event from escrow contract
            const claimEvents = escrowContract.filterEvents(claimResult.events);
            const claimedEvent = claimEvents.find(e => e.type === "AlphaClaimed");
            expect(claimedEvent).toBeDefined();
            if (claimedEvent) {
                expect(claimedEvent.value.buyer).toBe(accounts.charlie.address);
                expect(claimedEvent.value.initiated_by).toBe(accounts.dave.address);
                expect(claimedEvent.value.netuid).toBe(netuid);
                const claimedAmount = BigInt(claimedEvent.value.amount_claimed);
                expect(claimedAmount).toBeGreaterThanOrEqual(escrowStakeBeforeClaim);
                expect(claimedAmount - escrowStakeBeforeClaim).toBeLessThanOrEqual(CLAIM_FINALIZATION_EMISSION_TOLERANCE);
                console.log(`AlphaClaimed event - amount_claimed: ${claimedEvent.value.amount_claimed}`);
            }

            // Verify Charlie received the Alpha
            const buyerStakeAfter = await getStakeBalance(
                context.api,
                context.accounts.alice.address, // contract's hotkey
                netuid,
                accounts.charlie.address
            );
            console.log(`Charlie's stake after claim: ${formatStakeAmount(buyerStakeAfter)}`);

            // Charlie should have received the emitted claim amount. Runtime stake
            // snapshots may include a tiny amount of live-chain accounting drift.
            const stakeIncrease = buyerStakeAfter - buyerStakeBefore;
            const tolerance = SMALL_ACCOUNTING_TOLERANCE;
            expect(stakeIncrease).toBeGreaterThanOrEqual(BigInt(claimedEvent!.value.amount_claimed) - tolerance);

            const syncClosedEscrowResult = await submitOk(contract.send("sync_escrow_hotkey", withIntegrationGas({
                origin: accounts.eve.address,
                data: {
                    netuid,
                    listing_id: listingId,
                    purchase_id: purchaseId,
                }
            })), accounts.eve.signer, "prune closed escrow");
            const closedEvent = contract.filterEvents(syncClosedEscrowResult.events)
                .find(event => event.type === "EscrowClosedPruned");
            expect(closedEvent).toBeDefined();
            expect(closedEvent!.value.escrow).toBe(escrowAddress);
            expect(closedEvent!.value.initiated_by).toBe(accounts.eve.address);
            expect(closedEvent!.value.netuid).toBe(netuid);
            expect(closedEvent!.value.listing_id).toBe(listingId);
            expect(closedEvent!.value.purchase_id).toBe(purchaseId);

            const escrowAfterPrune = queryOk<string | undefined>(await contract.query("get_escrow", {
                origin: accounts.eve.address,
                data: {
                    netuid,
                    listing_id: listingId,
                    purchase_id: purchaseId,
                }
            }), "get_escrow after pruning");
            expect(escrowAfterPrune).toBeUndefined();
        }, 600000); // 10 minute timeout for full lifecycle test
    });

    describe("Claim Error Cases", () => {
        let escrowAddress: string;

        beforeAll(async () => {
            const { accounts } = context;
            const listAmount = VALID_LISTING_AMOUNT;
            const lockupDuration = 100; // Longer lockup for error tests

            const purchase = await createPurchasedEscrow(accounts.charlie, listAmount, lockupDuration);
            escrowAddress = purchase.escrowAccount;
        }, 300000);

        it("should fail to claim before unlock block", async () => {
            const { accounts } = context;

            const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);

            // Check blocks remaining
            const blocksRemainingResult = await escrowContract.query("blocks_until_unlock", {
                origin: accounts.charlie.address,
                data: {}
            });

            if (blocksRemainingResult.success) {
                console.log(`Blocks until unlock: ${blocksRemainingResult.value.response}`);
            }

            // Try to claim before unlock
            const claimTx = escrowContract.send("claim", {
                origin: accounts.charlie.address,
                data: {}
            });

            await submitReverted(claimTx, accounts.charlie.signer, "claim before unlock");
        }, 60000);

        it("should fail before unlock even when triggered by a third party", async () => {
            const { accounts } = context;

            const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);

            // Dave tries to trigger claim before unlock
            const claimTx = escrowContract.send("claim", {
                origin: accounts.dave.address,
                data: {}
            });

            await submitReverted(claimTx, accounts.dave.signer, "third-party claim before unlock");
        }, 60000);
    });

    describe("Beneficiary Transfer and Recovery", () => {
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

            const unlockBlock = queryOk<number>(await escrowContract.query("get_unlock_block", {
                origin: accounts.alice.address,
                data: {}
            }), "get_unlock_block");
            await waitUntilBlock(context.api, unlockBlock);

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

        it("should allow coldkey handoff before backend-triggered claim", async () => {
            const { accounts } = context;
            const listAmount = VALID_LISTING_AMOUNT;
            const lockupDuration = SHORT_LOCKUP_DURATION;

            const daveStakeBefore = await getStakeBalance(
                context.api,
                context.accounts.alice.address,
                netuid,
                accounts.dave.address
            );

            const takenEvent = (await createPurchasedEscrow(accounts.charlie, listAmount, lockupDuration)).event;
            const escrowAddress = takenEvent.value.escrow_account;

            const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);

            const proposeTx = escrowContract.send("propose_beneficiary", {
                origin: accounts.charlie.address,
                data: {
                    new_beneficiary: accounts.dave.address
                }
            });
            await submitOk(proposeTx, accounts.charlie.signer, "propose_beneficiary");

            const acceptTx = escrowContract.send("accept_beneficiary", {
                origin: accounts.dave.address,
                data: {}
            });
            await submitOk(acceptTx, accounts.dave.signer, "accept_beneficiary");

            const beneficiaryResult = await escrowContract.query("get_beneficiary", {
                origin: accounts.alice.address,
                data: {}
            });
            expect(beneficiaryResult.success).toBe(true);
            if (beneficiaryResult.success) {
                expect(beneficiaryResult.value.response).toBe(accounts.dave.address);
            }

            const unlockBlockResult = await escrowContract.query("get_unlock_block", {
                origin: accounts.alice.address,
                data: {}
            });
            if (!unlockBlockResult.success) {
                throw new Error("Failed to query unlock block");
            }

            await waitUntilBlock(context.api, unlockBlockResult.value.response);

            const escrowHotkey = await queryOk<string>(await escrowContract.query("get_hotkey", {
                origin: accounts.alice.address,
                data: {}
            }), "get_hotkey");
            const escrowStakeBeforeClaim = await getStakeBalance(
                context.api,
                escrowHotkey,
                netuid,
                escrowAddress
            );
            expect(escrowStakeBeforeClaim).toBeGreaterThanOrEqual(listAmount);

            const claimTx = escrowContract.send("claim", withIntegrationGas({
                origin: accounts.eve.address,
                data: {}
            }));
            const claimResult = await submitOk(claimTx, accounts.eve.signer, "claim after beneficiary handoff");
            const claimEvents = escrowContract.filterEvents(claimResult.events);
            const claimedEvent = claimEvents.find(e => e.type === "AlphaClaimed");
            expect(claimedEvent).toBeDefined();
            if (claimedEvent) {
                expect(claimedEvent.value.buyer).toBe(accounts.dave.address);
                expect(claimedEvent.value.initiated_by).toBe(accounts.eve.address);
                const claimedAmount = BigInt(claimedEvent.value.amount_claimed);
                expect(claimedAmount).toBeGreaterThanOrEqual(escrowStakeBeforeClaim);
                expect(claimedAmount - escrowStakeBeforeClaim).toBeLessThanOrEqual(CLAIM_FINALIZATION_EMISSION_TOLERANCE);
            }

            const daveStakeAfter = await getStakeBalance(
                context.api,
                context.accounts.alice.address,
                netuid,
                accounts.dave.address
            );

            expect(daveStakeAfter - daveStakeBefore).toBeGreaterThanOrEqual(
                BigInt(claimedEvent!.value.amount_claimed) - SMALL_ACCOUNTING_TOLERANCE
            );
        }, 600000);

        it("should recover escrow TAO to beneficiary after unlock when no Alpha stake is present", async () => {
            const { accounts } = context;
            const currentBlock = await getCurrentBlock(context.api);
            const unlockBlock = currentBlock + SHORT_LOCKUP_DURATION;
            const recoveryAmount = taoToRao(2);
            const subnetStateResult = await contract.query("get_current_subnet_registration_state", {
                origin: accounts.alice.address,
                data: { netuid }
            });
            if (!subnetStateResult.success) {
                throw new Error(`Failed to query subnet registration state: ${JSON.stringify(subnetStateResult.value, bigintReplacer)}`);
            }
            const staleGeneration = BigInt(subnetStateResult.value.response.registered_subnet_counter) + 1n;

            const wasmPath = path.join(process.cwd(), "..", "target", "ink", "alpha_lockup", "alpha_lockup.wasm");
            const wasmFile = await fs.readFile(wasmPath);
            const deployer = context.alphaLockupSdk.getDeployer(Binary.fromBytes(new Uint8Array(wasmFile)));

            const dryRunResult = await deployer.dryRun("new", {
                origin: accounts.alice.address,
                data: {
                    buyer: accounts.charlie.address,
                    netuid,
                    alpha_amount: 0n,
                    unlock_block: unlockBlock,
                    hotkey: context.accounts.alice.address,
                    subnet_generation: staleGeneration,
                }
            });

            if (!dryRunResult.success) {
                throw new Error(`Failed to dry-run TAO recovery escrow deployment: ${JSON.stringify(dryRunResult.value, bigintReplacer)}`);
            }

            await trackTx(dryRunResult.value.deploy().signSubmitAndWatch(accounts.alice.signer));

            const escrowAddress = dryRunResult.value.address;
            const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);

            await fundAccount(context.api, escrowAddress, recoveryAmount, accounts.alice.signer);

            const balanceResult = await escrowContract.query("get_tao_balance", {
                origin: accounts.alice.address,
                data: {}
            });
            expect(balanceResult.success).toBe(true);
            if (balanceResult.success) {
                expect(BigInt(balanceResult.value.response)).toBeGreaterThanOrEqual(recoveryAmount);
            }

            await waitUntilBlock(context.api, unlockBlock);

            const charlieBalanceBefore = await getBalance(context.api, accounts.charlie.address);

            const thirdPartyRecoverTx = escrowContract.send("recover_tao_after_deregistration", {
                origin: accounts.dave.address,
                data: {}
            });
            const thirdPartyRecoverResult = await submitOk(thirdPartyRecoverTx, accounts.dave.signer, "recover_tao_after_deregistration");

            const recoveryEvents = escrowContract.filterEvents(thirdPartyRecoverResult.events);
            const recoveredEvent = recoveryEvents.find(e => e.type === "TaoRecovered");
            expect(recoveredEvent).toBeDefined();
            if (recoveredEvent) {
                expect(recoveredEvent.value.beneficiary).toBe(accounts.charlie.address);
                expect(recoveredEvent.value.initiated_by).toBe(accounts.dave.address);
                expect(BigInt(recoveredEvent.value.subnet_generation)).toBe(staleGeneration);
            }

            const charlieBalanceAfter = await getBalance(context.api, accounts.charlie.address);
            expect(charlieBalanceAfter).toBeGreaterThan(charlieBalanceBefore);
        }, 600000);
    });

    describe("Escrow Query Functions", () => {
        it("should return correct buyer from escrow", async () => {
            const { accounts } = context;
            const listAmount = VALID_LISTING_AMOUNT;

            // Fund Dave and have him take the listing
            await fundAccount(context.api, accounts.dave.address, taoToRao(100), accounts.alice.signer);

            const { escrowAccount } = await createPurchasedEscrow(accounts.dave, listAmount, SHORT_LOCKUP_DURATION);
            const escrowAddress = escrowAccount;

            const escrowContract = context.alphaLockupSdk.getContract(escrowAddress);

            // Query buyer
            const buyerResult = await escrowContract.query("get_buyer", {
                origin: accounts.alice.address,
                data: {}
            });

            expect(buyerResult.success).toBe(true);
            if (buyerResult.success) {
                expect(buyerResult.value.response).toBe(accounts.dave.address);
            }
        }, 180000);
    });
});
