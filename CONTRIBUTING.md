# Contributing

Thanks for helping. GameLink is small on purpose; please keep it that way.

## Ground rules

- **Security first.** Do not add a non-loopback listener except the explicit
  Bedrock Console Bridge LAN discovery/media path, which must remain pinned to
  the selected interface/subnet and configured BDS endpoint. Never open router
  ports, run remote commands, transfer files, add an admin API or telemetry, or
  set Tailcat forwarding hooks.
- **No secrets in the repository,** including test fixtures. Tests generate
  fresh keys. Never paste a real invitation or `nodekey` into an issue.
- **No fake data in the UI.** Demo mode is the only place sample data may
  appear, and it must stay clearly labelled.
- Do not claim a game "works" unless you ran the real game. State what you
  actually verified in the pull request.

## Getting set up

Start with [the agent environment guide](docs/agent-environment.md) for pinned
tools, a read-only doctor and the shared local/CI checks. Build the Rust/Go
archives before running Go checks; `bash scripts/check.sh fast` handles this.

```sh
npm --prefix apps/desktop/frontend ci
bash scripts/check.sh doctor
bash scripts/check.sh fast          # NO production tags: tests need the full feature set
```

Production builds use the tag set in `build-tags.txt`; see
[docs/building.md](docs/building.md). Run `go test -race ./shim/... ./tests/...`
before sending changes to the transport.

Optional: `go test -tags external -run TestPublicRelay -v ./tests/integration`
uses the real public relays.

## Pull requests

1. Keep changes focused; include tests (the integration tests in
   `tests/integration` spin up a local relay and need no internet).
2. If you touch access control, revocation, invitation parsing or redaction,
   add a test that fails without your change.
3. Update the docs that your change makes untrue.
4. If you add a dependency, regenerate `THIRD_PARTY_NOTICES.md` (command in that
   file) and check the license is compatible with GPL-3.0.
5. By contributing you agree your work is licensed under GPL-3.0.

## Reporting bugs

Use issues for bugs, with the output of **Diagnostics** (it is redacted). For
security problems, follow [SECURITY.md](SECURITY.md) instead.
