# Taostats OTC Smart Contracts

ink! smart contracts for Bittensor Alpha and TAO over-the-counter markets.

The repository currently contains five deployable contracts:

| Contract | Purpose |
| --- | --- |
| `otc_contract` | TAO offers: buyers deposit TAO to buy Alpha at a market-relative price, and sellers fill them. TAO isn't tied to a subnet, so offers stay pooled here. Upgrades through the registry. |
| `lockup_listings` | Factory for all Alpha listings, spot and lockup. Each listing is its own `listing_vault`; the factory holds no Alpha. It keeps the listing index and the settings vaults read (validator hotkey per subnet, fee recipient, trading pause), and upgrades through the registry. |
| `listing_vault` | One listing's Alpha, on the vault's own coldkey. Sellers fund it without a proxy, buyers take from it directly, and lockup takes create an escrow. Anyone can sweep its TAO and staking rewards to the seller. It upgrades only to the code the registry allows. |
| `alpha_lockup` | Per-purchase escrow created by a listing vault. It holds purchased Alpha until `unlock_block`, then lets anyone claim the Alpha plus any staking rewards for the beneficiary. It upgrades only to the code the registry allows. |
| `code_registry` | Timelocked allowlist of the code each contract kind may upgrade to, with a guardian veto. It is never upgraded itself, and every other contract upgrades through it; see the [design](docs/design/permissionless-positions.md). |

Shared Bittensor environment types, runtime calls, chain extensions, and fixed-point helpers live in `shared`.

## Repository Layout

```text
.
|-- contracts/
|   |-- alpha_lockup/        # Escrow contract created by lockup takes
|   |-- code_registry/       # Timelocked allowlist of upgrade code
|   |-- listing_vault/       # One contract per listing, holding its Alpha
|   |-- lockup_listings/     # Listing factory, spot and lockup
|   `-- otc/                 # TAO offers
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

cargo contract build --release --manifest-path contracts/code_registry/Cargo.toml
cargo contract build --release --manifest-path contracts/alpha_lockup/Cargo.toml
cargo contract build --release --manifest-path contracts/listing_vault/Cargo.toml
cargo contract build --release --manifest-path contracts/otc/Cargo.toml
cargo contract build --release --manifest-path contracts/lockup_listings/Cargo.toml
```

Contract artifacts are written under `target/ink/<contract_name>/`. `npm run build:contracts` in `integration-tests` builds all of them, plus the test-only builds described below.

The workspace keeps `Cargo.lock` tracked on purpose. These are deployable smart contracts, so reproducible dependency resolution is more important than treating the workspace like a reusable library crate.

## Integration Tests

The integration test suite expects a local contracts node at `ws://127.0.0.1:9944` unless `CONTRACTS_NODE_URL` is set.

```bash
cd integration-tests
npm install
npm run build:contracts
CONTRACTS_NODE_URL=ws://127.0.0.1:9944 npm test -- --run
```

The suite deploys local artifacts from:

- `target/ink/code_registry/code_registry.wasm`
- `target/ink/alpha_lockup/alpha_lockup.wasm`
- `target/ink/listing_vault/listing_vault.wasm`
- `target/ink/lockup_listings/lockup_listings.wasm`
- `target/ink/otc_contract/otc_contract.wasm`
- `target/upgrade-test/ink/alpha_lockup/alpha_lockup.wasm`: the escrow built with `--features upgrade-test`, which only bumps its storage version, for the escrow tests to upgrade to.
- `target/upgrade-test/ink/listing_vault/listing_vault.wasm`: the vault built with `--features upgrade-test`, which bumps its storage version and adds a field that its migration sets, for the vault upgrade tests. Its papi descriptor is `listing_vault_v2`.
- `target/upgrade-test/ink/code_registry/code_registry.wasm`: the registry built with `--features short-delays`, so the upgrade tests can activate and force an upgrade within a few blocks.

The suites cover deployment through the deploy script, listings and their vaults, escrows, vault and escrow upgrades through the registry, a subnet dissolution with open positions, and TAO offers.

Each run deploys fresh contracts. To reuse deployments across runs, set `OTC_TEST_REUSE_DEPLOYMENTS=1`; the cached addresses are stored in these files, which Git ignores:

- `integration-tests/.contract-address`
- `integration-tests/.lockup-listings-address`

### Localnet Runner

`integration-tests` ships a runner that manages the localnet for you:

```bash
npm run test:localnet -- src/lockup-listings/listing.test.ts
npm run test:localnet:flaky
```

`test:localnet` starts `../subtensor-fork/scripts/localnet.sh`, waits for RPC on `127.0.0.1:9944`, clears the deployment cache, and runs Vitest serially. After each batch it stops the node and checks that its process group has exited and the RPC port is closed. By default it restarts localnet for every test file, so each suite gets a fresh chain and fresh contracts.

- `--reuse-deployments` (or `OTC_TEST_REUSE_DEPLOYMENTS=1`) keeps the deployment cache.
- `npm run test:localnet:combined -- ...` runs all files against one node. Use it only to diagnose coupling between suites: subnets registered later on a long-lived localnet can have different economics.
- Test runs use the node binary that is already built. Build or rebuild it with `npm run test:localnet -- --build-only`, or set `BUILD_BINARY=1` to rebuild before every restart.

## Deployment

The deploy script runs the whole deployment in order and checks the result:

```bash
cd integration-tests
npm run build:contracts
DEPLOYER_SURI='<mnemonic>//<derivation path>' RPC_URL=wss://<node> npm run deploy -- <config.json> --out deployment.json
```

- `DEPLOYER_SURI` is the deployer's secret URI: a mnemonic, optionally followed by derivation paths and a `///password`. It is read from the environment so the secret never sits in a file. A bare path such as `//Alice` uses the development phrase, for localnet only.
- `RPC_URL` defaults to `ws://127.0.0.1:9944`.
- The config is a JSON `DeployConfig` (see `integration-tests/src/deploy.ts`). Amounts must be strings, since JSON numbers lose precision, and artifact paths are relative to the repository root. [`scripts/deploy.localnet.json`](integration-tests/scripts/deploy.localnet.json) is an example; it uses the `short-delays` registry, which is for localnet only.
- The script prints the deployment as JSON, with every address and code hash, and `--out` also writes it to a file.

The script does what a manual deployment would:

1. Upload the `alpha_lockup` and `listing_vault` code, unless it is already on chain.
2. Instantiate `code_registry` with the initial codes: `listing_vault` (kind `1`), `alpha_lockup` (kind `2`), and the code hashes of `lockup_listings` (kind `3`) and `otc_contract` (kind `4`).
3. Instantiate `lockup_listings` and `otc_contract` with the registry address.
4. Check that the registry offers each kind's code, and that both contracts answer to the registry and run the code it offers for them.

Every constructor is dry-run with the config before anything is uploaded, so a config the contracts reject changes nothing on chain. A later failure, such as running out of funds, leaves the earlier steps in place; a rerun reuses code already uploaded and deploys new contracts. The integration tests deploy through the same code.

Listings need no proxy. Filling a TAO offer moves the seller's stake through a proxy, which frontends add and remove in the seller's own batch (see below).

### `otc_contract` Constructor

```rust
new(
    owner: AccountId,
    registry: AccountId,
    fee_rate: u128,
    fee_recipient: AccountId,
    min_offer_amount: u64,
) -> Result<Self>
```

It fails if `fee_rate` exceeds `MAX_FEE_RATE` (5%).

### `lockup_listings` Constructor

```rust
new(
    owner: AccountId,
    registry: AccountId,
    default_hotkey: AccountId,
    fee_rate: u128,
    fee_recipient: AccountId,
    min_listing_amount: u64,
    min_purchase_amount: u64,
    min_lockup_duration: u32,
    max_lockup_duration: u32,
    min_listing_age: u32,
) -> Result<Self>
```

It fails if `fee_rate` exceeds `MAX_FEE_RATE` (5%) or the minimum lockup exceeds the maximum. `default_hotkey` is the validator hotkey vaults delegate to on subnets without an override. `min_listing_age` is how long a spot listing must stay open before the seller can cancel it.

### `listing_vault` Constructor

```rust
new(
    seller: AccountId,
    netuid: u16,
    hotkey: AccountId,
    amount: u64,
    price_offset_bps: i32,
    min_price: u64,
    fee_rate: FixedDecimal,
    min_purchase: u64,
    lockup_duration: Option<u32>,
    cancel_delay: u32,
    registry: AccountId,
)
```

Vaults are created by a factory, which becomes the vault's `factory` and keeps the listing's id. `lockup_duration` is `None` for spot listings. `cancel_delay` counts from activation: spot listings use the factory's `min_listing_age`, and lockup listings pass 0, so the seller can cancel at any time.

### `alpha_lockup` Constructor

```rust
new(
    buyer: AccountId,
    netuid: u16,
    alpha_amount: u64,
    unlock_block: u32,
    hotkey: AccountId,
    registry: AccountId,
)
```

A listing vault creates an `alpha_lockup` escrow when a buyer takes a lockup listing. The escrow binds to the vault's registry for life.

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

## TAO Offers Interface

```rust
#[payable] create_tao_offer(netuid, price_offset_bps, max_price) -> Result<u64>
take_tao_offer(netuid, buyer, offer_id, hotkey, max_alpha) -> Result<u64>
cancel_tao_offer(netuid, offer_id) -> Result<()>
quote_offer_fill(netuid, buyer, offer_id) -> Result<OfferQuote>
get_offer(netuid, buyer, offer_id) -> Option<TaoOffer>
get_user_offers(buyer, netuid) -> Vec<u64>
get_config() -> Config
is_trading_paused() -> bool
```

A buyer deposits TAO with `create_tao_offer`, at the market price moved by `price_offset_bps`. `max_price` is the buyer's ceiling, in TAO per Alpha scaled by 1e9 (0 for none): fills above it fail, so a seller can't push the price up and fill at it. Users are limited to 25 open offers per subnet, and buyers can cancel at any time, even while trading is paused.

A seller fills a whole offer with `take_tao_offer`. The fee comes out of the offer, and the rest buys Alpha at the current price; `quote_offer_fill` shows the numbers. The `TaoOfferTaken` event's `tao_amount` is what the seller received, so `tao_amount + fee` is the offer. `max_alpha` is the seller's cap: the fill fails if the price has fallen far enough to need more Alpha than that. The Alpha moves straight from the seller's stake on `hotkey` to the buyer through a `Transfer` proxy, which frontends add and remove within the seller's own transaction:

```text
utility.batch_all([
  proxy.add_proxy(otc, Transfer, 0),
  contracts.call(otc.take_tao_offer(..)),
  proxy.remove_proxy(otc, Transfer, 0),
])
```

If any call fails, the batch reverts and the proxy is never left behind. A frontend sets `max_alpha` from `quote_offer_fill` plus the slippage the seller accepts. The fill's gas limit must cover the proxied transfer, which pre-charges a worst-case weight (about 90e9 `ref_time`) before refunding it. A dry run made before the proxy exists fails, so it can't measure this. The integration tests use 250e9 `ref_time` and 3,000,000 `proof_size`, and unused gas is refunded.

## Listings Interface

Each listing, spot or lockup, is its own `listing_vault`, holding the seller's Alpha on the vault's own coldkey. The `lockup_listings` factory creates vaults and keeps the index; it never holds Alpha.

### Selling

A seller opens a listing with one transaction and no proxy. The batch creates the vault, moves the seller's stake to it, and activates it:

```text
utility.batch_all([
  contracts.call(factory.create_listing(netuid, hotkey, amount, price_offset_bps, min_price, lockup_duration)),
  subtensorModule.transfer_stake(vault, hotkey, netuid, netuid, amount),
  contracts.call(vault.activate()),
])
```

- `create_listing` creates a Pending vault and returns the listing id and the vault's address; the seller pays the vault's storage deposit. With a `lockup_duration`, buyers receive the Alpha in an escrow that locks it for that long, within the factory's limits. With `None` it is a spot listing: buyers receive the Alpha directly, and the seller can cancel only once the listing has been open for `min_listing_age`.
- `activate` requires the vault to hold `amount` within the transfer tolerance and lists what it actually holds. It then moves the stake to the factory's validator hotkey for the subnet, when Subtensor allows. If activation fails, the batch reverts the transfer too.
- `min_price` is the seller's floor, in TAO per Alpha scaled by 1e9 (0 for none). Takes below it fail, so a buyer can't push the spot price down earlier in the same batch and buy cheap.

The batch needs the vault's address before the vault exists, and it is predictable. pallet-contracts derives a contract's address as

```text
blake2_256(SCALE(b"contract_addr_v1", deployer, code_hash, input_data, salt))
```

where `b"contract_addr_v1"` is 16 raw bytes, and `input_data` (the constructor's selector and arguments) and `salt` carry length prefixes. For a vault:

- the deployer is the factory;
- the code hash is the registry's `current(1)`;
- the input is `listing_vault::new` with the seller's terms and the factory's `get_config()` fee rate, minimum purchase and registry, with `cancel_delay` set to `min_listing_age` for a spot listing and 0 for a lockup listing;
- the salt is the seller's account followed by `get_seller_nonce(seller)` as a little-endian `u64`.

[`integration-tests/src/vault-address.ts`](integration-tests/src/vault-address.ts) implements this (`predictVaultAddress`), and the listing tests open every listing this way. If anything changes between the prediction and the batch, such as the factory's settings, the registry's vault code or the seller's nonce, the vault lands at another address. Activating the predicted address then fails, and the whole batch reverts.

A frontend can also fund in two transactions: `create_listing` first, then a `batch_all` of the transfer and `activate` against the address it returned. Until it is funded, a Pending vault can be cancelled and closed at any time.

### Vault

```rust
activate() -> Result<()>
#[payable] take(amount, recipient) -> Result<Option<AccountId>>
quote(amount) -> Result<Quote>
cancel() -> Result<()>
close() -> Result<()>
sweep_stake() -> Result<()>
sweep_tao() -> Result<()>
resync_hotkey(new_hotkey) -> Result<()>
consolidate() -> Result<()>
upgrade(code_hash) -> Result<()>
set_upgrade_policy(policy) -> Result<()>
migrate()
get_info() -> Result<VaultInfo>
get_status() -> Status
get_seller() -> AccountId
get_factory() -> AccountId
get_registry() -> AccountId
get_hotkey() -> AccountId
get_remaining() -> u64
get_upgrade_policy() -> UpgradePolicy
get_version() -> u16
```

- `take` buys `amount` at the market price moved by the listing's offset, plus the fee. `quote` shows the cost, and any excess TAO is refunded. A lockup take creates an `alpha_lockup` escrow for `recipient` from the registry's current escrow code, and returns it. A partial take must leave nothing or at least Bittensor's minimum stake of `2_000_000` rao. Takes below the minimum purchase fail, except for the whole remainder: stake transfers can round down by a rao, so a vault can hold slightly less than was listed. Takes fail while the factory is paused. The seller receives the price, and the factory's fee recipient the fee.
- The seller can `cancel` at any time, and the stake returns to them, rewards included. If Subtensor refuses the transfer, for example while the subnet is being dissolved, the listing still closes and the sweeps return what is left later. Cancelling never consults the factory and is never paused.
- Anyone can call `sweep_stake`, which sends stake beyond the listing, such as rewards, to the seller (all of it once the listing is closed), and `sweep_tao`, which sends the vault's TAO, for example from a subnet liquidation, to the seller. Neither terminates the vault.
- Anyone can call `resync_hotkey` after a validator hotkey swap, by the same rule as escrows, and `consolidate` to move an open listing to the factory's current validator hotkey.
- `close` lets the seller terminate a closed vault, recovering its TAO and storage deposit. Remaining stake goes to the seller first; only dust worth less than Subtensor's minimum transfer is left behind. Anything sent to a terminated vault is lost.
- Upgrades work as for escrows, against the registry's vault code, with the seller as the owner.

### Factory

```rust
create_listing(netuid, hotkey, amount, price_offset_bps, min_price, lockup_duration: Option<u32>) -> Result<(u64, AccountId)>
prune_listing(netuid, seller, listing_id) -> Result<()>
get_listing(netuid, seller, listing_id) -> Option<AccountId>
get_vault_listing(vault) -> Option<(u16, AccountId, u64)>
get_user_listings(seller, netuid) -> Vec<u64>
get_seller_nonce(seller) -> u64
is_subnet_frozen(netuid) -> bool
get_config() -> Config
get_owner() -> AccountId
get_pending_owner() -> Option<AccountId>
get_registry() -> AccountId
get_version() -> u16
VaultFactory::trading_paused() -> bool
VaultFactory::fee_recipient() -> AccountId
VaultFactory::validator_hotkey(netuid) -> AccountId
VaultFactory::listing_closed()
```

The index holds open listings; a vault removes its listing when it fills or is cancelled. That report is best effort, so if it ever fails, anyone can `prune_listing` once the vault is closed or gone. A seller can have 25 open listings per subnet, spot and lockup together. Vaults copy the fee rate and minimum purchase when they are created, and read the fee recipient, validator hotkey and pause from the factory.

### Subnet Dissolution

When a subnet is dissolved, Subtensor sells every position on it in the blocks that follow, crediting each coldkey's share of the subnet's TAO to its free balance. A vault or escrow is a coldkey of its own, so the TAO arrives in the contract, and nothing needs a backend:

- Takes fail once the subnet is dissolved.
- Anyone can `sweep_tao` a vault to its seller and an escrow to its beneficiary. Sweeping never terminates a contract, so TAO that arrives later can be swept too.
- An escrow has no Alpha left to claim (`claim` fails with `NoAlphaToClaim`). Its beneficiary can `close` it once its stake is gone, even before `unlock_block`.
- The seller cancels each listing (a spot listing once its minimum age has passed), which returns nothing and drops it from the index. Then they close the vault.
- A new subnet can reuse the netuid, and new listings on it trade as usual. Listings the seller hasn't cancelled stay in the factory's index under that netuid and count toward their 25-listing cap there until they cancel them.

The owner can `set_subnet_frozen` a subnet at risk, to stop new listings on it beforehand.

## Alpha Lockup Interface

```rust
claim() -> Result<()>
close() -> Result<()>
sweep_tao() -> Result<()>
resync_hotkey(new_hotkey) -> Result<()>
propose_beneficiary(new_beneficiary) -> Result<()>
accept_beneficiary() -> Result<()>
cancel_beneficiary_proposal() -> Result<()>
upgrade(code_hash) -> Result<()>
set_upgrade_policy(policy) -> Result<()>
migrate()
get_info() -> Result<LockupInfo>
get_buyer() -> AccountId
get_beneficiary() -> AccountId
get_pending_beneficiary() -> Option<AccountId>
get_tao_balance() -> Balance
get_hotkey() -> AccountId
get_registry() -> AccountId
get_upgrade_policy() -> UpgradePolicy
get_version() -> u16
get_unlock_block() -> u32
is_claimed() -> bool
blocks_until_unlock() -> u32
account_id() -> AccountId
```

Anyone can call `claim` once `unlock_block` is reached; claiming earlier fails. The Alpha always goes to the current beneficiary, never to the caller. A claim transfers all the escrow's stake on its hotkey, including any staking rewards, and emits `AlphaClaimed`. Claiming never closes the escrow, so stake that reaches its hotkey later, for example after `resync_hotkey`, can be claimed too. The beneficiary terminates an empty escrow with `close`, which sends its TAO balance and storage deposit to the beneficiary and fails while the escrow holds stake on its hotkey. Anything sent to a closed escrow is lost, so close only once nothing else is on its way.

Beneficiary changes, for example after a coldkey swap, take two steps: the current beneficiary calls `propose_beneficiary`, and the proposed account calls `accept_beneficiary`. The current beneficiary can withdraw a proposal with `cancel_beneficiary_proposal`. While a proposal is pending, `claim`, `sweep_tao` and `close` fail with `BeneficiaryTransferPending`, so a payout can't race a handoff away from a key that is being retired.

Anyone can also call:

- `sweep_tao`, which sends the escrow's TAO balance, for example from a subnet liquidation, to the beneficiary. The escrow stays open.
- `resync_hotkey`, which points the escrow at a new hotkey after its stake moved there, for example in a validator hotkey swap. It moves nothing, and it is accepted only when the escrow holds more stake on the new hotkey than on its current one.

`upgrade` installs only the registry's current escrow code. The beneficiary can upgrade at any time. Anyone else can apply the code once it has been the registry's current code for the force delay, unless the beneficiary called `set_upgrade_policy(ConsentOnly)`; the default is `Auto`. The new code migrates storage on its next call, or when anyone calls `migrate`.

## Code Registry Interface

```rust
new(owner, guardian, upgrade_delay, force_delay, initial_codes: Vec<(u16, Hash)>)
propose(kind, code_hash) -> Result<()>
veto(kind, code_hash) -> Result<()>
activate(kind, code_hash) -> Result<()>
revoke(kind, code_hash) -> Result<()>
propose_role_change(role, account) -> Result<()>
veto_role_change() -> Result<()>
execute_role_change() -> Result<()>
current(kind) -> Option<CurrentCode>
is_current(kind, code_hash) -> bool
force_target(kind) -> Option<Hash>
proposal(kind, code_hash) -> Option<Proposal>
is_revoked(kind, code_hash) -> bool
delays() -> (u32, u32)
owner() -> AccountId
guardian() -> AccountId
pending_role_change() -> Option<RoleChange>
```

The registry tracks one current code hash per contract kind (`otc-shared` defines the kinds: listing vault, escrow, lockup factory and otc). The owner proposes a code hash. Once `upgrade_delay` blocks have passed, anyone can `activate` it, unless the guardian vetoed it first. Proposals are numbered, and only a proposal newer than the kind's last activated one can be activated, so a stale proposal can never roll the code back; rolling back on purpose means proposing the old code again. Code that has been current for `force_delay` blocks becomes the `force_target`, which anyone may apply to positions that allow forced upgrades. The owner or the guardian can `revoke` a code hash, which clears it if it is current and blocks it from ever being proposed or activated again for that kind.

Owner and guardian changes go through the same delay and guardian veto, and the two roles must always be different accounts. Both delays are fixed at deployment and must be at least `MIN_DELAY` (7,200 blocks, about a day). Build with `--features short-delays` only for localnet tests, which need shorter delays.

Both roles should be multisigs. A compromised key can stall upgrades but cannot move funds: a rogue owner can only propose code that the guardian then has to veto or revoke, and a rogue guardian can veto every proposal, including its own replacement. The registry has no way to recover from either. Recovering means deploying a new registry and new factories, and positions bound to the old registry keep their current code.

## Owner Controls

Owner settings only shape future listings, offers and trades, and none can block an exit. Upgrades are the one power that reaches funds: listings and escrows only take code the owner proposed through the registry's delay and guardian veto (and escrows can refuse forced upgrades), while TAO offers sit in `otc_contract` itself, so a buyer's protection there is the registry's delay, during which they can cancel. Both `lockup_listings` and `otc_contract` have:

- `transfer_ownership(new_owner)`, which the new owner completes with `accept_ownership()`; `None` withdraws a proposal
- `set_fee_rate`, for new listings or offers only and at most `MAX_FEE_RATE` (5%), and `set_fee_recipient`
- `pause(blocks)` for at most `MAX_PAUSE` (50,400 blocks, about a week), renewable, and `resume()`. A pause blocks new listings, offers and trades, never cancels, sweeps or claims.
- `upgrade(code_hash)` to the registry's current code for the contract; the new code migrates on its next call, or through `migrate()`.

`lockup_listings` also has:

- `set_default_hotkey` and `set_validator_hotkey(netuid, hotkey)`, where `None` removes a subnet's override
- `set_min_listing_amount`, `set_min_purchase_amount` (new listings only), `set_lockup_duration_limits`, and `set_min_listing_age` (at most `MAX_LISTING_AGE`, 50,400 blocks, since sellers don't choose it)
- `set_subnet_frozen(netuid, frozen, reason)`, which stops new listings on a subnet, for example while it is at risk of deregistration; open listings are unaffected

`otc_contract` also has `set_min_offer_amount`.

## Security Notes

- These contracts are not presented here as externally audited.
- The contracts depend on Bittensor runtime calls and chain extension behavior for stake movement, stake queries, and live Alpha pricing.
- Listings are funded without a proxy. Filling a TAO offer is the one flow that needs one: frontends add it and remove it again in the seller's own batch, so it never outlives the fill. A seller who adds the proxy on its own leaves `otc_contract` able to move their stake until they remove it.
- Market-relative prices are evaluated at execution time, not listing creation time.
- A listing vault or escrow only ever sends Alpha or TAO to its owner, a buyer who paid in the same call, or an escrow it creates in the same call; fees go to the factory's fee recipient out of the buyer's payment.
- Anyone can deploy the vault code with themselves as the "factory" and a registry of their own, which would let them charge any fee and create escrows from code they control. Before a buyer pays a vault, frontends must check that `get_factory()` is the official factory, that the factory's `get_vault_listing(vault)` returns the listing, that `get_registry()` is the official registry, and that the vault runs the registry's vault code.
- Trade flows update listing state before paying out. Escrow claims mark the escrow claimed only after the stake transfer is verified. Any failure reverts the whole call, so no partial state is kept.
- Every stake transfer is verified against the stake actually moved, within 10 rao either way: stake is a share of a hotkey's pool, and Subtensor's rounding can shave a rao off a transfer or add one to a recipient that already stakes on the hotkey. A transfer that fails verification traps, reverting the whole call. Listings record the stake that actually arrived, so later moves never ask for stake the contract does not hold.
- Keep owner keys and upgrade authority operationally separate from test keys and development accounts.

Report suspected vulnerabilities privately using the instructions in `SECURITY.md`.

## License

This project is licensed under the MIT License. See `LICENSE` for details.
