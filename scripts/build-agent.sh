#!/usr/bin/env bash
# Build a fully static Linux agent with Rust + Go, using Zig's musl toolchain.
# Usage: scripts/build-agent.sh [amd64|arm64] [output]
set -euo pipefail
cd "$(dirname "$0")/.."
arch="${1:-amd64}"
case "$arch" in
    amd64) target=x86_64-unknown-linux-musl ;;
    arm64) target=aarch64-unknown-linux-musl ;;
    *) echo "unsupported Linux architecture: $arch" >&2; exit 2 ;;
esac
out="${2:-dist/gamelink-agent-linux-$arch}"
# Never accidentally ship the test-only panic method.
if [[ -n "${GAMELINK_FFI_FEATURES:-}" ]]; then
    echo "GAMELINK_FFI_FEATURES must be unset for release builds" >&2
    exit 2
fi
bash scripts/build-agent-shim.sh "$target"
mkdir -p "$(dirname "$out")"
# Rust's bundled LLD and musl provide the final static runtime; Zig compiles
# the Go archive. This avoids two competing sets of C startup objects.
link_flags="-C linker=rust-lld -C linker-flavor=ld.lld -C relocation-model=static -C strip=symbols"
# Preserve the target's Cortex-A53 erratum workaround when bypassing cc.
[[ "$arch" != arm64 ]] || link_flags+=" -C link-arg=--fix-cortex-a53-843419"
RUSTFLAGS="${RUSTFLAGS:-} $link_flags" cargo build --manifest-path core/Cargo.toml --locked --release -p gamelink-agent --target "$target"
cp "core/target/$target/release/gamelink-agent" "$out"
