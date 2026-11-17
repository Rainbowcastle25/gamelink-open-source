package integration

import (
	"encoding/base64"
	"encoding/json"
	"net"
	"net/netip"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/gamelink-project/gamelink/mobile"
	"github.com/gamelink-project/gamelink/shim/consolebridge"
	"github.com/gamelink-project/gamelink/shim/consolebridge/discovery"
	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/invite"
	"github.com/gamelink-project/gamelink/shim/transport"
	"github.com/tailscale/tailcat"
	"tailscale.com/types/key"
)

// TestAndroidTailcatFixture starts a real Tailcat GameServer and consolebridge
// Host, then publishes a protected fixture that an Android emulator can use to
// prove it connects from the app's Go sandbox. It is opt-in because it waits
// for a separately run instrumentation test.
func TestAndroidTailcatFixture(t *testing.T) {
	fixturePath := os.Getenv("GAMELINK_ANDROID_FIXTURE")
	if fixturePath == "" {
		t.Skip("set GAMELINK_ANDROID_FIXTURE to enable the Android sandbox fixture")
	}
	if !filepath.IsAbs(fixturePath) {
		t.Fatal("Android fixture path must be absolute")
	}
	stopPath := fixturePath + ".stop"
	t.Cleanup(func() {
		_ = os.Remove(stopPath)
		_ = os.Remove(fixturePath)
	})
	if err := os.MkdirAll(filepath.Dir(fixturePath), 0700); err != nil {
		t.Fatal("cannot create protected fixture directory")
	}
	_ = os.Remove(stopPath)

	relay := startRelay(t)
	loopback := netip.MustParseAddr("127.0.0.1")
	upstream, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal("cannot bind synthetic Bedrock discovery endpoint")
	}
	t.Cleanup(func() { _ = upstream.Close() })
	go serveFixtureDiscovery(upstream)

	media, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal("cannot reserve synthetic media port")
	}
	mediaPort := uint16(media.LocalAddr().(*net.UDPAddr).Port)
	_ = media.Close()
	host, err := consolebridge.NewHost(consolebridge.HostConfig{
		ServerIP: loopback, BindIP: loopback,
		DiscoveryPort:  uint16(upstream.LocalAddr().(*net.UDPAddr).Port),
		MediaPortFirst: mediaPort, MediaPortLast: mediaPort,
	})
	if err != nil {
		t.Fatal("cannot create synthetic Bedrock bridge host")
	}

	deviceKeyBytes, err := mobile.GenerateIdentity()
	if err != nil {
		t.Fatal("cannot create fixture device identity")
	}
	defer clear(deviceKeyBytes)
	var deviceKey key.NodePrivate
	if err := deviceKey.UnmarshalText(deviceKeyBytes); err != nil || deviceKey.IsZero() {
		t.Fatal("cannot decode fixture device identity")
	}

	serverIdentity := tailcat.NewPrivateKey()
	server, err := transport.StartGameServer(transport.ServerOptions{
		ID: "android-fixture", Protocol: games.UDP, TunnelPort: 19132,
		Dest:    netip.AddrPortFrom(loopback, 19132),
		Private: serverIdentity.Private, PSK: serverIdentity.Public.PresharedKey,
		Region: relay, Approved: []key.NodePublic{deviceKey.Public()},
		ConsoleBridge: host, Logf: quietLogf(t),
	})
	if err != nil {
		_ = host.Close()
		t.Fatal("cannot start protected Tailcat fixture server")
	}
	t.Cleanup(server.Close)

	clientInfo, err := tailcat.ParseAddr(server.Addr())
	if err != nil || len(clientInfo.Region) == 0 {
		t.Fatal("cannot prepare emulator-only relay address")
	}
	// The native test process talks to the local DERP listener directly, while
	// Android's emulator reaches that same host listener through 10.0.2.2.
	// This rewrite applies only to the protected test fixture address.
	for _, region := range clientInfo.Region {
		for _, node := range region.Nodes {
			if node.STUNOnly {
				continue
			}
			node.HostName = "10.0.2.2"
			node.IPv4 = "10.0.2.2"
			node.IPv6 = "none"
			node.CertName = ""
		}
	}
	invitation, err := invite.Encode(invite.Invite{
		Name: "Android bridge fixture", Game: games.MinecraftBedrock,
		Proto: games.UDP, Port: 19132, Addr: string(clientInfo.Addr()), ConsoleBridge: 1,
	}, invite.Options{AllowLocalRelay: true})
	if err != nil {
		t.Fatal("cannot encode protected Android fixture invitation")
	}
	fixture := struct {
		Invitation string `json:"invitation"`
		PrivateKey string `json:"privateKey"`
	}{Invitation: invitation, PrivateKey: base64.StdEncoding.EncodeToString(deviceKeyBytes)}
	encoded, err := json.Marshal(fixture)
	if err != nil || os.WriteFile(fixturePath, encoded, 0600) != nil {
		t.Fatal("cannot write protected Android fixture")
	}
	if err := os.Chmod(fixturePath, 0600); err != nil {
		t.Fatal("cannot protect Android fixture permissions")
	}
	t.Log("Android Tailcat fixture ready; copy the protected fixture to the target app and run the opt-in instrumentation test")

	deadline := time.NewTimer(3 * time.Minute)
	defer deadline.Stop()
	ticker := time.NewTicker(200 * time.Millisecond)
	defer ticker.Stop()
	for {
		if _, err := os.Stat(stopPath); err == nil {
			_ = os.Remove(stopPath)
			_ = os.Remove(fixturePath)
			return
		}
		select {
		case <-deadline.C:
			_ = os.Remove(fixturePath)
			t.Fatal("Android fixture timed out waiting for instrumentation completion")
		case <-ticker.C:
		}
	}
}

func serveFixtureDiscovery(upstream *net.UDPConn) {
	data, err := (&discovery.ServerData{
		ServerName: "GameLink fixture", Protocol: 2259, Version: "1.26.70",
		LevelName: "Fixture world", GameType: discovery.GameTypeSurvival,
		PlayerCount: 0, MaxPlayerCount: 8, AcceptsOnlineAuth: true,
		AcceptsSelfSignedAuth: true, Nonce: "fixture-nonce-stable", ConnectionType: 4,
	}).MarshalBinary()
	if err != nil {
		return
	}
	buf := make([]byte, 4096)
	for {
		_ = upstream.SetReadDeadline(time.Now().Add(500 * time.Millisecond))
		n, source, err := upstream.ReadFromUDP(buf)
		if err != nil {
			if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
				continue
			}
			return
		}
		packet, _, err := discovery.Unmarshal(buf[:n])
		if err != nil {
			continue
		}
		if _, ok := packet.(*discovery.RequestPacket); !ok {
			continue
		}
		_, _ = upstream.WriteToUDP(discovery.Marshal(&discovery.ResponsePacket{ApplicationData: data}, 42), source)
	}
}
