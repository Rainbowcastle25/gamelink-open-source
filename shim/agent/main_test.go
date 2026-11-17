package main

import (
	"bytes"
	"encoding/json"
	"testing"
)

func TestOwnedLifecycleAndBounds(t *testing.T) {
	response([]byte(`{"method":"shutdown"}`))
	if _, err := dispatch([]byte(`{"method":"generate"}`)); err == nil {
		t.Fatal("operation before init")
	}
	if _, err := dispatch(bytes.Repeat([]byte("x"), maxInput+1)); err == nil {
		t.Fatal("unbounded request")
	}
	if _, err := dispatch([]byte(`{"method":"init"}`)); err != nil {
		t.Fatal(err)
	}
	if _, err := dispatch([]byte(`{"method":"init"}`)); err == nil {
		t.Fatal("double init")
	}
	var out map[string]json.RawMessage
	if err := json.Unmarshal(response([]byte(`{"method":"generate"}`)), &out); err != nil || out["error"] != nil {
		t.Fatal("key generation failed")
	}
	if _, err := dispatch([]byte(`{"method":"shutdown"}`)); err != nil {
		t.Fatal(err)
	}
	if _, err := dispatch([]byte(`{"method":"generate"}`)); err == nil {
		t.Fatal("operation after shutdown")
	}
	if _, err := dispatch([]byte(`{"method":"shutdown"}`)); err != nil {
		t.Fatal("shutdown not idempotent")
	}
}
