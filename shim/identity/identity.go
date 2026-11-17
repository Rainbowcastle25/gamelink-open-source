// Package identity manages persistent cryptographic identities.
//
// A GameLink device identity is a Tailcat/WireGuard node key pair. The public
// half ("nodekey:<64 hex>") is what a server owner approves; the private half
// never leaves the device and is sealed at rest (DPAPI on Windows, 0600 file
// permissions everywhere).
package identity

import (
	"fmt"
	"time"

	"tailscale.com/types/key"
)

// Identity is a device's persistent node key.
type Identity struct {
	Private key.NodePrivate
	Created time.Time
}

// New generates a fresh identity in memory.
func New() *Identity {
	return &Identity{Private: key.NewNode(), Created: time.Now().UTC().Truncate(time.Second)}
}

// PublicKey returns the node public key.
func (i *Identity) PublicKey() key.NodePublic { return i.Private.Public() }

// PublicKeyString returns the shareable form "nodekey:<64 hex chars>".
func (i *Identity) PublicKeyString() string { return i.Private.Public().String() }

// ParsePublicKey strictly parses a node public key as printed by GameLink:
// "nodekey:" followed by exactly 64 hex digits. Anything else (including the
// all-zero key and keys with surrounding noise) is rejected.
func ParsePublicKey(s string) (key.NodePublic, error) {
	canonical, err := validatePublicKeyFormat(s)
	if err != nil {
		return key.NodePublic{}, err
	}
	var k key.NodePublic
	if err := k.UnmarshalText([]byte(canonical)); err != nil {
		return key.NodePublic{}, fmt.Errorf("invalid public key: %w", err)
	}
	return k, nil
}
