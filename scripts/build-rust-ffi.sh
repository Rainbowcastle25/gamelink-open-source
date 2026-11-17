#!/usr/bin/env bash
# Build the gamelink-ffi Rust library (staticlib + cdylib) that Go links via cgo.
#
#   scripts/build-rust-ffi.sh [triple...]   build for each target triple
#                                           (default: x86_64-unknown-linux-gnu)
#   scripts/build-rust-ffi.sh --check-header
#                                           regenerate the C header to a temp file
#                                           and fail if the checked-in one drifted
#   scripts/build-rust-ffi.sh --write-header
#                                           regenerate the checked-in C header
#
# Supported triples: x86_64-unknown-linux-gnu, x86_64-unknown-linux-musl,
# aarch64-unknown-linux-musl, x86_64-pc-windows-gnu, aarch64-linux-android,
# x86_64-linux-android. Only the staticlib is built here
# (cargo rustc --crate-type staticlib), so no target linker is needed. Windows
# raw-dylib imports require Zig for dlltool and archive processing; the
# Android JNI cdylib is built by scripts/build-android.sh. The staticlib is built
# without the default `jni` feature: it holds only the C ABI, never the Android
# JNI entrypoint (see the feature comment in core/crates/gamelink-ffi/Cargo.toml).
#
# Rust is always built with an explicit --target so output paths are uniform:
# core/target/<triple>/release/libgamelink_ffi.a.
#
# Go links a copy of the staticlib at core/target/cgo/<triple>/libgamelink_ffi.a
# with the LLVM bitcode sections (.llvmbc/.llvmcmd) removed. Rust's prebuilt
# compiler_builtins objects embed bitcode, and cmd/go refuses to link archives
# that contain LTO sections. The bitcode is unused by the system linker, so
# removing it does not change the linked code.
#
# cmd/go does not track the contents of external cgo archives, so replacing
# the .a alone can leave a cached Go binary or test holding the old Rust code.
# After each build the script writes a git-ignored stamp file,
# shim/rustcore/zz_ffistamp_<goos>_<goarch>.go, holding the archive's
# SHA-256. Go hashes package sources, so a changed archive forces the
# rustcore package (and everything linking it, including gomobile) to rebuild.
#
# GAMELINK_FFI_FEATURES (comma-separated) enables extra cargo features, e.g.
# `panic-probe` for the test-only panic method. Never set it for release builds.
#
# The objcopy must understand the target's object format (ELF for Linux and
# Android on x86_64 and aarch64, COFF for windows-gnu). The default is the
# rust-objcopy (llvm-objcopy) that ships inside the pinned Rust toolchain, so
# install it with `rustup component add llvm-tools`; it handles every format above. Set OBJCOPY to use a
# different llvm-objcopy (for example the NDK's) instead.
set -euo pipefail
cd "$(dirname "$0")/.."

CRATE_DIR=core/crates/gamelink-ffi
HEADER="$CRATE_DIR/include/gamelink_ffi.h"
CBINDGEN_VERSION=0.29.4

generate_header() {
    if ! command -v cbindgen >/dev/null; then
        echo "cbindgen not found; install with: cargo install cbindgen --version =$CBINDGEN_VERSION --locked" >&2
        exit 1
    fi
    local have
    have="$(cbindgen --version | awk '{print $2}')"
    if [[ "$have" != "$CBINDGEN_VERSION" ]]; then
        echo "cbindgen $CBINDGEN_VERSION required, found $have" >&2
        exit 1
    fi
    cbindgen --quiet --config "$CRATE_DIR/cbindgen.toml" --crate gamelink-ffi \
        --output "$1" "$CRATE_DIR"
}

case "${1:-}" in
--check-header)
    tmp="$(mktemp)"
    trap 'rm -f "$tmp"' EXIT
    generate_header "$tmp"
    if ! diff -u "$HEADER" "$tmp"; then
        echo "$HEADER is out of date; run: bash scripts/build-rust-ffi.sh --write-header" >&2
        exit 1
    fi
    echo "$HEADER is up to date"
    exit 0
    ;;
--write-header)
    generate_header "$HEADER"
    echo "wrote $HEADER"
    exit 0
    ;;
-*)
    echo "unknown option: $1" >&2
    exit 2
    ;;
esac

targets=("$@")
[[ ${#targets[@]} -gt 0 ]] || targets=(x86_64-unknown-linux-gnu)
for target in "${targets[@]}"; do
    case "$target" in
    x86_64-unknown-linux-gnu | x86_64-unknown-linux-musl | aarch64-unknown-linux-musl | x86_64-pc-windows-gnu | aarch64-linux-android | x86_64-linux-android) ;;
    *) echo "unsupported target: $target" >&2; exit 2 ;;
    esac
done

objcopy_cmd=()
if [[ -n "${OBJCOPY:-}" ]]; then
    objcopy_cmd=("$OBJCOPY")
else
    sysroot="$(rustc --print sysroot)"
    # Git Bash needs POSIX paths for file checks; rustc prints a Windows path.
    if [[ "${OS:-}" == Windows_NT ]]; then
        sysroot="$(cygpath -u "$sysroot")"
    fi
    host="$(rustc -vV | awk '/^host:/ {print $2}')"
    rust_objcopy="$sysroot/lib/rustlib/$host/bin/rust-objcopy"
    [[ "${OS:-}" != Windows_NT ]] || rust_objcopy+=".exe"
    [[ -x "$rust_objcopy" ]] || { echo "rust-objcopy not found at $rust_objcopy; run rustup component add llvm-tools or set OBJCOPY to an llvm-objcopy" >&2; exit 1; }
    # rust-objcopy links the toolchain's libLLVM, which lives in <sysroot>/lib
    # and is not on its rpath.
    objcopy_cmd=(env "PATH=$sysroot/bin:$sysroot/lib:$PATH" "LD_LIBRARY_PATH=$sysroot/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$rust_objcopy")
fi

for target in "${targets[@]}"; do
    features=()
    [[ -z "${GAMELINK_FFI_FEATURES:-}" ]] || features=(--features "$GAMELINK_FFI_FEATURES")
    # The windows crate generates raw-dylib import libraries even for a
    # staticlib. Use Zig's dlltool rather than requiring a MinGW installation.
    cargo_env=()
    if [[ "$target" == x86_64-pc-windows-gnu ]]; then
        dlltool="$PWD/scripts/zig-dlltool.sh"
        if [[ "${OS:-}" == Windows_NT ]]; then
            dlltool="$(cygpath -m "$PWD/scripts/zig-dlltool.cmd")"
        fi
        rust_flags="${CARGO_ENCODED_RUSTFLAGS:-}"
        if [[ -z "$rust_flags" && -n "${RUSTFLAGS:-}" ]]; then
            # Match Cargo's whitespace splitting of ordinary RUSTFLAGS.
            read -r -a flags <<< "$RUSTFLAGS"
            printf -v rust_flags '%s\x1f' "${flags[@]}"
            rust_flags="${rust_flags%$'\x1f'}"
        fi
        rust_flags+="${rust_flags:+$'\x1f'}-C"$'\x1f'"dlltool=$dlltool"
        cargo_env=(env "CARGO_ENCODED_RUSTFLAGS=$rust_flags")
    fi
    "${cargo_env[@]}" cargo rustc --manifest-path core/Cargo.toml --locked --release -p gamelink-ffi --lib \
        --no-default-features "${features[@]}" --crate-type staticlib --target "$target"
    out="core/target/cgo/$target"
    mkdir -p "$out"
    if [[ "$target" == x86_64-pc-windows-gnu ]]; then
        # raw-dylib creates COFF short import members (*.dll), which objcopy
        # cannot read. Preserve them in the original archive; replace only
        # ordinary objects. Extracting duplicate DLL names is harmless because
        # those extracted files are never reinserted.
        work="$(mktemp -d)"
        trap 'rm -rf "$work"' EXIT
        "${ZIG:-zig}" ar x --output "$work" "core/target/$target/release/libgamelink_ffi.a"
        work_native="$work"
        [[ "${OS:-}" != Windows_NT ]] || work_native="$(cygpath -m "$work")"
        : > "$work/objects.rsp"
        for object in "$work"/*.o; do
            "${objcopy_cmd[@]}" --remove-section=.llvmbc --remove-section=.llvmcmd "$object"
            printf '"%s/%s"\n' "$work_native" "${object##*/}" >> "$work/objects.rsp"
        done
        cp "core/target/$target/release/libgamelink_ffi.a" "$out/libgamelink_ffi.a.tmp"
        "${ZIG:-zig}" ar r "$out/libgamelink_ffi.a.tmp" "@$work_native/objects.rsp"
        rm -rf "$work"
        trap - EXIT
    else
        "${objcopy_cmd[@]}" --remove-section=.llvmbc --remove-section=.llvmcmd \
            "core/target/$target/release/libgamelink_ffi.a" "$out/libgamelink_ffi.a.tmp"
    fi
    mv "$out/libgamelink_ffi.a.tmp" "$out/libgamelink_ffi.a"
    case "$target" in
    x86_64-unknown-linux-gnu | x86_64-unknown-linux-musl) goplat=linux_amd64 ;;
    aarch64-unknown-linux-musl) goplat=linux_arm64 ;;
    x86_64-pc-windows-gnu) goplat=windows_amd64 ;;
    aarch64-linux-android) goplat=android_arm64 ;;
    x86_64-linux-android) goplat=android_amd64 ;;
    esac
    sum="$(sha256sum "$out/libgamelink_ffi.a" | awk '{print $1}')"
    printf '// Code generated by scripts/build-rust-ffi.sh. DO NOT EDIT.\n\npackage rustcore\n\n// SHA-256 of core/target/cgo/%s/libgamelink_ffi.a; changes force a cgo relink.\nconst _ = "%s"\n' \
        "$target" "$sum" >"shim/rustcore/zz_ffistamp_$goplat.go"
    echo "built $out/libgamelink_ffi.a"
done
