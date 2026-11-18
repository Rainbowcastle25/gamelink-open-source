package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"net/netip"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/invite"
	"github.com/gamelink-project/gamelink/shim/transport"
	"github.com/tailscale/tailcat"
	"tailscale.com/tailcfg"
	"tailscale.com/types/key"
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
	selectedName := strings.Repeat("界", 64)
	otherName := "Other managed game"
	inventoryGame := func(uuid, name string, dest netip.AddrPort) map[string]any {
		allocation := map[string]any{"id": 1, "ip": dest.Addr().String(), "port": dest.Port()}
		return map[string]any{"attributes": map[string]any{
			"uuid": uuid, "identifier": uuid[:8], "name": name, "node": 7, "egg": 10,
			"allocation": 1, "suspended": false,
			"relationships": map[string]any{"allocations": map[string]any{"data": []any{map[string]any{"attributes": allocation}}}},
		}}
	}
	inventory := []any{
		inventoryGame("12345678-1234-4321-abcd-123456789abc", strings.Repeat("界", 191), tcpEcho(t)),
		inventoryGame("87654321-1234-4321-abcd-123456789abc", otherName, tcpEcho(t)),
	}
	panel := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasPrefix(r.URL.Path, "/api/application/") {
			inventoryHits.Add(1)
			json.NewEncoder(w).Encode(map[string]any{"data": inventory, "meta": map[string]any{"pagination": map[string]int{"current_page": 1, "total_pages": 1}}})
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
	id, otherID := "", ""
	for _, game := range cfg.Games {
		if game.Origin != "" {
			switch game.Name {
			case selectedName:
				id = game.ID
			case otherName:
				otherID = game.ID
			default:
				t.Fatal("Panel display name was not shortened to GameLink's character limit")
			}
		}
	}
	if id == "" || otherID == "" || inventoryHits.Load() != 1 || powerHits.Load() != 2 {
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
	request := func(method, path string, body any, authenticated bool, wantStatus int) (map[string]any, error) {
		data, _ := json.Marshal(body)
		req, _ := http.NewRequest(method, "http://"+address+path, bytes.NewReader(data))
		if authenticated {
			req.Header.Set("Authorization", "Bearer "+token)
		}
		resp, err := client.Do(req)
		if err != nil {
			return nil, err
		}
		defer resp.Body.Close()
		var value map[string]any
		err = json.NewDecoder(resp.Body).Decode(&value)
		if resp.StatusCode != wantStatus {
			t.Fatalf("host returned HTTP %d for %s, want %d", resp.StatusCode, path, wantStatus)
		}
		return value, err
	}
	call := func(method, path string, body any, wantStatus int) map[string]any {
		t.Helper()
		value, err := request(method, path, body, true, wantStatus)
		if err != nil {
			t.Fatalf("could not call %s: %v", path, err)
		}
		return value
	}
	deadline := time.Now().Add(5 * time.Second)
	for {
		if _, err := request("GET", "/v1/host/status", nil, true, 200); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("admin listener did not start")
		}
		time.Sleep(20 * time.Millisecond)
	}
	if inventoryHits.Load() != 1 || powerHits.Load() != 2 {
		t.Fatal("starting the agent or reading cached status initiated a scan")
	}
	offline.Store(true)
	snapshot := call("POST", "/v1/host/refresh", nil, 200)
	registered := snapshot["games"].([]any)
	if len(registered) != 2 || registered[0].(map[string]any)["state"] != "offline" || registered[1].(map[string]any)["state"] != "offline" {
		t.Fatal("manual refresh did not update sampled power state")
	}
	if inventoryHits.Load() != 2 || powerHits.Load() != 4 {
		t.Fatal("unexpected Panel request count")
	}
	after, err := os.ReadFile(filepath.Join(dir, "keys", id+".json"))
	if err != nil || string(keys) != string(after) {
		t.Fatal("refresh changed a stable game identity")
	}

	statusFor := func(snapshot map[string]any, gameID string) map[string]any {
		t.Helper()
		for _, entry := range snapshot["games"].([]any) {
			status := entry.(map[string]any)
			if status["id"] == gameID {
				return status
			}
		}
		t.Fatal("managed game missing from host status")
		return nil
	}
	selected := statusFor(snapshot, id)
	other := statusFor(snapshot, otherID)
	if selected["canCopyInvitation"] != true || selected["serverPublicKey"] == other["serverPublicKey"] {
		t.Fatal("host status did not identify the selected game")
	}
	selection := map[string]any{"gameId": id, "serverPublicKey": selected["serverPublicKey"]}
	if _, err := request("POST", "/v1/host/invitation", selection, false, 401); err != nil {
		t.Fatal(err)
	}
	call("POST", "/v1/host/invitation", map[string]any{"gameId": otherID, "serverPublicKey": selected["serverPublicKey"]}, 400)
	result := call("POST", "/v1/host/invitation", selection, 200)
	copied, details, err := invite.Parse(result["invitation"].(string), invite.Options{})
	if err != nil || copied.Name != selectedName || details.ServerPublicKey != selected["serverPublicKey"] {
		t.Fatal("copied invitation did not import as the selected game")
	}
	snapshot = call("GET", "/v1/host/status", nil, 200)
	if statusFor(snapshot, id)["approvedDevices"] != float64(0) || statusFor(snapshot, otherID)["approvedDevices"] != float64(0) {
		t.Fatal("copying approved a device")
	}

	// The copy endpoint enforces production relay policy. Switch only the
	// relay for the wire test, keeping the keys from the copied invitation.
	relay := startRelay(t)
	setLocalRelay := func(gameID string) {
		t.Helper()
		path := filepath.Join(dir, "keys", gameID+".json")
		data, err := os.ReadFile(path)
		var stored map[string]any
		if err != nil || json.Unmarshal(data, &stored) != nil {
			t.Fatal("could not read fixture game keys")
		}
		stored["region"] = relay
		data, _ = json.Marshal(stored)
		if err := writeFileAtomic(path, data); err != nil {
			t.Fatal(err)
		}
	}
	setLocalRelay(id)
	setLocalRelay(otherID)
	// Enable pending requests on the other game too, so the denial below uses
	// a live game tunnel with an empty allowlist.
	copiedAddress, err := tailcat.ParseAddr(tailcat.Addr(copied.Addr))
	if err != nil {
		t.Fatal("invalid copied address")
	}
	copiedAddress.Region = []*tailcfg.DERPRegion{relay}
	otherInvite, _, err := invite.Parse(strings.TrimSpace(string(cli("invite", "--game", otherID, "--name", "", "--allow-local-relay"))), invite.Options{AllowLocalRelay: true})
	if err != nil || otherInvite.Name != otherName {
		t.Fatal("invalid other game invitation")
	}
	friend, friendPublic := newKey()
	otherDevice, otherPublic := newKey()
	join := func(addr tailcat.Addr, device key.NodePrivate) (*transport.Tunnel, error) {
		opts := joinOpts(addr, games.TCP, 25565, device)
		opts.ConnectTimeout = 3 * time.Second
		return transport.Join(context.Background(), opts)
	}
	deny := func(addr tailcat.Addr, device key.NodePrivate) {
		t.Helper()
		tunnel, err := join(addr, device)
		if tunnel != nil {
			tunnel.Stop()
		}
		if !errors.Is(err, transport.ErrNoResponse) {
			t.Fatal("a device without approval was not refused")
		}
	}
	deny(copiedAddress.Addr(), friend)
	var requestID any
	for deadline := time.Now().Add(agentWait); time.Now().Before(deadline); time.Sleep(50 * time.Millisecond) {
		snapshot = call("GET", "/v1/host/status", nil, 200)
		for _, item := range statusFor(snapshot, id)["pending"].([]any) {
			pending := item.(map[string]any)
			if pending["publicKey"] == friendPublic {
				requestID = pending["id"]
			}
		}
		if requestID != nil {
			break
		}
		deny(copiedAddress.Addr(), friend)
	}
	if requestID == nil || len(statusFor(snapshot, otherID)["pending"].([]any)) != 0 {
		t.Fatal("friend's join request was not scoped to the copied game")
	}
	approval := map[string]any{"gameId": otherID, "serverPublicKey": other["serverPublicKey"], "requestId": requestID, "publicKey": friendPublic, "label": "Friend"}
	call("POST", "/v1/host/requests/approve", approval, 400)
	approval["gameId"], approval["serverPublicKey"], approval["publicKey"] = id, selected["serverPublicKey"], otherPublic
	call("POST", "/v1/host/requests/approve", approval, 400)
	approval["publicKey"] = friendPublic
	call("POST", "/v1/host/requests/approve", approval, 200)
	var handled struct {
		Games map[string]struct{ Dismissed map[string]time.Time }
	}
	handledBytes, err := os.ReadFile(filepath.Join(dir, "requests.json"))
	if err != nil || json.Unmarshal(handledBytes, &handled) != nil || handled.Games[id].Dismissed[friendPublic].IsZero() {
		t.Fatal("approval did not retire the old request")
	}

	var tunnel *transport.Tunnel
	for deadline := time.Now().Add(agentWait); time.Now().Before(deadline); {
		tunnel, err = join(copiedAddress.Addr(), friend)
		if err == nil {
			break
		}
	}
	if err != nil || tunnel == nil {
		t.Fatal("approved device could not join the selected game")
	}
	defer tunnel.Stop()
	if roundTripTCP(t, tunnel.Status().LocalAddr, "selected game") != "selected game" {
		t.Fatal("approved tunnel did not reach the selected destination")
	}
	deny(tailcat.Addr(otherInvite.Addr), friend)
	deny(copiedAddress.Addr(), otherDevice)
}
