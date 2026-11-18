package transport

import (
	"testing"
	"time"

	"golang.org/x/time/rate"
	"tailscale.com/types/key"
)

func TestJoinRequestBatchesDeduplicateRateLimitAndCap(t *testing.T) {
	var g GameServer
	g.requests = joinRequests{
		newKeys: rate.NewLimiter(4, 8),
		pending: make(map[key.NodePublic]JoinRequest),
	}
	now := time.Now().UTC()
	peer := key.NewNode().Public()
	g.requests.record(peer, now)
	for i := 0; i < 20; i++ {
		g.requests.record(key.NewNode().Public(), now)
	}
	g.requests.record(peer, now.Add(time.Millisecond))
	batch := g.TakeRequests()
	if len(batch) != 8 {
		t.Fatalf("burst accepted %d keys, want 8", len(batch))
	}
	var repeated JoinRequest
	for _, entry := range batch {
		if entry.PublicKey == peer {
			repeated = entry
			break
		}
	}
	if repeated.Attempts != 2 || !repeated.FirstSeen.Equal(now) || !repeated.LastSeen.Equal(now.Add(time.Millisecond)) {
		t.Fatal("repeat knock lost its count or timestamps")
	}
	if len(g.TakeRequests()) != 0 {
		t.Fatal("a published batch was returned twice")
	}
	for i := 0; i < 40; i++ {
		g.requests.record(key.NewNode().Public(), now.Add(time.Duration(i+1)*time.Second))
	}
	if len(g.TakeRequests()) != maxPendingRequests {
		t.Fatal("batch did not cap its pending keys")
	}
}
