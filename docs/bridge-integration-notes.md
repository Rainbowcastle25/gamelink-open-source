# Bridge integration boundary

The integration work described in this former planning note is implemented.
Kotlin owns the Android service and runs lifecycle effects: it calls Rust for
state reduction and calls the Go mobile API directly for transport. Rust is
not in the packet path. The Linux agent hosts the authenticated Bedrock
LAN-to-LAN bridge. This file remains as a compatibility pointer for existing
links.

See [Android Console Bridge](android-console-bridge.md) for host configuration,
build/install steps, current limits and the outstanding Xbox-first then
PlayStation hardware checklist. See [Rust/Go boundary](core-boundary.md) for
the mobile lifecycle API and secret-handling boundary. Do not treat the older
protocol research in `bedrock-research.md` as implementation status; it records
upstream observations and limitations separately.
