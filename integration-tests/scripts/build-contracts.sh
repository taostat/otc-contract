#!/usr/bin/env bash
# Build the release contracts the integration tests deploy, plus the test-only
# escrow build (a newer storage version) that the upgrade tests install.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

for contract in code_registry alpha_lockup lockup_listings otc; do
  cargo contract build --release --manifest-path "$root/contracts/$contract/Cargo.toml"
done

CARGO_TARGET_DIR="$root/target/upgrade-test" cargo contract build --release \
  --features upgrade-test \
  --manifest-path "$root/contracts/alpha_lockup/Cargo.toml"
