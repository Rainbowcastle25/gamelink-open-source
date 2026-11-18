package transport

import (
	"net"
	"net/netip"
	"testing"

	"tailscale.com/types/key"
)

func TestPeerActivityTracksOnlyApprovedSessions(t *testing.T) {
	activity := NewPeerActivity()
	peer := key.NewNode().Public()
	approved := netip.MustParseAddr("100.64.0.1")
	allowed := map[netip.Addr]key.NodePublic{approved: peer}
	activity.recordSession(allowed, net.TCPAddrFromAddrPort(netip.MustParseAddrPort("100.64.0.2:1234")))
	if len(activity.Snapshot()) != 0 {
		t.Fatal("recorded an unapproved session")
	}
	activity.recordSession(allowed, net.TCPAddrFromAddrPort(netip.AddrPortFrom(approved, 1234)))
	snapshot := activity.Snapshot()
	if len(snapshot) != 1 || snapshot[peer.String()].IsZero() {
		t.Fatal("missing last authorized session")
	}
	delete(snapshot, peer.String())
	if len(activity.Snapshot()) != 1 {
		t.Fatal("mutating the snapshot changed live activity")
	}
}
