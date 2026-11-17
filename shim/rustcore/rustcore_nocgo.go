//go:build !cgo

package rustcore

import "encoding/json"

// Call reports that the Rust core is unavailable: it is linked through cgo,
// and this binary was built with CGO_ENABLED=0.
func Call(method string, input []byte) (json.RawMessage, error) {
	if err := precheck(method, input); err != nil {
		return nil, err
	}
	return nil, &Error{Code: "unavailable", Message: "The Rust core requires a cgo build."}
}
