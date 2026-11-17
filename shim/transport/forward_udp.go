package transport

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/netip"
	"sync"
	"time"

	"github.com/tailscale/tailcat"
)

// udpFlow is one local UDP endpoint (a game client socket) mapped to one
// Tailcat packet flow. Replies on the flow are sent back to that endpoint.
type udpFlow struct {
	src    netip.AddrPort
	conn   tailcat.ConnPacketConn
	mu     sync.Mutex
	last   time.Time
	closed bool
}

func (f *udpFlow) touch() {
	f.mu.Lock()
	f.last = time.Now()
	f.mu.Unlock()
}

func (f *udpFlow) idleFor() time.Duration {
	f.mu.Lock()
	defer f.mu.Unlock()
	return time.Since(f.last)
}

func (f *udpFlow) close() {
	f.mu.Lock()
	f.closed = true
	f.mu.Unlock()
	_ = f.conn.Close()
}

func (t *Tunnel) flowCount() int {
	t.flowsMu.Lock()
	defer t.flowsMu.Unlock()
	return len(t.flows)
}

func (t *Tunnel) closeFlows() {
	t.flowsMu.Lock()
	flows := t.flows
	t.flows = map[netip.AddrPort]*udpFlow{}
	t.flowsMu.Unlock()
	for _, f := range flows {
		f.close()
	}
}

func (t *Tunnel) serveUDP() {
	t.wg.Add(1)
	go func() { defer t.wg.Done(); t.sweepFlows() }()

	buf := make([]byte, readBufSize)
	for {
		n, src, err := t.udpPC.ReadFromUDPAddrPort(buf)
		if err != nil {
			if t.ctx.Err() != nil || errors.Is(err, net.ErrClosed) {
				return
			}
			if isTransientUDPErr(err) {
				continue // ICMP noise on loopback
			}
			t.fail(StateError, "The local UDP socket failed: "+err.Error())
			return
		}
		src = netip.AddrPortFrom(src.Addr().Unmap(), src.Port())
		if n > MaxUDPPayload {
			// Tailcat carries at most MaxUDPPayload bytes per datagram. We
			// neither truncate nor fragment; the datagram is dropped and counted.
			t.oversize.Add(1)
			t.oversizeLg.do(func() {
				t.setStatus(func(s *Status) {
					s.Message = fmt.Sprintf("Dropped a %d-byte packet: GameLink/Tailcat can carry at most %d bytes per UDP packet. This game may not work over GameLink.", n, MaxUDPPayload)
				})
			})
			continue
		}
		t.sendUDP(src, buf[:n])
	}
}

func (t *Tunnel) sendUDP(src netip.AddrPort, pkt []byte) {
	for attempt := 0; attempt < 2; attempt++ {
		f := t.flowFor(src)
		if f == nil {
			return
		}
		if _, err := f.conn.Write(pkt); err == nil {
			f.touch()
			t.bytesUp.Add(int64(len(pkt)))
			return
		}
		// The flow died (idle timeout on the server, reconnect, ...): drop it
		// and retry once on a fresh one.
		t.dropFlow(f)
	}
	t.dialFails.Add(1)
	t.kickHealth()
}

func (t *Tunnel) flowFor(src netip.AddrPort) *udpFlow {
	t.flowsMu.Lock()
	if f, ok := t.flows[src]; ok {
		t.flowsMu.Unlock()
		return f
	}
	if len(t.flows) >= maxUDPFlows {
		t.flowsMu.Unlock()
		return nil
	}
	t.flowsMu.Unlock()

	cl := t.client()
	if cl == nil {
		return nil
	}
	ctx, cancel := context.WithTimeout(t.ctx, t.opts.DialTimeout)
	conn, err := cl.DialUDPPort(ctx, t.opts.TunnelPort)
	cancel()
	if err != nil {
		if t.ctx.Err() == nil {
			t.dialFails.Add(1)
			t.kickHealth()
		}
		return nil
	}
	t.dialFails.Store(0)
	f := &udpFlow{src: src, conn: conn, last: time.Now()}

	t.flowsMu.Lock()
	if existing, ok := t.flows[src]; ok { // lost a race
		t.flowsMu.Unlock()
		_ = conn.Close()
		return existing
	}
	t.flows[src] = f
	t.flowsMu.Unlock()

	t.wg.Add(1)
	go func() {
		defer t.wg.Done()
		t.readFlow(f)
	}()
	return f
}

func (t *Tunnel) readFlow(f *udpFlow) {
	defer t.dropFlow(f)
	buf := make([]byte, readBufSize)
	for {
		n, err := f.conn.Read(buf)
		if err != nil {
			return
		}
		f.touch()
		if n > MaxUDPPayload {
			t.oversize.Add(1)
			continue
		}
		if _, err := t.udpPC.WriteToUDPAddrPort(buf[:n], f.src); err != nil {
			if t.ctx.Err() != nil || errors.Is(err, net.ErrClosed) {
				return
			}
			continue
		}
		t.bytesDown.Add(int64(n))
	}
}

func (t *Tunnel) dropFlow(f *udpFlow) {
	t.flowsMu.Lock()
	if t.flows[f.src] == f {
		delete(t.flows, f.src)
	}
	t.flowsMu.Unlock()
	f.close()
}

// sweepFlows closes flows that have been idle longer than the configured
// timeout, matching the server side's idle expiry.
func (t *Tunnel) sweepFlows() {
	interval := t.opts.UDPIdle / 4
	if interval > 15*time.Second {
		interval = 15 * time.Second
	}
	if interval < 50*time.Millisecond {
		interval = 50 * time.Millisecond
	}
	tick := time.NewTicker(interval)
	defer tick.Stop()
	for {
		select {
		case <-t.ctx.Done():
			return
		case <-tick.C:
		}
		t.flowsMu.Lock()
		var stale []*udpFlow
		for _, f := range t.flows {
			if f.idleFor() > t.opts.UDPIdle {
				stale = append(stale, f)
			}
		}
		t.flowsMu.Unlock()
		for _, f := range stale {
			t.dropFlow(f)
		}
	}
}
