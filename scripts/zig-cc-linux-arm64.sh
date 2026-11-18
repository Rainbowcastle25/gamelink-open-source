#!/usr/bin/env bash
# Musl C compiler/linker for the fully static Linux agent. Set ZIG if needed.
set -euo pipefail
# cc-rs passes a Rust target spelling that Zig does not accept.
args=()
for arg in "$@"; do
    [[ "$arg" == --target=aarch64-unknown-linux-musl ]] || args+=("$arg")
done
exec "${ZIG:-zig}" cc -target aarch64-linux-musl "${args[@]}"
