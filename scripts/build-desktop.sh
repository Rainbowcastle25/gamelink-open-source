#!/usr/bin/env bash
# Prebuild archives BEFORE Cargo: invoking Cargo from build.rs deadlocks its
# target lock when the Go shim itself depends on the Rust FFI archive.
set -euo pipefail
cd "$(dirname "$0")/.."
archives_only=false
if [[ "${1:-}" == --archives-only ]]; then archives_only=true; shift; fi
target="${1:-$(rustc -vV | awk '/^host:/ {print $2}')}"
case "$target" in
  x86_64-unknown-linux-gnu) goos=linux; cc="${CC:-cc}" ;;
  x86_64-pc-windows-gnu) goos=windows; cc="$PWD/scripts/zig-cc-windows-amd64.sh" ;;
  *) echo "unsupported desktop target: $target" >&2; exit 2 ;;
esac
[[ -z "${GAMELINK_FFI_FEATURES:-}" ]] || { echo "desktop releases cannot enable FFI probe features" >&2; exit 2; }
bash scripts/build-rust-ffi.sh "$target"
archive="$PWD/.tools/desktop-shim/$target"
mkdir -p "$archive"
CGO_ENABLED=1 GOOS="$goos" GOARCH=amd64 CC="$cc" go build \
  -tags "$(tr -d '\r\n' < build-tags.txt)" -buildmode=c-archive \
  -o "$archive/libgamelink_desktop.a" ./shim/desktop
if $archives_only; then exit 0; fi
if [[ "$target" == x86_64-pc-windows-gnu ]]; then
    # GNU Tauri needs MinGW windres and linker (WebView2 import library/CRT).
    # The Go archive continues using the proven Zig cgo toolchain.
    command -v x86_64-w64-mingw32-gcc >/dev/null || { echo "install MinGW-w64 gcc and binutils for the desktop Rust host" >&2; exit 2; }
    export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER:-x86_64-w64-mingw32-gcc}"
    export CC_x86_64_pc_windows_gnu="$cc"
    export RUSTFLAGS="${RUSTFLAGS:-} -C dlltool=$PWD/scripts/zig-dlltool.sh"
fi
(cd apps/desktop/frontend && npm ci && npm run build && touch dist/.gitkeep)
cargo build --manifest-path apps/desktop-tauri/Cargo.toml --locked --release --target "$target"

output="apps/desktop-tauri/target/$target/release"
mkdir -p "dist/desktop/$target"
if [[ "$goos" == windows ]]; then
    cp "$output/gamelink-desktop.exe" "dist/desktop/$target/GameLink.exe"
    # GNU webview2-com uses the vendor DLL; ship it next to the executable.
    loader="$(find "$output/build" -path '*/out/x64/WebView2Loader.dll' -print -quit)"
    [[ -n "$loader" ]] || { echo "WebView2 loader missing" >&2; exit 1; }
    cp "$loader" "dist/desktop/$target/"
else
    cp "$output/gamelink-desktop" "dist/desktop/$target/"
fi
cp LICENSE THIRD_PARTY_NOTICES.md "dist/desktop/$target/"
mkdir -p "dist/desktop/$target/licenses"
cp -R third_party/licenses/. "dist/desktop/$target/licenses/"
