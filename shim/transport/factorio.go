package transport

import (
	"context"
	"crypto/rand"
	"errors"
	"net"
	"time"

	"github.com/tailscale/tailcat"
)

const factorioProbeTimeout = 2 * time.Second

func isFactorioPing(packet []byte) bool {
	return len(packet) == 3 && (packet[0] == 0x00 || packet[0] == 0x20)
}

// Reuse the connected UDP socket across health checks. The agent's end of a
// UDP flow stays open until it idles out, even after the client closes its end.
type factorioProbe struct {
	conn net.Conn
}

func (p *factorioProbe) close() {
	if p.conn != nil {
		_ = p.conn.Close()
		p.conn = nil
	}
}

func (p *factorioProbe) ping(ctx context.Context, cl *tailcat.Client, port uint16) bool {
	pctx, cancel := context.WithTimeout(ctx, factorioProbeTimeout)
	defer cancel()
	if pctx.Err() != nil {
		return false
	}
	if p.conn == nil {
		conn, err := cl.DialUDPPort(pctx, port)
		if err != nil {
			return false
		}
		p.conn = conn
	}
	conn := p.conn
	stop := context.AfterFunc(ctx, func() {
		if errors.Is(ctx.Err(), context.Canceled) {
			_ = conn.Close()
		}
	})
	defer stop()
	deadline, _ := pctx.Deadline()
	if err := conn.SetDeadline(deadline); err != nil {
		p.close()
		return false
	}

	// A direct Factorio Ping is type 0 followed by a 16-bit sequence.
	// PingReply echoes the sequence with header 0x01 or 0x21. Both forms
	// were observed on headless 1.1.110, 2.0.77, 2.1.24 and 2.1.25.
	// https://github.com/Hornwitser/factorio_tools/blob/master/hornwitser/factorio_tools/ping.py
	var ping [3]byte
	rand.Read(ping[1:])
	if _, err := conn.Write(ping[:]); err != nil {
		p.close()
		return false
	}
	var reply [4]byte // Allow an extra byte so truncated datagrams are rejected.
	for pctx.Err() == nil {
		n, err := conn.Read(reply[:])
		if err != nil {
			var ne net.Error
			if !errors.As(err, &ne) || !ne.Timeout() {
				p.close()
			}
			return false
		}
		if n == len(ping) && (reply[0] == 0x01 || reply[0] == 0x21) &&
			reply[1] == ping[1] && reply[2] == ping[2] {
			return pctx.Err() == nil
		}
	}
	return false
}
