package integration

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
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

// serverFingerprint mirrors gamelink_core::requests::server_fingerprint.
func serverFingerprint(publicKey string) string {
	sum := sha256.Sum256([]byte("gamelink server fingerprint v1\n" + strings.ToLower(publicKey)))
	return hex.EncodeToString(sum[:])
}

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
	if selected["canCopyInvitation"] != true || selected["serverFingerprint"] == other["serverFingerprint"] {
		t.Fatal("host status did not identify the selected game")
	}
	if _, exposed := selected["serverPublicKey"]; exposed {
		t.Fatal("host status exposed a full server public key")
	}
	selection := map[string]any{"gameId": id, "serverFingerprint": selected["serverFingerprint"]}
	call("POST", "/v1/host/invitation", selection, 404)
	if _, err := request("POST", "/v2/host/invitation", selection, false, 401); err != nil {
		t.Fatal(err)
	}
	call("POST", "/v2/host/invitation", map[string]any{"gameId": otherID, "serverFingerprint": selected["serverFingerprint"]}, 400)
	result := call("POST", "/v2/host/invitation", selection, 200)
	copied, details, err := invite.Parse(result["invitation"].(string), invite.Options{})
	if err != nil || copied.Name != selectedName || serverFingerprint(details.ServerPublicKey) != selected["serverFingerprint"] {
		t.Fatal("copied invitation did not import as the selected game")
	}
	snapshot = call("GET", "/v1/host/status", nil, 200)
	if statusFor(snapshot, id)["approvedDevices"] != float64(0) || statusFor(snapshot, otherID)["approvedDevices"] != float64(0) {
		t.Fatal("copying approved a device")
	}
	rejects := func(path string, body map[string]any, want string) {
		t.Helper()
		if got := call("POST", path, body, 400)["error"]; got != want {
			t.Fatalf("%s returned %q, want %q", path, got, want)
		}
	}
	const changedGame = "the selected game identity has changed"
	const notPending = "request is no longer pending for that device and game"
	otherDevice, otherPublic := newKey()
	// Import approves only the game whose current identity was selected.
	imported := map[string]any{"gameId": otherID, "serverFingerprint": selected["serverFingerprint"], "publicKey": otherPublic}
	rejects("/v2/host/import", imported, changedGame)
	if statusFor(call("GET", "/v1/host/status", nil, 200), otherID)["approvedDevices"] != float64(0) {
		t.Fatal("a rejected import approved a device")
	}
	imported["serverFingerprint"] = other["serverFingerprint"]
	call("POST", "/v1/host/import", imported, 404)
	if statusFor(call("GET", "/v1/host/status", nil, 200), otherID)["approvedDevices"] != float64(0) {
		t.Fatal("the retired import route approved a device")
	}
	call("POST", "/v2/host/import", imported, 200)

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
	// The other game serves only its imported device; the positive control
	// below proves it is live when the friend is denied there.
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
	stranger, strangerPublic := newKey()
	probe, _ := newKey()
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
	pendingIDs := func(snapshot map[string]any, gameID string) map[any]any {
		ids := map[any]any{}
		for _, item := range statusFor(snapshot, gameID)["pending"].([]any) {
			pending := item.(map[string]any)
			ids[pending["publicKey"]] = pending["id"]
		}
		return ids
	}
	// knockUntil knocks with the devices in order until the last one is listed.
	knockUntil := func(devices ...key.NodePrivate) map[string]any {
		t.Helper()
		last := devices[len(devices)-1].Public().String()
		for deadline := time.Now().Add(agentWait); time.Now().Before(deadline); time.Sleep(50 * time.Millisecond) {
			for _, device := range devices {
				deny(copiedAddress.Addr(), device)
			}
			snapshot := call("GET", "/v1/host/status", nil, 200)
			if pendingIDs(snapshot, id)[last] != nil {
				return snapshot
			}
		}
		t.Fatal("a join request was not listed")
		return nil
	}
	snapshot = knockUntil(friend, stranger)
	requestID := pendingIDs(snapshot, id)[friendPublic]
	if requestID == nil || len(statusFor(snapshot, otherID)["pending"].([]any)) != 0 {
		t.Fatal("friend's join request was not scoped to the copied game")
	}

	// Dismiss requires the same selection as approval, then ignores the key.
	dismissal := map[string]any{"gameId": id, "serverFingerprint": other["serverFingerprint"], "requestId": pendingIDs(snapshot, id)[strangerPublic], "publicKey": strangerPublic}
	rejects("/v2/host/requests/dismiss", dismissal, changedGame)
	dismissal["serverFingerprint"], dismissal["publicKey"] = selected["serverFingerprint"], friendPublic
	rejects("/v2/host/requests/dismiss", dismissal, notPending)
	dismissal["publicKey"] = strangerPublic
	call("POST", "/v1/host/requests/dismiss", dismissal, 404)
	call("POST", "/v2/host/requests/dismiss", dismissal, 200)
	snapshot = knockUntil(stranger, probe)
	if pending := pendingIDs(snapshot, id); pending[strangerPublic] != nil || pending[friendPublic] != requestID {
		t.Fatal("a dismissed device was listed again before rotation")
	}

	approval := map[string]any{"gameId": otherID, "serverFingerprint": other["serverFingerprint"], "requestId": requestID, "publicKey": friendPublic, "label": "Friend"}
	rejects("/v2/host/requests/approve", approval, notPending)
	approval["gameId"] = id
	rejects("/v2/host/requests/approve", approval, changedGame)
	approval["serverFingerprint"], approval["publicKey"] = selected["serverFingerprint"], otherPublic
	rejects("/v2/host/requests/approve", approval, notPending)
	approval["publicKey"] = friendPublic
	call("POST", "/v1/host/requests/approve", approval, 404)
	call("POST", "/v2/host/requests/approve", approval, 200)
	var handled struct {
		Games map[string]struct {
			Pending   []struct{ PublicKey string }
			Dismissed map[string]time.Time `json:"dismissedUntilRotation"`
		}
	}
	handledBytes, err := os.ReadFile(filepath.Join(dir, "requests.json"))
	if err != nil || json.Unmarshal(handledBytes, &handled) != nil || handled.Games[id].Dismissed[strangerPublic].IsZero() || !handled.Games[id].Dismissed[friendPublic].IsZero() {
		t.Fatal("approval or dismissal did not update the request store")
	}
	for _, pending := range handled.Games[id].Pending {
		if pending.PublicKey == friendPublic {
			t.Fatal("approval did not retire the old request")
		}
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
	// Positive control: the other game is live and admits its own device, so
	// the denials below come from per-game approval, not a missing tunnel.
	var otherTunnel *transport.Tunnel
	for deadline := time.Now().Add(agentWait); time.Now().Before(deadline); {
		otherTunnel, err = join(tailcat.Addr(otherInvite.Addr), otherDevice)
		if err == nil {
			break
		}
	}
	if err != nil || otherTunnel == nil {
		t.Fatal("a device approved for the other game could not join it")
	}
	defer otherTunnel.Stop()
	if roundTripTCP(t, otherTunnel.Status().LocalAddr, "other game") != "other game" {
		t.Fatal("approved tunnel did not reach the other game")
	}
	deny(tailcat.Addr(otherInvite.Addr), friend)
	deny(copiedAddress.Addr(), otherDevice)
}
