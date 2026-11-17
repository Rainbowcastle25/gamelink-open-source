package rustcore

import (
	"encoding/json"
	"errors"
)

// Invoke calls a domain operation, preserving its user-visible error text.
// The transport envelope never echoes malformed JSON or raw secret input.
func Invoke(method string, input any, output any) error {
	data, err := json.Marshal(input)
	if err != nil {
		return err
	}
	raw, err := Call(method, data)
	if err != nil {
		return err
	}
	var result struct {
		Value json.RawMessage `json:"value"`
		Error string          `json:"error"`
	}
	if err := json.Unmarshal(raw, &result); err != nil {
		return errMalformed
	}
	if result.Error != "" {
		return errors.New(result.Error)
	}
	if output == nil {
		return nil
	}
	return json.Unmarshal(result.Value, output)
}
