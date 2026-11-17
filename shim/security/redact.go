package security

import (
	"github.com/gamelink-project/gamelink/shim/rustcore"
	"sync"
)

// Redactor owns the host's registered secrets. Rust performs all filtering
// and redaction without retaining a handle or allocating persistent state.
type Redactor struct {
	mu      sync.RWMutex
	secrets []string
}

func NewRedactor(secrets ...string) *Redactor { r := &Redactor{}; r.Add(secrets...); return r }
func (r *Redactor) Add(secrets ...string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	var accepted []string
	if err := rustcore.Invoke("security.redact_add", map[string]any{"secrets": secrets}, &accepted); err != nil {
		// Keep all supplied values on a transport failure. Subsequent redaction
		// fails closed if the complete request cannot fit within its bound.
		r.secrets = append(r.secrets, secrets...)
		return
	}
	r.secrets = append(r.secrets, accepted...)
}
func (r *Redactor) Redact(s string) string {
	r.mu.RLock()
	defer r.mu.RUnlock()
	var out string
	if err := rustcore.Invoke("security.redact", map[string]any{"text": s, "secrets": r.secrets}, &out); err != nil {
		return "[redacted]"
	}
	return out
}
