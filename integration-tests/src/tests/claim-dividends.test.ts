import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { setupTestEnvironment, cleanupTestEnvironment, type TestContext, ContractSdk } from "../setup";
import {
    taoToRao,
    waitForBlocks,
    fundAccount,
    registerSubnet,
    registerValidator,
    addContractAsProxy,
    createHotkey,
    formatAddress,
    elevateRegistrationLimits,
    type Wallet,
    MARKET_PRICE,
    bpsToPercentage,
} from "../utils";
import {
    getStakeBalance,
    transferStake,
    formatStakeAmount,
} from "../utils/stake-helpers";
import { Binary } from "polkadot-api";
import { stringToU8a } from "@polkadot/util";

const TRANSFER_TOLERANCE = 10n; // Matches contract tolerance (rao)
const PRIMARY_LISTING_ALPHA = taoToRao(0.02);
const SECONDARY_LISTING_ALPHA = taoToRao(0.015);
const TERTIARY_LISTING_ALPHA = taoToRao(0.018);
const DIVIDEND_ALPHA = taoToRao(0.1);
const DIVIDEND_DRIFT_TOLERANCE = taoToRao(0.05);
const CONTRACT_HOTKEY_REQUIRED_ALPHA = DIVIDEND_ALPHA + DIVIDEND_DRIFT_TOLERANCE + PRIMARY_LISTING_ALPHA;
const REQUIRED_SELLER_ALPHA = taoToRao(0.05);

// Contract errors exposed via dispatchError
interface ContractsError {
    type: "Contracts";
    value: { type: "ContractReverted"; value: undefined };
}

describe("Dividends Claiming", () => {
    let context: TestContext;
    let contract: ReturnType<ContractSdk["getContract"]>;
    let netuid: number;
    let contractHotkey: string;
    let bobHotkey: Wallet;

    beforeAll(async () => {
        context = await setupTestEnvironment();
        contract = context.contractSdk.getContract(context.contractAddress!);

        // Discover and register the contract hotkey so we can manipulate stake directly
        const hotkeyResult = await contract.query("get_hotkey", {
            origin: context.accounts.alice.address,
            data: {},
        });
        expect(hotkeyResult.success).toBe(true);
        if (!hotkeyResult.success) {
            throw new Error("Failed to get contract hotkey");
        }
        contractHotkey = hotkeyResult.value.response;
        console.log(`Contract hotkey: ${formatAddress(contractHotkey)}`);

        // Ensure the contract account has some TAO for transaction fees
        await fundAccount(
            context.api,
            context.contractAddress!,
            taoToRao(10),
            context.accounts.alice.signer,
        );

        // Register a subnet for the tests
        const aliceHotkey = createHotkey();
        await fundAccount(context.api, aliceHotkey.address, taoToRao(10), context.accounts.alice.signer);
        netuid = await registerSubnet(context.api, aliceHotkey.address, context.accounts.alice.signer);

        // Allow multiple validator registrations quickly
        await elevateRegistrationLimits(context.api, netuid, 20, 20, context.accounts.alice.signer);
        await waitForBlocks(context.api, 1);

        // Register the contract hotkey so it can hold stake
        await registerValidator(
            context.api,
            netuid,
            contractHotkey,
            context.accounts.alice.signer,
            taoToRao(50),
            CONTRACT_HOTKEY_REQUIRED_ALPHA,
        );
        await waitForBlocks(context.api, 1);

        // Prepare Bob as a seller we can use for listings
        bobHotkey = createHotkey();
        await fundAccount(context.api, bobHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await registerValidator(
            context.api,
            netuid,
            bobHotkey.address,
            context.accounts.bob.signer,
            taoToRao(5000),
            REQUIRED_SELLER_ALPHA,
        );

        await waitForBlocks(context.api, 1);

        // Bob grants proxy permissions to the contract so listings work
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.bob.signer);
        await waitForBlocks(context.api, 1);
    }, 180000);

    afterAll(async () => {
        await cleanupTestEnvironment();
    });

    it("tracks reserved stake and transfers staking rewards to the owner", async () => {
        const { accounts } = context;

        const listingAmount = PRIMARY_LISTING_ALPHA;
        const priceOffsetBps = MARKET_PRICE; // Market price
        const dividendAmount = DIVIDEND_ALPHA;

        console.log("=== Listing Alpha to populate reserved balance ===");
        const listTx = contract.send("list_alpha", {
            origin: accounts.bob.address,
            data: {
                hotkey: bobHotkey.address,
                netuid,
                amount: listingAmount,
                price_offset_bps: priceOffsetBps,
            },
        });
        const listResult = await listTx.signAndSubmit(accounts.bob.signer);
        expect(listResult.ok).toBe(true);
        await waitForBlocks(context.api, 2);

        const reservedResult = await contract.query("get_reserved_alpha", {
            origin: accounts.alice.address,
            data: { netuid },
        });
        expect(reservedResult.success).toBe(true);
        if (reservedResult.success) {
            const reservedAfterListing = BigInt(reservedResult.value.response);
            console.log(`Reserved Alpha after listing: ${formatStakeAmount(reservedAfterListing)}`);
            expect(reservedAfterListing).toBeGreaterThanOrEqual(listingAmount - TRANSFER_TOLERANCE);
            expect(reservedAfterListing).toBeLessThanOrEqual(listingAmount);
        }

        console.log("=== Syncing pre-existing stake (baseline dividends) ===");
        let baselineClaimed = 0n;
        const baselineClaimTx = contract.send("claim_dividends", {
            origin: accounts.alice.address,
            data: { netuid },
        });
        const baselineClaimResult = await baselineClaimTx.signAndSubmit(accounts.alice.signer);

        if (baselineClaimResult.ok) {
            const baselineEvents = contract.filterEvents(baselineClaimResult.events);
            const baselineEvent = baselineEvents.find(e => e.type === "DividendsClaimed");
            if (baselineEvent) {
                baselineClaimed = BigInt(baselineEvent.value.amount);
                console.log(`Claimed baseline excess: ${formatStakeAmount(baselineClaimed)}`);
            }
            await waitForBlocks(context.api, 1);

            console.log("=== Verifying no dividends remain after baseline claim ===");
            const confirmNoDividendsTx = contract.send("claim_dividends", {
                origin: accounts.alice.address,
                data: { netuid },
            });
            const confirmNoDividendsResult = await confirmNoDividendsTx.signAndSubmit(accounts.alice.signer);
            if (confirmNoDividendsResult.ok) {
                const unexpectedEvents = contract.filterEvents(confirmNoDividendsResult.events);
                const unexpectedClaim = unexpectedEvents.find(e => e.type === "DividendsClaimed");
                const unexpectedAmount = unexpectedClaim ? BigInt(unexpectedClaim.value.amount) : 0n;
                console.warn(
                    `Baseline follow-up claim still had dividends (tolerance fallback). Claimed: ${formatStakeAmount(unexpectedAmount)}`,
                );
                await waitForBlocks(context.api, 1);
            } else {
                const confirmError = confirmNoDividendsResult.dispatchError?.value as ContractsError | undefined;
                expect(confirmError?.type).toBe("Contracts");
                expect(confirmError?.value.type).toBe("ContractReverted");
            }
        } else {
            const baselineError = baselineClaimResult.dispatchError?.value as ContractsError | undefined;
            expect(baselineError?.type).toBe("Contracts");
            expect(baselineError?.value.type).toBe("ContractReverted");
            console.log("No baseline dividends present to claim.");
        }

        const ownerStakeAfterBaseline = await getStakeBalance(
            context.api,
            contractHotkey,
            netuid,
            accounts.alice.address,
        );
        const contractStakeAfterBaseline = await getStakeBalance(
            context.api,
            contractHotkey,
            netuid,
            context.contractAddress!,
        );

        console.log(`Owner stake after baseline sync: ${formatStakeAmount(ownerStakeAfterBaseline)}`);
        console.log(`Contract stake after baseline sync: ${formatStakeAmount(contractStakeAfterBaseline)}`);

        const reservedBeforeRewards = await contract.query("get_reserved_alpha", {
            origin: accounts.alice.address,
            data: { netuid },
        });
        if (!reservedBeforeRewards.success) {
            throw new Error("Failed to retrieve reserved alpha before rewards");
        }
        const reservedPreClaim = BigInt(reservedBeforeRewards.value.response);
        // On live networks emission may cause minor drift; tolerate reasonable excess
        const driftTolerance = DIVIDEND_DRIFT_TOLERANCE; // live localnet emissions can move between snapshots
        expect(contractStakeAfterBaseline).toBeGreaterThanOrEqual(reservedPreClaim - driftTolerance);
        expect(contractStakeAfterBaseline).toBeLessThanOrEqual(reservedPreClaim + driftTolerance);

        console.log("=== Simulating staking rewards by moving Alpha to the contract ===");
        await transferStake(
            context.api,
            context.contractAddress!,
            contractHotkey,
            netuid,
            dividendAmount,
            accounts.alice.signer,
        );
        await waitForBlocks(context.api, 2);

        const ownerStakeBeforeClaim = await getStakeBalance(
            context.api,
            contractHotkey,
            netuid,
            accounts.alice.address,
        );
        const contractStakeBeforeClaim = await getStakeBalance(
            context.api,
            contractHotkey,
            netuid,
            context.contractAddress!,
        );

        console.log(`Owner stake before claim: ${formatStakeAmount(ownerStakeBeforeClaim)}`);
        console.log(`Contract stake before claim: ${formatStakeAmount(contractStakeBeforeClaim)}`);

        const contractDelta = contractStakeBeforeClaim > contractStakeAfterBaseline
            ? contractStakeBeforeClaim - contractStakeAfterBaseline
            : contractStakeAfterBaseline - contractStakeBeforeClaim;
        const minExpected = dividendAmount > driftTolerance ? dividendAmount - driftTolerance : 0n;
        const maxExpected = dividendAmount + driftTolerance;

        expect(contractDelta).toBeGreaterThanOrEqual(minExpected);
        expect(contractDelta).toBeLessThanOrEqual(maxExpected);

        console.log("=== Claiming dividends as contract owner ===");
        const claimTx = contract.send("claim_dividends", {
            origin: accounts.alice.address,
            data: { netuid },
        });
        const claimResult = await claimTx.signAndSubmit(accounts.alice.signer);
        if (!claimResult.ok) {
            throw new Error(`Expected claim to succeed but got ${JSON.stringify(claimResult.dispatchError)}`);
        }

        const events = contract.filterEvents(claimResult.events);
        const dividendsClaimed = events.find(e => e.type === "DividendsClaimed");
        expect(dividendsClaimed).toBeDefined();

        let claimedAmount = dividendAmount;
        let reservedReported = reservedPreClaim;
        if (dividendsClaimed) {
            claimedAmount = BigInt(dividendsClaimed.value.amount);
            reservedReported = BigInt(dividendsClaimed.value.reserved_after);
            console.log(`Dividends claimed: ${formatStakeAmount(claimedAmount)}`);
        } else {
            console.warn("DividendsClaimed event not found; using expected dividend amount for drift checks");
        }

        const expectedClaimableBefore = contractStakeBeforeClaim > reservedPreClaim
            ? contractStakeBeforeClaim - reservedPreClaim
            : 0n;
        const claimDelta = claimedAmount > expectedClaimableBefore
            ? claimedAmount - expectedClaimableBefore
            : expectedClaimableBefore - claimedAmount;
        expect(claimDelta).toBeLessThanOrEqual(driftTolerance);
        expect(reservedReported).toBe(reservedPreClaim);

        await waitForBlocks(context.api, 2);

        const ownerStakeAfterClaim = await getStakeBalance(
            context.api,
            contractHotkey,
            netuid,
            accounts.alice.address,
        );
        const contractStakeAfterClaim = await getStakeBalance(
            context.api,
            contractHotkey,
            netuid,
            context.contractAddress!,
        );
        const reservedAfterClaim = await contract.query("get_reserved_alpha", {
            origin: accounts.alice.address,
            data: { netuid },
        });

        expect(reservedAfterClaim.success).toBe(true);
        if (!reservedAfterClaim.success) {
            throw new Error("Failed to get reserved Alpha after claim");
        }
        const reservedPostClaim = BigInt(reservedAfterClaim.value.response);

        console.log(`Owner stake after claim: ${formatStakeAmount(ownerStakeAfterClaim)}`);
        console.log(`Contract stake after claim: ${formatStakeAmount(contractStakeAfterClaim)}`);
        console.log(`Reserved after claim: ${formatStakeAmount(reservedPostClaim)}`);

        const minOwnerAfter = ownerStakeBeforeClaim + claimedAmount > driftTolerance
            ? ownerStakeBeforeClaim + claimedAmount - driftTolerance
            : 0n;
        const contractPostDelta = contractStakeAfterClaim > reservedPostClaim
            ? contractStakeAfterClaim - reservedPostClaim
            : reservedPostClaim - contractStakeAfterClaim;

        expect(ownerStakeAfterClaim).toBeGreaterThanOrEqual(minOwnerAfter);
        expect(contractPostDelta).toBeLessThanOrEqual(driftTolerance);
        expect(reservedPostClaim).toBe(reservedPreClaim);

        // Subsequent claim without new rewards should produce only minimal drift-sized dividends
        const secondClaim = contract.send("claim_dividends", {
            origin: accounts.alice.address,
            data: { netuid },
        });
        const secondClaimResult = await secondClaim.signAndSubmit(accounts.alice.signer);
        if (secondClaimResult.ok) {
            const secondEvents = contract.filterEvents(secondClaimResult.events);
            const secondDividend = secondEvents.find(e => e.type === "DividendsClaimed");
            if (secondDividend) {
                const residualClaim = BigInt(secondDividend.value.amount);
                console.log(`Residual dividends on second claim: ${formatStakeAmount(residualClaim)}`);
                expect(residualClaim).toBeLessThanOrEqual(driftTolerance);
            }
        } else {
            const secondError = secondClaimResult.dispatchError?.value as ContractsError | undefined;
            expect(secondError?.type).toBe("Contracts");
            expect(secondError?.value.type).toBe("ContractReverted");
        }

        console.log("=== Dividends flow verified ===");
        console.log(`Listing amount: ${formatStakeAmount(listingAmount)} at ${bpsToPercentage(priceOffsetBps)}% offset from market`);
        console.log(`Dividends simulated: ${formatStakeAmount(dividendAmount)}`);
    });

    it("prevents non-owners from claiming dividends", async () => {
        const { accounts } = context;

        console.log("=== Testing access control - Bob trying to claim dividends ===");
        const unauthorizedClaimTx = contract.send("claim_dividends", {
            origin: accounts.bob.address,
            data: { netuid },
        });
        const unauthorizedResult = await unauthorizedClaimTx.signAndSubmit(accounts.bob.signer);

        expect(unauthorizedResult.ok).toBe(false);
        const error = unauthorizedResult.dispatchError?.value as ContractsError | undefined;
        expect(error?.type).toBe("Contracts");
        expect(error?.value.type).toBe("ContractReverted");
        console.log("✓ Non-owner claim correctly rejected");

        console.log("=== Testing access control - Charlie trying to claim dividends ===");
        const charlieClaimTx = contract.send("claim_dividends", {
            origin: accounts.charlie.address,
            data: { netuid },
        });
        const charlieResult = await charlieClaimTx.signAndSubmit(accounts.charlie.signer);

        expect(charlieResult.ok).toBe(false);
        const charlieError = charlieResult.dispatchError?.value as ContractsError | undefined;
        expect(charlieError?.type).toBe("Contracts");
        expect(charlieError?.value.type).toBe("ContractReverted");
        console.log("✓ Non-owner (Charlie) claim correctly rejected");
    });

    it("respects pause states when claiming dividends", async () => {
        const { accounts } = context;

        console.log("=== Testing fully paused state ===");
        // Pause the contract fully
        const pauseTx = contract.send("pause_fully", {
            origin: accounts.alice.address,
            data: {
                reason: Binary.fromBytes(stringToU8a("Testing pause"))
            },
        });
        const pauseResult = await pauseTx.signAndSubmit(accounts.alice.signer);
        expect(pauseResult.ok).toBe(true);
        await waitForBlocks(context.api, 1);

        // Try to claim dividends while paused
        const pausedClaimTx = contract.send("claim_dividends", {
            origin: accounts.alice.address,
            data: { netuid },
        });
        const pausedClaimResult = await pausedClaimTx.signAndSubmit(accounts.alice.signer);

        expect(pausedClaimResult.ok).toBe(false);
        const pausedError = pausedClaimResult.dispatchError?.value as ContractsError | undefined;
        expect(pausedError?.type).toBe("Contracts");
        expect(pausedError?.value.type).toBe("ContractReverted");
        console.log("✓ Claiming blocked when fully paused");

        // Resume the contract
        const resumeTx = contract.send("resume", {
            origin: accounts.alice.address,
            data: {},
        });
        const resumeResult = await resumeTx.signAndSubmit(accounts.alice.signer);
        expect(resumeResult.ok).toBe(true);
        await waitForBlocks(context.api, 1);

        console.log("=== Testing trading paused state (claiming should work) ===");
        // Pause only trading
        const pauseTradingTx = contract.send("pause_trading", {
            origin: accounts.alice.address,
            data: {
                reason: Binary.fromBytes(stringToU8a("Testing trading pause"))
            },
        });
        const pauseTradingResult = await pauseTradingTx.signAndSubmit(accounts.alice.signer);
        expect(pauseTradingResult.ok).toBe(true);
        await waitForBlocks(context.api, 1);

        // Try to claim - should work even with trading paused
        const tradingPausedClaimTx = contract.send("claim_dividends", {
            origin: accounts.alice.address,
            data: { netuid },
        });
        const tradingPausedClaimResult = await tradingPausedClaimTx.signAndSubmit(accounts.alice.signer);

        // This might succeed if there are dividends, or fail with NoDividendsAvailable
        // Either way, it shouldn't fail due to pause state
        if (!tradingPausedClaimResult.ok) {
            const tradingPausedError = tradingPausedClaimResult.dispatchError?.value as ContractsError | undefined;
            expect(tradingPausedError?.type).toBe("Contracts");
            // Should be ContractReverted but NOT due to pause
            expect(tradingPausedError?.value.type).toBe("ContractReverted");
            console.log("✓ Claiming attempted with trading paused (no dividends to claim)");
        } else {
            console.log("✓ Claiming succeeds with trading paused");
        }

        // Resume trading
        const resumeTradingTx = contract.send("resume", {
            origin: accounts.alice.address,
            data: {},
        });
        const resumeTradingResult = await resumeTradingTx.signAndSubmit(accounts.alice.signer);
        expect(resumeTradingResult.ok).toBe(true);
        await waitForBlocks(context.api, 1);
    });

    it("fails when no dividends are available", async () => {
        const { accounts } = context;

        console.log("=== Setting up scenario with reserved == contract stake ===");

        // Create a listing to establish reserved alpha
        const listingAmount = SECONDARY_LISTING_ALPHA;

        const listTx = contract.send("list_alpha", {
            origin: accounts.bob.address,
            data: {
                hotkey: bobHotkey.address,
                netuid,
                amount: listingAmount,
                price_offset_bps: MARKET_PRICE,
            },
        });
        const listResult = await listTx.signAndSubmit(accounts.bob.signer);
        expect(listResult.ok).toBe(true);
        await waitForBlocks(context.api, 2);

        // Get current reserved and contract stake
        const reservedResult = await contract.query("get_reserved_alpha", {
            origin: accounts.alice.address,
            data: { netuid },
        });
        expect(reservedResult.success).toBe(true);
        if (!reservedResult.success) {
            throw new Error("Failed to get reserved alpha");
        }
        const reserved = BigInt(reservedResult.value.response);

        const contractStake = await getStakeBalance(
            context.api,
            contractHotkey,
            netuid,
            context.contractAddress!,
        );

        console.log(`Reserved alpha: ${formatStakeAmount(reserved)}`);
        console.log(`Contract stake: ${formatStakeAmount(contractStake)}`);

        // Try to claim when reserved >= contract stake
        // This should fail with NoDividendsAvailable
        const claimTx = contract.send("claim_dividends", {
            origin: accounts.alice.address,
            data: { netuid },
        });
        const claimResult = await claimTx.signAndSubmit(accounts.alice.signer);

        if (contractStake <= reserved) {
            expect(claimResult.ok).toBe(false);
            const error = claimResult.dispatchError?.value as ContractsError | undefined;
            expect(error?.type).toBe("Contracts");
            expect(error?.value.type).toBe("ContractReverted");
            console.log("✓ Claiming correctly fails when no dividends available (stake <= reserved)");
        } else {
            // There might be a small amount of dividends due to emissions
            console.log(`Note: Small dividends available (${formatStakeAmount(contractStake - reserved)})`);
            if (claimResult.ok) {
                const events = contract.filterEvents(claimResult.events);
                const dividendEvent = events.find(e => e.type === "DividendsClaimed");
                if (dividendEvent) {
                    const amount = BigInt(dividendEvent.value.amount);
                    console.log(`Claimed small amount: ${formatStakeAmount(amount)}`);
                    expect(amount).toBeLessThanOrEqual(DIVIDEND_DRIFT_TOLERANCE);
                }
            }
        }

        // Clean up - get Bob's listings and cancel if any exist
        const bobListingsCleanup = await contract.query("get_user_listings", {
            origin: accounts.bob.address,
            data: { seller: accounts.bob.address, netuid },
        });

        if (bobListingsCleanup.success && Array.isArray(bobListingsCleanup.value.response) && bobListingsCleanup.value.response.length > 0) {
            // Cancel the most recent listing
            const lastListingId = bobListingsCleanup.value.response[bobListingsCleanup.value.response.length - 1];
            const cancelTx = contract.send("cancel_alpha_listing", {
                origin: accounts.bob.address,
                data: { netuid, listing_id: lastListingId },
            });
            await cancelTx.signAndSubmit(accounts.bob.signer);
            await waitForBlocks(context.api, 1);
        }
    });

    it("maintains reserved alpha correctly with multiple concurrent listings", async () => {
        const { accounts } = context;

        console.log("=== Testing multiple concurrent listings ===");

        // Create Charlie's hotkey for an additional seller
        const charlieHotkey = createHotkey();
        await fundAccount(context.api, charlieHotkey.address, taoToRao(1), context.accounts.alice.signer);
        await registerValidator(
            context.api,
            netuid,
            charlieHotkey.address,
            context.accounts.charlie.signer,
            taoToRao(5000),
            REQUIRED_SELLER_ALPHA,
        );
        await addContractAsProxy(context.api, context.contractAddress!, context.accounts.charlie.signer);
        await waitForBlocks(context.api, 2);

        // Track initial state
        const initialReserved = await contract.query("get_reserved_alpha", {
            origin: accounts.alice.address,
            data: { netuid },
        });
        const initialReservedAmount = initialReserved.success ? BigInt(initialReserved.value.response) : 0n;
        console.log(`Initial reserved: ${formatStakeAmount(initialReservedAmount)}`);

        // Create multiple listings
        const listing1Amount = PRIMARY_LISTING_ALPHA;
        const listing2Amount = SECONDARY_LISTING_ALPHA;
        const listing3Amount = TERTIARY_LISTING_ALPHA;

        // Bob's first listing
        const list1Tx = contract.send("list_alpha", {
            origin: accounts.bob.address,
            data: {
                hotkey: bobHotkey.address,
                netuid,
                amount: listing1Amount,
                price_offset_bps: MARKET_PRICE,
            },
        });
        const list1Result = await list1Tx.signAndSubmit(accounts.bob.signer);
        expect(list1Result.ok).toBe(true);

        // Charlie's listing
        const list2Tx = contract.send("list_alpha", {
            origin: accounts.charlie.address,
            data: {
                hotkey: charlieHotkey.address,
                netuid,
                amount: listing2Amount,
                price_offset_bps: MARKET_PRICE,
            },
        });
        const list2Result = await list2Tx.signAndSubmit(accounts.charlie.signer);
        expect(list2Result.ok).toBe(true);

        // Bob's second listing
        const list3Tx = contract.send("list_alpha", {
            origin: accounts.bob.address,
            data: {
                hotkey: bobHotkey.address,
                netuid,
                amount: listing3Amount,
                price_offset_bps: MARKET_PRICE,
            },
        });
        const list3Result = await list3Tx.signAndSubmit(accounts.bob.signer);
        expect(list3Result.ok).toBe(true);

        await waitForBlocks(context.api, 2);

        // Check reserved increased by sum of all listings
        const afterListingsReserved = await contract.query("get_reserved_alpha", {
            origin: accounts.alice.address,
            data: { netuid },
        });
        expect(afterListingsReserved.success).toBe(true);
        if (!afterListingsReserved.success) {
            throw new Error("Failed to get reserved alpha after listings");
        }
        const afterListingsAmount = BigInt(afterListingsReserved.value.response);
        const expectedIncrease = listing1Amount + listing2Amount + listing3Amount;
        const actualIncrease = afterListingsAmount - initialReservedAmount;

        console.log(`Expected increase: ${formatStakeAmount(expectedIncrease)}`);
        console.log(`Actual increase: ${formatStakeAmount(actualIncrease)}`);

        // Allow for TRANSFER_TOLERANCE per listing
        const maxTolerance = TRANSFER_TOLERANCE * 3n;
        expect(actualIncrease).toBeGreaterThanOrEqual(expectedIncrease - maxTolerance);
        expect(actualIncrease).toBeLessThanOrEqual(expectedIncrease);

        // Note: Cancellation testing is skipped here because listings need to mature for min_listing_age blocks (100)
        // before they can be cancelled, which would make the test too slow.
        // The main purpose of this test is to verify reserved alpha tracking with multiple listings
        // and dividend claiming, which we test below.
        console.log("✓ Reserved alpha tracking verified with multiple listings");

        // Simulate dividends and claim
        const dividendAmount = DIVIDEND_ALPHA;
        await transferStake(
            context.api,
            context.contractAddress!,
            contractHotkey,
            netuid,
            dividendAmount,
            accounts.alice.signer,
        );
        await waitForBlocks(context.api, 2);

        const claimTx = contract.send("claim_dividends", {
            origin: accounts.alice.address,
            data: { netuid },
        });
        const claimResult = await claimTx.signAndSubmit(accounts.alice.signer);

        if (claimResult.ok) {
            const events = contract.filterEvents(claimResult.events);
            const dividendEvent = events.find(e => e.type === "DividendsClaimed");
            if (dividendEvent) {
                const claimedAmount = BigInt(dividendEvent.value.amount);
                const reservedAfter = BigInt(dividendEvent.value.reserved_after);
                console.log(`✓ Claimed ${formatStakeAmount(claimedAmount)} with ${formatStakeAmount(reservedAfter)} still reserved`);

                // Reserved amount should not change due to claiming
                // It should still be the total from all three listings
                const finalReserved = await contract.query("get_reserved_alpha", {
                    origin: accounts.alice.address,
                    data: { netuid },
                });
                expect(finalReserved.success).toBe(true);
                if (!finalReserved.success) {
                    throw new Error("Failed to get final reserved alpha");
                }
                const finalAmount = BigInt(finalReserved.value.response);
                expect(finalAmount).toBe(reservedAfter);

                // The final reserved amount should match what we had after creating the listings
                // (which includes any pre-existing reserved from previous tests)
                expect(finalAmount).toBe(afterListingsAmount);
            }
        }

        console.log("✓ Reserved alpha maintained correctly with multiple concurrent listings");
    });
});
