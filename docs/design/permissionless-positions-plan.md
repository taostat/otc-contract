# Implementation Plan: Permissionless Positions

| | |
|---|---|
| Status | Approved, in progress |
| Date | 2026-09-28 |
| Design | [permissionless-positions.md](permissionless-positions.md) |
| Integration branch | `feat/permissionless-positions` (from `main`) |

## How the work is delivered

Six PRs, each targeting `feat/permissionless-positions`. A PR branches from the previous one until that one merges. The integration branch merges to `main` when the series is complete, and new contracts are deployed from it.

Every PR must pass the following before review:

- `cargo fmt -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --workspace`
- `cargo contract build --release` for every changed contract, with the papi descriptors regenerated
- `npx tsc --noEmit` in `integration-tests`
- `npm run test:localnet` for the flows the PR touches

## Rules that apply to every PR

**Chain surface.** Contracts call only chain-extension functions 0 (`get_stake_info`), 5 (`move_stake`), 6 (`transfer_stake`) and 15 (`get_alpha_price`). The only `call_runtime` left is the proxied `transfer_stake` in `take_tao_offer`. Functions 34 and 36 are not used. All Subtensor encoding lives in `otc-shared`.

**Testing seam.** ink!'s off-chain test environment can't call other contracts, instantiate contracts, or run `set_code_hash` or `code_hash`. Every such call goes through a small private helper with a `#[cfg(test)]` stand-in, so unit tests cover the message logic. Localnet integration tests cover the real calls.

**Upgrade-safe storage.** The root storage struct holds only fields that never change shape. Everything else lives in a `Mapping` or `Lazy` with an explicit key. Each contract stores `version: u16`, and new code runs `migrate()` before doing anything else when the stored version is older.

**Measured amounts.** On the current runtime a stake transfer can deliver a rao or two less than requested, and moving the requested amount afterwards fails with `NotEnoughStakeToWithdraw`. Contracts record the stake they actually hold after each transfer and move measured amounts, never requested ones.

**Fund invariant.** A position contract only sends Alpha or TAO to its owner, to a buyer who paid in the same call, or to an escrow it creates in the same call. Each PR that touches a position includes a test for this.

**Decisions** (from the design doc):

- Escrows default to `Auto` upgrades; the beneficiary can switch to `ConsentOnly`.
- TAO-offer fills keep the proxy, added and removed within the seller's own transaction.
- The lost `claim_dividends` revenue is accepted.
- `consolidate()` is kept.

## PR 1: Foundation

Branch `pp/01-foundation`, from `main`. Brings the reusable parts of PR #6 onto `main`'s contracts, without the pooled-custody mitigations.

**In scope**

- `docs/design/`: the design doc and this plan.
- `otc-shared`: `stake.rs` (`TRANSFER_TOLERANCE`, `stake_delta_verified`). All three contracts use it, and trap when a completed transfer fails verification.
- `alpha_lockup`, ported from PR #6:
  - permissionless `claim` after unlock, paying the beneficiary;
  - two-step beneficiary change (`propose_beneficiary`, `accept_beneficiary`, `cancel_beneficiary_proposal`);
  - `BeneficiaryTransferPending` blocks payouts while a change is pending;
  - `get_beneficiary`, `get_pending_beneficiary`.

  The constructor stays as on `main`.
- `integration-tests`:
  - the localnet runner and its npm scripts;
  - fresh random accounts per run, topped up with sudo;
  - random deployment salts;
  - the deployment cache made opt-in;
  - targeted validator staking;
  - `test-helpers.ts`;
  - the escrow beneficiary tests from PR #6.
- README: the localnet runner section and the escrow interface.

**Not ported from PR #6:** subnet-generation tracking, functions 34 and 36, `recover_tao_after_deregistration`, `sync_hotkey_from_parent`, the escrow `factory` field, recovery pools, the risk canceller, the hotkey registry, reservations per hotkey or generation, and the risk runbook.

**Done when:** all checks pass, and the localnet suite passes for `main`'s flows plus the beneficiary flows.

## PR 2: Upgrade mechanics

New contract `contracts/code_registry`. It is the only immutable contract and makes no chain calls.

**Storage**

- `owner`, `guardian`
- `upgrade_delay`, `force_delay`: set in the constructor, with no setter
- `proposals: Mapping<(u16, Hash), Proposal { proposed_at, vetoed }>`
- `current: Mapping<u16, CurrentCode { hash, since }>`
- `revoked: Mapping<(u16, Hash), ()>`
- `pending_role_change: Option<RoleChange { role, account, proposed_at, vetoed }>`

Kinds are plain `u16` ids defined in `otc-shared` (`LISTING_VAULT = 1`, `ESCROW = 2`, `LOCKUP_FACTORY = 3`, `OTC = 4`), so adding a kind later needs no new registry.

**Messages**

| Message | Caller | Rule |
|---|---|---|
| `new(owner, guardian, upgrade_delay, force_delay, initial: Vec<(u16, Hash)>)` | deployer | Initial codes become current immediately |
| `propose(kind, hash)` | owner | Records `proposed_at`; errors if already proposed or revoked |
| `veto(kind, hash)` | guardian | Only while pending |
| `activate(kind, hash)` | anyone | After `upgrade_delay`, if not vetoed or revoked; sets `current = { hash, since: now }` |
| `revoke(kind, hash)` | owner or guardian | Blocks future adoption; clears `current` if it is that hash |
| `propose_role_change(role, account)` | owner | Starts `upgrade_delay` |
| `veto_role_change()` | guardian | While pending |
| `execute_role_change()` | anyone | After the delay, if not vetoed |

**Views:** `current(kind)`, `is_current(kind, hash)`, `force_target(kind)` (the current hash once `since + force_delay` has passed), `proposal(kind, hash)`, `delays()`, `owner()`, `guardian()`.

**`otc-shared/src/upgrade.rs`**

- `UpgradePolicy { Auto, ConsentOnly }`
- A pure rule function:
  ```rust
  fn check_upgrade(
      caller_is_owner: bool,
      policy: UpgradePolicy,
      requested: Hash,
      current: Option<Hash>,
      force_target: Option<Hash>,
  ) -> Result<(), UpgradeError>
  ```
  The position owner may adopt `current`. Anyone may apply `force_target` when the policy is `Auto`.
- The storage-version helper pattern.

**Tests:** unit tests for every registry transition, both delays, veto, revoke and role changes; table tests for `check_upgrade`.

## PR 3: Escrow v2

**Storage:** `beneficiary`, `pending_beneficiary`, `registry`, `netuid`, `alpha_amount`, `unlock_block`, `hotkey`, `claimed`, `upgrade_policy`, `version`.

**Constructor:** `new(beneficiary, netuid, alpha_amount, unlock_block, hotkey, registry)`.

**Messages**

| Message | Caller | Rule |
|---|---|---|
| `claim()` | anyone | After unlock; not claimed; no pending beneficiary; stake > 0. Transfers all stake on `hotkey` to the beneficiary; traps if more than the tolerance remains; terminates, sending any TAO to the beneficiary. |
| `sweep_tao()` | anyone | No pending beneficiary; balance > 0. Sends the whole balance to the beneficiary. Never terminates and never sets `claimed`. |
| `resync_hotkey(hk)` | anyone | `hk != hotkey` and stake on `hk` is greater than stake on `hotkey` (a missing stake entry counts as 0). |
| `propose_beneficiary`, `accept_beneficiary`, `cancel_beneficiary_proposal` | as in PR 1 | |
| `upgrade(hash)` | beneficiary or anyone | Checked with `check_upgrade` against the registry's `ESCROW` kind, then `set_code_hash` |
| `set_upgrade_policy(p)` | beneficiary | |
| views | anyone | Info, beneficiary, pending, hotkey, policy, version, TAO balance |

`lockup_listings` (still `main`'s pooled version until PR 4) gains a `registry` constructor argument and passes it to new escrows.

**Tests**

- Unit tests for each rule, using the chain-extension mock and the registry seam.
- Integration tests:
  - claim by a third party;
  - beneficiary change;
  - `sweep_tao` after TAO is sent to the escrow;
  - `swap_hotkey`, then `resync_hotkey`, then claim;
  - an upgrade by the beneficiary, and a forced upgrade after `force_delay` (short delays on localnet).

## PR 4: Listing vault and `lockup_listings` v2

### `contracts/listing_vault`

**Constructor:** `new(seller, netuid, hotkey, amount, price_offset_bps, min_price, fee_rate, min_purchase, lockup_duration: Option<u32>, cancellable_after: BlockNumber, registry)`. `factory` is the caller.

**Factory interface.** An ink! trait in `otc-shared`, `VaultFactory`, with `trading_paused()`, `fee_recipient()`, `validator_hotkey(netuid)` and `listing_closed(listing_id)`. Both factories implement it. The vault calls `listing_closed` best-effort and ignores failures.

**Messages**

| Message | Caller | Rule |
|---|---|---|
| `activate()` | seller | Pending only. Stake on `hotkey` must be within the transfer tolerance of the requested amount; `total` and `remaining` record the measured stake. Best-effort `move_stake` of that amount to `validator_hotkey(netuid)`, re-measured afterwards. Status becomes Open. |
| `take(amount, recipient)` payable | anyone | Open; factory not paused; `min_purchase <= amount <= remaining`; the remainder is 0 or at least `BITTENSOR_MIN_STAKE`; price = spot × (10000 + offset) / 10000, rejected below `min_price`; value must cover price × amount plus the fee. Spot listings transfer to `recipient`. Lockup listings instantiate an escrow from the registry's current `ESCROW` code (salt = vault address + purchase counter) and transfer into it. Pays the seller and the fee recipient and refunds the excess. On the final fill, status becomes Closed, and the vault terminates if its stake is 0 after the transfer. |
| `cancel()` | seller | Not Closed; `now >= cancellable_after`. Status becomes Closed. Tries to transfer all stake to the seller; if that fails the listing still closes. Terminates only if the transfer succeeded and the stake is 0. |
| `sweep_stake()` | anyone | Sends stake above `remaining` to the seller (all of it once Closed). Terminates when Closed and the stake reaches 0 in this call. |
| `sweep_tao()` | anyone | Sends the whole balance to the seller. Never terminates. |
| `resync_hotkey(hk)` | anyone | Same rule as the escrow. |
| `consolidate()` | anyone | Open; `validator_hotkey(netuid) != hotkey`; moves all stake there with verified deltas. |
| `upgrade(hash)`, `set_upgrade_policy(p)` | seller or anyone | As for the escrow, against the `LISTING_VAULT` kind |

**Events:** `Activated`, `Taken { buyer, recipient, amount, price, tao, fee, escrow }`, `Cancelled`, `Closed`, `StakeSwept`, `TaoSwept`, `HotkeyResynced`, `Consolidated`, `Upgraded`.

### `lockup_listings` v2 (rewrite)

**Storage**

- `owner`, `registry`, `version`
- `next_listing_id`, and a per-seller nonce used for vault salts
- `listings: Mapping<(netuid, seller, id), AccountId>` and `user_listings` (up to 25 per seller per netuid)
- config: `default_hotkey`, `validator_hotkeys: Mapping<netuid, AccountId>`, `fee_rate` (at most `MAX_FEE_RATE`), `fee_recipient`, `min_listing_amount`, `min_purchase_amount`, lockup bounds, `paused_until`

**Messages**

- `create_listing(netuid, hotkey, amount, offset, min_price, lockup_duration)`: checks the config and instantiates a Pending vault from the registry's current `LISTING_VAULT` code. The seller pays the storage deposit. Emits `ListingCreated { vault, .. }`.
- `listing_closed(id)`: callable only by that listing's vault; removes it from the index.
- Admin setters.
- `pause(blocks)`: at most `MAX_PAUSE`; renewable. `resume()`.
- `upgrade(hash)` by the owner, against `LOCKUP_FACTORY`, then `migrate()`.
- Views.

All of `main`'s pooled code goes: `reserved_alpha`, and take, cancel and escrow creation inside the factory.

**Tests**

- Unit tests: config validation, the fee cap, pause expiry, the index, and every vault rule, with mocked stake and price.
- Integration tests:
  - proxy-free funding: create, then `batch_all(transfer_stake, activate)`;
  - a partial lockup take, then claiming the escrow;
  - a spot-style take with a recipient;
  - cancel with emissions;
  - the price floor holding against a batch that moves the spot price;
  - locked Alpha rejected when funding.

## PR 5: `otc` v2

- Spot listings are created as vaults with `lockup_duration = None` and `cancellable_after = now + min_listing_age`. `otc` implements `VaultFactory`.
- TAO offers stay pooled:
  - `create_tao_offer(netuid, offset, max_price)` (payable);
  - `take_tao_offer(netuid, buyer, offer_id, hotkey, max_alpha)`, still proxied;
  - `cancel_tao_offer` works in every pause state.
- Freezing new listings on a netuid stays. Pausing gets an expiry.
- Removed: `list_alpha`, `take_alpha_listing`, `cancel_alpha_listing`, `force_cancel_alpha_listing`, `claim_dividends`, `reserved_alpha`, `update_hotkey` (replaced by validator hotkey config), and `set_code` (replaced by registry upgrades).
- Tests:
  - unit tests for the offer guards and the pause rules;
  - integration tests for spot vault flows, offers with the proxy added and removed in one `batch_all`, and cancel while paused.

## PR 6: Localnet tests and docs

- Deregistration on localnet:
  - `sudo(dissolve_network)` with open vaults and expired escrows;
  - `sweep_tao` pays the owners;
  - dust sent during cleanup doesn't lose TAO;
  - a re-registered netuid doesn't affect new listings.
- A full upgrade drill: propose, veto, activate, consent upgrade, forced upgrade after `force_delay`, and `ConsentOnly` blocking a forced upgrade. It includes a test-only vault v2 that adds a storage field and migrates.
- A deployment script: the registry first with the initial codes, then the factories.
- A README rewrite covering the frontend batching for vault funding and TAO-offer fills.
- Delete `docs/backend-subnet-risk-runbook.md`.

## Risks

| Risk | Mitigation |
|---|---|
| Cross-contract logic has no off-chain test coverage | Seams for unit tests; every real cross-contract path has a localnet test |
| Cost of a vault per listing | Measure the storage deposit and weights in PR 4 and report them in the PR |
| Frontend single-batch funding depends on predicting the vault address | Two-transaction flow first; address prediction added and tested in PR 6 |
| Subtensor changes during implementation | Every chain call stays in `otc-shared`; the localnet suite runs against the fork's `main` |
| A proxied `transfer_stake` pre-charges a full 256-entry `StakingHotkeys` walk (about 90e9 ref_time) before refunding, so contract calls that make it need a large gas limit | Vault funding is proxy-free. `take_tao_offer` keeps the proxy, so frontends must use the dry-run gas estimate rather than a fixed limit |
