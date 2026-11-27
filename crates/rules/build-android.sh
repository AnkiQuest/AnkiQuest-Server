#!/usr/bin/env bash
# Builds the Android libraries and the Kotlin bindings into <out> under their
# release names (libankiquest_rules-android-<abi>.so, ankiquest_rules.kt) and
# writes <out>/SHA256SUMS, so <out> is a folder the Android app's build takes
# as ANKIQUEST_RULES_DIR. From clean target directories, with the toolchain
# crates/rules pins, NDK r28c (ANDROID_NDK_HOME), cargo-ndk 4.1.2, no build id,
# 16 KB pages and no machine paths, so a commit builds the same bytes on any
# machine: the Shared rules workflow builds it twice from different folders,
# and the app's F-Droid build runs it (docs/shared-rules.md).
set -euo pipefail
out=$(mkdir -p "$1" && cd "$1" && pwd)
root=$(cd "$(dirname "$0")/../.." && pwd)
target=$(mktemp -d)
trap 'rm -rf "$target"' EXIT

# The minSdk of the Android app, and the NDK and cargo-ndk the release uses.
api=24
ndk=28.2.13676358
cargo_ndk=4.1.2
abis="arm64-v8a armeabi-v7a x86 x86_64"

if ! grep -qx "Pkg.Revision = $ndk" "${ANDROID_NDK_HOME:?}/source.properties"; then
  echo "ANDROID_NDK_HOME is not NDK $ndk" >&2
  exit 1
fi
if [ "$(cargo ndk --version | awk '{ print $NF }')" != "$cargo_ndk" ]; then
  echo "cargo ndk is not $cargo_ndk" >&2
  exit 1
fi

cd "$root/crates/rules"

# The bindings come from the UniFFI metadata of an unstripped host build.
cargo build --locked -p ankiquest-rules --features ffi --target-dir "$target/host"
cargo run --locked -p ankiquest-rules --features bindgen --bin uniffi-bindgen --target-dir "$target/host" -- \
  generate --no-format --library "$target/host/debug/libankiquest_rules.so" --language kotlin --out-dir "$target/kotlin"
cp "$target/kotlin/org/ankiquest/rules/ankiquest_rules.kt" "$out/"

# Every ABI in one cargo-ndk run.
targets=()
for abi in $abis; do targets+=(-t "$abi"); done
RUSTFLAGS="-C link-arg=-Wl,--build-id=none -C link-arg=-Wl,-z,max-page-size=16384 \
--remap-path-prefix=$root=. --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$target=/target" \
  cargo ndk "${targets[@]}" -P "$api" build --locked -p ankiquest-rules --features ffi --profile bindings --target-dir "$target/android"
for abi in $abis; do
  case "$abi" in
    arm64-v8a) triple=aarch64-linux-android ;;
    armeabi-v7a) triple=armv7-linux-androideabi ;;
    x86) triple=i686-linux-android ;;
    x86_64) triple=x86_64-linux-android ;;
  esac
  cp "$target/android/$triple/bindings/libankiquest_rules.so" "$out/libankiquest_rules-android-$abi.so"
done

(cd "$out" && sha256sum ankiquest_rules.kt libankiquest_rules-android-*.so > SHA256SUMS)
