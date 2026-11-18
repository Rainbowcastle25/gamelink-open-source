package transport

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/netip"
	"sync"
	"sync/atomic"
	"time"

	"github.com/tailscale/tailcat"
	"golang.org/x/time/rate"
	"tailscale.com/tailcfg"
	"tailscale.com/types/key"
	"tailscale.com/types/logger"
	"tailscale.com/wgengine/filter"

	"github.com/gamelink-project/gamelink/shim/bedrock"
	"github.com/gamelink-project/gamelink/shim/games"
)

// ErrNoApprovedDevices is returned when a game has no approved devices and
// knock-only mode is disabled. Tailcat never receives an empty allowlist.
var ErrNoApprovedDevices = errors.New("game has no approved devices; refusing to start it")

// ServerOptions configures one exposed game.
type ServerOptions struct {
	ID         string
	Game       games.Type
	Protocol   games.Protocol
	TunnelPort uint16         // the single port reachable through the tunnel
	Dest       netip.AddrPort // where connections are forwarded
	Private    key.NodePrivate
	PSK        tailcat.PresharedKey
	Region     *tailcfg.DERPRegion // pinned relay region; avoids any DERP map fetch
	Approved   []key.NodePublic
	// CollectRequests records refused peers. With an empty approval list it
	// starts in knock-only mode, with no admitted peers or served game ports.
	CollectRequests bool
	ExpiresAt       time.Time
	// ConsoleBridge explicitly replaces the normal single-protocol forwarder
	// with both authenticated control and media handlers on TunnelPort.
	ConsoleBridge ConsoleBridgeHandler
	// Bedrock media destinations are pinned independently of TCP signaling.
	// Zero defaults to the configured destination port (single-port BDS).
	BedrockMediaPortFirst, BedrockMediaPortLast uint16

	// Logf receives Tailcat's own log output. It must already be redacted.
	// Nil discards it.
	Logf logger.Logf
	// Event, if set, receives human-readable events (denials, dial failures).
	Event func(kind, msg string)

	// AllowTCPDial optionally gates each new TCP connection to Dest.
	AllowTCPDial func() bool

	// Peers optionally records authorized gameplay sessions.
	Peers *PeerActivity

	MaxSessions    int           // concurrent TCP connections / UDP flows (default 256)
	DialTimeout    time.Duration // time allowed to reach Dest (default 5s)
	UDPIdleTimeout time.Duration // idle UDP flow lifetime (default tailcat's 2 min)
}

// ConsoleBridgeHandler owns a bridge's control and media state. Methods must
// return when their connection is closed; Close releases all bridge resources.
type ConsoleBridgeHandler interface {
	HandleTCP(net.Conn)
	HandleUDP(tailcat.ConnPacketConn)
	Close() error
}

// ServerStats are the counters of a running GameServer.
type ServerStats struct {
	Active        int   `json:"active"`
	Accepted      int64 `json:"accepted"`
	Denied        int64 `json:"denied"`
	DialFailures  int64 `json:"dialFailures"`
	OversizeDrops int64 `json:"oversizeDrops"`
	Overloaded    int64 `json:"overloaded"`
}

// GameServer exposes one game port through its own Tailcat identity to exactly
// the approved devices. Bedrock also serves authenticated TCP signaling.
type GameServer struct {
	opts    ServerOptions
	srv     *tailcat.Server
	allowed map[netip.Addr]key.NodePublic
	conns   *connSet
	logf    func(kind, msg string)

	active     atomic.Int64
	accepted   atomic.Int64
	denied     atomic.Int64
	dialFails  atomic.Int64
	oversize   atomic.Int64
	overloaded atomic.Int64
	probes     atomic.Int64

	wg          sync.WaitGroup
	closeOnce   sync.Once
	stopping    chan struct{}
	dialLog     rateLimited
	sizeLog     rateLimited
	denyLog     rateLimited
	requests    joinRequests
	bedrockHost *bedrock.Host
}

// StartGameServer starts a Tailcat server for one game. It fails with
// ErrNoApprovedDevices if there are no approved devices and knocks are off.
func StartGameServer(opts ServerOptions) (*GameServer, error) {
	knockOnly := len(opts.Approved) == 0
	if knockOnly && !opts.CollectRequests {
		return nil, ErrNoApprovedDevices
	}
	if !opts.ExpiresAt.IsZero() && !time.Now().Before(opts.ExpiresAt) {
		return nil, errors.New("game invitation has expired")
	}
	if !opts.Protocol.Valid() {
		return nil, fmt.Errorf("invalid protocol %q", opts.Protocol)
	}
	if opts.TunnelPort == 0 {
		return nil, errors.New("tunnel port is required")
	}
	if !opts.Dest.IsValid() || opts.Dest.Port() == 0 {
		return nil, errors.New("destination is required")
	}
	if opts.Private.IsZero() || opts.PSK.IsZero() || opts.Region == nil {
		return nil, errors.New("server identity is incomplete (private key, pre-shared key and relay region are required)")
	}
	for _, k := range opts.Approved {
		if k.IsZero() {
			return nil, errors.New("approved list contains a zero key")
		}
	}
	if opts.MaxSessions <= 0 {
		opts.MaxSessions = 256
	}
	if opts.DialTimeout <= 0 {
		opts.DialTimeout = 5 * time.Second
	}

	g := &GameServer{
		opts:     opts,
		allowed:  make(map[netip.Addr]key.NodePublic, len(opts.Approved)),
		conns:    newConnSet(),
		stopping: make(chan struct{}),
		logf:     func(kind, msg string) {},
		dialLog:  rateLimited{interval: 10 * time.Second},
		sizeLog:  rateLimited{interval: 10 * time.Second},
		denyLog:  rateLimited{interval: 10 * time.Second},
	}
	if opts.Event != nil {
		g.logf = opts.Event
	}
	if opts.CollectRequests {
		g.requests.newKeys = rate.NewLimiter(4, 8)
		g.requests.pending = make(map[key.NodePublic]JoinRequest)
	}
	if !knockOnly && opts.Game == games.MinecraftBedrock && opts.ConsoleBridge == nil {
		if opts.Protocol != games.UDP {
			return nil, errors.New("Bedrock requires UDP gameplay")
		}
		host, err := bedrock.NewHost(opts.Dest, opts.BedrockMediaPortFirst, opts.BedrockMediaPortLast)
		if err != nil {
			return nil, err
		}
		g.bedrockHost = host
	}
	for _, k := range opts.Approved {
		g.allowed[PeerAddr(k)] = k
	}

	port := filter.PortRange{First: opts.TunnelPort, Last: opts.TunnelPort}
	approved := append([]key.NodePublic(nil), opts.Approved...)
	if knockOnly {
		// Tailcat interprets an empty list as allow-all. Use a disposable
		// public key whose private key is never retained or shared.
		approved = []key.NodePublic{key.NewNode().Public()}
	}
	s := &tailcat.Server{
		Key:          opts.Private,
		PresharedKey: opts.PSK,
		Region:       opts.Region,
		Logf:         g.tailcatLog,
		// Layer 1: Tailcat itself ignores every key not in this list.
		AllowedClients: approved,
		// Layer 2: the packet filter admits only the one served port, for one
		// protocol. The other protocol gets an empty (non-nil) list = none.
		ServedTCPPorts: []filter.PortRange{},
		ServedUDPPorts: []filter.PortRange{},
		UDPIdleTimeout: opts.UDPIdleTimeout,
		// OnTCPForward / OnUDPForward / AllowProxy are deliberately never set:
		// that would make this server an exit node.
	}
	// In knock-only mode both filters remain empty and both handlers stay nil.
	if !knockOnly {
		bridge := opts.ConsoleBridge != nil || g.bedrockHost != nil
		if bridge || opts.Protocol == games.TCP {
			s.ServedTCPPorts = []filter.PortRange{port}
			s.OnTCP = g.onTCP
		}
		if bridge || opts.Protocol == games.UDP {
			s.ServedUDPPorts = []filter.PortRange{port}
			s.OnUDP = g.onUDP
		}
	}
	if err := s.Start(); err != nil {
		if g.bedrockHost != nil {
			g.bedrockHost.Close()
		}
		return nil, fmt.Errorf("start tailcat server: %w", err)
	}
	g.srv = s
	if !opts.ExpiresAt.IsZero() {
		go func() {
			timer := time.NewTimer(time.Until(opts.ExpiresAt))
			defer timer.Stop()
			select {
			case <-timer.C:
				g.Close()
			case <-g.stopping:
			}
		}()
	}
	return g, nil
}

func (g *GameServer) expired() bool {
	return !g.opts.ExpiresAt.IsZero() && !time.Now().Before(g.opts.ExpiresAt)
}

// Addr returns the server's full Tailcat address (a secret).
func (g *GameServer) Addr() tailcat.Addr { return g.srv.TailcatAddr() }

// Stats returns a snapshot of the counters.
func (g *GameServer) Stats() ServerStats {
	return ServerStats{
		Active: int(g.active.Load()), Accepted: g.accepted.Load(), Denied: g.denied.Load(),
		DialFailures: g.dialFails.Load(), OversizeDrops: g.oversize.Load(), Overloaded: g.overloaded.Load(),
	}
}

// Close shuts the Tailcat server down and terminates every live session.
// Removing an approved device requires a new GameServer; Tailcat has no way to
// remove a client from a running server, and Close is what makes a revocation
// take effect on sessions that are already open.
func (g *GameServer) Close() {
	g.closeOnce.Do(func() {
		close(g.stopping)
		g.conns.closeAll()
		if g.bedrockHost != nil {
			g.bedrockHost.Close()
		}
		if g.opts.ConsoleBridge != nil {
			_ = g.opts.ConsoleBridge.Close()
		}
		// Netstack queues TCP FINs asynchronously. Keep the encrypted transport
		// alive long enough to deliver them before destroying its packet queues.
		// Active closers may remain in TIME-WAIT, so draining must be bounded.
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		_ = g.srv.DrainTCP(ctx)
		cancel()
		_ = g.srv.Close()
		done := make(chan struct{})
		go func() { g.wg.Wait(); close(done) }()
		select {
		case <-done:
		case <-time.After(3 * time.Second):
		}
	})
}

// authorize checks the tunnel-authenticated peer of a connection against the
// approved set (defence in depth on top of Tailcat's AllowedClients).
func (g *GameServer) authorize(remote net.Addr) bool {
	if g.expired() {
		return false
	}
	ip, ok := addrOf(remote)
	if ok {
		if _, allowed := g.allowed[ip]; allowed {
			return true
		}
	}
	g.denied.Add(1)
	g.logRefusal()
	return false
}

func (g *GameServer) logRefusal() {
	g.denyLog.do(func() { g.logf("security", "refused a connection from a peer that is not approved for this game") })
}

func (g *GameServer) admit() bool {
	if g.active.Add(1) > int64(g.opts.MaxSessions) {
		g.active.Add(-1)
		g.overloaded.Add(1)
		return false
	}
	g.accepted.Add(1)
	return true
}

func (g *GameServer) onTCP(port uint16) func(net.Conn) {
	if g.expired() {
		return nil
	}
	select {
	case <-g.stopping:
		return nil // reject new handshakes while existing TCP FINs drain
	default:
	}
	if port != g.opts.TunnelPort || (g.opts.ConsoleBridge == nil && g.bedrockHost == nil && g.opts.Protocol != games.TCP) {
		return nil // Tailcat answers with a reset
	}
	return func(c net.Conn) {
		g.wg.Add(1)
		defer g.wg.Done()
		if !g.admit() {
			c.Close()
			return
		}
		defer g.active.Add(-1)
		if !g.conns.add(c) {
			return
		}
		defer g.conns.remove(c)
		if !g.authorizeTCP(c) {
			c.Close()
			return
		}
		if g.opts.ConsoleBridge != nil {
			defer c.Close()
			g.opts.ConsoleBridge.HandleTCP(c)
			return
		}
		if g.bedrockHost != nil {
			g.opts.Peers.recordSession(g.allowed, c.RemoteAddr())
			g.bedrockHost.Handle(c)
			return
		}
		if g.opts.AllowTCPDial != nil && !g.opts.AllowTCPDial() {
			c.Close()
			return
		}
		d := net.Dialer{Timeout: g.opts.DialTimeout}
		dst, err := d.Dial("tcp", g.opts.Dest.String())
		if err != nil {
			g.dialFails.Add(1)
			g.dialLog.do(func() { g.logf("warning", "cannot reach the game server at "+g.opts.Dest.String()+": "+err.Error()) })
			c.Close()
			return
		}
		if !g.conns.add(dst) {
			return
		}
		defer g.conns.remove(dst)
		g.opts.Peers.recordSession(g.allowed, c.RemoteAddr())
		tailcat.ProxyConns(c, dst) // closes both when finished
	}
}

// Tailcat's netstack calls TCP handlers before the handshake necessarily
// finishes; gonet then returns a nil RemoteAddr. Await the address without
// consuming application bytes (server-first protocols must still work). Pending
// handshakes already occupy a session slot and are cancelled by server shutdown.
func (g *GameServer) authorizeTCP(c net.Conn) bool {
	if remote := c.RemoteAddr(); remote != nil {
		return g.authorize(remote)
	}
	timeout := time.NewTimer(g.opts.DialTimeout)
	defer timeout.Stop()
	tick := time.NewTicker(time.Millisecond)
	defer tick.Stop()
	for {
		select {
		case <-g.stopping:
			return false
		case <-timeout.C:
			return g.authorize(nil)
		case <-tick.C:
			if remote := c.RemoteAddr(); remote != nil {
				return g.authorize(remote)
			}
		}
	}
}

func (g *GameServer) onUDP(port uint16) func(tailcat.ConnPacketConn) {
	if g.expired() {
		return nil
	}
	select {
	case <-g.stopping:
		return nil
	default:
	}
	if port != g.opts.TunnelPort || (g.opts.ConsoleBridge == nil && g.opts.Protocol != games.UDP) {
		return nil
	}
	return g.handleUDP
}

func (g *GameServer) handleUDP(c tailcat.ConnPacketConn) {
	g.wg.Add(1)
	defer g.wg.Done()
	defer c.Close()
	if !g.authorize(c.RemoteAddr()) {
		return
	}
	if !g.conns.add(c) {
		return
	}
	defer g.conns.remove(c)

	// A Factorio browser or GameLink check can ping before sending gameplay
	// traffic. Inspect the first datagram to keep these flows out of the
	// gameplay limit and out of the peer's last-connected timestamp.
	var buf []byte
	var n int
	first := g.opts.Game == games.Factorio && g.opts.ConsoleBridge == nil
	probe := false
	if first {
		buf = make([]byte, readBufSize)
		var err error
		n, err = c.Read(buf)
		if err != nil {
			return
		}
		probe = isFactorioPing(buf[:n])
	}
	if probe {
		const maxFactorioProbeFlows = 256
		if g.probes.Add(1) > maxFactorioProbeFlows {
			g.probes.Add(-1)
			g.overloaded.Add(1)
			return
		}
	} else if !g.admit() {
		return
	}
	defer func() {
		if probe {
			g.probes.Add(-1)
		} else {
			g.active.Add(-1)
		}
	}()

	if g.opts.ConsoleBridge != nil {
		g.opts.ConsoleBridge.HandleUDP(c)
		return
	}

	// One connected socket per tunnel flow so the game server sees each
	// client as a distinct endpoint, as it would on a LAN.
	dst, err := net.DialUDP("udp", nil, net.UDPAddrFromAddrPort(g.opts.Dest))
	if err != nil {
		g.dialFails.Add(1)
		g.dialLog.do(func() { g.logf("warning", "cannot open a socket to "+g.opts.Dest.String()+": "+err.Error()) })
		return
	}
	defer dst.Close()
	if !g.conns.add(dst) {
		return
	}
	defer g.conns.remove(dst)

	if !probe {
		g.opts.Peers.recordSession(g.allowed, c.RemoteAddr())
	}
	done := make(chan struct{})
	go func() { // game server -> client
		defer close(done)
		buf := make([]byte, readBufSize)
		for {
			n, err := dst.Read(buf)
			if err != nil {
				if isTransientUDPErr(err) {
					continue
				}
				c.Close()
				return
			}
			if n > MaxUDPPayload {
				// Never truncate, never fragment: drop and say so.
				g.oversize.Add(1)
				g.sizeLog.do(func() {
					g.logf("warning", fmt.Sprintf("dropped a %d-byte datagram from the game server: Tailcat carries at most %d bytes per datagram", n, MaxUDPPayload))
				})
				continue
			}
			if _, err := c.Write(buf[:n]); err != nil {
				dst.Close()
				return
			}
		}
	}()
	if buf == nil {
		buf = make([]byte, readBufSize)
	}
	for { // client -> game server
		if first {
			first = false
		} else {
			n, err = c.Read(buf)
			if err != nil {
				break
			}
		}
		if probe {
			if isFactorioPing(buf[:n]) {
				_ = c.SetReadDeadline(time.Now().Add(2 * factorioProbeTimeout))
			} else {
				if !g.admit() {
					break
				}
				probe = false
				g.probes.Add(-1)
				_ = c.SetReadDeadline(time.Time{})
				g.opts.Peers.recordSession(g.allowed, c.RemoteAddr())
			}
		}
		if _, err := dst.Write(buf[:n]); err != nil && !isTransientUDPErr(err) {
			break
		}
	}
	dst.Close()
	<-done
}
