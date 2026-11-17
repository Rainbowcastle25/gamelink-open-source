#!/usr/bin/env bash
# Rust windows-gnu raw-dylib imports use Zig's bundled llvm-dlltool.
set -euo pipefail
exec "${ZIG:-zig}" dlltool "$@"
