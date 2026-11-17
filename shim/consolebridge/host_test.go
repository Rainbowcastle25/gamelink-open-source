package consolebridge

import (
	"bytes"
	"encoding/base64"
	"fmt"
	"net"
	"net/netip"
	"strings"
	"testing"
	"time"

	"github.com/gamelink-project/gamelink/shim/consolebridge/discovery"
)

type remoteConn struct {
	net.Conn
	remote net.Addr
}

func (c remoteConn) RemoteAddr() net.Addr { return c.remote }

func TestHostDisconnectsPeerWhenUpstreamNetworkIDChanges(t *testing.T) {
	ip := netip.MustParseAddr("127.0.0.1")
	upstream, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer upstream.Close()
	h, err := NewHost(HostConfig{ServerIP: ip, BindIP: ip,
		DiscoveryPort: uint16(upstream.LocalAddr().(*net.UDPAddr).Port), MediaPortFirst: 19132, MediaPortLast: 19132})
	if err != nil {
		t.Fatal(err)
	}
	defer h.Close()
	a, b := net.Pipe()
	defer b.Close()
	done := make(chan struct{})
	go func() {
		h.HandleTCP(remoteConn{a, &net.TCPAddr{IP: net.ParseIP("fd00::123"), Port: 12}})
		close(done)
	}()
	b.SetDeadline(time.Now().Add(time.Second))
	if err := WriteControl(b, ControlMessage{Version: 1, Type: "hello"}); err != nil {
		t.Fatal(err)
	}
	if _, err := ReadControl(b); err != nil {
		t.Fatal(err)
	}
	buf := make([]byte, 4096)
	upstream.SetReadDeadline(time.Now().Add(time.Second))
	_, peer, err := upstream.ReadFromUDP(buf)
	if err != nil {
		t.Fatal(err)
	}
	data, _ := (&discovery.ServerData{Nonce: "before-restart", AcceptsOnlineAuth: true}).MarshalBinary()
	upstream.WriteToUDP(discovery.Marshal(&discovery.ResponsePacket{ApplicationData: data}, 42), peer)
	if _, err := ReadControl(b); err != nil {
		t.Fatal(err)
	}
	data, _ = (&discovery.ServerData{Nonce: "after-restart", AcceptsOnlineAuth: true}).MarshalBinary()
	upstream.WriteToUDP(discovery.Marshal(&discovery.ResponsePacket{ApplicationData: data}, 43), peer)
	if _, err := ReadControl(b); err == nil {
		t.Fatal("changed upstream identity left the old peer connected")
	} else if e, ok := err.(net.Error); ok && e.Timeout() {
		t.Fatal("upstream restart did not close stale control and discovery state")
	}
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("stale upstream peer did not finish cleanup")
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if len(h.peers) != 0 {
		t.Fatal("stale peer prevents a reconnect from learning new metadata")
	}
}

func TestHostDiscoveryAndSignaling(t *testing.T) {
	upstream, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer upstream.Close()
	media, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer media.Close()
	mediaPort := uint16(media.LocalAddr().(*net.UDPAddr).Port)
	loop := netip.MustParseAddr("127.0.0.1")
	h, err := NewHost(HostConfig{ServerIP: loop, BindIP: loop, DiscoveryPort: uint16(upstream.LocalAddr().(*net.UDPAddr).Port), MediaPortFirst: mediaPort, MediaPortLast: mediaPort})
	if err != nil {
		t.Fatal(err)
	}
	defer h.Close()
	server, client := net.Pipe()
	defer client.Close()
	go h.HandleTCP(remoteConn{server, &net.TCPAddr{IP: net.ParseIP("fd00::123"), Port: 12}})
	if err := WriteControl(client, ControlMessage{Version: 1, Type: "hello"}); err != nil {
		t.Fatal(err)
	}
	m, err := ReadControl(client)
	if err != nil || m.Type != "hello" || m.Data != "console-v1" {
		t.Fatalf("hello: %+v %v", m, err)
	}
	buf := make([]byte, 65536)
	upstream.SetReadDeadline(time.Now().Add(3 * time.Second))
	n, source, err := upstream.ReadFromUDP(buf)
	if err != nil {
		t.Fatal(err)
	}
	_, id, err := discovery.Unmarshal(buf[:n])
	if err != nil {
		t.Fatal(err)
	}
	data, _ := (&discovery.ServerData{ServerName: "test", Nonce: "secret", AcceptsOnlineAuth: true}).MarshalBinary()
	upstream.WriteToUDP(discovery.Marshal(&discovery.ResponsePacket{ApplicationData: data}, 42), source)
	m, err = ReadControl(client)
	if err != nil || m.Type != "advertisement" || m.Data != base64.StdEncoding.EncodeToString(data) {
		t.Fatalf("advertisement: %+v %v", m, err)
	}
	offer := "v=0\r\na=candidate:1 1 udp 1 10.0.0.2 40000 typ host\r\n"
	if err := WriteControl(client, ControlMessage{Version: 1, Type: "offer", Session: 33, PeerIP: "10.0.0.2", Data: offer}); err != nil {
		t.Fatal(err)
	}
	n, _, err = upstream.ReadFromUDP(buf)
	if err != nil {
		t.Fatal(err)
	}
	pk, _, err := discovery.Unmarshal(buf[:n])
	if err != nil {
		t.Fatal(err)
	}
	msg := pk.(*discovery.MessagePacket)
	sig, err := ParseSignal(msg.Data)
	if err != nil || msg.RecipientID != 42 || sig.ConnectionID != 33 || strings.Contains(sig.Data, "10.0.0.2") {
		t.Fatalf("offer: %+v %+v %v", msg, sig, err)
	}
	answer := fmt.Sprintf("v=0\r\na=candidate:1 1 udp 1 127.0.0.1 %d typ host\r\n", mediaPort)
	upstream.WriteToUDP(discovery.Marshal(&discovery.MessagePacket{RecipientID: id, Data: FormatSignal(Signal{SignalAnswer, 33, answer})}, 42), source)
	m, err = ReadControl(client)
	if err != nil || m.Type != "answer" || !strings.Contains(m.Data, "10.0.0.2 40000") {
		t.Fatalf("answer: %+v %v", m, err)
	}
	a, b := net.Pipe()
	defer b.Close()
	go h.HandleUDP(packetConn{remoteConn{a, &net.UDPAddr{IP: net.ParseIP("fd00::123"), Port: 33}}})
	// Wait until the authenticated packet flow has attached.
	until := time.Now().Add(time.Second)
	for {
		h.mu.Lock()
		p := h.peers[netip.MustParseAddr("fd00::123")]
		h.mu.Unlock()
		p.mu.Lock()
		ready := p.flow != nil
		p.mu.Unlock()
		if ready {
			break
		}
		if time.Now().After(until) {
			t.Fatal("flow not attached")
		}
		time.Sleep(time.Millisecond)
	}
	payload := bytes.Repeat([]byte("x"), 3000)
	packets, _ := Fragment(33, 1, payload)
	for _, packet := range packets {
		if _, err = b.Write(packet); err != nil {
			t.Fatal(err)
		}
	}
	media.SetReadDeadline(time.Now().Add(time.Second))
	n, relay, err := media.ReadFromUDP(buf)
	if err != nil || !bytes.Equal(buf[:n], payload) {
		t.Fatalf("media: %d %v", n, err)
	}
	media.WriteToUDP([]byte("response"), relay)
	b.SetReadDeadline(time.Now().Add(time.Second))
	n, err = b.Read(buf)
	if err != nil {
		t.Fatal(err)
	}
	var re Reassembler
	_, reply, err := re.Add(buf[:n], time.Now())
	if err != nil || string(reply) != "response" {
		t.Fatalf("reply %q %v", reply, err)
	}
	client.Close()
	deadline := time.Now().Add(time.Second)
	for time.Now().Before(deadline) {
		h.mu.Lock()
		count := h.sessions
		h.mu.Unlock()
		if count == 0 {
			return
		}
		time.Sleep(time.Millisecond)
	}
	t.Fatal("sessions not cleaned up")
}
func TestHostConfigLimits(t *testing.T) {
	ip := netip.MustParseAddr("127.0.0.1")
	if _, err := NewHost(HostConfig{ServerIP: ip, BindIP: ip, MediaPortFirst: 1, MediaPortLast: 257}); err == nil {
		t.Fatal("accepted broad range")
	}
}

func TestHostBoundsSignalingWaitingForTrickleCandidate(t *testing.T) {
	s := &hostSession{}
	p := &hostPeer{sessions: map[uint64]*hostSession{1: s}}
	for i := 0; i < 40; i++ {
		p.upstreamSignal(Signal{Type: SignalAnswer, ConnectionID: 1, Data: strings.Repeat("x", 65536)})
	}
	if len(s.pending) != 2 || s.pendingBytes != MaxControlFrame {
		t.Fatalf("unbounded pending signaling: %d messages, %d bytes", len(s.pending), s.pendingBytes)
	}
}

func TestHostRejectsSessionOverloadAndReleasesEverySocket(t *testing.T) {
	ip := netip.MustParseAddr("127.0.0.1")
	h, err := NewHost(HostConfig{ServerIP: ip, BindIP: ip, MediaPortFirst: 19132, MediaPortLast: 19132})
	if err != nil {
		t.Fatal(err)
	}
	disc, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	a, b := net.Pipe()
	defer b.Close()
	p := &hostPeer{h: h, ip: ip, tcp: a, disc: disc, remote: 1, done: make(chan struct{}), sessions: map[uint64]*hostSession{}}
	h.peers[ip] = p
	defer func() { p.close(); p.wg.Wait() }()
	for id := uint64(1); id <= 33; id++ {
		p.offer(ControlMessage{Version: 1, Type: "offer", Session: id, PeerIP: "192.168.1.2", Data: "v=0\r\n"})
	}
	if len(p.sessions) != 32 || h.sessions != 32 || p.sessions[33] != nil {
		t.Fatalf("overload admitted: peer=%d host=%d", len(p.sessions), h.sessions)
	}
	var ports []netip.AddrPort
	for _, s := range p.sessions {
		ports = append(ports, s.socket.LocalAddr().(*net.UDPAddr).AddrPort())
	}
	p.close()
	p.wg.Wait()
	if h.sessions != 0 || len(h.peers) != 0 {
		t.Fatal("host retained stopped sessions")
	}
	for _, port := range ports {
		c, err := net.ListenUDP("udp4", net.UDPAddrFromAddrPort(port))
		if err != nil {
			t.Fatalf("media port survived Stop: %v", err)
		}
		c.Close()
	}
}
func TestSignalFormat(t *testing.T) {
	want := Signal{SignalCandidate, 42, "candidate:1 1 udp 1 1.2.3.4 5000 typ host"}
	got, err := ParseSignal(FormatSignal(want))
	if err != nil || got != want {
		t.Fatalf("%+v %v", got, err)
	}
	if _, err = ParseSignal("OTHER 42 test"); err == nil {
		t.Fatal("accepted unknown signal")
	}
}

// packetConn adapts a pipe into a test packet flow. Each write is one packet.
type packetConn struct{ remoteConn }

func (c packetConn) ReadFrom(p []byte) (int, net.Addr, error) {
	n, e := c.Read(p)
	return n, c.RemoteAddr(), e
}
func (c packetConn) WriteTo(p []byte, _ net.Addr) (int, error) { return c.Write(p) }
