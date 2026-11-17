package identity

import "github.com/gamelink-project/gamelink/shim/rustcore"

// validatePublicKeyFormat leaves only conversion to Tailcat's Go key type in
// the host. All text validation and canonicalization are owned by Rust.
func validatePublicKeyFormat(s string) (string, error) {
	var canonical string
	err := rustcore.Invoke("identity.public_key", map[string]any{"text": s}, &canonical)
	return canonical, err
}
