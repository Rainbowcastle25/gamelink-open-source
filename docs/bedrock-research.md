# Bedrock NetherNet bridge research

Research checked 2026-11-14 against Mojang's protocol guide and Bedrock 26.60
changelog, and `paddedhelmets/bedrockproxy` at commit
[`d3fa0ecb1e53bd5f55bcb9d24267f99c11067e4c`](https://github.com/paddedhelmets/bedrockproxy/tree/d3fa0ecb1e53bd5f55bcb9d24267f99c11067e4c).
The upstream repository is MIT licensed. This is a source-based architecture
review, not a test on Minecraft hardware.

## Protocol facts

Mojang's [NetherNet onboarding guide](https://mojang.github.io/bedrock-protocol-docs/guides/nether-net-onboarding-guide/)
defines an HTTP `GET /v1/join` capability/metadata check and a `POST
/v1/join/{networkId}` carrying a complete SDP offer; the response carries a
complete SDP answer. `networkId` is explicitly opaque. After signaling, ICE
checks establish the UDP path, then DTLS and SCTP data channels carry game
traffic. The guide describes two channels (reliable and unreliable), and full
ICE gathering with UDP candidates.

The guide's identity assertions bind the authenticated account's public key to
the SDP's DTLS certificate fingerprint. The client requires a server
`a=identity` assertion in the answer. WebRTC then checks that the presented
DTLS certificate matches the SDP fingerprint. Candidate addresses are not part
of the fingerprint assertion. Therefore an L3/L4 relay can rewrite only
`a=candidate` address/port fields while forwarding the identity and fingerprint
lines verbatim, then relay UDP packets without decoding them. The signature and
DTLS protection remain end-to-end. Rewriting the fingerprint, identity, or
certificate would break this property or change which identity the player
authenticates. ICE credentials are not covered by the fingerprint assertion;
changing them would instead require corresponding changes to STUN
authentication/routing and is unnecessary for a candidate-only relay.

Mojang's [26.60 changelog](https://www.minecraft.net/en-us/article/minecraft--bedrock-edition-26-60-changelog)
says the dedicated server's non-NetherNet warning becomes an error because
RakNet is deprecated. The protocol reference currently describes 1.26.70 beta
as protocol 2259 (dated 2026-11-10). These are live/versioned sources; server
and client behavior should still be checked against the exact deployed builds.

## Reference implementation findings

The pinned MIT project has two useful but distinct paths:

- `netherproxy/relay.go`: bounded HTTP forwarding for `GET /v1/join` and
  `POST /v1/join/{id}`; SDP candidate rewriting; a per-negotiation UDP socket;
  ICE STUN authentication and bounded candidate/transaction/rate handling;
  byte-forwarding of media. It does not terminate DTLS or inspect Bedrock
  payloads. Candidate rewriting in this implementation is IPv4/UDP only.
- `netherproxy/lan.go`: experimental UDP LAN discovery/signaling bridge using
  `github.com/df-mc/go-nethernet`. It advertises `discovery.ServerData`, accepts
  offer/candidate/cancel signals, bounds pending negotiations and buffers,
  gathers trickled LAN candidates, then translates the offer into the HTTP
  signaling handler. It does not manufacture an identity assertion.
- `netherproxy/tunnel.go`: LAN-to-LAN NetherNet signaling forwarding, with
  optional ICE media assistance if upstream BDS candidates are unreachable.
- `nethernet/nethernet.go`, `proxy/nethernet.go`: lifecycle/configuration and
  CLI wiring. `netherproxy/relay_test.go`, `lan_test.go`, and `tunnel_test.go`
  exercise local protocol behavior.

The module uses Go 1.25, `github.com/df-mc/go-nethernet v1.0.26`,
`github.com/pion/webrtc/v4` (indirect), Pion ICE/DTLS/SCTP, and go-raknet.
Notable production limits in the reference: IPv4-only candidate relay; one
ICE credential pair and bundled transport; per-session dynamically allocated
UDP relay ports; it requires the operator to supply reachable advertised
addresses and firewall/NAT mappings. The upstream README says LAN discovery is
experimental and not validated on Xbox/PlayStation/Switch hardware; the HTTP
relay has been joined from a mobile client. Treat those as the upstream
author's claims, not independent verification.

## GameLink architecture needs

Before desktop NetherNet support, GameLink's Bedrock entry was just UDP port 19132. The legacy tunnel
forwards one configured TCP or UDP port to one local destination. It has no
NetherNet discovery listener, signaling bridge, per-peer ICE/media allocations,
or SDP candidate mapper. Its UDP tunnel also enforces a 1232-byte datagram
limit. A single static UDP forward is not a complete NetherNet bridge: the SDP
exchanges routable ICE candidates, and the media UDP path must follow those
candidates for each peer.

A crucial hardware finding in the pinned upstream's
[`docs/nethernet-migration.md`](https://github.com/paddedhelmets/bedrockproxy/blob/d3fa0ecb1e53bd5f55bcb9d24267f99c11067e4c/docs/nethernet-migration.md)
changes the console design: the author reports that LAN-discovered clients omit
the `a=identity` GameServerToken assertion, while Mojang BDS rejects an
assertion-less offer with NetherNet error 37 (`IdentityNotAllowed`). The
upstream author says LAN-to-HTTP offer translation is therefore blocked for
Mojang BDS; that author reports successful LAN-to-LAN signaling tunnel play on
hardware (2026-10-31). This is an upstream author's packet-capture and hardware
report, not independently reproduced here. Do not use the HTTP-translation
`LANBridge` as the console path or claim console support from it. For the
console workflow, preserve the upstream LAN discovery advertisement and relay
native LAN signaling messages through GameLink to the host's NetherNet LAN
endpoint. HTTP signaling remains a separate route for clients that use manual
server entry and produce identity assertions.

A complete LAN-to-LAN console bridge needs three cooperating pieces:

1. **LAN discovery/signaling on the joining device.** Listen on the Bedrock
   NetherNet LAN discovery port (the reference uses UDP 7551), relay the
   upstream server's actual discovery metadata, and receive/forward native
   console discovery and signaling messages. Preserve the upstream metadata,
   nonce/auth policy, network identity, and signal payloads. Bind/advertise a
   real LAN-reachable address for console discovery; `127.0.0.1` is not
   sufficient for a console.
2. **Authenticated signaling over GameLink.** Carry bounded discovery/signaling
   messages over the already authenticated Tailcat peer tunnel to the host
   agent, which forwards them to the real BDS NetherNet LAN discovery endpoint
   and returns replies unchanged. Preserve the native LAN authentication and
   nonce flow. A bounded binary or length-prefixed control protocol is
   preferable to exposing a new unauthenticated HTTP listener. A lifecycle API such as
   `Start(config)`, `Stop()`, `Status()` can own its listeners, sessions,
   limits, and cleanup.
3. **Per-session UDP media relay across Tailcat.** Allocate an isolated client
   facing socket and an agent/upstream facing socket per ICE negotiation.
   Rewrite each side's SDP candidates to the opposite relay endpoint. Forward
   STUN checks and encrypted DTLS/SCTP datagrams byte-for-byte through Tailcat;
   do not parse game channels or terminate DTLS. Expire idle sessions and
   release ports promptly. The host side must dial only the configured BDS
   address and an explicit configured media-port range (not arbitrary SDP
   candidate targets).

NetherNet LAN signaling has its own discovery metadata, nonce/auth policy and
trickled signaling sequence; preserve and forward these per the native
protocol. Do not synthesize them from the HTTP `/v1/join` metadata, which does
not contain all LAN discovery fields. The upstream HTTP relay can still serve
HTTP-signaling clients as a distinct route; it must not substitute for LAN
signaling to Mojang BDS.

## Security and limits

- Tailcat authenticates/authorizes the device-to-agent control and media
  transport; it does not authorize a Bedrock account. BDS remains responsible
  for Xbox/Microsoft account validation and player admission.
- LAN discovery and its shared wire key are not authentication. Treat the
  discovery listener as LAN-trusted; do not expose it to the internet. An
  untrusted LAN peer could otherwise create signaling load or abuse ICE
  forwarding.
- SDP candidates are attacker-controlled input. Do not dial arbitrary
  candidate IPs/ports. Pin upstream destinations to configured server
  addresses and media-port ranges, enforce candidate/body/session limits, and
  constrain forwarding to peers authenticated by the Tailcat allowlist.
- Signaling may expose player/session metadata and ICE credentials. Redact SDP,
  `a=identity`, ICE passwords, and candidates from normal logs and diagnostics.
- Keep IPv4/IPv6 support explicit. The reference relay is IPv4-only; silently
  dropping IPv6 candidates may fail on IPv6-only or dual-stack console LANs.
- Relay availability depends on UDP reachability through both the local LAN and
  the Tailcat path. Do not imply NAT traversal or console support until verified
  for the target topology.
- The upstream README calls NetherNet LAN discovery experimental. Its later
  migration note reports LAN-to-LAN play through the tunnel on a physical LAN
  client, but does not independently validate Xbox, PlayStation, or Switch
  here. No Bedrock hardware test was run for this research.

## Version/toolchain availability

The repository pins `go 1.27.4`. On the research date, the official Go download
index listed `go1.27.4` (and `go1.26.11`), so that version is real and
published. The default shell PATH on this research worker had no `go` binary;
the parent agent separately supplied a Go toolchain under
`/tmp/gamelink-tools/go/bin`. No build/test result is claimed here.

## Sources

- [Mojang NetherNet HTTP Signaling — Partner Onboarding Guide](https://mojang.github.io/bedrock-protocol-docs/guides/nether-net-onboarding-guide/)
- [Minecraft Bedrock Edition 26.60 changelog](https://www.minecraft.net/en-us/article/minecraft--bedrock-edition-26-60-changelog)
- [Pinned bedrockproxy tree](https://github.com/paddedhelmets/bedrockproxy/tree/d3fa0ecb1e53bd5f55bcb9d24267f99c11067e4c)
- [Official Go downloads JSON index](https://go.dev/dl/?mode=json)

## Desktop manual-server connections

Update both the desktop app and host agent. Normal `minecraft-bedrock` games
now open TCP HTTP signaling and legacy UDP at the same loopback port, normally
`127.0.0.1:19132`. An occupied TCP **or** UDP port causes a fallback (or an
error if the player requested an exact port). Enter the address GameLink shows.

Configure BDS with a bounded `server-udp-ports` range, then allow that same
range in GameLink. For example, with BDS `server-udp-ports=19140-19145`:

```sh
gamelink-agent games add --type minecraft-bedrock --id bedrock \
  --dest 127.0.0.1:19132 --media-ports 19140-19145
```

For an existing game, the equivalent game entry in `agent.json` is
`"bedrock": {"mediaPortFirst": 19140, "mediaPortLast": 19145}`. The agent
reloads it and ends existing sessions. The range may contain at most 256 ports.
Without it, GameLink permits only the destination's port; this supports a
single-port setup but does not cover dynamically allocated BDS media ports.
The configured IPv4 destination must match the BDS answer's candidate address
and its media ports must be reachable from the agent. IPv6-only and translated
candidate addresses/ports are not supported by this desktop path.

GameLink relays `GET /v1/join` metadata and `POST /v1/join/{networkId}` offers.
Each successful negotiation owns two UDP sockets, with only candidate
address/port fields changed. Identity assertions, fingerprints, ICE credentials
and encrypted packets are preserved. Media datagrams up to 65,507 bytes are
length-framed over a dedicated authenticated Tailcat TCP stream. This avoids
the legacy 1232-byte limit, but TCP can add head-of-line delay on lossy links.
Sessions expire after two minutes without traffic and close on disconnect or
device revocation. Legacy UDP forwarding remains available on the displayed port.

The desktop probes BDS's HTTP capability endpoint separately from the agent.
An agent that responds while BDS does not is shown as a game server offline;
generic UDP games and silent TCP endpoints, which have no universal safe probe,
show game status unknown.
Health refreshes while connected. This checks endpoint availability, not player
admission or completion of a Minecraft join. Automated tests use synthetic
signaling/media through the real encrypted tunnel; no real Minecraft play is
claimed.

## Hosting the console bridge

Configure a Bedrock LAN server and its permitted UDP media ports on the server agent:

```sh
gamelink-agent games add --type minecraft-bedrock --console-bridge \
  --bedrock-ip 192.168.1.10 --bridge-bind-ip 192.168.1.20 \
  --media-ports 19132-19136
```

The bind address must belong to the agent's server-facing LAN interface. The Bedrock server's discovery service must be reachable at UDP port 7551, and its negotiated UDP media ports must fall within the configured range (at most 256 ports). Approve the phone's device and create an invitation as for other games; the invitation identifies the console bridge automatically. Configuration changes and device revocation restart the bridge and end active sessions. The bridge forwards the server's unmodified discovery advertisement and authentication settings. Control and media travel through the authenticated Tailcat connection; the agent only sends LAN traffic to the configured server and negotiated allowed media ports.
