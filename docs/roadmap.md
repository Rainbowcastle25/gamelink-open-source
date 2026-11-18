# Roadmap

The roadmap lists priorities. Checked implementation items track code delivery.
Game and console validation have their own checklists.

## First priority: PC real-game validation

Run the [real-game validation procedure](validation.md) for Minecraft Java,
Minecraft Bedrock on Windows, and Factorio. Record gameplay, revocation,
reconnect after a host restart, and local-port fallback on both direct and
forced DERP paths. Every matrix row is currently **not yet verified**.

## Near-term work in progress

- Knock-to-join requests
- Rotatable and expiring invitations
- Desktop hosting of LAN worlds with a People screen
- Stable local ports and Minecraft server-list integration
- Opt-in presence notifications
- Signed update notifications (notifications only, never auto-install)

## 0.2 alpha implementation

- [x] Tailcat TCP/UDP transport, direct path and DERP fallback
- [x] Per-game device allowlist, empty-list fail-closed behavior and revocation
- [x] Windows desktop client and Linux host agent
- [x] Android Go/Rust/Kotlin app with Keystore-backed, no-backup storage
- [x] Explicit LAN-to-LAN Bedrock Console Bridge with source/interface/subnet
  filters, candidate policy, bounded sessions and fragmented media datagrams
- [x] Android foreground-service start/stop, explicit consent, selected Wi-Fi
  binding and stop on selected-network change
- [x] CI and local Go, frontend and Android build/test paths

## Android Console Bridge freeze

The bridge is frozen for maintenance only. New bridge features are on hold
until the PC path has been validated with real games using the
[validation procedure](validation.md). The checks below track the evidence
needed to claim console support.

- [x] Android API 37 16 KiB emulator suite for Keystore storage, JNI reducer,
  Go identity/error redaction, DNS/TLS and Tailcat fixture discovery/cleanup
- [ ] Exercise the interactive API 37 `ACCESS_LOCAL_NETWORK` permission prompt;
  the emulator run pre-granted it through instrumentation
- [ ] Install the debug APK on physical Android hardware and verify LAN
  discovery, signaling, gameplay and clean Stop
- [ ] Validate with Xbox first, then PlayStation, using the same Bedrock server
- [ ] Repeat with two consoles connected through one approved phone
- [ ] Revoke the phone while sessions are active; confirm sessions end and do
  not reconnect
- [ ] Lose or change Wi-Fi and the selected interface; confirm the bridge stops
  and only restarts after another explicit user action
- [ ] Test screen-off operation, battery drain, sustained thermals and longer
  sessions on target phones
- [ ] Test both direct Tailcat path and forced/observed DERP relay on hardware
- [ ] Confirm BDS discovery on UDP 7551 and the configured media-port range

The hardware checks have not been run. There is no app-store release and no
public deployment. See [the test checklist](android-console-bridge.md#hardware-validation-checklist)
for the ordered procedure and recording template.

## Later

- Signed Windows installer and signed release checksums
- Prebuilt Linux agent binaries and `.deb`/systemd packaging
- Host-side activity history and per-device nicknames
- macOS and Linux desktop clients
- Self-hosted DERP relay recipe, reproducible builds and an SBOM per release
- Third-party security review of the Tailcat wrapper and console bridge
- iOS client research only after a separate local-network permission and
  background-networking design; no iOS bridge is currently planned for 0.2

## Explicitly not planned

Hosted accounts, public server discovery, mod installation, automatic updates,
general-purpose mobile VPN apps, or a public admin API. These add attack
surface or central trust that cut against the project's purpose.
