package integration

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/netip"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gamelink-project/gamelink/shim/consolebridge"
	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/transport"
)

// Uses the shipped Rust host and real encrypted Tailcat path. The BDS endpoint
// is synthetic: this proves forwarding and health, not a real Minecraft join.
func TestDesktopBedrockSignalingMediaAndGameHealth(t *testing.T) {
	var online atomic.Bool
	online.Store(true)
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !online.Load() {
			http.Error(w, "offline", 503)
			return
		}
		if r.Method == http.MethodGet {
			_, _ = io.WriteString(w, `{"name":"Synthetic BDS","protocol":2259}`)
			return
		}
		// Same-number TCP signaling and UDP media is the default single-port
		// configuration; separate media-port range validation has unit coverage.
		_, port, _ := net.SplitHostPort(r.Host)
		w.Header().Set("Content-Type", "application/sdp")
		_, _ = fmt.Fprintf(w, "v=0\r\na=identity:preserved\r\na=fingerprint:sha-256 preserved\r\na=candidate:a 1 UDP 1 127.0.0.1 %s typ host\r\n", port)
	}))
	t.Cleanup(upstream.Close)
	dest := upstream.Listener.Addr().(*net.TCPAddr).AddrPort()
	media, err := net.ListenUDP("udp4", net.UDPAddrFromAddrPort(dest))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { media.Close() })
	go func() {
		buf := make([]byte, 65535)
		for {
			n, src, err := media.ReadFromUDPAddrPort(buf)
			if err != nil {
				return
			}
			_, _ = media.WriteToUDPAddrPort(buf[:n], src)
		}
	}()
	env := newAgentEnv(t, startRelay(t))
	env.addGame("bedrock", games.MinecraftBedrock, games.UDP, 19132, dest)
	identity, pub := newKey()
	env.approve("bedrock", pub, "Bedrock player")
	checkIdentity, checkPub := newKey()
	env.approve("bedrock", checkPub, "Endpoint check")
	env.serve()
	opts := joinOpts(env.addr("bedrock"), games.UDP, 19132, identity)
	opts.Game = games.MinecraftBedrock
	opts.HealthInterval = 100 * time.Millisecond
	tun, err := transport.Join(context.Background(), opts)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(tun.Stop)
	if tun.Status().GameReachable != "yes" {
		t.Fatal("live BDS not detected")
	}
	url := "http://" + tun.Status().LocalAddr
	resp, err := http.Get(url + "/v1/join")
	if err != nil {
		t.Fatal(err)
	}
	data, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if resp.StatusCode != 200 || !bytes.Contains(data, []byte("Synthetic BDS")) {
		t.Fatal("metadata did not cross tunnel")
	}
	player, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer player.Close()
	playerAddr := player.LocalAddr().(*net.UDPAddr).AddrPort()
	offer := fmt.Sprintf("v=0\r\na=identity:preserved\r\na=fingerprint:sha-256 preserved\r\na=candidate:a 1 UDP 1 127.0.0.1 %d typ host\r\n", playerAddr.Port())
	resp, err = http.Post(url+"/v1/join/opaque-id", "application/sdp", strings.NewReader(offer))
	if err != nil {
		t.Fatal(err)
	}
	data, _ = io.ReadAll(resp.Body)
	resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("offer returned %d", resp.StatusCode)
	}
	_, candidates, err := consolebridge.MapCandidates(string(data), netip.MustParseAddr("127.0.0.1"), 1, 65535, playerAddr)
	if err != nil || len(candidates) != 1 {
		t.Fatal("answer has no local relay")
	}
	payload := bytes.Repeat([]byte("opaque-encrypted-media"), 150)
	_, _ = player.WriteToUDPAddrPort(payload, candidates[0])
	_ = player.SetReadDeadline(time.Now().Add(5 * time.Second))
	buf := make([]byte, 65535)
	n, _, err := player.ReadFromUDPAddrPort(buf)
	if err != nil || !bytes.Equal(buf[:n], payload) {
		t.Fatalf("media roundtrip failed: %v", err)
	}
	// The original legacy UDP path remains usable at the same loopback port.
	legacy, err := net.Dial("udp4", tun.Status().LocalAddr)
	if err != nil {
		t.Fatal(err)
	}
	defer legacy.Close()
	_ = legacy.SetDeadline(time.Now().Add(5 * time.Second))
	_, _ = legacy.Write([]byte("legacy"))
	n, err = legacy.Read(buf)
	if err != nil || string(buf[:n]) != "legacy" {
		t.Fatalf("legacy UDP failed: %v", err)
	}
	for _, want := range []string{"no", "yes"} {
		online.Store(want == "yes")
		deadline := time.Now().Add(10 * time.Second)
		for tun.Status().GameReachable != want && time.Now().Before(deadline) {
			time.Sleep(20 * time.Millisecond)
		}
		status := tun.Status()
		if status.State != transport.StateConnected || status.GameReachable != want {
			t.Fatalf("game health did not update: state=%s game=%s", status.State, status.GameReachable)
		}
		if want == "no" && !strings.Contains(status.Message, "server offline") {
			t.Fatal("offline game presented as online")
		}
		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		checkOpts := opts
		checkOpts.Identity = checkIdentity
		_, _, reachable, err := transport.CheckEndpoint(ctx, checkOpts)
		cancel()
		if err != nil || reachable != want {
			t.Fatalf("endpoint check game=%s want=%s err=%v", reachable, want, err)
		}
	}
}

func TestBedrockStalledCapabilityKeepsAgentConnected(t *testing.T) {
	release := make(chan struct{})
	defer close(release)
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		select {
		case <-r.Context().Done():
		case <-release:
		}
	}))
	t.Cleanup(upstream.Close)
	env := newAgentEnv(t, startRelay(t))
	env.addGame("bedrock", games.MinecraftBedrock, games.UDP, 19132, upstream.Listener.Addr().(*net.TCPAddr).AddrPort())
	identity, pub := newKey()
	env.approve("bedrock", pub, "Bedrock player")
	env.serve()
	opts := joinOpts(env.addr("bedrock"), games.UDP, 19132, identity)
	opts.Game = games.MinecraftBedrock
	opts.HealthInterval = time.Hour
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	started := time.Now()
	tun, err := transport.Join(ctx, opts)
	if err != nil {
		t.Fatalf("stalled game capability ended agent connection: %v", err)
	}
	defer tun.Stop()
	if time.Since(started) < 7*time.Second {
		t.Fatal("capability probe did not exercise its internal timeout")
	}
	if status := tun.Status(); status.State != transport.StateConnected || status.GameReachable != "no" {
		t.Fatalf("agent/game status = %s/%s", status.State, status.GameReachable)
	}
	if ctx.Err() != nil {
		t.Fatal("parent canceled instead of internal probe timing out")
	}
}
