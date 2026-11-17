package consolebridge

import (
	"bytes"
	"encoding/binary"
	"io"
	"net/netip"
	"strings"
	"testing"
)

func TestRustPolicyPreservesWireBytes(t *testing.T) {
	raw := "CONNECTREQUEST 18446744073709551615 opaque\x00\xff"
	signal, err := ParseSignal(raw)
	if err != nil || FormatSignal(signal) != raw {
		t.Fatalf("signal byte parity: %v", err)
	}
	candidate := "a=candidate:1\t1 UDP 1 192.0.2.4\t4000 typ host\r\n"
	got, targets, err := MapCandidates("opaque\xff\r\n"+candidate, netip.MustParseAddr("192.0.2.4"), 4000, 4010, netip.MustParseAddrPort("192.0.2.8:5000"))
	if err != nil || !strings.HasPrefix(got, "opaque\xff\r\n") || len(targets) != 1 || !strings.Contains(got, "192.0.2.8\t5000") {
		t.Fatalf("candidate byte parity: %v", err)
	}
	var wire bytes.Buffer
	if err := WriteControl(&wire, ControlMessage{Version: 1, Type: "offer", Session: ^uint64(0), Data: "<>&\u2028\u2029"}); err != nil {
		t.Fatal(err)
	}
	want := `{"version":1,"type":"offer","session":18446744073709551615,"data":"\u003c\u003e\u0026\u2028\u2029"}`
	if string(wire.Bytes()[4:]) != want {
		t.Fatalf("control JSON wire encoding differs")
	}
}

// A hostile length must fail before ReadControl attempts to consume a body.
type headerOnlyReader struct {
	header   *bytes.Reader
	bodyRead bool
}

func (r *headerOnlyReader) Read(b []byte) (int, error) {
	if r.header.Len() == 0 {
		r.bodyRead = true
		return 0, io.ErrUnexpectedEOF
	}
	return r.header.Read(b)
}
func TestRustControlLimitsAndMalformedFields(t *testing.T) {
	for _, n := range []uint32{0, MaxControlFrame + 1, ^uint32(0)} {
		r := &headerOnlyReader{header: bytes.NewReader(binary.BigEndian.AppendUint32(nil, n))}
		if _, err := ReadControl(r); err == nil || r.bodyRead {
			t.Fatalf("length %d did not fail before body read", n)
		}
	}
	for _, raw := range []string{`{"version":1,"type":"hello","extra":1}`, `{"version":1,"type":"hello"} {}`, `{"version":1,"type":"offer","session":0}`} {
		wire := append(binary.BigEndian.AppendUint32(nil, uint32(len(raw))), raw...)
		if _, err := ReadControl(bytes.NewReader(wire)); err == nil {
			t.Fatal("accepted invalid control frame")
		}
	}
	var out bytes.Buffer
	if err := WriteControl(&out, ControlMessage{Version: 1, Type: "offer", Session: 1, Data: strings.Repeat("\x00", 30000)}); err == nil || out.Len() != 0 {
		t.Fatal("oversized escaped frame wrote data")
	}
	raw := append([]byte(`{"version":1,"type":"hello","data":"`), 0xe2, 0x82)
	raw = append(raw, []byte(`"}`)...)
	wire := append(binary.BigEndian.AppendUint32(nil, uint32(len(raw))), raw...)
	got, err := ReadControl(bytes.NewReader(wire))
	if err != nil || got.Data != "\ufffd\ufffd" {
		t.Fatalf("invalid UTF-8 decode parity: %#v %v", got, err)
	}
}

func TestRustCandidateFilteringPreservesZonedIPv6Compatibility(t *testing.T) {
	ip := netip.MustParseAddr("192.0.2.4")
	relay := netip.MustParseAddrPort("192.0.2.8:5000")
	input := "v=0\r\na=candidate:1 1 UDP 1 fe80::1%eth0 4000 typ host\r\na=candidate:2 1 UDP 1 ::ffff:192.0.2.4%eth0 4000 typ host\r\n"
	got, targets, err := MapCandidates(input, ip, 4000, 4010, relay)
	if err != nil || len(targets) != 1 || strings.Contains(got, "fe80") || !strings.Contains(got, "192.0.2.8 5000") {
		t.Fatalf("zoned IPv6 compatibility: %v", err)
	}
}

func TestRustRejectsUnicodeCandidateShapedLines(t *testing.T) {
	for _, input := range []string{" A=CANDİDATE:1 1 UDP 1 192.0.2.4 4000 typ host", "a=REMOTE-CANDİDATES:1 192.0.2.4 4000"} {
		if _, _, err := MapCandidates(input, netip.MustParseAddr("192.0.2.4"), 4000, 4010, netip.MustParseAddrPort("192.0.2.8:5000")); err == nil {
			t.Fatal("accepted unparsed Unicode candidate-shaped line")
		}
	}
}
