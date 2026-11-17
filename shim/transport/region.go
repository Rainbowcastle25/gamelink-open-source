package transport

import (
	"context"
	"errors"
	"fmt"

	"github.com/tailscale/tailcat"
	"tailscale.com/tailcfg"

	"github.com/gamelink-project/gamelink/shim/security"
)

// RegionSpec says how an agent should choose its relay (DERP) region.
type RegionSpec struct {
	// Host, if set, uses your own DERP relay at this host name (region 900,
	// the range reserved for private relays). No network access is needed.
	Host string
	// RegionID, if non-zero, picks that region of the public Tailcat DERP map.
	RegionID int
	// MapURL overrides the DERP map URL (default: tailcat.dev's map).
	MapURL string
}

// ResolveRegion returns the pinned DERP region for a new game. Pinning it at
// creation time keeps the game's Tailcat address stable across restarts and
// lets the agent start without fetching a DERP map.
func ResolveRegion(ctx context.Context, spec RegionSpec) (*tailcfg.DERPRegion, error) {
	if spec.Host != "" {
		if err := security.ValidateHostname(spec.Host); err != nil {
			return nil, err
		}
		return &tailcfg.DERPRegion{
			RegionID: 900, RegionCode: "custom", RegionName: "Custom relay",
			Nodes: []*tailcfg.DERPNode{{Name: "900a", RegionID: 900, HostName: spec.Host}},
		}, nil
	}
	var opts []any
	if spec.MapURL != "" {
		opts = append(opts, tailcat.DERPMapURL(spec.MapURL))
	}
	opts = append(opts, tailcat.ExpandForServer)
	dm, err := tailcat.FetchDERPMap(ctx, opts...)
	if err != nil {
		return nil, fmt.Errorf("fetch relay map (internet access is required once when adding a game; "+
			"use --relay-host for a private relay): %w", err)
	}
	id := tailcfg.DERPRegionID(spec.RegionID)
	if id == 0 {
		id, err = tailcat.PickBestRegion(ctx, dm)
		if err != nil || id == 0 {
			return nil, errors.New("could not measure the nearest relay; pass --relay-region <id> to choose one")
		}
	}
	ci := tailcat.ConnInfo{RegionID: id}
	if err := ci.Expand(ctx, dm); err != nil {
		return nil, fmt.Errorf("relay region %d: %w", id, err)
	}
	if len(ci.Region) != 1 || len(ci.Region[0].Nodes) == 0 {
		return nil, fmt.Errorf("relay region %d has no usable relay nodes", id)
	}
	return ci.Region[0], nil
}
