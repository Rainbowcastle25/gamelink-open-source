# Set up a GameLink server agent (runbook for AI agents)

You are setting up GameLink on a game server for its **owner**, the person you
are working for. When you finish, the agent runs as a service and the owner's
games are registered. Nobody can reach a game until the owner approves a
device for it. Follow the steps in order. Each step ends with a **done when**
check; run that check before you move on.

GameLink exists to be **safer than port forwarding**. A game is reachable only
by devices the owner approved, for that game only, through an encrypted
Tailcat tunnel. Every rule below protects that property.

## Rules

1. **Secrets stay private.** Secrets are Panel API keys, the host admin
   token, invitations (`glink1.…`), and everything under the agent's
   configuration directory. Move them only through the owner's private input
   (a secret prompt, or the owner typing on the server), or as files with mode
   0600. Show the owner file *paths*, never secret values, including in
   command output, logs and summaries.
2. **The owner approves access.** Approve a device only after the owner
   explicitly confirms that device and that game. Approval is access.
3. **Use a read-only Panel key.** The Application API key needs Read on
   Servers, Allocations and Eggs, and None for everything else. If the owner
   offers an admin or writable key, ask for a read-only one instead.
4. **Stay inside the agent's footprint**, which is all you install:
   the `gamelink-agent` binary and systemd unit, the `gamelink` system user,
   and `/var/lib/gamelink-agent/`. Apt installs also add a GameLink source and
   keyring. Manual installs can add the signed-update path and service units.
   GameLink needs no changes to firewalls, router ports, Docker, Wings, the
   Panel or game containers. Ask the owner before touching anything outside
   the footprint, such as a VPN policy or a hypervisor.
5. **Administration stays on loopback.** The host admin API listens only on
   `127.0.0.1:8467`. The owner reaches it through their own SSH tunnel.

## Commands you will use

Run agent commands as the `gamelink` user:

```sh
agent_binary=/usr/bin/gamelink-agent  # Use /usr/local/bin/gamelink-agent for a manual install.
gl() { sudo -u gamelink env GAMELINK_AGENT_DIR=/var/lib/gamelink-agent/config "$agent_binary" "$@"; }
```

Add `--json` to `status`, `games list`, `devices list`, `host refresh`,
`host detect` and `host check` for machine-readable output. Run
`gamelink-agent <command> -h` for flags.

## Step 1: Learn the setup

Ask the owner for:

- which machine runs the games (one agent per machine; a Pterodactyl Panel
  with several Wings nodes needs one agent per node), and how you reach it
  (SSH user and host);
- whether it is a **plain** Linux game server or a **Pterodactyl** Wings node;
- for Pterodactyl, the Panel URL.

On the server, record `uname -m`, the OS, and whether `sudo -n true`
succeeds.

**Done when** you can run commands with sudo on the target and you know the
branch (plain or Pterodactyl).

Reaching the server:

- **No route to the machine**, for example a node on an isolated VLAN: ask
  the owner to choose. They can add Tailscale to the machine with a narrow
  policy grant (this machine → the node, `tcp:22` only), or allow you to run
  commands through the hypervisor (Proxmox: `qm guest exec <vmid>` on the
  host; copy files in gzip chunks with `--pass-stdin 1`).
- **Default-deny tailnet**: a new node is unreachable until the policy has a
  grant for it. `tailscale ping` succeeding while TCP times out means the
  grant is missing.
- **Panel on local DNS only**: the owner must create API keys from a device
  on that network.

## Install with apt (recommended)

Use this path for a new Debian or Ubuntu systemd installation on amd64 or
arm64 once the stable apt repository is live. Use the manual steps below for
other distributions or while the apt repository is unavailable.

With `curl` and `gpg` installed, verify the downloaded key against the
fingerprint published here and in [release/keys/apt.fingerprint](../release/keys/apt.fingerprint)
on GitHub. Then install the key and restrict this source to its keyring.

```sh
(
  set -eu
  expected_fingerprint=92C55F555AEDE9E5B9B5C3D1E8BB6D20616935C4
  apt_key="$(mktemp)"
  trap 'unlink "$apt_key"' EXIT
  curl -fsSL https://rainbowcastle25.github.io/gamelink-open-source/apt/apt.asc -o "$apt_key"
  key_records="$(gpg --batch --with-colons --show-keys "$apt_key")"
  if ! printf '%s\n' "$key_records" | awk -F: -v expected="$expected_fingerprint" '
    $1 == "pub" { keys++ }
    $1 == "fpr" && !fingerprint { fingerprint = $10 }
    END { exit !(keys == 1 && length(fingerprint) == 40 && fingerprint == expected) }
  '; then
    echo "GameLink apt signing key fingerprint mismatch. Check the fingerprint on GitHub." >&2
    exit 1
  fi
  sudo gpg --batch --yes --dearmor -o /usr/share/keyrings/gamelink.gpg "$apt_key"
  printf '%s\n' 'deb [signed-by=/usr/share/keyrings/gamelink.gpg] https://rainbowcastle25.github.io/gamelink-open-source/apt stable main' \
    | sudo tee /etc/apt/sources.list.d/gamelink.list
  sudo apt update
)
```

For a new setup, install and initialize the agent. For an existing manual
installation, follow [the migration steps](#migrate-a-manual-install-to-apt).

```sh
sudo apt install gamelink-agent
gl init
```

The package provides `/usr/bin/gamelink-agent`, the system account and state
directory, and the systemd units. Configure the agent in step 4 before
enabling the main service.

During an apt upgrade, the package stops a running service before replacing
the binary. It starts the service again only if it was running and enabled
when the upgrade began, its enablement is still the same, and local service
policy permits starting it.

**Done when** `gl status --json` reports `"running": false` and no games.
Continue to step 4a or 4b.

### Enable desktop updates with apt

Desktop-triggered upgrades are off by default. You can update explicitly
with `sudo apt update && sudo apt upgrade` at any time. To opt in to **Update all**,
first use apt to install a GameLink package that includes
`gamelink-agent-apt-update.path`. The GameLink service must be running and
enabled, and the apt source and keyring must be configured as above.

Run these commands once on the server:

```sh
apt_state=/var/lib/gamelink-agent-apt-update
sudo mkdir -m 0750 "$apt_state" &&
  sudo chgrp gamelink "$apt_state" &&
  sudo install -d -o gamelink -g gamelink -m 0700 "$apt_state/staged" &&
  sudo install -m 0644 /dev/null "$apt_state/swap.lock" &&
  sudo install -m 0755 /usr/bin/gamelink-agent "$apt_state/installer" &&
  sudo systemctl enable --now gamelink-agent-apt-update.path
```

If the directory already exists, inspect it before continuing. The root-owned
installer copy can restore the prior package if the new binary cannot run.
To stop accepting new requests, run
`sudo systemctl disable --now gamelink-agent-apt-update.path`.
If an upgrade is already running, let it finish before removing the package.

Click **Update all** on a linked desktop or run
`gl update stage --version <expected>` on the server to request an upgrade.
The exact release must be published on both GitHub and the apt source, and the
source must still contain your installed version for rollback. The helper
installs the package, watches the service for ten seconds, and restores the
prior package if the upgrade fails. Game sessions disconnect during a restart.
The desktop installs its update after the agent reports success for that
request. The path unit only reacts to a staged request.

### Migrate a manual install to apt

Arrange downtime with the owner. Save any custom service options from
`sudo systemctl cat gamelink-agent`. If the signed self-update helper is
installed, run `sudo systemctl disable --now gamelink-agent-update.path`.
Wait for any running `gamelink-agent-update.service` job to finish. Use
`sudo systemctl list-jobs` to check. Let any agent CLI commands finish, then
run `sudo systemctl disable --now gamelink-agent`.

Unlink the manual files while keeping `/var/lib/gamelink-agent` and the
`gamelink` account intact:

```sh
(
  set -eu
  for path in /etc/systemd/system/gamelink-agent.service \
    /etc/systemd/system/gamelink-agent-update.path /etc/systemd/system/gamelink-agent-update.service \
    /usr/local/bin/gamelink-agent /usr/local/bin/.gamelink-agent-update-helper; do
    if [ -e "$path" ] || [ -L "$path" ]; then
      sudo unlink "$path"
    fi
  done
  sudo systemctl daemon-reload
  sudo apt install gamelink-agent
)
```

The package reuses the existing configuration and state. Apply any saved
service options as a systemd override with `sudo systemctl edit gamelink-agent`,
using `/usr/bin/gamelink-agent` in any custom `ExecStart`. Enable and start
the packaged service with `sudo systemctl enable --now gamelink-agent`.
Set `agent_binary=/usr/bin/gamelink-agent` and verify that `gl status --json`
shows the service running and the existing games.

## Step 2: Get the agent binary for a manual install

Map the architecture: `x86_64` → `amd64`, `aarch64` → `arm64`. Use the first
source that works:

1. A GitHub release of this repository with `SHA256SUMS`.
2. The `gamelink-agent-linux` artifact of the latest green CI run on
   `master`:
   `gh run download <run-id> -n gamelink-agent-linux -D <empty dir>`.
   Artifacts expire after one day.
3. Build it: `bash scripts/build-agent.sh amd64`. See
   [building.md](building.md#server-agent-linux) for the toolchain.

Copy `gamelink-agent-linux-<arch>` to the server (for example with `scp`).

**Done when** `sha256sum` of the file on the server matches its line in
`SHA256SUMS`, or the hash of your own build.

## Step 3: Install the manual binary

```sh
sudo install -m 0755 gamelink-agent-linux-<arch> /usr/local/bin/gamelink-agent
sudo useradd --system --create-home --home-dir /var/lib/gamelink-agent --shell /usr/sbin/nologin gamelink
sudo chmod 0700 /var/lib/gamelink-agent
gl init
```

**Done when** `gl status --json` reports `"running": false` and no games.

## Step 4a: Plain Linux game server

For each game the owner wants to share:

```sh
gl games add --type minecraft-java --name "Our Minecraft"
```

Types are `minecraft-java`, `minecraft-bedrock`, `factorio` and `custom`;
`custom` also needs `--proto tcp|udp --port N`. If the game is not listening
on this machine's loopback (Docker, another host), add `--dest ip:port`.
Each game forwards one port.

Apt provides the service unit. For a manual install, copy
[`config/examples/gamelink-agent.service`](../config/examples/gamelink-agent.service)
to `/etc/systemd/system/` and preserve its hardening (`AF_NETLINK` must stay in
`RestrictAddressFamilies`). Run
`sudo systemctl daemon-reload && sudo systemctl enable --now gamelink-agent`.

**Done when** `gl status --json` reports `"running": true` and lists every
game the owner asked for. Go to step 5.

## Step 4b: Pterodactyl Wings node

1. **Panel key.** Ask the owner to create an Application API key in
   *Panel → Admin → Application API*, with Read on Servers, Allocations and
   Eggs and None for everything else. Write it to
   `/var/lib/gamelink-agent/panel-application.token`, owned by `gamelink`
   with mode 0600, without echoing it. Either the owner pastes it on the
   server:
   `sudo -u gamelink sh -c 'umask 077; cat > /var/lib/gamelink-agent/panel-application.token'`,
   or you pipe it from your private secret input over SSH stdin into that
   same command.
2. **Detect.** Run
   `gl host detect --panel-url <url> --token-file /var/lib/gamelink-agent/panel-application.token --json`.
   - Exit code 3 means the node is ambiguous. Show the owner the nodes and
     rerun with `--node-id N`.
   - Show the owner the proposed mapping: each egg, its servers, the proposed
     type and port, and `confidence` (`name`, `startup`, `guess` or `none`).
     Ask about every egg that is not `name`. `--out` leaves out `guess` and
     `none`, so add those yourself if the owner confirms them. An egg
     excluded as "not a joinable game server" stays out. Games that need several ports (ARK, for example)
     cannot be fully forwarded; only the primary port is.
   - Egg names showing as null mean the key lacks Eggs read. Detection then
     guessed from startup commands. Ask the owner to add Eggs read, or
     confirm each egg.
3. **Write the settings.** Rerun the detect command with
   `--out /var/lib/gamelink-agent/host.json` (plus `--node-id` if you used
   it). This writes `host.json` (0600) and creates `host-admin.token` next
   to it unless one exists. It never overwrites `host.json`. To
   change a mapping the owner decided, edit `eggProfiles` in `host.json` and
   keep it 0600 and owned by `gamelink`.
4. **Optional power status.** A Client API key for a user who can see this
   node's servers enables running/offline status. Store it like the
   application key as `panel-client.token`, and add
   `"clientTokenFile": "panel-client.token"` to `host.json`. Without it,
   status shows `unknown`; tunnels work regardless.
5. **Service.** For apt installations, run
   `sudo systemctl edit gamelink-agent` and enter the override below. On a
   manual install, copy the unit from step 4a and set its existing
   `ExecStart` to the same command with `/usr/local/bin/gamelink-agent`.
   Run `sudo systemctl daemon-reload && sudo systemctl enable --now gamelink-agent`.

   ```ini
   [Service]
   ExecStart=
   ExecStart=/usr/bin/gamelink-agent serve --host-config /var/lib/gamelink-agent/host.json
   ```
6. **Register.** Run
   `gl host refresh --config /var/lib/gamelink-agent/host.json --json`.
   Refresh is manual by design: rerun it whenever the owner adds a server.
7. **Check.** Run
   `gl host check --config /var/lib/gamelink-agent/host.json --json`.

**Done when** `host check` reports `"ok": true`, every remaining `warn` is
either resolved or explained to the owner, and every server the owner wanted
appears in `gl status --json`.

## Step 5: Share access

Every game starts with no approved devices, so nothing is reachable yet.
Offer the owner both routes:

- **From the command line, through you.** Run `gl invite --game <id>`. Write the
  invitation to a 0600 file and give the owner the path, or let them run the
  command themselves. The owner shares it privately. With the agent running,
  the friend imports the invite and clicks **Join**. Run
  `gl requests list --game <id>` after the attempt. With the owner's
  confirmation, run `gl requests approve <request-id> --label "<who>"`. If the
  first attempt timed out, ask the friend to click **Join** again. If the owner cannot identify a request,
  compare its key with the friend's **Settings → Copy Public Key**. The owner
  can use `gl requests dismiss <request-id>` to clear an unwanted request.
- **From the GameLink desktop Host screen** (Pterodactyl only). The owner
  opens a tunnel, `ssh -N -L 8467:127.0.0.1:8467 <user>@<node>`, sets the
  endpoint to `http://127.0.0.1:8467`, and enters the admin token. Give the
  owner a command that copies the token straight to their clipboard, for
  example on Windows:
  `ssh <user>@<node> "sudo cat /var/lib/gamelink-agent/host-admin.token" | Set-Clipboard`.
  *Add to GameLink* then approves the owner's own device. See
  [pterodactyl.md](pterodactyl.md).

**Done when** the owner knows how to approve devices and send invitations,
and has approved any devices they asked for.

Use `gl invite rotate --game <id>` to replace a shared invitation. The agent
ends sessions on the old address when it reloads, and approved devices keep
their approval when they import the new invitation. Use
`gl invite --game <id> --expires 24h` to rotate to a time-limited identity.
At its deadline the agent ends that game's sessions and stops accepting joins
until the owner rotates again.

## Step 6: Report

Give the owner:

- the machine, agent version and binary SHA-256;
- each registered game (name, id, protocol/port, destination);
- eggs or games left out, and why;
- remaining `host check` warnings;
- where the secrets live (paths only);
- how to share access (step 5), and that a new Pterodactyl server needs
  `host refresh`.

## Updating and removing

- **Apt upgrade**. Run `sudo apt update && sudo apt upgrade`. Apt stops a
  running GameLink service before replacing the binary and starts it after
  configuration. If the repository goes 60 days without re-signing,
  `apt update` reports expired metadata until the publisher re-signs it.
- **Remove an apt install**. If you enabled desktop updates, disable the
  watcher first and let any running upgrade finish. Run
  `sudo systemctl disable --now gamelink-agent`, then
  `sudo apt remove gamelink-agent` or `sudo apt purge gamelink-agent`.
  Both commands keep `/var/lib/gamelink-agent` for a later reinstall.
- **Manual replacement:** verify the new binary (step 2). If the update path
  unit is enabled, stop it and wait for any running update to finish. Run
  `sudo systemctl stop gamelink-agent`, install the binary over
  `/usr/local/bin/gamelink-agent`. If signed updates are configured, relink the
  helper with
  `sudo ln -f /usr/local/bin/gamelink-agent /usr/local/bin/.gamelink-agent-update-helper`.
  Then run `sudo systemctl start gamelink-agent`.
  Start the path unit again if you stopped it.
  Let any running agent CLI commands finish before the swap and wait until the
  service is back up before running another.
  Approvals, invitations and game identities survive.
- **Remove a manual install:** if installed, disable `gamelink-agent-update.path` and let any
  running update finish. Then run `sudo systemctl disable --now gamelink-agent`
  and remove the units, binary and `/usr/local/bin/.gamelink-agent-update-helper`.

Deleting `/var/lib/gamelink-agent` destroys every game identity and
invitation. After removing the agent, ask the owner to approve deletion, then
run `sudo find /var/lib/gamelink-agent -xdev -depth -delete`.

### Signed agent updates

For manual installations, first install a release build with embedded
Minisign pins that supports `update stage`. Use `/usr/local/bin/gamelink-agent`
and the system account and unit name from this guide. Wait for other agent
CLI commands to finish, then run this one-time setup on the server.

```sh
sudo chown -h root:gamelink /var/lib/gamelink-agent &&
  sudo test ! -L /var/lib/gamelink-agent &&
  sudo test -d /var/lib/gamelink-agent &&
  sudo chmod 1770 /var/lib/gamelink-agent &&
  test "$(sudo stat -c '%u:%G:%a' /var/lib/gamelink-agent)" = '0:gamelink:1770' &&
  sudo mkdir -m 0750 /var/lib/gamelink-agent/update &&
  sudo chgrp gamelink /var/lib/gamelink-agent/update &&
  sudo install -d -o gamelink -g gamelink -m 0700 /var/lib/gamelink-agent/update/staged &&
  sudo install -m 0644 /dev/null /var/lib/gamelink-agent/update/swap.lock
```

The command chain stops if `mkdir` finds an existing update directory.
Inspect its ownership and permissions before continuing. Taking ownership
before setting the sticky bit protects the root-owned update directory from
replacement while letting `gamelink` manage its existing state files. Use
local filesystems for the state and binary.

Install the two helper files from
[`packaging/systemd/`](../packaging/systemd/) in the same trusted release's
source tree.

```sh
helper=/usr/local/bin/.gamelink-agent-update-helper
sudo install -m 0644 packaging/systemd/gamelink-agent-update.path /etc/systemd/system/ &&
  sudo install -m 0644 packaging/systemd/gamelink-agent-update.service /etc/systemd/system/ &&
  { sudo test -e "$helper" || sudo ln /usr/local/bin/gamelink-agent "$helper"; } &&
  sudo test ! -L "$helper" &&
  test "$(sudo stat -c '%u:%a' "$helper")" = '0:755' &&
  sudo "$helper" --version &&
  sudo systemctl daemon-reload &&
  sudo systemctl enable --now gamelink-agent-update.path
```

The chain only enables the watcher once the helper exists, is root-owned and
runs. If it stops, fix the helper before enabling the path unit. Otherwise
staged requests would wait for a helper that can't start. If you later replace
the agent by hand, the helper keeps the previous trusted version until the next
signed update advances it.

Stage the latest signed release with `gl update stage`. Add
`--version <expected>` to require an exact match with the latest signed
manifest. The path unit starts the root helper as soon as staging finishes.
For a manual handoff, leave the path unit disabled, run `gl update stage`,
then run `sudo /usr/local/bin/.gamelink-agent-update-helper update install-staged`.
`sudo systemctl start gamelink-agent-update.service` runs the same command in
the unit's sandbox. Enable the path unit again if you want automatic handoff
on later requests.

With host administration linked to the desktop app, **Update all** asks the
server to stage the same signed release offered to the PC. Before installing
the desktop update, the app waits for the agent to restart, pass its health
check and report success for that request with the expected version running.
Agents that cannot report which request completed need a manual update.
Choose **Update this app only** to skip the server. Release checks only display
the available update.

The root-owned helper is a separate hard link to the trusted binary. Its
path stays executable if the candidate fails to launch. The helper advances
the link only after the replacement passes its health check and commits.
It stops the main service, keeps the previous binary as
`/usr/local/bin/.gamelink-agent.previous`, and atomically installs the verified
binary. It checks the version and watches the service for 10 seconds. A bad
start or health check triggers rollback and a restart of the previous agent.
Game sessions disconnect during the restart. The helper waits up to five
seconds for existing CLI commands before it gives up on the swap. A root-owned
transaction record lets the helper recover or finish cleanup after interruption.
Agent update attempts have a five-minute cooldown, including after failures
and service restarts. The host update endpoint returns HTTP 429 during cooldown.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| Panel answers 403 | The key lacks a Read permission listed in step 4b. |
| Egg names are null | The key lacks Eggs read; see step 4b. |
| "requires owner-only permissions (0600)" | `chmod 600` the file and `chown gamelink:gamelink` it. |
| "wildcard allocation requires a configured node address" | Set `nodeAddress` in `host.json` to the node's IP. |
| `host check` admin_api warns "not listening" | `ExecStart` lacks `--host-config`. |
| The service starts but the game never comes up | `AF_NETLINK` was removed from `RestrictAddressFamilies`. |
| A friend's GameLink shows "no response" | Check that the agent is running, the invitation is current, and the friend's request appears in `gl requests list --game <id>`. |

### Signed update fails or stays staged

- Read `updateState` in `gl status --json` once the agent is back, or check
  `GET /v1/host/status` for `agentVersion` and `update`. A failed update
  includes a reason. The admin API is briefly unavailable while the agent
  restarts.
- Check `sudo journalctl -u gamelink-agent-update.service -u gamelink-agent -n 100`
  for verification, startup and rollback errors. On a rejected download,
  check outbound HTTPS access, the version and the installed release pins.
- If the state stays `staged`, check
  `sudo systemctl status gamelink-agent-update.path gamelink-agent-update.service`.
  If recovery fails, fix the reported service or filesystem problem, then
  run the root helper manually to finish the recorded transaction.
  Use the manual stop-replace-start procedure above if the helper itself
  cannot run.
