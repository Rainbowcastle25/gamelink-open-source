package integration

import (
	"context"
	"errors"
	"io"
	"net"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/types/key"

	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/transport"
)

// An approved device can reach the game through the full production path:
// Rust agent CLI/serve -> Tailcat -> client Join -> loopback listener.
func TestTCPEchoThroughTailcat(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)

	devKey, devPub := newKey()
	env.approve("mc", devPub, "alice")
	env.serve()

	opts := joinOpts(env.addr("mc"), games.TCP, 25565, devKey)
	tun, err := transport.Join(context.Background(), opts)
	if err != nil {
		t.Fatalf("Join: %v", err)
	}
	defer tun.Stop()

	st := tun.Status()
	if st.State != transport.StateConnected {
		t.Fatalf("state = %s (%s), want connected", st.State, st.Message)
	}
	if !strings.HasPrefix(st.LocalAddr, "127.0.0.1:") {
		t.Fatalf("local address %q is not loopback", st.LocalAddr)
	}
	if st.Path == transport.PathUnknown || !st.LatencyKnown {
		t.Errorf("path/latency were not measured: %+v", st)
	}
	// A silent TCP echo server cannot be distinguished from a pending
	// upstream dial without speaking its application protocol.
	if st.GameReachable != "unknown" {
		t.Errorf("GameReachable = %q, want unknown", st.GameReachable)
	}
	t.Logf("connected: addr=%s path=%s relay=%q latency=%.1fms", st.LocalAddr, st.Path, st.Relay, st.LatencyMS)

	// Several simultaneous connections, each with its own payload.
	done := make(chan error, 8)
	for i := 0; i < 8; i++ {
		go func() {
			msg := strings.Repeat("minecraft-handshake-", 400) // 8 KB, crosses several segments
			c, err := net.Dial("tcp", st.LocalAddr)
			if err != nil {
				done <- err
				return
			}
			defer c.Close()
			c.SetDeadline(time.Now().Add(20 * time.Second))
			go io.WriteString(c, msg)
			buf := make([]byte, len(msg))
			if _, err := io.ReadFull(c, buf); err != nil {
				done <- err
				return
			}
			if string(buf) != msg {
				done <- errors.New("echo mismatch")
				return
			}
			done <- nil
		}()
	}
	for i := 0; i < 8; i++ {
		if err := <-done; err != nil {
			t.Fatalf("concurrent connection %d: %v", i, err)
		}
	}
	if s := tun.Status(); s.BytesToServer == 0 || s.BytesFromServer == 0 {
		t.Errorf("byte counters not updated: %+v", s)
	}
}

// TCP half-close must be preserved: a client that sends a request and then
// closes its write side must still receive the full response.
func TestTCPHalfClosePreserved(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("g", games.Custom, games.TCP, 7000, echo)
	devKey, devPub := newKey()
	env.approve("g", devPub, "d")
	env.serve()

	tun, err := transport.Join(context.Background(), joinOpts(env.addr("g"), games.TCP, 7000, devKey))
	if err != nil {
		t.Fatal(err)
	}
	defer tun.Stop()

	c, err := net.Dial("tcp", tun.Status().LocalAddr)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	c.SetDeadline(time.Now().Add(15 * time.Second))
	io.WriteString(c, "request-then-fin")
	c.(*net.TCPConn).CloseWrite()
	got, err := io.ReadAll(c)
	if err != nil || string(got) != "request-then-fin" {
		t.Fatalf("after half-close got %q, %v", got, err)
	}
}

// A key the server never approved must not get through even when it holds the
// complete, correct Tailcat address (which includes the pre-shared key).
func TestUnauthorizedKeyCannotConnectEvenWithAddress(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)
	_, okPub := newKey()
	env.approve("mc", okPub, "approved-friend")
	env.serve()
	addr := env.addr("mc") // the attacker knows the full address

	stranger, _ := newKey()
	opts := joinOpts(addr, games.TCP, 25565, stranger)
	opts.ConnectTimeout = 4 * time.Second
	start := time.Now()
	tun, err := transport.Join(context.Background(), opts)
	if err == nil {
		tun.Stop()
		t.Fatal("unauthorized key joined successfully")
	}
	if !errors.Is(err, transport.ErrNoResponse) {
		t.Fatalf("error = %v, want ErrNoResponse", err)
	}
	t.Logf("stranger rejected after %v: %v", time.Since(start).Round(time.Millisecond), err)

	// Also try raw Tailcat, bypassing GameLink's client entirely.
	cl := &tailcat.Client{Server: addr, Key: stranger, Logf: quietLogf(t)}
	defer cl.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if c, err := cl.DialTCPPort(ctx, 25565); err == nil {
		c.Close()
		t.Fatal("raw tailcat dial from unapproved key succeeded")
	}
	// The agent publishes stats on a heartbeat; read one published after both
	// attempts have finished, so it reflects them.
	if g := env.statsAfter("mc", time.Now()); g.Stats.Accepted != 0 {
		t.Fatalf("server accepted %d sessions from nobody", g.Stats.Accepted)
	}
}

// A game with zero approved devices must not be reachable at all, and
// GameLink must refuse to even start a Tailcat server for it (Tailcat's own
// empty allowlist means "everyone").
func TestEmptyAllowlistNeverExposesGame(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)

	// Layer 1: the transport refuses to start without approved devices.
	priv := tailcat.NewPrivateKey()
	_, err := transport.StartGameServer(transport.ServerOptions{
		ID: "x", Protocol: games.TCP, TunnelPort: 25565, Dest: echo,
		Private: priv.Private, PSK: priv.Public.PresharedKey, Region: relay,
		Approved: nil,
	})
	if !errors.Is(err, transport.ErrNoApprovedDevices) {
		t.Fatalf("StartGameServer with no approved devices = %v, want ErrNoApprovedDevices", err)
	}

	// Layer 2: through the Rust agent a configured game with no devices stays
	// inactive and is not served.
	env := newAgentEnv(t, relay)
	g := env.addGame("empty", games.MinecraftJava, games.TCP, 25565, echo)
	env.serve()
	env.waitState("empty", "inactive")
	if env.isServing("empty") {
		t.Fatal("agent is serving a game with no approved devices")
	}

	// Layer 3: even someone holding the address computed from the persistent
	// keys cannot get an answer, because nothing is listening on the relay.
	keys := env.keys(g.ID)
	anyone, _ := newKey()
	opts := joinOpts(keys.Addr(), games.TCP, 25565, anyone)
	opts.ConnectTimeout = 3 * time.Second
	if tun, err := transport.Join(context.Background(), opts); err == nil {
		tun.Stop()
		t.Fatal("joined a game that has no approved devices")
	}
}

func TestRustAgentSubprocessApprovalKeepsExistingTCPSession(t *testing.T) {
	env := newAgentEnv(t, startRelay(t))
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, tcpEcho(t))
	alice, alicePub := newKey()
	bob, bobPub := newKey()
	env.approve("mc", alicePub, "alice")
	env.serve()
	addr := env.addr("mc")

	connect := func(device key.NodePrivate) net.Conn {
		t.Helper()
		opts := joinOpts(addr, games.TCP, 25565, device)
		opts.AutoReconnect = false
		tun, err := transport.Join(context.Background(), opts)
		if err != nil {
			t.Fatalf("approved device could not join: %v", err)
		}
		t.Cleanup(tun.Stop)
		c, err := net.DialTimeout("tcp", tun.Status().LocalAddr, 5*time.Second)
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { c.Close() })
		return c
	}
	echo := func(c net.Conn, payload string) {
		t.Helper()
		c.SetDeadline(time.Now().Add(5 * time.Second))
		if _, err := io.WriteString(c, payload); err != nil {
			t.Fatalf("write %q: %v", payload, err)
		}
		got := make([]byte, len(payload))
		if _, err := io.ReadFull(c, got); err != nil || string(got) != payload {
			t.Fatalf("echo %q: got %q, err = %v", payload, got, err)
		}
	}
	live := connect(alice)
	echo(live, "alice before approval")

	env.approve("mc", bobPub, "bob")
	if env.addr("mc") != addr {
		t.Fatal("approval changed the game's Tailcat address")
	}
	env.waitApproved("mc", 2)
	echo(live, "alice after approval on the same connection")
	bobConn := connect(bob)
	echo(bobConn, "newly approved bob")

	env.revoke("mc", bobPub)
	bobConn.SetReadDeadline(time.Now().Add(20 * time.Second))
	if _, err := bobConn.Read(make([]byte, 1)); err == nil || errors.Is(err, os.ErrDeadlineExceeded) {
		t.Fatalf("Bob's TCP session was still open after revocation: %v", err)
	}
	env.waitApproved("mc", 1)

	opts := joinOpts(addr, games.TCP, 25565, bob)
	opts.ConnectTimeout = 4 * time.Second
	if tun, err := transport.Join(context.Background(), opts); err == nil {
		tun.Stop()
		t.Fatal("revoked Bob was able to rejoin the TCP game")
	} else if !errors.Is(err, transport.ErrNoResponse) {
		t.Fatalf("revoked Bob's TCP join: got %v, want ErrNoResponse", err)
	}
}

// Revoking a device must end its live session and block new ones, while the
// remaining approved devices can reconnect after the restart.
func TestRevokedDeviceLosesAccess(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)
	aliceKey, alicePub := newKey()
	bobKey, bobPub := newKey()
	env.approve("mc", alicePub, "alice")
	env.approve("mc", bobPub, "bob")
	env.serve()
	addr := env.addr("mc")

	alice, err := transport.Join(context.Background(), joinOpts(addr, games.TCP, 25565, aliceKey))
	if err != nil {
		t.Fatalf("alice join: %v", err)
	}
	defer alice.Stop()
	bob, err := transport.Join(context.Background(), joinOpts(addr, games.TCP, 25565, bobKey))
	if err != nil {
		t.Fatalf("bob join: %v", err)
	}
	defer bob.Stop()

	// Alice holds a live session open.
	live, err := net.Dial("tcp", alice.Status().LocalAddr)
	if err != nil {
		t.Fatal(err)
	}
	defer live.Close()
	live.SetDeadline(time.Now().Add(30 * time.Second))
	io.WriteString(live, "hello")
	buf := make([]byte, 5)
	if _, err := io.ReadFull(live, buf); err != nil {
		t.Fatalf("pre-revoke echo: %v", err)
	}

	// The admin revokes alice with the shipped CLI; the running agent notices
	// the change on its next poll.
	env.revoke("mc", alicePub)

	// Alice's already-open session must be terminated, not left running.
	// The read must end with EOF/reset, not a timeout: a timeout would mean the
	// session is still open and merely idle.
	live.SetReadDeadline(time.Now().Add(20 * time.Second))
	for {
		_, err := live.Read(buf)
		if err == nil {
			continue
		}
		if errors.Is(err, os.ErrDeadlineExceeded) {
			t.Fatal("alice's session was still open 20s after revocation")
		}
		t.Logf("alice's live session ended after revocation: %v", err)
		break
	}

	// Alice cannot establish anything new, even with the (unchanged) address.
	opts := joinOpts(addr, games.TCP, 25565, aliceKey)
	opts.ConnectTimeout = 4 * time.Second
	if tun, err := transport.Join(context.Background(), opts); err == nil {
		tun.Stop()
		t.Fatal("revoked device was able to join again")
	}

	// Bob is unaffected: his tunnel recovers from the server restart on its
	// own and traffic flows again.
	deadline := time.Now().Add(40 * time.Second)
	for {
		c, err := net.DialTimeout("tcp", bob.Status().LocalAddr, 3*time.Second)
		if err == nil {
			c.SetDeadline(time.Now().Add(8 * time.Second))
			io.WriteString(c, "bob-ok")
			b := make([]byte, 6)
			if _, err = io.ReadFull(c, b); err == nil && string(b) == "bob-ok" {
				c.Close()
				break
			}
			c.Close()
		}
		if time.Now().After(deadline) {
			t.Fatalf("bob never recovered after the revocation restart: status %+v", bob.Status())
		}
		time.Sleep(500 * time.Millisecond)
	}
	if bob.Status().State != transport.StateConnected {
		t.Errorf("bob state = %s", bob.Status().State)
	}
}

// If the last approved device is revoked the game becomes unreachable.
func TestRevokingLastDeviceStopsServing(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)
	k, pub := newKey()
	env.approve("mc", pub, "only")
	env.serve()
	addr := env.addr("mc")

	tun, err := transport.Join(context.Background(), joinOpts(addr, games.TCP, 25565, k))
	if err != nil {
		t.Fatal(err)
	}
	defer tun.Stop()
	env.revoke("mc", pub)
	env.waitState("mc", "inactive")
	if env.isServing("mc") {
		t.Fatal("game still served after its last device was revoked")
	}
}

// The tunnel's reach is limited to the one served port: other ports are not
// answered and the server is not an exit node.
func TestServerIsNotAnExitNodeAndServesOnlyItsPort(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("g", games.Custom, games.TCP, 9000, echo)
	k, pub := newKey()
	env.approve("g", pub, "d")
	env.serve()
	addr := env.addr("g")

	cl := &tailcat.Client{Server: addr, Key: k, Logf: quietLogf(t)}
	defer cl.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	if _, err := cl.Ping(ctx); err != nil {
		t.Fatal(err)
	}

	// The real port works.
	c, err := cl.DialTCPPort(ctx, 9000)
	if err != nil {
		t.Fatalf("served port: %v", err)
	}
	c.Close()

	// Any other port on the server is refused.
	for _, p := range []uint16{22, 80, 25565, 9001} {
		dctx, dcancel := context.WithTimeout(ctx, 2*time.Second)
		if c, err := cl.DialTCPPort(dctx, p); err == nil {
			c.Close()
			t.Fatalf("unserved port %d accepted a connection", p)
		}
		dcancel()
	}
	// Arbitrary IP destinations (exit node / LAN access) are refused.
	for _, dst := range []string{echo.String(), "192.168.1.1:80", "8.8.8.8:53", "127.0.0.1:22"} {
		dctx, dcancel := context.WithTimeout(ctx, 2*time.Second)
		ap, _ := parseAP(dst)
		if c, err := cl.DialTCP(dctx, ap); err == nil {
			c.Close()
			t.Fatalf("server relayed a connection to %s (exit-node behaviour)", dst)
		}
		dcancel()
	}
	// A TCP game does not answer UDP.
	pc, err := cl.DialUDPPort(ctx, 9000)
	if err == nil {
		defer pc.Close()
		pc.Write([]byte("ping"))
		pc.SetReadDeadline(time.Now().Add(1500 * time.Millisecond))
		if n, err := pc.Read(make([]byte, 16)); err == nil {
			t.Fatalf("TCP-only game answered a UDP packet (%d bytes)", n)
		}
	}
}

// The peer address derivation GameLink uses for its second authorisation layer
// must match what Tailcat really reports; if upstream ever changes it,
// approved clients would fail closed and this test says why.
func TestPeerAddrMatchesTailcat(t *testing.T) {
	relay := startRelay(t)
	got := make(chan net.Addr, 1)
	srvKey := key.NewNode()
	s := &tailcat.Server{
		Key: srvKey, Region: relay, Logf: quietLogf(t),
		AllowedClients: []key.NodePublic{key.NewNode().Public()},
		OnTCP: func(port uint16) func(net.Conn) {
			return func(c net.Conn) {
				got <- c.RemoteAddr()
				c.Close()
			}
		},
	}
	clientKey := key.NewNode()
	s.AddAllowedClient(clientKey.Public())
	if err := s.Start(); err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	cl := &tailcat.Client{Server: s.TailcatAddr(), Key: clientKey, Logf: quietLogf(t)}
	defer cl.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	c, err := cl.DialTCPPort(ctx, 1234)
	if err != nil {
		t.Fatal(err)
	}
	c.Close()
	select {
	case ra := <-got:
		ap, _ := parseAP(ra.String())
		if ap.Addr() != transport.PeerAddr(clientKey.Public()) {
			t.Fatalf("remote %v != PeerAddr %v", ap.Addr(), transport.PeerAddr(clientKey.Public()))
		}
	case <-time.After(10 * time.Second):
		t.Fatal("handler never ran")
	}
}

// The address derived offline from the saved keys (Go side) must equal the one
// the shipped agent prints in its invitation, be accepted by a real client and
// reach the live server.
func TestOfflineAddressMatchesLiveServer(t *testing.T) {
	relay := startRelay(t)
	echo := tcpEcho(t)
	env := newAgentEnv(t, relay)
	g := env.addGame("mc", games.MinecraftJava, games.TCP, 25565, echo)
	k, pub := newKey()
	env.approve("mc", pub, "d")
	env.serve()
	live := env.addr("mc")
	keys := env.keys(g.ID)
	if keys.Addr() != live {
		t.Fatalf("address derived from the saved keys differs from the shipped agent's invitation address")
	}
	tun, err := transport.Join(context.Background(), joinOpts(keys.Addr(), games.TCP, 25565, k))
	if err != nil {
		t.Fatal(err)
	}
	tun.Stop()
}
