package transport

import (
	"net"
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
