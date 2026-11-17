package mobile

import (
	"context"
	"encoding/json"
	"net"
	"net/http"
	"runtime"
	"sync"
	"time"
)

// SandboxDiagnostics probes only a fixed public Mojang documentation endpoint
// from the Go runtime. It returns booleans and an interface count only; it
// never returns addresses, response content, or raw network errors.
func SandboxDiagnostics() string {
	const host = "mojang.github.io"
	const endpoint = "https://mojang.github.io/bedrock-protocol-docs/guides/nether-net-onboarding-guide/"

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	var dnsOK, tlsOK bool
	var probes sync.WaitGroup
	probes.Add(2)
	go func() {
		defer probes.Done()
		_, err := net.DefaultResolver.LookupHost(ctx, host)
		dnsOK = err == nil
	}()
	go func() {
		defer probes.Done()
		request, err := http.NewRequestWithContext(ctx, http.MethodGet, endpoint, nil)
		if err == nil {
			client := &http.Client{
				Timeout: 8 * time.Second,
				CheckRedirect: func(_ *http.Request, _ []*http.Request) error {
					return http.ErrUseLastResponse
				},
			}
			if response, requestErr := client.Do(request); requestErr == nil {
				// A valid response proves certificate validation succeeded using the
				// Go runtime's standard platform root handling. Read only a small
				// amount and do not retain or return document data.
				_, _ = response.Body.Read(make([]byte, 1))
				tlsOK = response.TLS != nil && len(response.TLS.PeerCertificates) > 0
				_ = response.Body.Close()
			}
		}
	}()
	probes.Wait()

	interfaces, _ := net.Interfaces()
	result := struct {
		DNSOK      bool   `json:"dnsOK"`
		TLSOK      bool   `json:"tlsOK"`
		Interfaces int    `json:"interfaces"`
		GoVersion  string `json:"goVersion"`
	}{dnsOK, tlsOK, len(interfaces), runtime.Version()}
	encoded, err := json.Marshal(result)
	if err != nil {
		return `{"dnsOK":false,"tlsOK":false,"interfaces":0,"goVersion":"unknown"}`
	}
	return string(encoded)
}
