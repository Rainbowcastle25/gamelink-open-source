package main

import (
	"errors"
	"net"
	"net/netip"
	"slices"
	"sync"
	"time"
	"unicode/utf8"
)

const (
	lanPacketLimit = 1024 // same bound as the Rust announcement parser
	lanPacketCount = 64
	lanPacketTTL   = 6 * time.Second
)

var lanGroup = netip.MustParseAddrPort("224.0.2.60:4445")

type lanSocket interface {
	join(iface *net.Interface) error
	read([]byte) (int, netip.Addr, error)
	close() error
}

type lanListener struct {
	conn    lanSocket
	done    chan struct{}
	mu      sync.Mutex
	packets map[string]time.Time
	failed  bool
}

func listenLAN() (*lanListener, error) {
	conn, err := bindMulticast()
	if err != nil {
		return nil, errors.New("could not open Minecraft LAN discovery")
	}
	ifaces, err := net.Interfaces()
	if err != nil {
		conn.close()
		return nil, errors.New("could not read this PC's network interfaces")
	}
	local := make(map[netip.Addr]bool)
	joined := false
	for _, iface := range ifaces {
		if iface.Flags&net.FlagUp == 0 || iface.Flags&(net.FlagMulticast|net.FlagLoopback) == 0 {
			continue
		}
		addrs, err := iface.Addrs()
		if err != nil {
			continue
		}
		for _, addr := range addrs {
			if prefix, err := netip.ParsePrefix(addr.String()); err == nil && prefix.Addr().Is4() {
				local[prefix.Addr()] = true
			}
		}
		if conn.join(&iface) == nil {
			joined = true
		}
	}
	if !joined {
		conn.close()
		return nil, errors.New("could not join Minecraft LAN discovery on a local interface")
	}
	l := &lanListener{conn: conn, done: make(chan struct{}), packets: make(map[string]time.Time)}
	go l.receive(local)
	return l, nil
}

func (l *lanListener) close() {
	l.conn.close()
	<-l.done
}

func (l *lanListener) receive(local map[netip.Addr]bool) {
	defer close(l.done)
	var buf [lanPacketLimit + 1]byte
	for {
		n, source, err := l.conn.read(buf[:])
		if err != nil {
			if !errors.Is(err, net.ErrClosed) {
				l.mu.Lock()
				l.failed = true
				l.mu.Unlock()
			}
			return
		}
		if !local[source.Unmap()] || n > lanPacketLimit || !utf8.Valid(buf[:n]) {
			continue
		}
		l.mu.Lock()
		now := time.Now()
		packet := string(buf[:n])
		if _, exists := l.packets[packet]; !exists && len(l.packets) >= lanPacketCount {
			var oldest string
			var earliest time.Time
			for candidate, received := range l.packets {
				if earliest.IsZero() || received.Before(earliest) {
					oldest, earliest = candidate, received
				}
			}
			delete(l.packets, oldest)
		}
		l.packets[packet] = now
		l.mu.Unlock()
	}
}

func (l *lanListener) snapshot() ([]string, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.failed {
		return nil, errors.New("Minecraft LAN discovery stopped receiving")
	}
	out := make([]string, 0, len(l.packets))
	now := time.Now()
	for packet, received := range l.packets {
		if now.Sub(received) >= lanPacketTTL {
			delete(l.packets, packet)
		} else {
			out = append(out, packet)
		}
	}
	slices.Sort(out)
	return out, nil
}
