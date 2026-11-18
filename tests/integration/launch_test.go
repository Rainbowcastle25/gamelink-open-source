package integration

import (
	"context"
	"net"
	"testing"
	"time"

	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/transport"
)

// "Play" starts the game against the local port the tunnel actually bound, so
// the status the desktop reads must name the real port, never the server's.
// With the game's standard UDP port taken, the join must fall back to another
// port, say so, and carry traffic there. (How that port is turned into game
// launch arguments is Rust-owned; see docs/history/test-migration-map.md.)
func TestJoinReportsActualLocalPortWhenStandardPortIsBusy(t *testing.T) {
	busy, err := net.ListenPacket("udp", "127.0.0.1:34197")
	if err != nil {
		t.Skipf("cannot occupy the standard Factorio port: %v", err)
	}
	defer busy.Close()

	relay := startRelay(t)
	env := newAgentEnv(t, relay)
	env.addGame("fac", games.Factorio, games.UDP, 34197, udpEcho(t))
	devKey, pub := newKey()
	env.approve("fac", pub, "me")
	env.serve()

	tun, err := transport.Join(context.Background(), desktopOpts(env.addr("fac"), games.UDP, 34197, devKey, true))
	if err != nil {
		t.Fatal(err)
	}
	defer tun.Stop()
	st := tun.Status()
	if !st.PortFallback || st.LocalPort == 34197 || st.LocalPort == 0 {
		t.Fatalf("expected a fallback port, got %+v", st)
	}
	ua, err := net.ResolveUDPAddr("udp", st.LocalAddr)
	if err != nil || ua.Port != st.LocalPort {
		t.Fatalf("LocalAddr %q does not name the bound port %d (%v)", st.LocalAddr, st.LocalPort, err)
	}
	c, err := net.DialUDP("udp", nil, ua)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	c.SetDeadline(time.Now().Add(15 * time.Second))
	if _, err := c.Write([]byte("play")); err != nil {
		t.Fatal(err)
	}
	buf := make([]byte, 16)
	if n, err := c.Read(buf); err != nil || string(buf[:n]) != "play" {
		t.Fatalf("echo through the fallback port = %q, %v", buf[:n], err)
	}
}
