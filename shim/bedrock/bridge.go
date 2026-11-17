// Package bedrock relays manual-entry NetherNet HTTP signaling and opaque UDP
// media. DTLS, identity assertions and game payloads remain end-to-end.
package bedrock

import (
	"bytes"
	"context"
	"encoding/binary"
	"encoding/json"
	"errors"
	"io"
	"net"
	"net/http"
	"net/netip"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/gamelink-project/gamelink/shim/rustcore"
	"golang.org/x/time/rate"
)

const (
	maxBody          = 65536
	maxFrame         = 128 * 1024
	maxPacket        = 65507
	signalingTimeout = 25 * time.Second
	mediaIdle        = 2 * time.Minute
)

// A candidate placeholder is meaningful only inside the authenticated stream.
// Neither side sends UDP to it; it is replaced by that side's real relay.
var placeholder = netip.MustParseAddrPort("127.0.0.1:9")

type request struct {
	Method, URI, Accept, ContentType, UserAgent string
	Body                                        []byte
}
type response struct {
	Status      int
	ContentType string
	Body        []byte
}

func writeFrame(w io.Writer, data []byte) error {
	if len(data) == 0 || len(data) > maxFrame {
		return errors.New("invalid Bedrock frame length")
	}
	var header [4]byte
	binary.BigEndian.PutUint32(header[:], uint32(len(data)))
	for _, part := range [][]byte{header[:], data} {
		for len(part) > 0 {
			n, err := w.Write(part)
			if err != nil {
				return err
			}
			if n <= 0 {
				return io.ErrShortWrite
			}
			part = part[n:]
		}
	}
	return nil
}
func readFrame(r io.Reader, max int) ([]byte, error) {
	var header [4]byte
	if _, err := io.ReadFull(r, header[:]); err != nil {
		return nil, err
	}
	n := binary.BigEndian.Uint32(header[:])
	if n == 0 || n > uint32(max) {
		return nil, errors.New("invalid Bedrock frame length")
	}
	data := make([]byte, n)
	_, err := io.ReadFull(r, data)
	return data, err
}
func writeMessage(w io.Writer, v any) error {
	data, err := json.Marshal(v)
	if err != nil {
		return err
	}
	return writeFrame(w, data)
}
func readMessage(r io.Reader, v any) error {
	data, err := readFrame(r, maxFrame)
	if err != nil {
		return err
	}
	return json.Unmarshal(data, v)
}
func validRequest(r request) bool {
	if len(r.URI) > 4096 || len(r.Body) > maxBody || len(r.Accept)+len(r.ContentType)+len(r.UserAgent) > 8192 {
		return false
	}
	u, err := url.ParseRequestURI(r.URI)
	if err != nil || u.IsAbs() || u.Host != "" {
		return false
	}
	return (r.Method == http.MethodGet && u.Path == "/v1/join" && len(r.Body) == 0) ||
		(r.Method == http.MethodPost && strings.HasPrefix(u.Path, "/v1/join/") && len(u.Path) > len("/v1/join/"))
}
func readBody(r io.Reader) ([]byte, error) {
	b, err := io.ReadAll(io.LimitReader(r, maxBody+1))
	if err != nil || len(b) > maxBody {
		return nil, errors.New("Bedrock signaling body exceeds limit")
	}
	return b, nil
}
func success(r response) bool { return r.Status >= 200 && r.Status < 300 }
func numericError(b []byte) bool {
	_, err := strconv.ParseUint(strings.TrimSpace(string(b)), 10, 32)
	return err == nil
}

// mapSDP uses the same bounded, byte-preserving Rust candidate policy as the
// console bridge. Only candidates at socket-owner-approved addresses survive.
func mapSDP(data []byte, allowed []string, first, last uint16, relay netip.AddrPort) ([]byte, []netip.AddrPort, error) {
	var out struct {
		Data    []byte
		Targets []string
	}
	err := rustcore.Invoke("console.candidates_map", map[string]any{"data": data, "allowed": allowed, "first": first, "last": last, "relay": relay.String()}, &out)
	if err != nil || len(out.Targets) == 0 {
		return nil, nil, errors.New("no permitted IPv4 UDP media candidates")
	}
	var targets []netip.AddrPort
	for _, s := range out.Targets {
		a, err := netip.ParseAddrPort(s)
		if err != nil {
			return nil, nil, errors.New("invalid media candidate")
		}
		targets = append(targets, a)
	}
	return out.Data, targets, nil
}
func localIPs() ([]string, error) {
	addrs, err := net.InterfaceAddrs()
	if err != nil {
		return nil, err
	}
	ips := []string{"127.0.0.1"}
	for _, addr := range addrs {
		prefix, err := netip.ParsePrefix(addr.String())
		if err != nil || !prefix.Addr().Is4() || prefix.Addr().IsUnspecified() {
			continue
		}
		if len(ips) < 64 {
			ips = append(ips, prefix.Addr().String())
		}
	}
	return ips, nil
}

// Client serves HTTP on the desktop's loopback TCP listener. Each successful
// offer owns its authenticated stream and UDP socket until idle or disconnected.
type Client struct {
	ctx     context.Context
	dial    func(context.Context) (net.Conn, error)
	mu      sync.Mutex
	closed  bool
	streams map[net.Conn]bool
	slots   chan struct{}
	wg      sync.WaitGroup
}

func NewClient(ctx context.Context, dial func(context.Context) (net.Conn, error)) *Client {
	return &Client{ctx: ctx, dial: dial, streams: map[net.Conn]bool{}, slots: make(chan struct{}, 32)}
}
func (c *Client) Close() {
	c.mu.Lock()
	c.closed = true
	for stream := range c.streams {
		stream.Close()
	}
	c.mu.Unlock()
	c.wg.Wait()
}
func (c *Client) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	select {
	case c.slots <- struct{}{}:
	default:
		http.Error(w, "Bedrock relay busy", http.StatusServiceUnavailable)
		return
	}
	keep := false
	defer func() {
		if !keep {
			<-c.slots
		}
	}()
	req := request{Method: r.Method, URI: r.URL.RequestURI(), Accept: r.Header.Get("Accept"), ContentType: r.Header.Get("Content-Type"), UserAgent: r.UserAgent()}
	if !validRequest(req) {
		http.NotFound(w, r)
		return
	}
	var err error
	if r.Method == http.MethodPost {
		req.Body, err = readBody(r.Body)
		if err != nil {
			http.Error(w, "SDP offer too large or unreadable", http.StatusRequestEntityTooLarge)
			return
		}
	}
	var socket *net.UDPConn
	var targets []netip.AddrPort
	if r.Method == http.MethodPost {
		socket, err = net.ListenUDP("udp4", net.UDPAddrFromAddrPort(netip.MustParseAddrPort("127.0.0.1:0")))
		if err != nil {
			http.Error(w, "Bedrock media unavailable", 503)
			return
		}
		defer func() {
			if !keep {
				socket.Close()
			}
		}()
		ips, err := localIPs()
		if err == nil {
			req.Body, targets, err = mapSDP(req.Body, ips, 1, 65535, placeholder)
		}
		if err != nil {
			http.Error(w, "Offer has no supported local IPv4 UDP candidates", http.StatusBadRequest)
			return
		}
	}
	ctx, cancel := context.WithTimeout(r.Context(), signalingTimeout)
	defer cancel()
	stream, err := c.dial(ctx)
	if err != nil {
		http.Error(w, "GameLink agent signaling unavailable; update the host agent", 502)
		return
	}
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		stream.Close()
		http.Error(w, "GameLink disconnected", 503)
		return
	}
	c.streams[stream] = true
	c.wg.Add(1)
	c.mu.Unlock()
	cleanup := func() { stream.Close(); c.mu.Lock(); delete(c.streams, stream); c.mu.Unlock(); c.wg.Done() }
	defer func() {
		if !keep {
			cleanup()
		}
	}()
	_ = stream.SetDeadline(time.Now().Add(signalingTimeout))
	stop := context.AfterFunc(ctx, func() { stream.Close() })
	defer stop()
	var resp response
	if writeMessage(stream, req) != nil || readMessage(stream, &resp) != nil || resp.Status < 200 || resp.Status > 599 || len(resp.Body) > maxBody {
		http.Error(w, "Bedrock signaling unavailable; check the game server and host agent version", 502)
		return
	}
	if r.Method == http.MethodPost && success(resp) && !numericError(resp.Body) {
		resp.Body, _, err = mapSDP(resp.Body, []string{placeholder.Addr().String()}, placeholder.Port(), placeholder.Port(), socket.LocalAddr().(*net.UDPAddr).AddrPort())
		if err != nil {
			http.Error(w, "Invalid Bedrock SDP answer", 502)
			return
		}
		// Detach the media lifetime from this completed HTTP request.
		if !stop() || ctx.Err() != nil {
			http.Error(w, "Bedrock signaling timed out", 504)
			return
		}
		_ = stream.SetDeadline(time.Time{})
		keep = true
		go func() { defer cleanup(); defer func() { <-c.slots }(); relay(c.ctx, stream, socket, targets) }()
	}
	if resp.ContentType != "" {
		w.Header().Set("Content-Type", resp.ContentType)
	}
	w.WriteHeader(resp.Status)
	_, _ = w.Write(resp.Body)
}

// Host accepts only the configured signaling endpoint and media-port range.
// It is called after Tailcat's per-game authorization and concurrency checks.
type Host struct {
	dest        netip.AddrPort
	first, last uint16
	ctx         context.Context
	cancel      context.CancelFunc
	http        *http.Client
}

func NewHost(dest netip.AddrPort, first, last uint16) (*Host, error) {
	if !dest.IsValid() || !dest.Addr().Is4() || dest.Addr().IsUnspecified() || dest.Addr().IsMulticast() || dest.Addr() == netip.MustParseAddr("255.255.255.255") || dest.Port() == 0 {
		return nil, errors.New("desktop Bedrock requires a unicast IPv4 destination")
	}
	if first == 0 && last == 0 {
		first, last = dest.Port(), dest.Port()
	}
	if first == 0 || last < first || int(last)-int(first) >= 256 {
		return nil, errors.New("invalid Bedrock media port range")
	}
	ctx, cancel := context.WithCancel(context.Background())
	transport := &http.Transport{Proxy: nil, DisableCompression: true, MaxResponseHeaderBytes: 8192, DialContext: (&net.Dialer{Timeout: 5 * time.Second}).DialContext}
	return &Host{dest: dest, first: first, last: last, ctx: ctx, cancel: cancel, http: &http.Client{Transport: transport, Timeout: signalingTimeout, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}}, nil
}
func (h *Host) Close() { h.cancel(); h.http.CloseIdleConnections() }
func (h *Host) Handle(stream net.Conn) {
	defer stream.Close()
	_ = stream.SetDeadline(time.Now().Add(signalingTimeout))
	var req request
	if readMessage(stream, &req) != nil || !validRequest(req) {
		return
	}
	resp := response{Status: 502, ContentType: "text/plain", Body: []byte("Bedrock server signaling is offline or unavailable")}
	var socket *net.UDPConn
	var targets []netip.AddrPort
	if req.Method == http.MethodPost {
		// The routing socket picks a real server-facing interface, including
		// loopback for a BDS running alongside the agent.
		route, err := net.DialUDP("udp4", nil, net.UDPAddrFromAddrPort(h.dest))
		if err != nil {
			_ = writeMessage(stream, resp)
			return
		}
		bind := netip.AddrPortFrom(route.LocalAddr().(*net.UDPAddr).AddrPort().Addr(), 0)
		route.Close()
		socket, err = net.ListenUDP("udp4", net.UDPAddrFromAddrPort(bind))
		if err != nil {
			_ = writeMessage(stream, resp)
			return
		}
		defer socket.Close()
		req.Body, _, err = mapSDP(req.Body, []string{placeholder.Addr().String()}, placeholder.Port(), placeholder.Port(), socket.LocalAddr().(*net.UDPAddr).AddrPort())
		if err != nil {
			return
		}
	}
	ctx, cancel := context.WithTimeout(h.ctx, signalingTimeout)
	defer cancel()
	up, err := http.NewRequestWithContext(ctx, req.Method, "http://"+h.dest.String()+req.URI, bytes.NewReader(req.Body))
	if err != nil {
		return
	}
	for name, value := range map[string]string{"Accept": req.Accept, "Content-Type": req.ContentType, "User-Agent": req.UserAgent, "Accept-Encoding": "identity"} {
		if value != "" {
			up.Header.Set(name, value)
		}
	}
	answer, err := h.http.Do(up)
	if err == nil {
		body, readErr := readBody(answer.Body)
		answer.Body.Close()
		if readErr == nil && (answer.Header.Get("Content-Encoding") == "" || answer.Header.Get("Content-Encoding") == "identity") {
			resp = response{Status: answer.StatusCode, ContentType: answer.Header.Get("Content-Type"), Body: body}
			if socket != nil && success(resp) && !numericError(resp.Body) {
				resp.Body, targets, err = mapSDP(resp.Body, []string{h.dest.Addr().String()}, h.first, h.last, placeholder)
				if err != nil {
					resp = response{Status: 502, ContentType: "text/plain", Body: []byte("No allowed Bedrock UDP media candidates; configure --media-ports to match BDS server-udp-ports")}
					targets = nil
				}
			}
		}
	}
	if writeMessage(stream, resp) != nil {
		return
	}
	if len(targets) > 0 {
		_ = stream.SetDeadline(time.Time{})
		relay(h.ctx, stream, socket, targets)
	}
}

// Probe checks the actual BDS HTTP capability endpoint, not just the agent.
func Probe(ctx context.Context, stream net.Conn) bool {
	defer stream.Close()
	if deadline, ok := ctx.Deadline(); ok {
		_ = stream.SetDeadline(deadline)
	}
	stop := context.AfterFunc(ctx, func() { stream.Close() })
	defer stop()
	var resp response
	return writeMessage(stream, request{Method: http.MethodGet, URI: "/v1/join"}) == nil && readMessage(stream, &resp) == nil && success(resp)
}

// relay preserves datagram boundaries over one encrypted Tailcat stream per
// negotiation. Framing avoids Tailcat's 1232-byte UDP limit; TCP can introduce
// head-of-line delay on lossy paths. No STUN/DTLS/SCTP payload is interpreted.
func relay(ctx context.Context, stream net.Conn, socket *net.UDPConn, targets []netip.AddrPort) {
	defer socket.Close()
	defer stream.Close()
	stop := context.AfterFunc(ctx, func() { stream.Close(); socket.Close() })
	defer stop()
	var mu sync.Mutex
	var selected netip.AddrPort
	done := make(chan struct{})
	go func() {
		defer close(done)
		defer stream.Close()
		buf := make([]byte, maxPacket+1)
		packets := rate.NewLimiter(4096, 512)
		for {
			_ = socket.SetReadDeadline(time.Now().Add(mediaIdle))
			n, src, err := socket.ReadFromUDPAddrPort(buf)
			if err != nil {
				return
			}
			if n == 0 || n > maxPacket || !packets.Allow() {
				continue
			}
			allowed := false
			for _, target := range targets {
				if src == target {
					allowed = true
					break
				}
			}
			if !allowed {
				continue
			}
			mu.Lock()
			selected = src
			mu.Unlock()
			// Activity in either direction keeps this negotiation alive.
			_ = stream.SetReadDeadline(time.Now().Add(mediaIdle))
			_ = stream.SetWriteDeadline(time.Now().Add(5 * time.Second))
			if writeFrame(stream, buf[:n]) != nil {
				return
			}
		}
	}()
	for {
		_ = stream.SetReadDeadline(time.Now().Add(mediaIdle))
		packet, err := readFrame(stream, maxPacket)
		if err != nil {
			break
		}
		_ = socket.SetReadDeadline(time.Now().Add(mediaIdle))
		mu.Lock()
		target := selected
		mu.Unlock()
		if target.IsValid() {
			_, _ = socket.WriteToUDPAddrPort(packet, target)
		} else {
			for _, addr := range targets {
				_, _ = socket.WriteToUDPAddrPort(packet, addr)
			}
		}
	}
	socket.Close()
	<-done
}
