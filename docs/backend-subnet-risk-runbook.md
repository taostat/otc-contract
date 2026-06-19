# Backend Subnet Risk Runbook

This runbook describes the preventive backend flow for open OTC Alpha lockup listings on subnets that are at risk of deregistration. Risk detection is owned by the backend and the existing subnet-risk API; this contract only provides a narrow cancellation role plus an admin-mediated stale TAO recovery fallback.

## Goals

- Force-cancel open listings before a subnet deregisters, returning Alpha to each stored seller.
- Avoid giving automation any destination-selection or contract-admin authority.
- Alert operators if the race is lost and a listing becomes stale before cancellation.
- Use stale listing TAO recovery only as a fallback, with the owner/multisig attesting the aggregate TAO pool amount.

## Required Configuration

- RPC endpoint for the target chain.
- `lockup_listings` contract address.
- Subnet-risk API endpoint.
- `risk_canceller` account and signing key.
- Poll interval.
- Retry and backoff settings.
- Alert channel.
- Maximum transactions per run.

## Contract Setup

The contract owner or multisig must configure the automation account:

```text
set_risk_canceller(account)
```

The `risk_canceller` can call only:

```text
force_cancel_lockup_listing(netuid, seller, listing_id)
```

It cannot update owner/admin configuration, pause or resume the contract, upgrade code, update hotkeys, or open/increase stale recovery pools.

## Cron Flow

1. Poll the subnet-risk API for netuids that are at risk of deregistration.
2. Query open lockup listings for those netuids from the indexer or contract views.
3. For each open listing, submit:

```text
force_cancel_lockup_listing(netuid, seller, listing_id)
```

4. Treat `ListingNotFound` as already resolved.
5. Retry transient RPC, nonce, inclusion, or runtime failures with bounded backoff.
6. Stop at the configured maximum transactions per run and continue on the next poll.
7. Monitor `LockupListingForceCancelled` events to confirm Alpha was returned to the stored seller.

The cron never supplies a payout destination. The contract always returns Alpha to the seller stored in the listing.

## Stale Listing Race

If force-cancel fails with either of these errors, the subnet generation changed before cancellation completed:

- `SubnetNotFound`
- `SubnetGenerationMismatch`

At that point normal listing cancellation is intentionally blocked because the contract can no longer safely move Alpha for the original subnet generation.

Operators should:

1. Confirm the old subnet generation is gone or the netuid has been reused.
2. Determine the aggregate TAO amount credited to the factory from that stale listing generation.
3. Store supporting evidence off-chain and compute an `evidence_hash`.
4. Fund or leave enough TAO in the factory contract.
5. Have the owner/multisig call:

```text
open_stale_listing_recovery_pool(netuid, subnet_generation, tao_amount, evidence_hash)
```

If additional TAO must be added later:

```text
increase_stale_listing_recovery_pool(netuid, subnet_generation, additional_tao, evidence_hash)
```

After a pool is open, anyone can call:

```text
recover_stale_listing_tao(netuid, seller, listing_id)
```

The recovery payout always goes to the stored listing seller. Distribution is pro-rata against the pool's remaining TAO and remaining stale Alpha; the final recovered listing receives any rounding dust.

## Monitoring

Monitor these events:

- `RiskCancellerUpdated`
- `LockupListingForceCancelled`
- `StaleListingRecoveryPoolOpened`
- `StaleListingRecoveryPoolIncreased`
- `StaleListingTaoRecovered`
- `StaleListingRecoveryPoolClosed`

Alert on:

- Repeated force-cancel transaction failures.
- Any force-cancel result of `SubnetNotFound` or `SubnetGenerationMismatch`.
- Stale recovery pools that remain open longer than expected.
- Non-zero `get_reserved_alpha_for_generation(netuid, generation)` after a recovery pool closes.
- Non-zero `get_reserved_recovery_tao()` that does not correspond to open recovery pools.

## Trust Boundary

The cron flow is preventive and race-prone. It is safer than doing nothing, but it cannot guarantee cancellation before deregistration.

Stale listing TAO recovery is not fully trustless because the owner/multisig attests the aggregate TAO amount placed into the recovery pool. The contract does enforce the individual seller destination, per-listing eligibility, pro-rata distribution, final dust assignment, and reserved-accounting cleanup.

The long-term trustless design remains per-listing custody escrows, so each stale listing can recover only its own liquidated TAO without an owner-attested aggregate pool.
