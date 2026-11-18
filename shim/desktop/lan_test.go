package main

import (
	"errors"
	"net"
	"net/netip"
	"slices"
	"strings"
	"testing"
	"time"

	"golang.org/x/net/ipv4"
)

func TestMinecraftDiscoveryReceivesOnlyGroupTraffic(t *testing.T) {
	discovery := newLANDiscovery()
	_, err := discovery.poll(lanPacketTTL)
	if err != nil {
		t.Fatal(err)
	}
	defer discovery.close()
	loopback := net.IPv4(127, 0, 0, 1)
	sender, err := net.ListenUDP("udp4", &net.UDPAddr{IP: loopback})
	if err != nil {
		t.Fatal(err)
	}
	defer sender.Close()
	ifaces, err := net.Interfaces()
	if err != nil {
		t.Fatal(err)
	}
	var iface *net.Interface
	for i := range ifaces {
		if ifaces[i].Flags&net.FlagLoopback != 0 {
			iface = &ifaces[i]
			break
		}
	}
	packetConn := ipv4.NewPacketConn(sender)
	if iface == nil || packetConn.SetMulticastInterface(iface) != nil {
		t.Skip("loopback multicast unavailable on this machine")
	}
	if err := packetConn.SetMulticastLoopback(true); err != nil {
		t.Fatal(err)
	}
	const unicast = "[MOTD]Unicast[/MOTD][AD]25565[/AD]"
	const multicast = "[MOTD]Local World[/MOTD][AD]51000[/AD]"
	send := func(to net.IP, packet string) {
		t.Helper()
		if _, err := sender.WriteToUDP([]byte(packet), &net.UDPAddr{IP: to, Port: 4445}); err != nil {
			t.Fatal(err)
		}
	}
	send(loopback, unicast)
	send(net.IPv4(224, 0, 2, 60), multicast)
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		packets, err := discovery.snapshot(lanPacketTTL)
		if err != nil {
			t.Fatal(err)
		}
		if slices.Contains(packets, lanWorld{Name: "Unicast", Port: 25565}) {
			t.Fatal("discovery accepted unicast traffic")
		}
		if slices.Contains(packets, lanWorld{Name: "Local World", Port: 51000}) {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("did not receive the locally sent multicast announcement")
}

type lanTestDatagram struct {
	source netip.Addr
	text   string
}

type lanTestSocket struct {
	datagrams chan lanTestDatagram
	closed    chan struct{}
	joinGroup func(*net.Interface) error
}

func (s *lanTestSocket) join(iface *net.Interface) error {
	if s.joinGroup != nil {
		return s.joinGroup(iface)
	}
	return nil
}

func (s *lanTestSocket) read(buf []byte) (int, netip.Addr, error) {
	select {
	case p := <-s.datagrams:
		return copy(buf, p.text), p.source, nil
	case <-s.closed:
		return 0, netip.Addr{}, net.ErrClosed
	}
}

func (s *lanTestSocket) close() error {
	close(s.closed)
	return nil
}

func TestLANDiscoveryRebindsAfterAddressChange(t *testing.T) {
	oldAddr := netip.MustParseAddr("192.0.2.1")
	newAddr := netip.MustParseAddr("192.0.2.2")
	addr := oldAddr
	probes := 0
	discovery := newLANDiscovery()
	discovery.interfaces = func() ([]lanInterface, error) {
		probes++
		return []lanInterface{{iface: net.Interface{Index: 1}, addrs: []netip.Addr{addr}}}, nil
	}
	var sockets []*lanTestSocket
	discovery.bind = func() (lanSocket, error) {
		s := &lanTestSocket{datagrams: make(chan lanTestDatagram), closed: make(chan struct{})}
		sockets = append(sockets, s)
		return s, nil
	}
	defer discovery.close()

	poll := func() {
		t.Helper()
		if _, err := discovery.poll(lanShareTTL); err != nil {
			t.Fatal(err)
		}
	}
	send := func(source netip.Addr, text string) {
		t.Helper()
		select {
		case sockets[len(sockets)-1].datagrams <- lanTestDatagram{source, text}:
		case <-time.After(time.Second):
			t.Fatal("listener did not read the datagram")
		}
	}
	// The next read cannot start until the receiver has processed this packet.
	flush := func() { send(netip.Addr{}, "") }

	poll()
	send(oldAddr, "[MOTD]Cabin[/MOTD][AD]51000[/AD]")
	flush()
	cabin := lanWorld{Name: "Cabin", Port: 51000}
	if !discovery.live(cabin) {
		t.Fatal("the original interface address was not accepted")
	}
	poll()
	if len(sockets) != 1 {
		t.Fatal("an unchanged interface caused a rebind")
	}

	addr = newAddr
	poll()
	if len(sockets) != 2 {
		t.Fatal("an address change did not rebind discovery")
	}
	select {
	case <-sockets[0].closed:
	default:
		t.Fatal("the former socket is still open")
	}
	send(oldAddr, "[MOTD]Old source[/MOTD][AD]52000[/AD]")
	send(newAddr, "[MOTD]New source[/MOTD][AD]52000[/AD]")
	flush()
	if discovery.live(lanWorld{Name: "Old source", Port: 52000}) ||
		!discovery.live(lanWorld{Name: "New source", Port: 52000}) ||
		!discovery.live(cabin) {
		t.Fatal("rebind did not update source filtering or preserve the existing share's deadline")
	}
	if probes != 3 {
		t.Fatalf("interface source was called %d times for three polls", probes)
	}
}

func TestLANDiscoveryRetriesOnlyFailedMemberships(t *testing.T) {
	discovery := newLANDiscovery()
	discovery.interfaces = func() ([]lanInterface, error) {
		return []lanInterface{{iface: net.Interface{Index: 1}}, {iface: net.Interface{Index: 2}}}, nil
	}
	var attempts []int
	binds := 0
	discovery.bind = func() (lanSocket, error) {
		binds++
		return &lanTestSocket{
			closed: make(chan struct{}),
			joinGroup: func(iface *net.Interface) error {
				attempts = append(attempts, iface.Index)
				if iface.Index == 2 && len(attempts) < 4 {
					return errors.New("temporary join failure")
				}
				return nil
			},
		}, nil
	}
	defer discovery.close()

	for range 4 {
		if _, err := discovery.poll(lanShareTTL); err != nil {
			t.Fatal(err)
		}
	}
	if want := []int{1, 2, 2, 2}; !slices.Equal(attempts, want) {
		t.Fatalf("join attempts = %v, want %v", attempts, want)
	}
	if binds != 1 {
		t.Fatalf("socket bound %d times, want 1", binds)
	}
}

func TestParseLANAnnouncement(t *testing.T) {
	got, ok := parseLANAnnouncement([]byte("[MOTD] §a森林\u202e [/MOTD][AD]65535[/AD]"))
	if !ok || got != (lanWorld{Name: "森林", Port: 65535}) {
		t.Fatalf("unexpected normalized world: %+v", got)
	}
	got, ok = parseLANAnnouncement([]byte("[MOTD][/MOTD][AD]1[/AD]"))
	if !ok || got.Name != "Minecraft LAN world" {
		t.Fatalf("unexpected fallback world: %+v", got)
	}

	good := "[MOTD]World[/MOTD][AD]25565[/AD]"
	for n := range len(good) {
		if _, ok := parseLANAnnouncement([]byte(good[:n])); ok {
			t.Fatalf("parsed a truncated packet: %q", good[:n])
		}
	}
	for _, port := range []string{"", "0", "65536", "+1", "000001", "١", "123456", "1[/AD][AD]2", "127.0.0.1:1"} {
		if _, ok := parseLANAnnouncement([]byte("[MOTD]World[/MOTD][AD]" + port + "[/AD]")); ok {
			t.Fatalf("parsed invalid port text: %q", port)
		}
	}
	for _, packet := range []string{"[MOTD]\xff[/MOTD][AD]1[/AD]", "junk" + good, good + "\x00", "[MOTD][MOTD]World[/MOTD][AD]1[/AD]"} {
		if _, ok := parseLANAnnouncement([]byte(packet)); ok {
			t.Fatalf("parsed invalid packet: %q", packet)
		}
	}

	const prefix, suffix = "[MOTD]", "[/MOTD][AD]9[/AD]"
	atLimit := prefix + strings.Repeat("x", lanPacketLimit-len(prefix)-len(suffix)) + suffix
	if got, ok := parseLANAnnouncement([]byte(atLimit)); !ok || len(got.Name) != 64 {
		t.Fatalf("unexpected result for the largest valid packet: %+v", got)
	}
	if _, ok := parseLANAnnouncement([]byte(prefix + "x" + atLimit[len(prefix):])); ok {
		t.Fatal("parsed an oversized packet")
	}
	if got, ok := parseLANAnnouncement([]byte(prefix + strings.Repeat("界", 80) + suffix)); !ok || got.Name != strings.Repeat("界", 64) {
		t.Fatalf("truncated Unicode name incorrectly: %+v", got)
	}
}
