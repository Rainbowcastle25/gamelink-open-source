# Architecture

GameLink has a Linux Rust host agent with a Go Tailcat shim, a Windows desktop client, and an Android
app for the Bedrock console bridge. Rust owns shared validation, invitation
and configuration policy, private storage, console protocol codecs, and the
Android, desktop client, and supervisor lifecycle reducers.
Go supplies Tailcat transport through cgo adapters; Kotlin owns Android UI and
service code.

```
core/crates/gamelink-agent/ Rust CLI, file observer, effect loop and runtime publication
apps/desktop-tauri/         Rust desktop application host and Tauri native shell
apps/desktop/frontend/      React/TypeScript frontend
apps/android/              Kotlin UI, Keystore and foreground service
mobile/                    Stable public gomobile API (compatibility facade)
core/                      Rust validation, storage, policy, codecs and reducers
shim/agent/                Generation-scoped Tailcat server C archive + musl startup
shim/desktop/              Generation-scoped tunnel/probe C archive + callback mailbox
shim/transport/            Tailcat clients/servers, handles and forwarding loops
shim/consolebridge/        Network sessions, discovery sockets, framing/reassembly
shim/{invite,identity,security,games,rustcore}/  Typed Tailcat/Rust compatibility adapters
tests/integration/         End-to-end tests: shipped Rust agent (subprocess) + real Tailcat + local DERP
```

## Tailcat transport

[Tailcat](https://github.com/tailscale/tailcat) (v0.7.0) supplies encrypted,
authenticated WireGuard tunnels, direct UDP traversal and DERP relay fallback.
The server's Tailcat address contains its public key, pre-shared key and home
relay region. Treat the address as a secret capability. A peer must also hold a
device key in that game's allowlist.

Each hosted game has its own Tailcat identity. An empty allowlist is never
served. Revocation closes the game's listeners and live sessions, then starts
a replacement with the remaining approved keys. The supervisor checks config
once per second by default; Rust decides whether each observation requires
closing or starting a game server. The Rust agent serializes observations and
effects, including completing Go closure barriers before replacements; config changes, including key-file mtime changes,
are noticed on the next poll. Runtime status is updated on state changes and
with a five-second heartbeat. Invalid config stops running game servers.

The ordinary TCP and UDP forwarders carry only the configured tunnel port to
one configured destination, defaulting to `127.0.0.1:<port>`. Legacy UDP keeps
Tailcat's 1232-byte packet ceiling: larger datagrams are dropped and counted.

## Optional Bedrock Console Bridge

`gamelink-agent games add --type minecraft-bedrock --console-bridge ...`
explicitly enables the bridge. The default Bedrock configuration remains the
legacy single UDP tunnel. With the bridge enabled, the server handles both
authenticated TCP control and UDP media on that game's tunnel port.

The Android phone has one GameLink identity and approval per game. Every
console using that phone inherits the same approved-device access; consoles do
not receive separate GameLink approvals. Minecraft account identity and player
admission stay end-to-end with Minecraft/BDS. GameLink forwards native LAN
discovery/signaling and rewrites only candidate endpoints. It does not
translate LAN signaling into Mojang's HTTP join API, synthesize an identity,
or terminate the encrypted game transport.

The phone binds discovery to the selected active Wi-Fi IPv4 interface and
prefix. It rejects traffic outside that subnet and filters candidates to the
selected interface. The agent sends discovery only to the configured BDS
IPv4 address on UDP 7551, and accepts server media candidates only for that
same configured IP and the inclusive configured media-port range. The range is
limited to 256 ports. No general LAN forwarding or exit-node routing is
created.

Bridge limits are 8 phone peers, 32 sessions per phone, and 256 sessions total
on the host. Control frames are limited to 128 KiB and control data/SDP to
64 KiB. Reassembly allows 64 incomplete datagrams per peer for up to five
seconds. IPv4 UDP datagrams up to 65,507 bytes are split into at most 55
Tailcat packets of 1,208 payload bytes each. These bridge limits do not change
the legacy UDP 1232-byte drop behavior.

The Android app requires explicit user action and LAN consent to start a
`connectedDevice` foreground service with a persistent notification and Stop
action. The service is non-sticky: process death does not silently restart the
bridge. Tailcat connection loss retries with exponential backoff. Loss or
change of the selected Wi-Fi network/interface stops the bridge; the user must
select a network and start it again. A partial wake lock, Wi-Fi lock and
multicast lock are held while active, so battery and thermal effects need
hardware measurement.

## Linux host agent

```
gamelink-agent serve
  └─ Rust host runtime (one-second config poll)
       └─ each game with ≥1 approved device:
            transport.StartGameServer
              ├─ TCP: accept → dial configured destination → proxy
              ├─ UDP: one connected socket per client flow → destination
              └─ optional Console Bridge: TCP control + UDP discovery/media
```

There are up to 32 games and 256 approved devices per game. A normal game
server permits up to 256 TCP sessions. The bridge has the additional limits
listed above. `runtime.json` is a five-second heartbeat plus immediate state
changes and is considered fresh for 20 seconds.

## Optional Pterodactyl host administration

The [Host screen](pterodactyl.md) triggers discovery, registration, and power
snapshots only on explicit Refresh. An opt-in bearer-authenticated HTTP listener
runs on loopback inside the existing Rust agent. Panel credentials stay on the
node. Rust owns inventory reconciliation and approvals; `shim/hosthttp` supplies
bounded HTTP I/O without holding Tailcat resource locks. Registration preserves
game keys and device approvals. Missing/suspended games are disabled on the next
successful Refresh. Explicit per-game import approves the current device and
passes a private invitation between native hosts without exposing it to the UI.
No recurring Panel scan or additional daemon is introduced.

## Windows desktop client

```
React UI ⇄ Rust desktop host + core policy ⇄ bounded Go resource API ⇄ Tailcat
```

The Rust desktop host opens private storage, imports invitations, validates
settings and executable paths, runs launch effects and projects server views,
activity and diagnostics. It owns reducer state, check scheduling, demo timers,
update notifications and shutdown. An application mutex serializes reducer
transitions and effect registration. A synchronous public check releases that
mutex while waiting, so disconnect and shutdown can cancel it.

Go owns generation-indexed tunnels, bounded probes and their cancellation/
closure barriers. Callback producers write into a coalesced mailbox, never call
back into Rust, and retain a terminal result separately from latest status.
Rust drains statuses before completions every 100 ms, rejects stale generations,
and explicitly releases each completed probe. A stop cancels and joins an
outstanding start, closes its returned tunnel, then removes its mailbox slot;
a stale stop cannot touch another generation. No application mutex is needed
by Go callbacks. Replacement starts occur only after the old resource closes.
A 15-second frontend poll remains for counters and missed events. The legacy
five-second tunnel health check and reconnect behavior remain in the Go
network runtime. Sample servers use only Rust timers and never open sockets.

## Android app

```
Compose UI ⇄ Android foreground service ⇄ Rust lifecycle reducer
                                  ⇄ Go mobile API ⇄ Tailcat + Console Bridge
```

Kotlin owns Android permissions, selected Wi-Fi, notifications, foreground
service lifecycle and secure storage. Rust reduces lifecycle events and
generation tokens and owns invitation/device-key format validation. Go parses
Tailcat address internals and owns discovery, signaling and media sockets.
Secrets cross the C ABI only for their Rust-owned operations and never enter
lifecycle events, intents, logs or diagnostics. Android Keystore AES-GCM protects the
identity and invitations in `noBackupFilesDir`; backup and device transfer are
disabled. See [the Android Console Bridge guide](android-console-bridge.md)
and [the Rust/Go boundary](core-boundary.md).

The supported Android LAN mode is Wi-Fi with a private IPv4 address and an
active selected network. Android API 37 requires runtime `ACCESS_LOCAL_NETWORK`
permission. There is no iOS client: local-network consent, multicast access,
background networking and lifecycle policy require a separate iOS design and
validation.

## Invitations and storage

Invitations use `glink1.` plus base64url JSON. Parsing validates game, protocol,
port, relay and optional Console Bridge v1 capability. A bridge invitation
does not approve a phone by itself; the host must approve its public key.

| Item | Location | Protection |
|---|---|---|
| Windows private device key | `%AppData%\GameLink\identity.json` | DPAPI-sealed, atomic write |
| Windows saved server address | `%AppData%\GameLink\config.json` | DPAPI-sealed |
| Android identity and invitations | app `noBackupFilesDir` | Android Keystore AES-GCM; backup disabled |
| Host per-game keys | `keys/<id>.json` | 0600 file, 0700 config directory, atomic replacement |
| Logs and diagnostics | app data | secrets and bridge signaling redacted |

Non-Windows development builds use owner-only files instead of DPAPI.

## Production build tags

Production Go binaries use `build-tags.txt`, which removes unused Tailscale
features including port mapping, and `ts_omit_ssh`, which removes Tailcat SSH
and SFTP. Tests run without those tags because the hermetic relay depends on
code they remove. Android's build script uses the production tags for its Go
mobile AAR.


The desktop archive exports `gamelink_desktop_call` and
`gamelink_desktop_free`; the agent has corresponding `gamelink_agent_*`
exports. Both copy requests before dispatch and return C malloc-owned buffers,
copied and zeroed/freed exactly once by Rust. Inputs are capped at 32 KiB and
responses at 4 MiB. No Go pointer crosses the ABI. The webview can invoke only
the explicit public desktop commands; raw resource operations, arbitrary core
methods, identity keys and full Tailcat addresses are never returned to it.

The Go archives expose resource initialization/shutdown, key generation and
Tailcat address validation. Agent operations also resolve relay metadata and
start/stop generation-scoped servers and return stats/logs. Desktop operations
start/stop generation-scoped joins/probes, return stats and drain callback
snapshots. Rust owns all file operations and application effect iteration.
Production dependency checks reject imports from `tests/`, alongside the
existing SSH/SFTP/port-mapping/router exclusions. There are no duplicate Go hosts: the integration tests drive the shipped Rust
agent binary as a subprocess, and the Go shim packages directly. Frozen original
Go oracles under `core/fixtures` remain non-compiling.
