package transport

import (
	"context"
	"errors"
	"net"

	"github.com/tailscale/tailcat"
)

// countingConn counts bytes in each direction and forwards CloseWrite so TCP
// half-close survives tailcat.ProxyConns.
type countingConn struct {
	net.Conn
	read, wrote func(int)
}

func (c *countingConn) Read(p []byte) (int, error) {
	n, err := c.Conn.Read(p)
	if n > 0 {
		c.read(n)
	}
	return n, err
}

func (c *countingConn) Write(p []byte) (int, error) {
	n, err := c.Conn.Write(p)
	if n > 0 {
		c.wrote(n)
	}
	return n, err
}

func (c *countingConn) CloseWrite() error {
	if cw, ok := c.Conn.(interface{ CloseWrite() error }); ok {
		return cw.CloseWrite()
	}
	return nil
}

func (t *Tunnel) serveTCP() {
	for {
		c, err := t.tcpLn.Accept()
		if err != nil {
			if t.ctx.Err() != nil || errors.Is(err, net.ErrClosed) {
				return
			}
			var ne net.Error
			if errors.As(err, &ne) && ne.Timeout() {
				continue
			}
			t.fail(StateError, "The local listener stopped: "+err.Error())
			return
		}
		if !t.reserveTCP() {
			_ = c.Close()
			continue
		}
		t.wg.Add(1)
		go func() {
			defer t.wg.Done()
			defer t.releaseTCP()
			t.handleTCP(c)
		}()
	}
}

// reserveTCP atomically reserves one handler slot before its goroutine is
// started. Counting connSet entries is insufficient because accepted sockets
// are registered only after each handler goroutine begins running.
func (t *Tunnel) reserveTCP() bool {
	select {
	case t.tcpSlots <- struct{}{}:
		return true
	default:
		return false
	}
}

func (t *Tunnel) releaseTCP() { <-t.tcpSlots }

func (t *Tunnel) handleTCP(local net.Conn) {
	if !t.conns.add(local) {
		return
	}
	defer t.conns.remove(local)
	cl := t.client()
	if cl == nil {
		_ = local.Close()
		return
	}
	ctx, cancel := context.WithTimeout(t.ctx, t.opts.DialTimeout)
	up, err := cl.DialTCPPort(ctx, t.opts.TunnelPort)
	cancel()
	if err != nil {
		_ = local.Close()
		if t.ctx.Err() == nil {
			t.dialFails.Add(1)
			t.kickHealth()
		}
		return
	}
	t.dialFails.Store(0)
	if !t.conns.add(up) {
		_ = local.Close()
		return
	}
	defer t.conns.remove(up)
	l := &countingConn{Conn: local, read: func(n int) { t.bytesUp.Add(int64(n)) }, wrote: func(n int) { t.bytesDown.Add(int64(n)) }}
	tailcat.ProxyConns(l, up) // preserves half-close; closes both when done
}
