package consolebridge

import "net/netip"

// MapCandidates filters and rewrites candidates using Rust's pinned endpoint
// policy. Other SDP bytes stay untouched; empty initial SDP supports trickle ICE.
func MapCandidates(input string, allowedIP netip.Addr, first, last uint16, relay netip.AddrPort) (string, []netip.AddrPort, error) {
	return candidates("console.candidates_map", input, allowedIP, first, last, relay)
}
