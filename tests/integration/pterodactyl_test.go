package integration

import (
	"context"
	"encoding/json"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

// Exercises the shipped Rust executable and its real HTTP shim. No Panel,
// relay, or administrative credentials from the user's machine are used.
func TestRustAgentPterodactylManualRefresh(t *testing.T) {
	binary := agentBinary(t)
	root := t.TempDir()
	dir := filepath.Join(root, "agent")
	cli := func(args ...string) []byte {
		t.Helper()
		ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
		defer cancel()
		out, err := exec.CommandContext(ctx, binary, append([]string{"--dir", dir}, args...)...).Output()
		if err != nil {
			t.Fatalf("Rust agent command %q failed (output withheld): %v", args[0], err)
		}
		return out
	}
	var inventoryHits, powerHits atomic.Int32
	var offline atomic.Bool
	panel := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasPrefix(r.URL.Path, "/api/application/") {
			inventoryHits.Add(1)
			json.NewEncoder(w).Encode(map[string]any{"data": []any{map[string]any{"attributes": map[string]any{"uuid": "12345678-1234-4321-abcd-123456789abc", "identifier": "12345678", "name": strings.Repeat("界", 191), "node": 7, "egg": 10, "allocation": 1, "suspended": false, "relationships": map[string]any{"allocations": map[string]any{"data": []any{map[string]any{"attributes": map[string]any{"id": 1, "ip": "127.0.0.1", "port": 25565}}}}}}}}, "meta": map[string]any{"pagination": map[string]int{"current_page": 1, "total_pages": 1}}})
		} else {
			powerHits.Add(1)
			state := "running"
			if offline.Load() {
				state = "offline"
			}
			json.NewEncoder(w).Encode(map[string]any{"attributes": map[string]any{"current_state": state}})
		}
	}))
	defer panel.Close()
	write := func(name string, data []byte) {
		t.Helper()
		if err := os.WriteFile(filepath.Join(root, name), data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	token := "test-only-admin-" + strings.Repeat("x", 40)
	write("application.token", []byte("test-application-token"))
	write("client.token", []byte("test-client-token"))
	write("admin.token", []byte(token))
	settings, _ := json.Marshal(map[string]any{"panelUrl": panel.URL, "applicationTokenFile": "application.token", "clientTokenFile": "client.token", "adminTokenFile": "admin.token", "nodeId": 7, "nodeAddress": "127.0.0.1", "eggProfiles": map[string]any{"10": map[string]any{"type": "minecraft-java", "protocol": "tcp", "tunnelPort": 25565}}})
	write("host.json", settings)
	cli("init")
	cli("games", "add", "--id", "seed", "--type", "minecraft-java", "--relay-host", "derp.example.com")
	cli("host", "refresh", "--config", filepath.Join(root, "host.json"))
	var cfg struct {
		Games []struct{ ID, Origin, Name string }
	}
	configBytes, err := os.ReadFile(filepath.Join(dir, "agent.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(configBytes, &cfg); err != nil {
		t.Fatal(err)
	}
	id := ""
	for _, game := range cfg.Games {
		if game.Origin != "" {
			id = game.ID
			if game.Name != strings.Repeat("界", 64) {
				t.Fatal("Panel display name was not shortened to GameLink's character limit")
			}
		}
	}
	if id == "" || inventoryHits.Load() != 1 || powerHits.Load() != 1 {
		t.Fatal("explicit refresh did not register and sample the fixture")
	}
	keys, err := os.ReadFile(filepath.Join(dir, "keys", id+".json"))
	if err != nil {
		t.Fatal(err)
	}
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	address := listener.Addr().String()
	listener.Close()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	cmd := exec.CommandContext(ctx, binary, "--dir", dir, "serve", "--host-config", filepath.Join(root, "host.json"), "--host-listen", address)
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	defer func() { cancel(); cmd.Wait() }()
	client := &http.Client{Timeout: 2 * time.Second}
	request := func(method, path string) (map[string]any, error) {
		req, _ := http.NewRequest(method, "http://"+address+path, strings.NewReader("{}"))
		req.Header.Set("Authorization", "Bearer "+token)
		resp, err := client.Do(req)
		if err != nil {
			return nil, err
		}
		defer resp.Body.Close()
		var value map[string]any
		err = json.NewDecoder(resp.Body).Decode(&value)
		if resp.StatusCode != 200 {
			t.Fatalf("host returned HTTP %d", resp.StatusCode)
		}
		return value, err
	}
	deadline := time.Now().Add(5 * time.Second)
	for {
		if _, err := request("GET", "/v1/host/status"); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("admin listener did not start")
		}
		time.Sleep(20 * time.Millisecond)
	}
	if inventoryHits.Load() != 1 || powerHits.Load() != 1 {
		t.Fatal("starting the agent or reading cached status initiated a scan")
	}
	offline.Store(true)
	snapshot, err := request("POST", "/v1/host/refresh")
	if err != nil {
		t.Fatal(err)
	}
	games := snapshot["games"].([]any)
	if len(games) != 1 || games[0].(map[string]any)["state"] != "offline" {
		t.Fatal("manual refresh did not update sampled power state")
	}
	if inventoryHits.Load() != 2 || powerHits.Load() != 2 {
		t.Fatal("unexpected Panel request count")
	}
	after, err := os.ReadFile(filepath.Join(dir, "keys", id+".json"))
	if err != nil || string(keys) != string(after) {
		t.Fatal("refresh changed a stable game identity")
	}
}
