package consolebridge

import (
	"context"
	"net/netip"
	"testing"
)

func TestStoppedClientCannotAllocateLateConsoleSession(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	c := &Client{ctx: ctx, opts: ClientOptions{LANIP: netip.MustParseAddr("127.0.0.1")},
		advertisement: []byte{1}, sessions: map[uint64]*clientSession{}, routes: map[clientRoute]uint64{}}
	c.handleSignal(clientRoute{source: netip.MustParseAddrPort("127.0.0.2:5000"), sender: 1, connection: 2},
		Signal{Type: SignalOffer, ConnectionID: 2, Data: "v=0\r\n"})
	if c.SessionCount() != 0 {
		t.Fatal("a discovery callback allocated a socket after Stop")
	}
}
