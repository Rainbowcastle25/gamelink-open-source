package integration

import (
	"context"
	"encoding/json"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/invite"
	"github.com/gamelink-project/gamelink/shim/transport"
	"github.com/tailscale/tailcat"
)

// Unlike the transport tests, this starts the shipped Rust CLI as a child.
// The only fixture injection is replacing the pinned relay with the local
// hermetic DERP (CLI deliberately exposes no insecure relay/testing flags).
func TestRustAgentSubprocessRevocationAndFailClosed(t *testing.T) {
	binary := agentBinary(t)
	dir := filepath.Join(t.TempDir(), "agent")
	cli := func(args ...string) []byte {
		t.Helper()
		return agentCLI(t, binary, dir, args...)
	}
	cli("init")
	relay := startRelay(t)
	echo := tcpEcho(t)
	cli("games", "add", "--type", "custom", "--proto", "tcp", "--port", "7000", "--id", "game", "--dest", echo.String(), "--relay-host", "derp.example.com")
	// Exercise the saved-config destination path, independently of CLI validation.
	configPath := filepath.Join(dir, "agent.json")
	configData, err := os.ReadFile(configPath)
	if err != nil {
		t.Fatal(err)
	}
	var saved map[string]any
	if json.Unmarshal(configData, &saved) != nil {
		t.Fatal("invalid config fixture")
	}
	saved["games"].([]any)[0].(map[string]any)["dest"] = "localhost:" + strconv.Itoa(int(echo.Port()))
	configData, _ = json.Marshal(saved)
	if err := writeFileAtomic(configPath, configData); err != nil {
		t.Fatal(err)
	}
	keyPath := filepath.Join(dir, "keys", "game.json")
	data, err := os.ReadFile(keyPath)
	if err != nil {
		t.Fatal(err)
	}
	var keys map[string]any
	if json.Unmarshal(data, &keys) != nil {
		t.Fatal("invalid key fixture")
	}
	keys["region"] = relay
	data, _ = json.Marshal(keys)
	if err := writeFileAtomic(keyPath, data); err != nil {
		t.Fatal(err)
	}
	alice, alicePub := newKey()
	bob, bobPub := newKey()
	invitation := cli("invite", "--game", "game", "--allow-local-relay")
	inv, _, err := invite.Parse(strings.TrimSpace(string(invitation)), invite.Options{AllowLocalRelay: true})
	if err != nil {
		t.Fatal("Rust invitation failed to parse")
	}
	cmd := exec.Command(binary, "--dir", dir, "serve")
	cmd.Stdout = io.Discard
	cmd.Stderr = io.Discard
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	done := make(chan struct{})
	var waitErr error
	go func() { waitErr = cmd.Wait(); close(done) }()
	t.Cleanup(func() {
		select {
		case <-done:
			return
		default:
			_ = cmd.Process.Kill()
			select {
			case <-done:
			case <-time.After(10 * time.Second):
				t.Error("Rust agent did not exit")
			}
		}
	})
	waitState := func(state string, approved int) {
		t.Helper()
		deadline := time.Now().Add(20 * time.Second)
		for time.Now().Before(deadline) {
			rs, fresh, err := readAgentRuntime(dir)
			if err == nil && fresh && len(rs.Games) == 1 && rs.Games[0].State == state && rs.Games[0].Approved == approved {
				return
			}
			time.Sleep(50 * time.Millisecond)
		}
		t.Fatalf("Rust agent never reached %s", state)
	}
	waitState("inactive", 0)
	cli("devices", "approve", "--game", "game", "--key", alicePub, "--label", "alice")
	cli("devices", "approve", "--game", "game", "--key", bobPub, "--label", "bob")
	waitState("serving", 2)
	opts := joinOpts(tailcat.Addr(inv.Addr), games.TCP, 7000, alice)
	opts.AutoReconnect = false
	tun, err := transport.Join(context.Background(), opts)
	if err != nil {
		t.Fatalf("approved Alice join: %v", err)
	}
	defer tun.Stop()
	open, err := net.DialTimeout("tcp", tun.Status().LocalAddr, 5*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	defer open.Close()
	echoOn := func(c net.Conn) {
		t.Helper()
		_ = c.SetDeadline(time.Now().Add(10 * time.Second))
		if _, err := io.WriteString(c, "hello"); err != nil {
			t.Fatal(err)
		}
		buf := make([]byte, 5)
		if _, err := io.ReadFull(c, buf); err != nil || string(buf) != "hello" {
			t.Fatal("subprocess agent did not forward echo")
		}
	}
	echoOn(open)
	cli("devices", "revoke", "--game", "game", "--key", alicePub)
	_ = open.SetReadDeadline(time.Now().Add(10 * time.Second))
	if _, err := open.Read(make([]byte, 1)); err == nil {
		t.Fatal("revoked Alice session survived")
	} else if ne, ok := err.(net.Error); ok && ne.Timeout() {
		t.Fatal("revocation did not terminate the established session")
	}
	// Surviving Bob proves the restart preserved per-device caller identity.
	waitState("serving", 1)
	deniedOpts := joinOpts(tailcat.Addr(inv.Addr), games.TCP, 7000, alice)
	deniedOpts.ConnectTimeout = 3 * time.Second
	deniedCtx, cancelDenied := context.WithTimeout(context.Background(), 5*time.Second)
	denied, deniedErr := transport.Join(deniedCtx, deniedOpts)
	cancelDenied()
	if deniedErr == nil {
		denied.Stop()
		t.Fatal("revoked Alice could open a new tunnel")
	}

	opts = joinOpts(tailcat.Addr(inv.Addr), games.TCP, 7000, bob)
	opts.AutoReconnect = false
	bobTun, err := transport.Join(context.Background(), opts)
	if err != nil {
		t.Fatalf("approved Bob join after revocation: %v", err)
	}
	defer bobTun.Stop()
	bobOpen, err := net.DialTimeout("tcp", bobTun.Status().LocalAddr, 5*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	defer bobOpen.Close()
	echoOn(bobOpen)
	// A corrupt snapshot must close all previously approved sessions, not keep
	// serving the cached configuration. No private contents appear in output.
	if err := writeFileAtomic(filepath.Join(dir, "agent.json"), []byte(`{"version":1,"games":`)); err != nil {
		t.Fatal(err)
	}
	_ = bobOpen.SetReadDeadline(time.Now().Add(10 * time.Second))
	if _, err := bobOpen.Read(make([]byte, 1)); err == nil {
		t.Fatal("session survived invalid config")
	} else if ne, ok := err.(net.Error); ok && ne.Timeout() {
		t.Fatal("invalid config did not fail closed")
	}
	// SIGTERM must execute explicit shutdown, delete runtime status, and exit 0.
	if err := cmd.Process.Signal(syscall.SIGTERM); err != nil {
		t.Fatal(err)
	}
	select {
	case <-done:
		if waitErr != nil {
			t.Fatalf("Rust agent shutdown: %v", waitErr)
		}
	case <-time.After(15 * time.Second):
		t.Fatal("Rust agent SIGTERM timed out")
	}
	if _, err := os.Stat(filepath.Join(dir, "runtime.json")); !os.IsNotExist(err) {
		t.Fatal("runtime status survived clean shutdown")
	}
}
