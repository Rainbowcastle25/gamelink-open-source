#!/usr/bin/env bash
# C compiler/linker for cross-compiling Go with cgo to windows/amd64 from
# Linux, using zig's bundled clang + MinGW-w64 headers and libraries:
#
#   CC="$PWD/scripts/zig-cc-windows-amd64.sh" CGO_ENABLED=1 GOOS=windows GOARCH=amd64 go build ...
#
# cmd/go splits CC on spaces, so `zig cc -target ...` is wrapped in a script.
# Set ZIG to the zig binary if it is not on PATH. See docs/building.md.
set -euo pipefail
exec "${ZIG:-zig}" cc -target x86_64-windows-gnu "$@"
