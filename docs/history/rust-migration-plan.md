# Rust migration plan

Goal: GameLink is mainly Rust. Go shrinks to a thin shim around Tailcat, which is a Go-only library (`github.com/tailscale/tailcat`, `tailscale.com`). Replacing Tailcat with a Rust WireGuard stack is **out of scope**.

## Principles

- **Move, never duplicate.** Each piece of logic lives in one language. When a Go package moves to Rust, Go callers call Rust (cgo/C ABI) and the Go implementation is deleted. There is never a second invitation parser.
- **Behavior parity first.** Before porting a package, turn its Go tests and some real inputs and outputs into shared fixtures (`core/fixtures/`). Rust must pass them before the Go version is removed.
- **No per-packet language crossings.** The packet forwarding loops stay in Go. Only control-plane decisions cross the boundary.
- **Keep the security properties.** Keep these as they are: the input limits, fail-closed empty allowlists, revocation semantics, redaction, and the Go validation of Tailcat addresses. If a validator needs Tailcat internals, Go keeps that check and Rust receives the result.
- Every phase ends green: `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, `cargo test`, `go test ./...` (without build tags), the frontend tests, and the Android build. CI is updated in the same phase.

## Target layout

```
core/                      Cargo workspace
  crates/gamelink-core     pure logic: invite, identity format, config,
                           validation/redaction, lifecycle reducers,
                           console-bridge protocol (candidates, signal,
                           control, discovery packets)
  crates/gamelink-ffi      C ABI (cbindgen header) for Go; JNI for Android
  crates/gamelink-agent    Rust host agent CLI (replaces apps/server-agent)
apps/desktop-tauri/        Tauri shell reusing apps/desktop/frontend
shim/ (Go)                 Tailcat listen/dial/revoke/status + packet loops,
                           exported as a C library (c-archive / gomobile)
```

## Phases

### 1. Workspace and cross-language build (the risky step; do it first)
- Turn `core/` into a workspace and keep the existing JNI reducer working.
- Add `gamelink-ffi` with a C ABI built on JSON in, JSON out, using owned-buffer free functions and input size limits. Generate the header with cbindgen.
- Prove Go can call Rust through cgo on linux/amd64, windows/amd64 (cross-compiled with a mingw or zig toolchain) and android arm64/x86_64 (gomobile and the NDK), and that CI builds all three. Document it in `docs/building.md`.
- **Stop if cgo cross-compiling to Windows or Android isn't workable**, and report the alternatives. The fallback is Rust as the main binary, with Go built as a c-archive and linked into it.

### 2. Invite, identity format, config, validation and redaction
- Port `internal/invite`, `internal/config`, and the pure parts of `internal/identity` and `internal/security` (`validate.go`, `redact.go`).
- Atomic writes and DPAPI (`atomicfile.go`, `protect_windows.go`) move to Rust too, using the `windows` crate.
- Go callers switch to the FFI, and the Go implementations are deleted.

### 3. Lifecycle state machines
- Record `internal/client/manager.go` states and events as fixtures. Move its state machine into `gamelink-core` next to the Android reducer, with the same effects/generation pattern. Effects are carried out by the host (Go shim for now, Tauri later).
- Move the supervisor's config diffing and revocation decisions from `shim/transport/supervisor.go` into the Rust reducer. Go keeps the code that actually starts and stops Tailcat.

### 4. Console bridge protocol logic
- Port `candidate.go`, `candidate_map.go`, `signal.go`, `control.go`, session policy, and discovery packet parsing to Rust.
- Keep datagram framing, fragmentation, and reassembly in Go alongside the packet forwarding loops. The Go forwarding loops call Rust once per session or signal, never once per datagram.

### 5. Desktop: Wails to Tauri
- Create `apps/desktop-tauri`, reusing `apps/desktop/frontend` and swapping `realBackend.ts` from Wails bindings to Tauri commands with the same `backend.ts` interface. Keep `mockBackend.ts` and the frontend tests passing.
- Launching games (`launch_windows.go`) moves to Rust.
- The Go shim is linked as a library. Delete `apps/desktop` (Go) at the end of this phase.

### 6. Host agent CLI in Rust
- Port `apps/server-agent` commands to `gamelink-agent` (clap), keeping the same commands, flags, output and exit codes.
- The integration tests in `tests/integration` must still pass against the Rust agent.

### 7. Shrink Go and update the docs
- Remove dead Go code, then rename what's left into `shim/`.
- Rewrite `docs/core-boundary.md` and `docs/architecture.md`, and update `THIRD_PARTY_NOTICES.md` and the Rust license copies for the new crates.

## Done when
- Rust holds parsing, validation, policy, state and app shells, except datagram framing, fragmentation, and reassembly in the Go packet loops.
- Go holds only the Tailcat calls and packet forwarding.
- All tests and CI pass on all platforms.
- No logic exists in both languages.
