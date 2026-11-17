# Shared Rust core boundary

`core/` is a Rust workspace. Shared policy and the Android, desktop client,
and host supervisor reducers live in the `core/crates/gamelink-core` rlib.
`core/crates/gamelink-ffi` (a
`cdylib`/`staticlib`) exports it over JNI. Kotlin serializes events on one
coroutine and retains the returned JSON state. Rust returns effects; Kotlin
executes them and reports their generation-tagged callbacks back to the reducer.

## JNI entrypoint

Android loads `gamelink_ffi` and calls:

```kotlin
System.loadLibrary("gamelink_ffi")
external fun nativeReduce(inputJson: String): String
```

The native symbol is
`Java_io_gamelink_app_Core_nativeReduce(JNIEnv*, jclass, jstring) -> jstring`.
Input is bounded to 32 KiB and has this envelope:

```json
{
  "state": {
    "phase": "idle",
    "generation": 0,
    "message": null,
    "name": null,
    "serverId": null
  },
  "event": {
    "type": "start",
    "server": {
      "id": "world-1",
      "name": "World",
      "game": "minecraft-bedrock",
      "bridgeVersion": 1
    },
    "localIpv4": "192.168.1.25"
  }
}
```

The result is `{ "state": { ... }, "effects": [ ... ] }`. Phases are
`idle`, `connecting`, `connected`, `reconnecting`, `unreachable`, and `error`.
Effects are:

```json
{"type":"start_transport","generation":1,"server_id":"world-1","name":"World","local_ipv4":"192.168.1.25"}
{"type":"stop_transport","generation":1}
```

Events are `start`, `stop`, `network_lost`, `transport_status`, and `error`.
Transport callbacks must include the generation from their start effect. The
reducer ignores stale generations. A stop is idempotent and invalidates any
outstanding callback before asking Kotlin to release the prior generation.
Unreachable status deliberately says “The server may be offline, or this device
may need approval.” It does not claim to know which condition applies.

## Ownership

- Rust owns invitation encoding/parsing and field/relay policy, the game
  catalog, public-key format checks, validation, redaction, configuration
  policy, identity file formats, atomic private writes, Windows DPAPI, desktop
  and supervisor lifecycle decisions, and console control/discovery codecs.
  Go owns Tailcat key generation and key types, `tailcat.ParseAddr`, network
  calls, resource ownership, and packet forwarding. Datagram framing,
  fragmentation, and reassembly stay beside the Go forwarding loops. Go passes decoded relay metadata to Rust for
  policy checks. These adapters contain no second invitation parser.
- Invitations and private keys now cross the C ABI for their owning Rust
  operations. They never enter the lifecycle reducer or UI events. The
  lifecycle input still contains only validated display metadata and bridge
  capability. Kotlin keeps Android Keystore protection and storage.
- The bridge start policy requires Bedrock game metadata and either
  `bridgeVersion: 1` or the `console-v1` capability. Missing capability returns
  a clear old-agent error before any transport effect is emitted.
- Kotlin owns Android permissions, selecting the current Wi-Fi private IPv4,
  binding the Android service, running effects, process lifecycle, and feeding
  callback events into the reducer. Rust independently bounds and checks that
  the selected address parses as a private IPv4 address.
- The lifecycle reducer owns deterministic transitions, user-safe status
  messages, generation checks, and effect descriptions. It does not perform
  sockets or authentication.

## C ABI and bounds

Go calls `gamelink_call` through `shim/rustcore`. Every result is copied
into Go memory and the Rust-owned response buffer is zeroed before release.
This does not guarantee that every intermediate serializer allocation is
zeroed. Transport errors never echo malformed inputs; domain operations return
their validated user-facing errors.

The default serialized input limit is 32 KiB. Redaction has a dedicated
256 KiB limit and replaces the entire line with `[redacted]` on oversized
input or failure. Explicit storage/config operations allow 8 MiB so supported
configurations with many approved devices still fit. `supervisor.reduce` uses
the same 8 MiB limit; each desktop lifecycle request retains the 32 KiB default.
Explicit console/discovery methods allow 1 MiB to accommodate bounded encoded
control frames and binary payloads. Unknown methods retain the default limit.
File reads are bounded too. Limits are checked on both sides of the C ABI, before Rust reads input
pointers. JSON escaping counts toward the limit.

## Host effects and console protocol

Rust owns both desktop and agent application hosts. The agent reads bounded
configuration snapshots and key files, observes key mtimes, supplies monotonic
retry time, executes ordered reducer effects, and atomically publishes runtime
status on changes and five-second heartbeats. Invalid configuration stops
resources before any replacement; duplicate IDs reject the entire observation.
The Go server API takes one generation's validated network parameters and
approved keys. It has no file observer, reducer state, effect loop or runtime
writer. Empty allowlists remain rejected at the Tailcat boundary too.

The Rust desktop host owns private storage, settings/executable validation,
invitation import sequencing, launch policy, views, activity, diagnostics,
reducer state, scheduling, demo timers and effect iteration. Go owns only
contexts, network handles, probes and closure barriers. Each callback carries
its resource generation. Stops cancel and join producers and close returned
handles before deleting their mailbox slot; callbacks never reenter the Rust
application lock. Latest status and terminal completion occupy separate slots.
Stale-generation cleanup releases only that generation, even after replacement.
The public synchronous Check call waits without holding the application mutex.

Rust encodes and parses candidates, signaling, control messages, discovery
packets, and advertisements. Go retains bounded length-prefix I/O, sockets,
source pinning, session handles, and all media forwarding. Control/discovery
traffic crosses the ABI; media datagrams do not. See the
[console protocol boundary](rust-console-protocol.md).

## Verification

From `core/`, run `cargo fmt --check`, `cargo test`, and `cargo clippy --all-targets
-- -D warnings`. The same reducer tests run natively. Android API 37 emulator
instrumentation has passed; physical console/device and hardware checks remain
separate.
