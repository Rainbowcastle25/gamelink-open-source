package consolebridge

import "github.com/gamelink-project/gamelink/shim/rustcore"

const (
	SignalOffer     = "offer"
	SignalAnswer    = "answer"
	SignalCandidate = "candidate"
	SignalError     = "error"
)

// Signal is the LAN NetherNet signaling payload.
type Signal struct {
	Type         string
	ConnectionID uint64
	Data         string
}
type wireSignal struct {
	Type         string
	ConnectionID uint64
	Data         []byte
}

// ParseSignal decodes bounded upstream signaling in the Rust core.
func ParseSignal(s string) (Signal, error) {
	var out wireSignal
	err := rustcore.Invoke("console.signal_parse", struct {
		Data []byte `json:"data"`
	}{[]byte(s)}, &out)
	if err != nil {
		return Signal{}, err
	}
	return Signal{out.Type, out.ConnectionID, string(out.Data)}, nil
}

// FormatSignal encodes the upstream signal format. An oversized or malformed
// host value fails closed to an empty string because this API has no error result.
func FormatSignal(s Signal) string {
	var out struct {
		Data []byte `json:"data"`
	}
	if rustcore.Invoke("console.signal_format", wireSignal{s.Type, s.ConnectionID, []byte(s.Data)}, &out) != nil {
		return ""
	}
	return string(out.Data)
}
