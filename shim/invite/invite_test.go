package invite

import (
	"encoding/base64"
	"encoding/json"
	"strings"
	"testing"

	"github.com/tailscale/tailcat"
	"tailscale.com/tailcfg"

	"github.com/gamelink-project/gamelink/shim/games"
)

func testAddr(t *testing.T, mutate func(*tailcfg.DERPNode)) string {
	t.Helper()
	pk := tailcat.NewPrivateKey()
	node := &tailcfg.DERPNode{Name: "900a", RegionID: 900, HostName: "derp.example.com"}
	if mutate != nil {
		mutate(node)
	}
	ci := tailcat.ConnInfo{
		ServerPublic:      pk.Public.ServerPublic,
		ServerDiscoPublic: tailcat.DiscoPublicForNode(pk.Private),
		PresharedKey:      pk.Public.PresharedKey,
		Region:            []*tailcfg.DERPRegion{{RegionID: 900, RegionCode: "custom", RegionName: "Custom", Nodes: []*tailcfg.DERPNode{node}}},
	}
	return string(ci.Addr())
}

func goodInvite(t *testing.T) Invite {
	return Invite{Name: "Survival World", Game: games.MinecraftJava, Proto: games.TCP, Port: 25565, Addr: testAddr(t, nil), Note: "hi"}
}

func TestEncodeParseRoundTrip(t *testing.T) {
	in := goodInvite(t)
	s, err := Encode(in, Options{})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(s, Prefix) {
		t.Fatalf("prefix: %s", s[:10])
	}
	// Pasted invitations may be wrapped by chat apps.
	wrapped := s[:30] + "\n" + s[30:60] + "  \r\n" + s[60:]
	out, det, err := Parse(wrapped, Options{})
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	in.V = 1
	if out != in {
		t.Fatalf("round trip changed the invite:\n got %+v\nwant %+v", out, in)
	}
	if len(det.RelayHosts) != 1 || det.RelayHosts[0] != "derp.example.com" || det.DefaultRelay {
		t.Fatalf("details: %+v", det)
	}
}

func rawInvite(t *testing.T, v any) string {
	t.Helper()
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return Prefix + base64.RawURLEncoding.EncodeToString(b)
}

func TestParseRejectsMalformed(t *testing.T) {
	good := goodInvite(t)
	good.V = 1
	asMap := func(mut func(map[string]any)) string {
		b, _ := json.Marshal(good)
		var m map[string]any
		json.Unmarshal(b, &m)
		mut(m)
		return rawInvite(t, m)
	}
	cases := map[string]string{
		"empty":              "",
		"whitespace":         "   \n ",
		"wrong prefix":       "glink2." + strings.TrimPrefix(rawInvite(t, good), Prefix),
		"not base64":         Prefix + "!!!not-base64!!!",
		"not json":           Prefix + base64.RawURLEncoding.EncodeToString([]byte("hello")),
		"json array":         Prefix + base64.RawURLEncoding.EncodeToString([]byte("[1,2]")),
		"unknown field":      asMap(func(m map[string]any) { m["dest"] = "192.168.1.1:22" }),
		"version 2":          asMap(func(m map[string]any) { m["v"] = 2 }),
		"version 0":          asMap(func(m map[string]any) { m["v"] = 0 }),
		"empty name":         asMap(func(m map[string]any) { m["name"] = "  " }),
		"huge name":          asMap(func(m map[string]any) { m["name"] = strings.Repeat("a", 65) }),
		"control chars":      asMap(func(m map[string]any) { m["name"] = "bad\x00name" }),
		"bidi override":      asMap(func(m map[string]any) { m["name"] = "evil‮gnp.exe" }),
		"unknown game":       asMap(func(m map[string]any) { m["game"] = "quake3" }),
		"proto mismatch":     asMap(func(m map[string]any) { m["proto"] = "udp" }),
		"bad proto":          asMap(func(m map[string]any) { m["proto"] = "icmp" }),
		"port zero":          asMap(func(m map[string]any) { m["port"] = 0 }),
		"port too big":       asMap(func(m map[string]any) { m["port"] = 70000 }),
		"negative port":      asMap(func(m map[string]any) { m["port"] = -1 }),
		"empty addr":         asMap(func(m map[string]any) { m["addr"] = "" }),
		"garbage addr":       asMap(func(m map[string]any) { m["addr"] = "tcnotreallyanaddress" }),
		"addr with space":    asMap(func(m map[string]any) { m["addr"] = good.Addr + " extra" }),
		"addr wrong prefix":  asMap(func(m map[string]any) { m["addr"] = "xx" + good.Addr[2:] }),
		"addr is a hostname": asMap(func(m map[string]any) { m["addr"] = "example.com" }),
		"addr is a url":      asMap(func(m map[string]any) { m["addr"] = "http://169.254.169.254/latest" }),
		"too long":           Prefix + strings.Repeat("A", maxInviteLen+1),
		"trailing data":      Prefix + base64.RawURLEncoding.EncodeToString([]byte(`{"v":1}{"v":1}`)),
	}
	for name, in := range cases {
		if _, _, err := Parse(in, Options{}); err == nil {
			t.Errorf("%s: invitation was accepted", name)
		}
	}
}

func TestAddrWithoutPresharedKeyOrRegionRejected(t *testing.T) {
	pk := tailcat.NewPrivateKey()
	noPSK := tailcat.ConnInfo{
		ServerPublic: pk.Public.ServerPublic, ServerDiscoPublic: tailcat.DiscoPublicForNode(pk.Private),
		Region: []*tailcfg.DERPRegion{{RegionID: 900, Nodes: []*tailcfg.DERPNode{{Name: "a", RegionID: 900, HostName: "derp.example.com"}}}},
	}
	if _, err := ValidateAddr(string(noPSK.Addr()), Options{}); err == nil {
		t.Error("address without a pre-shared key accepted")
	}
	noRegion := tailcat.ConnInfo{
		ServerPublic: pk.Public.ServerPublic, ServerDiscoPublic: tailcat.DiscoPublicForNode(pk.Private),
		PresharedKey: pk.Public.PresharedKey, RegionID: 302,
	}
	if _, err := ValidateAddr(string(noRegion.Addr()), Options{}); err == nil {
		t.Error("address that needs a DERP map download accepted")
	}
}

// A hostile invitation must not be able to point the client's relay
// connection at private networks or turn off TLS verification.
func TestHostileRelayDetailsRejected(t *testing.T) {
	bad := map[string]func(*tailcfg.DERPNode){
		"loopback ip":      func(n *tailcfg.DERPNode) { n.IPv4 = "127.0.0.1" },
		"private ip":       func(n *tailcfg.DERPNode) { n.IPv4 = "192.168.1.10" },
		"metadata ip":      func(n *tailcfg.DERPNode) { n.IPv4 = "169.254.169.254" },
		"cgnat ip":         func(n *tailcfg.DERPNode) { n.IPv4 = "100.64.0.1" },
		"private v6":       func(n *tailcfg.DERPNode) { n.IPv6 = "fd00::1" },
		"insecure tls":     func(n *tailcfg.DERPNode) { n.InsecureForTests = true },
		"odd port":         func(n *tailcfg.DERPNode) { n.DERPPort = 22 },
		"ip hostname":      func(n *tailcfg.DERPNode) { n.HostName = "10.0.0.5" },
		"loopback host ip": func(n *tailcfg.DERPNode) { n.HostName = "127.0.0.1" },
		"bad hostname":     func(n *tailcfg.DERPNode) { n.HostName = "evil host/../x" },
		"bad certname":     func(n *tailcfg.DERPNode) { n.CertName = "not a name!" },
	}
	for name, mut := range bad {
		if _, err := ValidateAddr(testAddr(t, mut), Options{}); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
	// The same details are permitted only with the explicit local-relay option.
	local := testAddr(t, func(n *tailcfg.DERPNode) {
		n.HostName = "localhost"
		n.IPv4 = "127.0.0.1"
		n.InsecureForTests = true
		n.DERPPort = 8443
	})
	if _, err := ValidateAddr(local, Options{AllowLocalRelay: true}); err != nil {
		t.Errorf("AllowLocalRelay should accept a local relay: %v", err)
	}
	// A good public relay with a literal public IP override is fine.
	if _, err := ValidateAddr(testAddr(t, func(n *tailcfg.DERPNode) { n.IPv4 = "203.0.113.7" }), Options{}); err != nil {
		t.Errorf("public IP override rejected: %v", err)
	}
}

func TestEncodeValidates(t *testing.T) {
	in := goodInvite(t)
	in.Port = 0
	if _, err := Encode(in, Options{}); err == nil {
		t.Fatal("Encode accepted an invalid invite")
	}
}
