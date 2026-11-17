#!/usr/bin/env bash
# Read-only prerequisites for the shared check sections. No installs or secrets.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/agent-tools.sh
# A rustup shim must not download a toolchain just to diagnose it.
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-$GAMELINK_RUST_VERSION}" RUSTUP_AUTO_INSTALL=0
# Old Go launchers must report their version rather than auto-fetching go.mod's.
export GOTOOLCHAIN=local
sections=("${@:-fast}")
status=0
has() { [[ " ${sections[*]} " == *" $1 "* ]]; }
need() { command -v "$1" >/dev/null || { echo "MISSING $1: $2"; status=2; }; }
exact() {
    local label="$1" expected="$2" actual
    shift 2
    if actual=$("$@" 2>&1) && [[ "$actual" == "$expected" ]]; then
        echo "OK $label: $actual"
    else
        echo "MISSING/WRONG $label: expected $expected; found ${actual:-unavailable}"
        status=2
    fi
}
target() {
    if ! rustup target list --installed 2>/dev/null | grep -qx "$1"; then
        echo "MISSING Rust target $1: owner runs rustup target add --toolchain $GAMELINK_RUST_VERSION $1"
        status=2
    fi
}
for section in "${sections[@]}"; do
    case "$section" in
        fast|full|native|go|race|frontend|safety|rust|header|static|desktop|windows-cross|android|playtest) ;;
        *) echo "Unknown section: $section" >&2; exit 2 ;;
    esac
done
if has full; then sections+=(fast race rust header static desktop windows-cross android playtest); fi
if has fast; then sections+=(go frontend safety); fi
if has go || has race || has rust; then sections+=(native); fi
need git 'install Git on the host'
need bash 'install Bash on the host'
if has native || has safety || has desktop || has static || has windows-cross || has android; then
    exact Go "go$GAMELINK_GO_VERSION" go env GOVERSION
    exact 'Go in child shell' "go$GAMELINK_GO_VERSION" bash -c 'go env GOVERSION'
    exact 'go.mod pin' "$GAMELINK_GO_VERSION" awk '/^go / {print $2}' go.mod
    need cc 'owner provisions a C compiler and libc development headers'
    need python3 'Python 3 standard library runs safety gate regressions'
fi
if has native || has header || has desktop || has static || has windows-cross || has android; then
    exact Rust "rustc $GAMELINK_RUST_VERSION" bash -c 'rustc --version | cut -d " " -f 1-2'
    exact Cargo "cargo $GAMELINK_RUST_VERSION" bash -c 'cargo --version | cut -d " " -f 1-2'
    exact 'Rust in child shell' "rustc $GAMELINK_RUST_VERSION" bash -c 'bash -c "rustc --version" | cut -d " " -f 1-2'
    need cargo 'activate the pinned Rust toolchain (see docs/agent-environment.md)'
    if has native || has desktop || has static || has windows-cross || has android; then
        sysroot=$(rustc --print sysroot 2>/dev/null || true)
        host=$(rustc -vV 2>/dev/null | awk '/^host:/ {print $2}' || true)
        if [[ ! -x "$sysroot/lib/rustlib/$host/bin/rust-objcopy" ]]; then
            echo "MISSING llvm-tools: owner runs rustup component add --toolchain $GAMELINK_RUST_VERSION llvm-tools"
            status=2
        fi
    fi
    if has rust || has desktop; then
        need cargo-fmt 'owner provisions rustfmt for the pinned Rust toolchain'
        need cargo-clippy 'owner provisions clippy for the pinned Rust toolchain'
    fi
fi
if has frontend || has desktop; then
    exact Node "v$GAMELINK_NODE_VERSION" node --version
    exact 'Node in child shell' "v$GAMELINK_NODE_VERSION" bash -c 'node --version'
    need npm 'activate the pinned Node distribution with bundled npm'
    if has frontend && [[ ! -d apps/desktop/frontend/node_modules ]]; then
        echo 'MISSING frontend dependencies: npm --prefix apps/desktop/frontend ci'
        status=2
    fi
fi
if has header; then exact cbindgen "cbindgen $GAMELINK_CBINDGEN_VERSION" cbindgen --version; fi
if has static || has desktop || has windows-cross; then
    exact Zig "$GAMELINK_ZIG_VERSION" "${ZIG:-zig}" version
fi
if has static; then
    target x86_64-unknown-linux-musl
    target aarch64-unknown-linux-musl
    if ! command -v qemu-aarch64 >/dev/null && ! command -v qemu-aarch64-static >/dev/null; then
        echo 'MISSING qemu-aarch64 or qemu-aarch64-static: owner provisions qemu-user (ARM static agent execution)'
        status=2
    fi
    need readelf 'owner provisions binutils (static linkage verification)'
    need file 'owner provisions file (artifact inspection)'
fi
if has desktop || has windows-cross; then target x86_64-pc-windows-gnu; fi
if has windows-cross; then need file 'owner provisions file (PE artifact inspection)'; fi
if has desktop; then
    need x86_64-w64-mingw32-gcc 'owner provisions MinGW-w64 gcc'
    need x86_64-w64-mingw32-windres 'owner provisions MinGW-w64 binutils'
    if ! pkg-config --exists gtk+-3.0 webkit2gtk-4.1 librsvg-2.0; then
        echo 'MISSING Linux desktop libraries: owner provisions GTK3, WebKitGTK4.1, librsvg development packages'
        status=2
    fi
fi
if has android; then
    need rg 'owner provisions ripgrep (Android dependency exclusion gate)'
    target aarch64-linux-android
    target x86_64-linux-android
    sdk="${ANDROID_HOME:-}"
    for path in platforms/android-37.0/android.jar build-tools/36.0.0/aapt2 ndk/29.0.14206865/source.properties; do
        if [[ -z "$sdk" || ! -f "$sdk/$path" ]]; then
            echo "MISSING Android SDK $path: set ANDROID_HOME to the owner's prepared SDK"
            status=2
        fi
    done
    if [[ -z "${JAVA_HOME:-}" || ! -x "$JAVA_HOME/bin/java" ]]; then
        echo 'MISSING JDK 21: set JAVA_HOME to the prepared JDK'
        status=2
    elif ! "$JAVA_HOME/bin/java" -version 2>&1 | grep -q 'version "21\.'; then
        echo 'WRONG Java: CI uses JDK 21; select it with JAVA_HOME'
        status=2
    fi
fi
if has playtest; then need harness 'owner provides harness; GameLink is currently UNWIRED'; fi
if [[ $status != 0 ]]; then echo 'Repair: docs/agent-environment.md. Doctor makes no changes.'; fi
exit "$status"
