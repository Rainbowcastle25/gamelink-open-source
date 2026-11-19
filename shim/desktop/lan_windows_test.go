//go:build windows

package main

import (
	"errors"
	"net"
	"slices"
	"testing"
	"time"

	"golang.org/x/sys/windows"
)

// Minecraft sends announcements using the default IPv4 multicast interface.
func TestWindowsDiscoveryAcceptsDefaultInterfaceMulticastAndRejectsUnicast(t *testing.T) {
	checkRoute := func(err error) {
		t.Helper()
		if errors.Is(err, windows.WSAENETUNREACH) || errors.Is(err, windows.WSAEHOSTUNREACH) ||
			errors.Is(err, windows.WSAEADDRNOTAVAIL) || errors.Is(err, windows.WSAENETDOWN) {
			t.Skipf("default IPv4 multicast route unavailable: %v", err)
		}
		if err != nil {
			t.Fatal(err)
		}
	}

	sender, err := net.DialUDP("udp4", nil, net.UDPAddrFromAddrPort(lanGroup))
	checkRoute(err)
	defer sender.Close()
	ip := sender.LocalAddr().(*net.UDPAddr).IP
	if ip.IsUnspecified() {
		t.Fatal("the multicast socket did not select a local IPv4 address")
	}
	unicast, err := net.ListenUDP("udp4", &net.UDPAddr{IP: ip})
	if err != nil {
		t.Fatal(err)
	}
	defer unicast.Close()

	discovery := newLANDiscovery()
	if _, err := discovery.poll(lanPacketTTL); err != nil {
		t.Fatal(err)
	}
	defer discovery.close()

	rejected := lanWorld{Name: "Windows Unicast", Port: 51002}
	accepted := lanWorld{Name: "Windows LAN World", Port: 51001}
	if _, err := unicast.WriteToUDP([]byte("[MOTD]Windows Unicast[/MOTD][AD]51002[/AD]"),
		&net.UDPAddr{IP: ip, Port: int(lanGroup.Port())}); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(3 * time.Second)
	var received time.Time
	for time.Now().Before(deadline) {
		if received.IsZero() {
			_, err := sender.Write([]byte("[MOTD]Windows LAN World[/MOTD][AD]51001[/AD]"))
			checkRoute(err)
		}
		time.Sleep(50 * time.Millisecond)
		worlds, err := discovery.snapshot(lanPacketTTL)
		if err != nil {
			t.Fatal(err)
		}
		if slices.Contains(worlds, rejected) {
			t.Fatal("discovery accepted unicast addressed to the multicast interface")
		}
		if received.IsZero() && slices.Contains(worlds, accepted) {
			received = time.Now()
			deadline = received.Add(250 * time.Millisecond)
		}
	}
	if received.IsZero() {
		t.Fatalf("discovery did not receive multicast sent on the default interface (%v)", ip)
	}
}
