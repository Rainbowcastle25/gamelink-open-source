// Package mobile exposes the small, gomobile-compatible GameLink API.
package mobile

import (
	"encoding/json"
	"errors"

	"github.com/gamelink-project/gamelink/shim/identity"
	"github.com/gamelink-project/gamelink/shim/invite"
	"tailscale.com/types/key"
)

// GenerateIdentity returns opaque private key bytes. The caller must protect
// these bytes in device secure storage and must never log them.
func GenerateIdentity() ([]byte, error) { return identity.New().Private.MarshalText() }

// PublicKey returns the shareable key corresponding to privateKey.
func PublicKey(privateKey []byte) (string, error) {
	var k key.NodePrivate
	if err := k.UnmarshalText(privateKey); err != nil || k.IsZero() {
		return "", errors.New("invalid device private key")
	}
	return k.Public().String(), nil
}

// PreviewInvitation validates an invitation and returns only public display
// information. The invitation and embedded Tailcat address are never returned.
func PreviewInvitation(text string) (string, error) {
	inv, details, err := invite.Parse(text, invite.Options{})
	if err != nil {
		return "", err
	}
	out := struct {
		Name          string         `json:"name"`
		Game          string         `json:"game"`
		Protocol      string         `json:"protocol"`
		Port          uint16         `json:"port"`
		Note          string         `json:"note,omitempty"`
		ConsoleBridge int            `json:"consoleBridge"`
		Details       invite.Details `json:"details"`
	}{inv.Name, string(inv.Game), string(inv.Proto), inv.Port, inv.Note, inv.ConsoleBridge, details}
	b, err := json.Marshal(out)
	return string(b), err
}
