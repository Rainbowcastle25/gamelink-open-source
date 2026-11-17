package transport

import (
	"context"
	"errors"
	"net"
	"net/http"
	"net/netip"
	"net/url"
	"strings"

	"github.com/tailscale/tailcat"
	"tailscale.com/feature"
	"tailscale.com/feature/buildfeatures"
	"tailscale.com/tailcfg"

	"github.com/gamelink-project/gamelink/shim/security"
)

// relayResolver is kept small so DNS pinning can be tested without depending
// on the machine's resolver configuration.
type relayResolver interface {
	LookupNetIP(context.Context, string, string) ([]netip.Addr, error)
}

// PinRelayAddress resolves each DERP hostname once and embeds the resulting
// literal addresses in the Tailcat address. Tailcat's own DERP dialer otherwise
// performs a later hostname lookup, which would leave a DNS rebinding window
// between policy validation and the actual socket dial. The original DERP
// hostname remains in HostName for TLS SNI and certificate verification; the
// explicit IPv4/IPv6 fields pin direct socket dials to the checked answers.
// When Tailscale's optional proxy hook would route this DERP connection through
// a proxy, pinning cannot constrain the proxy's own DNS resolution, so fail
// closed instead of silently losing the address pin.
//
// A zero allowLocal value admits only globally routable unicast addresses.
// Local relay opt-in relaxes that address check, but still pins the DNS answer.
func PinRelayAddress(ctx context.Context, addr tailcat.Addr, allowLocal bool) (tailcat.Addr, error) {
	return pinRelayAddress(ctx, addr, allowLocal, net.DefaultResolver)
}

func pinRelayAddress(ctx context.Context, addr tailcat.Addr, allowLocal bool, resolver relayResolver) (tailcat.Addr, error) {
	ci, err := tailcat.ParseAddr(addr)
	if err != nil || ci.ServerPublic.IsZero() || len(ci.Region) == 0 {
		return "", errors.New("invalid relay address")
	}
	for _, region := range ci.Region {
		if region == nil || len(region.Nodes) == 0 {
			return "", errors.New("invalid relay address")
		}
		for _, node := range region.Nodes {
			if node == nil || node.HostName == "" {
				return "", errors.New("invalid relay address")
			}
			if err := pinRelayNode(ctx, node, allowLocal, resolver); err != nil {
				return "", err
			}
		}
	}
	return ci.Addr(), nil
}

func pinRelayNode(ctx context.Context, node *tailcfg.DERPNode, allowLocal bool, resolver relayResolver) error {
	host := strings.TrimSuffix(node.HostName, ".")
	if host == "" || strings.ContainsAny(host, "\x00\r\n") {
		return errors.New("invalid relay host")
	}
	if node.DERPPort < 0 || node.DERPPort > 65535 {
		return errors.New("invalid relay port")
	}
	if !allowLocal {
		if node.InsecureForTests {
			return errors.New("insecure relay is disabled")
		}
		if node.DERPPort != 0 && node.DERPPort != 443 {
			return errors.New("non-standard relay port is disabled")
		}
	}
	if err := rejectRelayProxy(host); err != nil {
		return err
	}
	var ips []netip.Addr
	if ip, err := netip.ParseAddr(host); err == nil {
		ip = ip.Unmap()
		host = ip.String()
		ips = []netip.Addr{ip}
	} else {
		if err := security.ValidateHostname(host); err != nil {
			return errors.New("invalid relay host")
		}
		resolved, err := resolver.LookupNetIP(ctx, "ip", host)
		if ctx.Err() != nil {
			return ctx.Err()
		}
		if err != nil || len(resolved) == 0 {
			return errors.New("could not resolve relay host")
		}
		seen := map[netip.Addr]bool{}
		for _, ip := range resolved {
			if !ip.IsValid() || ip.Is4In6() {
				return errors.New("relay host resolved to an invalid address")
			}
			ip = ip.Unmap()
			if !allowLocal && !security.IsPublicIP(ip) {
				return errors.New("relay host resolved to a private or invalid address")
			}
			if !seen[ip] {
				seen[ip] = true
				ips = append(ips, ip)
			}
		}
	}
	if len(ips) == 0 {
		return errors.New("relay host has no usable address")
	}
	for _, ip := range ips {
		if !allowLocal && !security.IsPublicIP(ip) {
			return errors.New("relay host is private or invalid")
		}
	}
	if node.CertName == "" {
		if _, err := netip.ParseAddr(host); err != nil {
			node.CertName = host
		}
	}

	// Explicit family fields take precedence over HostName in Tailcat's DERP
	// dialer, so populate them from the checked answer and prevent a second DNS
	// lookup through HostName as a fallback.
	node.IPv4, node.IPv6 = "none", "none"
	for _, ip := range ips {
		if ip.Is4() && node.IPv4 == "none" {
			node.IPv4 = ip.String()
		} else if ip.Is6() && node.IPv6 == "none" {
			node.IPv6 = ip.String()
		}
	}
	// HostName is also DERP's TLS SNI name. Keep the validated DNS name here;
	// derphttp uses the explicit family fields for direct dialing and HostName
	// only as a fallback when neither family has an address.
	node.HostName = host
	return nil
}

func rejectRelayProxy(host string) error {
	if !buildfeatures.HasUseProxy {
		return nil
	}
	proxyFromEnvironment, ok := feature.HookProxyFromEnvironment.GetOk()
	if !ok {
		return nil
	}
	req := &http.Request{Method: http.MethodGet, URL: &url.URL{Scheme: "https", Host: host, Path: "/"}}
	proxy, err := proxyFromEnvironment(req)
	if err != nil || proxy != nil {
		return errors.New("system proxy is not supported for a pinned relay")
	}
	return nil
}
