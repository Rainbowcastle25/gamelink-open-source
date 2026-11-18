package main

import (
	"net"
	"slices"
	"testing"
	"time"

	"golang.org/x/net/ipv4"
)

func TestMinecraftDiscoveryReceivesOnlyGroupTraffic(t *testing.T) {
	listener, err := listenLAN()
	if err != nil {
		t.Fatal(err)
	}
	defer listener.close()
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
		packets, err := listener.snapshot()
		if err != nil {
			t.Fatal(err)
		}
		if slices.Contains(packets, unicast) {
			t.Fatal("discovery accepted unicast traffic")
		}
		if slices.Contains(packets, multicast) {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("did not receive the locally sent multicast announcement")
}
