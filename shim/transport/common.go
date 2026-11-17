// Package transport is GameLink's networking layer: the server side that
// exposes one game port per Tailcat identity, and the client side that turns a
// Tailcat tunnel into a local loopback TCP or UDP listener a game can use.
//
// Everything in here talks to the real github.com/tailscale/tailcat library;
// there is no mock transport in production code.
package transport

import (
	"errors"
	"net"
	"net/netip"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/types/key"
	"tailscale.com/types/logger"
)

// MaxUDPPayload is the largest datagram Tailcat can carry without
// fragmentation (1232 bytes). GameLink never fragments or truncates: larger
// datagrams are dropped, counted and reported.
const MaxUDPPayload = tailcat.MaxUDPPayload

// readBufSize is large enough for any UDP datagram so that an oversized one is
// seen as oversized rather than silently truncated by the socket.
const readBufSize = 64 * 1024

// PeerAddr returns the Tailcat tunnel IPv6 address that a client with node
// public key k appears to have on the server. It mirrors tailcat's own
// derivation (fd7a:115c:a1e0::/48 plus the first 10 bytes of the key); the
// integration tests assert it matches the real value, so a future change in
// tailcat makes approved clients fail closed rather than open.
func PeerAddr(k key.NodePublic) netip.Addr {
	var a [16]byte
	r := k.Raw32()
	a[0], a[1], a[2], a[3], a[4], a[5] = 0xfd, 0x7a, 0x11, 0x5c, 0xa1, 0xe0
	copy(a[6:], r[:10])
	return netip.AddrFrom16(a)
}

// addrOf extracts the IP from a net.Addr.
func addrOf(a net.Addr) (netip.Addr, bool) {
	switch v := a.(type) {
	case *net.TCPAddr:
		ip, ok := netip.AddrFromSlice(v.IP)
		return ip.Unmap(), ok
	case *net.UDPAddr:
		ip, ok := netip.AddrFromSlice(v.IP)
		return ip.Unmap(), ok
	}
	if a == nil {
		return netip.Addr{}, false
	}
	ap, err := netip.ParseAddrPort(a.String())
	if err != nil {
		return netip.Addr{}, false
	}
	return ap.Addr().Unmap(), true
}

func discardLogf(string, ...any) {}

func orDiscard(l logger.Logf) logger.Logf {
	if l == nil {
		return discardLogf
	}
	return l
}

// connSet tracks live connections so they can all be torn down at once.
type connSet struct {
	mu     sync.Mutex
	conns  map[closer]struct{}
	closed bool
}

type closer = interface{ Close() error }

func newConnSet() *connSet { return &connSet{conns: map[closer]struct{}{}} }

// add registers c; it reports false (and closes c) if the set is closed.
func (s *connSet) add(c closer) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		_ = c.Close()
		return false
	}
	s.conns[c] = struct{}{}
	return true
}

func (s *connSet) remove(c closer) {
	s.mu.Lock()
	delete(s.conns, c)
	s.mu.Unlock()
}

func (s *connSet) len() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.conns)
}

// closeAll closes every tracked connection and refuses new ones.
func (s *connSet) closeAll() {
	s.mu.Lock()
	s.closed = true
	conns := make([]closer, 0, len(s.conns))
	for c := range s.conns {
		conns = append(conns, c)
	}
	s.conns = map[closer]struct{}{}
	s.mu.Unlock()
	for _, c := range conns {
		_ = c.Close()
	}
}

// rateLimited runs f at most once per interval.
type rateLimited struct {
	last     atomic.Int64
	interval time.Duration
}

func (r *rateLimited) do(f func()) {
	now := time.Now().UnixNano()
	prev := r.last.Load()
	if now-prev < int64(r.interval) {
		return
	}
	if r.last.CompareAndSwap(prev, now) {
		f()
	}
}

// isTransientUDPErr reports errors on a connected UDP socket that just mean
// "nothing was listening for that datagram" (ICMP port unreachable) and must
// not tear the flow down.
func isTransientUDPErr(err error) bool {
	var en syscall.Errno
	if errors.As(err, &en) {
		switch en {
		case syscall.ECONNREFUSED, 10054, 10061: // 10054/10061: WSAECONNRESET/WSAECONNREFUSED
			return true
		}
	}
	return false
}
