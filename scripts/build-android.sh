#!/usr/bin/env bash
# Build the actual Go transport AAR and Rust JNI library before compiling UI.
#
# The Go AAR links the Rust core statically through cgo (shim/rustcore),
# so the Rust staticlibs for both Android ABIs are built first. The JNI cdylib
# (libgamelink_ffi.so) is a separate library loaded by the Kotlin code.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${ANDROID_HOME:?Set ANDROID_HOME to an Android SDK with API 37 and NDK 29.0.14206865}"
: "${JAVA_HOME:?Set JAVA_HOME to JDK 21 or newer}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/29.0.14206865}"
export PATH="$JAVA_HOME/bin:$PATH"
MOBILE_VERSION=v0.0.0-20261011034528-197770444867
TOOL_BIN="$PWD/.tools/android/bin"
mkdir -p "$TOOL_BIN" apps/android/app/libs
export GOBIN="$TOOL_BIN"
export PATH="$TOOL_BIN:$PATH"
go install "golang.org/x/mobile/cmd/gomobile@$MOBILE_VERSION"
go install "golang.org/x/mobile/cmd/gobind@$MOBILE_VERSION"
NDK_BIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK_BIN/aarch64-linux-android26-clang"
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$NDK_BIN/x86_64-linux-android26-clang"
# 16 KiB page alignment for the Rust cdylib. (The staticlibs are not linked by
# cargo; gomobile's NDK link of libgojni.so aligns LOAD segments to 16 KiB.)
export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-z,max-page-size=16384"
rustup target add aarch64-linux-android x86_64-linux-android
bash scripts/build-rust-ffi.sh aarch64-linux-android x86_64-linux-android
gomobile init
gomobile bind -target=android/arm64,android/amd64 -androidapi=26 \
    -tags "$(cat build-tags.txt)" -trimpath -ldflags='-s -w' \
    -o apps/android/app/libs/gamelink.aar ./mobile
for spec in aarch64-linux-android:arm64-v8a x86_64-linux-android:x86_64; do
    target="${spec%:*}"; abi="${spec#*:}"
    cargo build --manifest-path core/Cargo.toml --locked --release -p gamelink-ffi --target "$target"
    mkdir -p "apps/android/app/src/main/jniLibs/$abi"
    cp "core/target/$target/release/libgamelink_ffi.so" "apps/android/app/src/main/jniLibs/$abi/"
done
cd apps/android
./gradlew --no-daemon assembleDebug lintDebug "$@"
