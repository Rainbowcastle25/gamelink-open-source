package security

import (
	"strings"
	"sync"
	"testing"
)

func TestRustRedactionBoundsFailClosed(t *testing.T) {
	r := NewRedactor("SYNTHETIC_SECRET")
	line := strings.Repeat("x", 40*1024) + " SYNTHETIC_SECRET"
	if got := r.Redact(line); got != strings.Repeat("x", 40*1024)+" [redacted]" {
		t.Fatal("dedicated redaction bound should accept a line above 32 KiB")
	}
	for _, line := range []string{strings.Repeat("x", 256*1024), strings.Repeat("\x00", 45*1024)} {
		if got := r.Redact(line); got != "[redacted]" {
			t.Fatal("oversized serialized request was not fully redacted")
		}
	}
}
func TestRustRedactionConcurrentRegistration(t *testing.T) {
	r := NewRedactor("SYNTHETIC_SECRET")
	var wg sync.WaitGroup
	for i := 0; i < 12; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := 0; j < 30; j++ {
				r.Add("ANOTHER_SYNTHETIC_SECRET")
				if strings.Contains(r.Redact("SYNTHETIC_SECRET ANOTHER_SYNTHETIC_SECRET"), "SECRET") {
					t.Error("secret leaked")
				}
			}
		}()
	}
	wg.Wait()
}
