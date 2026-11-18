package main

import (
	"context"
	"errors"
	"net/netip"
	"time"

	"github.com/gamelink-project/gamelink/shim/gamehost"
	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/transport"
)

type hostedGame struct {
	Type       games.Type
	Protocol   games.Protocol
	TunnelPort uint16
	Dest       string
}

type hostedResource struct {
	server *transport.GameServer
	peers  *transport.PeerActivity
}

type hostSnapshot struct {
	Connected map[string]time.Time    `json:"connected"`
	Requests  []transport.JoinRequest `json:"requests,omitempty"`
}

func resolveHostRelay() (any, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	region, err := transport.ResolveRegion(ctx, transport.RegionSpec{})
	if err != nil {
		return nil, errors.New("could not select a Tailcat relay; check your internet connection and try again")
	}
	return region, nil
}

func startHost(id resourceID, game hostedGame, keys gamehost.Keys, approved []string) error {
	if id.ID == "" || id.Generation == 0 {
		return errors.New("invalid host resource generation")
	}
	for live := range owner.hosts {
		if live.ID == id.ID {
			return errors.New("host resource must be stopped before replacement")
		}
	}
	dest, err := netip.ParseAddrPort(game.Dest)
	if err != nil || dest.Addr() != netip.AddrFrom4([4]byte{127, 0, 0, 1}) {
		return errors.New("desktop hosting requires a 127.0.0.1 destination")
	}
	private, psk, err := keys.Parse()
	if err != nil {
		return err
	}
	peers, err := gamehost.ParseApproved(approved)
	if err != nil {
		return err
	}
	activity := transport.NewPeerActivity()
	server, err := transport.StartGameServer(transport.ServerOptions{
		ID: id.ID, Game: game.Type, Protocol: game.Protocol, TunnelPort: game.TunnelPort,
		Dest: dest, Private: private, PSK: psk, Region: keys.Region, Approved: peers, Peers: activity,
		CollectRequests: true, ExpiresAt: keys.ExpiresAt,
	})
	if err != nil {
		return err
	}
	owner.hosts[id] = hostedResource{server: server, peers: activity}
	return nil
}

func stopHost(id resourceID) (any, error) {
	if host, ok := owner.hosts[id]; ok {
		host.server.Close()
		delete(owner.hosts, id)
		return hostSnapshot{Connected: host.peers.Snapshot()}, nil
	}
	return nil, nil
}
