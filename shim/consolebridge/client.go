package consolebridge

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"net"
	"net/netip"
	"sync"
	"sync/atomic"
	"time"

	"github.com/gamelink-project/gamelink/shim/consolebridge/discovery"
	"github.com/tailscale/tailcat"
	"golang.org/x/time/rate"
)

var ErrUnsupportedBridge = errors.New("host does not support console bridge version 1")

// ClientOptions names the explicit LAN exposure for a single foreground bridge.
type ClientOptions struct {
	LANIP          netip.Addr
	Prefix         netip.Prefix
	InterfaceIndex int
	DiscoveryPort  uint16 // zero selects the standard NetherNet port, 7551
	OnReady        func()
}

type clientRoute struct {
	source             netip.AddrPort
	sender, connection uint64
}
type clientSession struct {
	id       uint64
	route    clientRoute
	socket   *net.UDPConn
	targets  []netip.AddrPort
	selected netip.AddrPort
	last     time.Time
	created  time.Time
	active   bool
	sequence atomic.Uint64
	packets  *rate.Limiter
}

// Client owns discovery and media sockets. All gameplay stays inside Go and
// Tailcat. Close is concurrent/idempotent; Run returns after all workers exit.
type Client struct {
	opts          ClientOptions
	ctx           context.Context
	cancel        context.CancelFunc
	control       net.Conn
	media         tailcat.ConnPacketConn
	lan           *discovery.Listener
	writeMu       sync.Mutex
	mediaMu       sync.Mutex
	mu            sync.Mutex
	advertisement []byte
	sessions      map[uint64]*clientSession
	routes        map[clientRoute]uint64
	early         map[clientRoute][]string
	earlyAt       map[clientRoute]time.Time
	reassembly    Reassembler
	packets       *rate.Limiter
	signals       *rate.Limiter
	once          sync.Once
	wg            sync.WaitGroup
}

func NewClient(parent context.Context, control net.Conn, media tailcat.ConnPacketConn, opts ClientOptions) (*Client, error) {
	if control == nil || media == nil || !opts.LANIP.Is4() || !opts.Prefix.IsValid() || !opts.Prefix.Contains(opts.LANIP) {
		return nil, errors.New("invalid bridge LAN configuration")
	}
	if opts.DiscoveryPort == 0 {
		opts.DiscoveryPort = 7551
	}
	l, err := discovery.ListenLAN(opts.LANIP, opts.Prefix, opts.InterfaceIndex, opts.DiscoveryPort)
	if err != nil {
		return nil, errors.New("cannot bind console discovery on the selected Wi-Fi")
	}
	ctx, cancel := context.WithCancel(parent)
	return &Client{opts: opts, ctx: ctx, cancel: cancel, control: control, media: media, lan: l,
		sessions: map[uint64]*clientSession{}, routes: map[clientRoute]uint64{}, early: map[clientRoute][]string{}, earlyAt: map[clientRoute]time.Time{},
		packets: rate.NewLimiter(4096, 512), signals: rate.NewLimiter(100, 64)}, nil
}

func (c *Client) SessionCount() int { c.mu.Lock(); defer c.mu.Unlock(); return len(c.sessions) }
func (c *Client) DiscoveryAddr() netip.AddrPort {
	return netip.AddrPortFrom(c.opts.LANIP, c.lan.LocalAddr().Port())
}

func (c *Client) Run() error {
	defer func() { c.Close(); c.wg.Wait() }()
	c.wg.Add(1)
	go func() { defer c.wg.Done(); <-c.ctx.Done(); c.Close() }()
	if err := c.send(ControlMessage{Version: 1, Type: "hello"}); err != nil {
		return err
	}
	_ = c.control.SetReadDeadline(time.Now().Add(15 * time.Second))
	hello, err := ReadControl(c.control)
	if err != nil {
		return err
	}
	if hello.Type != "hello" || hello.Data != "console-v1" {
		return ErrUnsupportedBridge
	}
	// Materialize the authenticated UDP flow before the upstream starts ICE
	// checks. This reserved non-fragment heartbeat never reaches game sockets.
	if _, err = c.media.Write([]byte("GB-keepalive-v1")); err != nil {
		return err
	}
	c.wg.Add(3)
	go func() { defer c.wg.Done(); c.discoveryLoop() }()
	go func() { defer c.wg.Done(); c.mediaLoop() }()
	go func() { defer c.wg.Done(); c.maintenance() }()
	ready := false
	for {
		_ = c.control.SetReadDeadline(time.Now().Add(45 * time.Second))
		msg, err := ReadControl(c.control)
		if err != nil {
			return err
		}
		if !c.signals.Allow() {
			return errors.New("host signaling rate exceeded")
		}
		switch msg.Type {
		case "advertisement":
			data, err := base64.StdEncoding.DecodeString(msg.Data)
			var ad discovery.ServerData
			if err != nil || len(data) > 4096 || ad.UnmarshalBinary(data) != nil || ad.ConnectionType != 4 || !ad.AcceptsOnlineAuth {
				return errors.New("upstream does not advertise authenticated NetherNet LAN play")
			}
			c.mu.Lock()
			c.advertisement = append([]byte(nil), data...)
			c.mu.Unlock()
			if !ready {
				ready = true
				if c.opts.OnReady != nil {
					c.opts.OnReady()
				}
			}
		case "answer", "candidate", "error":
			c.deliverSignal(msg)
		case "status": // bounded control heartbeat, no raw status text in UI
		default:
			return errors.New("unexpected bridge control message")
		}
	}
}

func (c *Client) send(msg ControlMessage) error {
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	_ = c.control.SetWriteDeadline(time.Now().Add(5 * time.Second))
	return WriteControl(c.control, msg)
}

func (c *Client) discoveryLoop() {
	limiter := rate.NewLimiter(100, 32)
	for {
		d, err := c.lan.Receive()
		if err != nil {
			c.Close()
			return
		}
		if !limiter.Allow() {
			continue
		}
		switch p := d.Packet.(type) {
		case *discovery.RequestPacket:
			c.mu.Lock()
			ad := append([]byte(nil), c.advertisement...)
			c.mu.Unlock()
			if len(ad) > 0 {
				_ = c.lan.Send(&discovery.ResponsePacket{ApplicationData: ad}, d.Source)
			}
		case *discovery.MessagePacket:
			if p.RecipientID != c.lan.NetworkID() {
				continue
			}
			// Signal parser never accepts an arbitrary destination or text type.
			sig, err := ParseSignal(p.Data)
			if err != nil {
				continue
			}
			route := clientRoute{d.Source, d.SenderID, sig.ConnectionID}
			c.handleSignal(route, sig)
		}
	}
}

func (c *Client) handleSignal(route clientRoute, sig Signal) {
	c.mu.Lock()
	if c.ctx.Err() != nil {
		c.mu.Unlock()
		return
	}
	id := c.routes[route]
	s := c.sessions[id]
	if s == nil && sig.Type == "candidate" {
		if len(c.early) < 32 || len(c.early[route]) > 0 {
			if len(c.early[route]) < 16 && len(sig.Data) <= 2048 {
				c.early[route] = append(c.early[route], sig.Data)
				c.earlyAt[route] = time.Now()
			}
		}
		c.mu.Unlock()
		return
	}
	if s == nil && sig.Type == "offer" {
		if len(c.sessions) >= 32 || len(c.advertisement) == 0 {
			c.mu.Unlock()
			return
		}
		var b [8]byte
		if _, err := rand.Read(b[:]); err != nil {
			c.mu.Unlock()
			return
		}
		id = binary.BigEndian.Uint64(b[:])
		if id == 0 || c.sessions[id] != nil {
			c.mu.Unlock()
			return
		}
		socket, err := net.ListenUDP("udp4", net.UDPAddrFromAddrPort(netip.AddrPortFrom(c.opts.LANIP, 0)))
		if err != nil {
			c.mu.Unlock()
			return
		}
		s = &clientSession{id: id, route: route, socket: socket, last: time.Now(), created: time.Now(), packets: rate.NewLimiter(2048, 256)}
		c.sessions[id] = s
		c.routes[route] = id
		c.wg.Add(1)
		go func() { defer c.wg.Done(); c.readConsole(s) }()
	}
	if s == nil {
		c.mu.Unlock()
		return
	}
	if sig.Type == "error" {
		c.mu.Unlock()
		_ = c.send(ControlMessage{Version: 1, Type: "error", Session: id, Data: sig.Data})
		c.drop(id)
		return
	}
	if sig.Type != "offer" && sig.Type != "candidate" {
		c.mu.Unlock()
		return
	}
	relay := s.socket.LocalAddr().(*net.UDPAddr).AddrPort()
	mapped, targets, err := MapCandidates(sig.Data, route.source.Addr(), 1024, 65535, relay)
	if err != nil || (sig.Type == "candidate" && len(targets) == 0) {
		c.mu.Unlock()
		return
	}
	for _, target := range targets {
		s.targets = appendUniqueTarget(s.targets, target)
	}
	early := c.early[route]
	delete(c.early, route)
	delete(c.earlyAt, route)
	s.last = time.Now()
	c.mu.Unlock()
	if err = c.send(ControlMessage{Version: 1, Type: sig.Type, Session: id, Data: mapped, PeerIP: c.opts.LANIP.String()}); err != nil {
		c.Close()
		return
	}
	for _, candidate := range early {
		c.handleSignal(route, Signal{Type: "candidate", ConnectionID: route.connection, Data: candidate})
	}
}

func appendUniqueTarget(targets []netip.AddrPort, target netip.AddrPort) []netip.AddrPort {
	for _, t := range targets {
		if t == target {
			return targets
		}
	}
	if len(targets) < 8 {
		return append(targets, target)
	}
	return targets
}

func (c *Client) deliverSignal(msg ControlMessage) {
	c.mu.Lock()
	s := c.sessions[msg.Session]
	if s == nil {
		c.mu.Unlock()
		return
	}
	route := s.route
	data := msg.Data
	if msg.Type != "error" {
		var err error
		// The host has replaced its BDS candidates with the phone relay IP.
		// Map again here to the concrete port for this session, never pass a
		// host-private candidate back to a console.
		data, _, err = MapCandidates(data, c.opts.LANIP, 1, 65535, s.socket.LocalAddr().(*net.UDPAddr).AddrPort())
		if err != nil {
			c.mu.Unlock()
			c.drop(msg.Session)
			return
		}
	}
	s.last = time.Now()
	c.mu.Unlock()
	_ = c.lan.Send(&discovery.MessagePacket{RecipientID: route.sender, Data: FormatSignal(Signal{Type: msg.Type, ConnectionID: route.connection, Data: data})}, route.source)
	if msg.Type == "error" {
		c.drop(msg.Session)
	}
}

func (c *Client) readConsole(s *clientSession) {
	defer c.drop(s.id)
	buf := make([]byte, 65535)
	for {
		n, src, err := s.socket.ReadFromUDPAddrPort(buf)
		if err != nil {
			return
		}
		src = netip.AddrPortFrom(src.Addr().Unmap(), src.Port())
		c.mu.Lock()
		allowed := false
		for _, target := range s.targets {
			if target == src {
				allowed = true
				break
			}
		}
		if allowed && s.selected.IsValid() && s.selected != src {
			allowed = false
		}
		if allowed {
			s.selected = src
			s.last = time.Now()
			s.active = true
		}
		c.mu.Unlock()
		if !allowed {
			continue
		}
		seq := s.sequence.Add(1)
		parts, err := Fragment(s.id, seq, buf[:n])
		if err != nil {
			continue
		}
		if !s.packets.AllowN(time.Now(), len(parts)) || !c.packets.AllowN(time.Now(), len(parts)) {
			continue
		}
		c.mediaMu.Lock()
		for _, p := range parts {
			if _, err = c.media.Write(p); err != nil {
				break
			}
		}
		c.mediaMu.Unlock()
		if err != nil {
			c.Close()
			return
		}
	}
}

func (c *Client) mediaLoop() {
	buf := make([]byte, MaxPacket+1)
	for {
		n, err := c.media.Read(buf)
		if err != nil {
			c.Close()
			return
		}
		if n < 24 || n > MaxPacket || !c.packets.Allow() {
			continue
		}
		id := binary.BigEndian.Uint64(buf[8:16])
		c.mu.Lock()
		s := c.sessions[id]
		c.mu.Unlock()
		if s == nil {
			continue
		}
		_, data, err := c.reassembly.Add(buf[:n], time.Now())
		if err != nil || data == nil {
			continue
		}
		c.mu.Lock()
		targets := append([]netip.AddrPort(nil), s.targets...)
		if s.selected.IsValid() {
			targets = []netip.AddrPort{s.selected}
		}
		s.last = time.Now()
		s.active = true
		c.mu.Unlock()
		if !s.packets.Allow() {
			continue
		}
		for _, target := range targets {
			_, _ = s.socket.WriteToUDPAddrPort(data, target)
		}
	}
}

func (c *Client) maintenance() {
	sweep := time.NewTicker(5 * time.Second)
	defer sweep.Stop()
	ping := time.NewTicker(15 * time.Second)
	defer ping.Stop()
	for {
		select {
		case <-c.ctx.Done():
			return
		case <-ping.C:
			if c.send(ControlMessage{Version: 1, Type: "status"}) != nil {
				c.Close()
				return
			}
			c.mediaMu.Lock()
			_, err := c.media.Write([]byte("GB-keepalive-v1"))
			c.mediaMu.Unlock()
			if err != nil {
				c.Close()
				return
			}
		case now := <-sweep.C:
			c.reassembly.Expire(now)
			c.mu.Lock()
			var dead []uint64
			for id, s := range c.sessions {
				if (!s.active && now.Sub(s.created) > 30*time.Second) || now.Sub(s.last) > 5*time.Minute {
					dead = append(dead, id)
				}
			}
			for route, t := range c.earlyAt {
				if now.Sub(t) > 5*time.Second {
					delete(c.early, route)
					delete(c.earlyAt, route)
				}
			}
			c.mu.Unlock()
			for _, id := range dead {
				_ = c.send(ControlMessage{Version: 1, Type: "error", Session: id, Data: "1"})
				c.drop(id)
			}
		}
	}
}
func (c *Client) drop(id uint64) {
	c.mu.Lock()
	s := c.sessions[id]
	if s != nil {
		delete(c.sessions, id)
		delete(c.routes, s.route)
	}
	c.mu.Unlock()
	if s != nil {
		_ = s.socket.Close()
	}
	c.reassembly.DropSession(id)
}
func (c *Client) Close() error {
	c.once.Do(func() {
		c.cancel()
		_ = c.lan.Close()
		_ = c.control.Close()
		_ = c.media.Close()
		c.mu.Lock()
		for _, s := range c.sessions {
			_ = s.socket.Close()
		}
		c.sessions = map[uint64]*clientSession{}
		c.routes = map[clientRoute]uint64{}
		c.mu.Unlock()
	})
	return nil
}

// ResolveLAN verifies an explicit address belongs to a real interface. Android
// supplies its prefix/index from ConnectivityManager to avoid netlink access.
func ResolveLAN(ip netip.Addr, prefixBits, index int) (netip.Prefix, int, error) {
	if !ip.Is4() || ip.IsUnspecified() || ip.IsMulticast() {
		return netip.Prefix{}, 0, errors.New("invalid LAN address")
	}
	if prefixBits > 0 && prefixBits <= 32 && index > 0 {
		return netip.PrefixFrom(ip, prefixBits), index, nil
	}
	interfaces, err := net.Interfaces()
	if err != nil {
		return netip.Prefix{}, 0, err
	}
	for _, iface := range interfaces {
		addrs, _ := iface.Addrs()
		for _, a := range addrs {
			p, err := netip.ParsePrefix(a.String())
			if err == nil && p.Addr().Unmap() == ip {
				return p, iface.Index, nil
			}
		}
	}
	return netip.Prefix{}, 0, errors.New("selected LAN address is unavailable")
}
