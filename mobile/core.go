package mobile

import (
	"encoding/json"
	"strconv"

	"github.com/gamelink-project/gamelink/shim/rustcore"
)

// CoreABIVersion reports the C ABI version of the Rust core linked into this
// library, for diagnostics. It returns the decimal ABI number, or a string
// starting with "error: " if the core is unavailable or answered unexpectedly.
func CoreABIVersion() string {
	out, err := rustcore.Call("version", nil)
	if err != nil {
		return "error: " + err.Error()
	}
	var v struct {
		ABI *int `json:"abi"`
	}
	if err := json.Unmarshal(out, &v); err != nil || v.ABI == nil {
		return "error: malformed version response"
	}
	return strconv.Itoa(*v.ABI)
}
