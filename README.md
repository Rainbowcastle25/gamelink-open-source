<img src="assets/icon.svg" width="64" height="64" alt="">

# GameLink

**Private gaming, made simple.**

GameLink lets you host a game server on your own machine and let a few friends
join it **without opening a single port on your router**. There is no account,
no public server list and no cloud service of ours in the middle.

> **Status: working alpha (0.2.0-alpha).** The Go transport, access control,
> legacy TCP/UDP tunnels, and LAN-to-LAN Bedrock Console Bridge have automated
> coverage. Android builds for arm64 and x86_64 are available for local
> evaluation. Xbox and PlayStation hardware, screen-off play, and sustained
> thermal and battery behavior have **not** been validated. There is no
> app-store release; see [Alpha limitations](#alpha-limitations) and the
> [Console Bridge guide](docs/android-console-bridge.md).

## Goal

A safe and secure way to play with your friends without opening ports on your
router or setting up port forwarding.

The aim is to support pretty much any game you can play in multiplayer, from
dedicated servers to plain LAN sessions: a Minecraft world opened to LAN, a
Stronghold Crusader lobby, and so on.

GameLink starts with three games:

- Minecraft: Bedrock Edition
- Minecraft: Java Edition
- Factorio

Other games come later; see the [roadmap](docs/roadmap.md).

## How it works

```
 Friend's PC / Android phone                            Your server (Linux)
┌────────────────────┐                                  ┌──────────────────────┐
│ Minecraft          │                                  │  game server         │
│  ↓ 127.0.0.1:25565 │                                  │  127.0.0.1:25565     │
│ GameLink (Windows) │  ── encrypted WireGuard (Tailcat) │        ↑             │
│ Android bridge    │ ═══════════════════════════════► │  gamelink-agent      │
└────────────────────┘   direct, or via a relay if the   └──────────────────────┘
                         NAT cannot be crossed
```

- **`gamelink-agent`** (Rust host with a Go Tailcat shim, Linux) runs next to your game server. For each
  game it creates its own [Tailcat](https://github.com/tailscale/tailcat)
  identity and forwards one tunnel port to the game. Only devices **you have
  approved** can reach it.
- **GameLink** (desktop app: Tauri, Rust, React, TypeScript) is what your
  friends run. It has its own device key, imports an invitation, and offers a
  local `127.0.0.1` address that the game connects to. The Join button opens
  the tunnel; Disconnect closes it. With the game's program set in Settings,
  **Play** does both in one click: it connects, then starts the game pointed at
  the server (Factorio and Minecraft Bedrock join by themselves; see below).
- **Tailcat** provides the encrypted transport: WireGuard, direct UDP
  hole-punching, and a DERP relay fallback when a direct path is impossible.
- **Android Console Bridge** is an explicit LAN-to-LAN extension for Bedrock
  consoles. The phone is approved for a game, then forwards console discovery
  and encrypted media over Tailcat to the configured Bedrock server. Consoles
  share that phone's per-game GameLink approval; they are not approved as
  separate GameLink devices.

The invitation contains the server's Tailcat address, which is a **secret
capability**. Treat it like a password and share it privately. A leaked
address alone is still not enough: the person must also hold a device key you
approved.

## Features

| | |
|---|---|
| Transport | Real Tailcat: WireGuard, hole-punching, relay fallback, no port forwarding |
| Access control | Per-game allowlist. **A game with no approved devices is never served.** Revocation restarts the game's listener and drops live sessions |
| Protocols | TCP (Minecraft Java), UDP (Factorio, legacy Bedrock), desktop Bedrock NetherNet signaling/media, and optional Console Bridge for Bedrock consoles |
| Identity | Windows device key sealed with DPAPI; Android bridge key protected by Android Keystore; separate key per hosted game |
| Desktop app | Dashboard, Servers, Requests, Friends, Settings; connection status with the real network path (direct or relay) and latency |
| Resilience | Auto-reconnect with backoff, local port fallback if the preferred port is busy, diagnostics report with secrets redacted |
| Agent CLI | `init`, `games add/list/remove`, `devices approve/revoke/list`, `invite`, `serve`, `status` |
| Multiple servers | Import several invitations; connect to several games at once |

See [docs/getting-started.md](docs/getting-started.md) for the full tutorial.
To have an AI coding agent set up your server, ask it to "set up GameLink on
my game server"; it follows [docs/agent-setup.md](docs/agent-setup.md).
For the Android LAN bridge, see [docs/android-console-bridge.md](docs/android-console-bridge.md).
For desktop Bedrock, update both app and agent and configure the BDS media range;
see [desktop NetherNet setup](docs/bedrock-research.md#desktop-manual-server-connections).
GameLink reports agent connectivity separately from game endpoint availability.

## Quick start

On the Linux machine that runs your game server (Minecraft Java as the example):

```sh
gamelink-agent init
gamelink-agent games add --type minecraft-java --name "Our Minecraft"
# your friend opens GameLink, copies their public key from Settings and sends it to you
gamelink-agent devices approve --game our-minecraft --key nodekey:<friend's key> --label "Sam's PC"
gamelink-agent invite --game our-minecraft      # prints a SECRET invitation; send it privately
gamelink-agent serve                            # or run it as a service (see docs)
```

On your friend's Windows PC: GameLink, **Servers**, **Add server**, paste the
invitation, **Join**, then in Minecraft add the server at the address GameLink
shows (normally `127.0.0.1:25565`).

**One-click Play.** In Settings, under *Local game executables*, pick the
game's program. The server card then shows **Play**. It joins, waits until the
tunnel is up, and starts the game:

| Game | What Play does |
|---|---|
| Factorio | Starts `factorio.exe --mp-connect 127.0.0.1:<port>`; Factorio joins on start |
| Minecraft Bedrock | Needs no program: opens a `minecraft://connect` link that joins the server |
| Minecraft Java (official launcher) | Starts the launcher and copies the address; the launcher cannot be told a server, so paste it under Multiplayer |
| Minecraft Java (Prism Launcher) | Set the connect arguments to `-l <instance> -s {address}` and it joins directly |
| Other | Your own connect arguments, with `{address}`, `{host}` or `{port}` |

The arguments only ever contain GameLink's local address, never the server's.
**Join only** still connects without starting anything.

To stop someone: `gamelink-agent devices revoke --game our-minecraft --key nodekey:...`

## Build from source

Requirements: Go 1.27+, Rust 1.100 with `llvm-tools`, Zig 0.15.1, Node 22+, and for the desktop app the
GTK 3/WebKitGTK 4.1 on Linux, or MinGW-w64 gcc/binutils and the WebView2 runtime
on Windows. See `scripts/build-desktop.sh` and the build guide.
For Android builds, use Go 1.27.4, Rust 1.100, JDK 21, Android SDK 37,
NDK 29.0.14206865, and the checked-in Gradle 9.9.1 wrapper; see
[docs/building.md](docs/building.md).

```sh
# Server agent (Linux, static binary, with the hardened tag set)
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
rustup component add llvm-tools
bash scripts/build-agent.sh amd64          # use arm64 for ARM servers

# Desktop app (on Windows): build Rust and configure Zig cgo first; see docs/building.md

# Tests (build the host Rust archive first)
bash scripts/build-rust-ffi.sh
go vet ./...
go test ./...                              # hermetic: spins up its own local relay, no internet needed
go test -tags external -run TestPublicRelay -v ./tests/integration   # optional: uses the real public relays
cd apps/desktop/frontend && npm ci && npm run lint && npm run typecheck && npm test && npm run build
```

**Important:** the tests must be built *without* the `build-tags.txt` tags;
those tags remove code the test relay needs. Production binaries must be built
*with* them (they also strip UPnP/NAT-PMP port mapping and the Tailcat SSH
server, neither of which GameLink wants). [docs/building.md](docs/building.md)
explains this.

## Security in one paragraph

The desktop tunnel listener is loopback-only. The optional Console Bridge
exposes only a constrained Bedrock discovery/media path to the selected Wi-Fi
subnet while its foreground service runs. GameLink has no SSH, shell, file
transfer, UPnP, NAT-PMP, or exit-node feature. Optional Pterodactyl host
administration uses a separate authenticated, loopback-only interface. Tailcat's SSH/SFTP
server and the UPnP/NAT-PMP port mapper are compiled out of the production
binaries, and CI fails if that code is linked back in; exit-node and routing
behaviour are covered by tests rather than by a build check. Secrets (device
key, game keys, invitations, Tailcat addresses) are written with owner-only
permissions, sealed with DPAPI on Windows, and redacted from logs and
diagnostics. GameLink is **not** "100% secure"; read the
[threat model and its limits](docs/security.md) before relying on it.

## Documentation

- [Getting started](docs/getting-started.md): the end-to-end tutorial
- [Pterodactyl host administration](docs/pterodactyl.md): manual discovery and registration
- [Server setup runbook for AI agents](docs/agent-setup.md)
- [Architecture](docs/architecture.md)
- [Security and threat model](docs/security.md)
- [Building and releasing](docs/building.md)
- [Android Console Bridge](docs/android-console-bridge.md)
- [Roadmap](docs/roadmap.md)
- [Contributing](CONTRIBUTING.md), [Security policy](SECURITY.md),
  [Third-party notices](THIRD_PARTY_NOTICES.md)

## Alpha limitations

- Verified with automated TCP and UDP echo/protocol tests through a real
  Tailcat relay. **Not verified with the real game clients** (Minecraft Java,
  Factorio, Minecraft Bedrock).
- The Windows desktop and Linux host agent are for PC play. The Android app is
  an experimental Bedrock console bridge; direct Xbox/PlayStation hardware,
  screen-off operation, revocation while consoles are active, Wi-Fi-loss
  recovery, battery use, and thermal behavior still need validation. No iOS
  client is supported; iOS has separate local-network and background-service
  requirements.
- A rejected (unapproved) device sees the same "no response" as an offline
  server. That is deliberate (it reveals nothing to strangers) but means
  GameLink can only say "offline, or not approved".
- Relays (DERP) can see that two keys talk and how much, though never the
  contents. Using the public relays is a trust decision; you can run your own
  with `--relay-host`.
- The legacy UDP tunnel drops datagrams over 1232 bytes. The opt-in console
  bridge fragments and reassembles Bedrock media datagrams up to 65,507 bytes;
  desktop NetherNet frames media over authenticated TCP streams. TCP may add
  head-of-line delay on lossy paths. Desktop NetherNet supports IPv4 candidates
  at the configured BDS address and permitted media range.
- No auto-update. No installer signing yet. Windows SmartScreen will warn.
- The Tailcat wrapper has not been hardened against a hostile approved
  device. Only approve people you trust.

## License

Copyright (C) 2026 the GameLink authors.

GameLink is free software: you can redistribute it and/or modify it under the
terms of the GNU General Public License, version 3, as published by the Free
Software Foundation. It is distributed WITHOUT ANY WARRANTY; see
[LICENSE](LICENSE) for the full text. Third-party components keep their own
licenses and are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
