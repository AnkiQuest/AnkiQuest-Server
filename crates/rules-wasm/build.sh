#!/usr/bin/env bash
# Builds the WebAssembly module into <out>/ankiquest_rules.wasm and writes
# <out>/SHA256SUMS: from a clean target directory, with the toolchain
# crates/rules pins and no machine paths, so a commit always builds the
# same bytes (docs/shared-rules.md).
set -euo pipefail
out=$(mkdir -p "$1" && cd "$1" && pwd)
root=$(cd "$(dirname "$0")/../.." && pwd)
target=$(mktemp -d)
trap 'rm -rf "$target"' EXIT

cd "$root/crates/rules"
RUSTFLAGS="--remap-path-prefix=$root=. --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo" \
  cargo build --locked -p ankiquest-rules-wasm --target wasm32-unknown-unknown --profile wasm --target-dir "$target"
cp "$target/wasm32-unknown-unknown/wasm/ankiquest_rules_wasm.wasm" "$out/ankiquest_rules.wasm"
(cd "$out" && sha256sum ankiquest_rules.wasm > SHA256SUMS)
