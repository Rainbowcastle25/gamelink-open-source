# Phase 6: Rust host agent

Base: c320fc7100c32b37118c7c210bf7815811614bb5 (phases 1–5).
The isolated implementation branch was advanced from its initially prepared
6c632de checkout before any changes; no parent worktree was modified.

`core/crates/gamelink-agent` owns the local CLI, command dispatch, clap flags,
validation sequencing, command output and exit codes. It calls `gamelink-core`
configuration, private storage, invitation and security operations directly.
There is no compiled Go CLI and no second Go command/config parser.
`apps/server-agent` has been removed. Non-compiling copies of its original
implementation/tests and Go-generated stdout/stderr/exit fixtures preserve the
migration oracle, including tabular admin output. The compatibility adapter
retains Go's single-dash long flags, last-value-wins, bool `=false`, parsing
stopping at the first positional, help on stderr with exit 2, and leading-only
`--dir` handling. Clap owns the resulting arguments; built-in clap help is
replaced with the established Go help text. Uncommon shell pipe/error details
outside the frozen cases are not claimed as exhaustive compatibility.

## Boundary and lifetime

The Go archive accepts only semantic operations: initialize/shutdown, generate
Tailcat keys, resolve a relay, format/validate the persistent Tailcat address,
and start/poll the existing supervisor effect runner. No CLI argument vector
or entire command is sent to Go. Input is copied before dispatch, capped at
32 KiB; output is capped at 4 MiB, copied into `C.malloc` memory, cleared and
freed exactly once by the Rust RAII owner. No Go pointer escapes. Initialization
is exclusive; shutdown is idempotent, cancels and joins the supervisor and
releases all live Tailcat resources. The Go runtime itself has process lifetime.
The Rust signal handler covers both SIGINT and SIGTERM and retains
`SA_ONSTACK`, as required by Go when a signal lands on a runtime thread.

### Deliberate phase 7 follow-up

The existing Go `transport.Supervisor` remains the observation/effect runner
for this phase, as allowed by the phase 6 brief. Its configuration decisions,
approved-set comparisons, retry state, generation checks, fail-closed handling,
revocation and shutdown policy already belong exclusively to
`gamelink-core::supervisor`; Go calls that reducer and applies its ordered
start/stop effects. No Go supervisor policy was added or copied. Tailcat still
receives each caller's device identity and the reducer-authorized approved set.
Phase 7 should move observation, runtime-status writing and effect iteration
into Rust and narrow this ABI to generation-scoped start/stop/stats operations,
then remove the unused Go store/runner adapters. That is a real remaining
migration step, not a claim that the final thin shim is complete here.

## Static Linux linking

`scripts/build-agent.sh` builds the same production hardening tags, now into a
Go C archive compiled by Zig's musl wrappers. Rust's bundled LLD and musl own
the final static link. Static relocation avoids conflicting PIE startup; the
ARM Cortex-A53 erratum linker workaround remains enabled. Both architectures
must have no `INTERP` or `NEEDED` entries.

Musl invokes `.init_array` without argc/argv, but Go's Linux library startup
expects glibc's argument-passing convention. Unmodified archives therefore
crashed in Go `runtime.sysargs` even for `--version`. The archive builder removes
only `go.o`'s `.init_array` and exposes its existing architecture startup symbol.
`runtime_musl.c` invokes it once explicitly with a retained C argv/environment
vector and a terminated empty auxiliary vector (Go falls back to `/proc/self/auxv`).
Go still owns all runtime initialization; no toolchain source is patched.
This small transformation is tied to the pinned Go toolchain and guarded by
native and QEMU admin flows plus the static-binary network integration in CI.
GNU development builds retain their normal constructor. Windows agents are not
shipped. Android and desktop shim paths are unchanged.

## Verification

See `docs/building.md` for reproducible build/check commands.
`scripts/test-agent-cli.sh` exercises init/add/list/approve/invite/status/revoke/
remove on native amd64 and QEMU arm64, discarding secret invitation stdout.
The Rust subprocess suite checks frozen Go output, bad inputs, invitation
contents, and 0600 files/0700 directories.

`TestRustAgentSubprocessRevocationAndFailClosed` requires
`GAMELINK_AGENT_BINARY` in CI. It administers the actual Rust binary, replaces
only its pinned relay metadata with the existing local DERP/STUN fixture,
starts `serve` with no approvals, approves two identities, echoes over Tailcat,
revokes Alice, verifies her established session closes and a new join fails,
checks Bob still reconnects, corrupts the snapshot and verifies Bob's existing
session closes, then sends SIGTERM and verifies exit 0 and runtime-file removal.
No invitation or private key is printed, even on failure. The rest of the Go
integration suite still exercises the shared transport and Android adapters.
