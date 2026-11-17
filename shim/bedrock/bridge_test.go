package bedrock

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/netip"
	"strings"
	"testing"
	"time"
)

func TestHostPreservesResponsesAndPinsMedia(t *testing.T) {
	for _, tc := range []struct {
		name   string
		status int
		body   string
		want   int
	}{
		{"metadata", 200, `{"name":"BDS","protocol":2259}`, 200},
		{"identity rejection", 403, "37", 403},
		{"numeric NetherNet error", 200, "37", 200},
		{"unconfigured media port", 200, "v=0\r\na=candidate:a 1 UDP 1 127.0.0.1 1 typ host\r\n", 502},
		{"foreign media IP", 200, "v=0\r\na=candidate:a 1 UDP 1 192.0.2.20 19132 typ host\r\n", 502},
	} {
		t.Run(tc.name, func(t *testing.T) {
			up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.Header.Get("Accept-Encoding") != "identity" {
					t.Error("unexpected compression")
				}
				w.Header().Set("Content-Type", "application/sdp")
				w.WriteHeader(tc.status)
				_, _ = io.WriteString(w, tc.body)
			}))
			defer up.Close()
			dest := up.Listener.Addr().(*net.TCPAddr).AddrPort()
			host, err := NewHost(dest, 19132, 19136)
			if err != nil {
				t.Fatal(err)
			}
			defer host.Close()
			client, server := net.Pipe()
			defer client.Close()
			go host.Handle(server)
			_ = client.SetDeadline(time.Now().Add(5 * time.Second))
			req := request{Method: http.MethodPost, URI: "/v1/join/opaque%2Fid?x=1", Body: []byte("v=0\r\na=candidate:a 1 UDP 1 127.0.0.1 9 typ host\r\n")}
			if tc.name == "metadata" {
				req.Method = http.MethodGet
				req.URI = "/v1/join"
				req.Body = nil
			}
			if err := writeMessage(client, req); err != nil {
				t.Fatal(err)
			}
			var resp response
			if err := readMessage(client, &resp); err != nil {
				t.Fatal(err)
			}
			if resp.Status != tc.want {
				t.Fatalf("status %d, want %d", resp.Status, tc.want)
			}
			if tc.want != 502 && string(resp.Body) != tc.body {
				t.Fatal("upstream response changed")
			}
		})
	}
}

func TestClientPreservesSignedSDPAndRelaysDatagrams(t *testing.T) {
	media, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer media.Close()
	mediaAddr := media.LocalAddr().(*net.UDPAddr).AddrPort()
	signed := "v=0\r\na=identity:opaque-account-assertion\r\na=fingerprint:sha-256 DO:NOT:CHANGE\r\na=ice-ufrag:original\r\na=ice-pwd:original-secret\r\n"
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.RequestURI != "/v1/join/opaque%2Fnetwork?key=1" {
			t.Error("opaque network ID or query changed")
		}
		body, _ := io.ReadAll(r.Body)
		if !strings.HasPrefix(string(body), signed) {
			t.Error("offer identity or credentials changed")
		}
		if bytes.Contains(body, []byte("192.0.2.99")) {
			t.Error("foreign candidate forwarded")
		}
		w.Header().Set("Content-Type", "application/sdp")
		_, _ = fmt.Fprintf(w, "%sa=candidate:b 1 UDP 1 %s %d typ host\r\n", signed, mediaAddr.Addr(), mediaAddr.Port())
	}))
	defer up.Close()
	host, err := NewHost(up.Listener.Addr().(*net.TCPAddr).AddrPort(), mediaAddr.Port(), mediaAddr.Port())
	if err != nil {
		t.Fatal(err)
	}
	defer host.Close()
	client := NewClient(context.Background(), func(context.Context) (net.Conn, error) { a, b := net.Pipe(); go host.Handle(b); return a, nil })
	defer client.Close()
	local := httptest.NewServer(client)
	defer local.Close()
	player, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer player.Close()
	playerAddr := player.LocalAddr().(*net.UDPAddr).AddrPort()
	offer := fmt.Sprintf("%sa=candidate:a 1 UDP 1 %s %d typ host\r\na=candidate:bad 1 UDP 1 192.0.2.99 1234 typ host\r\n", signed, playerAddr.Addr(), playerAddr.Port())
	resp, err := http.Post(local.URL+"/v1/join/opaque%2Fnetwork?key=1", "application/sdp", strings.NewReader(offer))
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	answer, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != 200 {
		t.Fatalf("join returned %d: %s", resp.StatusCode, answer)
	}
	if !strings.HasPrefix(string(answer), signed) {
		t.Fatal("answer identity or credentials changed")
	}
	_, targets, err := mapSDP(answer, []string{"127.0.0.1"}, 1, 65535, placeholder)
	if err != nil || len(targets) != 1 || targets[0] == mediaAddr {
		t.Fatal("answer does not point at local relay")
	}
	for _, size := range []int{1, 1232, 4096, 16384} {
		packet := bytes.Repeat([]byte{byte(size % 251)}, size)
		if _, err := player.WriteToUDPAddrPort(packet, targets[0]); err != nil {
			t.Fatal(err)
		}
		_ = media.SetReadDeadline(time.Now().Add(3 * time.Second))
		buffer := make([]byte, 65535)
		n, source, err := media.ReadFromUDPAddrPort(buffer)
		if err != nil || !bytes.Equal(buffer[:n], packet) {
			t.Fatalf("upstream %d-byte datagram failed: %v", size, err)
		}
		_, _ = media.WriteToUDPAddrPort(buffer[:n], source)
		_ = player.SetReadDeadline(time.Now().Add(3 * time.Second))
		n, _, err = player.ReadFromUDPAddrPort(buffer)
		if err != nil || !bytes.Equal(buffer[:n], packet) {
			t.Fatalf("downstream %d-byte datagram failed: %v", size, err)
		}
	}
	client.Close() // active media must not keep shutdown waiting
}

func TestRequestAndFrameLimits(t *testing.T) {
	for _, req := range []request{
		{Method: "GET", URI: "http://192.0.2.1/v1/join"},
		{Method: "POST", URI: "/admin"},
		{Method: "DELETE", URI: "/v1/join"},
		{Method: "GET", URI: "/v1/join", Body: []byte("unexpected")},
	} {
		if validRequest(req) {
			t.Fatal("accepted unsupported signaling request")
		}
	}
	if _, err := readFrame(bytes.NewReader([]byte{0, 2, 0, 1}), maxFrame); err == nil {
		t.Fatal("accepted oversized frame")
	}
	if _, _, err := mapSDP([]byte("a=candidate:a 1 UDP 1 192.0.2.99 9000 typ host\r\n"), []string{"127.0.0.1"}, 1, 65535, netip.MustParseAddrPort("127.0.0.1:1234")); err == nil {
		t.Fatal("accepted foreign offer candidate")
	}
}
