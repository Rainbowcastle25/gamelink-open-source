# Signed release manifests

GameLink desktop and the Linux agent share a verifier for release notices. A
release check fetches only the JSON manifest and its detached Minisign signature.
Users follow the release notes link to get and install updates themselves.

## Publish a release

Produce the artifacts first, then compute the SHA-256 of each final asset.
Generate a UTF-8 JSON file named `release-manifest.json` with this shape.
The version, date and digest below are examples.

```json
{
  "schemaVersion": 1,
  "version": "0.3.0",
  "releaseDate": "2026-11-17",
  "severity": "security",
  "releaseNotesUrl": "https://github.com/Rainbowcastle25/gamelink-open-source/releases/tag/v0.3.0",
  "artifacts": [
    {
      "name": "gamelink-agent-linux-amd64",
      "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    }
  ]
}
```

- `version` is the release's SemVer, without a leading `v`. Set the agent and
  desktop package versions in their respective `Cargo.toml` files to match it.
  GameLink compares SemVer precedence, including prereleases and excluding build
  metadata.
- `releaseDate` is the UTC calendar date in `YYYY-MM-DD` format.
- `severity` is `normal` or `security`. Use `security` when the release
  includes fixes that users should prioritize.
- `releaseNotesUrl` is an HTTPS URL without embedded credentials. Point it at
  the notes for this specific release.
- `artifacts` lists the published binaries or archives and their SHA-256
  digests. Use unique asset basenames with ASCII letters, digits, periods,
  underscores or hyphens. Encode each digest as 64 hex digits.

Sign the exact bytes after generating the file. A release-signing script can
generate this JSON from the release metadata and the same artifact hashes used
for `SHA256SUMS`, then run the following commands. Use current Minisign's
default prehashed signature format.

```sh
minisign -S -s /path/to/offline-release.key -m release-manifest.json
minisign -Vm release-manifest.json -P "$release_public_key"
```

Publish `release-manifest.json` and `release-manifest.json.minisig` alongside
the release artifacts. Treat a version's signed metadata as immutable and assign
a newer version if the artifacts or release metadata change. The standard feed
is the GitHub Releases [latest manifest asset][manifest]. GitHub's `latest`
alias normally excludes prereleases. To check a prerelease or a mirror, set
`GAMELINK_UPDATE_MANIFEST_URL` in the environment before launching GameLink or
running `gamelink-agent update check`. The URL must use HTTPS and have no
credentials, query or fragment. Serve the signature at the same URL with
`.minisig` appended. The asset limits are 64 KiB for the manifest and 4 KiB for
the signature.

[manifest]: https://github.com/Rainbowcastle25/gamelink-open-source/releases/latest/download/release-manifest.json

## Embed and rotate the verification keys

Build both binaries with `GAMELINK_UPDATE_PUBLIC_KEYS` set to the base64
public key line from `minisign.pub` (the value accepted by `minisign -P`).
For example, with the public key value in `release_public_key`:

```sh
export GAMELINK_UPDATE_PUBLIC_KEYS="$release_public_key"
bash scripts/build-agent.sh amd64
bash scripts/build-desktop.sh x86_64-pc-windows-gnu
```

The build environment accepts whitespace-separated public keys for planned
rotation. The verifier accepts a signature under any embedded pin. Builds
with no pins make no release requests: the desktop shows **Check for updates**
as unavailable, and `gamelink-agent update check` exits nonzero. Keep the private signing key offline.
The feed URL override changes where the app fetches metadata. Trust still
comes from the keys in the binary.

For planned rotation, ship a transition release that embeds both public keys
and publish its manifest with the existing signing key. Give users time to
install that release before switching the feed to the new signing key.
Subsequent builds should drop the retired key. Users still running builds
that only trust the retired key need to upgrade manually after the switch.
If a signing key is compromised, use a trusted out-of-band channel to publish
the incident and distribute replacement builds with the compromised pin removed.

## Check behavior

Desktop **Settings → About → Check for updates** is off by default because the
release host can see the IP address of a request. Opting in permits a check
when GameLink is open, at most once in each 24-hour interval.
Failures count toward the interval. If the system clock moved back, so the
last attempt looks like it is in the future, the next check runs right away.
An unavailable or unverified result produces no banner and a local debug log
entry for failures. A verified newer release appears with a link to its notes,
with stronger styling for security releases. A failed check does not hide a
notice that was already verified. An unreadable or invalid local cache is
treated as empty, logged at debug level and replaced by the next successful
check. A manifest for the same version as the last verified one is rejected if
its signed bytes differ.

The Linux agent sends release requests only on `gamelink-agent update check`.
Add `--json` for an object containing `installedVersion`, `checkedAt`,
`available` and the verified `release` manifest. Failures exit nonzero and
write to stderr. `gamelink-agent status` reads the last verified result
locally. `status --json` includes it under `update` when available.

Checks use HTTPS GETs with `User-Agent: GameLink` and carry no cookies,
credentials, installed version, or device identifiers. HTTPS redirects are
allowed for release asset hosting. The release host and its CDN can see the
request's IP address and timing. No check fetches a listed artifact.
