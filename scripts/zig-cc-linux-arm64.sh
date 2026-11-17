#!/usr/bin/env bash
# Musl C compiler/linker for the fully static Linux agent. Set ZIG if needed.
set -euo pipefail
exec "${ZIG:-zig}" cc -target aarch64-linux-musl "$@"
