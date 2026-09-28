# Taostats OTC Smart Contracts

ink! smart contracts for Bittensor Alpha and TAO over-the-counter markets.

The repository currently contains three deployable contracts:

| Contract | Purpose |
| --- | --- |
| `otc_contract` | Spot Alpha listings and TAO offers with market-relative pricing, fee collection, subnet listing freezes, pause controls, and owner-controlled upgrades. |
| `lockup_listings` | Alpha listings that sell into per-purchase lockup escrows. Supports partial fills, configurable lockup duration bounds, fee collection, pause controls, and owner-controlled upgrades. |
| `alpha_lockup` | Per-purchase escrow contract instantiated by `lockup_listings`. It holds purchased Alpha until `unlock_block`, then lets the buyer claim the Alpha plus any staking rewards. |

Shared Bittensor environment types, runtime calls, chain extensions, and fixed-point helpers live in `shared`.

## Repository Layout

```text
.
|-- contracts/
|   |-- alpha_lockup/        # Escrow contract used by lockup listings
|   |-- lockup_listings/     # Lockup listing marketplace
|   `-- otc/                 # Spot OTC marketplace
|-- integration-tests/       # TypeScript integration tests for a local contracts node
|-- shared/                  # Shared ink! environment, runtime types, and helpers
`-- Cargo.toml               # Rust workspace
```

## Prerequisites

- Rust from `rust-toolchain.toml` (`1.89`) with `rust-src`, `rustfmt`, `clippy`, and `wasm32-unknown-unknown`.
- `cargo-contract` 5.x.
- Node.js 22 LTS for the TypeScript integration tests.
- A Bittensor/Subtensor contracts development node with the expected chain extension methods.

Install the Rust contract tool if needed:

```bash
cargo install --force --locked cargo-contract
```

## Build And Check

```bash
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test

cargo contract build --release --manifest-path contracts/alpha_lockup/Cargo.toml
cargo contract build --release --manifest-path contracts/otc/Cargo.toml
cargo contract build --release --manifest-path contracts/lockup_listings/Cargo.toml
```

Contract artifacts are written under `target/ink/<contract_name>/`.

The workspace keeps `Cargo.lock` tracked on purpose. These are deployable smart contracts, so reproducible dependency resolution is more important than treating the workspace like a reusable library crate.

## Integration Tests

The integration test suite expects a local contracts node at `ws://127.0.0.1:9944` unless `CONTRACTS_NODE_URL` is set.

```bash
cd integration-tests
npm install
CONTRACTS_NODE_URL=ws://127.0.0.1:9944 npm test -- --run
```

The suite deploys or reuses local artifacts from:

- `target/ink/alpha_lockup/alpha_lockup.wasm`
- `target/ink/lockup_listings/lockup_listings.wasm`
- `target/ink/otc_contract/otc_contract.wasm`

Local deployment cache files are ignored by Git:

- `integration-tests/.contract-address`
- `integration-tests/.lockup-listings-address`
- `integration-tests/.alpha-lockup-code-hash`

### Deterministic Localnet Gate

`integration-tests` also ships a deterministic localnet runner:

```bash
npm run test:localnet -- src/lockup-listings/create-listing.test.ts
npm run test:localnet:flaky
```

`test:localnet` starts `../subtensor-fork/scripts/localnet.sh`, waits for RPC readiness on `127.0.0.1:9944`, clears deployment caches by default, runs Vitest serially, stops the node, and verifies both the detached process group has exited and the RPC port is closed before continuing. By default it restarts localnet per test file so each suite gets a fresh chain, fresh contracts, and isolated subnet economics.

Deployment cache reuse is intentionally opt-in via `--reuse-deployments` or `OTC_TEST_REUSE_DEPLOYMENTS=1`. Use `npm run test:localnet:combined -- ...` only as a shared-node coupling diagnostic; later subnets on one long-lived localnet can have different runtime economics, so the confidence gate should remain restart-per-file.

## Deployment Order

1. Build and upload `alpha_lockup`; keep its code hash.
2. Instantiate `lockup_listings` with the `alpha_lockup` code hash.
3. Instantiate `otc_contract` for spot listings and offers.
4. Before using listing or selling flows that move stake from a user, that user must authorize the marketplace contract as a proxy on the chain.

### `otc_contract` Constructor

```rust
new(
    owner: AccountId,
    hotkey: AccountId,
    fee_rate: u128,
    min_listing_amount: u64,
    min_offer_amount: u64,
    min_listing_age: u32,
)
```

### `lockup_listings` Constructor

```rust
new(
    owner: AccountId,
    hotkey: AccountId,
    escrow_code_hash: Hash,
    fee_rate: u128,
    min_listing_amount: u64,
    min_purchase_amount: u64,
    min_lockup_duration: u32,
    max_lockup_duration: u32,
)
```

### `alpha_lockup` Constructor

```rust
new(
    buyer: AccountId,
    netuid: u16,
    alpha_amount: u64,
    unlock_block: u32,
    hotkey: AccountId,
    subnet_generation: u64,
)
```

`alpha_lockup` is normally instantiated by `lockup_listings` when a buyer takes a lockup listing.

## Units And Pricing

- TAO amounts are represented in rao: `1 TAO = 1_000_000_000 rao`.
- Alpha amounts are also represented in rao-style integer units.
- Fee rates are raw `U64F64` fixed-point bits. For example, `0.005` or 0.5% is `92233720368547758`.
- The chain extension returns Alpha market price as `TAO per Alpha * 1e9`.
- `price_offset_bps` is an offset from the live market price in basis points:
  - `0` means market price.
  - `500` means 5% above market.
  - `-500` means 5% below market.
  - Values must be greater than `-10000`.

Execution price is calculated at execution time:

```text
executed_price = market_price * (10000 + price_offset_bps) / 10000
```

## Spot OTC Interface

### Trading

```rust
list_alpha(hotkey, netuid, amount, price_offset_bps) -> Result<u64>
#[payable] create_tao_offer(netuid, price_offset_bps) -> Result<u64>
#[payable] take_alpha_listing(netuid, seller, listing_id) -> Result<()>
take_tao_offer(netuid, buyer, offer_id, hotkey) -> Result<()>
cancel_alpha_listing(netuid, listing_id) -> Result<()>
cancel_tao_offer(netuid, offer_id) -> Result<()>
force_cancel_alpha_listing(netuid, seller, listing_id) -> Result<()>
claim_dividends(netuid) -> Result<()>
```

`list_alpha` and `take_tao_offer` move Alpha from the caller through the Bittensor proxy flow. The caller must have authorized the contract as a proxy first.

`take_alpha_listing` accepts overpayment and refunds excess TAO after the trade executes.

`claim_dividends` is owner-only. It claims stake held by the contract above the Alpha reserved for open listings.

### Queries

```rust
get_listing(netuid, seller, listing_id) -> Option<AlphaListing>
get_user_listings(seller, netuid) -> Vec<u64>
get_offer(netuid, buyer, offer_id) -> Option<TaoOffer>
get_user_offers(buyer, netuid) -> Vec<u64>
estimate_listing_price(netuid, seller, listing_id) -> Result<u64>
estimate_offer_price(netuid, buyer, offer_id) -> Result<u64>
get_reserved_alpha(netuid) -> u64
is_subnet_frozen(netuid) -> bool
get_pause_state() -> PauseState
```

Users are limited to 25 active Alpha listings and 25 active TAO offers per subnet.

## Lockup Listings Interface

### Trading

```rust
create_lockup_listing(hotkey, netuid, amount, price_offset_bps, lockup_duration) -> Result<u64>
#[payable] take_lockup_listing(netuid, seller, listing_id, amount) -> Result<u64>
cancel_lockup_listing(netuid, listing_id) -> Result<()>
force_cancel_lockup_listing(netuid, seller, listing_id) -> Result<()>
```

`create_lockup_listing` moves Alpha from the seller through the Bittensor proxy flow. The seller must have authorized the contract as a proxy first.

`take_lockup_listing` can partially fill a listing. Each purchase instantiates a new `alpha_lockup` escrow and returns its purchase id. If a partial fill leaves a nonzero remainder below Bittensor's minimum stake amount of `2_000_000` rao, the purchase is rejected.

### Queries

```rust
get_listing(netuid, seller, listing_id) -> Option<LockupListing>
get_user_listings(seller, netuid) -> Vec<u64>
get_escrow(netuid, listing_id, purchase_id) -> Option<AccountId>
estimate_lockup_price(netuid, seller, listing_id, amount) -> Result<u64>
get_reserved_alpha(netuid) -> u64
get_reserved_alpha_for_hotkey(netuid, hotkey) -> u64
get_active_hotkey_for_subnet(netuid) -> AccountId
get_escrow_code_hash() -> Hash
get_lockup_duration_limits() -> (u32, u32)
get_pause_state() -> PauseState
```

Users are limited to 25 active lockup listings per subnet.

### Maintenance

```rust
consolidate_listing_hotkey(netuid, seller, listing_id) -> Result<()>
sync_escrow_hotkey(netuid, listing_id, purchase_id) -> Result<()>
```

Both methods are permissionless maintenance entry points used after the owner records a validator hotkey rotation with `update_hotkey_for_subnet`. `consolidate_listing_hotkey` attempts to move an open listing's Alpha to the active company hotkey; a failed consolidation leaves the listing purchasable and cancellable from its recorded custody hotkey. `sync_escrow_hotkey` points a purchased escrow directly at the current active subnet hotkey, so missed intermediate rotations do not require walking a successor chain.

## Alpha Lockup Interface

```rust
claim() -> Result<()>
propose_beneficiary(new_beneficiary) -> Result<()>
accept_beneficiary() -> Result<()>
cancel_beneficiary_proposal() -> Result<()>
recover_tao_after_deregistration() -> Result<()>
sync_hotkey_from_parent(expected_old_hotkey, new_hotkey, initiated_by) -> Result<()>
get_info() -> Result<LockupInfo>
get_buyer() -> AccountId
get_beneficiary() -> AccountId
get_pending_beneficiary() -> Option<AccountId>
get_tao_balance() -> Balance
get_hotkey() -> AccountId
get_subnet_generation() -> u64
get_unlock_block() -> u32
is_claimed() -> bool
blocks_until_unlock() -> u32
account_id() -> AccountId
```

Anyone can call `claim` after `unlock_block`; claiming earlier fails. A successful claim transfers all current escrow stake to the current beneficiary, including any staking rewards, emits `AlphaClaimed`, and terminates the escrow contract, sending any remaining TAO balance to the beneficiary.

Beneficiary changes for coldkey swaps are two-step: the current beneficiary calls `propose_beneficiary`, and the proposed account calls `accept_beneficiary`. A pending proposal can be withdrawn with `cancel_beneficiary_proposal`.

`recover_tao_after_deregistration` handles subnets that Bittensor deregisters while Alpha is escrowed: after the unlock block, once the runtime reports that the stored subnet generation no longer exists or the netuid has been reused by a newer generation, anyone can trigger recovery of the liquidated TAO to the current beneficiary.

`sync_hotkey_from_parent` is callable only by the `lockup_listings` factory contract, via `sync_escrow_hotkey`. No claim, sync, or recovery method accepts an arbitrary payout destination.

## Owner Controls

Both marketplace contracts have owner-only controls for:

- owner transfer
- hotkey updates
- fee rate updates
- minimum listing, offer, or purchase amount updates
- pause and resume
- `set_code` upgrades

Additional owner controls:

- `otc_contract`: `set_subnet_listing_status(netuid, frozen)` prevents new Alpha listings on a subnet while preserving cancellation and other allowed maintenance flows.
- `lockup_listings`: `update_hotkey_for_subnet(netuid, new_hotkey)` sets the official active hotkey for a subnet (`update_hotkey` remains as a backwards-compatible default hotkey update), `update_escrow_code_hash`, `update_lockup_duration_limits`, `set_risk_canceller` / `clear_risk_canceller` for the narrow force-cancel role, and the stale listing TAO recovery pool flow (`open_stale_listing_recovery_pool`, `increase_stale_listing_recovery_pool`, `recover_stale_listing_tao`).

Pause states are shared:

| State | Effect |
| --- | --- |
| `NotPaused` | Normal operation. |
| `TradingPaused` | New trading actions are blocked while maintenance and cancellation flows remain available where implemented. |
| `FullyPaused` | Trading and most state-changing flows are blocked except owner recovery actions such as `resume`. |

## Lockup Escrow Rollout

The lockup listings contract instantiates escrow contracts through its configured `escrow_code_hash`. To enable beneficiary handoff, runtime-backed subnet identity checks, hotkey sync, and TAO recovery for new purchases, upload the upgraded `alpha_lockup` code and call `update_escrow_code_hash` on the lockup listings contract. Existing escrows that were instantiated from older code are not upgraded by this change.

After a real Bittensor hotkey swap with `keep_stake = false`, the contract owner should call `update_hotkey_for_subnet(netuid, new_hotkey)` on `lockup_listings`. A backend service, beneficiary, or any other caller can lazily call `consolidate_listing_hotkey(netuid, seller, listing_id)` for open listings and `sync_escrow_hotkey(netuid, listing_id, purchase_id)` for purchased escrows. Sync targets the current active subnet hotkey directly, so missed intermediate rotations do not require walking a successor chain.

If a subnet is deregistered and Bittensor liquidates escrow Alpha into TAO, anyone can call `recover_tao_after_deregistration()` after the unlock block once the runtime reports that the stored subnet generation no longer exists or the netuid has been reused by a newer generation. The recovered TAO always terminates to the current escrow beneficiary.

Open listings on at-risk subnets should be force-cancelled before deregistration by the backend risk cron using the narrow `risk_canceller` role. If the subnet deregisters first, stale listing TAO recovery is owner-mediated for the aggregate recovery pool amount, while individual seller destinations and pro-rata distribution are enforced by the contract. See [Backend Subnet Risk Runbook](docs/backend-subnet-risk-runbook.md).

## Security Notes

- These contracts are not presented here as externally audited.
- The contracts depend on Bittensor runtime calls and chain extension behavior for stake movement, stake queries, and live Alpha pricing.
- Stake-moving seller flows require proxy authorization from the user before they can succeed.
- Market-relative prices are evaluated at execution time, not listing creation time.
- State is updated before external transfers in trade and claim flows.
- Stake transfer verification allows a 10 rao tolerance for Subtensor rounding or micro-fees.
- Lockup escrows support two-step beneficiary changes for coldkey swaps, permissionless fixed-destination post-unlock claims, parent-governed direct hotkey sync after validator hotkey swaps, and permissionless fixed-destination TAO recovery when runtime state proves the original subnet generation is gone. No claim, sync, or recovery method accepts an arbitrary payout destination.
- Lockup listings record the actual custody hotkey for factory-held Alpha. The factory attempts to consolidate listings to the company validator hotkey, but failed consolidation leaves listings purchasable and cancellable from their recorded custody hotkey.
- Lockup listings and escrows store the runtime `registered_subnet_counter` for their `netuid`, so reused netuids cannot be mistaken for the original subnet lifetime.
- Keep owner keys and upgrade authority operationally separate from test keys and development accounts.

Report suspected vulnerabilities privately using the instructions in `SECURITY.md`.

## License

This project is licensed under the MIT License. See `LICENSE` for details.
