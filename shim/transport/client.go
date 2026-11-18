package transport

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/netip"
	"os"
	"slices"
	"sync"
	"sync/atomic"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/ipn/ipnstate"
	"tailscale.com/types/key"
	"tailscale.com/types/logger"

	"github.com/gamelink-project/gamelink/shim/bedrock"
	"github.com/gamelink-project/gamelink/shim/games"
	"golang.org/x/net/netutil"
)

// State is the lifecycle state of a tunnel.
type State string

const (
	StateConnecting   State = "connecting"
	StateConnected    State = "connected"
	StateReconnecting State = "reconnecting"
	// StateUnreachable means the server did not answer. Tailcat deliberately
	// ignores devices that are not approved, so "server offline" and "this
	// device is not (or no longer) approved" look identical from here.
	StateUnreachable State = "unreachable"
	StateStopped     State = "stopped"
	StateError       State = "error"
)

// Path says how traffic currently reaches the server.
type Path string

const (
	PathUnknown Path = "unknown"
	PathDirect  Path = "direct"
	PathRelay   Path = "relay"
)

// ErrNoResponse is returned when the server never answered the handshake.
var ErrNoResponse = errors.New("no response from the server: it may be offline, or this device has not been approved for this game")

// Status is a snapshot of a tunnel. Every field reflects something actually
// measured or observed; unknown values are left at their zero value with the
// matching *Known flag false.
type Status struct {
	State           State     `json:"state"`
	Message         string    `json:"message"`
	LocalAddr       string    `json:"localAddr"` // e.g. 127.0.0.1:25565, only while bound
	LocalPort       int       `json:"localPort"`
	PortFallback    bool      `json:"portFallback"` // the preferred port was busy
	Path            Path      `json:"path"`
	Relay           string    `json:"relay"` // relay region code when relayed
	LatencyMS       float64   `json:"latencyMs"`
	LatencyKnown    bool      `json:"latencyKnown"`
	GameReachable   string    `json:"gameReachable"` // "yes", "no" or "unknown"
	Since           time.Time `json:"since"`
	ActiveConns     int       `json:"activeConns"`
	UDPFlows        int       `json:"udpFlows"`
	Reconnects      int       `json:"reconnects"`
	OversizeDrops   int64     `json:"oversizeDrops"`
	BytesToServer   int64     `json:"bytesToServer"`
	BytesFromServer int64     `json:"bytesFromServer"`
}

// TunnelOptions configure a client tunnel.
type TunnelOptions struct {
	Name       string
	Game       games.Type
	Proto      games.Protocol
	Addr       tailcat.Addr
	TunnelPort uint16
	// LocalPort, if non-zero, is the exact loopback port to use; the join
	// fails if it is busy. If zero, PreferPort is tried first, then any free
	// port.
	LocalPort  uint16
	PreferPort uint16
	// ReservedPorts belong to other saved desktop servers. A temporary
	// fallback must not route those servers' saved addresses to this game.
	ReservedPorts []uint16
	Identity      key.NodePrivate
	// AllowLocalRelay permits relay nodes on private networks for LAN testing.
	// Relay DNS answers are still pinned before each Tailcat client is started.
	AllowLocalRelay bool

	Logf          logger.Logf // Tailcat log output; must already be redacted
	OnStatus      func(Status)
	AutoReconnect bool

	HealthInterval time.Duration // default 5s
	ConnectTimeout time.Duration // default 25s
	DialTimeout    time.Duration // default 10s
	UDPIdle        time.Duration // default 2 min
}

const (
	maxLocalConns = 256
	maxUDPFlows   = 64
)

// Tunnel forwards loopback game traffic through Tailcat. Bedrock additionally
// owns a TCP signaling listener and isolated negotiated media sessions.
type Tunnel struct {
	opts   TunnelOptions
	ctx    context.Context
	cancel context.CancelFunc

	mu     sync.RWMutex
	cl     *tailcat.Client
	status Status

	tcpLn      net.Listener
	udpPC      *net.UDPConn
	bedrock    *bedrock.Client
	httpServer *http.Server
	conns      *connSet
	tcpSlots   chan struct{}
	wg         sync.WaitGroup
	kick       chan struct{}

	flowsMu sync.Mutex
	flows   map[netip.AddrPort]*udpFlow

	factorio factorioProbe // used by Join until healthLoop takes ownership

	dialFails  atomic.Int64 // consecutive tunnel dial failures
	oversize   atomic.Int64
	bytesUp    atomic.Int64
	bytesDown  atomic.Int64
	stopOnce   sync.Once
	oversizeLg rateLimited
}

// Join connects to a game server and exposes it on a loopback port. It returns
// only after the agent answered the handshake and a path was measured. TCP
// games and Bedrock also require an authorized tunnel-port connection. Actual
// game endpoint availability is reported separately in GameReachable.
func Join(parent context.Context, opts TunnelOptions) (*Tunnel, error) {
	if !opts.Proto.Valid() {
		return nil, fmt.Errorf("invalid protocol %q", opts.Proto)
	}
	if opts.TunnelPort == 0 {
		return nil, errors.New("tunnel port is required")
	}
	if opts.Identity.IsZero() {
		return nil, errors.New("device identity is required")
	}
	if opts.HealthInterval <= 0 {
		opts.HealthInterval = 5 * time.Second
	}
	if opts.ConnectTimeout <= 0 {
		opts.ConnectTimeout = 25 * time.Second
	}
	if opts.DialTimeout <= 0 {
		opts.DialTimeout = 10 * time.Second
	}
	if opts.UDPIdle <= 0 {
		opts.UDPIdle = tailcat.DefaultUDPIdleTimeout
	}
	ctx, cancel := context.WithCancel(context.Background())
	t := &Tunnel{
		opts: opts, ctx: ctx, cancel: cancel, conns: newConnSet(),
		tcpSlots: make(chan struct{}, maxLocalConns),
		kick:     make(chan struct{}, 1), flows: map[netip.AddrPort]*udpFlow{},
		oversizeLg: rateLimited{interval: 10 * time.Second},
	}
	t.setStatus(func(s *Status) {
		s.State = StateConnecting
		s.Message = "Connecting…"
		s.GameReachable = "unknown"
		s.Path = PathUnknown
	})
	if opts.Game == games.MinecraftBedrock {
		if opts.Proto != games.UDP {
			cancel()
			return nil, errors.New("Bedrock requires UDP gameplay")
		}
		t.bedrock = bedrock.NewClient(ctx, func(ctx context.Context) (net.Conn, error) {
			cl := t.client()
			if cl == nil {
				return nil, errors.New("agent disconnected")
			}
			return cl.DialTCPPort(ctx, opts.TunnelPort)
		})
		t.httpServer = &http.Server{Handler: t.bedrock, ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 30 * time.Second, WriteTimeout: 30 * time.Second, IdleTimeout: 30 * time.Second, MaxHeaderBytes: 8192}
	}

	// Bind first so a port conflict fails immediately, before any network use.
	if err := t.bind(); err != nil {
		t.fail(StateError, err.Error())
		cancel()
		return nil, err
	}

	cctx, ccancel := context.WithTimeout(parent, opts.ConnectTimeout)
	defer ccancel()
	cl, m, err := t.connectClient(cctx, true)
	if err != nil {
		t.closeSockets()
		state := StateError
		if errors.Is(err, ErrNoResponse) {
			state = StateUnreachable
		}
		t.fail(state, err.Error())
		cancel()
		return nil, err
	}
	t.mu.Lock()
	t.cl = cl
	t.mu.Unlock()
	t.applyMeasurement(m)
	t.setStatus(func(s *Status) {
		s.State = StateConnected
		s.Message = connectedMessage(m.gameReachable)
		s.Since = time.Now()
	})

	t.wg.Add(2)
	go func() { defer t.wg.Done(); t.serve() }()
	go func() { defer t.wg.Done(); t.healthLoop() }()
	return t, nil
}

// Status returns a snapshot.
func (t *Tunnel) Status() Status {
	t.mu.RLock()
	s := t.status
	t.mu.RUnlock()
	s.ActiveConns = t.conns.len()
	s.UDPFlows = t.flowCount()
	s.OversizeDrops = t.oversize.Load()
	s.BytesToServer = t.bytesUp.Load()
	s.BytesFromServer = t.bytesDown.Load()
	return s
}

// Stop tears the tunnel down and releases the local port and all connections.
func (t *Tunnel) Stop() {
	t.stopOnce.Do(func() {
		t.cancel()
		t.closeSockets()
		t.conns.closeAll()
		t.closeFlows()
		t.mu.Lock()
		cl := t.cl
		t.cl = nil
		t.mu.Unlock()
		if cl != nil {
			_ = cl.Close()
		}
		done := make(chan struct{})
		go func() { t.wg.Wait(); close(done) }()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
		}
		t.setStatus(func(s *Status) {
			s.State = StateStopped
			s.Message = "Disconnected"
			s.LocalAddr, s.LocalPort = "", 0
		})
	})
}

// Done is closed when the tunnel has been stopped.
func (t *Tunnel) Done() <-chan struct{} { return t.ctx.Done() }

func (t *Tunnel) setStatus(f func(*Status)) {
	t.mu.Lock()
	f(&t.status)
	s := t.status
	t.mu.Unlock()
	if t.opts.OnStatus != nil {
		s.ActiveConns = t.conns.len()
		t.opts.OnStatus(s)
	}
}

func (t *Tunnel) fail(state State, msg string) {
	t.setStatus(func(s *Status) {
		s.State = state
		s.Message = msg
		s.LocalAddr, s.LocalPort = "", 0
	})
}

func (t *Tunnel) client() *tailcat.Client {
	t.mu.RLock()
	defer t.mu.RUnlock()
	return t.cl
}

func (t *Tunnel) kickHealth() {
	select {
	case t.kick <- struct{}{}:
	default:
	}
}

// ---- local sockets -------------------------------------------------------

func (t *Tunnel) bind() error {
	loop := netip.AddrFrom4([4]byte{127, 0, 0, 1})
	reserved := func(requested, bound int) bool {
		return requested == 0 && slices.Contains(t.opts.ReservedPorts, uint16(bound))
	}
	reservedError := errors.New("temporary port belongs to another saved server")
	try := func(port int) (bool, error) {
		switch t.opts.Proto {
		case games.TCP:
			ln, err := net.Listen("tcp", netip.AddrPortFrom(loop, uint16(port)).String())
			if err != nil {
				return false, err
			}
			boundPort := ln.Addr().(*net.TCPAddr).Port
			if reserved(port, boundPort) {
				ln.Close()
				return false, reservedError
			}
			t.tcpLn = ln
			t.noteBound(boundPort)
		default:
			pc, err := net.ListenUDP("udp", net.UDPAddrFromAddrPort(netip.AddrPortFrom(loop, uint16(port))))
			if err != nil {
				return false, err
			}
			boundPort := pc.LocalAddr().(*net.UDPAddr).Port
			if reserved(port, boundPort) {
				pc.Close()
				return false, reservedError
			}
			if t.opts.Game == games.MinecraftBedrock {
				ln, err := net.Listen("tcp4", netip.AddrPortFrom(loop, uint16(boundPort)).String())
				if err != nil {
					pc.Close()
					return false, err
				}
				t.tcpLn = ln
			}
			t.udpPC = pc
			t.noteBound(boundPort)
		}
		return true, nil
	}
	if t.opts.LocalPort != 0 {
		if _, err := try(int(t.opts.LocalPort)); err != nil {
			return fmt.Errorf("local port %d is already in use on this PC (or not allowed); pick a different local port", t.opts.LocalPort)
		}
		return nil
	}
	if t.opts.PreferPort != 0 {
		if ok, _ := try(int(t.opts.PreferPort)); ok {
			return nil
		}
		defer t.setStatus(func(s *Status) { s.PortFallback = true })
	}
	var err error
	for i := 0; i < 16; i++ {
		if _, err = try(0); err == nil {
			return nil
		}
	}
	return fmt.Errorf("could not open a local port: %w", err)
}

func (t *Tunnel) noteBound(port int) {
	t.mu.Lock()
	t.status.LocalPort = port
	t.status.LocalAddr = netip.AddrPortFrom(netip.AddrFrom4([4]byte{127, 0, 0, 1}), uint16(port)).String()
	t.mu.Unlock()
}

func (t *Tunnel) closeSockets() {
	if t.httpServer != nil {
		_ = t.httpServer.Close()
	}
	if t.bedrock != nil {
		t.bedrock.Close()
	}
	if t.tcpLn != nil {
		_ = t.tcpLn.Close()
	}
	if t.udpPC != nil {
		_ = t.udpPC.Close()
	}
}

// ---- connecting and health ----------------------------------------------

type measurement struct {
	latency       time.Duration
	latencyKnown  bool
	path          Path
	relay         string
	gameReachable string
}

func (t *Tunnel) newClient(addr tailcat.Addr) *tailcat.Client {
	return &tailcat.Client{Server: addr, Key: t.opts.Identity, Logf: orDiscard(t.opts.Logf)}
}

// connectClient builds a fresh Tailcat client and proves it works: the server
// must answer the handshake (which only approved devices get), then a path
// measurement is taken and, when probe is set, the game endpoint is checked.
func (t *Tunnel) connectClient(ctx context.Context, probe bool) (*tailcat.Client, measurement, error) {
	m := measurement{path: PathUnknown, gameReachable: "unknown"}
	addr, err := PinRelayAddress(ctx, t.opts.Addr, t.opts.AllowLocalRelay)
	if err != nil {
		return nil, m, err
	}
	cl := t.newClient(addr)
	if _, err := cl.Ping(ctx); err != nil {
		_ = cl.Close()
		if ctx.Err() != nil && errors.Is(ctx.Err(), context.Canceled) {
			return nil, m, ctx.Err()
		}
		return nil, m, ErrNoResponse
	}
	m = measure(ctx, cl)
	if probe {
		m.gameReachable, err = probeEndpoint(ctx, cl, t.opts, &t.factorio)
		if err != nil {
			t.factorio.close()
			_ = cl.Close()
			return nil, m, fmt.Errorf("reached the agent but could not open the game port through the tunnel: %w", err)
		}
	}
	return cl, m, nil
}

// probeGame inspects a freshly opened tunnel connection. The server accepts
// the tunnel connection before it dials the game, so an immediate EOF or reset
// means the game server refused it. Silence cannot distinguish a pending
// upstream dial from a game waiting for the client to speak first.
func probeGame(c net.Conn) string {
	_ = c.SetReadDeadline(time.Now().Add(600 * time.Millisecond))
	var b [1]byte
	_, err := c.Read(b[:])
	var ne net.Error
	switch {
	case err == nil:
		return "yes"
	case errors.Is(err, os.ErrDeadlineExceeded), errors.As(err, &ne) && ne.Timeout():
		return "unknown"
	case errors.Is(err, io.EOF), errors.Is(err, net.ErrClosed):
		return "no"
	default:
		return "no"
	}
}

func measure(ctx context.Context, cl *tailcat.Client) measurement {
	m := measurement{path: PathUnknown, gameReachable: "unknown"}
	// A direct path can take a moment to be discovered; ping a few times and
	// keep the best answer. Every value below comes from a real ping result.
	for i := 0; i < 4; i++ {
		pctx, cancel := context.WithTimeout(ctx, 3*time.Second)
		res, err := cl.DiscoPing(pctx)
		cancel()
		if err != nil {
			if i == 0 {
				continue
			}
			break
		}
		applyPing(&m, res)
		if m.path == PathDirect {
			break
		}
		select {
		case <-ctx.Done():
			return m
		case <-time.After(400 * time.Millisecond):
		}
	}
	return m
}

func applyPing(m *measurement, res *ipnstate.PingResult) {
	m.latency = time.Duration(res.LatencySeconds * float64(time.Second))
	m.latencyKnown = true
	switch {
	case res.Endpoint != "":
		m.path, m.relay = PathDirect, ""
	case res.PeerRelay != "":
		m.path, m.relay = PathRelay, "peer"
	case res.DERPRegionID != 0:
		m.path, m.relay = PathRelay, res.DERPRegionCode
	default:
		m.path = PathUnknown
	}
}

func (t *Tunnel) applyMeasurement(m measurement) {
	t.setStatus(func(s *Status) {
		s.Path, s.Relay = m.path, m.relay
		s.LatencyKnown = m.latencyKnown
		s.LatencyMS = float64(m.latency) / float64(time.Millisecond)
		s.GameReachable = m.gameReachable
	})
}

func (t *Tunnel) healthLoop() {
	defer t.factorio.close()
	tick := time.NewTicker(t.opts.HealthInterval)
	defer tick.Stop()
	fails := 0
	for {
		select {
		case <-t.ctx.Done():
			return
		case <-tick.C:
		case <-t.kick:
		}
		cl := t.client()
		if cl == nil {
			return
		}
		pctx, cancel := context.WithTimeout(t.ctx, 5*time.Second)
		res, err := cl.DiscoPing(pctx)
		cancel()
		if t.ctx.Err() != nil {
			return
		}
		if err == nil {
			fails = 0
			var m measurement
			applyPing(&m, res)
			m.gameReachable, _ = probeEndpoint(t.ctx, cl, t.opts, &t.factorio)
			t.setStatus(func(s *Status) {
				s.Path, s.Relay = m.path, m.relay
				s.LatencyKnown, s.LatencyMS = true, float64(m.latency)/float64(time.Millisecond)
				s.GameReachable = m.gameReachable
				s.Message = connectedMessage(m.gameReachable)
			})
			if t.dialFails.Load() < 2 {
				continue
			}
		} else {
			fails++
		}
		if fails >= 2 || t.dialFails.Load() >= 2 {
			fails = 0
			t.reconnect()
		}
	}
}

func (t *Tunnel) reconnect() {
	t.factorio.close()
	if !t.opts.AutoReconnect {
		// Give the local port back now rather than when the user dismisses
		// the error. Stop() can't be used here: this runs inside healthLoop,
		// which Stop() waits for.
		t.cancel()
		t.closeSockets()
		t.conns.closeAll()
		t.closeFlows()
		t.mu.Lock()
		cl := t.cl
		t.cl = nil
		t.mu.Unlock()
		if cl != nil {
			_ = cl.Close()
		}
		t.fail(StateError, "Connection lost")
		return
	}
	t.setStatus(func(s *Status) {
		s.State = StateReconnecting
		s.Message = "Connection interrupted. Reconnecting…"
		s.LatencyKnown = false
	})
	backoff := time.Second
	for attempt := 1; ; attempt++ {
		cctx, cancel := context.WithTimeout(t.ctx, t.opts.ConnectTimeout)
		cl, m, err := t.connectClient(cctx, true)
		cancel()
		if t.ctx.Err() != nil {
			if cl != nil {
				_ = cl.Close()
			}
			return
		}
		if err == nil {
			t.mu.Lock()
			old := t.cl
			t.cl = cl
			t.mu.Unlock()
			if old != nil {
				go old.Close()
			}
			t.dialFails.Store(0)
			t.applyMeasurement(m)
			t.setStatus(func(s *Status) {
				s.State = StateConnected
				s.Message = connectedMessage(m.gameReachable)
				s.Since = time.Now()
				s.Reconnects++
			})
			return
		}
		if attempt >= 3 {
			t.setStatus(func(s *Status) {
				s.State = StateUnreachable
				s.Message = ErrNoResponse.Error() + ". GameLink keeps retrying."
			})
		}
		select {
		case <-t.ctx.Done():
			return
		case <-time.After(backoff):
		}
		if backoff < 30*time.Second {
			backoff *= 2
		}
	}
}

// ---- serving -------------------------------------------------------------

func (t *Tunnel) serve() {
	if t.bedrock != nil {
		t.wg.Add(1)
		go func() { defer t.wg.Done(); _ = t.httpServer.Serve(netutil.LimitListener(t.tcpLn, maxLocalConns)) }()
		t.serveUDP()
		return
	}
	if t.tcpLn != nil {
		t.serveTCP()
	} else {
		t.serveUDP()
	}
}

func connectedMessage(reachable string) string {
	switch reachable {
	case "no":
		return "Connected, but server offline"
	case "yes":
		return "Connected"
	default:
		return "Agent connected; game status unknown"
	}
}

func (opts TunnelOptions) probesFactorio() bool {
	return opts.Game == games.Factorio && opts.Proto == games.UDP
}

func probeEndpoint(ctx context.Context, cl *tailcat.Client, opts TunnelOptions, factorio *factorioProbe) (string, error) {
	if opts.probesFactorio() {
		if factorio.ping(ctx, cl, opts.TunnelPort) {
			return "yes", nil
		}
		if errors.Is(ctx.Err(), context.Canceled) {
			return "unknown", ctx.Err()
		}
		// A failed UDP probe cannot distinguish a stopped game from packet
		// loss, an unsupported reply format, or an unavailable tunnel flow.
		return "unknown", nil
	}
	if opts.Game != games.MinecraftBedrock && opts.Proto != games.TCP {
		return "unknown", nil
	}
	pctx, cancel := context.WithTimeout(ctx, 8*time.Second)
	defer cancel()
	c, err := cl.DialTCPPort(pctx, opts.TunnelPort)
	if err != nil {
		return "no", err
	}
	if opts.Game == games.MinecraftBedrock {
		if bedrock.Probe(pctx, c) {
			return "yes", nil
		}
		return "no", ctx.Err()
	}
	defer c.Close()
	reachable := probeGame(c)
	return reachable, ctx.Err()
}

// CheckEndpoint separates authenticated agent reachability from the actual
// game endpoint. Generic UDP has no safe universal probe and stays unknown.
func CheckEndpoint(ctx context.Context, opts TunnelOptions) (time.Duration, Path, string, error) {
	pinned, err := PinRelayAddress(ctx, opts.Addr, opts.AllowLocalRelay)
	if err != nil {
		return 0, PathUnknown, "unknown", err
	}
	cl := &tailcat.Client{Server: pinned, Key: opts.Identity, Logf: orDiscard(opts.Logf)}
	defer cl.Close()
	if _, err := cl.Ping(ctx); err != nil {
		if ctx.Err() != nil {
			return 0, PathUnknown, "unknown", ctx.Err()
		}
		return 0, PathUnknown, "unknown", ErrNoResponse
	}
	m := measure(ctx, cl)
	var factorio factorioProbe
	defer factorio.close()
	reachable, _ := probeEndpoint(ctx, cl, opts, &factorio)
	if err := ctx.Err(); err != nil {
		// The agent already answered. Expiring the overall check budget
		// during the Factorio probe still leaves the game status inconclusive.
		if !opts.probesFactorio() || !errors.Is(err, context.DeadlineExceeded) {
			return m.latency, m.path, "unknown", err
		}
	}
	return m.latency, m.path, reachable, nil
}

// Check reports whether the agent answers an approved device, and over which
// path, without opening a local port. Use CheckEndpoint for game availability.
func Check(ctx context.Context, addr tailcat.Addr, id key.NodePrivate, logf logger.Logf) (latency time.Duration, path Path, err error) {
	return CheckWithRelayPolicy(ctx, addr, id, logf, false)
}

// CheckWithRelayPolicy checks a Tailcat address after resolving and pinning
// every relay node under the same policy used by Join.
func CheckWithRelayPolicy(ctx context.Context, addr tailcat.Addr, id key.NodePrivate, logf logger.Logf, allowLocal bool) (latency time.Duration, path Path, err error) {
	pinned, err := PinRelayAddress(ctx, addr, allowLocal)
	if err != nil {
		return 0, PathUnknown, err
	}
	cl := &tailcat.Client{Server: pinned, Key: id, Logf: orDiscard(logf)}
	defer cl.Close()
	if _, err := cl.Ping(ctx); err != nil {
		if ctx.Err() != nil && errors.Is(ctx.Err(), context.Canceled) {
			return 0, PathUnknown, ctx.Err()
		}
		return 0, PathUnknown, ErrNoResponse
	}
	m := measure(ctx, cl)
	return m.latency, m.path, nil
}
