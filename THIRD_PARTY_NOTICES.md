# Third-party notices

GameLink itself is licensed under the GNU General Public License v3.0 (see [LICENSE](LICENSE)).

GameLink is built on open-source software. This file lists what is compiled
into the shipped binaries and what each component's license is. Components include permissive licenses, MPL-2.0 and BSL-1.0;
exact declarations and source availability are recorded below. The full license text and copyright notice of
every component is in [`third_party/licenses/`](third_party/licenses/), laid
out by package path. Anyone distributing a GameLink binary must ship that
directory (or its contents) and this file alongside it, because the BSD and
MIT licenses require the notice to travel with binaries.

The most important dependencies:

- **Tailcat** (`github.com/tailscale/tailcat`) and **tailscale.com**, BSD-3-Clause,
  Copyright (c) Tailscale Inc & AUTHORS. The encrypted transport (WireGuard
  with DERP relay fallback) that GameLink uses.
- **wireguard-go** (`github.com/tailscale/wireguard-go`), MIT, Copyright (c)
  WireGuard LLC.
- **gVisor** (`gvisor.dev/gvisor`), Apache-2.0 / MIT / BSD-3-Clause, the
  user-space network stack.
- **Tauri 2**, MIT OR Apache-2.0, the Rust desktop shell; native webviews use
  GTK/WebKitGTK on Linux and Microsoft WebView2 on Windows.
- **React**, MIT, Copyright (c) Meta Platforms, Inc. and affiliates.
- **Lucide** (`lucide-react`), ISC, with portions under MIT (Feather).
- **go-nethernet** (`github.com/df-mc/go-nethernet`), MIT, Copyright (c)
  2019 Dragonfly Tech. The LAN discovery codec adaptation lives in
  `core/crates/gamelink-core/src/discovery.rs`; Go compatibility types and
  the original license/notice remain in `shim/consolebridge/discovery`.

"WireGuard" is a registered trademark of Jason A. Donenfeld. GameLink is not
affiliated with or endorsed by Tailscale, WireGuard or Microsoft.

This list was generated with
[`google/go-licenses`](https://github.com/google/go-licenses) against the
exact production build tags in `build-tags.txt`, so it reflects what is really
linked into the binaries. Regenerate it whenever dependencies change:

```sh
export GOFLAGS="-tags=$(cat build-tags.txt)"
GOOS=linux   go-licenses report ./apps/server-agent
GOOS=windows go-licenses report ./shim/desktop
# license texts: `go-licenses save <pkg> --save_path=<dir>` for each, merged into third_party/licenses/
```

## Server agent (`gamelink-agent`, Linux)

28 Go packages: 6 x Apache-2.0, 1 x BSD-2-Clause, 15 x BSD-3-Clause, 10 x MIT.

| Package | License |
|---|---|
| `github.com/fxamacker/cbor/v2` | MIT |
| `github.com/gaissmai/bart` | MIT |
| `github.com/go-json-experiment/json` | BSD-3-Clause |
| `github.com/godbus/dbus/v5` | BSD-2-Clause |
| `github.com/golang/groupcache/lru` | Apache-2.0 |
| `github.com/google/btree` | Apache-2.0 |
| `github.com/jsimonetti/rtnetlink` | MIT |
| `github.com/klauspost/compress` | Apache-2.0, BSD-3-Clause, MIT |
| `github.com/klauspost/compress/internal/snapref` | BSD-3-Clause |
| `github.com/klauspost/compress/zstd/internal/xxhash` | MIT |
| `github.com/mdlayher/netlink` | MIT |
| `github.com/mdlayher/socket` | MIT |
| `github.com/safchain/ethtool` | Apache-2.0 |
| `github.com/tailscale/tailcat` | BSD-3-Clause |
| `github.com/tailscale/wireguard-go` | MIT |
| `github.com/x448/float16` | MIT |
| `go4.org/mem` | Apache-2.0 |
| `go4.org/netipx` | BSD-3-Clause |
| `golang.org/x/crypto` | BSD-3-Clause |
| `golang.org/x/exp` | BSD-3-Clause |
| `golang.org/x/net` | BSD-3-Clause |
| `golang.org/x/sync/errgroup` | BSD-3-Clause |
| `golang.org/x/sys` | BSD-3-Clause |
| `golang.org/x/term` | BSD-3-Clause |
| `golang.org/x/text` | BSD-3-Clause |
| `golang.org/x/time/rate` | BSD-3-Clause |
| `gvisor.dev/gvisor/pkg` | Apache-2.0, BSD-3-Clause, MIT |
| `tailscale.com` | BSD-3-Clause |

## Desktop app (Windows), additional Go packages

These are linked into `GameLink.exe` in addition to everything in the agent
list above.

| Package | License |
|---|---|
| `github.com/dblohm7/wingoes` | BSD-3-Clause |
| `github.com/go-ole/go-ole` | MIT |
| `github.com/tailscale/certstore` | MIT |
| `github.com/tailscale/go-winio` | MIT |
| `golang.zx2c4.com/wintun` | MIT |
| `golang.zx2c4.com/wireguard/windows/tunnel/winipcfg` | MIT |

## Desktop app front end (npm, production dependencies only)

| Package | Version | License |
|---|---|---|
| `@tauri-apps/api` | 2.12.4 | MIT OR Apache-2.0 |
| `lucide-react` | 1.72.0 | ISC |
| `react` | 19.3.1 | MIT |
| `react-dom` | 19.3.1 | MIT |
| `scheduler` | 0.28.0 | MIT |

Development-only tooling (Vite, TypeScript, ESLint, Vitest, Tailwind CSS and
their dependencies) is used to build the front end but is not shipped inside
the application.

The Microsoft Edge WebView2 runtime used by the Windows app is a separate
Microsoft component that is installed on the system, not distributed by
GameLink; it is governed by Microsoft's own license terms.

## Android app: shipped runtime libraries

The Android APK includes the Go mobile AAR, the GameLink Rust JNI library, and
the following direct Kotlin/AndroidX runtime dependencies. AndroidX, Kotlin and
Kotlinx Coroutines use Apache-2.0; its full text is included at
[`third_party/licenses/android/Apache-2.0`](third_party/licenses/android/Apache-2.0).
The Compose BOM pins the Compose artifact versions resolved by Gradle.

| Component | Pinned version | License |
|---|---|---|
| `golang.org/x/mobile` gomobile/bind toolchain | `v0.0.0-20261011034528-197770444867` | BSD-3-Clause; text in `third_party/licenses/golang.org/x/mobile/LICENSE` |
| Rust `jni` | 0.21.1 | MIT OR Apache-2.0 |
| Rust `serde`, `serde_core`, `serde_derive` | 1.0.228 | MIT OR Apache-2.0 |
| Rust `serde_json` | 1.0.149 | MIT OR Apache-2.0 |
| Rust `jni-sys` | 0.3.1 and 0.4.1 | MIT OR Apache-2.0 |
| Rust transitive crates in `core/Cargo.lock` | pinned by lockfile; see package/version directories in `third_party/licenses/rust/` | MIT, Apache-2.0, BSD-3-Clause, or Unicode-3.0 as declared by each crate |
| AndroidX Activity Compose | 1.13.0 | Apache-2.0 |
| AndroidX Lifecycle Runtime Compose | 2.10.0 | Apache-2.0 |
| Jetpack Compose Material 3 / UI / tooling preview | Compose BOM 2026.09.00 | Apache-2.0 |
| Kotlinx Coroutines Android | 1.10.2 | Apache-2.0 |
| Kotlin standard library | 2.2.10 | Apache-2.0 |

Rust crate license copies and notices are under `third_party/licenses/rust/`;
Unicode-ident's additional Unicode-3.0 text is included alongside its MIT and
Apache texts. `cesu8` declares Apache-2.0/MIT and documents adapted Rust core
string code; its package notice records the Rust Project attribution. These
Android copies are retained even where a dependency is also covered by a dual
license so the notice set is complete for the shipped native library.

The shared core also uses `base64` 0.22.1, `regex` 1.12.2, `tempfile` 3.24.0,
`chrono` 0.4.44, `getrandom` 0.3.4, `zeroize` 1.8.2, and Windows `windows`
0.62.2. The NetherNet LAN discovery wire codec uses RustCrypto `aes` 0.8.4,
`hmac` 0.12.1, and `sha2` 0.10.9 (MIT OR Apache-2.0). Their additional
transitive crates are `block-buffer` 0.10.4, `cipher` 0.4.4, `cpufeatures`
0.2.17, `crypto-common` 0.1.7, `digest` 0.10.7, `generic-array` 0.14.7,
`inout` 0.1.4, `subtle` 2.6.1, `typenum` 1.20.1, and `version_check` 0.9.5.
`generic-array` is MIT; `subtle` is BSD-3-Clause; the others offer MIT or
Apache-2.0. Actual upstream license files are included in
`third_party/licenses/rust/<crate>-<version>/`. The Rust discovery adaptation
retains Dragonfly Tech's attribution and MIT text in
`core/crates/gamelink-core/src/discovery-NOTICE` and `discovery-LICENSE`.

Their dependency versions are
pinned in `core/Cargo.lock`; license texts are copied into the corresponding
package/version directories. Except as identified above, these dependencies offer MIT or Apache-2.0
terms (and `aho-corasick` also offers the Unlicense). Target-specific
dependencies with additional license alternatives are used under their MIT
or Apache-2.0 alternative.

## Android build and test tooling (not bundled in the APK)

| Tool | Pinned version | License / distribution |
|---|---|---|
| Rust compiler and Cargo | 1.100.0 | Apache-2.0 or MIT; toolchain distribution terms |
| Android Gradle Plugin | 9.5.1 | Apache-2.0 |
| Kotlin Compose compiler plugin | 2.2.10 | Apache-2.0 |
| Gradle wrapper | 9.9.1 | Apache-2.0 |
| Android SDK platform / build tools | API 37 / build tools 36.0.0 | Google Android SDK license |
| Android NDK | 29.0.14206865 | Google NDK terms; contains LLVM/Clang components under their own licenses |

These compilers, SDKs and build tools are developer/CI prerequisites, not
libraries redistributed in the APK. AndroidX Test Runner 1.7.0, AndroidX Test
JUnit 1.3.0, and debug-only Compose tooling are test/development dependencies;
they are not part of the release runtime. The app is currently distributed only
as a locally built debug APK; there is no app-store release.

To review the exact Rust Android dependency graph use:

```sh
cargo tree --manifest-path core/Cargo.toml --locked --target aarch64-linux-android
```

To review the resolved Kotlin runtime graph use:

```sh
cd apps/android
./gradlew :app:dependencies --configuration debugRuntimeClasspath
```

The supervisor reducer uses RustCrypto `sha2` 0.10.9 for compatibility with
the original SHA-256 config fingerprint. Its pinned transitive dependencies
and MIT/Apache-2.0 license copies are included under `third_party/licenses/rust/`.

## Tauri desktop Rust dependencies

The standalone desktop lockfile is `apps/desktop-tauri/Cargo.lock`. The full
resolved crate/version/license inventory (including build-time and other-target
crates) is [DESKTOP_DEPENDENCIES.md](third_party/licenses/rust/DESKTOP_DEPENDENCIES.md).
Actual crate license/copyright/notice files are copied to versioned directories
under `third_party/licenses/rust/`; when a crate omits its workspace license,
`UPSTREAM_SOURCE.md` records the exact upstream revision used. `r-efi` carries
its MIT permission and copyright notice in AUTHORS, copied unchanged.

`selectors` is unmodified MPL-2.0 code used by the Tauri HTML/codegen dependency
graph. Source code form is available from
https://crates.io/api/v1/crates/selectors/0.38.0/download and
https://crates.io/api/v1/crates/selectors/0.41.0/download. Its MPL text and source
notice are included in the corresponding crate directories. BSL-1.0 applies
to `clipboard-win`; its upstream license is included. Alternative-license
crates are used under their MIT/Apache-2.0 terms where available.

Windows GNU artifacts include `WebView2Loader.dll` supplied by
`webview2-com-sys`, byte-identical to Microsoft.Web.WebView2 SDK 1.0.3800.47.
Its Microsoft redistributable SDK terms are copied unchanged under
`third_party/licenses/microsoft/WebView2-SDK-1.0.3800.47/`, with the source
package URL and DLL hash. The Rust bindings have their own MIT license. The
Microsoft WebView2 system runtime remains separately installed. Linux GTK,
WebKitGTK, JavaScriptCore and system libraries are dynamically linked platform
prerequisites, not copied into the application artifact.

Desktop presence notifications on Linux use `tauri-plugin-notification`
and `notify-rust`, under their MIT or Apache-2.0 terms. The Windows
notification area uses the existing MIT/Apache-2.0 `windows` bindings.
The desktop installer updater uses `tauri-plugin-updater` under its MIT or
Apache-2.0 terms.

To refresh the conservative Rust inventory and copies:

```sh
cargo metadata --manifest-path apps/desktop-tauri/Cargo.toml --locked --format-version 1 > /tmp/desktop-metadata.json
python3 scripts/collect-desktop-licenses.py /tmp/desktop-metadata.json
```

The collector reports missing packaged licenses for explicit upstream retrieval;
it never fabricates license text. Frontend API license copies are under
`third_party/licenses/npm/@tauri-apps/api/`. Historical unused dependency notices
may remain in the shared notices directory and are not evidence of linkage.

### Linux Rust agent CLI

The Linux agent is a Rust clap host linked to a Go Tailcat C archive. Its
additional crates include clap/clap_builder/clap_lex (MIT OR Apache-2.0),
anstyle (MIT OR Apache-2.0), ctrlc (MIT OR Apache-2.0), nix (MIT), and
cfg_aliases (MIT). Actual upstream crate license and notice files are copied
under their versioned directories in `third_party/licenses/rust/`.
[Agent and core inventory](third_party/licenses/rust/AGENT_DEPENDENCIES.md)
is resolved from `core/Cargo.lock` and conservatively includes all targets,
including build-time and non-Linux dependencies. Existing Go/Tailcat and Rust
core notices continue to apply. Refresh with `cargo metadata --manifest-path
core/Cargo.toml --locked --format-version 1` and
`scripts/collect-agent-licenses.py <metadata.json>`.

The desktop and Linux agent release checks use `minisign-verify` 0.3.0 (MIT),
`semver` 1.0.28 (MIT OR Apache-2.0), and `ureq` 3.5.2 (MIT OR Apache-2.0)
with Rustls and Web PKI roots. The resolved inventories above list the
transitive crates and link to their license texts. The core verification
tests use `minisign` 0.10.0 (MIT) to generate ephemeral keys and signatures.
