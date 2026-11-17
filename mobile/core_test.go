//go:build cgo

package mobile

import "testing"

func TestCoreABIVersion(t *testing.T) {
	if got := CoreABIVersion(); got != "1" {
		t.Fatalf("CoreABIVersion() = %q, want %q", got, "1")
	}
}
