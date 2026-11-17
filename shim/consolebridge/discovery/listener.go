package discovery

import (
	"crypto/rand"
	"encoding/binary"
	"errors"
	"net"
	"net/netip"
)

// Datagram includes the actual UDP sender so callers can pin it per session.
// The source is not authenticated by LAN discovery's shared wire key.
type Datagram struct {
	SenderID uint64
	Source   netip.AddrPort
	Packet   Packet
}

// Listener is a minimal discovery socket with no network-ID address cache.
// Callers must validate and pin Source before accepting signals or sending
// responses. A nonzero peer restricts all received packets to that endpoint.
type Listener struct {
	conn *net.UDPConn
	id   uint64
	peer netip.AddrPort
	lan  *lanSocket
}

func Listen(bind netip.AddrPort, peer netip.AddrPort) (*Listener, error) {
	if !bind.IsValid() || !bind.Addr().Is4() {
		return nil, errors.New("discovery requires an IPv4 bind address")
	}
	if peer.IsValid() && (!peer.Addr().Is4() || peer.Port() == 0) {
		return nil, errors.New("invalid discovery peer")
	}
	c, err := net.ListenUDP("udp4", net.UDPAddrFromAddrPort(bind))
	if err != nil {
		return nil, err
	}
	var b [8]byte
	if _, err = rand.Read(b[:]); err != nil {
		c.Close()
		return nil, err
	}
	id := binary.LittleEndian.Uint64(b[:])
	if id == 0 {
		id = 1
	}
	return &Listener{conn: c, id: id, peer: peer}, nil
}
func (l *Listener) NetworkID() uint64         { return l.id }
func (l *Listener) LocalAddr() netip.AddrPort { return l.conn.LocalAddr().(*net.UDPAddr).AddrPort() }
func (l *Listener) Close() error              { return l.conn.Close() }

// Receive skips invalid packets and packets from unconfigured upstreams.
func (l *Listener) Receive() (Datagram, error) {
	b := make([]byte, 65535)
	for {
		n, src, err := l.readPacket(b)
		if err != nil {
			return Datagram{}, err
		}
		src = netip.AddrPortFrom(src.Addr().Unmap(), src.Port())
		if l.peer.IsValid() && src != l.peer {
			continue
		}
		p, id, err := Unmarshal(b[:n])
		if err != nil || id == 0 || id == l.id {
			continue
		}
		return Datagram{SenderID: id, Source: src, Packet: p}, nil
	}
}

// Send writes only to a caller-supplied pinned source or configured upstream.
func (l *Listener) Send(p Packet, target netip.AddrPort) error {
	if p == nil || !target.IsValid() || !target.Addr().Is4() || target.Port() == 0 || (l.peer.IsValid() && target != l.peer) {
		return errors.New("invalid discovery target")
	}
	data := Marshal(p, l.id)
	if len(data) == 0 || len(data) > 65507 {
		return errors.New("discovery packet too large")
	}
	return l.writePacket(data, target)
}
