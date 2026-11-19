# Signed release manifests

GameLink desktop and the Linux agent share a verifier for release notices. A
release check fetches only the JSON manifest and its detached Minisign signature.
Users follow the release notes link to get and install updates themselves.

## Release files

The [release workflow](../.github/workflows/release.yml) builds the binaries,
signs the release files and uploads them to a **draft** GitHub Release. See
[building and releasing](building.md#releasing) for the tag and publish steps.
It generates UTF-8 `release-manifest.json` with the following schema. The
version, date and digest below are examples.

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
- `releaseDate` is the UTC calendar date when the workflow stages the release,
  in `YYYY-MM-DD` format.
- `severity` is `normal` by default. Set the annotated tag trailer
  `GameLink-Release-Severity: security` for fixes users should prioritize.
- `releaseNotesUrl` is an HTTPS URL without embedded credentials. Point it at
  the notes for this specific release.
- `artifacts` lists the published binaries or archives and their SHA-256
  digests. Use unique asset basenames with ASCII letters, digits, periods,
  underscores or hyphens. Encode each digest as 64 hex digits.

The pipeline signs the exact manifest bytes and each raw agent binary using
Minisign's default prehashed signature format. Each gets a detached `.minisig`.
To verify a downloaded manifest or binary, use the second line of the
signer's Minisign public file:

```sh
minisign -Vm release-manifest.json -P "$release_public_key"
minisign -Vm gamelink-agent-linux-amd64 -P "$release_public_key"
```

Tauri signs the Windows x64 NSIS installer and Linux x86_64 AppImage, producing
detached `.sig` files. The pipeline copies those signatures into `latest.json`
with tag-specific download URLs. The [Tauri feed][tauri-feed] and [GameLink
manifest feed][manifest] both use GitHub's latest stable release alias.
`SHA256SUMS` covers all other uploaded files, including the two feeds,
signatures and the license archive.

Once published, keep the artifacts and signed metadata immutable. Assign a
newer version for any change. GitHub's latest alias excludes prereleases.
To check a prerelease for release notices or to use a mirror, set
`GAMELINK_UPDATE_MANIFEST_URL` in the environment before launching GameLink or
running `gamelink-agent update check`. The URL must use HTTPS and have no
credentials, query or fragment. Serve the signature at the same URL with
`.minisig` appended. The asset limits are 64 KiB for the manifest and 4 KiB for
the signature.

[manifest]: https://github.com/Rainbowcastle25/gamelink-open-source/releases/latest/download/release-manifest.json
[tauri-feed]: https://github.com/Rainbowcastle25/gamelink-open-source/releases/latest/download/latest.json

## Set up the signing keys

Use two key pairs. Tauri's signer wraps its Minisign key and signature
records in an extra layer of base64 and binds the release version in the
installer signature. Separate keys allow independent rotation and limit a
compromised key to one signing purpose.

Before the first release, generate the pairs in a private directory outside
the checkout. Minisign's `-W` creates a key for unattended signing. The Tauri
signer prompts for a password. Set a nonempty password and retain private
backups of both keys and the password.

```sh
keydir="$(mktemp -d)"
minisign -G -W -s "$keydir/manifest.key" -p "$keydir/manifest.pub"
npx --yes @tauri-apps/cli@2.12.4 signer generate --write-keys "$keydir/tauri.key"

sed -n '2p' "$keydir/manifest.pub" > release/keys/manifest.pub
cp "$keydir/tauri.key.pub" release/keys/tauri.pub
```

Commit the two public files. Add these GitHub Actions repository secrets with
the exact file contents or password:

| Secret | Value |
| --- | --- |
| `GAMELINK_RELEASE_SIGNING_KEY` | Full contents of `manifest.key` |
| `TAURI_SIGNING_PRIVATE_KEY` | Full contents of `tauri.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password set during Tauri key generation |

From an authenticated checkout, you can set the file secrets with
`gh secret set GAMELINK_RELEASE_SIGNING_KEY < "$keydir/manifest.key"` and
`gh secret set TAURI_SIGNING_PRIVATE_KEY < "$keydir/tauri.key"`. Use
`gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD` to enter the password
interactively. The release job fails if any secret or public file is missing.

The pipeline passes the checked-in Minisign public key lines as
`GAMELINK_UPDATE_PUBLIC_KEYS` to both the agent and desktop builds. It injects
the checked-in Tauri key as `plugins.updater.pubkey` at build and bundle time.
Plain local builds without Minisign pins have release checks disabled.
The notification feed URL override uses the same compiled-in pins.

## Rotate verification keys

For a planned Minisign rotation, add the next public key on a new line in
`release/keys/manifest.pub` and ship a transition release signed by the
existing key. Give users time to install it before changing the signing secret
and dropping the old public key from future builds. Clients with only the old
pin need a manual upgrade after the switch.

Tauri's updater config has one public key. The pipeline requires signatures
that verify against the key embedded in the new bundle. Once the updater is
in use, changing this key requires users of the old key to install a trusted
new bundle manually unless a separate migration is implemented. If either
signing key is compromised, announce the incident through a trusted channel
and distribute replacement builds with the compromised key removed.

## Check behavior

Desktop **Settings → About → Check for updates** is off by default because the
release host can see the IP address of a request. Opting in permits a check
when GameLink is open, at most once in each 24-hour interval.
Failures count toward the interval. If the system clock moved back, so the
last attempt looks like it is in the future, the next check runs right away.
An unavailable or unverified result produces no banner and a local debug log
entry for failures. A verified newer release appears with a link to its notes,
with stronger styling for security releases. A failed check does not hide a
notice that was already verified. A damaged local cache triggers an immediate
check, and its rollback protection is lost only if the saved manifest or
signature can no longer be read or verified. A manifest for the same version
as the last verified one is rejected if its signed bytes differ.

The Linux agent sends release requests only on `gamelink-agent update check`.
Add `--json` for an object containing `installedVersion`, `checkedAt`,
`available` and the verified `release` manifest. Failures exit nonzero and
write to stderr. `gamelink-agent status` reads the last verified result
locally. `status --json` includes it under `update` when available.

Checks use HTTPS GETs with `User-Agent: GameLink` and carry no cookies,
credentials, installed version, or device identifiers. HTTPS redirects are
allowed for release asset hosting. The release host and its CDN can see the
request's IP address and timing. No check fetches a listed artifact.
