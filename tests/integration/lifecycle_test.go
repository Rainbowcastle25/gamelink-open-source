package integration

import (
	"context"
	"net"
	"runtime"
	"testing"

	"tailscale.com/envknob"
	"time"

	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/transport"
)

// A busy explicit local port is an immediate, clear error and leaves nothing
// behind; a busy preferred port falls back to a free one.
func TestLocalPortConflicts(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)
	k, pub := newKey()
	env.approve("mc", pub, "d")
	env.serve()
	addr := env.addr("mc")

	// Occupy a port on loopback.
	busy, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer busy.Close()
	busyPort := uint16(busy.Addr().(*net.TCPAddr).Port)

	// Explicit port that is busy: error, no network use, no leak.
	opts := joinOpts(addr, games.TCP, 25565, k)
	opts.LocalPort = busyPort
	start := time.Now()
	if tun, err := transport.Join(context.Background(), opts); err == nil {
		tun.Stop()
		t.Fatal("Join succeeded on a busy explicit port")
	} else {
		t.Logf("busy explicit port: %v", err)
	}
	if time.Since(start) > 3*time.Second {
		t.Error("port conflict should fail fast, before any network work")
	}

	// Preferred (standard) port busy: fall back to a free port and say so.
	opts = joinOpts(addr, games.TCP, 25565, k)
	opts.PreferPort = busyPort
	tun, err := transport.Join(context.Background(), opts)
	if err != nil {
		t.Fatalf("Join with busy preferred port: %v", err)
	}
	defer tun.Stop()
	st := tun.Status()
	if !st.PortFallback || st.LocalPort == int(busyPort) || st.LocalPort == 0 {
		t.Fatalf("expected fallback to another port, got %+v", st)
	}
}

// When the preferred port is free it is used.
func TestPreferredPortUsedWhenFree(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)
	k, pub := newKey()
	env.approve("mc", pub, "d")
	env.serve()

	free, _ := net.Listen("tcp", "127.0.0.1:0")
	port := uint16(free.Addr().(*net.TCPAddr).Port)
	free.Close()

	opts := joinOpts(env.addr("mc"), games.TCP, 25565, k)
	opts.PreferPort = port
	tun, err := transport.Join(context.Background(), opts)
	if err != nil {
		t.Fatal(err)
	}
	defer tun.Stop()
	if st := tun.Status(); st.LocalPort != int(port) || st.PortFallback {
		t.Fatalf("status %+v, want port %d without fallback", st, port)
	}
}

// Disconnect must release the local port, all connections and goroutines.
func TestDisconnectReleasesResources(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)
	k, pub := newKey()
	env.approve("mc", pub, "d")
	env.serve()
	addr := env.addr("mc")

	// Let startup noise settle, then take a goroutine baseline.
	time.Sleep(500 * time.Millisecond)
	base := runtime.NumGoroutine()

	tun, err := transport.Join(context.Background(), joinOpts(addr, games.TCP, 25565, k))
	if err != nil {
		t.Fatal(err)
	}
	local := tun.Status().LocalAddr
	if got := roundTripTCP(t, local, "ping"); got != "ping" {
		t.Fatalf("echo = %q", got)
	}
	// Leave a connection open on purpose.
	open, err := net.Dial("tcp", local)
	if err != nil {
		t.Fatal(err)
	}
	defer open.Close()

	tun.Stop()

	if st := tun.Status(); st.State != transport.StateStopped || st.ActiveConns != 0 || st.LocalAddr != "" {
		t.Fatalf("after Stop: %+v", st)
	}
	// The local port is free again immediately.
	ln, err := net.Listen("tcp", local)
	if err != nil {
		t.Fatalf("local port %s was not released: %v", local, err)
	}
	ln.Close()
	// The held connection was closed by us.
	open.SetReadDeadline(time.Now().Add(3 * time.Second))
	if _, err := open.Read(make([]byte, 1)); err == nil {
		t.Fatal("open local connection survived Stop")
	}
	// Goroutines return to (about) the baseline; Stop is idempotent.
	tun.Stop()
	deadline := time.Now().Add(15 * time.Second)
	for {
		n := runtime.NumGoroutine()
		if n <= base+3 {
			break
		}
		if time.Now().After(deadline) {
			buf := make([]byte, 1<<20)
			t.Fatalf("goroutines did not drain: baseline %d, now %d\n%s", base, n, buf[:runtime.Stack(buf, true)])
		}
		time.Sleep(200 * time.Millisecond)
	}
}

// The same release guarantee for UDP.
func TestUDPDisconnectReleasesPort(t *testing.T) {
	_, tun, local := newUDPEnv(t, 34197)
	c, _ := net.DialUDP("udp", nil, local)
	c.Write([]byte("x"))
	c.SetReadDeadline(time.Now().Add(15 * time.Second))
	c.Read(make([]byte, 8))
	c.Close()
	addr := tun.Status().LocalAddr
	tun.Stop()
	pc, err := net.ListenPacket("udp", addr)
	if err != nil {
		t.Fatalf("udp port %s was not released: %v", addr, err)
	}
	pc.Close()
	if tun.Status().UDPFlows != 0 {
		t.Error("udp flows leaked")
	}
}

// If the game server restarts (new Tailcat server process with the same
// identity) the client reconnects by itself and traffic flows again.
func TestClientReconnectsAfterServerRestart(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)
	k, pub := newKey()
	env.approve("mc", pub, "d")
	env.serve()
	addr := env.addr("mc")

	var seen []transport.State
	opts := joinOpts(addr, games.TCP, 25565, k)
	opts.OnStatus = func(s transport.Status) { seen = append(seen, s.State) }
	tun, err := transport.Join(context.Background(), opts)
	if err != nil {
		t.Fatal(err)
	}
	defer tun.Stop()
	local := tun.Status().LocalAddr
	if roundTripTCP(t, local, "before") != "before" {
		t.Fatal("echo before restart failed")
	}

	// Restart the whole agent (graceful shutdown, then start again).
	env.stop()
	env.serve()
	env.addr("mc")

	deadline := time.Now().Add(60 * time.Second)
	for {
		c, err := net.DialTimeout("tcp", local, 3*time.Second)
		if err == nil {
			c.SetDeadline(time.Now().Add(8 * time.Second))
			c.Write([]byte("after"))
			b := make([]byte, 5)
			if _, err = c.Read(b); err == nil && string(b) == "after" {
				c.Close()
				break
			}
			c.Close()
		}
		if time.Now().After(deadline) {
			t.Fatalf("tunnel never recovered after the server restart: %+v", tun.Status())
		}
		time.Sleep(500 * time.Millisecond)
	}
	if tun.Status().Reconnects < 1 {
		t.Errorf("Reconnects = %d, want >= 1 (states seen: %v)", tun.Status().Reconnects, seen)
	}
}

// With direct UDP disabled (Tailscale's TS_DEBUG_ALWAYS_USE_DERP knob) traffic
// must still flow through the DERP relay, and GameLink must report the path
// as relayed rather than direct. This is the NAT-traversal-failed fallback.
func TestRelayOnlyFallbackWorksAndIsReportedAsRelayed(t *testing.T) {
	envknob.Setenv("TS_DEBUG_ALWAYS_USE_DERP", "true")
	t.Cleanup(func() { envknob.Setenv("TS_DEBUG_ALWAYS_USE_DERP", "") })

	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)
	k, pub := newKey()
	env.approve("mc", pub, "d")
	env.serve()

	tun, err := transport.Join(context.Background(), joinOpts(env.addr("mc"), games.TCP, 25565, k))
	if err != nil {
		t.Fatalf("Join over relay only: %v", err)
	}
	defer tun.Stop()
	st := tun.Status()
	if st.Path != transport.PathRelay {
		t.Fatalf("path = %q, want relay (status %+v)", st.Path, st)
	}
	t.Logf("relay-only: path=%s relay=%q latency=%.1fms", st.Path, st.Relay, st.LatencyMS)
	if got := roundTripTCP(t, st.LocalAddr, "via-derp"); got != "via-derp" {
		t.Fatalf("echo over relay = %q", got)
	}
}
