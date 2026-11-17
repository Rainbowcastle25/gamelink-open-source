//go:build ignore

// Run BEFORE migrating the Go implementation. All inputs are synthetic.
package main

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/identity"
	"github.com/gamelink-project/gamelink/shim/invite"
	"github.com/gamelink-project/gamelink/shim/security"
	"github.com/tailscale/tailcat"
	"net/netip"
	"os"
	"strings"
	"tailscale.com/tailcfg"
	"unicode"
)

type fixture struct {
	Method string `json:"method"`
	Input  any    `json:"input"`
	Value  any    `json:"value"`
	Error  string `json:"error"`
}

func main() {
	var cases []fixture
	add := func(m string, in, v any, e error) {
		s := ""
		if e != nil {
			s = e.Error()
			v = nil
		}
		cases = append(cases, fixture{m, in, v, s})
	}
	for _, p := range []int{-1, 0, 1, 80, 65535, 65536, 100000} {
		for _, opt := range []bool{false, true} {
			e := security.ValidatePort(p)
			if opt {
				e = security.ValidateOptionalPort(p)
			}
			add("security.port", map[string]any{"port": p, "optional": opt}, nil, e)
		}
	}
	for _, s := range []string{"", "a", "a-b", "A", "-a", "a-", "a_b", "a.b", "../x", "a/b", strings.Repeat("a", 32), strings.Repeat("a", 33), "a\n", "\x00", "\u0085", "\u00ad", "\u2028", "\U0001f600"} {
		add("security.slug", map[string]any{"text": s}, nil, security.ValidateSlug(s))
	}
	for _, s := range []string{"derp.example.com", "", "a b", "-bad.com", "a.", "127.0.0.1", strings.Repeat("a", 64), strings.Repeat("a", 65), "x\n", "é"} {
		add("security.hostname", map[string]any{"text": s}, nil, security.ValidateHostname(s))
	}
	for _, s := range []string{"", "  ", "ok name", "tab\there", "x\u202ey", strings.Repeat("é", 11), "\u00a0hi\u0085", "a\u00a0b", "a\u200bb", "😀", "a\u2069b", "\ufeff", "\u034f", "\ue000", "\u0378"} {
		for _, required := range []bool{false, true} {
			add("security.display", map[string]any{"field": "name", "text": s, "maxRunes": 10, "required": required}, nil, security.ValidateDisplayText("name", s, 10, required))
		}
	}
	for _, s := range []string{"", "127.0.0.1", "127.0.0.1:0", "127.0.0.1:99999", "example.com:80", "0.0.0.0:80", "[::]:80", "224.0.0.1:80", "127.0.0.1:abc", "127.0.0.1:80x", " localhost:+80 ", "[::ffff:127.0.0.1]:80", "[fe80::1%eth0]:80", "[127.0.0.1]:80", "127.0.0.1:00080", ":80", "[::1]x:80", "[::1]:80:90", "127.0.0.1:+9223372036854775808", "[::1%]:80", "127.0.0.1:+", "[::1]:-80", "[fe80::1%zone space]:80"} {
		v, e := security.ParseDestination(s)
		add("security.destination", map[string]any{"text": s}, v.String(), e)
	}
	for _, s := range []string{"", "8.8.8.8", "203.0.113.5", "2606:4700::1", "127.0.0.1", "10.1.2.3", "192.168.0.1", "172.16.5.5", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "fe80::1", "fd00::1", "224.0.0.1", "ff02::1", "::ffff:10.0.0.1", "255.255.255.255", "0.1.2.3"} {
		ip, _ := netip.ParseAddr(s)
		add("security.public_ip", map[string]any{"text": s}, security.IsPublicIP(ip), nil)
	}
	for _, s := range []string{"", "nodekey:" + strings.Repeat("0", 64), "nodekey:" + strings.Repeat("a", 64), " \nnodekey:" + strings.Repeat("1", 64) + "\u00a0", "nodekey:abc", "nodekey:" + strings.Repeat("A", 64), "nodekey:" + strings.Repeat("é", 32), "x"} {
		v, e := identity.ParsePublicKey(s)
		add("identity.public_key", map[string]any{"text": s}, v.String(), e)
	}
	add("games.all", map[string]any{}, games.All(), nil)
	for _, g := range []games.Type{games.MinecraftJava, games.MinecraftBedrock, games.Factorio, games.Custom, "bad", "x\n"} {
		for _, p := range []games.Protocol{games.TCP, games.UDP, "icmp"} {
			add("games.check", map[string]any{"game": g, "proto": p}, nil, games.CheckProtocol(g, p))
		}
	}
	for _, s := range []string{"", "--mp-connect {address}", "{host}:{port}", "{address}{host}{port}", "{{host}}"} {
		add("games.expand", map[string]any{"text": s, "port": 19132}, games.ExpandLaunch(s, 19132), nil)
		add("games.placeholder", map[string]any{"text": s}, games.HasPlaceholder(s), nil)
	}
	for _, s := range []string{"normal tcp message", "secret SYNTHETIC_SECRET ok", "privkey:0123456789abcdef", "tc" + strings.Repeat("a", 60), "étc" + strings.Repeat("a", 60), "xtc" + strings.Repeat("a", 60), "tc" + strings.Repeat("a", 59)} {
		secrets := []string{"short", "SYNTHETIC_SECRET"}
		add("security.redact", map[string]any{"text": s, "secrets": secrets}, security.NewRedactor(secrets...).Redact(s), nil)
	}
	pk := tailcat.NewPrivateKey()
	ci := tailcat.ConnInfo{ServerPublic: pk.Public.ServerPublic, ServerDiscoPublic: tailcat.DiscoPublicForNode(pk.Private), PresharedKey: pk.Public.PresharedKey, Region: []*tailcfg.DERPRegion{{RegionID: 900, RegionName: "Synthetic", Nodes: []*tailcfg.DERPNode{{Name: "a", RegionID: 900, HostName: "derp.example.com"}}}}}
	inv := invite.Invite{V: 1, Name: "Synthetic fixture", Game: games.MinecraftJava, Proto: games.TCP, Port: 25565, Addr: string(ci.Addr())}
	raw, _ := json.Marshal(inv)
	good := invite.Prefix + base64.RawURLEncoding.EncodeToString(raw)
	for _, s := range []string{"", " \n", "bad", "glink1.!!!", "glink1." + base64.RawURLEncoding.EncodeToString([]byte(`{"v":1,"extra":1}`)), "glink1." + base64.RawURLEncoding.EncodeToString([]byte(`{"v":1}{}`)), good, good + "===", good[:30] + "\n " + good[30:]} {
		v, _, e := invite.Parse(s, invite.Options{})
		add("invite.decode", map[string]any{"text": s}, v, e)
	}

	// Capture policy against Tailcat-decoded synthetic addresses.
	for _, mutate := range []func(*tailcfg.DERPNode){
		func(n *tailcfg.DERPNode) {},
		func(n *tailcfg.DERPNode) { n.IPv4 = "127.0.0.1" },
		func(n *tailcfg.DERPNode) { n.IPv4 = "100.64.0.1" },
		func(n *tailcfg.DERPNode) { n.IPv6 = "fd00::1" },
		func(n *tailcfg.DERPNode) { n.InsecureForTests = true },
		func(n *tailcfg.DERPNode) { n.DERPPort = 22 },
		func(n *tailcfg.DERPNode) { n.HostName = "10.0.0.1" },
		func(n *tailcfg.DERPNode) { n.HostName = "evil host" },
		func(n *tailcfg.DERPNode) { n.CertName = "bad cert" },
		func(n *tailcfg.DERPNode) { n.CertName = "sha256-raw:anything" },
		func(n *tailcfg.DERPNode) { n.HostName = "relay.tailcat.dev" },
		func(n *tailcfg.DERPNode) { n.IPv4 = "203.0.113.7" },
	} {
		node := &tailcfg.DERPNode{Name: "a", RegionID: 900, HostName: "derp.example.com"}
		mutate(node)
		original := ci
		original.Region = []*tailcfg.DERPRegion{{RegionID: 900, RegionName: "Synthetic", Nodes: []*tailcfg.DERPNode{node}}}
		address := string(original.Addr())
		decoded, e := tailcat.ParseAddr(tailcat.Addr(address))
		if e != nil {
			panic(e)
		}
		for _, local := range []bool{false, true} {
			details, e := invite.ValidateAddr(address, invite.Options{AllowLocalRelay: local})
			add("invite.relay", map[string]any{"serverKeyPresent": !decoded.ServerPublic.IsZero(), "presharedKeyPresent": !decoded.PresharedKey.IsZero(), "serverKey": decoded.ServerPublic.String(), "regions": decoded.Region, "allowLocalRelay": local, "fixtureAddr": address}, details, e)
		}
	}
	for _, data := range []string{`null`, `[]`, `{"V":2,"v":1}`, strings.Replace(string(raw), `"v":1`, `"v":1,"V":2`, 1), strings.Replace(string(raw), `"v":1`, `"V":2,"v":1`, 1), strings.Replace(string(raw), `"v":1`, `"v":1,"v":2`, 1), string(raw) + "}", string(raw) + "]", string(raw) + " trailing", strings.Replace(string(raw), `"name":"Synthetic fixture"`, `"name":"\ud800"`, 1)} {
		encoded := invite.Prefix + base64.RawURLEncoding.EncodeToString([]byte(data))
		v, _, e := invite.Parse(encoded, invite.Options{})
		add("invite.decode", map[string]any{"text": encoded}, v, e)
	}

	for _, name := range []string{"Synthetic fixture", "<&é😀"} {
		sample := inv
		sample.Name = name
		sample.Note = "<&é\u2028"
		encoded, e := invite.Encode(sample, invite.Options{})
		add("invite.encode", sample, encoded, e)
	}
	b, _ := json.MarshalIndent(cases, "", "  ")
	if e := os.WriteFile("core/fixtures/pure.json", b, 0644); e != nil {
		panic(e)
	}
	f, e := os.Create("core/crates/gamelink-core/src/go_unicode.rs")
	if e != nil {
		panic(e)
	}
	defer f.Close()
	fmt.Fprintf(f, "// Generated from Go unicode %s by internal/purefixtures/generate.go.\n// Preserve the original Go validator's Unicode semantics.\n", unicode.Version)
	for _, t := range []struct {
		name string
		fn   func(rune) bool
	}{{"PRINT", unicode.IsPrint}, {"SPACE", unicode.IsSpace}, {"CONTROL", unicode.IsControl}} {
		fmt.Fprintf(f, "pub const %s: &[(u32,u32)] = &[\n", t.name)
		start := -1
		for r := 0; r <= unicode.MaxRune+1; r++ {
			yes := r <= unicode.MaxRune && t.fn(rune(r))
			if yes && start < 0 {
				start = r
			}
			if !yes && start >= 0 {
				fmt.Fprintf(f, "(%d,%d),\n", start, r-1)
				start = -1
			}
		}
		fmt.Fprintln(f, "];")
	}
}
