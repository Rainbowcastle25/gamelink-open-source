#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
target="${1:-x86_64-unknown-linux-gnu}"
tags="$(tr -d '\r\n' < build-tags.txt),netgo,osusergo"
case "$target" in
 x86_64-unknown-linux-gnu) arch=amd64; cc="${CC:-cc}" ;;
 x86_64-unknown-linux-musl) arch=amd64; cc="$PWD/scripts/zig-cc-linux-amd64.sh"; tags+=,rust_musl ;;
 aarch64-unknown-linux-musl) arch=arm64; cc="$PWD/scripts/zig-cc-linux-arm64.sh"; tags+=,rust_musl ;;
 *) echo "unsupported agent target: $target" >&2; exit 2 ;;
esac
[[ -z "${GAMELINK_FFI_FEATURES:-}" ]] || { echo 'GAMELINK_FFI_FEATURES must be unset for agent builds' >&2; exit 2; }
bash scripts/build-rust-ffi.sh "$target"
out="$PWD/.tools/agent-shim/$target"
mkdir -p "$out"
CGO_ENABLED=1 GOOS=linux GOARCH="$arch" CC="$cc" go build -trimpath -tags "$tags" -buildmode=c-archive -o "$out/libgamelink_agent.a" ./shim/agent

if [[ "$target" == *-musl ]]; then
    # Go assumes glibc-style argc/argv constructor arguments; musl supplies
    # none. Expose that entrypoint and start it explicitly from the Rust host.
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    "${ZIG:-zig}" ar x --output "$work" "$out/libgamelink_agent.a" go.o
    sysroot="$(rustc --print sysroot)"
    rust_host="$(rustc -vV | awk '/^host:/ {print $2}')"
    objcopy="$sysroot/lib/rustlib/$rust_host/bin/rust-objcopy"
    symbol="_rt0_${arch}_linux_lib"
    [[ "$arch" != arm64 ]] || symbol=_rt0_arm64_linux_lib
    env "LD_LIBRARY_PATH=$sysroot/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$objcopy" \
        --remove-section=.init_array --redefine-sym "$symbol=gamelink_go_runtime_init" "$work/go.o"
    env "LD_LIBRARY_PATH=$sysroot/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$objcopy" \
        --globalize-symbol=gamelink_go_runtime_init "$work/go.o"
    "${ZIG:-zig}" ar r "$out/libgamelink_agent.a" "$work/go.o"
    rm -rf "$work"
    trap - EXIT
fi
