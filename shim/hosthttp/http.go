// Package hosthttp provides bounded HTTP I/O to the Rust host applications.
// Rust owns registration, credentials at rest, and all response interpretation.
package hosthttp

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/netip"
	"net/url"
	"time"
)

func Do(input []byte) (any, error) {
	var args struct {
		URL            string          `json:"url"`
		Verb           string          `json:"verb"`
		Token          string          `json:"token"`
		Body           json.RawMessage `json:"body"`
		TimeoutSeconds int             `json:"timeoutSeconds"`
		MaxBytes       int64           `json:"maxBytes"`
	}
	if json.Unmarshal(input, &args) != nil {
		return nil, errors.New("invalid HTTP transport request")
	}
	u, err := url.Parse(args.URL)
	if err != nil || u.Host == "" || u.User != nil || u.Fragment != "" || (u.Scheme != "http" && u.Scheme != "https") {
		return nil, errors.New("HTTP transport requires an HTTPS URL without credentials")
	}
	if u.Scheme == "http" {
		ip, err := netip.ParseAddr(u.Hostname())
		if err != nil || !ip.IsLoopback() {
			return nil, errors.New("unencrypted HTTP transport is limited to literal loopback addresses")
		}
	}
	if args.Verb != http.MethodGet && args.Verb != http.MethodPost {
		return nil, errors.New("unsupported HTTP transport method")
	}
	if args.TimeoutSeconds <= 0 || args.TimeoutSeconds > 95 {
		return nil, errors.New("invalid HTTP transport timeout")
	}
	if args.MaxBytes <= 0 || args.MaxBytes > 2<<20 {
		args.MaxBytes = 2 << 20
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Duration(args.TimeoutSeconds)*time.Second)
	defer cancel()
	var body io.Reader
	if len(args.Body) > 0 && string(args.Body) != "null" {
		body = bytes.NewReader(args.Body)
	}
	req, err := http.NewRequestWithContext(ctx, args.Verb, args.URL, body)
	if err != nil {
		return nil, errors.New("could not prepare HTTP transport request")
	}
	req.Header.Set("Authorization", "Bearer "+args.Token)
	req.Header.Set("Accept", "application/json")
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	transport := &http.Transport{Proxy: nil, DialContext: (&net.Dialer{Timeout: 5 * time.Second}).DialContext, TLSHandshakeTimeout: 5 * time.Second}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: time.Duration(args.TimeoutSeconds) * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	resp, err := client.Do(req)
	if err != nil {
		return nil, errors.New("HTTP transport request failed")
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, fmt.Errorf("HTTP transport returned status %d", resp.StatusCode)
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, args.MaxBytes+1))
	if err != nil || int64(len(data)) > args.MaxBytes {
		return nil, errors.New("HTTP transport response exceeded its limit or could not be read")
	}
	var value any
	if json.Unmarshal(data, &value) != nil {
		return nil, errors.New("HTTP transport response was not valid JSON")
	}
	return value, nil
}
