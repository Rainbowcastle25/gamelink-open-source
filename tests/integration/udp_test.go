package integration

import (
	"bytes"
	"context"
	"net"
	"testing"
	"time"

	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/transport"
)

func joinUDP(t *testing.T, env *agentEnv, id string, port uint16) (*transport.Tunnel, *net.UDPAddr) {
	t.Helper()
	k, pub := newKey()
	env.approve(id, pub, "udp-dev")
	tun, err := transport.Join(context.Background(), joinOpts(env.addr(id), games.UDP, port, k))
	if err != nil {
		t.Fatalf("Join: %v", err)
	}
	t.Cleanup(tun.Stop)
	ua, err := net.ResolveUDPAddr("udp", tun.Status().LocalAddr)
	if err != nil {
		t.Fatal(err)
	}
	return tun, ua
}

func newUDPEnv(t *testing.T, port uint16) (*agentEnv, *transport.Tunnel, *net.UDPAddr) {
	t.Helper()
	relay := startRelay(t)
	echo := udpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("fx", games.Factorio, games.UDP, port, echo)
	// Approve before serving so the server starts with the device.
	k, pub := newKey()
	env.approve("fx", pub, "udp-dev")
	env.serve()
	tun, err := transport.Join(context.Background(), joinOpts(env.addr("fx"), games.UDP, port, k))
	if err != nil {
		t.Fatalf("Join: %v", err)
	}
	t.Cleanup(tun.Stop)
	ua, err := net.ResolveUDPAddr("udp", tun.Status().LocalAddr)
	if err != nil {
		t.Fatal(err)
	}
	return env, tun, ua
}

func TestRustAgentSubprocessApprovalKeepsExistingUDPFlow(t *testing.T) {
	env, _, local := newUDPEnv(t, 34197)
	alice, err := net.DialUDP("udp", nil, local)
	if err != nil {
		t.Fatal(err)
	}
	defer alice.Close()
	echo := func(payload string) {
		t.Helper()
		alice.SetDeadline(time.Now().Add(5 * time.Second))
		if _, err := alice.Write([]byte(payload)); err != nil {
			t.Fatal(err)
		}
		got := make([]byte, 64)
		if n, err := alice.Read(got); err != nil || string(got[:n]) != payload {
			t.Fatalf("UDP echo %q: got %q, err = %v", payload, got[:n], err)
		}
	}
	echo("alice before approval")
	joinUDP(t, env, "fx", 34197)
	// Check before sending more traffic: a rebuilt flow would also echo.
	if g, ok := env.gameState("fx"); !ok || g.Stats.Active != 1 {
		t.Fatal("the original UDP flow did not survive approval")
	}
	echo("alice after approval")
}

// UDP echo through the real tunnel, with datagram boundaries preserved: every
// datagram comes back as exactly one datagram with identical content.
func TestUDPEchoPreservesDatagramBoundaries(t *testing.T) {
	_, tun, local := newUDPEnv(t, 34197)

	c, err := net.DialUDP("udp", nil, local)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()

	sizes := []int{1, 2, 17, 100, 512, 1000, 1200, transport.MaxUDPPayload}
	for _, n := range sizes {
		want := bytes.Repeat([]byte{byte(n)}, n)
		want[0] = byte(n >> 8) // make the head distinctive
		if _, err := c.Write(want); err != nil {
			t.Fatalf("write %d: %v", n, err)
		}
		c.SetReadDeadline(time.Now().Add(15 * time.Second))
		got := make([]byte, 4096)
		m, err := c.Read(got)
		if err != nil {
			t.Fatalf("read reply for %d-byte datagram: %v", n, err)
		}
		if !bytes.Equal(got[:m], want) {
			t.Fatalf("datagram of %d bytes came back as %d different bytes", n, m)
		}
	}

	// Send three datagrams back-to-back; they must come back as three separate
	// datagrams, not one merged stream.
	msgs := [][]byte{[]byte("one"), []byte("second-datagram"), []byte("3")}
	for _, m := range msgs {
		c.Write(m)
	}
	seen := map[string]bool{}
	for range msgs {
		c.SetReadDeadline(time.Now().Add(15 * time.Second))
		buf := make([]byte, 4096)
		n, err := c.Read(buf)
		if err != nil {
			t.Fatal(err)
		}
		seen[string(buf[:n])] = true
	}
	for _, m := range msgs {
		if !seen[string(m)] {
			t.Fatalf("datagram %q was lost or merged; got %v", m, seen)
		}
	}
	if st := tun.Status(); st.OversizeDrops != 0 {
		t.Errorf("unexpected oversize drops: %d", st.OversizeDrops)
	}
}

// Two local endpoints must be tracked as separate flows and replies must go to
// the right one.
func TestUDPMultipleLocalEndpointsGetOwnReplies(t *testing.T) {
	_, tun, local := newUDPEnv(t, 34197)
	a, _ := net.DialUDP("udp", nil, local)
	b, _ := net.DialUDP("udp", nil, local)
	defer a.Close()
	defer b.Close()
	a.Write([]byte("from-A"))
	b.Write([]byte("from-B"))
	for name, c := range map[string]*net.UDPConn{"A": a, "B": b} {
		c.SetReadDeadline(time.Now().Add(15 * time.Second))
		buf := make([]byte, 64)
		n, err := c.Read(buf)
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if string(buf[:n]) != "from-"+name {
			t.Fatalf("endpoint %s received %q (reply routed to the wrong flow)", name, buf[:n])
		}
	}
	if f := tun.Status().UDPFlows; f != 2 {
		t.Errorf("UDPFlows = %d, want 2", f)
	}
}

// Datagrams larger than Tailcat's limit are dropped and reported, never
// truncated or fragmented.
func TestUDPOversizeIsDroppedAndReportedNotTruncated(t *testing.T) {
	_, tun, local := newUDPEnv(t, 34197)
	c, _ := net.DialUDP("udp", nil, local)
	defer c.Close()

	c.Write(bytes.Repeat([]byte("Z"), transport.MaxUDPPayload+1))
	c.SetReadDeadline(time.Now().Add(1500 * time.Millisecond))
	buf := make([]byte, 4096)
	if n, err := c.Read(buf); err == nil {
		t.Fatalf("oversize datagram produced a %d-byte reply (truncated/forwarded?)", n)
	}
	st := tun.Status()
	if st.OversizeDrops != 1 {
		t.Fatalf("OversizeDrops = %d, want 1", st.OversizeDrops)
	}
	if st.Message == "" || st.State != transport.StateConnected {
		t.Errorf("status = %+v", st)
	}

	// A normal-sized packet still works afterwards.
	c.Write([]byte("still-ok"))
	c.SetReadDeadline(time.Now().Add(15 * time.Second))
	n, err := c.Read(buf)
	if err != nil || string(buf[:n]) != "still-ok" {
		t.Fatalf("after oversize drop got %q, %v", buf[:n], err)
	}
}

// An unapproved key cannot use a UDP game either.
func TestUDPUnauthorizedKeyCannotConnect(t *testing.T) {
	env, _, _ := newUDPEnv(t, 34197)
	stranger, _ := newKey()
	opts := joinOpts(env.addr("fx"), games.UDP, 34197, stranger)
	opts.ConnectTimeout = 4 * time.Second
	if tun, err := transport.Join(context.Background(), opts); err == nil {
		tun.Stop()
		t.Fatal("unauthorized key joined a UDP game")
	}
}
