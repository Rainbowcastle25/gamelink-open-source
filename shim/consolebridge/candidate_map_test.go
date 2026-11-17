package consolebridge

import (
	"net/netip"
	"strings"
	"testing"
	"time"
)

func TestMapCandidatesTrickleAndMultiInterface(t *testing.T) {
	ip := netip.MustParseAddr("192.168.1.3")
	relay := netip.MustParseAddrPort("192.168.1.2:5000")
	base := "v=0\r\na=fingerprint:sha-256 signed\r\na=identity:preserved\r\na=ice-ufrag:a\r\na=ice-pwd:untouched\r\n"
	initial, targets, err := MapCandidates(base, ip, 1024, 65535, relay)
	if err != nil || initial != base || len(targets) != 0 {
		t.Fatal(initial, targets, err)
	}
	s := base + "a=candidate:1 1 UDP 100 192.168.1.3 4000 typ host\r\n" +
		"a=candidate:2 1 UDP 100 10.0.0.3 4001 typ host\r\n" +
		"a=candidate:3 1 UDP 100 fd00::3 4002 typ host\r\n"
	mapped, targets, err := MapCandidates(s, ip, 1024, 65535, relay)
	if err != nil || len(targets) != 1 || !strings.HasPrefix(mapped, base) || strings.Contains(mapped, "10.0.0.3") || strings.Contains(mapped, "fd00") || !strings.Contains(mapped, "192.168.1.2 5000") {
		t.Fatal(mapped, targets, err)
	}
	if _, _, err = MapCandidates("a=remote-candidates:1 10.0.0.1 22", ip, 1, 65535, relay); err == nil {
		t.Fatal("accepted a bypass")
	}
}

func TestReassemblyExpiresWithoutFurtherTraffic(t *testing.T) {
	parts, _ := Fragment(1, 1, make([]byte, 4000))
	r := new(Reassembler)
	now := time.Now()
	_, _, _ = r.Add(parts[0], now)
	r.Expire(now.Add(6 * time.Second))
	if len(r.pending) != 0 {
		t.Fatal("idle fragments retained")
	}
	_, _, _ = r.Add(parts[0], now)
	r.DropSession(1)
	if len(r.pending) != 0 {
		t.Fatal("closed session fragments retained")
	}
}
