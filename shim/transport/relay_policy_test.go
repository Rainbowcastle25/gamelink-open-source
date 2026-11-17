package transport

import (
	"context"
	"net/http"
	"net/netip"
	"net/url"
	"testing"

	"github.com/tailscale/tailcat"
	"tailscale.com/feature"
	"tailscale.com/feature/buildfeatures"
	"tailscale.com/tailcfg"
	"tailscale.com/types/key"
)

type relayTestResolver map[string][]netip.Addr

func (r relayTestResolver) LookupNetIP(_ context.Context, _, host string) ([]netip.Addr, error) {
	return r[host], nil
}

func relayTestAddr(host string) tailcat.Addr {
	ci := tailcat.ConnInfo{
		ServerPublic: tailcat.NodePublic{NodePublic: key.NewNode().Public()},
		RegionID:     tailcfg.DERPRegionID(1),
		Region: []*tailcfg.DERPRegion{{
			RegionID: 1,
			Nodes:    []*tailcfg.DERPNode{{HostName: host}},
		}},
	}
	return ci.Addr()
}

func TestPinRelayAddressPinsPublicDNSAndPreservesTLSName(t *testing.T) {
	addr, err := pinRelayAddress(context.Background(), relayTestAddr("relay.example"), false, relayTestResolver{
		"relay.example": {netip.MustParseAddr("8.8.8.8"), netip.MustParseAddr("2001:4860:4860::8888")},
	})
	if err != nil {
		t.Fatal(err)
	}
	ci, err := tailcat.ParseAddr(addr)
	if err != nil {
		t.Fatal(err)
	}
	n := ci.Region[0].Nodes[0]
	if n.HostName != "relay.example" || n.IPv4 != "8.8.8.8" || n.IPv6 != "2001:4860:4860::8888" {
		t.Fatalf("relay node was not pinned to checked addresses: %+v", n)
	}
	if n.CertName != "relay.example" {
		t.Fatalf("TLS certificate hostname = %q, want relay.example", n.CertName)
	}
}

func TestPinRelayAddressRejectsMixedPublicAndPrivateDNS(t *testing.T) {
	_, err := pinRelayAddress(context.Background(), relayTestAddr("relay.example"), false, relayTestResolver{
		"relay.example": {netip.MustParseAddr("8.8.8.8"), netip.MustParseAddr("127.0.0.1")},
	})
	if err == nil {
		t.Fatal("accepted DNS answer containing a private address")
	}
}

func TestPinRelayAddressLocalOptInStillPinsDNS(t *testing.T) {
	addr, err := pinRelayAddress(context.Background(), relayTestAddr("relay.example"), true, relayTestResolver{
		"relay.example": {netip.MustParseAddr("127.0.0.1")},
	})
	if err != nil {
		t.Fatal(err)
	}
	ci, err := tailcat.ParseAddr(addr)
	if err != nil {
		t.Fatal(err)
	}
	n := ci.Region[0].Nodes[0]
	if n.HostName != "relay.example" || n.IPv4 != "127.0.0.1" || n.IPv6 != "none" {
		t.Fatalf("local relay was not pinned: %+v", n)
	}
}

func TestPinRelayAddressPreservesCustomSNIAndCertificateName(t *testing.T) {
	ci, err := tailcat.ParseAddr(relayTestAddr("relay.example"))
	if err != nil {
		t.Fatal(err)
	}
	ci.Region[0].Nodes[0].CertName = "cert.example"
	addr, err := pinRelayAddress(context.Background(), ci.Addr(), false, relayTestResolver{
		"relay.example": {netip.MustParseAddr("8.8.8.8")},
	})
	if err != nil {
		t.Fatal(err)
	}
	got, err := tailcat.ParseAddr(addr)
	if err != nil {
		t.Fatal(err)
	}
	n := got.Region[0].Nodes[0]
	if n.HostName != "relay.example" || n.CertName != "cert.example" || n.IPv4 != "8.8.8.8" {
		t.Fatalf("SNI, certificate name, or checked IP changed: %+v", n)
	}
}

func TestPinRelayAddressRejectsConfiguredProxy(t *testing.T) {
	if !buildfeatures.HasUseProxy {
		t.Skip("this test binary omits optional proxy support")
	}
	restore := feature.HookProxyFromEnvironment.SetForTest(func(_ *http.Request) (*url.URL, error) {
		return &url.URL{Scheme: "http", Host: "proxy.example:8080"}, nil
	})
	defer restore()
	_, err := pinRelayAddress(context.Background(), relayTestAddr("relay.example"), false, relayTestResolver{
		"relay.example": {netip.MustParseAddr("8.8.8.8")},
	})
	if err == nil || err.Error() != "system proxy is not supported for a pinned relay" {
		t.Fatalf("configured proxy was not rejected without leaking proxy details: %v", err)
	}
}

func TestPinRelayAddressKeepsLocalRelayControlsOptIn(t *testing.T) {
	ci, err := tailcat.ParseAddr(relayTestAddr("8.8.8.8"))
	if err != nil {
		t.Fatal(err)
	}
	n := ci.Region[0].Nodes[0]
	n.DERPPort = 8443
	if _, err := pinRelayAddress(context.Background(), ci.Addr(), false, relayTestResolver{}); err == nil {
		t.Fatal("accepted a non-standard relay port without opt-in")
	}
	if _, err := pinRelayAddress(context.Background(), ci.Addr(), true, relayTestResolver{}); err != nil {
		t.Fatalf("local relay opt-in should permit a custom port: %v", err)
	}
	n.DERPPort = 0
	n.InsecureForTests = true
	if _, err := pinRelayAddress(context.Background(), ci.Addr(), false, relayTestResolver{}); err == nil {
		t.Fatal("accepted an insecure test relay without opt-in")
	}
}
