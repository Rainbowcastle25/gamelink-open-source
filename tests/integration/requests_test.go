package integration

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/invite"
	"github.com/gamelink-project/gamelink/shim/transport"
	"github.com/tailscale/tailcat"
	"tailscale.com/types/key"
)

func TestRustAgentSubprocessKnockRotateAndExpire(t *testing.T) {
	env := newAgentEnv(t, startRelay(t))
	const gameID = "knock"
	const port = 31100
	env.addGame(gameID, games.Custom, games.TCP, port, tcpEcho(t))
	original := tailcat.Addr(env.invite(gameID).Addr)
	env.serve()
	friend, friendPublic := newKey()
	other, otherPublic := newKey()
	localRelay := invite.Options{AllowLocalRelay: true}

	type pendingRequest struct {
		ID        uint64
		Game      string
		PublicKey string
		FirstSeen time.Time
		LastSeen  time.Time
		Attempts  uint64
	}
	list := func() []pendingRequest {
		var snapshot struct {
			Requests []pendingRequest
		}
		if json.Unmarshal(env.cli("requests", "list", "--game", gameID, "--json"), &snapshot) != nil {
			t.Fatal("invalid request listing")
		}
		return snapshot.Requests
	}
	waitRequest := func(public string) uint64 {
		t.Helper()
		for deadline := time.Now().Add(agentWait); time.Now().Before(deadline); time.Sleep(50 * time.Millisecond) {
			for _, row := range list() {
				if row.PublicKey == public {
					if row.ID == 0 || row.Game != gameID || row.FirstSeen.IsZero() || row.LastSeen.Before(row.FirstSeen) || row.Attempts == 0 {
						t.Fatal("request missing details")
					}
					return row.ID
				}
			}
		}
		t.Fatal("refused peer never appeared in the private request list")
		return 0
	}
	deny := func(addr tailcat.Addr, identity key.NodePrivate) {
		t.Helper()
		opts := joinOpts(addr, games.TCP, port, identity)
		opts.ConnectTimeout = 3 * time.Second
		tunnel, err := transport.Join(context.Background(), opts)
		if err == nil {
			tunnel.Stop()
			t.Fatal("a denied peer opened a tunnel")
		}
		if !errors.Is(err, transport.ErrNoResponse) {
			t.Fatalf("refusal was not silent: %v", err)
		}
	}
	connect := func(addr tailcat.Addr) (*transport.Tunnel, net.Conn) {
		t.Helper()
		opts := joinOpts(addr, games.TCP, port, friend)
		opts.AutoReconnect = false
		tunnel, err := transport.Join(context.Background(), opts)
		if err != nil {
			t.Fatalf("approved peer could not join: %v", err)
		}
		t.Cleanup(tunnel.Stop)
		conn, err := net.DialTimeout("tcp", tunnel.Status().LocalAddr, 5*time.Second)
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { conn.Close() })
		_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
		if _, err := io.WriteString(conn, "hello"); err != nil {
			t.Fatal(err)
		}
		buf := make([]byte, 5)
		if _, err := io.ReadFull(conn, buf); err != nil || string(buf) != "hello" {
			t.Fatal("approved tunnel did not forward echo")
		}
		return tunnel, conn
	}
	waitClosed := func(conn net.Conn, timeout time.Duration) {
		t.Helper()
		_ = conn.SetReadDeadline(time.Now().Add(timeout))
		_, err := conn.Read(make([]byte, 1))
		var networkError net.Error
		if err == nil || (errors.As(err, &networkError) && networkError.Timeout()) {
			t.Fatal("established session survived the identity change")
		}
	}

	deny(original, friend)
	friendRequest := waitRequest(friendPublic)
	stat, err := os.Stat(filepath.Join(env.dir, "requests.json"))
	if err != nil || stat.Mode().Perm() != 0o600 {
		t.Fatal("request store lacks owner-only permissions")
	}
	if state, ok := env.gameState(gameID); !ok || state.Approved != 0 || state.State != "inactive" {
		t.Fatal("a knock automatically approved a device")
	}
	deny(original, other)
	waitRequest(otherPublic)
	env.cli("requests", "dismiss", otherPublic)
	if entries := list(); len(entries) != 1 || entries[0].ID != friendRequest {
		t.Fatal("dismissal hid the wrong request or changed its ID")
	}
	// The shim reports every knock again; a dismissal holds until rotation.
	probe, probePublic := newKey()
	deny(original, other)
	deny(original, probe)
	waitRequest(probePublic)
	for _, entry := range list() {
		if entry.PublicKey == otherPublic {
			t.Fatal("a dismissed device was listed again before rotation")
		}
	}
	env.cli("requests", "dismiss", probePublic)

	env.stop()
	env.serve()
	if waitRequest(friendPublic) != friendRequest {
		t.Fatal("request did not survive restart with the same ID")
	}
	env.cli("requests", "approve", strconv.FormatUint(friendRequest, 10), "--label", "Friend")
	env.waitState(gameID, "serving")
	if len(list()) != 0 {
		t.Fatal("approved request is still listed")
	}
	firstTunnel, firstConn := connect(original)
	rotation := strings.TrimSpace(string(env.cli("invite", "rotate", "--game", gameID, "--allow-local-relay")))
	rotated, _, err := invite.Parse(rotation, localRelay)
	if err != nil || rotated.Addr == string(original) {
		t.Fatal("rotation did not issue a distinct invitation")
	}
	waitClosed(firstConn, 10*time.Second)
	firstTunnel.Stop()
	secondTunnel, secondConn := connect(tailcat.Addr(rotated.Addr))
	deny(original, friend)
	deny(tailcat.Addr(rotated.Addr), other)
	waitRequest(otherPublic)
	env.cli("requests", "dismiss", otherPublic)

	expiringText := strings.TrimSpace(string(env.cli("invite", "--game", gameID, "--expires", "15s", "--allow-local-relay")))
	expiring, _, err := invite.Parse(expiringText, localRelay)
	if err != nil || expiring.Addr == rotated.Addr || expiring.ExpiresAt == "" {
		t.Fatal("expiring invitation did not use a new identity with a deadline")
	}
	waitClosed(secondConn, 10*time.Second)
	secondTunnel.Stop()
	_, expiringConn := connect(tailcat.Addr(expiring.Addr))
	waitClosed(expiringConn, 20*time.Second)
	env.waitState(gameID, "inactive")
	if _, _, err := invite.Parse(expiringText, localRelay); err == nil || !strings.Contains(err.Error(), "expired") {
		t.Fatal("the expired invitation was accepted by the client parser")
	}
	// Bypass invitation parsing and use the raw address with a still-approved key.
	deny(tailcat.Addr(expiring.Addr), friend)
	env.stop()
	env.serve()
	env.waitState(gameID, "inactive")
	deny(tailcat.Addr(expiring.Addr), friend)
}
