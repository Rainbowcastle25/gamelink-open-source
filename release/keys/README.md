# Release verification keys

Before cutting the first release, commit these two public files:

- `manifest.pub` contains one bare Minisign public key per line (the second
  line of each `minisign -G` public file). Multiple lines allow a rotation.
- `tauri.pub` contains the full `.pub` output of `tauri signer generate`,
  which wraps a Minisign public key in base64.

See the [signed release manifest guide](../../docs/release-manifest.md) for
secret names, key generation and rotation. Keep private keys outside the
checkout. PR validation generates its own keys outside the checkout.
