package integration

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"net"
	"net/netip"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/gamelink-project/gamelink/shim/consolebridge"
	"github.com/gamelink-project/gamelink/shim/consolebridge/discovery"
	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/transport"
	"github.com/tailscale/tailcat"
	"tailscale.com/envknob"
	"tailscale.com/types/key"
)

type upstreamPacket struct {
	addr   netip.AddrPort
	sender uint64
	msg    *discovery.MessagePacket
}

type consoleUpstream struct {
	conn    *net.UDPConn
	packets chan upstreamPacket
	done    chan struct{}
}

func startConsoleUpstream(t *testing.T, ip netip.Addr) *consoleUpstream {
	t.Helper()
	conn, err := net.ListenUDP("udp4", net.UDPAddrFromAddrPort(netip.AddrPortFrom(ip, 0)))
	if err != nil {
		t.Fatal(err)
	}
	u := &consoleUpstream{conn: conn, packets: make(chan upstreamPacket, 64), done: make(chan struct{})}
	data, err := (&discovery.ServerData{ServerName: "integration", Protocol: 1, Version: "test", LevelName: "world", AcceptsOnlineAuth: true, ConnectionType: 4}).MarshalBinary()
	if err != nil {
		t.Fatal(err)
	}
	go func() {
		defer close(u.done)
		buf := make([]byte, 65535)
		for {
			n, from, err := conn.ReadFromUDPAddrPort(buf)
			if err != nil {
				return
			}
			packet, sender, err := discovery.Unmarshal(buf[:n])
			if err != nil {
				continue
			}
			switch packet.(type) {
			case *discovery.RequestPacket:
				_, _ = conn.WriteToUDPAddrPort(discovery.Marshal(&discovery.ResponsePacket{ApplicationData: data}, 4242), from)
			case *discovery.MessagePacket:
				m := packet.(*discovery.MessagePacket)
				select {
				case u.packets <- upstreamPacket{addr: from, sender: sender, msg: m}:
				default:
				}
			}
		}
	}()
	t.Cleanup(func() {
		_ = conn.Close()
		<-u.done
	})
	return u
}

func (u *consoleUpstream) readSignal(t *testing.T, typ string, timeout time.Duration) upstreamPacket {
	t.Helper()
	deadline := time.NewTimer(timeout)
	defer deadline.Stop()
	for {
		select {
		case p := <-u.packets:
			sig, err := consolebridge.ParseSignal(p.msg.Data)
			if err == nil && sig.Type == typ {
				return p
			}
		case <-deadline.C:
			t.Fatalf("timed out waiting for upstream %s signal", typ)
		}
	}
}

func udpFreePort(t *testing.T) uint16 {
	t.Helper()
	c, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4zero})
	if err != nil {
		t.Fatal(err)
	}
	p := uint16(c.LocalAddr().(*net.UDPAddr).Port)
	_ = c.Close()
	return p
}

func consolePair(t *testing.T, ip netip.Addr) (*net.UDPConn, *net.UDPConn, uint16, uint16) {
	t.Helper()
	for tries := 0; tries < 32; tries++ {
		first, err := net.ListenUDP("udp4", net.UDPAddrFromAddrPort(netip.AddrPortFrom(ip, 0)))
		if err != nil {
			t.Fatal(err)
		}
		p := uint16(first.LocalAddr().(*net.UDPAddr).Port)
		if p == 65535 {
			first.Close()
			continue
		}
		second, err := net.ListenUDP("udp4", net.UDPAddrFromAddrPort(netip.AddrPortFrom(ip, p+1)))
		if err == nil {
			return first, second, p, p + 1
		}
		first.Close()
	}
	t.Fatal("could not reserve adjacent console UDP ports")
	return nil, nil, 0, 0
}

func loopbackIndex(t *testing.T) int {
	t.Helper()
	i, err := net.InterfaceByName("lo")
	if err != nil {
		t.Skip("loopback interface is unavailable")
	}
	return i.Index
}

type bridgePhone struct {
	client *tailcat.Client
	bridge *consolebridge.Client
	done   chan struct{}
	err    error
	ready  chan struct{}
}

func startBridgePhone(t *testing.T, addr tailcat.Addr, key key.NodePrivate, ip netip.Addr, port uint16) *bridgePhone {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 25*time.Second)
	defer cancel()
	cl := &tailcat.Client{Server: addr, Key: key, Logf: quietLogf(t)}
	control, err := cl.DialTCPPort(ctx, 31011)
	if err != nil {
		_ = cl.Close()
		t.Fatal(err)
	}
	media, err := cl.DialUDPPort(ctx, 31011)
	if err != nil {
		_ = control.Close()
		_ = cl.Close()
		t.Fatal(err)
	}
	ready := make(chan struct{})
	bridge, err := consolebridge.NewClient(context.Background(), control, media, consolebridge.ClientOptions{
		LANIP: ip, Prefix: netip.MustParsePrefix("127.0.0.0/8"), InterfaceIndex: loopbackIndex(t), DiscoveryPort: port,
		OnReady: func() { close(ready) },
	})
	if err != nil {
		_ = cl.Close()
		t.Fatal(err)
	}
	p := &bridgePhone{client: cl, bridge: bridge, done: make(chan struct{}), ready: ready}
	go func() { p.err = bridge.Run(); close(p.done) }()
	t.Cleanup(func() {
		_ = bridge.Close()
		_ = cl.Close()
		select {
		case <-p.done:
		case <-time.After(3 * time.Second):
			t.Error("bridge client did not stop")
		}
	})
	return p
}

func sendDiscoveryRequest(t *testing.T, socket *net.UDPConn, phone *bridgePhone, sender uint64) uint64 {
	t.Helper()
	to := phone.bridge.DiscoveryAddr()
	_, err := socket.WriteToUDPAddrPort(discovery.Marshal(&discovery.RequestPacket{}, sender), to)
	if err != nil {
		t.Fatal(err)
	}
	_ = socket.SetReadDeadline(time.Now().Add(3 * time.Second))
	buf := make([]byte, 65535)
	n, _, err := socket.ReadFromUDPAddrPort(buf)
	if err != nil {
		t.Fatal(err)
	}
	p, id, err := discovery.Unmarshal(buf[:n])
	if err != nil || p.(*discovery.ResponsePacket).ApplicationData == nil || id == 0 {
		t.Fatalf("invalid discovery response: %T %d %v", p, id, err)
	}
	var data discovery.ServerData
	if err := data.UnmarshalBinary(p.(*discovery.ResponsePacket).ApplicationData); err != nil || data.ConnectionType != 4 || !data.AcceptsOnlineAuth {
		t.Fatalf("invalid upstream advertisement: %+v %v", data, err)
	}
	return id
}

func candidate(ip netip.Addr, port uint16) string {
	return fmt.Sprintf("v=0\r\na=candidate:1 1 udp 1 %s %d typ host\r\n", ip, port)
}

func sendConsoleSignal(t *testing.T, socket *net.UDPConn, phone *bridgePhone, phoneID, consoleID uint64, sig consolebridge.Signal) {
	t.Helper()
	msg := &discovery.MessagePacket{RecipientID: phoneID, Data: consolebridge.FormatSignal(sig)}
	if _, err := socket.WriteToUDPAddrPort(discovery.Marshal(msg, consoleID), phone.bridge.DiscoveryAddr()); err != nil {
		t.Fatal(err)
	}
}

func waitConsoleSignal(t *testing.T, socket *net.UDPConn, sender uint64, typ string, timeout time.Duration) consolebridge.Signal {
	t.Helper()
	deadline := time.Now().Add(timeout)
	buf := make([]byte, 65535)
	for time.Now().Before(deadline) {
		_ = socket.SetReadDeadline(time.Now().Add(250 * time.Millisecond))
		n, _, err := socket.ReadFromUDPAddrPort(buf)
		if err != nil {
			if ne, ok := err.(net.Error); ok && ne.Timeout() {
				continue
			}
			t.Fatal(err)
		}
		packet, id, err := discovery.Unmarshal(buf[:n])
		if err != nil || id != sender {
			continue
		}
		m, ok := packet.(*discovery.MessagePacket)
		if !ok {
			continue
		}
		sig, err := consolebridge.ParseSignal(m.Data)
		if err == nil && sig.Type == typ {
			return sig
		}
	}
	t.Fatalf("timed out waiting for console %s signal", typ)
	return consolebridge.Signal{}
}

func parseCandidateEndpoint(t *testing.T, data string) netip.AddrPort {
	t.Helper()
	for _, line := range strings.Split(data, "\n") {
		f := strings.Fields(strings.TrimPrefix(line, "a="))
		if len(f) >= 8 && strings.HasPrefix(f[0], "candidate:") {
			ip, err := netip.ParseAddr(f[4])
			port, perr := strconv.ParseUint(f[5], 10, 16)
			if err == nil && perr == nil && port > 0 {
				return netip.AddrPortFrom(ip, uint16(port))
			}
		}
	}
	t.Fatalf("candidate missing from signal: %q", data)
	return netip.AddrPort{}
}

func TestConsoleBridgeTailcatMediaAndSessionIsolation(t *testing.T) {
	for _, tc := range []struct {
		name      string
		relayOnly bool
	}{{"direct_or_auto", false}, {"forced_derp", true}} {
		t.Run(tc.name, func(t *testing.T) { testConsoleBridgeMediaAndIsolation(t, tc.relayOnly) })
	}
}

func testConsoleBridgeMediaAndIsolation(t *testing.T, relayOnly bool) {
	if relayOnly {
		envknob.Setenv("TS_DEBUG_ALWAYS_USE_DERP", "true")
	} else {
		envknob.Setenv("TS_DEBUG_ALWAYS_USE_DERP", "")
	}
	t.Cleanup(func() { envknob.Setenv("TS_DEBUG_ALWAYS_USE_DERP", "") })
	region := startRelay(t)
	phoneIP := netip.MustParseAddr("127.0.0.2")
	consoleIP := netip.MustParseAddr("127.0.0.3")
	console1, console2, _, _ := consolePair(t, consoleIP)
	defer console1.Close()
	defer console2.Close()
	console3, err := net.ListenUDP("udp4", net.UDPAddrFromAddrPort(netip.AddrPortFrom(consoleIP, 0)))
	if err != nil {
		t.Fatal(err)
	}
	defer console3.Close()
	bds, err := net.ListenUDP("udp4", net.UDPAddrFromAddrPort(netip.AddrPortFrom(consoleIP, 0)))
	if err != nil {
		t.Fatal(err)
	}
	defer bds.Close()
	bdsPort := uint16(bds.LocalAddr().(*net.UDPAddr).Port)
	go func() {
		buf := make([]byte, 65535)
		for {
			n, from, err := bds.ReadFromUDPAddrPort(buf)
			if err != nil {
				return
			}
			_, _ = bds.WriteToUDPAddrPort(buf[:n], from)
		}
	}()
	upstream := startConsoleUpstream(t, consoleIP)
	host, err := consolebridge.NewHost(consolebridge.HostConfig{
		ServerIP: consoleIP, BindIP: consoleIP, DiscoveryPort: uint16(upstream.conn.LocalAddr().(*net.UDPAddr).Port),
		MediaPortFirst: bdsPort, MediaPortLast: bdsPort,
	})
	if err != nil {
		t.Fatal(err)
	}
	serverKey := key.NewNode()
	psk := tailcat.NewPresharedKey()
	phoneKeys := []key.NodePrivate{key.NewNode(), key.NewNode()}
	approved := []key.NodePublic{phoneKeys[0].Public(), phoneKeys[1].Public()}
	server, err := transport.StartGameServer(transport.ServerOptions{
		ID: "console-bridge", Protocol: games.UDP, TunnelPort: 31011,
		Dest: netip.AddrPortFrom(consoleIP, bdsPort), Private: serverKey, PSK: psk, Region: region,
		Approved: approved, ConsoleBridge: host, Logf: quietLogf(t),
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(server.Close)
	if _, err := transport.StartGameServer(transport.ServerOptions{
		ID: "empty", Protocol: games.UDP, TunnelPort: 31012,
		Dest: netip.AddrPortFrom(consoleIP, bdsPort), Private: key.NewNode(), PSK: tailcat.NewPresharedKey(), Region: region,
	}); !errors.Is(err, transport.ErrNoApprovedDevices) {
		t.Fatalf("empty allowlist error = %v", err)
	}

	phone1 := startBridgePhone(t, server.Addr(), phoneKeys[0], phoneIP, udpFreePort(t))
	phone2 := startBridgePhone(t, server.Addr(), phoneKeys[1], netip.MustParseAddr("127.0.0.4"), udpFreePort(t))
	for i, phone := range []*bridgePhone{phone1, phone2} {
		select {
		case <-phone.done:
			t.Fatalf("phone %d ended before readiness: %v", i, phone.err)
		case <-phone.ready:
		case <-time.After(5 * time.Second):
			t.Fatalf("phone %d did not receive the upstream advertisement", i)
		}
	}

	phoneID1 := sendDiscoveryRequest(t, console1, phone1, 1001)
	phoneID2 := sendDiscoveryRequest(t, console2, phone2, 1002)
	phoneID3 := sendDiscoveryRequest(t, console3, phone1, 1003)
	consoleID1, consoleID2, consoleID3 := uint64(9001), uint64(9002), uint64(9003)
	source1 := console1.LocalAddr().(*net.UDPAddr).AddrPort()
	source2 := console2.LocalAddr().(*net.UDPAddr).AddrPort()
	source3 := console3.LocalAddr().(*net.UDPAddr).AddrPort()
	if source1 == source2 || source1 == source3 || source2 == source3 {
		t.Fatal("test consoles unexpectedly share an endpoint")
	}
	activate := func(socket *net.UDPConn, phone *bridgePhone, phoneID, consoleID uint64, source netip.AddrPort) (netip.AddrPort, uint64) {
		sendConsoleSignal(t, socket, phone, phoneID, consoleID, consolebridge.Signal{Type: consolebridge.SignalOffer, ConnectionID: 77, Data: candidate(consoleIP, uint16(source.Port()))})
		offer := upstream.readSignal(t, consolebridge.SignalOffer, 5*time.Second)
		signal, err := consolebridge.ParseSignal(offer.msg.Data)
		if err != nil {
			t.Fatal(err)
		}
		hostRelay := parseCandidateEndpoint(t, signal.Data)
		if hostRelay.Addr() != consoleIP {
			t.Fatalf("host relay candidate escaped the configured host: %v", hostRelay)
		}
		if bdsPort == 65535 {
			t.Fatal("adjacent test media ports unexpectedly leave no out-of-range port")
		}
		bad := discovery.Marshal(&discovery.MessagePacket{RecipientID: offer.sender, Data: consolebridge.FormatSignal(consolebridge.Signal{
			Type: consolebridge.SignalAnswer, ConnectionID: signal.ConnectionID, Data: candidate(consoleIP, bdsPort+1),
		})}, 4242)
		if _, err := upstream.conn.WriteToUDPAddrPort(bad, offer.addr); err != nil {
			t.Fatal(err)
		}
		filtered := waitConsoleSignal(t, socket, phoneID, consolebridge.SignalAnswer, time.Second)
		if strings.Contains(filtered.Data, "candidate:") || strings.Contains(filtered.Data, consoleIP.String()) {
			t.Fatalf("answer retained an out-of-range destination: %q", filtered.Data)
		}
		answer := discovery.Marshal(&discovery.MessagePacket{RecipientID: offer.sender, Data: consolebridge.FormatSignal(consolebridge.Signal{
			Type: consolebridge.SignalAnswer, ConnectionID: signal.ConnectionID, Data: candidate(consoleIP, bdsPort),
		})}, 4242)
		if _, err := upstream.conn.WriteToUDPAddrPort(answer, offer.addr); err != nil {
			t.Fatal(err)
		}
		answerSignal := waitConsoleSignal(t, socket, phoneID, consolebridge.SignalAnswer, 3*time.Second)
		phoneRelay := parseCandidateEndpoint(t, answerSignal.Data)
		if !phoneRelay.Addr().IsLoopback() {
			t.Fatalf("phone relay candidate escaped loopback LAN: %v", phoneRelay)
		}
		return phoneRelay, signal.ConnectionID
	}
	phoneRelay1, session1 := activate(console1, phone1, phoneID1, consoleID1, source1)
	phoneRelay2, session2 := activate(console2, phone2, phoneID2, consoleID2, source2)
	phoneRelay3, session3 := activate(console3, phone1, phoneID3, consoleID3, source3)
	if session1 == session2 || session1 == session3 || session2 == session3 {
		t.Fatal("host reused a bridge session across independent consoles")
	}
	if phone1.bridge.SessionCount() != 2 || phone2.bridge.SessionCount() != 1 {
		t.Fatalf("console session routing counts = %d and %d, want 2 and 1", phone1.bridge.SessionCount(), phone2.bridge.SessionCount())
	}

	buf := make([]byte, 65535)
	for _, tc := range []struct {
		phone  *bridgePhone
		from   *net.UDPConn
		target netip.AddrPort
	}{{phone1, console1, phoneRelay1}, {phone2, console2, phoneRelay2}, {phone1, console3, phoneRelay3}} {
		for _, size := range []int{1232, 1233, 3000} {
			payload := bytes.Repeat([]byte{byte(size)}, size)
			if _, err := tc.from.WriteToUDPAddrPort(payload, tc.target); err != nil {
				t.Fatal(err)
			}
			_ = tc.from.SetReadDeadline(time.Now().Add(3 * time.Second))
			n, _, err := tc.from.ReadFromUDPAddrPort(buf)
			if err != nil || !bytes.Equal(buf[:n], payload) {
				t.Fatalf("media payload size %d: got %d bytes, err %v; sessions=%d transport=%+v", size, n, err, tc.phone.bridge.SessionCount(), server.Stats())
			}
		}
	}
	server.Close() // revocation closes the host and every active bridge session
	if got := server.Stats().Active; got != 0 {
		t.Fatalf("active bridge connections survived revocation: %d", got)
	}
	for _, phone := range []*bridgePhone{phone1, phone2} {
		_ = phone.bridge.Close()
		_ = phone.client.Close()
	}
	cleanupDeadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(cleanupDeadline) && (phone1.bridge.SessionCount() != 0 || phone2.bridge.SessionCount() != 0) {
		time.Sleep(10 * time.Millisecond)
	}
	if phone1.bridge.SessionCount() != 0 || phone2.bridge.SessionCount() != 0 {
		t.Fatalf("sessions survived host revocation: %d and %d", phone1.bridge.SessionCount(), phone2.bridge.SessionCount())
	}
	for _, phone := range []*bridgePhone{phone1, phone2} {
		select {
		case <-phone.done:
		case <-time.After(5 * time.Second):
			t.Fatal("phone worker did not stop after host revocation")
		}
		if got := phone.bridge.SessionCount(); got != 0 {
			t.Fatalf("session survived bridge cleanup: %d", got)
		}
	}
}

func TestConsoleBridgeRejectsUnapprovedPeer(t *testing.T) {
	region := startRelay(t)
	serverKey, deviceKey := key.NewNode(), key.NewNode()
	host, err := consolebridge.NewHost(consolebridge.HostConfig{
		ServerIP: netip.MustParseAddr("127.0.0.3"), BindIP: netip.MustParseAddr("127.0.0.3"), DiscoveryPort: udpFreePort(t),
		MediaPortFirst: 19132, MediaPortLast: 19132,
	})
	if err != nil {
		t.Fatal(err)
	}
	s, err := transport.StartGameServer(transport.ServerOptions{
		ID: "approved-only", Protocol: games.UDP, TunnelPort: 31011,
		Dest: netip.MustParseAddrPort("127.0.0.3:19132"), Private: serverKey, PSK: tailcat.NewPresharedKey(), Region: region,
		Approved: []key.NodePublic{key.NewNode().Public()}, ConsoleBridge: host,
	})
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	unapproved := &tailcat.Client{Server: s.Addr(), Key: deviceKey}
	defer unapproved.Close()
	if _, err := unapproved.Ping(ctx); err == nil {
		t.Fatal("unapproved device reached the bridge")
	}
}
