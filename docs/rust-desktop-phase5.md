# Phase 5 implementation and local proof

Integrated on `feat/rust-tauri` above the reviewed phases 3–4 tree (`fe30f31`).
The standalone desktop lockfile includes the shared core protocol dependencies.

## Implementation

- `apps/desktop-tauri` is a standalone Tauri 2 Cargo workspace and lockfile.
  It reuses `apps/desktop/frontend` and preserves the Backend interface, mock
  mode, dimensions, native clipboard bounds, file filters/cancellation,
  configuration-folder opening and single-instance focus behavior.
- The in-process Go archive retains Manager/Tailcat ownership and public DTOs.
  See `apps/desktop-tauri/include/gamelink_desktop.h` and the architecture guide
  for JSON bounds, allocation/free, event topics and explicit ownership.
  Native host commands are allowlisted; JavaScript cannot init/shutdown the
  archive or invoke arbitrary Rust/core/process methods.
- Manager API/launch test hooks remain stable. The Go platform launchers are
  replaced by Rust `gamelink_core::launch` via the existing FFI. Windows still
  uses ShellExecute, raw command arguments and the executable's parent directory;
  Unix still splits on whitespace without quote interpretation. Existing
  Manager validation, loopback expansion and launch decisions remain intact.
- The shim owns startup checks/check-all scheduling through Manager.Check,
  avoiding Manager.CheckAll's untracked goroutine. Shutdown stops scheduling
  and waits for active calls; a current probe has the existing 20-second limit.
- Wails Go shell/build files, generated frontend bindings and runtime
  dependencies are removed. The frontend directory is retained. TypeScript
  DTOs now describe the JSON contract directly.
- The build script prebuilds hardened non-probe/no-JNI FFI and Go archives
  before desktop Cargo; build.rs only checks/links them. Windows GNU Tauri
  requires MinGW resource/link tools in addition to Zig for Go. Artifacts
  include licenses, notices and the required vendor WebView2Loader.dll.
- CI preserves the parent's `8a9f6f3` Windows runtime PowerShell fix verbatim
  (checkout, package cwd, quoted `'-test.v'`). Windows client launch tests are
  added to that existing cross-build/runtime matrix.

## Local verification (Linux x86-64)

All of the following completed successfully:

| Check | Result |
|---|---|
| Frontend lint, typecheck, tests, production build | 5 files / 60 tests passed; production bundle built |
| Production npm audit | 0 vulnerabilities |
| Core fmt, all-target Clippy with warnings denied, tests | 13 core + 13 FFI tests passed |
| FFI no-default-features Clippy and generated header check | passed; no header drift |
| Tauri fmt, all-target Clippy with warnings denied, tests | 2 tests passed, including repeated allocated-buffer calls and ownership lifecycle |
| Go vet ./... and untagged go test -count=1 -timeout 15m ./... | passed, including full integration suite |
| Go race tests for shim, client and integration | passed; shim rerun after scheduling fix |
| Production forbidden-dependency checks | agent + desktop shim on Linux/Windows passed |
| Secret scan | passed (also rerun after staging) |
| scripts/build-desktop.sh x86_64-unknown-linux-gnu | release executable and notices staged |
| scripts/build-desktop.sh x86_64-pc-windows-gnu | release PE32+ GUI x86-64 executable, loader and notices staged |
| Windows Go client test executable cross-build | PE32+ x86-64; retains the batch argument integration test |
| scripts/build-android.sh | both native ABIs, debug APK and Android lint passed |
| Actual Linux native release launch | private mktemp GAMELINK_HOME; native window, Manager initialization, second-instance rejection before storage, normal window close and exit 0 |

The Linux native check used the actual installed GTK 3/WebKitGTK 4.1 libraries
and Wayland desktop. It created only private temporary GameLink state. No real
user configuration was opened. The second process used its own empty temporary
home, which remained empty. No browser preview tools or network/backend service
were used. Production Windows imports were inspected: the extra vendor loader
is staged alongside the executable; MinGW runtime DLLs are not imported.

Useful local logs (temporary, not committed):

- `/tmp/gamelink-tauri-release-linux-final.log`
- `/tmp/gamelink-tauri-release-windows-final.log`
- `/tmp/gamelink-tauri-clippy-final.log`, `/tmp/gamelink-tauri-test-final.log`
- `/tmp/gamelink-core-phase5.log`, `/tmp/gamelink-core-clippy.log`
- `/tmp/gamelink-go-all-phase5.log`, `/tmp/gamelink-go-phase5.log`
- `/tmp/gamelink-shim-final.log`, `/tmp/gamelink-phase5-android.log`
- `/tmp/gamelink-tauri-launch.log`

## Remaining runtime checks and integration

Native Windows execution was not possible here (no Windows/Wine runtime).
The Windows CI client test will exercise ShellExecute/batch arguments. Native
clipboard, chooser, folder opening, game launches and the time-boxed frontend
playthrough remain the parent's UI/runtime checks; cross-building does not
prove them. No configured playtest harness exists and none was introduced.
There is no installer, signing, public release or deployment in this change.

Parent must integrate this local commit with its independent Manager,
Supervisor, protocol and CLI work. Phase 6 can reuse the archive ownership
pattern/Manager commands, but agent CLI operations need explicit methods;
this desktop API is not an arbitrary privileged dispatch or backend service.

## Parent integration proof

The integrated tree passed core and Tauri fmt/Clippy/tests, Go vet, shim/client
race tests, and the full Go suite including integration (203 seconds). Frontend
lint, types and all 60 tests passed. The Linux release rebuilt successfully.

A bounded native UI pass used Xvfb and private temporary state: settings loaded,
demo mode persisted across restart, demo join displayed Connected and a local
address, and disconnect returned to Join. Native clipboard invocation reported
success; clipboard contents were not independently read. The native chooser
opened and canceled, but its rendering was black under Xvfb. Normal close via
WM_DELETE_WINDOW exited 0. An earlier xdotool windowclose destroyed the window
and caused GTK BadDrawable; that tool action is not a normal window close.
Temporary app state and Xvfb were removed/stopped after the pass.

Parent logs: `/tmp/gamelink-phase5-integrated-linux.log`,
`/tmp/gamelink-phase5-integrated-gotests.log`,
`/tmp/gamelink-phase5-ui-restart.log`. Screenshots: `/tmp/gamelink-ui-join.png`,
`/tmp/gamelink-ui-disconnect.png`, `/tmp/gamelink-ui-restart.png`.
