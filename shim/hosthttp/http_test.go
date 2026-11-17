package hosthttp

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestHTTPTransportBoundsAndRedirects(t *testing.T) {
	hits := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		hits++
		if r.URL.Path == "/redirect" {
			http.Redirect(w, r, "/secret", 302)
			return
		}
		if r.URL.Path == "/error" {
			w.WriteHeader(403)
			w.Write([]byte("fixture-secret"))
			return
		}
		if r.URL.Path == "/large" {
			w.Write([]byte(strings.Repeat(" ", 100)))
			return
		}
		if r.Header.Get("Authorization") != "Bearer fixture-secret" {
			t.Error("missing credential")
		}
		w.Write([]byte(`{"state":"running"}`))
	}))
	defer server.Close()
	request := func(path string, limit int) (any, error) {
		data, _ := json.Marshal(map[string]any{"url": server.URL + path, "verb": "GET", "token": "fixture-secret", "timeoutSeconds": 1, "maxBytes": limit})
		return Do(data)
	}
	if _, err := request("/", 1024); err != nil {
		t.Fatal(err)
	}
	for _, path := range []string{"/redirect", "/error", "/large"} {
		if _, err := request(path, 32); err == nil || strings.Contains(err.Error(), "fixture-secret") {
			t.Fatalf("unsafe result %s: %v", path, err)
		}
	}
	if hits != 4 {
		t.Fatalf("redirect followed: %d requests", hits)
	}
	if _, err := Do([]byte(`{"url":"http://example.com","verb":"GET","timeoutSeconds":1}`)); err == nil {
		t.Fatal("non-loopback HTTP accepted")
	}
}
