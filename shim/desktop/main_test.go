package main

import (
	"encoding/json"
	"strings"
	"sync"
	"testing"
)

func TestOwnedNetworkBoundary(t *testing.T) {
	invoke := func(method string) (any, error) { return dispatch([]byte(`{"method":"` + method + `"}`)) }
	invoke("shutdown")
	if _, err := invoke("generate"); err == nil {
		t.Fatal("uninitialized operation")
	}
	if _, err := invoke("init"); err != nil {
		t.Fatal(err)
	}
	defer invoke("shutdown")
	if _, err := invoke("init"); err == nil {
		t.Fatal("double init")
	}
	var wg sync.WaitGroup
	for range 20 {
		wg.Go(func() {
			for range 10 {
				if _, err := invoke("stats"); err != nil {
					t.Error(err)
				}
			}
		})
	}
	wg.Wait()
	for _, raw := range []string{`{`, strings.Repeat("x", maxInput+1), `{"method":"settings"}`} {
		var out map[string]json.RawMessage
		if json.Unmarshal(response([]byte(raw)), &out) != nil || out["error"] == nil {
			t.Fatal("invalid request accepted")
		}
	}
	if _, err := dispatch([]byte(`{"method":"stop","id":"absent","generation":1}`)); err != nil {
		t.Fatal(err)
	}
	invoke("shutdown")
	invoke("shutdown")
}
func TestMailboxRetainsCompletionAndLatestStatus(t *testing.T) {
	dispatch([]byte(`{"method":"init"}`))
	defer dispatch([]byte(`{"method":"shutdown"}`))
	id := resourceID{"server", 7}
	mailbox.Lock()
	mailbox.events[id] = map[string]any{}
	mailbox.Unlock()
	var wg sync.WaitGroup
	for n := range 100 {
		wg.Go(func() { event(id, "status", n) })
	}
	event(id, "join_end", map[string]any{"failed": false})
	wg.Wait()
	out, err := dispatch([]byte(`{"method":"events"}`))
	if err != nil || len(out.([]map[string]any)) != 2 {
		t.Fatal("completion lost")
	}
	dispatch([]byte(`{"method":"stop","id":"server","generation":6}`))
	mailbox.Lock()
	_, live := mailbox.events[id]
	mailbox.Unlock()
	if !live {
		t.Fatal("stale stop released newer generation")
	}
}
