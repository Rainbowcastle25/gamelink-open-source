//go:build external

package integration

// These tests use the real public Tailcat relay network (tailcat.dev DERP and
// STUN) over the real internet. They are excluded from the default test run:
//
//	go test -tags external -run TestPublicRelay -v ./tests/integration

import (
	"bytes"
	"context"
	"net"
	"testing"
	"time"

	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/transport"
	"github.com/tailscale/tailcat"
)

func TestPublicRelayEndToEnd(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 40*time.Second)
	defer cancel()
	region, err := transport.ResolveRegion(ctx, transport.RegionSpec{})
	if err != nil {
		t.Fatalf("ResolveRegion (public DERP map): %v", err)
	}
	t.Logf("public relay region reachable: %d (%s)", region.RegionID, region.RegionName)

	tcp := tcpEcho(t)
	udp := udpEcho(t)
	// A nil region makes the shipped agent fetch the public relay map and pick
	// the nearest relay itself, exactly as an operator's `games add` does.
	env := newAgentEnv(t, nil)
	env.addGame("mc", games.MinecraftJava, games.TCP, 25565, tcp)
	env.addGame("fx", games.Factorio, games.UDP, 34197, udp)
	_ = region // the resolved public region is only logged above

	// Strict (production) invitation validation, i.e. NOT AllowLocalRelay: the
	// agent's `invite` and invite.Parse are both run without the local-relay opt-in.
	devKey, devPub := newKey()
	env.approve("mc", devPub, "me")
	env.approve("fx", devPub, "me")
	env.serve()
	env.addr("mc")
	env.addr("fx")
	mkOpts := func(id string, proto games.Protocol) transport.TunnelOptions {
		inv := env.invite(id)
		opts := joinOpts(tailcat.Addr(inv.Addr), proto, inv.Port, devKey)
		opts.AllowLocalRelay = false
		opts.ConnectTimeout = 60 * time.Second
		return opts
	}

	// TCP.
	ttun, err := transport.Join(context.Background(), mkOpts("mc", games.TCP))
	if err != nil {
		t.Fatal(err)
	}
	defer ttun.Stop()
	cv := ttun.Status()
	t.Logf("TCP over public relay: addr=%s path=%s relay=%q latency=%.1fms", cv.LocalAddr, cv.Path, cv.Relay, cv.LatencyMS)
	if got := roundTripTCP(t, cv.LocalAddr, "over the real internet"); got != "over the real internet" {
		t.Fatalf("echo %q", got)
	}

	// UDP.
	utun, err := transport.Join(context.Background(), mkOpts("fx", games.UDP))
	if err != nil {
		t.Fatal(err)
	}
	defer utun.Stop()
	ucv := utun.Status()
	t.Logf("UDP over public relay: addr=%s path=%s relay=%q", ucv.LocalAddr, ucv.Path, ucv.Relay)
	ua, _ := net.ResolveUDPAddr("udp", ucv.LocalAddr)
	c, err := net.DialUDP("udp", nil, ua)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	want := bytes.Repeat([]byte("U"), transport.MaxUDPPayload)
	c.Write(want)
	c.SetReadDeadline(time.Now().Add(20 * time.Second))
	buf := make([]byte, 4096)
	n, err := c.Read(buf)
	if err != nil || !bytes.Equal(buf[:n], want) {
		t.Fatalf("UDP echo over public relay: n=%d err=%v", n, err)
	}

	// A stranger with the full address still gets nothing.
	stranger, _ := newKey()
	opts := mkOpts("mc", games.TCP)
	opts.Identity = stranger
	opts.ConnectTimeout = 6 * time.Second
	if tun, err := transport.Join(context.Background(), opts); err == nil {
		tun.Stop()
		t.Fatal("stranger joined over the public relay")
	}
}
