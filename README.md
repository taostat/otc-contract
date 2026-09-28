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

The suite deploys local artifacts from:

- `target/ink/alpha_lockup/alpha_lockup.wasm`
- `target/ink/lockup_listings/lockup_listings.wasm`
- `target/ink/otc_contract/otc_contract.wasm`

Each run deploys fresh contracts. To reuse deployments across runs, set `OTC_TEST_REUSE_DEPLOYMENTS=1`; the cached addresses are stored in these files, which Git ignores:

- `integration-tests/.contract-address`
- `integration-tests/.lockup-listings-address`
- `integration-tests/.alpha-lockup-code-hash`

### Localnet Runner

`integration-tests` ships a runner that manages the localnet for you:

```bash
npm run test:localnet -- src/lockup-listings/claim.test.ts
npm run test:localnet:flaky
```

`test:localnet` starts `../subtensor-fork/scripts/localnet.sh`, waits for RPC on `127.0.0.1:9944`, clears the deployment cache, and runs Vitest serially. After each batch it stops the node and checks that its process group has exited and the RPC port is closed. By default it restarts localnet for every test file, so each suite gets a fresh chain and fresh contracts.

- `--reuse-deployments` (or `OTC_TEST_REUSE_DEPLOYMENTS=1`) keeps the deployment cache.
- `npm run test:localnet:combined -- ...` runs all files against one node. Use it only to diagnose coupling between suites: subnets registered later on a long-lived localnet can have different economics.
- `BUILD_BINARY=0` skips rebuilding the node when it is already built.

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
get_escrow_code_hash() -> Hash
get_lockup_duration_limits() -> (u32, u32)
get_pause_state() -> PauseState
```

Users are limited to 25 active lockup listings per subnet.

## Alpha Lockup Interface

```rust
claim() -> Result<()>
propose_beneficiary(new_beneficiary) -> Result<()>
accept_beneficiary() -> Result<()>
cancel_beneficiary_proposal() -> Result<()>
get_info() -> Result<LockupInfo>
get_buyer() -> AccountId
get_beneficiary() -> AccountId
get_pending_beneficiary() -> Option<AccountId>
get_tao_balance() -> Balance
get_hotkey() -> AccountId
get_unlock_block() -> u32
is_claimed() -> bool
blocks_until_unlock() -> u32
account_id() -> AccountId
```

Anyone can call `claim` once `unlock_block` is reached; claiming earlier fails. The Alpha always goes to the current beneficiary, never to the caller. A successful claim transfers all current escrow stake, including any staking rewards, emits `AlphaClaimed`, and terminates the escrow contract, sending any TAO balance to the beneficiary.

Beneficiary changes, for example after a coldkey swap, take two steps: the current beneficiary calls `propose_beneficiary`, and the proposed account calls `accept_beneficiary`. The current beneficiary can withdraw a proposal with `cancel_beneficiary_proposal`. While a proposal is pending, `claim` fails with `BeneficiaryTransferPending`, so a payout can't race a handoff away from a key that is being retired.

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
- `lockup_listings`: `update_escrow_code_hash` and `update_lockup_duration_limits`.

Pause states are shared:

| State | Effect |
| --- | --- |
| `NotPaused` | Normal operation. |
| `TradingPaused` | New trading actions are blocked while maintenance and cancellation flows remain available where implemented. |
| `FullyPaused` | Trading and most state-changing flows are blocked except owner recovery actions such as `resume`. |

## Security Notes

- These contracts are not presented here as externally audited.
- The contracts depend on Bittensor runtime calls and chain extension behavior for stake movement, stake queries, and live Alpha pricing.
- Stake-moving seller flows require proxy authorization from the user before they can succeed.
- Market-relative prices are evaluated at execution time, not listing creation time.
- State is updated before external transfers in trade and claim flows.
- Stake transfer verification allows a 10 rao tolerance for Subtensor rounding or micro-fees.
- Keep owner keys and upgrade authority operationally separate from test keys and development accounts.

Report suspected vulnerabilities privately using the instructions in `SECURITY.md`.

## License

This project is licensed under the MIT License. See `LICENSE` for details.
