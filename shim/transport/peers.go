package transport

import (
	"maps"
	"net"
	"net/netip"
	"sync"
	"time"

	"tailscale.com/types/key"
)

// PeerActivity tracks the last authorized gameplay session for each device.
type PeerActivity struct {
	mu        sync.Mutex
	connected map[string]time.Time
}

func NewPeerActivity() *PeerActivity {
	return &PeerActivity{connected: make(map[string]time.Time)}
}

func (p *PeerActivity) recordSession(allowed map[netip.Addr]key.NodePublic, remote net.Addr) {
	if p == nil {
		return
	}
	ip, ok := addrOf(remote)
	if k, found := allowed[ip]; ok && found {
		p.mu.Lock()
		defer p.mu.Unlock()
		p.connected[k.String()] = time.Now()
	}
}

// Snapshot returns the last gameplay session observed for each approved device.
func (p *PeerActivity) Snapshot() map[string]time.Time {
	p.mu.Lock()
	defer p.mu.Unlock()
	return maps.Clone(p.connected)
}
