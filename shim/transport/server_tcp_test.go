package transport

import (
	"io"
	"net"
	"net/netip"
	"sync"
	"testing"
	"time"

	"tailscale.com/types/key"

	"github.com/gamelink-project/gamelink/shim/games"
)

// Models Tailcat's documented asynchronous handshake: the handler receives a
// connection before RemoteAddr becomes available.
type pendingPeerConn struct {
	net.Conn
	peer    net.Addr
	ready   chan struct{}
	checked chan struct{}
	once    sync.Once
}

func (c *pendingPeerConn) RemoteAddr() net.Addr {
	var addr net.Addr
	select {
	case <-c.ready:
		addr = c.peer
	default:
	}
	c.once.Do(func() { close(c.checked) })
	return addr
}

func TestTCPHandlerAwaitsPeerBeforeAuthorization(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	go func() {
		c, err := ln.Accept()
		if err == nil {
			defer c.Close()
			_, _ = io.WriteString(c, "server-first")
		}
	}()
	device := key.NewNode().Public()
	g := &GameServer{
		opts: ServerOptions{Protocol: games.TCP, TunnelPort: 25565,
			Dest: ln.Addr().(*net.TCPAddr).AddrPort(), MaxSessions: 1, DialTimeout: time.Second},
		allowed: map[netip.Addr]key.NodePublic{PeerAddr(device): device},
		conns:   newConnSet(), stopping: make(chan struct{}), logf: func(string, string) {},
	}
	server, client := net.Pipe()
	defer client.Close()
	defer server.Close()
	pending := &pendingPeerConn{Conn: server,
		peer:  &net.TCPAddr{IP: net.IP(PeerAddr(device).AsSlice()), Port: 12345},
		ready: make(chan struct{}), checked: make(chan struct{})}
	done := make(chan struct{})
	go func() { defer close(done); g.onTCP(25565)(pending) }()
	<-pending.checked
	close(pending.ready)
	_ = client.SetReadDeadline(time.Now().Add(2 * time.Second))
	got := make([]byte, len("server-first"))
	if _, err := io.ReadFull(client, got); err != nil {
		t.Fatalf("approved pending handshake lost server-first response: %v", err)
	}
	if string(got) != "server-first" || g.Stats().Denied != 0 {
		t.Fatalf("response=%q stats=%+v", got, g.Stats())
	}
	client.Close()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("TCP handler did not clean up")
	}
}

func TestPendingTCPHandshakeBoundedAndCancelled(t *testing.T) {
	for _, stop := range []bool{false, true} {
		name := "timeout"
		if stop {
			name = "shutdown"
		}
		t.Run(name, func(t *testing.T) {
			g := &GameServer{opts: ServerOptions{Protocol: games.TCP, TunnelPort: 25565,
				MaxSessions: 1, DialTimeout: 200 * time.Millisecond},
				conns: newConnSet(), stopping: make(chan struct{}), logf: func(string, string) {}}
			if stop {
				g.opts.DialTimeout = time.Hour
			}
			server, client := net.Pipe()
			defer server.Close()
			defer client.Close()
			pending := &pendingPeerConn{Conn: server, ready: make(chan struct{}), checked: make(chan struct{})}
			done := make(chan struct{})
			go func() { defer close(done); g.onTCP(25565)(pending) }()
			<-pending.checked
			if g.Stats().Active != 1 || g.admit() {
				t.Fatal("pending handshake did not reserve the only session slot")
			}
			if stop {
				close(g.stopping)
				g.conns.closeAll()
			}
			select {
			case <-done:
			case <-time.After(time.Second):
				t.Fatal("pending handshake was not bounded/cancelled")
			}
			if g.Stats().Active != 0 || g.conns.len() != 0 {
				t.Fatalf("pending handshake leaked: %+v", g.Stats())
			}
		})
	}
}

func TestServerRejectsNewHandlersWhileDraining(t *testing.T) {
	g := &GameServer{opts: ServerOptions{Protocol: games.TCP, TunnelPort: 25565}, stopping: make(chan struct{})}
	if g.onTCP(25565) == nil {
		t.Fatal("TCP handler missing before shutdown")
	}
	close(g.stopping)
	if g.onTCP(25565) != nil {
		t.Fatal("new TCP handshake accepted during shutdown drain")
	}
	g.opts.Protocol = games.UDP
	if g.onUDP(25565) != nil {
		t.Fatal("new UDP flow accepted during shutdown drain")
	}
}
