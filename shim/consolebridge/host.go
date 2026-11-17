package consolebridge

import (
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
	"github.com/gamelink-project/gamelink/shim/rustcore"
	"github.com/tailscale/tailcat"
)

// HostConfig pins discovery and media to the configured server.
type HostConfig struct {
	ServerIP, BindIP                             netip.Addr
	DiscoveryPort, MediaPortFirst, MediaPortLast uint16
}
type Host struct {
	cfg      HostConfig
	mu       sync.Mutex
	peers    map[netip.Addr]*hostPeer
	sessions int
	closed   bool
}
type hostPeer struct {
	h             *Host
	ip            netip.Addr
	tcp           net.Conn
	disc          *net.UDPConn
	id            uint64
	remote        uint64
	mu            sync.Mutex
	writeMu       sync.Mutex
	flowWriteMu   sync.Mutex
	sessions      map[uint64]*hostSession
	flow          tailcat.ConnPacketConn
	done          chan struct{}
	once          sync.Once
	wg            sync.WaitGroup
	controlRate   limiter
	discoveryRate limiter
	mediaRate     limiter
	reassembly    Reassembler
}
type hostSession struct {
	peer         *hostPeer
	id           uint64
	socket       *net.UDPConn
	phoneIP      netip.Addr
	phoneRelay   netip.AddrPort
	pending      []Signal
	pendingBytes int
	targets      map[netip.AddrPort]bool
	selected     netip.AddrPort
	touched      time.Time
	created      time.Time
	active       bool
	seq          atomic.Uint64
}
type limiter struct {
	mu    sync.Mutex
	start time.Time
	count int
}

func (l *limiter) allow(max int) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	now := time.Now()
	if now.Sub(l.start) >= time.Second {
		l.start = now
		l.count = 0
	}
	l.count++
	return l.count <= max
}

func NewHost(c HostConfig) (*Host, error) {
	if c.DiscoveryPort == 0 {
		c.DiscoveryPort = 7551
	}
	var valid bool
	err := rustcore.Invoke("console.host_config", map[string]any{"server": c.ServerIP.String(), "bind": c.BindIP.String(), "first": c.MediaPortFirst, "last": c.MediaPortLast}, &valid)
	if err != nil || !valid {
		return nil, errors.New("invalid console bridge host configuration")
	}
	return &Host{cfg: c, peers: make(map[netip.Addr]*hostPeer)}, nil
}
func validHostIP(a netip.Addr) bool {
	var valid bool
	return rustcore.Invoke("console.host_ip", map[string]string{"ip": a.String()}, &valid) == nil && valid
}
func addrIP(a net.Addr) (netip.Addr, bool) {
	if a == nil {
		return netip.Addr{}, false
	}
	ap, e := netip.ParseAddrPort(a.String())
	if e != nil {
		return netip.Addr{}, false
	}
	return ap.Addr().Unmap(), true
}
func (h *Host) Close() error {
	h.mu.Lock()
	h.closed = true
	ps := make([]*hostPeer, 0, len(h.peers))
	for _, p := range h.peers {
		ps = append(ps, p)
	}
	h.mu.Unlock()
	for _, p := range ps {
		p.close()
	}
	return nil
}
func (p *hostPeer) close() {
	p.once.Do(func() {
		close(p.done)
		p.tcp.Close()
		p.disc.Close()
		p.mu.Lock()
		if p.flow != nil {
			p.flow.Close()
			p.flow = nil
		}
		ss := p.sessions
		p.sessions = map[uint64]*hostSession{}
		p.mu.Unlock()
		for _, s := range ss {
			s.socket.Close()
		}
		p.h.mu.Lock()
		p.h.sessions -= len(ss)
		if p.h.peers[p.ip] == p {
			delete(p.h.peers, p.ip)
		}
		p.h.mu.Unlock()
	})
}
func (p *hostPeer) send(m ControlMessage) error {
	p.writeMu.Lock()
	defer p.writeMu.Unlock()
	p.tcp.SetWriteDeadline(time.Now().Add(5 * time.Second))
	return WriteControl(p.tcp, m)
}
func (h *Host) HandleTCP(c net.Conn) {
	defer c.Close()
	ip, ok := addrIP(c.RemoteAddr())
	if !ok {
		return
	}
	c.SetReadDeadline(time.Now().Add(10 * time.Second))
	m, err := ReadControl(c)
	if err != nil || m.Type != "hello" {
		return
	}
	c.SetReadDeadline(time.Time{})
	disc, err := net.ListenUDP("udp4", net.UDPAddrFromAddrPort(netip.AddrPortFrom(h.cfg.BindIP, 0)))
	if err != nil {
		return
	}
	var b [8]byte
	if _, err = rand.Read(b[:]); err != nil {
		disc.Close()
		return
	}
	id := binary.LittleEndian.Uint64(b[:])
	if id == 0 {
		id = 1
	}
	p := &hostPeer{h: h, ip: ip, tcp: c, disc: disc, id: id, sessions: make(map[uint64]*hostSession), done: make(chan struct{})}
	h.mu.Lock()
	if h.closed || len(h.peers) >= 8 || h.peers[ip] != nil {
		h.mu.Unlock()
		disc.Close()
		return
	}
	h.peers[ip] = p
	h.mu.Unlock()
	defer func() { p.close(); p.wg.Wait() }()
	if p.send(ControlMessage{Version: 1, Type: "hello", Data: "console-v1"}) != nil {
		return
	}
	p.wg.Add(2)
	go func() { defer p.wg.Done(); p.discovery() }()
	go func() { defer p.wg.Done(); p.sweep() }()
	for {
		c.SetReadDeadline(time.Now().Add(45 * time.Second))
		m, err = ReadControl(c)
		if err != nil || !p.controlRate.allow(100) {
			return
		}
		switch m.Type {
		case "offer":
			p.offer(m)
		case "candidate":
			p.candidate(m)
		case "error":
			p.remove(m.Session)
		case "status":
			if p.send(ControlMessage{Version: 1, Type: "status", Data: m.Data}) != nil {
				return
			}
		default:
			return
		}
	}
}
func (p *hostPeer) upstream() netip.AddrPort {
	return netip.AddrPortFrom(p.h.cfg.ServerIP, p.h.cfg.DiscoveryPort)
}
func (p *hostPeer) signal(typ string, id uint64, data string) {
	p.mu.Lock()
	remote := p.remote
	p.mu.Unlock()
	if remote == 0 {
		return
	}
	packet := discovery.Marshal(&discovery.MessagePacket{RecipientID: remote, Data: FormatSignal(Signal{typ, id, data})}, p.id)
	if len(packet) <= 65507 {
		_, _ = p.disc.WriteToUDPAddrPort(packet, p.upstream())
	}
}
func (p *hostPeer) offer(m ControlMessage) {
	ip, err := netip.ParseAddr(m.PeerIP)
	if err != nil || !validHostIP(ip) {
		return
	}
	p.mu.Lock()
	select {
	case <-p.done:
		p.mu.Unlock()
		return
	default:
	}
	if p.remote == 0 || len(p.sessions) >= 32 || p.sessions[m.Session] != nil {
		p.mu.Unlock()
		return
	}
	p.h.mu.Lock()
	if p.h.closed || p.h.sessions >= 256 {
		p.h.mu.Unlock()
		p.mu.Unlock()
		return
	}
	p.h.sessions++
	p.h.mu.Unlock()
	sock, err := net.ListenUDP("udp4", net.UDPAddrFromAddrPort(netip.AddrPortFrom(p.h.cfg.BindIP, 0)))
	if err != nil {
		p.h.mu.Lock()
		p.h.sessions--
		p.h.mu.Unlock()
		p.mu.Unlock()
		return
	}
	relay := sock.LocalAddr().(*net.UDPAddr).AddrPort()
	data, targets, err := MapCandidates(m.Data, ip, 1, 65535, relay)
	if err != nil {
		sock.Close()
		p.h.mu.Lock()
		p.h.sessions--
		p.h.mu.Unlock()
		p.mu.Unlock()
		return
	}
	s := &hostSession{peer: p, id: m.Session, socket: sock, phoneIP: ip, targets: make(map[netip.AddrPort]bool), touched: time.Now(), created: time.Now()}
	if len(targets) > 0 {
		s.phoneRelay = targets[0]
	}
	p.sessions[m.Session] = s
	p.mu.Unlock()
	p.wg.Add(1)
	go func() { defer p.wg.Done(); s.readMedia() }()
	p.signal(SignalOffer, m.Session, data)
}
func (p *hostPeer) candidate(m ControlMessage) {
	p.mu.Lock()
	s := p.sessions[m.Session]
	if s == nil {
		p.mu.Unlock()
		return
	}
	relay := s.socket.LocalAddr().(*net.UDPAddr).AddrPort()
	data, targets, err := MapCandidates(m.Data, s.phoneIP, 1, 65535, relay)
	if err != nil || len(targets) == 0 {
		p.mu.Unlock()
		return
	}
	if !s.phoneRelay.IsValid() {
		s.phoneRelay = targets[0]
	}
	s.touched = time.Now()
	pending := s.pending
	s.pending = nil
	s.pendingBytes = 0
	p.mu.Unlock()
	p.signal(SignalCandidate, m.Session, data)
	for _, sig := range pending {
		p.upstreamSignal(sig)
	}
}
func (p *hostPeer) remove(id uint64) {
	p.mu.Lock()
	s := p.sessions[id]
	delete(p.sessions, id)
	if s != nil {
		p.reassembly.DropSession(id)
	}
	p.mu.Unlock()
	if s != nil {
		s.socket.Close()
		p.h.mu.Lock()
		p.h.sessions--
		p.h.mu.Unlock()
	}
}
func (p *hostPeer) sweep() {
	t := time.NewTicker(5 * time.Second)
	defer t.Stop()
	for {
		select {
		case <-p.done:
			return
		case <-t.C:
			p.reassembly.Expire(time.Now())
		}
		p.mu.Lock()
		var ids []uint64
		for id, s := range p.sessions {
			timeout := 30 * time.Second
			if s.active {
				timeout = 5 * time.Minute
			}
			last := s.touched
			if !s.active {
				last = s.created
			}
			if time.Since(last) >= timeout {
				ids = append(ids, id)
			}
		}
		p.mu.Unlock()
		for _, id := range ids {
			p.remove(id)
		}
	}
}
func (p *hostPeer) discovery() {
	request := func() {
		_, _ = p.disc.WriteToUDPAddrPort(discovery.Marshal(&discovery.RequestPacket{}, p.id), p.upstream())
	}
	request()
	tick := time.NewTicker(30 * time.Second)
	defer tick.Stop()
	p.wg.Add(1)
	go func() {
		defer p.wg.Done()
		for {
			select {
			case <-p.done:
				return
			case <-tick.C:
				request()
			}
		}
	}()
	buf := make([]byte, 65536)
	for {
		n, src, err := p.disc.ReadFromUDPAddrPort(buf)
		if err != nil {
			return
		}
		if src != p.upstream() || !p.discoveryRate.allow(100) {
			continue
		}
		pk, sender, err := discovery.Unmarshal(buf[:n])
		if err != nil || sender == 0 {
			continue
		}
		switch v := pk.(type) {
		case *discovery.ResponsePacket:
			if len(v.ApplicationData) > 4096 {
				continue
			}
			var d discovery.ServerData
			if d.UnmarshalBinary(v.ApplicationData) != nil {
				continue
			}
			p.mu.Lock()
			if p.remote != 0 && p.remote != sender {
				p.mu.Unlock()
				// BDS restarted. Its nonce and network identity belong to a
				// new server lifetime; reconnect rather than advertise stale
				// metadata or carry old sessions into that lifetime.
				p.close()
				return
			}
			p.remote = sender
			p.mu.Unlock()
			if p.send(ControlMessage{Version: 1, Type: "advertisement", Data: base64.StdEncoding.EncodeToString(v.ApplicationData)}) != nil {
				p.close()
				return
			}
		case *discovery.MessagePacket:
			p.mu.Lock()
			remote := p.remote
			p.mu.Unlock()
			if sender != remote || v.RecipientID != p.id {
				continue
			}
			sig, err := ParseSignal(v.Data)
			if err != nil {
				continue
			}
			p.upstreamSignal(sig)
		}
	}
}
func (p *hostPeer) upstreamSignal(sig Signal) {
	p.mu.Lock()
	s := p.sessions[sig.ConnectionID]
	if s == nil {
		p.mu.Unlock()
		return
	}
	if sig.Type == SignalError {
		p.mu.Unlock()
		_ = p.send(ControlMessage{Version: 1, Type: "error", Session: sig.ConnectionID, Data: sig.Data})
		p.remove(sig.ConnectionID)
		return
	}
	if sig.Type != SignalAnswer && sig.Type != SignalCandidate {
		p.mu.Unlock()
		return
	}
	if !s.phoneRelay.IsValid() {
		if len(s.pending) < 32 && s.pendingBytes+len(sig.Data) <= MaxControlFrame {
			s.pending = append(s.pending, sig)
			s.pendingBytes += len(sig.Data)
		}
		p.mu.Unlock()
		return
	}
	data, targets, err := MapCandidates(sig.Data, p.h.cfg.ServerIP, p.h.cfg.MediaPortFirst, p.h.cfg.MediaPortLast, s.phoneRelay)
	if err != nil || (sig.Type == SignalCandidate && len(targets) == 0) {
		p.mu.Unlock()
		return
	}
	for _, a := range targets {
		if len(s.targets) >= 32 && !s.targets[a] {
			continue
		}
		s.targets[a] = true
		if !s.selected.IsValid() {
			s.selected = a
		}
	}
	s.touched = time.Now()
	p.mu.Unlock()
	typ := "answer"
	if sig.Type == SignalCandidate {
		typ = "candidate"
	}
	if p.send(ControlMessage{Version: 1, Type: typ, Session: sig.ConnectionID, Data: data}) != nil {
		p.close()
	}
}
func (h *Host) HandleUDP(c tailcat.ConnPacketConn) {
	defer c.Close()
	ip, ok := addrIP(c.RemoteAddr())
	if !ok {
		return
	}
	h.mu.Lock()
	p := h.peers[ip]
	h.mu.Unlock()
	if p == nil {
		return
	}
	p.mu.Lock()
	if p.flow != nil {
		p.mu.Unlock()
		return
	}
	p.flow = c
	p.mu.Unlock()
	defer func() {
		p.mu.Lock()
		if p.flow == c {
			p.flow = nil
		}
		p.mu.Unlock()
	}()
	buf := make([]byte, MaxPacket+1)
	for {
		n, err := c.Read(buf)
		if err != nil {
			return
		}
		if n < 24 || n > MaxPacket || !p.mediaRate.allow(5000) {
			continue
		}
		id := binary.BigEndian.Uint64(buf[8:16])
		p.mu.Lock()
		s := p.sessions[id]
		if s == nil {
			p.mu.Unlock()
			continue
		}
		_, payload, err := p.reassembly.Add(buf[:n], time.Now())
		if err != nil || payload == nil {
			p.mu.Unlock()
			continue
		}
		dst := s.selected
		var destinations []netip.AddrPort
		if s.active && dst.IsValid() {
			destinations = append(destinations, dst)
		} else {
			for target := range s.targets {
				destinations = append(destinations, target)
			}
		}
		if len(destinations) > 0 {
			s.touched = time.Now()
		}
		p.mu.Unlock()
		for _, target := range destinations {
			if !p.mediaRate.allow(5000) {
				break
			}
			_, _ = s.socket.WriteToUDPAddrPort(payload, target)
		}
	}
}
func (s *hostSession) readMedia() {
	buf := make([]byte, MaxDatagram+1)
	p := s.peer
	for {
		n, src, err := s.socket.ReadFromUDPAddrPort(buf)
		if err != nil {
			return
		}
		if n == 0 || n > MaxDatagram || !p.mediaRate.allow(5000) {
			continue
		}
		p.mu.Lock()
		if p.sessions[s.id] != s || !s.targets[src] || p.flow == nil {
			p.mu.Unlock()
			continue
		}
		s.selected = src
		s.touched = time.Now()
		s.active = true
		flow := p.flow
		p.mu.Unlock()
		packets, err := Fragment(s.id, s.seq.Add(1), buf[:n])
		if err == nil {
			p.flowWriteMu.Lock()
			flow.SetWriteDeadline(time.Now().Add(5 * time.Second))
			for _, b := range packets {
				if !p.mediaRate.allow(5000) {
					break
				}
				if _, err = flow.Write(b); err != nil {
					break
				}
			}
			p.flowWriteMu.Unlock()
		}
	}
}
