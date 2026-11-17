package mobile

import (
	"context"
	"strings"
	"testing"
)

func TestIdentity(t *testing.T) {
	b, err := GenerateIdentity()
	if err != nil {
		t.Fatal(err)
	}
	pub, err := PublicKey(b)
	if err != nil || len(pub) == 0 {
		t.Fatal(pub, err)
	}
	if _, err = PublicKey([]byte("invalid")); err == nil {
		t.Fatal("accepted invalid key")
	}
}

func TestStopIsIdempotentAndStatusIsSanitized(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	r := &Runtime{cancel: cancel, status: runtimeStatus{State: "connecting", Message: "Connecting"}}
	r.Stop()
	r.Stop()
	if ctx.Err() == nil {
		t.Fatal("connection context not canceled")
	}
	s := r.Status()
	if !strings.Contains(s, `"state":"stopped"`) {
		t.Fatal(s)
	}
}
