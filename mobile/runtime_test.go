package mobile

import (
	"encoding/json"
	"net"
	"net/netip"
	"strings"
	"testing"
	"time"

	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/invite"
	"github.com/tailscale/tailcat"
	"tailscale.com/tailcfg"
	"tailscale.com/types/key"
)

func mobileTestInvitation(t *testing.T, bridge int) string {
	t.Helper()
	ci := tailcat.ConnInfo{
		ServerPublic: tailcat.NodePublic{NodePublic: key.NewNode().Public()},
		PresharedKey: tailcat.NewPresharedKey(),
		RegionID:     tailcfg.DERPRegionID(1),
		Region:       []*tailcfg.DERPRegion{{RegionID: 1, Nodes: []*tailcfg.DERPNode{{HostName: "127.0.0.1", InsecureForTests: true, DERPPort: 3340}}}},
	}
	text, err := invite.Encode(invite.Invite{V: 1, Name: "test", Game: games.MinecraftBedrock, Proto: games.UDP, Port: 19132, Addr: string(ci.Addr()), ConsoleBridge: bridge}, invite.Options{AllowLocalRelay: true})
	if err != nil {
		t.Fatal(err)
	}
	return text
}

func loopbackInterfaceIndex(t *testing.T) int {
	t.Helper()
	i, err := net.InterfaceByName("lo")
	if err != nil {
		t.Skip("loopback interface is unavailable")
	}
	return i.Index
}

func TestStartRequiresPrivateLANAndExplicitLoopbackOptIn(t *testing.T) {
	for _, cfg := range []runtimeConfig{
		{LANIPv4: "127.0.0.2"},
		{LANIPv4: "8.8.8.8", AllowLocalRelay: true},
		{LANIPv4: "169.254.1.2", AllowLocalRelay: true},
	} {
		b, _ := json.Marshal(cfg)
		if _, err := Start(string(b), nil); err == nil {
			t.Fatalf("accepted invalid LAN selection %+v", cfg)
		}
	}
}

func TestStartUnsupportedBridgeAndCancellableReconnect(t *testing.T) {
	idx := loopbackInterfaceIndex(t)
	identity, err := GenerateIdentity()
	if err != nil {
		t.Fatal(err)
	}
	base := runtimeConfig{LANIPv4: "127.0.0.2", LANPrefix: 8, InterfaceIndex: idx, AllowLocalRelay: true}
	base.Invitation = mobileTestInvitation(t, 0)
	b, _ := json.Marshal(base)
	if _, err := Start(string(b), identity); err == nil || err.Error() != "unsupported_bridge" {
		t.Fatalf("unsupported invitation error = %v", err)
	}

	base.Invitation = mobileTestInvitation(t, 1)
	b, _ = json.Marshal(base)
	// Stop immediately after Start to cover a cancellation while the dial worker
	// may not have reached a stable state yet.
	pending, err := Start(string(b), identity)
	if err != nil {
		t.Fatal(err)
	}
	pending.Stop()
	var stopped runtimeStatus
	if err := json.Unmarshal([]byte(pending.Status()), &stopped); err != nil || stopped.State != "stopped" {
		t.Fatalf("pending stop status: %+v %v", stopped, err)
	}

	started := time.Now()
	r, err := Start(string(b), identity)
	if err != nil {
		t.Fatal(err)
	}
	if time.Since(started) > time.Second {
		t.Fatal("Start waited for network connection instead of returning promptly")
	}
	deadline := time.Now().Add(3 * time.Second)
	observedUnreachable := false
	for time.Now().Before(deadline) {
		var s runtimeStatus
		_ = json.Unmarshal([]byte(r.Status()), &s)
		if s.State == "unreachable" {
			observedUnreachable = true
			if strings.Contains(r.Status(), string(identity)) || strings.Contains(r.Status(), base.Invitation) || s.Path != "unknown" {
				t.Fatalf("runtime status leaked data or path: %+v", s)
			}
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	if !observedUnreachable {
		t.Fatalf("runtime did not report unreachable: %s", r.Status())
	}
	stopAt := time.Now()
	r.Stop()
	if time.Since(stopAt) > 5*time.Second {
		t.Fatal("stop took too long")
	}
	var got runtimeStatus
	if err := json.Unmarshal([]byte(r.Status()), &got); err != nil || got.State != "stopped" {
		t.Fatalf("stopped status: %+v %v", got, err)
	}
	r.Stop()
}

func TestPrivateLANValidation(t *testing.T) {
	for _, tc := range []struct {
		ip   string
		loop bool
		want bool
	}{{"192.168.1.4", false, true}, {"10.0.0.2", false, true}, {"127.0.0.2", false, false}, {"127.0.0.2", true, true}, {"8.8.8.8", true, false}, {"169.254.0.1", true, false}} {
		got := validMobileLAN(netip.MustParseAddr(tc.ip), tc.loop)
		if got != tc.want {
			t.Errorf("validMobileLAN(%s,%t) = %t, want %t", tc.ip, tc.loop, got, tc.want)
		}
	}
}
