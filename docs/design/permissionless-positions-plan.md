# Implementation Plan: Permissionless Positions

| | |
|---|---|
| Status | Approved; all six PRs built |
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

**Upgrade-safe storage.** The root storage struct holds only fields that never change shape. Everything else lives in a `Mapping` or `Lazy` with an explicit key. Each contract stores `version: u16`, and new code runs `migrate()` before doing anything else when the stored version is older. Code never migrates downwards, and older code ignores storage added after it, so rolling back after a storage change means shipping new code with a higher version that restores the old behaviour, never re-proposing the old code.

**Measured amounts.** On the current runtime a stake transfer can deliver a rao or two less than requested, and moving the requested amount afterwards fails with `NotEnoughStakeToWithdraw`. A recipient that already holds a position on the hotkey can also gain a rao more, so verification accepts deltas within `TRANSFER_TOLERANCE` either way. Contracts record the stake they actually hold after each transfer and move measured amounts, never requested ones.

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
  - test accounts topped up with sudo, since pallet-contracts requires the caller to hold the full storage deposit limit;
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
- `upgrade_delay`, `force_delay`: set in the constructor, with no setter, and at least `MIN_DELAY` (7,200 blocks) unless built with the test-only `short-delays` feature
- `proposals: Mapping<(u16, Hash), Proposal { proposed_at, sequence }>`; a veto removes the proposal, so a vetoed hash can be proposed again with a fresh delay
- `activated_sequence: Mapping<u16, u64>`: only a proposal newer than the kind's last activated one can activate, so stale proposals cannot roll the code back, even after a revocation
- `current: Mapping<u16, CurrentCode { hash, since }>`
- `revoked: Mapping<(u16, Hash), bool>`
- `pending_role_change: Option<RoleChange { role, account, proposed_at }>`; a veto clears it

Kinds are plain `u16` ids defined in `otc-shared` (`LISTING_VAULT = 1`, `ESCROW = 2`, `LOCKUP_FACTORY = 3`, `OTC = 4`), so adding a kind later needs no new registry.

**Messages**

| Message | Caller | Rule |
|---|---|---|
| `new(owner, guardian, upgrade_delay, force_delay, initial: Vec<(u16, Hash)>)` | deployer | Initial codes become current immediately; owner and guardian must differ; no duplicate kinds |
| `propose(kind, hash)` | owner | Records `proposed_at`; errors if already proposed or revoked |
| `veto(kind, hash)` | guardian | Only while pending |
| `activate(kind, hash)` | anyone | After `upgrade_delay`, if not vetoed, revoked or superseded by a newer activation; sets `current = { hash, since: now }` |
| `revoke(kind, hash)` | owner or guardian | Blocks future adoption; clears `current` if it is that hash |
| `propose_role_change(role, account)` | owner | Starts `upgrade_delay` and replaces any pending change; the new account must not hold the other role |
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
- The storage-version pattern lands in PR 3, with the first upgradeable contract, where a real migration can test it.

**Tests:** unit tests for every registry transition, both delays, veto, revoke and role changes; table tests for `check_upgrade`.

## PR 3: Escrow v2

**Storage:** `beneficiary`, `pending_beneficiary`, `registry`, `netuid`, `alpha_amount`, `unlock_block`, `hotkey`, `claimed`, `upgrade_policy`, `version`.

**Constructor:** `new(beneficiary, netuid, alpha_amount, unlock_block, hotkey, registry)`.

**Messages**

| Message | Caller | Rule |
|---|---|---|
| `claim()` | anyone | After unlock; no pending beneficiary; stake > 0. Transfers all stake on `hotkey` to the beneficiary; traps if the transfer fails verification. Never terminates, and can run again when more stake reaches `hotkey`, for example after `resync_hotkey`. Sets `claimed`. |
| `close()` | beneficiary | No pending beneficiary; no stake on `hotkey`. Terminates, sending the TAO balance and the storage deposit to the beneficiary. |
| `sweep_tao()` | anyone | No pending beneficiary; balance > 0. Sends the whole balance to the beneficiary. Never terminates and never sets `claimed`. |
| `resync_hotkey(hk)` | anyone | `hk != hotkey` and stake on `hk` is greater than stake on `hotkey` (a missing stake entry counts as 0). |
| `propose_beneficiary`, `accept_beneficiary`, `cancel_beneficiary_proposal` | as in PR 1 | |
| `upgrade(hash)` | beneficiary or anyone | Checked with `check_upgrade` against the registry's `ESCROW` kind, then `set_code_hash` |
| `set_upgrade_policy(p)` | beneficiary | |
| views | anyone | Info, beneficiary, pending, hotkey, policy, version, TAO balance |

`lockup_listings` (still `main`'s pooled version until PR 4) gains a `registry` constructor argument and passes it to new escrows.

`otc-shared` gains a `transfer_stake_verified` helper that wraps the read, transfer, re-read and verify steps. Escrow v2 and the vault use it, instead of repeating that sequence at every transfer as the pooled contracts do.

**Tests**

- Unit tests for each rule, using the chain-extension mock and the registry seam.
- Integration tests:
  - claim by a third party;
  - beneficiary change;
  - `sweep_tao` after TAO is sent to the escrow;
  - `swap_hotkey`, then `resync_hotkey`, then claim;
  - an upgrade by the beneficiary, and a forced upgrade after `force_delay` (short delays on localnet).

**As built**

- `claim` does not terminate, and the beneficiary closes the escrow with `close`. In the first version, claim terminated, which let anyone strand stake after a hotkey swap: donate a little on the old hotkey and claim before anyone resyncs. The design doc's termination rule now covers vaults too.
- The beneficiary field keeps its storage name `buyer`, and `get_buyer` stays next to `get_beneficiary`.
- The escrow's `Error` enum is flat. The upgrade and registry failures are their own variants (`NoCurrentCode`, `NotCurrentCode`, `ConsentRequired`, `ForceDelayNotElapsed`, `RegistryQueryFailed`, `CodeUpgradeFailed`) rather than a wrapped `UpgradeError`. A variant that carries data trips the `cast_possible_truncation` lint that `cargo contract build` enforces.
- Seams: `registry_code` reads `current` and `force_target` from the registry, `install_code` runs `set_code_hash`, and `transfer_tao` sends TAO, because the off-chain engine models balances as `u128` and the escrow's are `u64`.
- The chain-extension mock is shared, as `otc_shared::testing::MockSubtensor`, behind otc-shared's `testing` feature, which contracts enable from their dev-dependencies. It also covers `transfer_stake_verified`.
- The upgrade target on localnet is the escrow built with `--features upgrade-test`, which only bumps `STORAGE_VERSION` to 2. `npm run build:contracts` builds it into `target/upgrade-test/`. The escrow tests deploy their own registry, which offers it as the current escrow code.
- A forced upgrade can't succeed on localnet with a release registry, since `MIN_DELAY` is 7,200 blocks. The localnet tests check that a third party is refused with `ForceDelayNotElapsed` before the delay and with `ConsentRequired` under `ConsentOnly`; unit tests cover the forced path itself. The full forced drill moves to PR 6, which runs against a `short-delays` registry.
- Third-party claims and the beneficiary change were already covered by PR 1's integration tests, which keep running against escrow v2.

## PR 4: Listing vault and `lockup_listings` v2

### `contracts/listing_vault`

**Constructor:** `new(seller, netuid, hotkey, amount, price_offset_bps, min_price, fee_rate, min_purchase, lockup_duration: Option<u32>, cancellable_after: BlockNumber, registry)`. `factory` is the caller.

**Factory interface.** An ink! trait in `otc-shared`, `VaultFactory`, with `trading_paused()`, `fee_recipient()`, `validator_hotkey(netuid)` and `listing_closed(listing_id)`. The listing factory implements it (PR 5 made `lockup_listings` the only one). The vault calls `listing_closed` best-effort and ignores failures.

**Messages**

| Message | Caller | Rule |
|---|---|---|
| `activate()` | seller | Pending only. Stake on `hotkey` must be within the transfer tolerance of the requested amount; `total` and `remaining` record the measured stake. Best-effort `move_stake` of that amount to `validator_hotkey(netuid)`, re-measured afterwards. Status becomes Open. |
| `take(amount, recipient)` payable | anyone | Open; factory not paused; `min_purchase <= amount <= remaining`; the remainder is 0 or at least `BITTENSOR_MIN_STAKE`; price = spot × (10000 + offset) / 10000, rejected below `min_price`; value must cover price × amount plus the fee. Spot listings transfer to `recipient`. Lockup listings instantiate an escrow from the registry's current `ESCROW` code (salt = vault address + purchase counter) and transfer into it. Pays the seller and the fee recipient and refunds the excess. On the final fill, status becomes Closed. Never terminates. |
| `cancel()` | seller | Not Closed; `now >= cancellable_after`. Status becomes Closed. Tries to transfer all stake to the seller; if that fails the listing still closes. Never terminates. |
| `sweep_stake()` | anyone | Sends stake above `remaining` to the seller (all of it once Closed). Never terminates. |
| `close()` | seller | Closed. Sends any stake left on `hotkey` to the seller, leaving only dust below Subtensor's minimum transfer, then terminates, sending the TAO balance and the storage deposit to the seller. |
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

**As built**

- `otc-shared` gains the pieces both factories and the vault share: listing pricing (`quote`, `apply_price_offset`, `BITTENSOR_MIN_STAKE`), `move_stake_verified`, the `VaultFactory` trait, and the position events (`TaoSwept`, `HotkeyResynced`, `Upgraded`, `UpgradePolicyChanged`), which escrows now use too. ink! puts every linked contract's events in a contract's metadata, and two events with the same name and fields collide.
- `listing_closed()` takes no argument: the factory looks the caller up in a reverse index (`vault_listings`), so only a listing's own vault can close it. The vault calls it with bounded ref_time and proof size, so a hostile factory can't make a cancel or the final take run out of gas.
- Takes, sweeps and consolidation set `remaining` to the lower of what is left and what the vault measures, since rounding can take a rao more than was sent.
- `close` sends any remaining stake, such as rewards that accrued after the final take, to the seller before terminating. Only dust Subtensor refuses with `AmountTooLow` (its minimum transfer, measured in TAO) is left behind.
- The factory's `prune_listing` lets anyone drop a listing whose vault is closed or gone, in case the vault's best-effort report failed, so a stale entry can't hold one of the seller's 25 slots.
- `create_listing` returns `(listing_id, vault)`. Vault addresses are salted with the seller and a per-seller nonce (`get_seller_nonce`). The vault's constructor arguments are part of its address, so they carry nothing the seller can't predict: the vault has no listing id (the factory's index maps vaults to listings), and lockup listings pass `cancellable_after = 0` rather than the current block. A test of address prediction stays in PR 6.
- The factory constructor returns a `Result`, rejecting a fee above `MAX_FEE_RATE` (5%) or inverted lockup limits. `MAX_PAUSE` is 50,400 blocks. Ownership moves in two steps: `transfer_ownership`, then `accept_ownership`.
- `transfer_stake_verified` returns the sender's stake after the transfer, so callers don't query it again.
- Integration tests: the lockup factory only creates lockup listings, so "a spot-style take with a recipient" is covered as a lockup take for another recipient; spot vaults get their factory with `otc` v2 in PR 5. The pooled-era lockup suites are replaced by `factory.test.ts` and `listing.test.ts`.
- Vaults create escrows from the registry's current escrow code, so the escrow suite now deploys a `short-delays` registry and runs the upgrade through it: propose, activate, the beneficiary's upgrade, and a forced upgrade after `force_delay` that `ConsentOnly` blocks. The forced path no longer waits for PR 6.

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

**As built**

- **One listing factory.** `lockup_listings` creates spot listings too, rather than `otc` becoming a second factory with the same index, vault creation, `VaultFactory`, pruning, pause, ownership and fee settings. `create_listing` takes `lockup_duration: Option<u32>`; `None` is a spot listing. The factory also gained the subnet freeze (`set_subnet_frozen`) and `min_listing_age`, capped at `MAX_LISTING_AGE` because sellers don't choose it. A seller's 25-listing cap per subnet now counts spot and lockup listings together. `otc` v2 keeps only the pooled TAO offers.
- The vault's `cancellable_after` constructor argument became `cancel_delay`, counted from activation. Spot listings use `min_listing_age`, lockup listings 0. A block number in the constructor would make the vault's address unpredictable, and the delay now protects buyers from the moment the listing opens.
- `otc` v2: `create_tao_offer(netuid, offset, max_price)`, `take_tao_offer(netuid, buyer, offer_id, hotkey, max_alpha)` returning the Alpha moved, `quote_offer_fill`, `cancel_tao_offer` never paused, fees to `fee_recipient` capped at `MAX_FEE_RATE`, `pause(blocks)` with `MAX_PAUSE`, two-step ownership, and `upgrade` against the registry's `OTC` code. `MAX_FEE_RATE` and `MAX_PAUSE` moved to `otc-shared`, and `PauseState` is gone.
- Integration tests: `otc.test.ts` (configuration, owner controls, pause, upgrade) and `offers.test.ts` (offers, the proxied fill in one batch, the price guards, pause and cancel) replace the seven pooled-era otc suites. The listing suite gains a spot take, the minimum listing age and subnet freezes.

## PR 6: Localnet tests and docs

- Deregistration on localnet:
  - `sudo(dissolve_network)` with open vaults and expired escrows;
  - `sweep_tao` pays the owners;
  - dust sent during cleanup doesn't lose TAO;
  - a re-registered netuid doesn't affect new listings.
- A full upgrade drill. The escrow suite already covers propose, activate, a consent upgrade, a forced upgrade after `force_delay` and `ConsentOnly` blocking it (PR 4). PR 6 adds a veto, and a test-only vault v2 that adds a storage field and migrates.
- A deployment script: the registry first with the initial codes, then the factories.
- A README rewrite covering the frontend batching for vault funding and TAO-offer fills.
- Delete `docs/backend-subnet-risk-runbook.md`.

**As built**

- **Deregistration** (`deregistration.test.ts`):
  - setup: an open lockup listing with a purchase in an expired escrow, plus an open spot listing;
  - Sudo's `root_dissolve_network` stops takes at once;
  - once Subtensor's cleanup has left the queue, `sweep_tao` pays each vault's liquidation TAO to the seller and the escrow's to its beneficiary;
  - `claim` fails with `NoAlphaToClaim`, the beneficiary closes the escrow, and the seller cancels and closes both vaults, which leaves the index empty;
  - a new subnet then reuses the netuid, and a listing on it trades as usual.
- **Dust during cleanup.** On localnet, a small subnet's cleanup finishes within a block or two of the dissolution, so the suite can't land a transfer mid-cleanup. A first attempt swept the liquidation together with the dust. What keeps late TAO safe is that sweeps never terminate a contract and only the owner can close one. The listing suite already covers a sweep that leaves the vault open.
- **Upgrade drill** (`vault-upgrade.test.ts`):
  - the guardian vetoes a vault code proposal, and the owner later proposes the same code again;
  - new listings get the new code at their predicted address;
  - the seller upgrades at once, and storage migrates on the next state change, a take;
  - a third party forces the upgrade after `force_delay`, and `ConsentOnly` refuses that while the seller can still upgrade.

  The test-only vault v2 (`--features upgrade-test`) adds a `Lazy` field under a fixed key, which its migration sets, and a getter for it. The field's key type is declared in every build, because ink!'s storage macro names field types without their `cfg`. The release metadata is unchanged.
- **Deployment script.** `integration-tests/src/deploy.ts` has `deployAll` and `verifyDeployment`, and `scripts/deploy.ts` is its CLI: a JSON config, the deployer secret in `DEPLOYER_SURI`, and the deployment printed as JSON. Every constructor is dry-run before anything is uploaded, so a rejected config changes nothing on chain. Verification checks the registry's codes, and that both contracts answer to it and run its code. The test setup deploys through the same code. `deploy.test.ts` runs the CLI against localnet with `scripts/deploy.localnet.json`, and checks that a rejected config leaves the chain untouched.
- **Address prediction.** `integration-tests/src/vault-address.ts` predicts the factory's next vault for a seller. The test helpers open every listing with one `batch_all([create_listing, transfer_stake, activate])`, and a test shows that a stale prediction reverts the whole batch.
- `registerSubnet` in the tests reads the netuid from `NetworkAdded`, since Subtensor reuses the lowest free netuid.
- `docs/backend-subnet-risk-runbook.md` doesn't exist on the integration branch, so nothing is deleted. The README's subnet dissolution section replaces it.

## Follow-up: Subtensor upstream to spec 482

The fork moved from `c004cebf3` (spec 471) to `25b3866e1` (spec 482): 194 commits. Reviewed for anything the contracts depend on. The full localnet suite passes on a node built from it: 38 tests across 8 files, with the devnet descriptors still generated from spec 471.

**Unchanged for the contracts**

- Chain-extension functions 0, 5, 6 and 15 keep their ids and encodings. Stake exits now charge the `StakingHotkeys` walk for the caller's own list rather than the 256-entry worst case, so contract calls that move stake cost less gas.
- The proxied fill's runtime call still encodes correctly: `SubtensorModule` (7), `Proxy` (16), `transfer_stake` (86), and `ProxyType::Transfer` (10). The new proxy types `Validate` and `Weights` are appended at 19 and 20, and `Transfer` still allows `transfer_stake`. The dispatch-side walk bound is unchanged, so the fill's gas note stands.
- No change to staking rate limits, transfer minimums or the dissolution settlement.
- The new `StakeMoveCooldownUntil` only gates the next owner-driven hotkey swap; delegator withdrawals ignore it.
- New pallet errors reach the contracts as the chain extension's generic `RuntimeError`, which they already treat as a refused transfer.

**Changes that reach the contracts**

1. **Recipient opt-out, `set_reject_staking_hotkey_growth`.** A third-party `transfer_stake` now fails when the recipient doesn't already stake on the hotkey and has either set this flag or holds 128 hotkeys (the cap is not new). Contracts receive stake only into fresh coldkeys, at funding and escrow creation, so their own receipts are unaffected. Payouts to users are affected: a spot take's recipient, an escrow claim's beneficiary, a cancel or sweep to the seller, and an offer fill's buyer. None of this loses funds. A take or fill reverts, a cancel still closes the listing and leaves the stake to sweep, and a claim waits. The user clears it by unsetting the flag.
2. **Coldkey-swap destination reservations.** Anyone can announce a V2 swap naming any account as its destination, paying the announcement fee. Until the announcement expires, every cross-coldkey stake transfer into that account fails with `ColdkeySwapDestinationReserved`. The reserved account can release the reservation itself, but contracts can't.
   - A known or predicted vault address can be reserved to block its funding. The seller relists under a new nonce, and the Pending vault can be cancelled and closed. Each announcer can hold one reservation, so this costs one fee per blocked listing.
   - Escrow addresses depend on the take's block, so reserving one ahead of time is impractical.
   - Users who are reserved as payout recipients release the reservation themselves.
3. **Swaps executed into a contract.** No delegation authority moves: proxy grants stay with the old coldkey. Locks move only to a destination that accepts locked Alpha, and a vault or escrow holding stake keeps its default refusal. Everything else is a donation: stake on the contract's own position can be swept to its owner, and positions elsewhere are stranded at the sender's cost. One edge case: a drained, unclosed vault or escrow whose staking list has been cleared counts as unused. It then gets the temporary accept-locked-Alpha override and could receive locked Alpha. Sweeps and `close` to an owner who refuses locked Alpha would then fail until the owner accepts it. The owner's own funds are never at risk.
4. **Users who swap coldkeys.** While a swap is announced, the account can only make swap calls, and nothing at all if the swap is disputed. A root reset can now keep it locked that way. So a seller can't cancel and a buyer can't cancel an offer until the swap completes. Escrows follow the user through the two-step beneficiary handoff. Vaults and TAO offers stay bound to the old key, so sellers and buyers should cancel before swapping.
5. **Hotkey swaps.** A third-party stake now follows the new hotkey only if its coldkey could accept a new hotkey, and otherwise stays on the old one. Contracts always can, so `resync_hotkey` works as before.

**As built (PR 7)**

- The devnet descriptors are regenerated from the spec 482 node. With them, the full localnet suite passes on that node: 40 tests across 9 files.
- `recipient-policies.test.ts` covers both behaviours on spec 482:
  - a buyer, beneficiary and seller with the growth opt-out: a spot take and a claim fail, a cancel closes the listing with the stake held back, and everything pays out once they clear the flag;
  - a vault address reserved as a coldkey-swap destination: the listing batch reverts with `ColdkeySwapDestinationReserved`. The seller uses up the nonce by creating, cancelling and closing that vault, and lists at the next address.
- The README's "Recipient policies and coldkey swaps" section covers the opt-outs, reserved destinations, and cancelling before a coldkey swap.

**Decision: no seller or buyer handoff for vaults and TAO offers.** Escrows keep their beneficiary handoff; vaults and offers stay bound to the account that opened them.

- **Nothing gets stuck without it.** A lockup listing or an offer can be cancelled at any time, and a spot listing once it has been open for `min_listing_age`, which is capped at about a week. A user who swaps coldkeys cancels first, or cancels afterwards from the old key once it has TAO for fees, and opens the position again from the new key. Escrows are different: their Alpha stays locked for the whole lockup, which can be months, so they need the handoff.
- **It wouldn't protect a compromised key.** Whoever holds the old key can call a handoff as easily as the owner.
- **It isn't cheap.** The factory indexes listings by seller and caps them per seller, so changing a vault's seller means re-keying that index through a new `VaultFactory` call, plus a vault upgrade that migrates storage. `otc` keys offers by buyer the same way.

## Risks

| Risk | Mitigation |
|---|---|
| Cross-contract logic has no off-chain test coverage | Seams for unit tests; every real cross-contract path has a localnet test |
| Cost of a vault per listing | Measure the storage deposit and weights in PR 4 and report them in the PR |
| Frontend single-batch funding depends on predicting the vault address | Two-transaction flow first; address prediction added in PR 6, and every listing test now opens its listing in one batch |
| Subtensor changes during implementation | Every chain call stays in `otc-shared`; the localnet suite runs against the fork's `main` |
| A proxied `transfer_stake` pre-charges a full 256-entry `StakingHotkeys` walk (about 90e9 ref_time) before refunding, so contract calls that make it need a large gas limit | Vault funding is proxy-free. `take_tao_offer` keeps the proxy, and a dry run made before the proxy exists can't measure it, so frontends set a limit that covers the pre-charge (the tests use 250e9 ref_time); unused gas is refunded |
