package main

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gamelink-project/gamelink/shim/gamehost"
	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/transport"
	"github.com/tailscale/tailcat"
	"tailscale.com/envknob"
	"tailscale.com/net/netcheck"
	"tailscale.com/syncs"
	"tailscale.com/tailcfg"
	"tailscale.com/tstest/integration"
	"tailscale.com/types/key"
	"tailscale.com/types/logger"
)

func TestDesktopHostAccessAndRevocation(t *testing.T) {
	envknob.Setenv("IN_TS_TEST", "true")
	netcheck.HookStartCaptivePortalDetection.SetForTest(func(context.Context, *netcheck.Client, *tailcfg.DERPMap, tailcfg.DERPRegionID, func(bool)) (<-chan struct{}, func()) {
		return syncs.ClosedChan(), func() {}
	})
	relay := integration.RunDERPAndSTUN(t, logger.Discard, "127.0.0.1").Regions[1]
	echo, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer echo.Close()
	var accepted atomic.Int64
	go func() {
		for {
			c, err := echo.Accept()
			if err != nil {
				return
			}
			accepted.Add(1)
			go func() {
				defer c.Close()
				_, _ = io.Copy(c, c)
			}()
		}
	}()
	invoke := func(request map[string]any) (any, error) {
		t.Helper()
		raw, err := json.Marshal(request)
		if err != nil {
			t.Fatal(err)
		}
		return dispatch(raw)
	}
	must := func(request map[string]any) any {
		t.Helper()
		out, err := invoke(request)
		if err != nil {
			t.Fatalf("%v: %v", request["method"], err)
		}
		return out
	}
	must(map[string]any{"method": "init"})
	defer must(map[string]any{"method": "shutdown"})
	generated := must(map[string]any{"method": "host_generate"}).(map[string]string)
	keys := gamehost.Keys{PrivateKey: generated["privateKey"], PresharedKey: generated["presharedKey"], Region: relay}
	alice, bob, stranger := key.NewNode(), key.NewNode(), key.NewNode()
	start := func(generation uint64, dest string, approved ...key.NodePublic) (any, error) {
		return invoke(map[string]any{
			"method": "host_start", "id": "game", "generation": generation,
			"keys": keys, "approved": approved,
			"game": hostedGame{Type: games.MinecraftJava, Protocol: games.TCP, TunnelPort: 25565, Dest: dest},
		})
	}
	if _, err := start(1, "192.0.2.3:25565", alice.Public()); err == nil {
		t.Fatal("accepted non-loopback destination")
	}
	if _, err := start(1, echo.Addr().String()); err != nil {
		t.Fatal(err)
	}
	if _, err := start(2, echo.Addr().String(), bob.Public()); err == nil {
		t.Fatal("replacement began before stop")
	}
	addr, err := keys.Address(true) // Only the test uses a loopback DERP.
	if err != nil {
		t.Fatal("failed to build relay address")
	}
	if string(owner.hosts[resourceID{"game", 1}].server.Addr()) != addr {
		t.Fatal("invitation address differs from the running host")
	}

	join := func(device key.NodePrivate) (*transport.Tunnel, error) {
		ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
		defer cancel()
		return transport.Join(ctx, transport.TunnelOptions{
			Identity: device, Game: games.MinecraftJava, Proto: games.TCP, TunnelPort: 25565,
			Addr: tailcat.Addr(addr), AllowLocalRelay: true, ConnectTimeout: 4 * time.Second,
			PreferPort: 25565,
		})
	}
	echoThrough := func(device key.NodePrivate) net.Conn {
		t.Helper()
		tun, err := join(device)
		if err != nil {
			t.Fatal("approved device could not join:", err)
		}
		t.Cleanup(tun.Stop)
		c, err := net.DialTimeout("tcp", tun.Status().LocalAddr, 5*time.Second)
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { c.Close() })
		c.SetDeadline(time.Now().Add(5 * time.Second))
		payload := []byte("desktop-hosted echo")
		if _, err := c.Write(payload); err != nil {
			t.Fatal(err)
		}
		got := make([]byte, len(payload))
		if _, err := io.ReadFull(c, got); err != nil || string(got) != string(payload) {
			t.Fatal("desktop host did not forward to the loopback game")
		}
		return c
	}

	noResponse := func(device key.NodePrivate) {
		t.Helper()
		tun, err := join(device)
		if tun != nil {
			tun.Stop()
		}
		if !errors.Is(err, transport.ErrNoResponse) {
			t.Fatalf("unapproved invitation holder: want no response, got %v", err)
		}
	}
	snapshot := func() hostSnapshot {
		return must(map[string]any{"method": "host_peers"}).(map[string]hostSnapshot)["game"]
	}
	expectRequest := func(peers hostSnapshot, device key.NodePublic) {
		t.Helper()
		if len(peers.Requests) != 1 || peers.Requests[0].PublicKey != device ||
			peers.Requests[0].FirstSeen.IsZero() || peers.Requests[0].Attempts == 0 {
			t.Fatal("missing refusal batch for the retrying device")
		}
	}
	noResponse(alice)
	knocks := snapshot()
	expectRequest(knocks, alice.Public())
	if len(knocks.Connected) != 0 || accepted.Load() != 0 {
		t.Fatal("knock-only mode reached the game port")
	}
	if len(snapshot().Requests) != 0 {
		t.Fatal("request batch was delivered twice")
	}
	must(map[string]any{"method": "host_stop", "id": "game", "generation": 1})
	if _, err := start(2, echo.Addr().String(), alice.Public(), bob.Public()); err != nil {
		t.Fatal(err)
	}
	aliceConn := echoThrough(alice)
	noResponse(stranger)
	peers := snapshot()
	expectRequest(peers, stranger.Public())
	if peers.Connected[alice.Public().String()].IsZero() || !peers.Connected[stranger.Public().String()].IsZero() {
		t.Fatal("incorrect authenticated gameplay timestamps")
	}
	must(map[string]any{"method": "host_stop", "id": "game", "generation": 2})
	aliceConn.SetReadDeadline(time.Now().Add(5 * time.Second))
	if _, err := aliceConn.Read(make([]byte, 1)); err == nil {
		t.Fatal("revoked device's old session is still open")
	} else if e, ok := err.(net.Error); ok && e.Timeout() {
		t.Fatal("revocation did not terminate the open session")
	}
	if _, err := start(3, echo.Addr().String(), bob.Public()); err != nil {
		t.Fatal(err)
	}
	must(map[string]any{"method": "host_stop", "id": "game", "generation": 2}) // stale stop
	noResponse(alice)
	echoThrough(bob).Close()
}
