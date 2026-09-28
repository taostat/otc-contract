# Design: Permissionless Positions

| | |
|---|---|
| Status | Draft for discussion |
| Date | 2026-09-28 |
| Scope | `lockup_listings`, `otc`, `alpha_lockup`, `otc-shared` |
| Supersedes | Pooled custody, including the mitigations added in PR #6 |

## Summary

Today each marketplace contract holds every seller's Alpha under one coldkey. That pooling is why we run an operator. When a subnet is deregistered, the chain pays one lump of TAO to the pool, and nothing on-chain says whose it is. So an owner has to attest amounts, and a cron has to cancel listings before it happens.

This design gives every position its own contract and coldkey: a **listing vault** per listing and, as today, an **escrow** per purchase. Each position settles itself. Anyone can trigger its exits, but funds only ever go to the position's owner or to a buyer who paid. Deregistration, hotkey swaps and re-registered netuids stop being operator problems, and the backend service goes away.

Contracts stay **upgradeable**, because Subtensor ships breaking changes. Upgrades go through a timelocked code registry with a guardian veto, and users can always exit (or, for locked escrows, opt out of forced upgrades) before an upgrade applies.

The design needs **no runtime changes**. It uses only the chain-extension functions `main` already uses (0, 5, 6 and 15).

## Goals and constraints

**Goals**

1. No operator is needed for fund safety: the user, or anyone, can trigger every exit.
2. No single key can take or redirect user funds, or change code without public notice.
3. No backend service: the risk cron and the owner-attested recovery flow are deleted.

**Constraints**

- **Keep upgradeability.** Subtensor changes chain-extension functions, call indices and staking semantics often. A contract we can't upgrade can strand funds.
- **No runtime changes.** Upstream changes take too long to reach mainnet. Use only chain-extension functions that are already live.

**Non-goals**

- Immutable contracts.
- Protecting against Subtensor itself (runtime upgrades, chain governance).
- Changing the pricing model (market price plus offset) or the fee model.

## Problems with the current design

### 1. Pooled custody forces an operator

| Operator job today | Why pooled custody needs it |
|---|---|
| Risk cron force-cancels listings on at-risk subnets (`risk_canceller`) | Liquidation TAO arrives as one lump for all listings |
| Owner opens stale-listing recovery pools and attests the TAO amount | Same |
| Owner records rotations (`update_hotkey_for_subnet`); keepers call `consolidate_listing_hotkey` and `sync_escrow_hotkey` | The contract must track which hotkey holds which part of the pool |
| Reserved Alpha per netuid, hotkey and generation; new listings blocked on re-registered netuids | Stops one listing's stake from being used for another |
| Owner calls `claim_dividends` (otc) | Emissions accrue to the pool |

`otc` has no deregistration handling at all. Liquidation TAO lands in the same balance as TAO-offer deposits and can't be withdrawn.

### 2. Owner keys can reach user funds with no notice

- Sellers give `otc` and `lockup_listings` a `Transfer` proxy, which can move all their TAO and all their stake. Both contracts can be replaced instantly with `set_code`. A stolen owner key can drain every seller who didn't remove the proxy.
- `update_escrow_code_hash` chooses the code that receives buyers' Alpha, effective immediately.
- `pause_fully` blocks `cancel_lockup_listing`, `cancel_alpha_listing` and `cancel_tao_offer`, so the owner can freeze funds indefinitely.

### 3. The chain surface is wide and fragile

- `call_runtime` hardcodes pallet and call indices (`SubtensorModule` 7 / `transfer_stake` 86, `Proxy` 16 / `proxy` 0). If Subtensor renumbers them, listing creation and TAO-offer fills break.
- PR #6 adds functions 34 (subnet registration state) and 36 (stake availability). Both are live on mainnet, but every extra function is one more thing a Subtensor change can break. If the runtime drops a function, calling it traps the contract rather than returning an error.
- Escrows have no upgrade path. If Subtensor changes or removes a function an escrow depends on, that escrow can never pay out.

### 4. PR #6 escrow recovery can lose the TAO

This bug is in PR #6, which will not ship. The escrow v2 design below avoids it.

`recover_tao_after_deregistration` terminates the escrow as soon as the subnet reports `exists == false`. In Subtensor, `do_dissolve_network` clears that flag immediately. The liquidation TAO is only credited later, by the `on_idle` cleanup (`destroy_alpha_in_out_stakes_settle_stakes`), which can take many blocks.

In that window, anyone can send 1 rao to an expired escrow and call recover. The escrow terminates with the dust, and the real TAO later lands on a dead address.

### 5. Spot-price manipulation

Function 15 returns the pool's spot price (`current_alpha_price`). In one `utility.batch_all`, a taker can sell Alpha to push the price down, take a listing, and buy the Alpha back. Listings have no price floor to stop this.

## Design

### Components

```text
code_registry       immutable   timelocked allowlist of code hashes; no chain calls
lockup_listings v2  upgradeable creates vaults; config, pause, listing index; holds no Alpha
otc v2              upgradeable TAO offers (pooled TAO); spot listings via vaults
listing_vault       per listing holds the seller's Alpha; settles itself; upgrades itself
alpha_lockup v2     per purchase holds the buyer's Alpha until unlock; upgrades itself
```

**Invariant.** A position contract only ever sends Alpha or TAO to one of three places:

- its owner (the seller or the beneficiary);
- a buyer who paid in the same call;
- an escrow it creates in the same call.

### Listing vault

State:

```rust
struct ListingVault {
    factory: AccountId,
    seller: AccountId,
    netuid: NetUid,
    hotkey: AccountId,                 // where the stake sits; changed only by resync/consolidate
    total: AlphaAmount,
    remaining: AlphaAmount,
    price_offset_bps: i32,
    min_price: u64,                    // new: seller's floor, TAO per Alpha * 1e9
    fee_rate: FixedDecimal,            // snapshot at creation
    lockup_duration: Option<BlockAge>, // None = spot listing
    status: Status,                    // Pending | Open | Closed
    upgrade_policy: UpgradePolicy,     // Auto | ConsentOnly
    version: u16,
}
```

Messages:

| Message | Caller | Effect |
|---|---|---|
| `activate()` | seller | Requires the vault's stake on `hotkey` to be within the transfer tolerance of the requested amount, and records the measured stake as `total`, since transfers can round down by a rao or two. Best-effort `move_stake` to the factory's validator hotkey. Status becomes Open. |
| `take(amount, recipient)` payable | anyone | Price = spot × (1 + offset); rejected if below `min_price`. Pays the seller and the fee recipient and refunds any excess. Spot listings: `transfer_stake` to `recipient`. Lockup listings: creates an escrow for `recipient` from the registry's current escrow code and transfers into it. Blocked while the factory's trading pause is active. |
| `cancel()` | seller | Status becomes Closed. Transfers all stake, including emissions, to the seller if possible, and sweeps TAO. Never pausable. The listing closes even if the transfer fails (for example, the subnet is dissolving). |
| `sweep_tao()` | anyone | Sends the whole TAO balance to the seller. Never terminates the vault. |
| `sweep_stake()` | anyone | Sends stake above `remaining` to the seller, when it's at least the chain minimum. Once the listing is closed, `remaining` is 0, so this returns everything left. |
| `resync_hotkey(hk)` | anyone | Re-points `hotkey` to `hk` if the vault holds more stake on `hk` than on the recorded hotkey. Moves nothing. |
| `consolidate()` | anyone | Optional. Moves stake to the factory's current validator hotkey. |
| `upgrade(hash)`, `set_upgrade_policy(p)` | see [Upgrades](#upgrades) | |

**Termination.** A vault terminates only in the call that successfully transferred out its last stake: a `cancel`, the final fill, or a `sweep_stake` after closing. A successful transfer proves the subnet was alive, so no liquidation TAO can still be on its way. The TAO sweep never terminates (see [Deregistration](#deregistration)).

**Funding without a proxy:**

1. The seller calls `factory.create_listing(netuid, hotkey, amount, offset, min_price, lockup)`. The factory checks the parameters against its config and creates a Pending vault. The salt is derived from the seller and a per-seller nonce, so the address is predictable. The seller pays the storage deposit.
2. The seller submits `utility.batch_all([subtensor.transfer_stake(vault, hotkey, netuid, netuid, amount), contracts.call(vault.activate())])`.

Steps 1 and 2 can also go in a single `batch_all` using the predicted address. `batch_all` reverts everything if activation fails, so stake can't land on the wrong address. An unfunded Pending vault is harmless, and the seller can close it.

Listings then use no proxy and no `call_runtime` at all.

### Escrow v2 (`alpha_lockup`)

Changes from today:

- Drop the `factory` field and `sync_hotkey_from_parent`. Add a permissionless `resync_hotkey(hk)` with the same rule as the vault.
- Replace `recover_tao_after_deregistration` with `sweep_tao()`. Anyone can call it at any time. It sends the whole TAO balance to the beneficiary, never terminates, and never sets `claimed`. It is blocked while a beneficiary transfer is pending, as today.
- Drop the subnet-generation check and the availability pre-check from `claim`. Isolated custody makes both unnecessary (see [Deregistration](#deregistration) and [Conviction locks](#conviction-locks)).
- Add `upgrade(hash)`, and `set_upgrade_policy(p)` controlled by the beneficiary.

Unchanged:

- Anyone can claim after unlock, and the Alpha goes to the beneficiary.
- Beneficiary changes are two-step.
- A pending beneficiary transfer blocks payouts.

### TAO offers (otc)

TAO isn't tied to a subnet, so offers stay pooled in `otc`. Changes:

- `cancel_tao_offer` can never be paused.
- Offers get a `max_price`, and `take_tao_offer` gets a `max_alpha`.
- Fills keep the proxy flow, but frontends wrap it so the proxy exists only inside the seller's own transaction:

  ```text
  utility.batch_all([
    proxy.add_proxy(otc, Transfer, 0),
    contracts.call(otc.take_tao_offer(..)),
    proxy.remove_proxy(otc, Transfer, 0),
  ])
  ```

  This leaves one `call_runtime` path, which an upgrade can fix if Subtensor renumbers calls. A proxy-free alternative is listed under [Open questions](#open-questions).

### Spot listings (otc)

Spot listings move to `listing_vault` with `lockup_duration = None`. `claim_dividends` goes away, because emissions on listed Alpha belong to the seller.

### Factories

`lockup_listings` v2 and the listing half of `otc` v2:

- create vaults from the registry's current vault code;
- keep the `(netuid, seller, listing_id) → vault` index and emit events for indexers;
- hold config that vaults read: the validator hotkey per netuid, the fee recipient, and the trading pause;
- copy the fee rate and limits into each vault at creation.

Factories hold no Alpha. A compromised factory could block takes or redirect future fees, but it can't touch principal: `cancel` and the sweeps never consult the factory.

### Hotkeys

- **Chain hotkey swap.** `swap_hotkey` moves the stake to the new hotkey. Anyone then calls `resync_hotkey(new)` on affected vaults and escrows. The call is accepted only when the position holds more stake on the new hotkey than on the recorded one. A position only ever points at its own stake, so no lineage check or owner registry is needed. Pointing a position at the wrong hotkey would mean donating more stake than it already holds, and that donation belongs to the position's owner.
- **Company changes validator without a chain swap.** Update the factory's validator hotkey. New listings use it, and open listings follow through the optional `consolidate()`. Escrows keep their hotkey; a chain swap covers them.

### Deregistration

Facts from Subtensor (`do_dissolve_network`, `destroy_alpha_in_out_stakes_settle_stakes`):

- The subnet stops existing immediately. Liquidation happens later, during `on_idle` cleanup, possibly many blocks later.
- Each staker's pro-rata TAO is credited straight to its coldkey's free balance.
- A netuid can only be reused after cleanup finishes.

With one coldkey per position, the TAO a vault or escrow receives is exactly its own. So:

- `sweep_tao()` needs no subnet-state query: any TAO a position holds belongs to its owner.
- It never terminates, so late or split credits are never sent to a dead address.
- Takes fail naturally once the stake is gone, and `cancel` closes the listing even if its transfer fails.
- Re-registered netuids don't matter, because a position only reads its own stake. There's no generation tracking and there are no reserved-Alpha maps, so new listings are never blocked.
- No pre-emptive cancellation is needed. The seller ends up with the liquidation TAO, the same outcome as holding the stake directly.

### Conviction locks

In the current runtime, a same-subnet `transfer_stake` moves any locked portion with the stake. But accounts reject incoming locked Alpha unless they opt in (`AccountFlags`, `ensure_can_receive_locked_alpha`), and contracts can neither opt in nor create locks. So a vault or escrow never holds locked Alpha, and a buyer never receives it from one. The function 36 pre-checks are therefore unnecessary.

If Subtensor later lets locked Alpha into accounts that haven't opted in, add a vault-side availability check using function 36 in an upgrade.

### Pricing guards

- Listings: a seller floor, `min_price`. Buyers are already capped by the TAO they send.
- Offers: a buyer ceiling, `max_price`, and a seller cap, `take_tao_offer(max_alpha)`.

These limit spot-price manipulation without needing a moving-average price from the runtime.

### Upgrades

#### Code registry (immutable)

This is the one immutable contract. It makes no chain-extension or runtime calls, so Subtensor changes can't break it. It tracks code per kind: `ListingVault`, `Escrow`, `LockupFactory`, `Otc`.

| Message | Caller | Effect |
|---|---|---|
| `propose(kind, hash)` | owner | Starts `UPGRADE_DELAY` |
| `veto(kind, hash)` | guardian | Cancels a pending proposal |
| `activate(kind, hash)` | anyone | Once the proposal matured unvetoed, makes it the kind's current code |
| `revoke(kind, hash)` | owner or guardian | Stops further adoption |

Changes to the owner and guardian roles go through the same delay, and the guardian can veto them.

Suggested values, assuming 12 s blocks: `UPGRADE_DELAY` = 3 days (21,600 blocks) and `FORCE_DELAY` = 7 days (50,400 blocks). They are constants in the registry.

#### Singletons (factories, otc)

`upgrade(hash)` can be called by the owner, and only with the registry's current code for that kind. The new code runs `migrate()` on its first call.

Users are protected by:

- the delay, so everyone sees an upgrade coming;
- exits that are never pausable (cancel listing, cancel offer);
- the guardian veto.

#### Positions (vaults, escrows)

Each position upgrades itself with `set_code_hash`, only to the registry's current code for its kind:

- The position owner can adopt it at any time.
- Anyone can apply it once it has been current for `FORCE_DELAY`, unless the owner set `ConsentOnly`.

Sellers can also just cancel during the delay. Escrow beneficiaries can't exit before unlock, so `ConsentOnly` is their protection against a bad upgrade.

Frontends check the position's code hash against the registry before a buyer pays.

#### When Subtensor ships a breaking change

1. The owner pauses new trading on the factories. A pause only restricts, so it needs no delay. Exits that still work keep working.
2. Build and test against the new runtime; Subtensor ships to testnet first. Propose the fix to the registry. If the change is additive (new functions before old ones are removed), the upgrade can land before mainnet breaks.
3. After `UPGRADE_DELAY`, upgrade the factories. Position owners upgrade on their next interaction; frontends batch `[upgrade, action]`. After `FORCE_DELAY`, anyone can migrate the remaining positions.

During the delay funds are illiquid but never at risk. A shorter delay means less downtime but less time to react to a malicious proposal.

#### Storage layout

Every upgradeable contract carries a `version` field and a `migrate()` path. CI decodes storage written by the previous version with the new code. This replaces PR #6's warning against running `set_code` over the old layout.

### What stays privileged

| Power | Holder | Delay | User's escape |
|---|---|---|---|
| Upgrade code | owner, via registry | 3 days, guardian veto | Cancel listings and offers; escrows set `ConsentOnly` |
| Force-upgrade positions | anyone | 7 more days | `ConsentOnly` |
| Fee rate for new listings (capped by `MAX_FEE`) | owner | none | Affects new listings only |
| Minimum amounts, lockup bounds, validator hotkey | owner | none | New listings only; `consolidate()` is optional |
| Pause new listings and takes | owner | none | Expires unless renewed; never blocks exits |
| Freeze new listings on a netuid (otc) | owner | none | Restrict-only |

Removed:

- `set_code` without a delay
- `update_escrow_code_hash`
- `force_cancel_*` and `risk_canceller`
- stale-listing recovery pools
- `claim_dividends`
- `update_hotkey_for_subnet`

### Chain dependencies

| Dependency | PR #6 | This design |
|---|---|---|
| Function 0 `get_stake_info` | yes | yes |
| Function 5 `move_stake` | consolidation | consolidation (best effort) |
| Function 6 `transfer_stake` | yes | yes |
| Function 15 `get_alpha_price` | yes | yes |
| Function 34 subnet registration state | generation checks | not used |
| Function 36 stake availability | pre-checks | not used |
| `call_runtime` proxy plus `transfer_stake` | listing creation, TAO-offer fills | TAO-offer fills only |

All Subtensor-specific encoding lives in `otc-shared`, so a Subtensor change means one code change plus one upgrade per kind.

`get_stake_info` returns the whole `StakeInfo`, but we only need `stake`. Decode a prefix struct (`hotkey`, `coldkey`, `netuid`, `stake`) and add a test showing trailing fields are ignored, so fields Subtensor appends later don't break decoding.

## What gets deleted

- `risk_canceller`, `set_risk_canceller`, `clear_risk_canceller`, `force_cancel_lockup_listing`, `force_cancel_alpha_listing`
- Stale-listing recovery pools and `reserved_recovery_tao`
- `reserved_alpha`, `reserved_alpha_by_hotkey`, `reserved_alpha_by_generation`
- `active_hotkeys`, `update_hotkey_for_subnet`, `consolidate_listing_hotkey`, `sync_escrow_hotkey`, `sync_hotkey_from_parent`
- Subnet-generation tracking and the function 34 and 36 wrappers
- `claim_dividends`
- `docs/backend-subnet-risk-runbook.md` and the backend cron

## Trade-offs

- **Revenue.** `otc` loses `claim_dividends`: emissions on listed Alpha go to the seller. The company still earns validator take while vault stake is delegated to its hotkey.
- **Cost.** Each listing pays a storage deposit (refunded when the vault closes) and a little more gas.
- **Deregistration outcome.** Sellers receive liquidation TAO rather than their Alpha back.
- **Trust model.** Governance is delayed, vetoable and escapable, not immutable. Liveness needs no operator.
- **Emergency downtime.** A Subtensor break can leave positions illiquid for up to `UPGRADE_DELAY`.

## Relationship to PR #6

Nothing from PR #6 is deployed, and it won't be merged. Its reusable parts land in the foundation PR below: the localnet runner and test helpers, the shared stake-verification helpers, and the escrow's permissionless claim and two-step beneficiary change. The rest (generation tracking, recovery pools, risk canceller, hotkey registry, reservations per hotkey and per generation) is superseded by this design.

## Rollout

The work ships as a series of PRs into an integration branch, `feat/permissionless-positions`. That branch merges to `main` once the series is complete, and new contracts are deployed from it. Nothing needs migrating, because nothing from PR #6 is deployed.

1. **Foundation:** reusable parts of PR #6, plus this doc.
2. **Upgrades:** `code_registry`, versioned storage and upgrade helpers.
3. **Escrow v2.**
4. **Listing vault and `lockup_listings` v2.**
5. **`otc` v2.**
6. **Localnet tests and docs,** including deleting the risk runbook.

Scope, tests and acceptance criteria for each PR are in the [implementation plan](permissionless-positions-plan.md).

## Testing

- **Unit tests** with the existing chain-extension mock: every message and error path, the invariant that funds only reach the owner, a paying buyer or a new escrow, and the upgrade rules (delays, veto, consent versus forced, revocation).
- **Localnet integration:**
  - proxy-free funding via `batch_all`;
  - dissolving a subnet with open vaults and expired escrows, then sweeping;
  - dust sent during cleanup doesn't lose TAO;
  - `swap_hotkey` then `resync_hotkey`;
  - an upgrade across a storage-layout change;
  - locked Alpha rejected on funding.
- **Price manipulation:** a batch that moves the spot price and then takes is rejected by `min_price`.

## Decisions

| Question | Decision |
|---|---|
| Default upgrade policy for escrows | `Auto`, so anyone can migrate escrows after `FORCE_DELAY`. The beneficiary can switch to `ConsentOnly`. |
| TAO-offer fills | Keep the proxy, added and removed within the seller's transaction. A proxy-free fill vault can follow later. |
| Lost `claim_dividends` revenue | Accepted. Revisit with an explicit fee if needed. |
| `consolidate()` | Kept, so open listings can follow a validator change. |
| PR #6 | Not shipped. Go straight to v2. |

## Open questions

These are deployment settings and don't block implementation.

1. Who holds the guardian role? A multisig separate from the owner is recommended.
2. Delay values: are 3 days and 7 days right, given how much notice Subtensor usually gives?
