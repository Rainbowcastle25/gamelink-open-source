package security

import (
	"github.com/gamelink-project/gamelink/shim/rustcore"
	"net/netip"
)

func ValidatePort(p int) error {
	return rustcore.Invoke("security.port", map[string]any{"port": p}, nil)
}
func ValidateOptionalPort(p int) error {
	return rustcore.Invoke("security.port", map[string]any{"port": p, "optional": true}, nil)
}
func ValidateSlug(s string) error {
	return rustcore.Invoke("security.slug", map[string]any{"text": s}, nil)
}
func ValidateDisplayText(field, s string, maxRunes int, required bool) error {
	return rustcore.Invoke("security.display", map[string]any{"field": field, "text": s, "maxRunes": maxRunes, "required": required}, nil)
}
func ValidateHostname(h string) error {
	return rustcore.Invoke("security.hostname", map[string]any{"text": h}, nil)
}
func ParseDestination(s string) (netip.AddrPort, error) {
	var address string
	if err := rustcore.Invoke("security.destination", map[string]any{"text": s}, &address); err != nil {
		return netip.AddrPort{}, err
	}
	return netip.ParseAddrPort(address)
}
func IsPublicIP(ip netip.Addr) bool {
	var public bool
	return rustcore.Invoke("security.public_ip", map[string]any{"text": ip.String()}, &public) == nil && public
}
