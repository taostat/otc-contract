#!/usr/bin/env bash
# Build the release contracts the integration tests deploy, plus two test-only
# builds: an escrow with a newer storage version for the upgrade tests to
# install, and a code registry without the minimum delays, so localnet tests can
# activate and force upgrades within a few blocks.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

for contract in code_registry alpha_lockup listing_vault lockup_listings otc; do
  cargo contract build --release --manifest-path "$root/contracts/$contract/Cargo.toml"
done

# Test-only builds, kept apart from the release artifacts.
CARGO_TARGET_DIR="$root/target/upgrade-test" cargo contract build --release \
  --features upgrade-test \
  --manifest-path "$root/contracts/alpha_lockup/Cargo.toml"
CARGO_TARGET_DIR="$root/target/upgrade-test" cargo contract build --release \
  --features short-delays \
  --manifest-path "$root/contracts/code_registry/Cargo.toml"
