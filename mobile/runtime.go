package mobile

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
	"net/netip"
	"strings"
	"sync"
	"time"

	"github.com/gamelink-project/gamelink/shim/consolebridge"
	"github.com/gamelink-project/gamelink/shim/invite"
	"github.com/gamelink-project/gamelink/shim/security"
	"github.com/gamelink-project/gamelink/shim/transport"
	"github.com/tailscale/tailcat"
	"tailscale.com/types/key"
)

const runtimeDialTimeout = 25 * time.Second

type runtimeConfig struct {
	Invitation      string `json:"invitation"`
	LANIPv4         string `json:"lanIPv4"`
	LANPrefix       int    `json:"lanPrefix"`
	InterfaceIndex  int    `json:"interfaceIndex"`
	AllowLocalRelay bool   `json:"allowLocalRelay"`
}

type runtimeStatus struct {
	State    string `json:"state"`
	Message  string `json:"message"`
	Code     string `json:"code,omitempty"`
	Stage    string `json:"stage"`
	Path     string `json:"path"`
	Sessions int    `json:"sessions"`
}

// Runtime owns a cancellable bridge connection. It never persists secrets.
type Runtime struct {
	mu      sync.Mutex
	cancel  context.CancelFunc
	done    chan struct{}
	status  runtimeStatus
	client  *tailcat.Client
	control net.Conn
	media   tailcat.ConnPacketConn
	bridge  *consolebridge.Client
	stopped bool
	stop    sync.Once
}

// Start validates inputs synchronously and starts the connection asynchronously.
// It returns before dialing so callers can always Stop a pending connection.
func Start(configJSON string, privateKey []byte) (*Runtime, error) {
	if len(configJSON) > 16384 {
		return nil, errors.New("invalid bridge configuration")
	}
	var cfg runtimeConfig
	d := json.NewDecoder(strings.NewReader(configJSON))
	d.DisallowUnknownFields()
	if err := d.Decode(&cfg); err != nil {
		return nil, errors.New("invalid bridge configuration")
	}
	var extra any
	if d.Decode(&extra) != io.EOF {
		return nil, errors.New("invalid bridge configuration")
	}
	ip, err := netip.ParseAddr(cfg.LANIPv4)
	if err != nil || !validMobileLAN(ip, cfg.AllowLocalRelay) {
		return nil, errors.New("a private local IPv4 address is required")
	}
	if !((cfg.LANPrefix == 0 && cfg.InterfaceIndex == 0) || (cfg.LANPrefix > 0 && cfg.LANPrefix <= 32 && cfg.InterfaceIndex > 0)) {
		return nil, errors.New("invalid selected network")
	}
	prefix, index, err := consolebridge.ResolveLAN(ip, cfg.LANPrefix, cfg.InterfaceIndex)
	if err != nil || !prefix.Contains(ip) || index <= 0 {
		return nil, errors.New("selected Wi-Fi network is unavailable")
	}
	inv, _, err := invite.Parse(cfg.Invitation, invite.Options{AllowLocalRelay: cfg.AllowLocalRelay})
	if err != nil {
		return nil, errors.New("invalid invitation")
	}
	if inv.ConsoleBridge != 1 {
		return nil, errors.New("unsupported_bridge")
	}
	var priv key.NodePrivate
	if err := priv.UnmarshalText(append([]byte(nil), privateKey...)); err != nil || priv.IsZero() {
		return nil, errors.New("invalid device private key")
	}
	ctx, cancel := context.WithCancel(context.Background())
	clearPlatform := setPlatformLANInterface(ip, prefix, index)
	r := &Runtime{
		cancel: cancel,
		done:   make(chan struct{}),
		status: runtimeStatus{State: "connecting", Message: "Connecting", Path: "unknown"},
	}
	go r.run(ctx, inv, priv, ip, prefix, index, cfg.AllowLocalRelay, clearPlatform)
	return r, nil
}

func validMobileLAN(ip netip.Addr, allowLoopback bool) bool {
	if !ip.Is4() || ip.IsUnspecified() || ip.IsMulticast() || ip.IsLinkLocalUnicast() || ip == netip.MustParseAddr("255.255.255.255") {
		return false
	}
	if ip.IsLoopback() {
		return allowLoopback
	}
	return ip.IsPrivate() && !security.IsPublicIP(ip)
}

func (r *Runtime) run(ctx context.Context, inv invite.Invite, priv key.NodePrivate, ip netip.Addr, prefix netip.Prefix, index int, allowLocal bool, clearPlatform func()) {
	defer close(r.done)
	defer clearPlatform()
	backoff := time.Second
	wasReady := false
	for ctx.Err() == nil {
		if wasReady {
			r.setStatus("reconnecting", "Reconnecting", "unknown")
		}
		ready, err := r.runSession(ctx, inv, priv, ip, prefix, index, allowLocal)
		if ctx.Err() != nil {
			return
		}
		if errors.Is(err, consolebridge.ErrUnsupportedBridge) {
			r.setError("unsupported_bridge", "Host upgrade required for Console Bridge v1")
			return
		}
		if ready {
			wasReady = true
		}
		if wasReady {
			r.setStatus("reconnecting", "Connection lost; retrying", "unknown")
		} else {
			// Tailcat deliberately makes an offline host and an unapproved device
			// indistinguishable. Keep that ambiguity in the user-facing status.
			r.setStatus("unreachable", "Server is unavailable or this device is not approved", "unknown")
		}
		if err == nil {
			backoff = time.Second
		}
		timer := time.NewTimer(backoff)
		select {
		case <-ctx.Done():
			timer.Stop()
			return
		case <-timer.C:
		}
		if backoff < 30*time.Second {
			backoff *= 2
			if backoff > 30*time.Second {
				backoff = 30 * time.Second
			}
		}
	}
}

func (r *Runtime) runSession(parent context.Context, inv invite.Invite, priv key.NodePrivate, ip netip.Addr, prefix netip.Prefix, index int, allowLocal bool) (ready bool, retErr error) {
	dialCtx, dialCancel := context.WithTimeout(parent, runtimeDialTimeout)
	r.setStage("relay_resolution")
	addr, err := transport.PinRelayAddress(dialCtx, tailcat.Addr(inv.Addr), allowLocal)
	if err != nil {
		dialCancel()
		return false, err
	}
	cl := &tailcat.Client{Server: addr, Key: priv, Logf: func(string, ...any) {}}
	if !r.install(clientResources{client: cl}) {
		dialCancel()
		_ = cl.Close()
		return false, context.Canceled
	}
	defer func() {
		dialCancel()
		r.clear(clientResources{client: cl})
		_ = cl.Close()
	}()
	r.setStage("tailcat_tcp")
	control, err := cl.DialTCPPort(dialCtx, inv.Port)
	if err != nil {
		return false, err
	}
	if !r.install(clientResources{control: control}) {
		_ = control.Close()
		return false, context.Canceled
	}
	defer func() {
		r.clear(clientResources{control: control})
		_ = control.Close()
	}()
	r.setStage("tailcat_udp")
	media, err := cl.DialUDPPort(dialCtx, inv.Port)
	if err != nil {
		return false, err
	}
	if !r.install(clientResources{media: media}) {
		_ = media.Close()
		return false, context.Canceled
	}
	defer func() {
		r.clear(clientResources{media: media})
		_ = media.Close()
	}()
	dialCancel()

	r.setStage("lan_discovery")
	var readyMu sync.Mutex
	ready = false
	bridge, err := consolebridge.NewClient(parent, control, media, consolebridge.ClientOptions{
		LANIP: ip, Prefix: prefix, InterfaceIndex: index,
		OnReady: func() {
			readyMu.Lock()
			ready = true
			readyMu.Unlock()
			r.mu.Lock()
			if !r.stopped {
				r.status.State = "connected"
				r.status.Message = "Console bridge ready"
				r.status.Stage = "ready"
			}
			r.mu.Unlock()
		},
	})
	if err != nil {
		return false, err
	}
	if !r.install(clientResources{bridge: bridge}) {
		_ = bridge.Close()
		return false, context.Canceled
	}
	defer func() {
		r.clear(clientResources{bridge: bridge})
		_ = bridge.Close()
	}()

	r.setStage("bridge_advertisement")
	sessionCtx, sessionCancel := context.WithCancel(parent)
	pathDone := make(chan struct{})
	go r.measurePath(sessionCtx, cl, pathDone)
	err = bridge.Run()
	sessionCancel()
	<-pathDone
	readyMu.Lock()
	wasReady := ready
	readyMu.Unlock()
	return wasReady, err
}

type clientResources struct {
	client  *tailcat.Client
	control net.Conn
	media   tailcat.ConnPacketConn
	bridge  *consolebridge.Client
}

func (r *Runtime) install(v clientResources) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.stopped {
		return false
	}
	if v.client != nil {
		r.client = v.client
	}
	if v.control != nil {
		r.control = v.control
	}
	if v.media != nil {
		r.media = v.media
	}
	if v.bridge != nil {
		r.bridge = v.bridge
	}
	return true
}

func (r *Runtime) clear(v clientResources) {
	r.mu.Lock()
	if v.client != nil && r.client == v.client {
		r.client = nil
	}
	if v.control != nil && r.control == v.control {
		r.control = nil
	}
	if v.media != nil && r.media == v.media {
		r.media = nil
	}
	if v.bridge != nil && r.bridge == v.bridge {
		r.bridge = nil
	}
	r.mu.Unlock()
}

func (r *Runtime) measurePath(ctx context.Context, cl *tailcat.Client, done chan<- struct{}) {
	defer close(done)
	tick := time.NewTicker(15 * time.Second)
	defer tick.Stop()
	for {
		pctx, cancel := context.WithTimeout(ctx, 2*time.Second)
		res, err := cl.DiscoPing(pctx)
		cancel()
		if err == nil && res != nil {
			path := "unknown"
			if res.Endpoint != "" {
				path = "direct"
			} else if res.DERPRegionID != 0 || res.PeerRelay != "" {
				path = "relay"
			}
			r.mu.Lock()
			if !r.stopped && r.status.State == "connected" {
				r.status.Path = path
			}
			r.mu.Unlock()
		}
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		}
	}
}

func (r *Runtime) statusPath() string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.status.Path
}

func (r *Runtime) setStatus(state, message, path string) {
	r.mu.Lock()
	if !r.stopped {
		r.status.State = state
		r.status.Message = message
		r.status.Code = ""
		r.status.Path = path
	}
	r.mu.Unlock()
}

func (r *Runtime) setError(code, message string) {
	r.mu.Lock()
	if !r.stopped {
		r.status.State = "error"
		r.status.Message = message
		r.status.Code = code
	}
	r.mu.Unlock()
}

func (r *Runtime) setStage(stage string) {
	r.mu.Lock()
	if !r.stopped {
		r.status.Stage = stage
	}
	r.mu.Unlock()
}

// Stop cancels a pending connection, closes all active resources, and waits
// briefly for the owner goroutine to finish cleanup. It is safe to call twice.
func (r *Runtime) Stop() {
	r.stop.Do(func() {
		r.mu.Lock()
		r.stopped = true
		r.status = runtimeStatus{State: "stopped", Message: "Stopped", Stage: "stopped", Path: "unknown"}
		if r.cancel != nil {
			r.cancel()
		}
		bridge, control, media, client := r.bridge, r.control, r.media, r.client
		r.bridge, r.control, r.media, r.client = nil, nil, nil, nil
		r.mu.Unlock()
		if bridge != nil {
			_ = bridge.Close()
		}
		if control != nil {
			_ = control.Close()
		}
		if media != nil {
			_ = media.Close()
		}
		if client != nil {
			_ = client.Close()
		}
	})
	if r.done != nil {
		select {
		case <-r.done:
		case <-time.After(5 * time.Second):
		}
	}
}

// Status returns a sanitized JSON status snapshot.
func (r *Runtime) Status() string {
	r.mu.Lock()
	status := r.status
	bridge := r.bridge
	r.mu.Unlock()
	if bridge != nil {
		status.Sessions = bridge.SessionCount()
	}
	b, _ := json.Marshal(status)
	return string(b)
}
