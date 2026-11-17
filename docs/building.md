# Building, testing and releasing

## Toolchain

| Tool | Version used |
|---|---|
| Go | 1.27.x (see `go.mod`) |
| Node.js | 22 LTS |
| Desktop shell | Tauri 2; pinned standalone `apps/desktop-tauri/Cargo.lock` |
| Desktop Windows GNU | MinGW-w64 gcc/binutils (including windres); Zig remains the Go cgo compiler |
| Desktop Linux | GTK 3 and WebKitGTK 4.1 development packages |
| Windows | WebView2 runtime (preinstalled on current Windows 11) |
| Android Go binding | Go 1.27.4; gomobile version pinned in `scripts/build-android.sh` |
| Android Rust | Rust 1.100; `aarch64-linux-android` and `x86_64-linux-android` targets |
| Android Java/SDK | JDK 21, Android SDK platform 37, NDK 29.0.14206865 |
| Android Gradle | Checked-in Gradle 9.9.1 wrapper; Android Gradle Plugin 9.5.1 |
| Rust core (host) | Rust 1.100 with `llvm-tools`; `x86_64-unknown-linux-gnu` target |
| Linux static agent | Rust `x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl` targets; Zig 0.15.1 |
| Rust core (Windows cross) | Rust 1.100 `x86_64-pc-windows-gnu` target; Zig 0.15.1 as the C compiler/linker |
| cbindgen | 0.29.4: `cargo install cbindgen --version =0.29.4 --locked` (only to regenerate or check the C header) |

## The two tag sets (important)

Production binaries are built with the tags in [`build-tags.txt`](../build-tags.txt)
(a single comma-separated line):

```sh
go build -tags "$(cat build-tags.txt)" ...
```

It is Tailcat's own release list (omits Tailscale features GameLink does not
use, including port mapping, taildrop, serve, exit-node/route features) plus
`ts_omit_ssh`, which removes Tailcat's SSH server and SFTP code.

**Tests must be built without those tags.** The hermetic DERP/STUN servers used
by the integration tests (`tailscale.com/tstest/integration`) need code the tags
remove. So `go test ./...` is always run plain, and the production check is a
separate step (`scripts/check-forbidden-deps.sh`) that inspects what the
production build would link.

## Server agent (Linux)

Install the Rust targets and LLVM tools once, then build with Zig on `PATH`
(or set `ZIG` to its executable):

```sh
rustup component add llvm-tools
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
bash scripts/build-agent.sh amd64              # dist/gamelink-agent-linux-amd64
bash scripts/build-agent.sh arm64              # dist/gamelink-agent-linux-arm64
```

The script builds the Rust core C ABI, then a Go `c-archive` through Zig's
musl toolchain with `CGO_ENABLED=1`, `netgo,osusergo,rust_musl` and the
production hardening tags. Cargo links that archive into the Rust clap host.
Rust's bundled LLD and musl supply the final static runtime; static relocation
avoids mixing PIE startup with Go's runtime. The script preserves ARM's
Cortex-A53 workaround. Because musl does not pass argc/argv to constructors,
the Go archive's constructor is removed and exposed as an explicit, once-only
startup call with a process-lifetime argument/environment vector. No Go pointers cross
the ABI. Windows agents are not shipped; Android's separate JNI/gomobile path
is unchanged.

For native development and subprocess tests:

```sh
bash scripts/build-agent-shim.sh x86_64-unknown-linux-gnu
cargo test --manifest-path core/Cargo.toml --locked
cargo clippy --manifest-path core/Cargo.toml --locked --all-targets -- -D warnings
GAMELINK_AGENT_BINARY="$PWD/core/target/debug/gamelink-agent" go test -count=1 ./tests/integration
```

Build the archive before workspace Cargo checks (its build script reports the
required command if absent). Check shipped binaries with `file` and `readelf`:
there must be no `INTERP` header or `NEEDED` library. CI runs CLI flows on both
architectures (`qemu-aarch64` for ARM), and runs the actual Rust agent against a
hermetic local DERP to verify echo, revocation, fail-closed config and SIGTERM.
See [phase 7 design](rust-phase7.md) for the final resource boundary.

## Desktop app (Tauri, Windows and Linux)

The Rust host in `apps/desktop-tauri` embeds the existing React frontend in
`apps/desktop/frontend`. It is a separate Cargo workspace: core/Android builds
do not resolve GUI dependencies. No Wails CLI, generated bindings or runtime
are used. Native clipboard, program chooser, configuration-folder opening and
single-instance focus are implemented by Tauri. The original 1280×800 dark
window and 1000×680 minimum size are preserved.

Install the toolchains above and, on Linux, GTK 3 and WebKitGTK 4.1 development
packages (`libgtk-3-dev libwebkit2gtk-4.1-dev librsvg2-dev` on Ubuntu). Build:

```sh
bash scripts/build-desktop.sh x86_64-unknown-linux-gnu
# On Linux with MinGW-w64 gcc/binutils and the Rust GNU Windows target:
bash scripts/build-desktop.sh x86_64-pc-windows-gnu
```

The script builds the release Rust FFI with JNI and panic probes disabled,
then the Go c-archive with `build-tags.txt`, then the frontend, then the Rust
host. **Do not build archives from Cargo build.rs:** the Go shim depends on
Rust FFI, and nested Cargo would deadlock its target lock. `build.rs` only
checks and links existing archives. `--archives-only <triple>` prebuilds them
for development/tests. Zig builds the Go Windows archive; MinGW links the
Tauri GNU host and compiles its Windows resources. Set
`CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` to override the MinGW linker.

Artifacts and notices are staged in `dist/desktop/<triple>/`. On Windows ship
`GameLink.exe` **and `WebView2Loader.dll` together**, with the copied notices and
licenses. The system WebView2 runtime is also required. This is an unsigned
application, not an installer or public release.

```sh
cargo fmt --manifest-path apps/desktop-tauri/Cargo.toml --check
cargo clippy --manifest-path apps/desktop-tauri/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path apps/desktop-tauri/Cargo.toml --locked
# Native smoke testing must use private throwaway state:
GAMELINK_HOME="$(mktemp -d)" apps/desktop-tauri/target/x86_64-unknown-linux-gnu/release/gamelink-desktop
```

For browser development use `VITE_MOCK=1 npm run dev` in the frontend; mock
code is excluded from production builds. A native build uses the real Tauri
commands. The parent integration UI playthrough and native Windows launch,
clipboard, chooser, folder opening and game execution remain runtime checks;
a cross-build does not establish Windows GUI parity.

## Android Console Bridge

Build on Linux x86_64 with the versions listed above. Set `ANDROID_HOME` to an
SDK installation containing platform 37 and NDK 29.0.14206865, and `JAVA_HOME`
to JDK 21. For arm64 and x86_64, the script builds the Rust staticlibs that cgo
links into the Go mobile AAR, then the AAR, then the Rust JNI library. After
that it builds the debug APK and runs Android lint:

```sh
export ANDROID_HOME=/path/to/android-sdk
export JAVA_HOME=/path/to/jdk-21
./scripts/build-android.sh
```

The generated APK is `apps/android/app/build/outputs/apk/debug/app-debug.apk`.
For local evaluation, install it with:

```sh
adb install -r apps/android/app/build/outputs/apk/debug/app-debug.apk
```

This debug APK is not a signed store release. No app-store package or public
deployment exists. The app targets API 37 and requests runtime
`ACCESS_LOCAL_NETWORK` permission on API 37. The API 37 16 KiB emulator
instrumentation suite passed with this permission pre-granted; its interactive
permission prompt and physical console testing remain separate checks.

The bridge requires the phone's active network to be the selected Wi-Fi with a
private IPv4 address. Loss or change of that network/interface stops the
service and requires an explicit restart. Tailcat disconnects retry with
backoff while the service remains active. Process death is non-sticky and does
not restart the bridge. Xbox and PlayStation gameplay, screen-off behavior,
and battery/thermal effects remain hardware checks, not build checks.

## Rust core and the Go↔Rust boundary

`core/` is a Cargo workspace with one shared `Cargo.lock`:

- `core/crates/gamelink-core`: shared logic (Android bridge lifecycle,
  invitation and identity formats, configuration, validation and redaction),
  plus atomic storage and Windows DPAPI. An `rlib` with no C ABI.
- `core/crates/gamelink-ffi`: a `staticlib` + `cdylib` that wraps the core. It
  exports the Android JNI entrypoint (Android loads it as `gamelink_ffi`) and a
  C ABI for Go: `gamelink_call(method, input) -> GamelinkBuffer` (JSON in, a
  `{"ok": ...}` or `{"error": {"code", "message"}}` envelope out) and
  `gamelink_buffer_free`. Inputs are limited to 32 KiB and method names to 64
  bytes. Error messages never echo the input.
- `shim/rustcore`: the Go cgo binding (`rustcore.Call`). The migrated Go
  invite, identity, config and security packages call it, alongside `mobile.CoreABIVersion` (Android diagnostics).

Build the static library before building or testing anything that links
`shim/rustcore`:

```sh
bash scripts/build-rust-ffi.sh                   # host: x86_64-unknown-linux-gnu
bash scripts/build-rust-ffi.sh <triple>...       # explicit targets
go test -count=1 -race ./shim/rustcore
```

Go does not track the contents of external cgo archives, so the script also
writes a git-ignored `shim/rustcore/zz_ffistamp_<goos>_<goarch>.go`
holding the archive's SHA-256. Rebuilding Rust changes that file, which makes
Go (including `gomobile bind`) relink instead of reusing a cached binary that
still contains the old Rust code. Always rebuild through the script, not with
plain `cargo build`.

`GAMELINK_FFI_FEATURES=panic-probe` adds a test-only `test.panic` method;
`TestPanicIsContained` uses it to prove a Rust panic is caught at the C ABI
on each platform's unwinder (CI requires it on the cross-built Windows
binary). Never set it for release builds.

Only the staticlib is built (`cargo rustc --crate-type staticlib`), and it
is built with `--no-default-features`, so it holds the C ABI but not the
Android JNI entrypoint (the `jni` feature, on by default for the cdylib and
for `cargo test`/`clippy`). The script always passes `--target`, so output goes to
`core/target/<triple>/release/`. It then copies the `.a` to
`core/target/cgo/<triple>/libgamelink_ffi.a` with the `.llvmbc`/`.llvmcmd`
sections removed. cgo links that copy by full path, so the `.so` built next to
it is never picked. The strip is needed because Rust's prebuilt
`compiler_builtins` objects embed LLVM bitcode, and `cmd/go` refuses to link
archives that contain LTO sections. The system linker ignores that bitcode, so
removing it doesn't change the linked code. The strip uses the
`rust-objcopy` (llvm-objcopy) inside the Rust toolchain, which handles ELF and
COFF for every supported triple; set `OBJCOPY` to use another llvm-objcopy.
Windows raw-dylib import members are preserved unchanged: the script strips
ordinary object members individually and replaces them with `zig ar`.
With `CGO_ENABLED=0`, `rustcore.Call` returns an `unavailable` error.
Application builds require cgo and Rust because configuration and security
operations now depend on the Rust core.

The C header `core/crates/gamelink-ffi/include/gamelink_ffi.h` is generated by
cbindgen (configured in `core/crates/gamelink-ffi/cbindgen.toml`) and checked
in. After changing the exported ABI, regenerate it. The check mode fails if the
checked-in header has drifted:

```sh
bash scripts/build-rust-ffi.sh --write-header
bash scripts/build-rust-ffi.sh --check-header
```

### Cross-compiling Go + Rust (cgo)

Supported cgo targets for `shim/rustcore`: linux/amd64 (GNU or musl), linux/arm64 (musl), windows/amd64,
android/arm64 and android/amd64. The `#cgo` lines in
`shim/rustcore/rustcore.go` link the matching
`core/target/cgo/<triple>/libgamelink_ffi.a`.

**Windows (from Linux, with Zig).** Zig's bundled clang, lld and MinGW-w64
libraries serve as the C toolchain; no MinGW install or Windows machine is
needed to build. The Rust staticlib build also uses `zig dlltool` to generate
Windows raw-dylib imports (via the checked-in shell / CMD wrappers), so Zig
must be available before building Rust. `scripts/zig-cc-windows-amd64.sh` wraps
`zig cc -target x86_64-windows-gnu` because cmd/go splits `CC` on spaces
(set `ZIG` if `zig` is not on `PATH`):

```sh
rustup target add x86_64-pc-windows-gnu
bash scripts/build-rust-ffi.sh x86_64-pc-windows-gnu
CGO_ENABLED=1 GOOS=windows GOARCH=amd64 CC="$PWD/scripts/zig-cc-windows-amd64.sh" \
  go test -c -o rustcore.test.exe ./shim/rustcore
```

Three link details for the windows-gnu staticlib, all resolved in the
`#cgo windows,amd64` line, without `--allow-multiple-definition`:

- *Unwinder.* rustc's windows-gnu target spec adds `-lgcc_eh` (and
  winpthreads) itself, so they are not in `--print native-static-libs`, and
  zig ships no libgcc. Zig's LLVM libunwind (`-lunwind`) is used instead. It
  implements the same SEH-based `_GCC_specific_handler`/`_Unwind_*` ABI as
  libgcc_eh, which Rust's panic runtime calls (llvm-mingw does the same).
- *`-lsynchronization`* (for `WaitOnAddress`) is the MinGW-w64 alias of the
  `api-ms-win-core-synch-l1-2-0` import library. Zig only ships the API-set
  name, so that name is used. The binary imports `api-ms-win-core-synch-l1-2-0.dll`.
- *Compiler builtins* (`___chkstk_ms`, `__udivti3`, ...). Both Rust's
  `compiler_builtins` (strong symbols, one per archive member) and zig's
  compiler-rt (weak symbols) define them, so the two never clash and
  `-lgcc` is not needed.

Zig links the UCRT (`api-ms-win-crt-*.dll`, present on Windows 10 and
later). The desktop Go archive (`shim/desktop`) imports `rustcore` through
configuration and security packages. Its Windows build uses this same GNU
archive and Zig C toolchain; the Rust Tauri executable is linked with MinGW.

**Android (NDK + gomobile).** Point `CC` at the NDK clang for the ABI and API
level:

```sh
bash scripts/build-rust-ffi.sh aarch64-linux-android x86_64-linux-android
NDK_BIN=$ANDROID_HOME/ndk/29.0.14206865/toolchains/llvm/prebuilt/linux-x86_64/bin
GOOS=android GOARCH=amd64 CGO_ENABLED=1 CC=$NDK_BIN/x86_64-linux-android26-clang \
  go test -c -o rustcore.android-amd64.test ./shim/rustcore
adb push rustcore.android-amd64.test /data/local/tmp/
adb shell 'cd /data/local/tmp && chmod 755 rustcore.android-amd64.test && ./rustcore.android-amd64.test -test.v'
```

`scripts/build-android.sh` builds both Android staticlibs before
`gomobile bind`, because `mobile` imports `rustcore` (`Mobile.coreABIVersion()`,
a diagnostics call). The APK then carries two native libraries that both hold
Rust code:

- `libgojni.so` (Go + the cgo staticlib). Go's c-shared link exports every
  global symbol, including `gamelink_call` and the Rust std symbols. The
  staticlib has no JNI entrypoint (`--no-default-features`), so
  `Java_io_gamelink_app_Core_nativeReduce` is exported only by
  `libgamelink_ffi.so`.
- `libgamelink_ffi.so` (the Rust cdylib that `Core.kt` loads). It exports
  only `Java_io_gamelink_app_Core_nativeReduce`, `gamelink_call` and
  `gamelink_buffer_free`, and binds its internal symbols locally.

`System.loadLibrary` opens each library with `RTLD_LOCAL`, so the duplicate
Rust symbols in `libgojni.so` never resolve references in the other
library. Both libraries' LOAD segments are 16 KiB aligned (checked with
`llvm-readelf -l`).

**What was verified, and where.**

| Target | Built locally (Linux) | Run locally | Run in CI |
|---|---|---|---|
| linux/amd64 | yes (GNU tests and static musl agent) | yes, incl. `-race` | `go`, `race` jobs |
| linux/arm64 | yes (static musl agent) | `--help` / `--version` via QEMU | `go` job |
| windows/amd64 | yes: `rustcore.test.exe`, PE32+ x86-64 | **no** (no Windows or Wine here) | `windows-cross` builds it, `windows-run` runs it on `windows-latest` |
| android/amd64 | yes | yes, on the API 37 16 KiB x86_64 emulator (`rustcore` and `mobile` tests) | build only (`android` job) |
| android/arm64 | yes | no (no arm64 device or emulator) | build only (`android` job) |

The "CI" column lists the jobs in `.github/workflows/ci.yml` that cover each
target. The Windows binary runs only in the `windows-run` job, so it is
verified only once that job passes. The `windows-run` job executes the Rust boundary panic probe and the migrated
security, invite, identity and config test binaries, including Windows DPAPI
round trips. Cross-compilation alone does not verify Windows runtime behavior.

From `core/`: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`,
`cargo test --locked`.

## Tests

```sh
go vet ./...
go test -count=1 ./...                                   # ~2-3 minutes; no internet needed
go test -race -count=1 ./shim/... ./tests/...        # needs CGO / a C compiler
go test -tags external -run TestPublicRelay -v ./tests/integration   # real public relays (optional)
python3 scripts/measure-resources.py --go go                       # opt-in Tailcat resource profile

bash scripts/check-forbidden-deps.sh                     # no SSH/SFTP/port-mapping code in production binaries
bash scripts/scan-secrets.sh                             # no keys or invitations in the tree

cd apps/desktop/frontend
npm ci && npm run lint && npm run typecheck && npm test && npm run build
```

What the integration tests do: start a local DERP + STUN relay, start a real
`gamelink` game server (Tailcat server with the supervisor) in front of a TCP or
UDP echo server, and then connect real clients through it. They cover echo and
half-close, UDP datagram boundaries and the size limit, unapproved and revoked
keys, the empty-allowlist rule, restarts and reconnect, relay-only fallback,
port conflicts and clean disconnect, and the full desktop manager journey
(import, join, persist across restart, disconnect).

Console Bridge tests cover protocol framing, candidate policy, discovery source
and subnet filters, bounded sessions, datagram fragmentation/reassembly, and
host cleanup/revocation in hermetic tests. Emulator/native checks and physical
Xbox/PlayStation tests are tracked separately in the
[Console Bridge guide](android-console-bridge.md); do not infer console
compatibility from a successful build or echo test.

## Releasing (manual for now)

1. Update the version in the Rust desktop host / agent, and `CHANGELOG` notes.
2. Run everything above on a clean checkout. CI does this on every push.
3. Build the artefacts with the commands above, compute `sha256sum`, and publish
   the checksums next to the downloads. Windows binaries are **not signed yet**;
   say so in the release notes.
4. Regenerate [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md) if dependencies changed.

### Console protocol policy

The Rust core owns candidate filtering/rewriting, signaling text, control JSON,
and NetherNet LAN discovery packet/advertisement codecs. The Go public APIs
remain adapters. Control and discovery calls have a 1 MiB serialized FFI input
cap to accommodate base64 and JSON escaping; Rust separately enforces the
64 KiB data/candidate limits, 128 KiB control frame cap, and bounded discovery
lengths. Wire byte strings use base64 across FFI, preserving non-UTF-8 bytes.

Go owns sockets, peer/source pinning, session/resource maps, control frame
length-prefix I/O (with the length guard before allocation), and all media
packet forwarding, fragmentation, and reassembly. Gameplay media datagrams
never cross FFI. Discovery packets are control traffic and may cross Rust.

Original Go wire fixtures captured before parser removal live under
`core/crates/gamelink-core/tests/fixtures/`; Rust tests check exact valid wire
bytes and rejection of malformed inputs. The discovery adaptation retains its
Dragonfly Tech MIT attribution and license in both source locations.
