// Package invite encodes, parses and validates GameLink invitations.
//
// An invitation is a secret capability: it contains the game's full Tailcat
// address (server keys, pre-shared key and relay details). It deliberately
// does NOT contain the server's real destination address, so importing a
// hostile or malformed invitation can never make the client connect anywhere
// except through Tailcat to the server it names.
package invite

import (
	"errors"
	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/rustcore"
	"github.com/tailscale/tailcat"
)

// Prefix starts every invitation string.
const Prefix = "glink1."

const (
	maxInviteLen = 8 * 1024
	maxAddrLen   = 2048
	maxNodes     = 8
)

// Invite is the decoded invitation payload.
type Invite struct {
	V         int            `json:"v"`
	Name      string         `json:"name"`
	Game      games.Type     `json:"game"`
	Proto     games.Protocol `json:"proto"`
	Port      uint16         `json:"port"` // port inside the tunnel
	Addr      string         `json:"addr"` // full Tailcat address: SECRET
	Note      string         `json:"note,omitempty"`
	ExpiresAt string         `json:"expiresAt,omitempty"`
	// ConsoleBridge identifies the optional LAN-to-LAN bridge protocol.
	// Zero means the invitation is for the original single-protocol forwarder.
	ConsoleBridge int `json:"consoleBridge,omitempty"`
}

// Options tune validation. The zero value is the strict production setting.
type Options struct {
	// AllowLocalRelay accepts relay (DERP) nodes on private or loopback IPs,
	// insecure TLS relays and non-standard ports. It exists for tests and
	// for people who run their own LAN relay; it is never the default.
	AllowLocalRelay bool
}

// Details is what the UI can safely show about an address.
type Details struct {
	RelayHosts   []string `json:"relayHosts"`
	RelayRegion  string   `json:"relayRegion"`
	ServerKey    string   `json:"serverKey"` // short fingerprint of the server's public key
	DefaultRelay bool     `json:"defaultRelay"`
}

// Encode serialises and validates an invitation.
func Encode(inv Invite, opts Options) (string, error) {
	inv.V = 1
	if _, err := Validate(inv, opts); err != nil {
		return "", err
	}
	var out string
	err := rustcore.Invoke("invite.encode", inv, &out)
	return out, err
}

func Parse(s string, opts Options) (Invite, Details, error) {
	var inv Invite
	if err := rustcore.Invoke("invite.decode", map[string]any{"text": s}, &inv); err != nil {
		return Invite{}, Details{}, err
	}
	det, err := ValidateAddr(inv.Addr, opts)
	if err != nil {
		return Invite{}, Details{}, err
	}
	return inv, det, nil
}

func Validate(inv Invite, opts Options) (Details, error) {
	if err := rustcore.Invoke("invite.fields", inv, nil); err != nil {
		return Details{}, err
	}
	return ValidateAddr(inv.Addr, opts)
}

// ValidateAddr keeps Tailcat's canonical address parser in Go; Rust receives
// decoded relay metadata and owns the policy decisions. No network is touched.
func ValidateAddr(addr string, opts Options) (Details, error) {
	var canonical string
	if err := rustcore.Invoke("invite.address_format", map[string]any{"text": addr}, &canonical); err != nil {
		return Details{}, err
	}
	ci, err := tailcat.ParseAddr(tailcat.Addr(canonical))
	if err != nil {
		return Details{}, errors.New("the server address is not a valid Tailcat address")
	}
	var det Details
	err = rustcore.Invoke("invite.relay", map[string]any{
		"serverKeyPresent": !ci.ServerPublic.IsZero(), "presharedKeyPresent": !ci.PresharedKey.IsZero(),
		"serverKey": ci.ServerPublic.String(), "regions": ci.Region, "allowLocalRelay": opts.AllowLocalRelay,
	}, &det)
	return det, err
}
