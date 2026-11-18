// Package main exports semantic Tailcat operations, never CLI parsing or policy.
package main

/*
#include <stdint.h>
#include <stdlib.h>
typedef struct { uint8_t *ptr; size_t len; } AgentBuffer;
*/
import "C"
import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/netip"
	"sync"
	"time"
	"unsafe"

	"github.com/gamelink-project/gamelink/shim/consolebridge"
	"github.com/gamelink-project/gamelink/shim/gamehost"
	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/hosthttp"
	"github.com/gamelink-project/gamelink/shim/security"
	"github.com/gamelink-project/gamelink/shim/transport"
)

const maxInput = 32 * 1024
const maxOutput = 4 * 1024 * 1024

var owner struct {
	sync.Mutex
	initialized bool
	running     map[resourceID]*transport.GameServer
}

var logs struct {
	sync.Mutex
	lines []string
}

func record(red *security.Redactor, format string, args ...any) {
	line := red.Redact(fmt.Sprintf(format, args...))
	logs.Lock()
	defer logs.Unlock()
	if len(logs.lines) == 128 {
		logs.lines = logs.lines[1:]
	}
	logs.lines = append(logs.lines, line)
}

type bridgeSpec struct {
	ServerIP       string
	BindIP         string
	DiscoveryPort  uint16
	MediaPortFirst uint16
	MediaPortLast  uint16
}
type gameSpec struct {
	Type          games.Type
	Protocol      games.Protocol
	TunnelPort    uint16
	Dest          string
	KnockEnabled  bool
	ConsoleBridge *bridgeSpec
	Bedrock       *struct{ MediaPortFirst, MediaPortLast uint16 }
}
type resourceID struct {
	ID         string
	Generation uint64
}

func dispatch(data []byte) (any, error) {
	if len(data) > maxInput {
		return nil, errors.New("agent request exceeds size limit")
	}
	var r struct {
		Method          string
		Verbose         bool
		Dir             string
		ID              string
		Host            string
		RegionID        int
		AllowLocalRelay bool
		Generation      uint64
		Game            gameSpec
		Keys            gamehost.Keys
		Approved        []string
	}
	if json.Unmarshal(data, &r) != nil {
		return nil, errors.New("invalid agent request")
	}
	if r.Method == "http" {
		return hosthttp.Do(data)
	}
	owner.Lock()
	defer owner.Unlock()
	if r.Method == "init" {
		if owner.initialized {
			return nil, errors.New("agent is already initialized")
		}
		owner.initialized = true
		owner.running = map[resourceID]*transport.GameServer{}
		return nil, nil
	}
	if r.Method == "shutdown" {
		for id, server := range owner.running {
			server.Close()
			delete(owner.running, id)
		}

		owner.initialized = false
		return nil, nil
	}
	if !owner.initialized {
		return nil, errors.New("agent is not initialized")
	}
	switch r.Method {
	case "generate":
		return gamehost.Generate(), nil
	case "resolve":
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		return transport.ResolveRegion(ctx, transport.RegionSpec{Host: r.Host, RegionID: r.RegionID})
	case "address":
		return r.Keys.Address(r.AllowLocalRelay)
	case "start":
		id := resourceID{r.ID, r.Generation}
		if r.Generation == 0 || r.ID == "" {
			return nil, errors.New("invalid resource generation")
		}
		for live := range owner.running {
			if live.ID == r.ID {
				return nil, errors.New("game resource already exists")
			}
		}
		private, psk, err := r.Keys.Parse()
		if err != nil {
			return nil, err
		}
		approved, err := gamehost.ParseApproved(r.Approved)
		if err != nil {
			return nil, err
		}
		dest, err := netip.ParseAddrPort(r.Game.Dest)
		if err != nil {
			return nil, errors.New("invalid destination")
		}
		var host *consolebridge.Host
		if b := r.Game.ConsoleBridge; b != nil && len(approved) != 0 {
			host, err = consolebridge.NewHost(consolebridge.HostConfig{ServerIP: netip.MustParseAddr(b.ServerIP), BindIP: netip.MustParseAddr(b.BindIP), DiscoveryPort: b.DiscoveryPort, MediaPortFirst: b.MediaPortFirst, MediaPortLast: b.MediaPortLast})
			if err != nil {
				return nil, err
			}
		}
		opts := transport.ServerOptions{ID: r.ID, Game: r.Game.Type, Protocol: r.Game.Protocol, TunnelPort: r.Game.TunnelPort, Dest: dest, Private: private, PSK: psk, Region: r.Keys.Region, Approved: approved, CollectRequests: r.Game.KnockEnabled || len(approved) != 0, ExpiresAt: r.Keys.ExpiresAt}
		if b := r.Game.Bedrock; b != nil {
			opts.BedrockMediaPortFirst, opts.BedrockMediaPortLast = b.MediaPortFirst, b.MediaPortLast
		}
		if host != nil {
			opts.ConsoleBridge = host
		}
		red := security.NewRedactor(r.Keys.PrivateKey, r.Keys.PresharedKey)
		opts.Event = func(kind, message string) { record(red, "[%s] %s: %s", r.ID, kind, message) }
		if r.Verbose {
			opts.Logf = func(format string, args ...any) { record(red, "tailcat: "+format, args...) }
		}
		server, err := transport.StartGameServer(opts)
		if err != nil {
			if host != nil {
				host.Close()
			}
			return nil, err
		}
		owner.running[id] = server
		return nil, nil
	case "stop":
		id := resourceID{r.ID, r.Generation}
		if server := owner.running[id]; server != nil {
			server.Close()
			delete(owner.running, id)
		}
		return nil, nil
	case "logs":
		logs.Lock()
		defer logs.Unlock()
		out := logs.lines
		logs.lines = nil
		return out, nil
	case "stats":
		result := map[string]transport.ServerStats{}
		for id, server := range owner.running {
			result[id.ID] = server.Stats()
		}
		return result, nil
	case "requests":
		result := map[string][]transport.JoinRequest{}
		for id, server := range owner.running {
			if requests := server.TakeRequests(); len(requests) != 0 {
				result[id.ID] = requests
			}
		}
		return result, nil

	default:
		return nil, errors.New("unknown agent operation")
	}
}
func response(data []byte) (out []byte) {
	defer func() {
		if recover() != nil {
			out = []byte(`{"error":"agent operation failed"}`)
		}
	}()
	v, err := dispatch(data)
	if err != nil {
		out, _ = json.Marshal(map[string]any{"error": err.Error()})
	} else {
		out, _ = json.Marshal(map[string]any{"ok": v})
	}
	if len(out) > maxOutput {
		return []byte(`{"error":"agent response exceeds size limit"}`)
	}
	return
}

//export gamelink_agent_call
func gamelink_agent_call(ptr *C.uint8_t, length C.size_t) C.AgentBuffer {
	var out []byte
	if length > maxInput || (length > 0 && ptr == nil) {
		out = []byte(`{"error":"invalid agent request size"}`)
	} else {
		out = response(C.GoBytes(unsafe.Pointer(ptr), C.int(length)))
	}
	return C.AgentBuffer{ptr: (*C.uint8_t)(C.CBytes(out)), len: C.size_t(len(out))}
}

//export gamelink_agent_free
func gamelink_agent_free(buf C.AgentBuffer) {
	if buf.ptr != nil {
		clear(unsafe.Slice((*byte)(unsafe.Pointer(buf.ptr)), int(buf.len)))
		C.free(unsafe.Pointer(buf.ptr))
	}
}
func main() {}
