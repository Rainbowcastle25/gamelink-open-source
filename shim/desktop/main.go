// The desktop archive owns only Tailcat resources. Rust owns the application,
// persistent storage, reducer state and iteration of generation-tagged effects.
package main

/*
#include <stdint.h>
#include <stdlib.h>
typedef struct { uint8_t *ptr; size_t len; } DesktopBuffer;
*/
import "C"
import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sync"
	"time"
	"unsafe"

	"github.com/gamelink-project/gamelink/shim/gamehost"
	"github.com/gamelink-project/gamelink/shim/hosthttp"
	"github.com/gamelink-project/gamelink/shim/invite"
	"github.com/gamelink-project/gamelink/shim/security"
	"github.com/gamelink-project/gamelink/shim/transport"
	"github.com/tailscale/tailcat"
	"tailscale.com/types/key"
)

const maxInput = 32 * 1024
const maxOutput = 4 * 1024 * 1024

type resourceID struct {
	ID         string
	Generation uint64
}
type resource struct {
	cancel context.CancelFunc
	done   chan struct{}
	tun    *transport.Tunnel
}

var owner struct {
	sync.Mutex
	initialized bool
	private     key.NodePrivate
	resources   map[resourceID]*resource
	hosts       map[resourceID]hostedResource
	lan         *lanListener
}
var mailbox struct {
	sync.Mutex
	events map[resourceID]map[string]any
}

// At most one latest status and one terminal result per owned generation.
// Release removes the slot only after the producer is joined.
func event(id resourceID, kind string, value any) {
	mailbox.Lock()
	defer mailbox.Unlock()
	if slot := mailbox.events[id]; slot != nil {
		slot[kind] = value
	}
}
func dispatch(input []byte) (any, error) {
	if len(input) > maxInput {
		return nil, errors.New("desktop request exceeds size limit")
	}
	var r struct {
		Method          string
		Verbose         bool
		ID              string
		Generation      uint64
		PrivateKey      string
		Address         string
		AllowLocalRelay bool
		Options         transport.TunnelOptions
		Game            hostedGame
		Keys            gamehost.Keys
		Approved        []string
	}
	if json.Unmarshal(input, &r) != nil {
		return nil, errors.New("invalid desktop request")
	}
	if r.Method == "http" {
		return hosthttp.Do(input)
	}
	if r.Method == "host_resolve" {
		return resolveHostRelay() // network I/O must not block stop or revocation
	}
	owner.Lock()
	defer owner.Unlock()
	if r.Method == "init" {
		if owner.initialized {
			return nil, errors.New("desktop is already initialized")
		}
		owner.resources = map[resourceID]*resource{}
		owner.hosts = map[resourceID]hostedResource{}
		mailbox.events = map[resourceID]map[string]any{}
		owner.initialized = true
		return nil, nil
	}
	if r.Method == "shutdown" {
		if owner.lan != nil {
			owner.lan.close()
			owner.lan = nil
		}
		for id := range owner.hosts {
			stopHost(id)
		}
		for id, res := range owner.resources {
			res.cancel()
			<-res.done
			if res.tun != nil {
				res.tun.Stop()
			}
			delete(owner.resources, id)
		}
		mailbox.Lock()
		mailbox.events = nil
		mailbox.Unlock()
		owner.private = key.NodePrivate{}
		owner.initialized = false
		return nil, nil
	}
	if !owner.initialized {
		return nil, errors.New("desktop is not initialized")
	}
	switch r.Method {
	case "host_generate":
		return gamehost.Generate(), nil
	case "host_address":
		return r.Keys.Address(false)
	case "host_start":
		return nil, startHost(resourceID{r.ID, r.Generation}, r.Game, r.Keys, r.Approved)
	case "host_stop":
		return stopHost(resourceID{r.ID, r.Generation})
	case "host_peers":
		snapshots := make(map[string]hostSnapshot, len(owner.hosts))
		for id, host := range owner.hosts {
			snapshots[id.ID] = hostSnapshot{Connected: host.peers.Snapshot(), Requests: host.server.TakeRequests()}
		}
		return snapshots, nil
	case "lan_scan":
		if owner.lan == nil {
			l, err := listenLAN()
			if err != nil {
				return nil, err
			}
			owner.lan = l
		}
		packets, err := owner.lan.snapshot()
		if err != nil {
			owner.lan.close()
			owner.lan = nil
		}
		return packets, err
	case "lan_packets":
		if owner.lan == nil {
			return nil, nil
		}
		return owner.lan.snapshot()
	case "lan_close":
		if owner.lan != nil {
			owner.lan.close()
			owner.lan = nil
		}
		return nil, nil
	case "generate":
		private := key.NewNode()
		text, _ := private.MarshalText()
		return map[string]any{"privateKey": string(text), "publicKey": private.Public().String()}, nil
	case "identity":
		if owner.private.UnmarshalText([]byte(r.PrivateKey)) != nil || owner.private.IsZero() {
			return nil, errors.New("invalid device identity")
		}
		return owner.private.Public().String(), nil
	case "validate":
		return invite.ValidateAddr(r.Address, invite.Options{AllowLocalRelay: r.AllowLocalRelay})
	case "events":
		mailbox.Lock()
		defer mailbox.Unlock()
		out := []map[string]any{}
		for id, slot := range mailbox.events {
			for kind, value := range slot {
				out = append(out, map[string]any{"id": id.ID, "generation": id.Generation, "kind": kind, "value": value})
				delete(slot, kind)
			}
		}
		return out, nil
	case "stats":
		out := map[string]transport.Status{}
		for id, res := range owner.resources {
			select {
			case <-res.done:
				if res.tun != nil {
					out[id.ID] = res.tun.Status()
				}
			default:
			}
		}
		return out, nil
	case "join", "check":
		id := resourceID{r.ID, r.Generation}
		if r.ID == "" || r.Generation == 0 {
			return nil, errors.New("invalid resource generation")
		}
		for live := range owner.resources {
			if live.ID == r.ID {
				return nil, errors.New("resource must be stopped before replacement")
			}
		}
		ctx, cancel := context.WithCancel(context.Background())
		res := &resource{cancel: cancel, done: make(chan struct{})}
		owner.resources[id] = res
		mailbox.Lock()
		mailbox.events[id] = map[string]any{}
		mailbox.Unlock()
		private := owner.private
		if r.Method == "join" {
			opts := r.Options
			opts.Identity = private
			opts.Addr = tailcat.Addr(r.Address)
			opts.AllowLocalRelay = r.AllowLocalRelay
			opts.OnStatus = func(s transport.Status) { event(id, "status", s) }
			if r.Verbose {
				raw, _ := private.MarshalText()
				red := security.NewRedactor(string(raw), r.Address)
				if ci, err := tailcat.ParseAddr(tailcat.Addr(r.Address)); err == nil {
					psk, _ := ci.PresharedKey.MarshalText()
					red.Add(string(psk))
				}
				opts.Logf = func(format string, args ...any) { event(id, "log", red.Redact(fmt.Sprintf(format, args...))) }
			}
			go func() {
				defer close(res.done)
				tun, err := transport.Join(ctx, opts)
				res.tun = tun
				result := map[string]any{"hasTunnel": tun != nil, "failed": err != nil, "cancelled": errors.Is(err, context.Canceled), "noResponse": errors.Is(err, transport.ErrNoResponse)}
				if err != nil {
					result["error"] = err.Error()
				}
				if tun != nil {
					result["status"] = tun.Status()
				}
				event(id, "join_end", result)
			}()
		} else {
			go func() {
				defer close(res.done)
				timeout, stop := context.WithTimeout(ctx, 20*time.Second)
				defer stop()
				opts := r.Options
				opts.Addr, opts.Identity, opts.AllowLocalRelay = tailcat.Addr(r.Address), private, r.AllowLocalRelay
				latency, path, reachable, err := transport.CheckEndpoint(timeout, opts)
				result := map[string]any{"outcome": "online", "latencyMs": float64(latency) / float64(time.Millisecond), "path": string(path), "gameReachable": reachable}
				if errors.Is(err, context.Canceled) {
					result["outcome"] = "cancelled"
				} else if err != nil {
					result["outcome"] = "error"
				}
				event(id, "check_end", result)
			}()
		}
		return nil, nil
	case "stop":
		id := resourceID{r.ID, r.Generation}
		if res := owner.resources[id]; res != nil {
			res.cancel()
			<-res.done
			if res.tun != nil {
				res.tun.Stop()
			}
			delete(owner.resources, id)
		}
		mailbox.Lock()
		delete(mailbox.events, id)
		mailbox.Unlock()
		return nil, nil
	default:
		return nil, errors.New("unknown desktop operation")
	}
}
func response(input []byte) (out []byte) {
	defer func() {
		if recover() != nil {
			out = []byte(`{"error":"desktop command failed"}`)
		}
	}()
	v, err := dispatch(input)
	if err != nil {
		out, _ = json.Marshal(map[string]any{"error": err.Error()})
	} else {
		out, err = json.Marshal(map[string]any{"ok": v})
		if err != nil {
			return []byte(`{"error":"desktop response failed"}`)
		}
	}
	if len(out) > maxOutput {
		return []byte(`{"error":"desktop response exceeds size limit"}`)
	}
	return out
}

//export gamelink_desktop_call
func gamelink_desktop_call(ptr *C.uint8_t, length C.size_t) C.DesktopBuffer {
	var out []byte
	if length > maxInput || (length > 0 && ptr == nil) {
		out = []byte(`{"error":"invalid desktop request size"}`)
	} else {
		out = response(C.GoBytes(unsafe.Pointer(ptr), C.int(length)))
	}
	return C.DesktopBuffer{ptr: (*C.uint8_t)(C.CBytes(out)), len: C.size_t(len(out))}
}

//export gamelink_desktop_free
func gamelink_desktop_free(buf C.DesktopBuffer) {
	// Buffers belong to C malloc, never Go. Exactly one free by the Rust caller.
	if buf.ptr != nil {
		clear(unsafe.Slice((*byte)(unsafe.Pointer(buf.ptr)), int(buf.len)))
		C.free(unsafe.Pointer(buf.ptr))
	}
}
func main() {}
