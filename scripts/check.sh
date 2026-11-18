#!/usr/bin/env bash
# The same section entrypoints for agents and Linux CI. Never deploys.
set -euo pipefail
cd "$(dirname "$0")/.."
export MISE_ACTIVATE_AGGRESSIVE=1
source scripts/agent-tools.sh
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-$GAMELINK_RUST_VERSION}" RUSTUP_AUTO_INSTALL=0
if [[ ${1:-} == doctor ]]; then shift; exec bash scripts/doctor.sh "$@"; fi
sections=()
for section in "${@:-fast}"; do
    case "$section" in
        fast) sections+=(go frontend safety) ;;
        full) sections+=(go frontend safety race rust header static desktop windows-cross android playtest) ;;
        native|go|race|frontend|safety|rust|header|static|desktop|windows-cross|android|playtest) sections+=("$section") ;;
        *) echo "Unknown section $section (see docs/agent-environment.md)" >&2; exit 2 ;;
    esac
done
# Do not let inherited cross/test settings change a native check or enable the
# optional public-relay test. Test code creates its own keys/state/relay.
unset GOOS GOARCH GAMELINK_FFI_FEATURES GAMELINK_AGENT_BINARY GAMELINK_REQUIRE_PANIC_PROBE
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_TARGET CARGO_TARGET_DIR
unset GAMELINK_ANDROID_FIXTURE GAMELINK_RESOURCE_PROFILE GAMELINK_TEST_TAILCAT_LOGS
export CGO_ENABLED=1 GOFLAGS= GOTOOLCHAIN=local
bash scripts/doctor.sh "${sections[@]}"
out="$PWD/.tools/checks/$(date -u +%Y%m%dT%H%M%SZ)-$$"
mkdir -p "$out"
echo "Check logs: $out"
index=0
run() {
    index=$((index + 1))
    local log="$out/$section-$index.log" started=$SECONDS code=0
    printf 'RUN'; printf ' %q' "$@"; printf '\n'
    "$@" >"$log" 2>&1 || code=$?
    cat "$log"
    echo "$section-$index: exit $code, $((SECONDS - started))s, $log" | tee -a "$out/summary.txt"
    return "$code"
}
native_ready=false
native() {
    if ! $native_ready; then
        run bash scripts/build-agent-shim.sh x86_64-unknown-linux-gnu
        run cargo build --manifest-path core/Cargo.toml --locked -p gamelink-agent
        native_ready=true
    fi
    export GAMELINK_AGENT_BINARY="$PWD/core/target/debug/gamelink-agent"
}
for section in "${sections[@]}"; do
    case "$section" in
        native) native ;;
        go)
            native
            run go vet ./...
            run go test -count=1 -timeout 15m ./...
            ;;
        race)
            native
            run go test -race -count=1 -timeout 20m ./shim/... ./shim/agent ./tests/...
            ;;
        safety)
            run bash scripts/check-forbidden-deps.sh
            run bash scripts/scan-secrets.sh
            run python3 scripts/test-safety-gates.py
            ;;
        frontend)
            for task in lint typecheck test build; do run npm --prefix apps/desktop/frontend run "$task"; done
            # Vite empties dist; preserve the tracked placeholder in worktrees.
            touch apps/desktop/frontend/dist/.gitkeep
            run npm --prefix apps/desktop/frontend audit --omit=dev --audit-level=high
            ;;
        rust)
            native
            run cargo fmt --manifest-path core/Cargo.toml --check
            run cargo test --manifest-path core/Cargo.toml --locked
            run cargo test --manifest-path core/Cargo.toml --locked -p gamelink-ffi --features panic-probe
            run cargo clippy --manifest-path core/Cargo.toml --locked --all-targets -- -D warnings
            run cargo clippy --manifest-path core/Cargo.toml --locked -p gamelink-ffi --no-default-features -- -D warnings
            ;;
        header) run bash scripts/build-rust-ffi.sh --check-header ;;
        static)
            for arch in amd64 arm64; do
                run bash scripts/build-agent.sh "$arch"
                binary="dist/gamelink-agent-linux-$arch"
                run file "$binary"
                # Capture readelf separately: a tool failure must not look like
                # absence of dynamic dependencies in a grep pipeline.
                run readelf -l "$binary"
                if grep -q INTERP "$out/$section-$index.log"; then echo 'unexpected ELF interpreter' >&2; exit 1; fi
                run readelf -d "$binary"
                if grep -q NEEDED "$out/$section-$index.log"; then echo 'unexpected dynamic dependency' >&2; exit 1; fi
            done
            run bash scripts/test-agent-cli.sh dist/gamelink-agent-linux-amd64
            qemu=$(command -v qemu-aarch64 || command -v qemu-aarch64-static)
            run bash scripts/test-agent-cli.sh dist/gamelink-agent-linux-arm64 "$qemu"
            # The untagged Go test links the native GNU FFI, even when testing a musl subprocess.
            run bash scripts/build-rust-ffi.sh x86_64-unknown-linux-gnu
            run env GAMELINK_AGENT_BINARY="$PWD/dist/gamelink-agent-linux-amd64" go test -count=1 -run TestRustAgentSubprocess -timeout 3m ./tests/integration
            (cd dist && sha256sum gamelink-agent-linux-* | tee SHA256SUMS)
            ;;
        desktop)
            run bash scripts/build-desktop.sh x86_64-unknown-linux-gnu
            run cargo fmt --manifest-path apps/desktop-tauri/Cargo.toml --check
            run cargo clippy --manifest-path apps/desktop-tauri/Cargo.toml --locked --all-targets -- -D warnings
            run cargo test --manifest-path apps/desktop-tauri/Cargo.toml --locked
            run bash scripts/build-desktop.sh x86_64-pc-windows-gnu
            ;;
        windows-cross)
            run env GAMELINK_FFI_FEATURES=panic-probe bash scripts/build-rust-ffi.sh x86_64-pc-windows-gnu
            mkdir -p dist
            for package in rustcore invite desktop; do
                run env GOOS=windows GOARCH=amd64 CC="$PWD/scripts/zig-cc-windows-amd64.sh" go test -c -o "dist/$package.test.exe" "./shim/$package"
                run file "dist/$package.test.exe"
            done
            ;;
        android)
            run bash scripts/build-android.sh assembleDebugAndroidTest
            ndk="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/29.0.14206865}"
            run env GOOS=android GOARCH=arm64 CC="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android26-clang" go list -tags "$(cat build-tags.txt)" -deps ./mobile
            if rg '^(github.com/pkg/sftp|github.com/tailscale/gliderssh|golang.org/x/crypto/ssh|tailscale.com/ssh/|tailscale.com/net/portmapper$|tailscale.com/feature/portmapper|github.com/huin/goupnp|tailscale.com/feature/relayserver|tailscale.com/wgengine/router/osrouter)' "$out/$section-$index.log"; then
                echo 'Forbidden Android dependency' >&2
                exit 1
            else
                code=$?
                [[ $code == 1 ]] || exit "$code"
            fi
            ;;
        playtest) run harness check --project "$PWD" --json ;;
    esac
done
echo "Evidence: $out/summary.txt"
