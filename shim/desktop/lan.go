package main

import (
	"cmp"
	"errors"
	"net"
	"net/netip"
	"slices"
	"strings"
	"sync"
	"time"
	"unicode"
	"unicode/utf8"
)

const (
	lanPacketLimit = 1024
	lanPacketCount = 64
	lanPacketTTL   = 6 * time.Second
	// The shared tunnel gets four extra seconds to tolerate missed announcements.
	lanShareTTL = lanPacketTTL + 4*time.Second
)

var lanGroup = netip.MustParseAddrPort("224.0.2.60:4445")

type lanWorld struct {
	Name string `json:"name"`
	Port uint16 `json:"port"`
}

type lanSocket interface {
	join(iface *net.Interface) error
	read([]byte) (int, netip.Addr, error)
	close() error
}

type lanInterface struct {
	iface net.Interface
	addrs []netip.Addr
}

func lanInterfaces() ([]lanInterface, error) {
	ifaces, err := net.Interfaces()
	if err != nil {
		return nil, errors.New("could not read this PC's network interfaces")
	}
	out := make([]lanInterface, 0, len(ifaces))
	for _, iface := range ifaces {
		if iface.Flags&net.FlagUp == 0 || iface.Flags&(net.FlagMulticast|net.FlagLoopback) == 0 {
			continue
		}
		addrs, err := iface.Addrs()
		if err != nil {
			continue
		}
		local := lanInterface{iface: iface}
		for _, addr := range addrs {
			if prefix, err := netip.ParsePrefix(addr.String()); err == nil && prefix.Addr().Is4() {
				local.addrs = append(local.addrs, prefix.Addr())
			}
		}
		slices.SortFunc(local.addrs, netip.Addr.Compare)
		out = append(out, local)
	}
	slices.SortFunc(out, func(a, b lanInterface) int { return cmp.Compare(a.iface.Index, b.iface.Index) })
	return out, nil
}

func sameLANInterface(a, b lanInterface) bool {
	return a.iface.Index == b.iface.Index && a.iface.Name == b.iface.Name && a.iface.Flags == b.iface.Flags &&
		slices.Equal(a.iface.HardwareAddr, b.iface.HardwareAddr) && slices.Equal(a.addrs, b.addrs)
}

type lanTime struct {
	monotonic time.Time
	wall      time.Time
}

func lanNow() lanTime {
	now := time.Now()
	return lanTime{monotonic: now, wall: now.Round(0)}
}

func (now lanTime) expired(received lanTime, ttl time.Duration) bool {
	// The monotonic clock protects against wall clock setbacks. The wall clock
	// advances across suspend even when the monotonic clock does not.
	return now.monotonic.Sub(received.monotonic) >= ttl || now.wall.Sub(received.wall) >= ttl
}

// The desktop owner serializes socket changes. The receiver and TCP dial
// guard access the announcement cache under mu, independently of that owner.
type lanDiscovery struct {
	clock      func() lanTime
	interfaces func() ([]lanInterface, error)
	bind       func() (lanSocket, error)
	conn       lanSocket
	done       chan struct{}
	network    []lanInterface

	mu      sync.Mutex
	packets map[lanWorld]lanTime
	failed  bool
}

func newLANDiscovery() *lanDiscovery {
	return &lanDiscovery{clock: lanNow, interfaces: lanInterfaces, bind: bindMulticast, packets: make(map[lanWorld]lanTime)}
}

func (d *lanDiscovery) poll(ttl time.Duration) ([]lanWorld, error) {
	network, err := d.interfaces()
	if err != nil {
		d.close()
		return nil, err
	}
	if d.conn == nil || !slices.EqualFunc(d.network, network, sameLANInterface) {
		d.closeSocket()
		if err := d.listen(network); err != nil {
			d.close()
			return nil, err
		}
	}
	worlds, err := d.snapshot(ttl)
	if err != nil {
		d.close()
	}
	return worlds, err
}

func (d *lanDiscovery) listen(network []lanInterface) error {
	conn, err := d.bind()
	if err != nil {
		return errors.New("could not open Minecraft LAN discovery")
	}
	local := make(map[netip.Addr]bool)
	joined := false
	for _, iface := range network {
		for _, addr := range iface.addrs {
			local[addr] = true
		}
		if conn.join(&iface.iface) == nil {
			joined = true
		}
	}
	if !joined {
		conn.close()
		return errors.New("could not join Minecraft LAN discovery on a local interface")
	}
	d.conn, d.done, d.network = conn, make(chan struct{}), network
	d.mu.Lock()
	d.failed = false
	d.mu.Unlock()
	go d.receive(local)
	return nil
}

func (d *lanDiscovery) closeSocket() {
	if d.conn != nil {
		d.conn.close()
		<-d.done
		d.conn, d.done, d.network = nil, nil, nil
	}
}

func (d *lanDiscovery) close() {
	d.closeSocket()
	d.mu.Lock()
	clear(d.packets)
	d.failed = false
	d.mu.Unlock()
}

func (d *lanDiscovery) receive(local map[netip.Addr]bool) {
	defer close(d.done)
	var buf [lanPacketLimit + 1]byte
	for {
		n, source, err := d.conn.read(buf[:])
		if err != nil {
			if !errors.Is(err, net.ErrClosed) {
				d.mu.Lock()
				d.failed = true
				d.mu.Unlock()
			}
			return
		}
		if !local[source.Unmap()] {
			continue
		}
		if world, ok := parseLANAnnouncement(buf[:n]); ok {
			d.record(world)
		}
	}
}

func (d *lanDiscovery) record(world lanWorld) {
	d.mu.Lock()
	defer d.mu.Unlock()
	if _, exists := d.packets[world]; !exists && len(d.packets) >= lanPacketCount {
		var oldest lanWorld
		var earliest time.Time
		for candidate, received := range d.packets {
			if earliest.IsZero() || received.monotonic.Before(earliest) {
				oldest, earliest = candidate, received.monotonic
			}
		}
		delete(d.packets, oldest)
	}
	d.packets[world] = d.clock()
}

func (d *lanDiscovery) snapshot(ttl time.Duration) ([]lanWorld, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	if d.failed {
		return nil, errors.New("Minecraft LAN discovery stopped receiving")
	}
	out := make([]lanWorld, 0, len(d.packets))
	now := d.clock()
	for world, received := range d.packets {
		if now.expired(received, lanShareTTL) {
			delete(d.packets, world)
		} else if !now.expired(received, ttl) {
			out = append(out, world)
		}
	}
	return out, nil
}

func (d *lanDiscovery) live(world lanWorld) bool {
	d.mu.Lock()
	defer d.mu.Unlock()
	received, ok := d.packets[world]
	return !d.failed && ok && !d.clock().expired(received, lanShareTTL)
}

func parseLANAnnouncement(packet []byte) (lanWorld, bool) {
	if len(packet) > lanPacketLimit || !utf8.Valid(packet) {
		return lanWorld{}, false
	}
	body, ok := strings.CutPrefix(string(packet), "[MOTD]")
	if !ok {
		return lanWorld{}, false
	}
	name, portText, ok := strings.Cut(body, "[/MOTD][AD]")
	if !ok || strings.Contains(name, "[MOTD]") || strings.Contains(name, "[/MOTD]") {
		return lanWorld{}, false
	}
	portText, ok = strings.CutSuffix(portText, "[/AD]")
	if !ok || len(portText) == 0 || len(portText) > 5 {
		return lanWorld{}, false
	}
	port := 0
	for _, digit := range portText {
		if digit < '0' || digit > '9' {
			return lanWorld{}, false
		}
		port = port*10 + int(digit-'0')
	}
	if port == 0 || port > 65535 {
		return lanWorld{}, false
	}
	// Remove Minecraft formatting codes and unsafe display characters.
	var clean strings.Builder
	skip := false
	for _, r := range name {
		switch {
		case skip:
			skip = false
		case r == '§':
			skip = true
		case unicode.IsControl(r), r == '\u061c', r == '\u200e', r == '\u200f',
			r >= '\u202a' && r <= '\u202e', r >= '\u2066' && r <= '\u2069':
		default:
			clean.WriteRune(r)
		}
	}
	runes := []rune(strings.TrimSpace(clean.String()))
	if len(runes) == 0 {
		return lanWorld{Name: "Minecraft LAN world", Port: uint16(port)}, true
	}
	if len(runes) > 64 {
		runes = runes[:64]
	}
	return lanWorld{Name: strings.TrimRightFunc(string(runes), unicode.IsSpace), Port: uint16(port)}, true
}
