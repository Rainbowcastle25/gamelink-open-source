package integration

import (
	"bytes"
	"context"
	"net"
	"testing"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/types/key"

	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/transport"
)

func TestFactorioReachability(t *testing.T) {
	relay := startRelay(t)
	for _, tc := range []struct {
		name string
		want string
	}{
		{"valid reply", "yes"},
		{"invalid replies", "unknown"},
		{"timeout", "unknown"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			server, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { server.Close() })
			pings := make(chan struct{}, 1)
			go func() {
				var request [4]byte
				for {
					n, src, err := server.ReadFromUDP(request[:])
					if err != nil {
						return
					}
					if n != 3 || request[0] != 0 {
						continue
					}
					select {
					case pings <- struct{}{}:
					default:
					}
					switch tc.name {
					case "valid reply":
						server.WriteToUDP([]byte{0x21, request[1], request[2]}, src)
					case "invalid replies":
						server.WriteToUDP(request[:n], src) // Echoed ping
						server.WriteToUDP([]byte{0x01, request[1] ^ 1, request[2]}, src)
						server.WriteToUDP([]byte{0x01, request[1], request[2], 0}, src)
					}
				}
			}()
			env := newAgentEnv(t, relay)
			const port = 34197
			env.addGame("factorio", games.Factorio, games.UDP, port, server.LocalAddr().(*net.UDPAddr).AddrPort())
			key, public := newKey()
			env.approve("factorio", public, "invited player")
			env.serve()
			opts := joinOpts(env.addr("factorio"), games.UDP, port, key)
			opts.Game = games.Factorio
			ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
			defer cancel()
			if _, _, got, err := transport.CheckEndpoint(ctx, opts); err != nil || got != tc.want {
				t.Fatalf("CheckEndpoint = %q, %v, want %q", got, err, tc.want)
			}
			select {
			case <-pings:
			default:
				t.Fatal("Factorio server did not receive a ping through the tunnel")
			}
			if tc.want == "yes" {
				tunnel, err := transport.Join(ctx, opts)
				if err != nil {
					t.Fatal(err)
				}
				defer tunnel.Stop()
				if got := tunnel.Status().GameReachable; got != tc.want {
					t.Fatalf("connected game reachability = %q, want %q", got, tc.want)
				}
			}
		})
	}
}

func TestFactorioProbesLeaveGameplayCapacityAndPresence(t *testing.T) {
	relay := startRelay(t)
	identity, public := newKey()
	hostKey := tailcat.NewPrivateKey()
	peers := transport.NewPeerActivity()
	const port = 34197
	host, err := transport.StartGameServer(transport.ServerOptions{
		Game: games.Factorio, Protocol: games.UDP, TunnelPort: port, Dest: udpEcho(t),
		Private: hostKey.Private, PSK: hostKey.Public.PresharedKey, Region: relay,
		Approved: []key.NodePublic{identity.Public()}, Peers: peers, MaxSessions: 1,
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(host.Close)

	client := &tailcat.Client{Server: host.Addr(), Key: identity, Logf: quietLogf(t)}
	defer client.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	open := func() net.Conn {
		t.Helper()
		conn, err := client.DialUDPPort(ctx, port)
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { conn.Close() })
		return conn
	}
	echo := func(conn net.Conn, want []byte) {
		t.Helper()
		if err := conn.SetDeadline(time.Now().Add(3 * time.Second)); err != nil {
			t.Fatal(err)
		}
		if _, err := conn.Write(want); err != nil {
			t.Fatal(err)
		}
		var reply [32]byte
		if n, err := conn.Read(reply[:]); err != nil || !bytes.Equal(reply[:n], want) {
			t.Fatalf("echo after %x = %x, %v", want, reply[:n], err)
		}
	}

	ping := []byte{0x00, 0x12, 0x34}
	echo(open(), ping)
	gameplay := open()
	// A game browser can ping on the socket it joins with.
	echo(gameplay, []byte{0x20, 0x12, 0x34})
	if host.Stats().Active != 0 || len(peers.Snapshot()) != 0 {
		t.Fatal("ping-only flows counted as gameplay")
	}
	echo(gameplay, []byte{0x02, 0x12, 0x34, 0x56})
	if host.Stats().Active != 1 || peers.Snapshot()[public].IsZero() {
		t.Fatal("gameplay flow did not claim a slot and record peer activity")
	}
}
