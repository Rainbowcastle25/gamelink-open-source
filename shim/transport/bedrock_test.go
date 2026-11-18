package transport

import (
	"fmt"
	"net"
	"strings"
	"testing"

	"github.com/gamelink-project/gamelink/shim/games"
)

func TestBedrockRequiresBothLoopbackProtocolsOnOnePort(t *testing.T) {
	busy, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer busy.Close()
	port := uint16(busy.Addr().(*net.TCPAddr).Port)
	exact := &Tunnel{opts: TunnelOptions{Game: games.MinecraftBedrock, Proto: games.UDP, LocalPort: port}}
	if err := exact.bind(); err == nil {
		exact.closeSockets()
		t.Fatal("accepted Bedrock port with TCP already occupied")
	}
	// A failed dual bind must release the UDP half immediately.
	udp, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: int(port)})
	if err != nil {
		t.Fatalf("failed bind leaked UDP: %v", err)
	}
	udp.Close()
	fallback := &Tunnel{opts: TunnelOptions{Game: games.MinecraftBedrock, Proto: games.UDP, PreferPort: port}}
	if err := fallback.bind(); err != nil {
		t.Fatal(err)
	}
	defer fallback.closeSockets()
	if !fallback.status.PortFallback || fallback.status.LocalPort == int(port) {
		t.Fatal("did not fall back from TCP conflict")
	}
	if fallback.tcpLn == nil || fallback.udpPC == nil || fallback.tcpLn.Addr().String() != fallback.udpPC.LocalAddr().String() {
		t.Fatal("TCP and UDP bound to different ports")
	}
}

// A port the user pinned must never silently move: a busy pin fails the join
// with an error naming the port instead of falling back.
func TestPinnedLocalPortFailsWhenBusy(t *testing.T) {
	busy, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer busy.Close()
	port := uint16(busy.Addr().(*net.TCPAddr).Port)
	pinned := &Tunnel{opts: TunnelOptions{Game: games.MinecraftJava, Proto: games.TCP, LocalPort: port, PreferPort: port}}
	err = pinned.bind()
	if err == nil {
		pinned.closeSockets()
		t.Fatal("pinned port fell back instead of failing")
	}
	if want := fmt.Sprintf("local port %d is already in use", port); !strings.Contains(err.Error(), want) {
		t.Fatalf("error %q does not name the pinned port", err)
	}
	if pinned.status.PortFallback || pinned.status.LocalPort != 0 {
		t.Fatalf("pinned bind changed status: %+v", pinned.status)
	}
}
