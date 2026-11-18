// Package gamehost adapts the shared game server to the native hosts' JSON boundary.
package gamehost

import (
	"errors"
	"time"

	"github.com/gamelink-project/gamelink/shim/invite"
	"github.com/tailscale/tailcat"
	"tailscale.com/tailcfg"
	"tailscale.com/types/key"
)

type Keys struct {
	PrivateKey   string
	PresharedKey string
	Region       *tailcfg.DERPRegion
	ExpiresAt    time.Time
}

func Generate() map[string]string {
	k := tailcat.NewPrivateKey()
	p, _ := k.Private.MarshalText()
	s, _ := k.Public.PresharedKey.MarshalText()
	return map[string]string{"privateKey": string(p), "presharedKey": string(s)}
}

func (k Keys) Parse() (private key.NodePrivate, psk tailcat.PresharedKey, err error) {
	if private.UnmarshalText([]byte(k.PrivateKey)) != nil || private.IsZero() {
		err = errors.New("invalid private key")
	} else if psk.UnmarshalText([]byte(k.PresharedKey)) != nil || psk.IsZero() {
		err = errors.New("invalid pre-shared key")
	}
	return
}

func (k Keys) PublicKey() (string, error) {
	private, _, err := k.Parse()
	if err != nil {
		return "", err
	}
	return private.Public().String(), nil
}

func (k Keys) Address(allowLocalRelay bool) (string, error) {
	private, psk, err := k.Parse()
	if err != nil {
		return "", err
	}
	info := tailcat.ConnInfo{
		ServerPublic:      tailcat.NodePublic{NodePublic: private.Public()},
		ServerDiscoPublic: tailcat.DiscoPublicForNode(private),
		PresharedKey:      psk,
		Region:            []*tailcfg.DERPRegion{k.Region},
	}
	addr := string(info.Addr())
	_, err = invite.ValidateAddr(addr, invite.Options{AllowLocalRelay: allowLocalRelay})
	return addr, err
}

func ParseApproved(texts []string) ([]key.NodePublic, error) {
	approved := make([]key.NodePublic, 0, len(texts))
	for _, text := range texts {
		var k key.NodePublic
		if k.UnmarshalText([]byte(text)) != nil || k.IsZero() {
			return nil, errors.New("invalid approved key")
		}
		approved = append(approved, k)
	}
	return approved, nil
}
