// Package consolebridge contains the bounded wire protocol for the console relay.
package consolebridge

import (
	"encoding/binary"
	"errors"
	"sync"
	"time"
)

const (
	MaxPacket        = 1232
	MaxDatagram      = 65507 // maximum IPv4 UDP payload
	headerSize       = 24
	fragmentSize     = MaxPacket - headerSize
	maxAssemblies    = 64
	assemblyLifetime = 5 * time.Second
)

var ErrInvalidFragment = errors.New("invalid console bridge datagram fragment")

// Fragment divides a datagram into bounded Tailcat packets. IDs must be
// unique within a session for at least the reassembly lifetime.
func Fragment(session, sequence uint64, data []byte) ([][]byte, error) {
	if session == 0 || sequence == 0 || len(data) == 0 || len(data) > MaxDatagram {
		return nil, ErrInvalidFragment
	}
	count := (len(data) + fragmentSize - 1) / fragmentSize
	out := make([][]byte, 0, count)
	for i := 0; i < count; i++ {
		start := i * fragmentSize
		end := start + fragmentSize
		if end > len(data) {
			end = len(data)
		}
		p := make([]byte, headerSize+end-start)
		copy(p[:2], "GB")
		p[2] = 1
		p[3] = byte(i)
		p[4] = byte(count)
		binary.BigEndian.PutUint16(p[6:8], uint16(len(data)))
		binary.BigEndian.PutUint64(p[8:16], session)
		binary.BigEndian.PutUint64(p[16:24], sequence)
		copy(p[headerSize:], data[start:end])
		out = append(out, p)
	}
	return out, nil
}

type assemblyKey struct{ session, sequence uint64 }
type assembly struct {
	created               time.Time
	size, count, received int
	parts                 [][]byte
}

// Reassembler bounds memory and lifetime. Use a separate instance per
// authenticated peer and direction, and discard it when the peer disconnects.
type Reassembler struct {
	mu      sync.Mutex
	pending map[assemblyKey]*assembly
}

// Expire releases incomplete datagrams even when no further traffic arrives.
func (r *Reassembler) Expire(now time.Time) {
	r.mu.Lock()
	defer r.mu.Unlock()
	for k, a := range r.pending {
		if now.Before(a.created) || now.Sub(a.created) >= assemblyLifetime {
			delete(r.pending, k)
		}
	}
}

// DropSession releases all buffered fragments at a session's stop boundary.
func (r *Reassembler) DropSession(session uint64) {
	r.mu.Lock()
	defer r.mu.Unlock()
	for k := range r.pending {
		if k.session == session {
			delete(r.pending, k)
		}
	}
}

// Add accepts one packet. A nil payload with no error means more fragments
// are needed. Malformed, conflicting, or excess fragments are rejected.
func (r *Reassembler) Add(p []byte, now time.Time) (uint64, []byte, error) {
	if len(p) < headerSize || len(p) > MaxPacket || string(p[:2]) != "GB" || p[2] != 1 || p[5] != 0 {
		return 0, nil, ErrInvalidFragment
	}
	idx, count, size := int(p[3]), int(p[4]), int(binary.BigEndian.Uint16(p[6:8]))
	session, seq := binary.BigEndian.Uint64(p[8:16]), binary.BigEndian.Uint64(p[16:24])
	if session == 0 || seq == 0 || size == 0 || size > MaxDatagram || count != (size+fragmentSize-1)/fragmentSize || idx >= count {
		return 0, nil, ErrInvalidFragment
	}
	want := fragmentSize
	if idx == count-1 {
		want = size - idx*fragmentSize
	}
	if len(p)-headerSize != want {
		return 0, nil, ErrInvalidFragment
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.pending == nil {
		r.pending = make(map[assemblyKey]*assembly)
	}
	for k, a := range r.pending {
		if now.Sub(a.created) >= assemblyLifetime || now.Before(a.created) {
			delete(r.pending, k)
		}
	}
	k := assemblyKey{session, seq}
	a := r.pending[k]
	if a == nil {
		if len(r.pending) >= maxAssemblies {
			return 0, nil, ErrInvalidFragment
		}
		a = &assembly{created: now, size: size, count: count, parts: make([][]byte, count)}
		r.pending[k] = a
	}
	if a.size != size || a.count != count || a.parts[idx] != nil {
		delete(r.pending, k)
		return 0, nil, ErrInvalidFragment
	}
	a.parts[idx] = append([]byte(nil), p[headerSize:]...)
	a.received++
	if a.received != count {
		return session, nil, nil
	}
	out := make([]byte, 0, size)
	for _, part := range a.parts {
		out = append(out, part...)
	}
	delete(r.pending, k)
	return session, out, nil
}
