package consolebridge

import (
	"errors"
	"github.com/gamelink-project/gamelink/shim/rustcore"
	"net/netip"
)

var ErrCandidate = errors.New("unsafe or unsupported ICE candidate")

// RewriteCandidates replaces only the candidate address and port. allowedIP
// must be the verified discovery peer or configured server; callers fail closed.
func RewriteCandidates(input string, allowedIP netip.Addr, firstPort, lastPort uint16, relay netip.AddrPort) (string, []netip.AddrPort, error) {
	return candidates("console.candidates_rewrite", input, allowedIP, firstPort, lastPort, relay)
}
func candidates(method, input string, allowedIP netip.Addr, first, last uint16, relay netip.AddrPort) (string, []netip.AddrPort, error) {
	var out struct {
		Data    []byte   `json:"data"`
		Targets []string `json:"targets"`
	}
	if err := rustcore.Invoke(method, struct {
		Data    []byte `json:"data"`
		Allowed string `json:"allowed"`
		First   uint16 `json:"first"`
		Last    uint16 `json:"last"`
		Relay   string `json:"relay"`
	}{[]byte(input), allowedIP.String(), first, last, relay.String()}, &out); err != nil {
		return "", nil, ErrCandidate
	}
	var targets []netip.AddrPort
	for _, s := range out.Targets {
		a, err := netip.ParseAddrPort(s)
		if err != nil {
			return "", nil, ErrCandidate
		}
		targets = append(targets, a)
	}
	return string(out.Data), targets, nil
}
