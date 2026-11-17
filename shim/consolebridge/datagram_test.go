package consolebridge

import (
	"bytes"
	"testing"
	"time"
)

func TestFragmentsRoundTrip(t *testing.T) {
	for _, n := range []int{1, fragmentSize, fragmentSize + 1, MaxDatagram} {
		data := bytes.Repeat([]byte{0x87}, n)
		packets, err := Fragment(12, 34, data)
		if err != nil {
			t.Fatal(err)
		}
		var r Reassembler
		for i := len(packets) - 1; i >= 0; i-- {
			if len(packets[i]) > MaxPacket {
				t.Fatal("packet too big")
			}
			id, out, err := r.Add(packets[i], time.Now())
			if err != nil || id != 12 {
				t.Fatalf("%d: %v", n, err)
			}
			if i == 0 && !bytes.Equal(out, data) {
				t.Fatal("payload mismatch")
			}
		}
	}
}
func TestFragmentRejectsMalformedAndBounded(t *testing.T) {
	if _, err := Fragment(1, 1, make([]byte, MaxDatagram+1)); err == nil {
		t.Fatal("accepted oversize")
	}
	packets, _ := Fragment(1, 1, make([]byte, fragmentSize+1))
	var r Reassembler
	now := time.Now()
	if _, _, err := r.Add(packets[0], now); err != nil {
		t.Fatal(err)
	}
	if _, _, err := r.Add(packets[0], now); err == nil {
		t.Fatal("accepted duplicate")
	}
	for i := uint64(1); i <= maxAssemblies; i++ {
		p, _ := Fragment(2, i, make([]byte, fragmentSize+1))
		if _, _, err := r.Add(p[0], now); err != nil {
			t.Fatal(err)
		}
	}
	p, _ := Fragment(2, 1000, make([]byte, fragmentSize+1))
	if _, _, err := r.Add(p[0], now); err == nil {
		t.Fatal("accepted too many assemblies")
	}
	if _, _, err := r.Add(p[0], now.Add(assemblyLifetime)); err != nil {
		t.Fatal(err)
	}
	packets[1][5] = 1
	if _, _, err := r.Add(packets[1], now); err == nil {
		t.Fatal("accepted reserved bits")
	}
}
