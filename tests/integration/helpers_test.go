// Package integration holds GameLink's end-to-end tests. They use the REAL
// Tailcat transport (WireGuard, magicsock, netstack) over a local DERP relay
// started in-process, so they need no internet access. Nothing in here mocks
// Tailcat, and the host side is the shipped Rust agent binary (the
// GAMELINK_AGENT_BINARY environment variable) driven as a subprocess through
// its public CLI, exactly as an operator would drive it.
package integration

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/netip"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/envknob"
	"tailscale.com/net/netcheck"
	"tailscale.com/syncs"
	"tailscale.com/tailcfg"
	"tailscale.com/tstest/integration"
	"tailscale.com/types/key"
	"tailscale.com/types/logger"

	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/invite"
	"github.com/gamelink-project/gamelink/shim/transport"
)

// TestMain keeps the engines hermetic, exactly as tailcat's own tests do:
// no UPnP probing of the default gateway and no captive-portal HTTP probes.
func TestMain(m *testing.M) {
	envknob.Setenv("IN_TS_TEST", "true")
	netcheck.HookStartCaptivePortalDetection.SetForTest(func(ctx context.Context, c *netcheck.Client, dm *tailcfg.DERPMap, preferredDERP tailcfg.DERPRegionID, setCaptivePortal func(bool)) (done <-chan struct{}, stop func()) {
		return syncs.ClosedChan(), func() {}
	})
	os.Exit(m.Run())
}

func quietLogf(t testing.TB) logger.Logf {
	return func(format string, args ...any) {
		if testing.Verbose() && os.Getenv("GAMELINK_TEST_TAILCAT_LOGS") != "" {
			t.Logf("  [tailcat] "+format, args...)
		}
	}
}

// startRelay runs a local DERP+STUN server and returns its region.
func startRelay(t *testing.T) *tailcfg.DERPRegion {
	t.Helper()
	dm := integration.RunDERPAndSTUN(t, quietLogf(t), "127.0.0.1")
	reg := dm.Regions[1]
	if reg == nil {
		t.Fatal("local DERP map has no region 1")
	}
	return reg
}

// tcpEcho starts a loopback TCP echo server and returns its address.
func tcpEcho(t *testing.T) netip.AddrPort {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			go func() {
				defer c.Close()
				io.Copy(c, c)
			}()
		}
	}()
	return ln.Addr().(*net.TCPAddr).AddrPort()
}

// udpEcho starts a loopback UDP echo server (one reply per datagram).
func udpEcho(t *testing.T) netip.AddrPort {
	t.Helper()
	pc, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { pc.Close() })
	go func() {
		buf := make([]byte, 65535)
		for {
			n, src, err := pc.ReadFromUDP(buf)
			if err != nil {
				return
			}
			pc.WriteToUDP(buf[:n], src)
		}
	}()
	return pc.LocalAddr().(*net.UDPAddr).AddrPort()
}

// agentBinary returns the absolute path of the shipped Rust agent. A missing
// binary is a hard failure in CI (so a misconfigured job cannot silently skip
// the end-to-end suite) and a skip on a developer machine.
func agentBinary(t testing.TB) string {
	t.Helper()
	binary := os.Getenv("GAMELINK_AGENT_BINARY")
	if binary == "" {
		if os.Getenv("CI") != "" {
			t.Fatal("GAMELINK_AGENT_BINARY must point at the built Rust agent in CI (cargo build -p gamelink-agent)")
		}
		t.Skip("set GAMELINK_AGENT_BINARY to the built Rust agent (cargo build -p gamelink-agent); required in CI")
	}
	abs, err := filepath.Abs(binary)
	if err != nil {
		t.Fatal(err)
	}
	return abs
}

// agentCLI runs one CLI command of the Rust agent and returns its stdout.
// Invitation output is a secret, so no subprocess output is ever printed;
// failures report only the command name and exit error.
func agentCLI(t testing.TB, binary, dir string, args ...string) []byte {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 40*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, binary, append([]string{"--dir", dir}, args...)...)
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("Rust agent command %s failed: %v", args[0], err)
	}
	return out
}

// writeFileAtomic replaces path without ever exposing a partial file to the
// agent's reconcile loop.
func writeFileAtomic(path string, data []byte) error {
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// agentGameState is one game's row in the agent's runtime.json.
type agentGameState struct {
	ID       string `json:"id"`
	State    string `json:"state"` // serving, inactive, error
	Detail   string `json:"detail"`
	Approved int    `json:"approved"`
	Stats    struct {
		Active   int64 `json:"active"`
		Accepted int64 `json:"accepted"`
		Denied   int64 `json:"denied"`
	} `json:"stats"`
}

// agentRuntime is the agent's runtime.json status file.
type agentRuntime struct {
	PID     int              `json:"pid"`
	Updated time.Time        `json:"updated"`
	Games   []agentGameState `json:"games"`
}

// readAgentRuntime reads runtime.json. fresh reports whether it was updated
// recently enough to mean the agent is currently running.
func readAgentRuntime(dir string) (rs agentRuntime, fresh bool, err error) {
	data, err := os.ReadFile(filepath.Join(dir, "runtime.json"))
	if err != nil {
		return rs, false, err
	}
	if err := json.Unmarshal(data, &rs); err != nil {
		return rs, false, err
	}
	return rs, time.Since(rs.Updated) < 20*time.Second, nil
}

// agentGame describes a game added to an agentEnv.
type agentGame struct {
	ID         string
	Name       string
	Type       games.Type
	Protocol   games.Protocol
	TunnelPort uint16
}

// agentKeys are a game's persistent Tailcat identity as stored by the agent.
type agentKeys struct {
	Private key.NodePrivate
	PSK     tailcat.PresharedKey
	Region  *tailcfg.DERPRegion
}

// Addr derives the full Tailcat address (a secret) from the saved keys,
// independently of the Rust agent, the same way the shim computes it.
func (k agentKeys) Addr() tailcat.Addr {
	ci := tailcat.ConnInfo{
		ServerPublic:      tailcat.NodePublic{NodePublic: k.Private.Public()},
		ServerDiscoPublic: tailcat.DiscoPublicForNode(k.Private),
		PresharedKey:      k.PSK,
		Region:            []*tailcfg.DERPRegion{k.Region},
	}
	return ci.Addr()
}

// agentEnv is a real agent configuration directory driven through the shipped
// Rust agent binary: CLI commands for configuration, and a `serve` child.
type agentEnv struct {
	t        *testing.T
	binary   string
	dir      string
	region   *tailcfg.DERPRegion // nil: use the real public relay map (external tests)
	games    map[string]agentGame
	approved map[string]int // devices currently approved per game, as set through this env

	cmd     *exec.Cmd
	done    chan struct{}
	waitErr error
}

// Generous per-wait deadline: the agent reconciles once per second.
const agentWait = 30 * time.Second

func newAgentEnv(t *testing.T, region *tailcfg.DERPRegion) *agentEnv {
	t.Helper()
	e := &agentEnv{
		t: t, binary: agentBinary(t), dir: filepath.Join(t.TempDir(), "agent"), region: region,
		games: map[string]agentGame{}, approved: map[string]int{},
	}
	e.cli("init")
	return e
}

func (e *agentEnv) cli(args ...string) []byte {
	e.t.Helper()
	return agentCLI(e.t, e.binary, e.dir, args...)
}

// addGame exposes a game through the agent CLI. With a local relay, the CLI is
// given a placeholder relay host (it exposes no insecure/testing flags) and the
// saved region is then pointed at the hermetic local DERP.
func (e *agentEnv) addGame(id string, typ games.Type, proto games.Protocol, port uint16, dest netip.AddrPort) agentGame {
	e.t.Helper()
	args := []string{"games", "add", "--type", string(typ), "--id", id, "--name", id, "--port", fmt.Sprint(port), "--dest", dest.String()}
	if typ == games.Custom {
		args = append(args, "--proto", string(proto))
	}
	if e.region != nil {
		args = append(args, "--relay-host", "derp.example.com")
	}
	e.cli(args...)
	if e.region != nil {
		keyPath := filepath.Join(e.dir, "keys", id+".json")
		data, err := os.ReadFile(keyPath)
		if err != nil {
			e.t.Fatal(err)
		}
		var keys map[string]any
		if json.Unmarshal(data, &keys) != nil {
			e.t.Fatal("invalid key file written by the Rust agent")
		}
		keys["region"] = e.region
		data, _ = json.Marshal(keys)
		if err := writeFileAtomic(keyPath, data); err != nil {
			e.t.Fatal(err)
		}
	}
	g := agentGame{ID: id, Name: id, Type: typ, Protocol: proto, TunnelPort: port}
	e.games[id] = g
	return g
}

// approve approves a device key for a game through the CLI.
func (e *agentEnv) approve(id, pub, label string) {
	e.t.Helper()
	e.cli("devices", "approve", "--game", id, "--key", pub, "--label", label)
	e.approved[id]++
}

// revoke revokes a device through the CLI; a running agent notices by polling.
func (e *agentEnv) revoke(id, pub string) {
	e.t.Helper()
	e.cli("devices", "revoke", "--game", id, "--key", pub)
	e.approved[id]--
}

// keys loads a game's saved identity.
func (e *agentEnv) keys(id string) agentKeys {
	e.t.Helper()
	data, err := os.ReadFile(filepath.Join(e.dir, "keys", id+".json"))
	if err != nil {
		e.t.Fatal(err)
	}
	var raw struct {
		PrivateKey   string              `json:"privateKey"`
		PresharedKey string              `json:"presharedKey"`
		Region       *tailcfg.DERPRegion `json:"region"`
	}
	if err := json.Unmarshal(data, &raw); err != nil {
		e.t.Fatal("invalid key file")
	}
	var k agentKeys
	if k.Private.UnmarshalText([]byte(raw.PrivateKey)) != nil || k.Private.IsZero() {
		e.t.Fatal("key file holds an invalid private key")
	}
	if k.PSK.UnmarshalText([]byte(raw.PresharedKey)) != nil || k.PSK.IsZero() {
		e.t.Fatal("key file holds an invalid pre-shared key")
	}
	k.Region = raw.Region
	return k
}

// invite returns the invitation the shipped CLI prints for a game, parsed.
// Invitations are secrets and are never logged.
func (e *agentEnv) invite(id string) invite.Invite {
	e.t.Helper()
	args := []string{"invite", "--game", id}
	opts := invite.Options{}
	if e.region != nil {
		args = append(args, "--allow-local-relay")
		opts.AllowLocalRelay = true
	}
	inv, _, err := invite.Parse(strings.TrimSpace(string(e.cli(args...))), opts)
	if err != nil {
		e.t.Fatalf("invitation printed by the Rust agent did not parse: %v", err)
	}
	return inv
}

// runtime returns the status published by the current serve child.
func (e *agentEnv) runtime() (agentRuntime, bool) {
	if e.cmd == nil {
		return agentRuntime{}, false
	}
	rs, _, err := readAgentRuntime(e.dir)
	// A killed or restarted agent leaves a stale file with another pid.
	if err != nil || rs.PID != e.cmd.Process.Pid {
		return agentRuntime{}, false
	}
	return rs, true
}

func (e *agentEnv) gameState(id string) (agentGameState, bool) {
	rs, ok := e.runtime()
	if !ok {
		return agentGameState{}, false
	}
	for _, g := range rs.Games {
		if g.ID == id {
			return g, true
		}
	}
	return agentGameState{}, false
}

// serve starts `gamelink-agent serve` as a child and waits for its first
// published status. Stop it with stop(); Cleanup also stops it.
func (e *agentEnv) serve() {
	e.t.Helper()
	if e.cmd != nil {
		e.t.Fatal("agent is already serving")
	}
	cmd := exec.Command(e.binary, "--dir", e.dir, "serve")
	cmd.Stdout, cmd.Stderr = io.Discard, io.Discard // never print: output may include secrets
	if err := cmd.Start(); err != nil {
		e.t.Fatal(err)
	}
	e.cmd = cmd
	done := make(chan struct{})
	e.done = done
	e.waitErr = nil
	go func() { e.waitErr = cmd.Wait(); close(done) }()
	e.t.Cleanup(e.stop)
	deadline := time.Now().Add(agentWait)
	for time.Now().Before(deadline) {
		if rs, ok := e.runtime(); ok && len(rs.Games) == len(e.games) {
			return
		}
		select {
		case <-done:
			e.t.Fatalf("Rust agent exited during startup: %v", e.waitErr)
		case <-time.After(50 * time.Millisecond):
		}
	}
	e.t.Fatal("Rust agent never published its first runtime status")
}

// stop shuts the agent down cleanly (SIGTERM, which the agent turns into an
// explicit resource shutdown), falling back to kill. It is idempotent.
func (e *agentEnv) stop() {
	if e.cmd == nil {
		return
	}
	cmd, done := e.cmd, e.done
	e.cmd = nil
	select {
	case <-done:
		return
	default:
	}
	_ = cmd.Process.Signal(syscall.SIGTERM)
	select {
	case <-done:
	case <-time.After(15 * time.Second):
		_ = cmd.Process.Kill()
		select {
		case <-done:
		case <-time.After(10 * time.Second):
			e.t.Error("Rust agent did not exit")
		}
		e.t.Error("Rust agent did not stop on SIGTERM within 15s")
	}
}

// isServing reports whether the game is currently served with every device
// approved through this env.
func (e *agentEnv) isServing(id string) bool {
	g, ok := e.gameState(id)
	return ok && g.State == "serving" && g.Approved == e.approved[id]
}

// addr waits until the game is served with all approved devices and returns
// its live Tailcat address (as printed by the shipped `invite` command).
func (e *agentEnv) addr(id string) tailcat.Addr {
	e.t.Helper()
	deadline := time.Now().Add(agentWait)
	for time.Now().Before(deadline) {
		if e.isServing(id) {
			return tailcat.Addr(e.invite(id).Addr)
		}
		time.Sleep(50 * time.Millisecond)
	}
	e.t.Fatalf("game %q is not being served: %+v", id, e.lastState(id))
	return ""
}

func (e *agentEnv) lastState(id string) any {
	g, _ := e.gameState(id)
	return g
}

// waitState waits for a game to reach a runtime state.
func (e *agentEnv) waitState(id, state string) {
	e.t.Helper()
	deadline := time.Now().Add(agentWait)
	for time.Now().Before(deadline) {
		if g, ok := e.gameState(id); ok && g.State == state {
			return
		}
		time.Sleep(50 * time.Millisecond)
	}
	e.t.Fatalf("game %q never reached state %q: %+v", id, state, e.lastState(id))
}

// statsAfter waits for a status publication that is certainly newer than t0
// (runtime.json has one-second timestamps and is rewritten at least every five
// seconds) and returns the game's row from it.
func (e *agentEnv) statsAfter(id string, t0 time.Time) agentGameState {
	e.t.Helper()
	min := t0.Truncate(time.Second).Add(time.Second)
	deadline := time.Now().Add(agentWait)
	for time.Now().Before(deadline) {
		if rs, ok := e.runtime(); ok && !rs.Updated.Before(min) {
			for _, g := range rs.Games {
				if g.ID == id {
					return g
				}
			}
		}
		time.Sleep(100 * time.Millisecond)
	}
	e.t.Fatalf("agent never published fresh stats for %q", id)
	return agentGameState{}
}

func newKey() (key.NodePrivate, string) {
	k := key.NewNode()
	return k, k.Public().String()
}

func joinOpts(addr tailcat.Addr, proto games.Protocol, port uint16, id key.NodePrivate) transport.TunnelOptions {
	return transport.TunnelOptions{
		Name: "test", Proto: proto, Addr: addr, TunnelPort: port, Identity: id,
		AllowLocalRelay: true, // hermetic relay tests bind loopback
		AutoReconnect:   true,
		HealthInterval:  500 * time.Millisecond,
		ConnectTimeout:  20 * time.Second,
		DialTimeout:     5 * time.Second,
	}
}

// desktopOpts are the tunnel options the shipped desktop host sends to the Go
// shim's `join` (apps/desktop-tauri/src/host.rs): only name, protocol, port,
// preferred port and auto-reconnect are set, so every timeout is a production
// default. AllowLocalRelay is the user setting the hermetic relay needs.
func desktopOpts(addr tailcat.Addr, proto games.Protocol, port uint16, id key.NodePrivate, autoReconnect bool) transport.TunnelOptions {
	return transport.TunnelOptions{
		Name: "test", Proto: proto, Addr: addr, TunnelPort: port, PreferPort: port, Identity: id,
		AllowLocalRelay: true, AutoReconnect: autoReconnect,
	}
}

// roundTripTCP sends msg to addr and returns what the echo returned.
func roundTripTCP(t *testing.T, addr string, msg string) string {
	t.Helper()
	c, err := net.DialTimeout("tcp", addr, 5*time.Second)
	if err != nil {
		t.Fatalf("dial %s: %v", addr, err)
	}
	defer c.Close()
	c.SetDeadline(time.Now().Add(15 * time.Second))
	if _, err := io.WriteString(c, msg); err != nil {
		t.Fatalf("write: %v", err)
	}
	buf := make([]byte, len(msg))
	if _, err := io.ReadFull(c, buf); err != nil {
		t.Fatalf("read echo: %v", err)
	}
	return string(buf)
}
