package consolebridge

import (
	"net/netip"
	"strings"
	"testing"
)

func TestRewriteCandidatesPinsAndPreservesIdentity(t *testing.T) {
	s := "v=0\r\na=identity:opaque\r\na=fingerprint:sha-256 abc\r\na=ice-pwd:secret\r\na=candidate:1 1 UDP 123 192.0.2.4 4000 typ host\r\n"
	got, targets, err := RewriteCandidates(s, netip.MustParseAddr("192.0.2.4"), 4000, 4010, netip.MustParseAddrPort("192.0.2.8:5000"))
	if err != nil {
		t.Fatal(err)
	}
	if len(targets) != 1 || targets[0].String() != "192.0.2.4:4000" || !strings.Contains(got, "192.0.2.8 5000") || !strings.Contains(got, "a=identity:opaque\r\na=fingerprint:sha-256 abc\r\na=ice-pwd:secret") {
		t.Fatal(got, targets)
	}
	for _, bad := range []string{strings.Replace(s, "192.0.2.4", "192.0.2.5", 1), strings.Replace(s, "4000", "3999", 1), strings.Replace(s, "UDP", "TCP", 1)} {
		if _, _, err := RewriteCandidates(bad, netip.MustParseAddr("192.0.2.4"), 4000, 4010, netip.MustParseAddrPort("192.0.2.8:5000")); err == nil {
			t.Fatal("accepted unsafe candidate")
		}
	}
}

func TestRejectsUnparsedCandidateLines(t *testing.T) {
	for _, s := range []string{"a=candidate:1 1 UDP 123 192.0.2.4 4000 typ host\r\na=remote-candidates:1 192.0.2.9 5000\r\n", " A=CANDIDATE:1 1 UDP 123 192.0.2.4 4000 typ host"} {
		if _, _, err := RewriteCandidates(s, netip.MustParseAddr("192.0.2.4"), 4000, 4010, netip.MustParseAddrPort("192.0.2.8:5000")); err == nil {
			t.Fatal("accepted unparsed candidate")
		}
	}
}
