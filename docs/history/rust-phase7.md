# Final Rust host and Go transport boundary

Rust now owns agent file observation, ordered supervisor effect execution,
monotonic retry time and runtime status publication. The desktop Rust host
owns commands, configuration, identity storage, settings, invitation imports,
views, activity, diagnostics, lifecycle state, probes and demo scheduling.
Shared deterministic policy lives in `gamelink-core`.

Production Go code lives under `shim/`. Agent and desktop archives expose
bounded owned buffers and generation-scoped network start/stop/stats operations.
Go owns Tailcat keys, address validation, sockets, network resources and closure
barriers. Desktop callbacks enter a bounded mailbox with separate latest-status
and completion slots. Stops cancel and join producers before releasing a slot.
Datagram framing, fragmentation and reassembly remain in Go packet loops;
media traffic never crosses the language boundary per packet.

`mobile/` preserves the public gomobile API. Kotlin retains Android lifecycle,
permissions, service and Keystore ownership. The legacy Go application/store
hosts that once lived under `tests/testsupport/` have been removed: the
integration tests now run the shipped Rust agent binary as a subprocess and the
transport shim directly (see the [test migration map](test-migration-map.md)).
Production dependency checks still reject any dependency on `tests/`.

## Verification

Recovered worker logs record full Go tests (integration 216.672 s), race tests
(integration 219.106 s), core and desktop fmt/Clippy/tests, static amd64/arm64
builds and native/QEMU CLI flows, Linux and Windows desktop releases, frontend
lint/types/build/60 tests with zero production audit findings, Android APK/lint,
and all five API37 instrumentation tests with the local relay fixture.
Logs are `/tmp/phase7-*.log` in the development environment.

Parent native Linux release checks used private temporary state on Xvfb:
settings and demo mode, demo join/disconnect, dashboard, persisted demo setting
after restart, and normal WM_DELETE_WINDOW close (exit 0). Screenshots include
`/tmp/phase7-ui-joined.png` and `/tmp/phase7-ui-restarted.png`. Temporary desktop
state, relay fixture state, emulator and Xvfb were cleaned up.

Native Windows GUI execution, physical console/gameplay, ARM64 Android runtime
and direct-versus-relay selection remain unverified. Cross-builds and mock/demo
flows do not establish those runtime behaviors. No public deployment is part
of this migration.
