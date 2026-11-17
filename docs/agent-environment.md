# GameLink agent environment

This is the Linux reference for a clean checkout or isolated feature worktree
based on `origin/master`. Checks use generated keys, temporary state and a local
relay. They need no production account, invitation, game server, Docker daemon
or deployment credentials. Dependency setup downloads packages; the hermetic
tests do not use public relays. Preserve unrelated branches/worktrees and edits.

## Pinned prerequisites

[`scripts/agent-tools.sh`](../scripts/agent-tools.sh) records the check versions.
Keep it aligned with `go.mod`, CI and the existing build scripts when updating.

| Tool | Version / source | Scope |
| --- | --- | --- |
| Go | 1.27.4; `go.mod` | Shim builds, vet, tests and race detector |
| Node / npm | 22.22.2 / bundled npm 10.9.7 | Frontend; CI uses the same Node patch |
| Rust / Cargo | 1.100.0 | Core, FFI, agent and desktop; rustfmt, clippy, llvm-tools |
| Zig | 0.15.1 | Static Linux agents and Windows cross-compilation |
| cbindgen | 0.29.4 | C header check only |
| Python | Host Python 3 standard library | Isolated safety gate regressions; no pip packages |
| Git, Bash 4+, C compiler, libc headers, GNU coreutils/grep | Host packages | Native checks and safety gates |
| GTK3, WebKitGTK 4.1, librsvg development packages, pkg-config | Host packages | Native Tauri build/tests |
| MinGW-w64 gcc/binutils (including windres) | Host packages | Windows Tauri cross-build |
| qemu-aarch64 or qemu-aarch64-static, binutils/readelf, file | Host packages | Static agent execution and artifact inspection |
| JDK / Android SDK | JDK 21, API 37, build-tools 36.0.0, NDK 29.0.14206865 | Android checks; `JAVA_HOME` and `ANDROID_HOME` |
| Gradle / Android Gradle plugin | 9.9.1 / 9.5.1, checked-in wrapper/config | Android builds |
| gomobile/gobind | `v0.0.0-20261011034528-197770444867` | Existing Android build installs into `.tools/android/bin` |
| harness | Existing machine-provided executable | `harness check` must currently report UNWIRED |

Go dependencies use `go.sum`; both Rust workspaces use their checked-in
`Cargo.lock` and `--locked`; frontend setup uses `package-lock.json` and `npm ci`.
Host package versions vary by runner image. Doctor verifies required executables,
GUI pkg-config packages, Rust target availability and exact language/Zig pins.
It does not install tools, invoke sudo, read application state or inspect secrets.

## Scoped setup

The machine/integration owner provisions tools, Rust components/targets, native
libraries and SDKs. Do not edit global mise/shell configuration or install shared
tools as a task side effect. The current host has Go/Node and a mise Rust launcher;
select tools per command rather than changing defaults:

```bash
source scripts/agent-tools.sh
export MISE_ACTIVATE_AGGRESSIVE=1
# The installed mise rust@stable launcher uses rustup. Checks default to the
# installed 1.100.0 toolchain and disable rustup automatic installation.
mise exec go@"$GAMELINK_GO_VERSION" node@"$GAMELINK_NODE_VERSION" \
  rust@stable zig@"$GAMELINK_ZIG_VERSION" -- bash scripts/check.sh doctor
mise exec node@"$GAMELINK_NODE_VERSION" -- npm --prefix apps/desktop/frontend ci
```

If another installed Rust channel contains **exactly 1.100.0**, select it with
`RUSTUP_TOOLCHAIN=stable` (or its installed name) in the command environment.
Doctor still rejects a different compiler version. Rustup target/component
installs are per toolchain: availability in `stable` does not prove availability
in the separately named `1.100.0` toolchain. `ZIG=/absolute/path/to/zig` can select
an existing Zig binary; doctor checks its version as well.

Inherited mise activation can put system Node/Go ahead of installed tools in
child shells. Use `MISE_ACTIVATE_AGGRESSIVE=1` on the **outer** mise command.
The check wrapper also exports it, and doctor checks direct and nested-shell
versions. Existing CI setup actions already put the tools on PATH; CI invokes
the same scripts directly and does not need mise.

Owner provisioning for full Linux CI needs Rust targets
`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`,
`x86_64-pc-windows-gnu`, `aarch64-linux-android`, `x86_64-linux-android`,
plus `llvm-tools`, `rustfmt` and `clippy` on the selected toolchain. cbindgen's
existing install command is `cargo install cbindgen --version =0.29.4 --locked`.
Use an owner-prepared SDK/JDK; the build pins the NDK and Gradle inputs already.
See [building.md](building.md) for platform build details.

## Shared checks

After tool selection, run these from any directory using the script's path:

```bash
bash scripts/check.sh doctor          # fast prerequisites, read-only
bash scripts/check.sh doctor full     # all Linux prerequisites, read-only
bash scripts/check.sh fast            # native agent build, Go vet/tests, frontend, safety
bash scripts/check.sh full            # fast + the remaining Linux CI sections + harness
# Or combine only relevant sections, for example:
bash scripts/check.sh rust header safety
bash scripts/check.sh race
bash scripts/check.sh playtest
```

| Section | Existing checks used by agents and CI |
| --- | --- |
| `native` | Rust FFI staticlib, production Go agent archive, debug Rust CLI |
| `go` | `native`, untagged `go vet ./...`, all untagged Go tests with the real Rust CLI |
| `frontend` | ESLint, TypeScript, all Vitest tests, production Vite build, production npm audit |
| `safety` | Existing forbidden-dependency gate, secret scan, isolated negative gate regressions |
| `race` | Native prerequisites and existing Go race suite |
| `rust` | Native prerequisites, core fmt/tests, panic-probe tests, both strict clippy configurations |
| `header` | Pinned cbindgen regeneration comparison |
| `static` | Both musl agents, ELF static linkage checks, native/ARM CLI smokes, shipped-agent integration tests, checksums |
| `desktop` | Linux release build/archives, fmt/clippy/tests, Windows GNU release build/archives |
| `windows-cross` | Panic-probe Rust FFI plus migrated Windows Go test executables |
| `android` | Native libraries/AAR/APKs/lint plus existing production dependency exclusions |
| `playtest` | Existing `harness check --project <repo> --json`; no new adapter |

Setup is separate from check execution. Refresh frontend dependencies with
`npm ci` after lock changes; do not update lockfiles to make verification pass.
The existing desktop build also runs its own `npm ci`. The Android build retains
its existing pinned scoped Go-tool setup and Rust target-add step. Doctor checks
targets first so missing machine prerequisites are reported before the build.

Checks clear inherited cross-build/test-probe, Rust target/output/flag and
optional Android-fixture/resource-profile overrides. Native builds set
`GAMELINK_AGENT_BINARY` explicitly so integration tests exercise the Rust host.
Tests must remain **untagged**; production dependency checks inspect the exact
`build-tags.txt` set. Go flags are explicitly empty (including machine-saved
defaults), and Go/Rust automatic toolchain downloads are disabled during checks.
The optional `external` public-relay workflow remains manual
and is excluded from fast/full. Never use a real game, invitation or signed-in
profile to fill a local verification gap.

## CI contract and evidence

All nine required job names and IDs remain unchanged: `go`, `race`, `frontend`,
`desktop`, `rust`, `windows-rust`, `windows-cross`, `windows-run`, `android`.
Linux runs on GitHub-hosted `ubuntu-24.04` and Windows on `windows-2025`.
Linux cross-builds do not replace native
Windows Rust/DPAPI execution or the downloaded Go-test execution job. Full local
Linux checks cannot prove those Windows executions.

Hosted runners start clean, so setup-go restores the Go module/build cache and
`Swatinem/rust-cache` restores Cargo dependencies for both Rust workspaces.
No runner-image persistence or preinstalled apt capabilities are assumed.
Existing apt, SDK and cbindgen prerequisite steps remain in CI.

Logs and timings go to `.tools/checks/<UTC-time>-<pid>/summary.txt` and sibling
per-command logs (ignored). Commands stop at the first failure; console output
includes the same logs in CI. Doctor failures exit 2 before check artifacts are
created. Keep exact versions, commands, exit codes and log paths for the parent.

The safety regressions create disposable repositories outside the checkout.
They prove a real `github.com/pkg/sftp` import fails the existing dependency
policy, a broken Go graph fails closed, and secret probes fail without exposing
contents. The original forbidden import patterns and fixture byte-hash exception
are preserved. The secret scanner handles NUL-delimited filenames and checks
each node-key match individually, so a placeholder on the same line cannot
hide another key. Matching file paths are shell-quoted and content is redacted.

| Failure | Action |
| --- | --- |
| Doctor exit 2 | Follow the named scoped repair or ask the machine owner to provision the missing prerequisite. Do not change shared configuration. |
| Missing/wrong tool in a child shell | Use outer aggressive mise activation and rerun doctor. |
| Native/GUI/cross tool missing | Run independent sections for partial evidence; preserve the exact missing package/target. Never report full green. |
| Gate regression fails | Read its isolated test failure and command log; fix the gate, without expanding dependency policy or whitelisting the probe. |
| npm audit warning | Production high/critical findings block the gate. Dev-only/moderate findings are reported; never run audit fix during checks. |
| Harness exit 0, UNWIRED | Expected: this repository has no configured harness. No new wiring is part of this task. |
| Harness exit 1 | Read the named `ticket.json`, fix change-caused bugs and rerun. Report pre-existing bugs for the user's decision. |
| Harness exit 2 | Record the infrastructure error and continue with independent project evidence; this is not an app failure or a passed playtest. |

No product flow changes are part of this environment audit. UI/gameplay changes
still require the configured harness and one short changed/main-flow playthrough.

## Delivery boundary

Implementation agents commit locally and hand off the branch/SHA, versions,
commands/results/logs and blockers. The parent owns sol-loop review, PR delivery,
required checks/protection enforcement, runner registration/isolation and any
deployment. Never alter labels, bypass checks, push master, or edit secrets,
`.env`, Compose or deployment configuration from this workflow. Deployment and
post-merge health/rollback remain the integration owner's responsibility.
