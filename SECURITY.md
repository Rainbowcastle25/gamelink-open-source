# Security policy

GameLink is an alpha. Please read [docs/security.md](docs/security.md) for the
threat model and known limitations before relying on it.

## Reporting a vulnerability

**Do not open a public issue** for a security problem, and never paste keys,
invitations (`glink1.…`), Tailcat addresses or `nodekey:`/private key material
into any issue, discussion or chat.

Report it privately using the repository's "Report a vulnerability" button
(GitHub private security advisories). Include:

- what you found and the impact you believe it has,
- steps or a test that reproduce it,
- the version and platform,
- whether you want to be credited.

We aim to acknowledge reports within 7 days. This is an unfunded alpha, so
please be patient, and please give us reasonable time to fix before disclosing.

## Scope

In scope: the agent, the desktop app, invitation parsing, access control
(allowlist, revocation), secret storage, log redaction, and the build
configuration that removes port mapping and SSH code.

Out of scope: vulnerabilities in the games themselves; vulnerabilities in
Tailscale/Tailcat/WireGuard (report those upstream; tell us too so we can
update); attacks that require an already-approved device to behave
maliciously against a game server it is allowed to play on; physical access.

## Supported versions

Only the latest alpha release is supported.
