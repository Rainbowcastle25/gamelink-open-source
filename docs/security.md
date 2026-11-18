# Security model and threat model

GameLink is an alpha. It is designed to be conservative, but **nobody should
call it "100% secure"**, and neither do we. This document says what it tries to
protect, what it assumes, and what it does not defend against.

## What we are protecting

1. **Your game server and the machine it runs on** from strangers on the internet.
2. **The list of people who may play** (the allowlist) from being widened by
   anyone but you.
3. **Your friends' devices** from being used as a way into their network.
4. **Secrets** (private keys, the server's Tailcat address) from leaking through
   logs, screenshots, telemetry or files in a repository.

## Design rules

| Rule | How it is enforced |
|---|---|
| No router port forwarding, no UPnP/NAT-PMP | Nothing opens an inbound port on the router. The port-mapper code is compiled out of production builds (`ts_omit_portmapper`) and CI fails if it comes back |
| A game with no approved devices is unreachable | The knock-only mode keeps Tailcat's allowlist restrictive and serves zero TCP or UDP ports (see [architecture](architecture.md)); covered by tests |
| Per-game identity | Own key + PSK per game; no key is shared across games or with the desktop identity |
| Agent invitation rotation and expiry | Rotation replaces the per-game key and PSK. At expiry the host retires the whole identity, including established sessions. Device approvals carry over to the next identity |
| Revocation works | Closes the game's server and restarts it with the remaining keys; live TCP and UDP sessions end. Tested end to end |
| Narrow listeners | Desktop client game sockets and desktop host destinations use `127.0.0.1`. Minecraft Java discovery receives only multicast announcements: Windows binds the wildcard address but accepts only group-destined datagrams from this PC. The Linux agent defaults to a loopback destination. The explicit Bedrock Console Bridge uses a Wi-Fi discovery/media path scoped to one configured server IPv4 and media-port range |
| No SSH, shell, file transfer | No such handler exists in GameLink. Tailcat's SSH/SFTP server is compiled out (`ts_omit_ssh`). Optional host administration is a bearer-authenticated, loopback-only API for explicit Pterodactyl refresh and per-game device approval/import. It is off by default and exposes no shell, console, files, or power controls |
| No general LAN access or exit node | No route forwarding is enabled. The optional Console Bridge only accepts traffic for the selected Bedrock server and configured media range; it does not expose other host or LAN ports |
| Secrets are not logged | A redactor scrubs private keys, PSKs, Tailcat addresses and invitations from logs and diagnostics. Verbose Tailcat output is off by default and redacted when on |
| Secrets are not committed | `.gitignore` excludes key material; `config/examples` contain placeholders only; CI scans for key-shaped strings |
| No telemetry | The apps make no analytics calls. Release checks are off by default on desktop and are manual on the Linux agent. The HTTPS GETs carry a generic User-Agent and no cookies, credentials, installed version or device identifiers |

Desktop presence notifications are off by default. With the setting enabled,
checks go through the same Tailcat probe as **Check** and reveal to the host's
agent that your device is online. Observations and notification limits are kept
in memory on the player’s device. The optional Minecraft list stores server
display names with loopback addresses.

## Trust boundaries

```
Internet strangers ──X──► your server    (no public game listener; handshake needs the
                                           address secret AND an approved key)
Approved PC ──────────────► one game port (desktop endpoint is loopback)
Approved Android phone ───► one configured Bedrock bridge, while foreground service runs
Console on phone's Wi-Fi ─► that phone's Bedrock game approval and server session
Relay operator ───────────► sees: which keys talk, when, how much.   NOT: contents
Invitation holder (not approved) ──X─►   (handshake gets no reply)
```

### Who can do what

- **A stranger with no information**: cannot find the server (it listens on no
  public port) and cannot guess a Tailcat address.
- **A stranger who obtains the invitation/address but not an approved key**:
  gets no handshake reply at all. This is tested. Note this means a rejected
  device sees exactly what an offline server looks like. An invitation holder
  can cause a DERP-authenticated peer key to appear in the host's private
  request list. This happens before WireGuard checks the PSK, so knowing the
  server's public key and relay suffices to knock. A request identifies a
  device key, and the host must separately confirm who controls it before
  approving.
- **An approved friend**: can connect to the single game port you exposed, as
  any player. They cannot reach other ports, the LAN, or run commands. If the
  game itself has a vulnerability or an admin console on that port, that is
  reachable by them.
- **An approved Android phone**: can run the explicit Bedrock Console Bridge
  while the foreground service is active. Consoles on that selected Wi-Fi
  share the phone's approval for that game; GameLink does not authenticate
  each console separately. Minecraft/BDS still handles player identity and
  admission. The host restricts discovery to the configured BDS IPv4 on UDP
  7551 and media candidates to that same IP and a configured range of at most
  256 ports. It does not become a general LAN forwarder.
- **A revoked friend**: loses access within a few seconds of the agent noticing
  (the revocation command prints when it will apply) and cannot reconnect.
- **A relay (DERP) operator, public or yours**: can see metadata (public keys,
  timing, volume) but not traffic contents (WireGuard, end to end).
- **An attacker on the friend's PC**: can read the friend's identity (DPAPI
  protects against other Windows users and offline disk theft, not against
  malware running as that user) and so could join games that device is
  approved for. Revoke that device.
- **An attacker on the server**: has the game keys and can impersonate the
  server. Treat the server as the root of trust.

## Desktop hosting

- The desktop hosts one loopback destination per game and uses the same
  Tailcat server, per-game identity, supervisor, and approval policy as the
  Linux agent. Windows protects the hosted private key and pre-shared key with
  DPAPI. The desktop writes invitation text directly to the native clipboard.
- Minecraft Java discovery joins IPv4 multicast `224.0.2.60:4445` on local
  interfaces only while **Host** is open or a LAN world is shared (including
  while that share is paused, below). It stops when Host is closed and no LAN
  world is shared. The socket is receive-only. Windows binds the wildcard
  address, because Winsock delivers multicast only to wildcard or
  receiving-interface binds, and accepts only datagrams whose destination is
  the group. Linux binds the group address. GameLink sends no discovery
  packets and forwards no discovery traffic. It filters announcements by this
  PC's interface addresses and bounds the datagram size, cache and lifetime.
- An announcement is an unauthenticated hint. Sharing or updating a LAN
  world's port requires a click by the host. The destination is always
  `127.0.0.1` on the host PC. A shared world's tunnel pauses when no
  announcement for that world name on its confirmed port has arrived for 10
  seconds (Minecraft announces about every 1.5 seconds), because a closed
  world's temporary port may be reused by another local program. It resumes
  only when the same name reappears on the same port; a new port needs the
  host's click again. A local program could still announce a matching name and
  port while the world is closed. **Stop sharing** forgets the port. The
  desktop requires re-detection of LAN worlds after the app restarts.
- **People** displays pending requests from the shared Tailcat refusal
  mechanism. The app records the time of the last authorized game connection
  locally for approved devices.

## Known limitations and risks (read these)

1. **The invitation/address is a bearer secret for the handshake.** Share it only
   privately. Leaking it is not enough to join (the key must be approved), but it
   removes one layer. Use `gamelink-agent invite rotate --game <id>` to replace
   it without losing device approvals. Rotation closes existing sessions on
   reload. Expiry applies to the entire identity and ends all sessions at the
   deadline. All copies of that identity's invitation share the deadline.
   On desktop, remove and recreate the game in **Host** to replace its
   identity, then approve devices for the new game.
2. **"Unauthorized" looks like "offline".** The protocol gives no rejection
   message; this is good against probing and bad for support. GameLink words its
   error as "no response from the server: it may be offline, or this device has
   not been approved for this game".
3. **Relay metadata.** Public relays learn that key A talked to key B. Use
   `--relay-host` to run your own DERP if that matters to you.
4. **Public relay availability.** If the public relays are unreachable and a
   direct path cannot be made, nobody can connect.
5. **Tailcat is young and the wrapper has not been hardened for hostile
   parties.** We rely on WireGuard/Tailscale's data plane for cryptography; we
   have not audited it. We validated the API behaviour from its source, and
   our own code adds the allowlist checks above, but a malicious *approved*
   device could try to exhaust resources (normal TCP sessions are capped at
   256 per game; the desktop client caps local UDP flows at 64; bridge peers
   and sessions have separate host limits) or exploit the game. Only approve
   people you trust.
6. **DPAPI binds the identity file to the Windows user.** Copying the file to
   another PC or user will not work; you must create a new identity and ask the
   host to approve it again. That is intentional.
7. **Local-machine exposure.** Any program on the friend's PC can connect to the
   loopback listener while it is open (loopback is not a security boundary
   against local software). On a shared computer, remember to disconnect.
8. **Console Bridge shares phone authorization.** Every console using a phone
   inherits that phone's per-game allowlist entry. Revoke the phone to revoke
   all of its console sessions. Console traffic is scoped to the configured
   server and media ports, but hardware behavior and the practical effect of
   revocation with active Xbox/PlayStation sessions still need testing.
9. **UDP limits differ by path.** The legacy UDP tunnel drops datagrams over
   1232 bytes and counts them. The opt-in Console Bridge fragments and
   reassembles IPv4 UDP datagrams up to 65,507 bytes in at most 55 Tailcat
   packets; it permits 64 incomplete reassemblies per peer for five seconds.
   Neither path has yet been validated with Xbox or PlayStation hardware.
10. **Android lifecycle and battery.** The bridge requires explicit foreground
    service start and local-network consent. A network/interface change stops
    it and requires another user start. Tailcat connection loss retries with
    backoff. Screen-off, Doze, sustained battery use, thermal behavior, and
    two-console load remain unmeasured.
11. **Android API 37 local-network permission.** Android API 37 prompts for
    `ACCESS_LOCAL_NETWORK`. iOS is not supported; its local-network and
    background-operation permissions require a separate design and validation.
12. **Windows binaries are unsigned,** so SmartScreen will warn. Users download
    and install fixes manually. The optional
    [release notifications](release-manifest.md) verify an Ed25519 signature
    against a build-time Minisign pin before using the manifest. A release host
    can withhold a manifest or replay one on a first check. Once a release is
    verified and cached, checks reject versions older than that verified version.
    The release host and its CDN see the IP address and timing of requests.
13. **Dependency risk.** The Windows/Linux builds include Go packages; Android
    also includes pinned Rust/JNI and AndroidX/Kotlin libraries. The desktop
    ships a small React stack. Versions and license notices are in
    [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md). The development tool
    chain (Vite etc.) has `npm audit` findings that affect only the dev server,
    not the shipped app; this is tracked, not hidden.
14. **Agent status heartbeat.** `gamelink-agent status` reports "running" for up
    to 20 seconds after a hard kill because it reads a heartbeat file.
15. **Pending requests.** Each game keeps at most 32 pending keys for 24 hours
    from their first appearance. New keys are rate-limited, but an invitation
    holder can still fill the list and delay legitimate requests. Dismissal
    frees a slot and a fresh knock can appear again. Both hosts track the
    key, first and last seen times, and attempt count. The Linux agent uses
    a private 0600 `requests.json` file. The desktop keeps pending requests in
    memory until the app exits. Approval always requires an explicit host
    action.

## Reporting vulnerabilities

See [SECURITY.md](../SECURITY.md).

## What the tests cover (security-relevant)

- No client other than an approved key can open a session; a stranger who holds
  the exact address gets nothing (hermetic and over the real public relay).
- A game with zero approved devices serves no game ports even while accepting
  knocks. The host can approve a recorded key and that device can then join.
- Rotation rejects the old address and keeps approvals. An expired address
  rejects an approved device even when it bypasses invitation parsing.
- Revoking a key ends its open TCP and UDP sessions and blocks new ones, while
  other approved keys keep working.
- Invitation parsing rejects oversize, malformed, private-IP and unexpected-port
  relays.
- Redaction removes keys, PSKs, addresses and invitations from logs and
  diagnostics.
- Atomic, owner-only writes of key files; crash-safe replacement.
- Release metadata verification rejects changed bytes, wrong keys, malformed
  manifests and versions older than the last verified release. Desktop retries
  respect the persisted 24-hour interval even after a failed attempt.
- Disconnect releases ports, connections and goroutines.
- Relay-only fallback works and is reported as "relay", not "direct".
- Console Bridge framing, candidate pinning, subnet/source filtering,
  fragmentation/reassembly, session limits, and host cleanup have hermetic
  protocol coverage; these tests do not validate Xbox or PlayStation clients.

## What is deliberately out of scope

Accounts, Cloudflare, Discord sign-in, mobile VPN apps, public discovery, mod
installation, automatic update downloads or installation, iOS, and any public
admin API are out of scope. The Android Console Bridge connects consoles on
the selected LAN to the configured Bedrock server. iOS needs separate
local-network permission, multicast, and background networking policy work
before it can be considered.
