package integration

import (
	"context"
	"errors"
	"net"
	"strings"
	"testing"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/types/key"

	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/invite"
	"github.com/gamelink-project/gamelink/shim/transport"
)

// The desktop application is Rust (apps/desktop-tauri, core/crates/gamelink-core)
// over the Go shim in shim/desktop, whose `join` and `check` operations are thin
// wrappers around transport.Join and transport.CheckWithRelayPolicy. These tests
// drive that exact shim layer with the options the shipped desktop host sends
// (see desktopOpts), against the shipped Rust agent, using the invitation the
// shipped agent CLI prints. Lifecycle/state-machine policy, persistence and
// settings are Rust-owned and covered by Rust tests; see
// docs/test-migration-map.md.

func waitTunnel(t *testing.T, tun *transport.Tunnel, state transport.State, d time.Duration) transport.Status {
	t.Helper()
	deadline := time.Now().Add(d)
	var last transport.Status
	for time.Now().Before(deadline) {
		last = tun.Status()
		if last.State == state {
			return last
		}
		time.Sleep(100 * time.Millisecond)
	}
	t.Fatalf("tunnel never reached %q; last: %+v", state, last)
	return last
}

// desktopCheck runs the reachability check as shim/desktop's `check` does:
// a 20 s budget and the same relay policy.
func desktopCheck(addr tailcat.Addr, id key.NodePrivate) (time.Duration, transport.Path, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	return transport.CheckWithRelayPolicy(ctx, addr, id, nil, true)
}

func portFree(t *testing.T, addr string) {
	t.Helper()
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		t.Fatalf("local port %s was not released: %v", addr, err)
	}
	ln.Close()
}

// The whole user journey the alpha promises at the transport layer: take the
// invitation the agent prints, check reachability, join, use the tunnel,
// disconnect (port released), and join again with the same device identity.
func TestDesktopJourneyInviteCheckJoinDisconnectRejoin(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)

	// The device identity exactly as the desktop shim generates one.
	devKey := key.NewNode()
	pubKey := devKey.Public().String()
	if !strings.HasPrefix(pubKey, "nodekey:") {
		t.Fatalf("public key %q", pubKey)
	}
	env.approve("mc", pubKey, "friend")
	env.serve()
	env.addr("mc")

	// The invitation from the shipped CLI carries the game's metadata.
	inv := env.invite("mc")
	if inv.Name != "mc" || inv.Port != 25565 || inv.Game != games.MinecraftJava || inv.Proto != games.TCP {
		// Never format the whole invitation: Addr carries the pre-shared key.
		t.Fatalf("invitation fields: name=%q port=%d game=%v proto=%v", inv.Name, inv.Port, inv.Game, inv.Proto)
	}
	if _, _, err := invite.Parse("glink1.garbage", invite.Options{AllowLocalRelay: true}); err == nil {
		t.Fatal("garbage invitation parsed")
	}
	addr := tailcat.Addr(inv.Addr)

	// Check reports online with measured latency and path for an approved device.
	latency, path, err := desktopCheck(addr, devKey)
	if err != nil || latency <= 0 || path == transport.PathUnknown {
		t.Fatalf("Check = %v, %q, %v", latency, path, err)
	}

	// Join reports connected only once verified, with a real local address.
	tun, err := transport.Join(context.Background(), desktopOpts(addr, games.TCP, 25565, devKey, true))
	if err != nil {
		t.Fatal(err)
	}
	defer tun.Stop()
	st := tun.Status()
	if st.State != transport.StateConnected || !strings.HasPrefix(st.LocalAddr, "127.0.0.1:") || !st.LatencyKnown || st.Path == transport.PathUnknown {
		t.Fatalf("connected status lacks verified data: %+v", st)
	}
	if got := roundTripTCP(t, st.LocalAddr, "hello gamelink"); got != "hello gamelink" {
		t.Fatalf("echo %q", got)
	}

	// Disconnect releases the port.
	local := st.LocalAddr
	tun.Stop()
	if got := tun.Status().State; got != transport.StateStopped {
		t.Fatalf("after Stop state %q", got)
	}
	portFree(t, local)

	// The same identity joins again (the desktop persists it across restarts).
	again, err := transport.Join(context.Background(), desktopOpts(addr, games.TCP, 25565, devKey, true))
	if err != nil {
		t.Fatalf("rejoin: %v", err)
	}
	defer again.Stop()
	if got := roundTripTCP(t, again.Status().LocalAddr, "again"); got != "again" {
		t.Fatalf("echo after rejoin %q", got)
	}
}

// A device that was never approved ends in an honest "unreachable" outcome
// that explains the likely cause, never a fake connected one.
func TestDesktopUnapprovedDeviceIsUnreachable(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)
	_, otherPub := newKey()
	env.approve("mc", otherPub, "someone-else")
	env.serve()
	addr := env.addr("mc")

	mine := key.NewNode()
	tun, err := transport.Join(context.Background(), desktopOpts(addr, games.TCP, 25565, mine, true))
	if err == nil {
		tun.Stop()
		t.Fatal("an unapproved device joined")
	}
	if !errors.Is(err, transport.ErrNoResponse) {
		t.Fatalf("error = %v, want ErrNoResponse", err)
	}
	if tun != nil {
		t.Fatal("a failed join must not hand out a tunnel (and so no local address)")
	}
	if !strings.Contains(strings.ToLower(err.Error()), "approved") {
		t.Errorf("message should explain the likely cause: %q", err)
	}
	// The reachability check agrees.
	if _, _, err := desktopCheck(addr, mine); !errors.Is(err, transport.ErrNoResponse) {
		t.Fatalf("Check error = %v, want ErrNoResponse", err)
	}
}

// When a connection is lost the UI shows Retry, which joins again. With
// auto-reconnect off, the lost tunnel must hand its local port back at once,
// and a new Join must succeed once the server is back.
func TestDesktopRetryAfterConnectionLost(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)
	devKey, pub := newKey()
	env.approve("mc", pub, "me")
	env.serve()
	addr := env.addr("mc")

	tun, err := transport.Join(context.Background(), desktopOpts(addr, games.TCP, 25565, devKey, false))
	if err != nil {
		t.Fatal(err)
	}
	defer tun.Stop()
	local := tun.Status().LocalAddr

	env.stop()
	waitTunnel(t, tun, transport.StateError, 90*time.Second)
	portFree(t, local) // handed back without Stop

	// The desktop stops the dead tunnel before replacing it.
	tun.Stop()
	env.serve()
	addr = env.addr("mc")
	retry, err := transport.Join(context.Background(), desktopOpts(addr, games.TCP, 25565, devKey, false))
	if err != nil {
		t.Fatalf("Retry after a lost connection failed: %v", err)
	}
	defer retry.Stop()
	if got := roundTripTCP(t, retry.Status().LocalAddr, "again"); got != "again" {
		t.Fatalf("echo after retry %q", got)
	}
}

// The default settings: auto-reconnect is on, so after the server goes away
// the tunnel keeps retrying in the background and ends "unreachable". Retry
// replaces that tunnel, ends honestly "unreachable" again while the server is
// still down, and connects once it is back.
func TestDesktopRetryWhileAutoReconnecting(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)
	devKey, pub := newKey()
	env.approve("mc", pub, "me")
	env.serve()
	addr := env.addr("mc")

	tun, err := transport.Join(context.Background(), desktopOpts(addr, games.TCP, 25565, devKey, true))
	if err != nil {
		t.Fatal(err)
	}
	defer tun.Stop()

	env.stop()
	waitTunnel(t, tun, transport.StateUnreachable, 150*time.Second)

	// Retry while the server is still down: the old tunnel is replaced and the
	// new attempt ends in an honest "no response" again.
	tun.Stop()
	if again, err := transport.Join(context.Background(), desktopOpts(addr, games.TCP, 25565, devKey, true)); err == nil {
		again.Stop()
		t.Fatal("joined a server that is down")
	} else if !errors.Is(err, transport.ErrNoResponse) {
		t.Fatalf("retry error = %v, want ErrNoResponse", err)
	}

	env.serve()
	addr = env.addr("mc")
	retry, err := transport.Join(context.Background(), desktopOpts(addr, games.TCP, 25565, devKey, true))
	if err != nil {
		t.Fatalf("Retry after the server came back failed: %v", err)
	}
	defer retry.Stop()
	if got := roundTripTCP(t, retry.Status().LocalAddr, "back"); got != "back" {
		t.Fatalf("echo after retry %q", got)
	}
}
