package integration

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"testing"
	"time"

	"tailscale.com/envknob"

	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/transport"
)

// A corrupt configuration snapshot must stop serving (fail closed) rather than
// keep exposing the cached configuration.
func TestInvalidConfigFailsClosed(t *testing.T) {
	relay := startRelay(t)
	env := newAgentEnv(t, relay)
	env.addGame("game", games.Custom, games.TCP, 17003, tcpEcho(t))
	devKey, devicePub := newKey()
	env.approve("game", devicePub, "approved")
	env.serve()
	addr := env.addr("game")
	// The approved device really is served before the corruption.
	tun, err := transport.Join(context.Background(), joinOpts(addr, games.TCP, 17003, devKey))
	if err != nil {
		t.Fatalf("approved device could not join before the corruption: %v", err)
	}
	defer tun.Stop()

	if err := writeFileAtomic(filepath.Join(env.dir, "agent.json"), []byte("{")); err != nil {
		t.Fatal(err)
	}
	env.waitState("game", "error")
	if env.isServing("game") {
		t.Fatal("server remained reachable after config became invalid")
	}
	// Not merely reported as an error: the game is unreachable for the
	// approved device that held the address.
	opts := joinOpts(addr, games.TCP, 17003, devKey)
	opts.ConnectTimeout = 3 * time.Second
	if again, err := transport.Join(context.Background(), opts); err == nil {
		again.Stop()
		t.Fatal("approved device could join after the configuration became invalid")
	}
}

// TestResourceProfile is an opt-in, real-Tailcat resource profile used by
// scripts/measure-resources.py. It is intentionally excluded from normal test
// runs because each profile starts a local DERP/STUN relay and tunnel clients.
//
// Two processes are sampled at every phase and emitted as separate RESOURCE
// lines: "test-process" is this test binary, which hosts the in-process DERP/STUN
// relay and the tunnel clients (the Go side, as before), and "agent" is the
// shipped Rust agent child process that serves the games.
func TestResourceProfile(t *testing.T) {
	if os.Getenv("GAMELINK_RESOURCE_PROFILE") != "1" {
		t.Skip("set GAMELINK_RESOURCE_PROFILE=1 to run the resource profile")
	}
	relay := startRelay(t)
	tcpDest := tcpEcho(t)
	udpDest := udpEcho(t)
	env := newAgentEnv(t, relay)
	env.addGame("tcp", games.Custom, games.TCP, 17001, tcpDest)
	env.addGame("udp", games.Custom, games.UDP, 17002, udpDest)
	env.serve()

	profileSample(t, env, "disconnected-idle", "none")
	tcpKey, tcpPub := newKey()
	env.approve("tcp", tcpPub, "profile-tcp")
	udpKey, udpPub := newKey()
	env.approve("udp", udpPub, "profile-udp")
	tcpTun, err := transport.Join(context.Background(), joinOpts(env.addr("tcp"), games.TCP, 17001, tcpKey))
	if err != nil {
		t.Fatal(err)
	}
	defer tcpTun.Stop()
	profileSample(t, env, "connected-idle-tcp", string(tcpTun.Status().Path))
	for i := 0; i < 20; i++ {
		if got := roundTripTCP(t, tcpTun.Status().LocalAddr, strings.Repeat("x", 4096)); got != strings.Repeat("x", 4096) {
			t.Fatal("TCP profile echo mismatch")
		}
	}
	profileSample(t, env, "active-tcp", string(tcpTun.Status().Path))

	udpTun, err := transport.Join(context.Background(), joinOpts(env.addr("udp"), games.UDP, 17002, udpKey))
	if err != nil {
		t.Fatal(err)
	}
	defer udpTun.Stop()
	profileSample(t, env, "connected-idle-udp", string(udpTun.Status().Path))
	udpConn, err := net.Dial("udp", udpTun.Status().LocalAddr)
	if err != nil {
		t.Fatal(err)
	}
	defer udpConn.Close()
	_ = udpConn.SetDeadline(time.Now().Add(10 * time.Second))
	for i := 0; i < 20; i++ {
		payload := []byte(fmt.Sprintf("profile-%03d", i))
		if _, err := udpConn.Write(payload); err != nil {
			t.Fatal(err)
		}
		buf := make([]byte, len(payload))
		if _, err := udpConn.Read(buf); err != nil || string(buf) != string(payload) {
			t.Fatalf("UDP profile echo: %q, %v", buf, err)
		}
	}
	profileSample(t, env, "active-udp", string(udpTun.Status().Path))

	// Force the in-process Tailcat client to use DERP, while reporting the
	// observed path in each emitted sample. (The agent child process started
	// earlier and is unaffected by this knob; it answers over whatever path the
	// client opens.)
	envknob.Setenv("TS_DEBUG_ALWAYS_USE_DERP", "true")
	relayKey, relayPub := newKey()
	env.approve("tcp", relayPub, "profile-relay")
	relayTun, err := transport.Join(context.Background(), joinOpts(env.addr("tcp"), games.TCP, 17001, relayKey))
	if err != nil {
		t.Fatal(err)
	}
	profileSample(t, env, "connected-idle-relay", string(relayTun.Status().Path))
	for i := 0; i < 3; i++ {
		relayTun.Stop()
		relayTun, err = transport.Join(context.Background(), joinOpts(env.addr("tcp"), games.TCP, 17001, relayKey))
		if err != nil {
			t.Fatalf("cleanup cycle %d reconnect: %v", i+1, err)
		}
		relayTun.Stop()
	}
	profileSample(t, env, "cleanup-cycles-3", string(tcpTun.Status().Path))
	relayTun.Stop()
	udpTun.Stop()
	tcpTun.Stop()
	profileSample(t, env, "disconnected-after-cleanup", "none")
	env.stop()
}

type resourceSnapshot struct {
	Process     string `json:"process"` // "test-process" or "agent"
	Phase       string `json:"phase"`
	Path        string `json:"path"`
	RSSBytes    int64  `json:"rss_bytes"`
	AllocBytes  uint64 `json:"alloc_bytes"`       // Go heap; test-process only
	TotalAlloc  uint64 `json:"total_alloc_bytes"` // Go heap; test-process only
	Goroutines  int    `json:"goroutines"`        // test-process only
	Threads     int    `json:"threads"`
	OpenFDs     int    `json:"open_fds"`
	UserTicks   int64  `json:"user_ticks"`
	SystemTicks int64  `json:"system_ticks"`
	Voluntary   int64  `json:"voluntary_context_switches"`
	Involuntary int64  `json:"involuntary_context_switches"`
	ReadBytes   int64  `json:"read_bytes"`
	WriteBytes  int64  `json:"write_bytes"`
}

// profileSample emits one RESOURCE line for this process and, while it is
// running, one for the Rust agent child.
func profileSample(t *testing.T, env *agentEnv, phase, path string) {
	t.Helper()
	s := readProc("self", "test-process", phase, path)
	s.Goroutines = runtime.NumGoroutine()
	var mem runtime.MemStats
	runtime.ReadMemStats(&mem)
	s.AllocBytes, s.TotalAlloc = mem.Alloc, mem.TotalAlloc
	emitResource(t, s)
	if env != nil && env.cmd != nil {
		emitResource(t, readProc(strconv.Itoa(env.cmd.Process.Pid), "agent", phase, path))
	}
}

func emitResource(t *testing.T, s resourceSnapshot) {
	t.Helper()
	data, err := json.Marshal(s)
	if err != nil {
		t.Fatal(err)
	}
	t.Logf("RESOURCE %s", data)
}

// readProc reads /proc/<proc> ("self" or a pid) counters.
func readProc(proc, process, phase, path string) resourceSnapshot {
	s := resourceSnapshot{Process: process, Phase: phase, Path: path}
	base := "/proc/" + proc
	if data, err := os.ReadFile(base + "/status"); err == nil {
		for _, line := range strings.Split(string(data), "\n") {
			fields := strings.Fields(line)
			if len(fields) < 2 {
				continue
			}
			switch fields[0] {
			case "VmRSS:":
				if n, e := strconv.ParseInt(fields[1], 10, 64); e == nil {
					s.RSSBytes = n * 1024
				}
			case "Threads:":
				if n, e := strconv.Atoi(fields[1]); e == nil {
					s.Threads = n
				}
			case "voluntary_ctxt_switches:":
				if n, e := strconv.ParseInt(fields[1], 10, 64); e == nil {
					s.Voluntary = n
				}
			case "nonvoluntary_ctxt_switches:":
				if n, e := strconv.ParseInt(fields[1], 10, 64); e == nil {
					s.Involuntary = n
				}
			}
		}
	}
	if entries, err := os.ReadDir(base + "/fd"); err == nil {
		s.OpenFDs = len(entries)
	}
	if data, err := os.ReadFile(base + "/stat"); err == nil {
		if close := strings.LastIndexByte(string(data), ')'); close >= 0 {
			fields := strings.Fields(string(data[close+1:]))
			// After the command name, fields begin at proc stat field 3
			// (state); utime/stime are fields 14 and 15.
			if len(fields) > 12 {
				s.UserTicks, _ = strconv.ParseInt(fields[11], 10, 64)
				s.SystemTicks, _ = strconv.ParseInt(fields[12], 10, 64)
			}
		}
	}
	if data, err := os.ReadFile(base + "/io"); err == nil {
		for _, line := range strings.Split(string(data), "\n") {
			fields := strings.Fields(line)
			if len(fields) != 2 {
				continue
			}
			n, e := strconv.ParseInt(fields[1], 10, 64)
			if e != nil {
				continue
			}
			switch fields[0] {
			case "read_bytes:":
				s.ReadBytes = n
			case "write_bytes:":
				s.WriteBytes = n
			}
		}
	}
	return s
}
