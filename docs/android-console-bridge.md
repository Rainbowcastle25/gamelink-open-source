# Android Console Bridge

**Frozen for maintenance only.** New bridge features are on hold until the PC
path has been validated with real games using the
[validation procedure](validation.md).

The Android app is an experimental LAN-to-LAN bridge for Bedrock consoles. It
lets an Xbox or PlayStation discover a Bedrock Dedicated Server (BDS) on the
host's LAN while the phone carries discovery, signaling and encrypted game
media through GameLink's authenticated Tailcat tunnel. The phone must be on the
same Wi-Fi as the console. This is not an Android VPN and does not change the
router.

**No Xbox or PlayStation hardware validation has been completed.** Screen-off
operation, multiple consoles, revocation during play, Wi-Fi loss, battery use
and sustained thermals remain open checks. There is no app-store release or
public deployment. The instructions below are for local evaluation only.

## How the bridge is scoped

The host must explicitly enable the bridge for a Bedrock game. That game then
accepts authenticated TCP control and UDP media on its Tailcat tunnel port.
The phone's GameLink device key must be approved for that game. Every console
using the phone inherits the phone's per-game approval; GameLink does not
approve individual consoles. Minecraft/BDS continues to authenticate player
accounts and enforce its own admission settings.

This is LAN-to-LAN signaling. The bridge forwards BDS's native discovery
advertisement and signaling messages. It does not translate console offers to
Mojang's HTTP join API, synthesize an identity assertion, inspect game data,
or terminate Bedrock's end-to-end encrypted media.

The phone selects its active Wi-Fi IPv4 interface and prefix. Discovery packets
are filtered to that interface and subnet. The host sends discovery only to
`--bedrock-ip` on UDP 7551 and only accepts server media candidates for that
same IP within `--media-ports`. The configured BDS address must therefore be
its actual LAN IPv4 address; the bridge cannot accept arbitrary SDP candidate
destinations. No general LAN routing or other host ports are exposed.

## Configure the Linux host

Run BDS on the Linux host or another machine reachable from the agent's LAN
interface. Use BDS's actual LAN IPv4 address in both `--bedrock-ip` and the
`--dest` host portion. Example: BDS at `192.168.1.20`, agent at
`192.168.1.10`, BDS game port UDP 19132, media ports 19132 through 19387:

```sh
gamelink-agent init
gamelink-agent games add \
  --type minecraft-bedrock \
  --name "Console Bedrock" \
  --console-bridge \
  --bedrock-ip 192.168.1.20 \
  --bridge-bind-ip 192.168.1.10 \
  --dest 192.168.1.20:19132 \
  --media-ports 19132-19387
```

`--media-ports first-last` is an inclusive UDP range with at most 256 ports.
Choose a range that contains BDS's advertised media candidates and allow it
through the host's local firewall from the LAN as needed. The bridge does not
configure BDS or the firewall. BDS discovery must answer on UDP 7551 at the
configured `--bedrock-ip`. The tunnel game port defaults to Bedrock's 19132;
this is the authenticated Tailcat port, not BDS's LAN discovery port.

| Option | Meaning |
|---|---|
| `--console-bridge` | Explicitly enable the bridge. Without it, Bedrock keeps the legacy single UDP forward. |
| `--bedrock-ip` | BDS's actual LAN IPv4; discovery targets this address on UDP 7551 and candidate policy pins to it. |
| `--bridge-bind-ip` | Agent's LAN IPv4 address used for bridge UDP sockets. |
| `--media-ports first-last` | Inclusive BDS media UDP port range, maximum 256 ports. |
| `--dest ip:port` | BDS game endpoint; use the same BDS IPv4 as `--bedrock-ip`. |

Approve the phone and create its invitation as for another GameLink device:

```sh
gamelink-agent devices approve --game console-bedrock \
  --key nodekey:<phone-public-key> --label "Living room phone"
gamelink-agent invite --game console-bedrock
gamelink-agent serve
```

The invitation advertises Console Bridge v1 capability. It is a secret; share
it privately. Revoking the phone restarts the host game server and ends bridge
sessions. Every console using that phone loses the same game access.

Tailcat attempts a direct UDP path and falls back to DERP when direct
connectivity is unavailable. DERP keeps payloads end-to-end encrypted but adds
latency. The phone currently has no user-facing direct/relay selector.

## Build and install the Android app

The supported local build uses Linux x86_64, Go 1.27.4, Rust 1.100, JDK 21,
Android SDK platform 37, NDK 29.0.14206865, and the checked-in Gradle 9.9.1
wrapper (Android Gradle Plugin 9.5.1). The script builds Go and Rust native
libraries for `arm64-v8a` and `x86_64`, then builds a debug APK and runs lint.

```sh
export ANDROID_HOME=/path/to/android-sdk
export JAVA_HOME=/path/to/jdk-21
./scripts/build-android.sh
adb install -r apps/android/app/build/outputs/apk/debug/app-debug.apk
```

The APK targets API 37 and has minSdk 26. On API 37, Android requires the
runtime `ACCESS_LOCAL_NETWORK` permission. Android 13 and later also prompt for
notifications. Grant the permissions, approve the phone's public key on the
host, import the invitation, select the active Wi-Fi network, check the LAN
access consent box and start the bridge from the visible app. Android API 36
smoke tests do not cover the API 37 local-network permission prompt.

The foreground service shows an ongoing notification with a Stop action. It is
non-sticky: process death does not restart it. Tailcat connection loss retries
with exponential backoff while the service is alive. If the selected Wi-Fi,
interface or IPv4 changes, the service stops and requires the user to select
the network and start again. The bridge uses a partial wake lock, Wi-Fi lock
and multicast lock while active; measure battery and heat on the target phone.

The Android API 37 16 KiB emulator instrumentation run passed storage, JNI,
Go identity/error, DNS/TLS and a local Tailcat fixture that reached discovery,
stopped, and released UDP 7551. The fixture reported an initial `path:
unknown`; the Android UI does not currently show the direct/relay path. The
local-network permission was pre-granted by instrumentation, so its interactive
prompt remains untested. These emulator checks do not prove BDS media gameplay.

Android Keystore AES-GCM protects the device identity and saved invitations in
`noBackupFilesDir`. Backup and device transfer are disabled. The Go runtime
owns secret parsing and network work; Rust receives metadata and lifecycle
events, not invitation text or private keys. See [the Rust/Go boundary](core-boundary.md)
and [Android sandbox test notes](android-sandbox-tests.md).

No iOS app is included. iOS requires a separate design for local-network and
multicast permission, background networking modes and lifecycle policy; the
Android foreground-service behavior cannot be assumed to transfer to iOS.

## Implemented limits

- Up to 8 phone peers on one host.
- Up to 32 Bedrock sessions per phone and 256 sessions total on the host.
- Control frames up to 128 KiB; control data/SDP to 64 KiB.
- At most 32 SDP candidates per message, each at most 2,048 bytes.
- 64 incomplete datagram assemblies per peer; fragments expire after five
  seconds.
- Maximum IPv4 UDP datagram 65,507 bytes, split into at most 55 Tailcat packets
  with 1,208 bytes of media payload each.
- Discovery and media sources, selected phone interface/subnet, BDS address,
  and configured media range are checked before forwarding.

The legacy Bedrock and other ordinary UDP tunnel path is unchanged: Tailcat
datagrams over 1,232 bytes are dropped and counted. Fragmentation applies only
to the explicit Console Bridge path.

## Ten-point implementation review

| # | Original code finding | Status | Current disposition |
|---:|---|---|---|
| 1 | `games.go` registers Bedrock as UDP-only. | Confirmed | The Bedrock game registry remains UDP. `--console-bridge` is a separate explicit opt-in. |
| 2 | `server.go` serves one protocol, port and destination per game. | Changed | Default games still do. A bridge-enabled Bedrock game additionally serves authenticated TCP control and UDP media on the same Tailcat game port. |
| 3 | `client.go` exposes gameplay through localhost listeners. | Confirmed | The Windows desktop tunnel remains loopback-only. Android binds only the selected Wi-Fi LAN discovery/media path while its service runs. |
| 4 | Ordinary UDP drops datagrams larger than 1,232 bytes. | Confirmed | Legacy UDP behavior remains. Only the opt-in bridge fragments/reassembles datagrams up to 65,507 bytes. |
| 5 | `protect_other.go` does not encrypt secrets at rest outside Windows. | Changed | Windows still uses DPAPI and other Go platforms retain owner-only files. Android app identity/invitations use Keystore AES-GCM in backup-excluded storage. |
| 6 | Supervisor reconciles each second, writes runtime status on every reconcile, and also has a status timer. | Changed | Config is still checked each poll; unchanged JSON is reused. Runtime writes occur on state changes and the five-second heartbeat. |
| 7 | Frontend polls every two seconds in addition to backend events; tunnel health check is five seconds. | Changed | Frontend counter refresh is now every 15 seconds; backend events still refresh immediately. The five-second tunnel health check remains. |
| 8 | Invitation parsing rejects literal private relay IPs but does not pin DNS at dial time. | Changed | Relay hostnames are resolved and their addresses pinned before Tailcat dials; private or invalid answers are rejected by default. |
| 9 | Desktop TCP admission checks current handlers before the next handler goroutine registers. | Changed | A slot is atomically reserved before the handler goroutine starts, enforcing the configured local connection ceiling. |
| 10 | Integration coverage uses generic echo servers; README says real clients are unverified. | Confirmed | Hermetic protocol and bridge tests exist, but real game and console compatibility remains explicitly unverified pending hardware checks. |

## Hardware validation checklist

No physical hardware run is complete. Work through this sequence and record
phone model/Android version, BDS version, console model/system version, Wi-Fi
layout, observed Tailcat path, outcomes and logs with secrets removed.

### Xbox first

- [ ] Start from a clean Android install; verify Keystore identity creation,
  invitation import, host approval and explicit LAN permission/consent.
- [ ] On one Xbox, discover the BDS world over the same Wi-Fi and join. Verify
  gameplay and player identity/authentication.
- [ ] Repeat with direct UDP connectivity available. Record the path using
  host-side observation; the current app UI does not show the Go path status.
  Do not infer it from latency alone.
- [ ] Prevent direct UDP while preserving DERP reachability. Confirm a DERP
  relay session can discover and play, and record the observed path.
- [ ] Lock the phone screen for 15 minutes, then test longer play. Confirm the
  foreground notification remains, discovery and media continue, and record
  battery and phone temperature before/after.
- [ ] Join from a second Xbox through the same phone. Confirm both consoles
  work concurrently and the session count is correct.
- [ ] Revoke the phone's approved key while both consoles are active. Confirm
  existing sessions end and neither console reconnects; re-approval should be
  required to use the bridge again.
- [ ] Turn Wi-Fi off, switch access points, and change the phone's interface or
  IPv4 address. Confirm the service stops, sockets/locks release and it does
  not restart until the user explicitly starts it again.
- [ ] Press Stop in the app, then use Stop in the notification. Confirm both
  stop paths end gameplay and release LAN resources.
- [ ] Run a 60-minute session; record battery percentage, thermal state,
  throughput, reconnects and cleanup after Stop.

### PlayStation second

- [ ] Repeat clean-install, approval, discovery and gameplay checks with a
  PlayStation console.
- [ ] Repeat direct and DERP relay paths, screen-off play, two simultaneous
  consoles, phone revocation, Wi-Fi loss/change and both Stop controls.
- [ ] Run a 60-minute session and record the same battery, thermal, throughput
  and cleanup measurements.

Do not publish console compatibility claims until Xbox and PlayStation results
are recorded separately. Emulator/JNI checks, protocol unit tests and
echo-server integration tests do not replace these hardware checks.
