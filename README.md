# Taostats OTC Smart Contract

A decentralized over-the-counter (OTC) trading desk smart contract for Bittensor, enabling trustless peer-to-peer trading between Alpha tokens (Bittensor subnet tokens) and TAO tokens.

## Features

- **Alpha Listings**: List Alpha tokens for sale with dynamic market-relative pricing
- **TAO Offers**: Create buy offers for Alpha tokens with market-relative pricing
- **Atomic Swaps**: Execute trades atomically with automatic fee collection
- **Cancellation**: Cancel listings/offers with full refunds
- **Admin Controls**: Manage fees, minimum amounts, and contract configuration

## Prerequisites

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install cargo-contract for ink! development
cargo install --force --locked cargo-contract

# Add WebAssembly target
rustup target add wasm32-unknown-unknown
```

## Building

```bash
# Build contract (debug mode)
cargo build

# Build for release
cargo build --release

# Build WASM for deployment
cargo contract build --release

# Run checks
cargo check
cargo clippy
cargo fmt -- --check
```

## Testing

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_list_alpha

# Run with output
cargo test -- --nocapture
```

## Deployment

1. **Build the contract**:
```bash
cargo contract build --release
```

2. **Deploy to chain**:
```bash
cargo contract instantiate \
    --constructor new \
    --args <owner> <hotkey> <fee_rate_bits> <min_listing> <min_offer> <min_age> \
    --suri <deployer_key> \
    --url <node_url>
```

Constructor parameters:
- `owner`: Contract administrator account
- `hotkey`: Validator hotkey for stake consolidation
- `fee_rate_bits`: Fee rate as U64F64 bits (e.g., `4607182418800017408` for 0.1%)
- `min_listing`: Minimum Alpha amount for listings (in rao)
- `min_offer`: Minimum TAO amount for offers (in rao)
- `min_age`: Minimum blocks before listing cancellation

## Contract Interface

### Trading Methods

#### List Alpha Tokens
```rust
list_alpha(hotkey: AccountId, netuid: u16, amount: u64, price_offset_bps: i32) -> Result<u64>
```
Lists Alpha tokens for sale at a price relative to market. The `price_offset_bps` is the offset from market price in basis points (1 bp = 0.01%). Examples: `0` = market price, `500` = +5% above market, `-500` = -5% below market. Requires seller to add contract as proxy first.

#### Create TAO Offer
```rust
#[payable]
create_tao_offer(netuid: u16, price_offset_bps: i32) -> Result<u64>
```
Creates a buy offer with TAO at a price relative to market. The `price_offset_bps` works the same as in `list_alpha`. Amount determined by transaction value.

#### Take Alpha Listing
```rust
#[payable]
take_alpha_listing(netuid: u16, seller: AccountId, listing_id: u64) -> Result<()>
```
Buy listed Alpha tokens. TAO amount is calculated at execution time based on current market price and the listing's price offset.

#### Take TAO Offer
```rust
take_tao_offer(netuid: u16, buyer: AccountId, offer_id: u64, hotkey: AccountId) -> Result<()>
```
Sell Alpha tokens to a TAO offer. Requires proxy authorization.

### Query Methods

```rust
get_listing(netuid: u16, seller: AccountId, listing_id: u64) -> Option<AlphaListing>
get_offer(netuid: u16, buyer: AccountId, offer_id: u64) -> Option<TaoOffer>
get_user_listings(seller: AccountId, netuid: u16) -> Vec<u64>  // Returns empty Vec if no listings
get_user_offers(buyer: AccountId, netuid: u16) -> Vec<u64>     // Returns empty Vec if no offers
get_active_hotkey_for_subnet(netuid: u16) -> AccountId
get_reserved_alpha_for_hotkey(netuid: u16, hotkey: AccountId) -> u64
```

### Lockup Escrow Operations

```rust
consolidate_listing_hotkey(netuid: u16, seller: AccountId, listing_id: u64) -> Result<()>
sync_escrow_hotkey(netuid: u16, listing_id: u64, purchase_id: u64) -> Result<()>
```

### Admin Methods

```rust
update_owner(new_owner: AccountId) -> Result<()>
update_hotkey(new_hotkey: AccountId) -> Result<()>  // Backwards-compatible default hotkey update
update_hotkey_for_subnet(netuid: u16, new_hotkey: AccountId) -> Result<()>  // Sets the official subnet hotkey
update_fee_rate(new_rate: u128) -> Result<()>
update_min_listing_amount(new_amount: u64) -> Result<()>
update_min_offer_amount(new_amount: u64) -> Result<()>
update_min_listing_age(new_age: u32) -> Result<()>
```

## Architecture

### Module Structure

- `lib.rs` - Main contract logic with all public functions and tests
- `types.rs` - Type definitions including FixedDecimal wrapper for U64F64 fixed-point math
- `errors.rs` - Comprehensive error enum for all failure cases
- `events.rs` - Event definitions with proper indexing for off-chain monitoring
- `runtime.rs` - Bittensor Subtensor pallet integration via runtime calls
- `chain_extension.rs` - Chain extension interface for querying stake information

### Key Components

- **Dynamic Pricing**: Market-relative pricing with basis points offset, calculated at execution time
- **Fixed-Point Arithmetic**: Uses `FixedDecimal` (U64F64) for deterministic fee calculations
- **Proxy Integration**: All stake transfers use Bittensor's Proxy pallet for authorization
- **Composite Storage**: Multi-dimensional lookups using `(netuid, user, id)` tuples
- **Event System**: Comprehensive events for off-chain monitoring
- **Transfer Tolerance**: Stake transfers verified with 10 rao tolerance to account for rounding/micro-fees
- **Lockup Escrow Safety**: Upgraded Alpha lockup escrows support two-step beneficiary changes for coldkey swaps, permissionless fixed-destination post-unlock claims, parent-governed direct hotkey sync after validator hotkey swaps, and permissionless fixed-destination TAO recovery when runtime state proves the original subnet generation is gone. No claim, sync, or recovery method accepts an arbitrary payout destination.
- **Listing Custody Safety**: Lockup listings record the actual custody hotkey for factory-held Alpha. The factory attempts to consolidate listings to the company validator hotkey, but failed consolidation leaves listings purchasable and cancellable from their recorded custody hotkey.
- **Subnet Identity**: Lockup listings and escrows store the runtime `registered_subnet_counter` for their `netuid`, so reused netuids cannot be mistaken for the original subnet lifetime.

### Lockup Escrow Rollout

The lockup listings contract instantiates escrow contracts through its configured `escrow_code_hash`. To enable beneficiary handoff, runtime-backed subnet identity checks, hotkey sync, and TAO recovery for new purchases, upload the upgraded `alpha_lockup` code and call `update_escrow_code_hash` on the lockup listings contract. Existing escrows that were instantiated from older code are not upgraded by this change.

After a real Bittensor hotkey swap with `keep_stake = false`, the contract owner should call `update_hotkey_for_subnet(netuid, new_hotkey)` on `lockup_listings`. A backend service, beneficiary, or any other caller can lazily call `consolidate_listing_hotkey(netuid, seller, listing_id)` for open listings and `sync_escrow_hotkey(netuid, listing_id, purchase_id)` for purchased escrows. Sync targets the current active subnet hotkey directly, so missed intermediate rotations do not require walking a successor chain.

If a subnet is deregistered and Bittensor liquidates escrow Alpha into TAO, anyone can call `recover_tao_after_deregistration()` after the unlock block once the runtime reports that the stored subnet generation no longer exists or the netuid has been reused by a newer generation. The recovered TAO always terminates to the current escrow beneficiary.

Open listings on at-risk subnets should be force-cancelled before deregistration by the backend risk cron using the narrow `risk_canceller` role. If the subnet deregisters first, stale listing TAO recovery is owner-mediated for the aggregate recovery pool amount, while individual seller destinations and pro-rata distribution are enforced by the contract. See [Backend Subnet Risk Runbook](docs/backend-subnet-risk-runbook.md).

### Storage Pattern

```rust
alpha_listings: Mapping<(netuid, seller, listing_id), AlphaListing>
tao_offers: Mapping<(netuid, buyer, offer_id), TaoOffer>
user_listings: Mapping<(seller, netuid), Vec<listing_id>>
user_offers: Mapping<(buyer, netuid), Vec<offer_id>>
```

### Dynamic Market-Relative Pricing

Prices are specified as offsets from the current market price in basis points (bps):
- 1 basis point = 0.01%, so 100 bps = 1%
- `price_offset_bps = 0`: Trade at current market price
- `price_offset_bps = 500`: Trade at 5% above market price
- `price_offset_bps = -500`: Trade at 5% below market price
- Valid range: -10000 to i32::MAX (cannot go below -100%)

The actual TAO amount is calculated at execution time:
```
executed_price = market_price × (10000 + price_offset_bps) / 10000
```

This allows listings and offers to automatically adjust with market movements while maintaining the seller's/buyer's desired premium or discount.

### Units

- 1 TAO = 10^9 rao (smallest unit)
- Fees calculated as percentage of TAO amount using fixed-point arithmetic (U64F64)

## Integration Testing

The deterministic localnet gate lives in `integration-tests`:

```bash
npm run test:localnet -- src/lockup-listings/create-listing.test.ts
npm run test:localnet:flaky
```

`test:localnet` starts `../subtensor-fork/scripts/localnet.sh`, waits for RPC readiness on `127.0.0.1:9944`, clears deployment caches by default, runs Vitest serially, stops the node, and verifies the RPC port is closed before continuing. By default it restarts localnet per test file so each suite gets a fresh chain, fresh contracts, and isolated subnet economics.

Deployment cache reuse is intentionally opt-in via `--reuse-deployments` or `OTC_TEST_REUSE_DEPLOYMENTS=1`. Use `npm run test:localnet:combined -- ...` only as a shared-node coupling diagnostic; later subnets on one long-lived localnet can have different runtime economics, so the confidence gate should remain restart-per-file.

## Security Considerations

1. **Proxy Authorization**: Users must add contract as proxy before listing Alpha
2. **Atomic Execution**: All trades execute atomically or revert completely
3. **Input Validation**: All amounts and prices validated against minimums
4. **Access Control**: Admin functions restricted to contract owner
5. **No Reentrancy**: State changes before external calls
