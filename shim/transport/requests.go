package transport

import (
	"sync"
	"time"

	"golang.org/x/time/rate"
	"tailscale.com/types/key"
)

const maxPendingRequests = 32

// JoinRequest is a batch of refusals for one DERP-authenticated node key.
// The agent merges batches into its private, expiring request list.
type JoinRequest struct {
	PublicKey key.NodePublic `json:"publicKey"`
	FirstSeen time.Time      `json:"firstSeen"`
	LastSeen  time.Time      `json:"lastSeen"`
	Attempts  uint64         `json:"attempts"`
}

type joinRequests struct {
	mu      sync.Mutex
	newKeys *rate.Limiter
	pending map[key.NodePublic]JoinRequest
}

func (r *joinRequests) record(peer key.NodePublic, now time.Time) {
	r.mu.Lock()
	defer r.mu.Unlock()
	entry, found := r.pending[peer]
	if !found {
		if len(r.pending) >= maxPendingRequests || !r.newKeys.AllowN(now, 1) {
			return
		}
		entry = JoinRequest{PublicKey: peer, FirstSeen: now}
	}
	entry.LastSeen = now
	entry.Attempts++
	r.pending[peer] = entry
}

func (g *GameServer) TakeRequests() []JoinRequest {
	g.requests.mu.Lock()
	defer g.requests.mu.Unlock()
	out := make([]JoinRequest, 0, len(g.requests.pending))
	for _, entry := range g.requests.pending {
		out = append(out, entry)
	}
	clear(g.requests.pending)
	return out
}

// Tailcat v0.7.0 exposes the DERP-authenticated source key at its allowlist
// rejection through this exact log call, before it configures any WireGuard
// peer or sends a reply. Keep the allowlist in place. The subprocess test
// exercises this adapter against real Tailcat to detect changes on upgrade.
func (g *GameServer) tailcatLog(format string, args ...any) {
	if format == "ignoring meow from %v: not in allowedClients" && len(args) == 1 && !g.expired() {
		if raw, ok := args[0].(string); ok {
			var peer key.NodePublic
			if peer.UnmarshalText([]byte(raw)) == nil && !peer.IsZero() {
				g.denied.Add(1)
				if g.requests.newKeys != nil {
					g.requests.record(peer, time.Now().UTC())
				}
				g.logRefusal()
			}
		}
	}
	if g.opts.Logf != nil {
		g.opts.Logf(format, args...)
	}
}
