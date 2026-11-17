# Rust desktop host

The Tauri shell reuses `apps/desktop/frontend`. `src/host.rs` owns storage,
commands, lifecycle state, effect iteration, checks, demo timers, activity and
shutdown; `gamelink-core::desktop` owns shared desktop command policy.
`shim/desktop` owns generation-scoped Tailcat joins/probes and a bounded callback
mailbox. It does not import Manager, configuration stores or activity services.
Requests and responses use bounded, copied, explicitly freed C buffers.

Build with `scripts/build-desktop.sh`; pass `--archives-only` before the target
for Cargo development checks. Linux uses GTK/WebKitGTK; Windows GNU uses
MinGW resources and WebView2. See [building](../../docs/building.md) and
[the phase 7 boundary](../../docs/rust-phase7.md).
