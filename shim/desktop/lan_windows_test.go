//go:build windows

package main

import (
	"net"
	"slices"
	"testing"
	"time"
)

// Minecraft sends its announcement from a plain UDP socket on the default
// multicast interface, not loopback. Discovery must receive that copy.
func TestWindowsDiscoveryReceivesDefaultInterfaceAnnouncement(t *testing.T) {
	listener, err := listenLAN()
	if err != nil {
		t.Fatal(err)
	}
	defer listener.close()
	sender, err := net.ListenUDP("udp4", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer sender.Close()
	const announcement = "[MOTD]Windows LAN World[/MOTD][AD]51001[/AD]"
	group := net.UDPAddrFromAddrPort(lanGroup)
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		if _, err := sender.WriteToUDP([]byte(announcement), group); err != nil {
			t.Skipf("no default IPv4 multicast route on this machine: %v", err)
		}
		time.Sleep(50 * time.Millisecond)
		packets, err := listener.snapshot()
		if err != nil {
			t.Fatal(err)
		}
		if slices.Contains(packets, announcement) {
			return
		}
	}
	t.Fatal("did not receive an announcement sent on the default interface")
}
