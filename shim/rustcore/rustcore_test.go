//go:build cgo

package rustcore

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"strings"
	"sync"
	"testing"
)

const startInput = `{"state":{"phase":"idle","generation":0,"message":null,"name":null,"serverId":null},` +
	`"event":{"type":"start","server":{"id":"world-1","name":"World","game":"minecraft-bedrock","bridgeVersion":1},"localIpv4":"192.168.1.25"}}`

func wantCode(t *testing.T, err error, code string) *Error {
	t.Helper()
	var e *Error
	if !errors.As(err, &e) {
		t.Fatalf("want *Error with code %q, got %v", code, err)
	}
	if e.Code != code {
		t.Fatalf("want code %q, got %q (%s)", code, e.Code, e.Message)
	}
	return e
}

func TestVersion(t *testing.T) {
	out, err := Call("version", nil)
	if err != nil {
		t.Fatal(err)
	}
	var v struct {
		ABI   int    `json:"abi"`
		Crate string `json:"crate"`
	}
	if err := json.Unmarshal(out, &v); err != nil {
		t.Fatal(err)
	}
	if v.ABI != 1 || v.Crate == "" {
		t.Fatalf("unexpected version %s", out)
	}
}

func TestLifecycleReduceRoundTrip(t *testing.T) {
	out, err := Call("lifecycle.reduce", []byte(startInput))
	if err != nil {
		t.Fatal(err)
	}
	var r struct {
		State struct {
			Phase      string `json:"phase"`
			Generation uint64 `json:"generation"`
			ServerID   string `json:"serverId"`
		} `json:"state"`
		Effects []struct {
			Type       string `json:"type"`
			Generation uint64 `json:"generation"`
			ServerID   string `json:"server_id"`
			LocalIPv4  string `json:"local_ipv4"`
		} `json:"effects"`
	}
	if err := json.Unmarshal(out, &r); err != nil {
		t.Fatal(err)
	}
	if r.State.Phase != "connecting" || r.State.Generation != 1 || r.State.ServerID != "world-1" {
		t.Fatalf("unexpected state %s", out)
	}
	if len(r.Effects) != 1 || r.Effects[0].Type != "start_transport" || r.Effects[0].Generation != 1 ||
		r.Effects[0].ServerID != "world-1" || r.Effects[0].LocalIPv4 != "192.168.1.25" {
		t.Fatalf("unexpected effects %s", out)
	}
}

func TestInvalidReducerInput(t *testing.T) {
	_, err := Call("lifecycle.reduce", []byte("{"))
	wantCode(t, err, "invalid_json")
}

func TestOversizedInput(t *testing.T) {
	big := bytes.Repeat([]byte(" "), MaxInputBytes+1)
	_, err := Call("lifecycle.reduce", big)
	wantCode(t, err, "input_too_large")
	// Rust enforces the same bound if the Go precheck is bypassed.
	_, err = callRust("lifecycle.reduce", big)
	wantCode(t, err, "input_too_large")
	// Exactly at the limit reaches the parser.
	_, err = Call("lifecycle.reduce", big[:MaxInputBytes])
	wantCode(t, err, "invalid_json")
}

func TestMethodTooLong(t *testing.T) {
	long := strings.Repeat("a", MaxMethodBytes+1)
	_, err := Call(long, nil)
	wantCode(t, err, "method_too_long")
	_, err = callRust(long, nil)
	wantCode(t, err, "method_too_long")
}

func TestUnknownMethod(t *testing.T) {
	_, err := Call("no.such.method", []byte("{}"))
	wantCode(t, err, "unknown_method")
	_, err = Call("", nil)
	wantCode(t, err, "unknown_method")
}

func TestInvalidUTF8(t *testing.T) {
	_, err := Call("lifecycle.reduce", []byte("{\"a\":\"\xff\"}"))
	wantCode(t, err, "invalid_utf8")
}

func TestErrorsDoNotEchoInput(t *testing.T) {
	const secret = "tskey-SECRET-must-not-echo"
	cases := []struct {
		method string
		input  []byte
	}{
		{"lifecycle.reduce", []byte(`{"` + secret + `":`)},
		{"lifecycle.reduce", []byte(`{"state":"` + secret + `"}`)},
		{"lifecycle.reduce", []byte(secret + "\xff")},
		{secret, []byte(secret)},
		{"lifecycle.reduce", []byte(strings.Repeat(secret, 2000))},
	}
	for i, tc := range cases {
		_, err := callRust(tc.method, tc.input)
		var e *Error
		if !errors.As(err, &e) {
			t.Fatalf("case %d: want *Error, got %v", i, err)
		}
		if strings.Contains(err.Error(), "SECRET") || strings.Contains(e.Message, "SECRET") {
			t.Fatalf("case %d: error echoes input: %v", i, err)
		}
	}
}

func TestConcurrentCalls(t *testing.T) {
	const workers = 64
	var wg sync.WaitGroup
	errs := make(chan error, workers)
	for i := range workers {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := range 50 {
				method, input := "lifecycle.reduce", []byte(startInput)
				if (i+j)%3 == 0 {
					method, input = "version", nil
				}
				out, err := Call(method, input)
				if err != nil {
					errs <- fmt.Errorf("worker %d call %d: %w", i, j, err)
					return
				}
				if !json.Valid(out) {
					errs <- fmt.Errorf("worker %d call %d: invalid JSON %q", i, j, out)
					return
				}
			}
			if _, err := Call("nope", nil); err == nil {
				errs <- fmt.Errorf("worker %d: unknown method accepted", i)
			}
		}()
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		t.Error(err)
	}
}

func TestDecodeEnvelopeRejectsMalformed(t *testing.T) {
	for _, raw := range []string{``, `{}`, `[]`, `{"ok":1,"error":{"code":"x","message":"y"}}`, `not json`} {
		_, err := decodeEnvelope([]byte(raw))
		wantCode(t, err, "internal")
	}
}

// TestPanicIsContained calls the test-only `test.panic` method, which exists
// only when the archive was built with GAMELINK_FFI_FEATURES=panic-probe. It
// proves a Rust panic is caught at the C ABI on this platform's unwinder (the
// Windows CI job sets GAMELINK_REQUIRE_PANIC_PROBE=1 so it cannot skip).
func TestPanicIsContained(t *testing.T) {
	_, err := Call("test.panic", nil)
	var e *Error
	if errors.As(err, &e) && e.Code == "unknown_method" {
		if os.Getenv("GAMELINK_REQUIRE_PANIC_PROBE") == "1" {
			t.Fatal("archive was built without the panic-probe feature")
		}
		t.Skip("archive built without the panic-probe feature")
	}
	wantCode(t, err, "internal")
	if _, err := Call("version", nil); err != nil {
		t.Fatalf("call after contained panic failed: %v", err)
	}
}
