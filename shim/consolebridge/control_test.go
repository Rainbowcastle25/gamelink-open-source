package consolebridge

import (
	"bytes"
	"encoding/binary"
	"testing"
)

func TestControlFraming(t *testing.T) {
	var b bytes.Buffer
	want := ControlMessage{Version: 1, Type: "offer", Session: 1, Data: "opaque"}
	if err := WriteControl(&b, want); err != nil {
		t.Fatal(err)
	}
	got, err := ReadControl(&b)
	if err != nil || got != want {
		t.Fatal(got, err)
	}
	var h [4]byte
	binary.BigEndian.PutUint32(h[:], MaxControlFrame+1)
	if _, err := ReadControl(bytes.NewReader(h[:])); err == nil {
		t.Fatal("accepted oversized frame")
	}
	if err := WriteControl(&b, ControlMessage{Version: 1, Type: "offer"}); err == nil {
		t.Fatal("accepted missing session")
	}
}
