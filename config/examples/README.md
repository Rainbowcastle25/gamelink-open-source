# Example configuration

These files only show the *shape* of GameLink's configuration. They contain
placeholders, never real keys. Do not copy them into a live config directory:
use the CLI instead (`gamelink-agent init`, `games add`, `devices approve`),
which generates the keys and validates everything.

- `agent.example.json`: what `agent.json` looks like (no secrets live in it; the
  game's private key and pre-shared key are in `keys/<id>.json`, mode 0600,
  which you should never share, back up insecurely, or commit).
- `gamelink-agent.service`: a hardened systemd unit for running `serve`.
