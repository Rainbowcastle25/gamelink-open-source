package rustcore

import (
	"encoding/json"
	"fmt"
)

// MaxInputBytes mirrors GAMELINK_MAX_INPUT_BYTES in the Rust C ABI.
const MaxInputBytes = 32 * 1024

// MaxMethodBytes mirrors GAMELINK_MAX_METHOD_BYTES in the Rust C ABI.
const MaxMethodBytes = 64

// Error is a failure reported by the Rust core (or by the Go-side prechecks,
// which use the same codes). Messages never contain input content.
type Error struct {
	Code    string
	Message string
}

func (e *Error) Error() string {
	return fmt.Sprintf("rustcore: %s: %s", e.Code, e.Message)
}

var errMalformed = &Error{Code: "internal", Message: "The core returned a malformed response."}

func precheck(method string, input []byte) error {
	if len(method) > MaxMethodBytes {
		return &Error{Code: "method_too_long", Message: "The method name is too long."}
	}
	if len(input) > inputLimit(method) {
		return &Error{Code: "input_too_large", Message: "The input exceeds the size limit."}
	}
	return nil
}

func decodeEnvelope(raw []byte) (json.RawMessage, error) {
	var env struct {
		Ok    json.RawMessage `json:"ok"`
		Error *struct {
			Code    string `json:"code"`
			Message string `json:"message"`
		} `json:"error"`
	}
	if err := json.Unmarshal(raw, &env); err != nil {
		return nil, errMalformed
	}
	switch {
	case env.Error != nil && env.Ok == nil:
		return nil, &Error{Code: env.Error.Code, Message: env.Error.Message}
	case env.Error == nil && env.Ok != nil:
		return env.Ok, nil
	default:
		return nil, errMalformed
	}
}

// Limits apply to serialized JSON, including string escaping expansion.
func inputLimit(method string) int {
	switch method {
	case "console.candidates_rewrite", "console.candidates_map", "console.signal_parse", "console.signal_format", "console.control_encode", "console.control_decode", "console.host_ip", "console.host_config", "discovery.packet_encode", "discovery.packet_decode", "discovery.body_encode", "discovery.body_decode", "discovery.header_encode", "discovery.header_decode", "discovery.ad_encode", "discovery.ad_decode":
		return 1024 * 1024
	case "storage.ensure_dir", "storage.write", "storage.read", "storage.seal", "storage.unseal", "storage.identity_load", "storage.identity_save", "config.bridge", "config.default_settings", "config.client_open", "config.agent_load", "config.agent_decode", "config.save", "config.client_mutate", "config.game_prepare", "config.new_id", "config.keys_load", "config.agent_mutate", "supervisor.reduce":
		return 8 * 1024 * 1024
	case "security.redact":
		return 256 * 1024
	default:
		return MaxInputBytes
	}
}
