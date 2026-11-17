# Getting started

This walks through a Linux machine hosting Minecraft Java and a friend on
Windows joining it. The automated tests prove the tunnel, access control and
TCP/UDP behavior with stand-in echo servers through Tailcat. Real PC game
clients and console hardware remain separate validation work; see the
[Android Console Bridge guide](android-console-bridge.md) for its current
build status and hardware checklist.

Legend: ✅ covered by automated tests, 🟡 documented but not exercised with the
real thing.

You will need:

- **Host:** a Linux machine (x86-64 or arm64) that runs your game server, with
  outbound internet access. No inbound ports need to be opened.
- **Friend:** a Windows 10/11 PC with the WebView2 runtime (already present on
  current Windows 11).

---

## A. Install the agent on the Linux server

Install Rust 1.100, Go 1.27.4 and Zig 0.15.1 as described in
[building](building.md#server-agent-linux), then build it (or download a release
when one exists):

```sh
git clone <this repository> gamelink && cd gamelink
bash scripts/build-agent.sh amd64 # use arm64 for an ARM server
sudo install -m 0755 dist/gamelink-agent-linux-amd64 /usr/local/bin/gamelink-agent
gamelink-agent version
```

Create a dedicated user and the configuration directory (recommended, so the
agent is not running as you or as root):

```sh
sudo useradd --system --create-home --home-dir /var/lib/gamelink-agent --shell /usr/sbin/nologin gamelink
sudo -u gamelink env GAMELINK_AGENT_DIR=/var/lib/gamelink-agent/config gamelink-agent init
```

For the rest of this guide, run agent commands as that user with
`GAMELINK_AGENT_DIR` set (or pass `--dir`). To make it painless:

```sh
alias gl='sudo -u gamelink env GAMELINK_AGENT_DIR=/var/lib/gamelink-agent/config gamelink-agent'
```

## B. Run Minecraft Java on the same machine

Run your server as usual. Make sure it listens on this machine, which is the
default (`server-port=25565` in `server.properties`). **Do not** port-forward it
on your router and you do not need to change firewalls: GameLink reaches it
through the loopback address.

If the server is in Docker or Pterodactyl, publish its port on the host and use
`--dest` below (for example `--dest 172.18.0.2:25565`).

Then tell the agent about it:

```sh
gl games add --type minecraft-java --name "Our Minecraft"
```

Output:

```
Added game "our-minecraft" (Our Minecraft)
  tcp port 25565 in the tunnel -> 127.0.0.1:25565
  relay: New York City
Nobody can reach it yet: no devices are approved.
```

(Your nearest public relay is picked automatically. ✅)

Starting the agent now would expose nothing: a game with no approved devices is
never served. ✅

## C. Install GameLink on the friend's Windows PC

Run `gamelink.exe` (build instructions: [building.md](building.md)). It is not
code-signed yet, so Windows SmartScreen shows "unknown publisher": choose
*More info*, then *Run anyway*, only if you got the file from someone you trust.

On first start GameLink creates this PC's **device key** and stores it
encrypted for the current Windows user. Nothing is sent anywhere.

## D. Copy the friend's public key

In GameLink open **Settings**, find the **Public key** row and press **Copy Public
Key** (the **Requests** tab shows the same key together with the approval command
to send to the host). The key starts with `nodekey:`. It is public by design (it only identifies this PC), so it is fine
to send in a chat message. ✅

The friend sends it to the host.

## E. Approve the key (host)

```sh
gl devices approve --game our-minecraft --key nodekey:<paste here> --label "Sam's PC"
gl devices list
```

Only the person who controls the server can do this. The optional
[Pterodactyl Host screen](pterodactyl.md) also lets an authenticated host
administrator approve their current device and import a game. ✅

## F. Create the invitation and share it privately (host)

```sh
gl invite --game our-minecraft
```

It prints one long line starting with `glink1.` to standard output (and a
reminder to standard error). **This is a secret.** It contains the server's
Tailcat address. Send it by direct message or in person. Never post it in a
public channel, a stream, a screenshot, a GitHub issue or a Discord server. ✅

## G. Start the agent (host)

Foreground, to try it:

```sh
gl serve
```

As a service (recommended): see
[`config/examples/gamelink-agent.service`](../config/examples/gamelink-agent.service),
then:

```sh
sudo cp config/examples/gamelink-agent.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now gamelink-agent
gl status
```

The unit's sandbox settings were tested by running `serve` under the same
properties with `systemd-run --user` (not as a root system service). Note that
`AF_NETLINK` must stay in `RestrictAddressFamilies`; without it the agent cannot
enumerate network interfaces and the game fails to start. 🟡

`gl status` shows each game, its approved device count, whether it is serving,
and the number of accepted and denied connections. ✅

## H. Import the invitation and Join (friend)

In GameLink: **Servers → Add server → Import invitation**, paste the whole
`glink1.…` text. GameLink shows the name, game and port for confirmation (never
the secret). Press **Add server** in the dialog, then press **Join** on the new
server's card. (If you have set Minecraft's program in **Settings → Local game
executables**, the card shows **Play** instead. Play joins and then starts the
game, copying the address for you; **Join only** just connects.)

Within a few seconds the card shows **Connected**, the real network path
(**Direct** or **Relay**), the latency, and the local address, normally
`127.0.0.1:25565`. If 25565 is busy (say the friend runs a local server too),
GameLink picks a free port and tells you which. ✅

If it says **no response**: the host's agent is not running, or the host has not
approved this PC's key yet (GameLink cannot tell the two apart; see
[security.md](security.md)). Use **Check** on the server card to retest, and open the card's details for
**Diagnostics**, a redacted report you can send to the host.

## I. Connect from Minecraft Java (friend)

With the tunnel connected, start Minecraft Java Edition, **Multiplayer → Add
Server**, and enter the local address shown in GameLink (for example
`127.0.0.1:25565`). Join. 🟡 *Minecraft's own protocol has not been exercised
end to end; the tunnel carries arbitrary TCP and is tested with an echo server,
including half-close behaviour.*

Tip: the server's `online-mode` and whitelist are separate from GameLink. Keep
`online-mode=true` unless you know why you are changing it, and consider also
using Minecraft's whitelist.

## J. Revoke access (host)

```sh
gl devices revoke --game our-minecraft --key nodekey:<the key>
```

A running agent applies it within a few seconds: that game's listener restarts,
the revoked device's open sessions end, and it can no longer connect. Other
approved devices reconnect automatically after the brief restart. ✅

If the invitation leaked (not just one device), remove and re-add the game to
issue a new address, then re-approve and re-share:

```sh
gl games remove our-minecraft
gl games add --type minecraft-java --name "Our Minecraft"
# approve devices again, create a new invite
```

---

## Factorio and legacy Bedrock forwarding (UDP) 🟡

Both use UDP. The ordinary UDP path (datagram boundaries, several local
endpoints, the 1232-byte size limit, unauthorized keys, and port release on
disconnect) is tested with an echo server, including over a relay. The real
games have not been tested.

```sh
gl games add --type factorio --name "Our Factory"          # UDP 34197
gl games add --type minecraft-bedrock --name "Bedrock"     # UDP 19132
```

Then approve, invite, join exactly as above. In Factorio use **Multiplayer →
Connect to address** with the local address GameLink shows.

Known caveats for this legacy single-port mode:

- UDP datagrams larger than **1232 bytes** are dropped (counted in `gl status`
  as oversize drops). If a game uses larger datagrams it will not work yet.
- **Bedrock on Windows (Microsoft Store edition)** blocks connections to
  `127.0.0.1` by default. You may need to grant a one-time loopback exemption
  from an administrator prompt, which weakens a Windows protection for that app:
  `CheckNetIsolation LoopbackExempt -a -n="Microsoft.MinecraftUWP_8wekyb3d8bbwe"`.
  Unverified. Consoles need the separate Android LAN-to-LAN bridge, not a
  loopback address; see [Android Console Bridge](android-console-bridge.md).
- Latency-sensitive games are best on a **Direct** path. If GameLink says
  **Relay**, a NAT could not be traversed and traffic goes via a relay
  (slower but still encrypted end to end).

The Android Console Bridge is opt-in and changes the host to authenticated TCP
control plus UDP media on the configured Bedrock game tunnel. It supports
larger fragmented media datagrams than this legacy path, but Xbox and
PlayStation hardware behavior is not yet validated. Follow the separate guide
for host configuration, local Android build/install, privacy boundaries and
hardware checks.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| "no response from the server" | Agent not running, not approved, wrong/old invitation, or the host has no outbound internet |
| Connected, but the game says refused/timeout | The game server itself is down or listens elsewhere. GameLink shows *game not reachable* when the agent cannot reach the destination |
| Works on Relay only | Strict NAT; normal. Latency will be higher |
| "port busy" | Pick another local port in the server's settings |
| `status` says "inactive (no approved devices)" | By design; approve a device |
| Friend's PC changed or reinstalled | New device key; approve the new key and revoke the old one |

Collect a redacted report with **Diagnostics** in the app, or
`gl status` on the host. Neither contains keys or addresses.
