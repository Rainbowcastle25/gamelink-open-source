# Pterodactyl host administration

Run one GameLink agent on each Wings node. In the desktop **Host** screen,
**Refresh** discovers that node's servers, registers mapped games, and takes a
power-status snapshot. It performs no scheduled Panel inventory or status
requests. Opening GameLink or its Host screen does not discover servers.

An AI coding agent can do the node setup below for you by following
[agent-setup.md](agent-setup.md).

New servers are enrolled on the next explicit Refresh. Existing registrations
retain their keys, invitations, approvals, and relay region through game and
agent restarts. A registered tunnel stays available while its game is stopped;
forwarding resumes when the game starts. The host's sampled `offline` status is
separate from the availability of the encrypted tunnel. `running` means Wings
reports a running process, not proof that a game has finished loading.

## Configure one Wings node

Install and initialize the agent as described in [getting started](getting-started.md).
To upgrade, [stop the service, replace the binary, and start it again](agent-setup.md#updating-and-removing).
Keep agent CLI commands idle during the swap.
Keep its configuration and keys outside Pterodactyl's game-container volumes.
The agent must reach the node's published gameplay allocations and the Panel
over HTTPS. No Docker socket, container image changes, or Panel modifications
are required.

Create a separate read-only Pterodactyl Application API key with server and
allocation read permissions. For power status, also create a Client API key
for a user with resource-read access to the games on this node. Without the
Client key, registration works and power status is `unknown`; permission or
connectivity failures also produce `unknown`, never an invented offline result.

Store those keys privately as `panel-application.token` and optionally
`panel-client.token`. For setup discovery, use the Application key with only
Servers and Allocations read permissions:

```sh
gamelink-agent host detect --panel-url https://panel.example.com \
  --token-file /var/lib/gamelink-agent/panel-application.token --json
gamelink-agent host detect --panel-url https://panel.example.com \
  --token-file /var/lib/gamelink-agent/panel-application.token \
  --node-id 2 --out /var/lib/gamelink-agent/host.json --json
```

Detect is read-only on the Panel. It selects an explicit node, otherwise the
single node whose candidate allocation address is a local interface, otherwise
the only node. Ambiguous discovery exits 3 with `selectedNode: null`; choose a
node explicitly before using `--out`. The output groups eggs and proposes
profiles for recognized games, warning about extra allocations. Without egg
name access it uses startup commands for classification only; Java jar
classification has confidence `guess` and `--out` leaves it out. Unknown eggs require a custom TCP/UDP
profile or can remain excluded; bot, Discord and broadcast eggs are excluded.
Startup commands and server environment variables are never printed.

`--out` creates a private host settings file without overwriting, and creates
an independent 0600 `host-admin.token` next to it if absent. It reuses an
existing private admin token. Token files must already be 0600. Review the
proposals before Refresh, and add `clientTokenFile` for power status if needed.
An empty `eggProfiles` object is valid and registers no games.

For manual setup instead, generate an independent admin token in a private file:

```sh
umask 077
openssl rand -hex 32 > host-admin.token
```

Use owner-only files (0600), readable by the agent user. Do not reuse a Panel
key as the admin token. Create `host.json`, also 0600, with your actual node ID,
node address, and egg IDs:

```json
{
  "panelUrl": "https://panel.example.com",
  "applicationTokenFile": "panel-application.token",
  "clientTokenFile": "panel-client.token",
  "adminTokenFile": "host-admin.token",
  "nodeId": 2,
  "nodeAddress": "192.168.1.20",
  "eggProfiles": {
    "1": {"type": "minecraft-java", "protocol": "tcp", "tunnelPort": 25565},
    "15": {"type": "factorio", "protocol": "udp", "tunnelPort": 34197},
    "23": {"type": "custom", "protocol": "udp", "tunnelPort": 27015}
  }
}
```

Egg IDs above are examples, not universal IDs. Unmapped eggs are excluded.
Only each server's primary allocation is forwarded. Additional allocations,
query ports, RCON, and admin ports are not inferred. Games needing multiple
ports require further integration; the existing single-port UDP payload limit
and Bedrock Console Bridge limitations still apply. A wildcard allocation uses
the explicit `nodeAddress`; an invalid/missing primary allocation fails the
refresh and preserves the previous registrations. Protocol and tunnel-port
changes can require clients to reimport their invitation; ordinary restarts,
renames, and destination port changes preserve the existing identity.

Credential paths resolve relative to `host.json`. Remove `clientTokenFile` if
power status is not configured. Optionally set `relayRegion` to a public relay
region ID for the first registration. Further registrations reuse a stored
region. Never put these private files in Git or game containers.

Start the existing agent with its optional administration interface enabled:

```sh
gamelink-agent --dir /var/lib/gamelink-agent/config serve \
  --host-config /var/lib/gamelink-agent/host.json
```

For an existing systemd unit, add `--host-config` to its `ExecStart` when you
deploy. This does not require another daemon. The API binds only to
`127.0.0.1:8467`; `--host-listen` can select another literal loopback address.
It never discovers on startup. A CLI refresh is also available:

```sh
gamelink-agent --dir /var/lib/gamelink-agent/config host refresh \
  --config /var/lib/gamelink-agent/host.json
```

Run diagnostics before or after starting the agent:

```sh
gamelink-agent --dir /var/lib/gamelink-agent/config host check \
  --config /var/lib/gamelink-agent/host.json --json
```

Check reports all checks, exits 1 for failures, and exits 0 for passes and
warnings. It checks Panel access with GET requests and compares registrations
without changing them. For a running agent it verifies that the loopback admin
API rejects unauthenticated requests and accepts the private admin token.
Use `--listen` to check a different literal loopback address. No Client token
is a warning because power status will be unknown. `status`, `games list`,
`devices list` and `host refresh` also accept `--json` for setup automation.

## Connect the desktop

The first version deliberately binds administration to loopback. Use your
existing authenticated remote-access tooling to forward a local port, for
example with OpenSSH already configured for the node:

```sh
ssh -N -L 8467:127.0.0.1:8467 your-admin-user@your-wings-node
```

In GameLink, open **Host admin**, set the endpoint to `http://127.0.0.1:8467`, enter
the separate admin token, and save. The desktop protects the token with Windows
DPAPI. GameLink does not implement SSH or open a forwarding session itself.
HTTPS endpoints are accepted by the desktop for an independently configured
private TLS proxy; the agent's own listener remains loopback-only. No proxy or
public endpoint is deployed by this change.

Click **Refresh** to register servers and fetch current status. Click **Add to
GameLink** for a game to explicitly approve this desktop's device key and
privately import its invitation for your own use. To invite a friend, open
**Dashboard** or **Servers** and select **Copy invitation** on that game's
card. Copying and reviewing join requests require the updated desktop and
Linux agent. Your friend imports the invitation and clicks **Join** to request access.
In **People**, review the request for that game, confirm the device's public
key, and approve it, or dismiss it. Dismissal on the node needs the updated
agent. The desktop manages one connected node at a time. Change
the endpoint and token in **Host admin** to administer another node.

Suspended, missing, and newly unmapped servers are disabled on the next
successful Refresh, retaining their keys for later recovery. Failures preserve
the previous configuration and last good status timestamp. Changes made in
Pterodactyl between refreshes are intentionally not synchronized. To revoke
access immediately between refreshes, use the existing local device-revocation
commands or stop the agent. Rotating the host admin token requires restarting
the agent and updating the desktop connection.

The authenticated interface serves cached status and pending join requests at
`GET /v1/host/status` and explicit Refresh at `POST /v1/host/refresh`. Status
identifies each game's server key by a one-way fingerprint. The following POST
routes apply to enabled managed games and require the fingerprint of the game's
current identity.

- `/v2/host/invitation` copies an invitation and enables join requests without
  granting access.
- `/v2/host/requests/approve` approves a pending device.
- `/v2/host/requests/dismiss` dismisses a pending request.
- `/v2/host/import` approves the importing device and returns its invitation.

These actions require an updated desktop and agent. The agent rejects the
previous `/v1` routes for these actions. The admin token also authorizes
`POST /v2/host/update` on nodes with
[signed updates enabled](agent-setup.md#signed-agent-updates). Send an empty
request or `{"version":"<expected>"}`. The API returns HTTP 202 when the
request is queued, with `accepted: true` and an `attemptId` in the response.
If the expected version differs from the latest signed manifest, staging fails
before the binary download. Poll `GET /v1/host/status` for `agentVersion` and
match `update.attemptId` to the queued request before using the result.
`update.attemptId` is null when there is no recorded attempt ID. Requests still
require a bearer token and must omit the `Origin` header.

The interface exposes no shell, console commands, file transfer, arbitrary
destinations, game power controls, or Panel credentials.
