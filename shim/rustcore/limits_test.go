package rustcore

import "testing"

func TestDedicatedMethodLimits(t *testing.T) {
	for _, tc := range []struct {
		method string
		limit  int
	}{
		{"version", 32 * 1024}, {"invite.decode", 32 * 1024}, {"security.redact_add", 32 * 1024},
		{"security.redact", 256 * 1024}, {"config.save", 8 * 1024 * 1024}, {"storage.write", 8 * 1024 * 1024},
		{"supervisor.reduce", 8 * 1024 * 1024}, {"supervisor.unknown", 32 * 1024},
		{"storage.unknown", 32 * 1024}, {"config.unknown", 32 * 1024},
		{"console.control_encode", 1024 * 1024}, {"discovery.packet_decode", 1024 * 1024},
		{"console.unknown", 32 * 1024}, {"discovery.unknown", 32 * 1024},
	} {
		if got := inputLimit(tc.method); got != tc.limit {
			t.Fatalf("%s limit %d != %d", tc.method, got, tc.limit)
		}
		if err := precheck(tc.method, make([]byte, tc.limit)); err != nil {
			t.Fatalf("at limit %s: %v", tc.method, err)
		}
		if err := precheck(tc.method, make([]byte, tc.limit+1)); err == nil {
			t.Fatalf("above limit %s accepted", tc.method)
		}
	}
}
