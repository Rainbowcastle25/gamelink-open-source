package consolebridge

import (
	"encoding/binary"
	"errors"
	"io"

	"github.com/gamelink-project/gamelink/shim/rustcore"
)

const MaxControlFrame = 128 * 1024

// ControlMessage is the typed, versioned signaling envelope. Data contains
// opaque signaling or advertisement bytes; callers must not log it.
type ControlMessage struct {
	Version int    `json:"version"`
	Type    string `json:"type"`
	Session uint64 `json:"session,omitempty"`
	PeerIP  string `json:"peerIp,omitempty"`
	Data    string `json:"data,omitempty"`
}

// WriteControl writes one complete length-prefixed JSON frame.
func WriteControl(w io.Writer, m ControlMessage) error {
	var out struct {
		Data []byte `json:"data"`
	}
	if err := rustcore.Invoke("console.control_encode", m, &out); err != nil {
		return err
	}
	data := out.Data
	var header [4]byte
	binary.BigEndian.PutUint32(header[:], uint32(len(data)))
	for _, b := range [][]byte{header[:], data} {
		for len(b) > 0 {
			n, err := w.Write(b)
			if err != nil {
				return err
			}
			if n <= 0 || n > len(b) {
				return io.ErrShortWrite
			}
			b = b[n:]
		}
	}
	return nil
}

// ReadControl enforces the limit before allocation and rejects unknown fields.
func ReadControl(r io.Reader) (ControlMessage, error) {
	var m ControlMessage
	var h [4]byte
	if _, err := io.ReadFull(r, h[:]); err != nil {
		return m, err
	}
	n := binary.BigEndian.Uint32(h[:])
	if n == 0 || n > MaxControlFrame {
		return m, errors.New("invalid console bridge frame length")
	}
	b := make([]byte, n)
	if _, err := io.ReadFull(r, b); err != nil {
		return m, err
	}
	err := rustcore.Invoke("console.control_decode", struct {
		Data []byte `json:"data"`
	}{b}, &m)
	return m, err
}
