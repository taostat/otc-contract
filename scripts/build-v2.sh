#!/bin/bash

# Build script for V2 contract
# This script builds the V2 version of the contract for upgrade testing

set -e

echo "Building V2 contract..."

# Create a temporary Cargo.toml that points to lib_v2.rs
cp Cargo.toml Cargo.toml.backup

# Modify the lib path in Cargo.toml temporarily
sed -i 's|path = "src/lib.rs"|path = "src/lib_v2.rs"|' Cargo.toml

# Build the V2 contract
echo "Compiling V2 contract..."
cargo contract build --release

# Create v2 directory if it doesn't exist
mkdir -p target/ink/v2

# Copy the built V2 artifacts
echo "Copying V2 artifacts..."
cp target/ink/otc_contract.wasm target/ink/v2/otc_contract_v2.wasm
cp target/ink/otc_contract.contract target/ink/v2/otc_contract_v2.contract
cp target/ink/otc_contract.json target/ink/v2/otc_contract_v2.json

# Restore original Cargo.toml
mv Cargo.toml.backup Cargo.toml

echo "V2 contract built successfully!"
echo "V2 artifacts saved to target/ink/v2/"

# Also build the original contract to ensure we have both versions
echo "Building original contract for comparison..."
cargo contract build --release

echo "Both contract versions built successfully!"