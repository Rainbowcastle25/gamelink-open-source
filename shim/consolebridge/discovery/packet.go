package discovery

import (
	"bytes"
	"encoding/binary"
	"errors"
	"io"

	"github.com/gamelink-project/gamelink/shim/rustcore"
)

// Packet is a typed LAN discovery control packet, encoded by the Rust core.
type Packet interface {
	ID() uint16
	Read(io.Reader) error
	Write(io.Writer)
}

const maxPacketPayloadLength = 1<<16 - 1
const (
	IDRequestPacket uint16 = iota
	IDResponsePacket
	IDMessagePacket
)

type packetWire struct {
	ID        uint16 `json:"id"`
	Sender    uint64 `json:"sender"`
	Recipient uint64 `json:"recipient,omitempty"`
	Data      []byte `json:"data,omitempty"`
}

func packetValue(pk Packet) (packetWire, bool) {
	var v packetWire
	switch p := pk.(type) {
	case *RequestPacket:
		if p == nil {
			return v, false
		}
		v.ID = IDRequestPacket
	case *ResponsePacket:
		if p == nil {
			return v, false
		}
		v.ID = IDResponsePacket
		v.Data = p.ApplicationData
	case *MessagePacket:
		if p == nil {
			return v, false
		}
		v.ID = IDMessagePacket
		v.Recipient = p.RecipientID
		v.Data = []byte(p.Data)
	default:
		return v, false
	}
	return v, true
}

// Marshal encodes and seals one control packet. Invalid or oversized host
// values return nil because this upstream-compatible API has no error result.
func Marshal(pk Packet, senderID uint64) []byte {
	v, ok := packetValue(pk)
	if !ok {
		return nil
	}
	v.Sender = senderID
	// Explicit empty byte values must cross as base64, not null or absent.
	var out struct {
		Data []byte `json:"data"`
	}
	if invokePacket("discovery.packet_encode", v, &out) != nil {
		return nil
	}
	return out.Data
}
func invokePacket(method string, v packetWire, out any) error {
	// Empty data and zero recipients are valid payload values.
	return rustcore.Invoke(method, struct {
		ID        uint16 `json:"id"`
		Sender    uint64 `json:"sender"`
		Recipient uint64 `json:"recipient"`
		Data      []byte `json:"data"`
	}{v.ID, v.Sender, v.Recipient, nonNil(v.Data)}, out)
}
func nonNil(b []byte) []byte {
	if b == nil {
		return []byte{}
	}
	return b
}

// Unmarshal verifies the wire checksum and decodes one bounded packet.
// The LAN key is shared: callers must still validate and pin its UDP source.
func Unmarshal(b []byte) (Packet, uint64, error) {
	var v packetWire
	if err := rustcore.Invoke("discovery.packet_decode", struct {
		Data []byte `json:"data"`
	}{nonNil(b)}, &v); err != nil {
		return nil, 0, err
	}
	switch v.ID {
	case IDRequestPacket:
		return &RequestPacket{}, v.Sender, nil
	case IDResponsePacket:
		return &ResponsePacket{ApplicationData: v.Data}, v.Sender, nil
	case IDMessagePacket:
		return &MessagePacket{RecipientID: v.Recipient, Data: string(v.Data)}, v.Sender, nil
	}
	return nil, v.Sender, errors.New("unknown discovery packet")
}
func writeBody(w io.Writer, pk Packet) {
	v, ok := packetValue(pk)
	if !ok {
		return
	}
	var out struct {
		Data []byte `json:"data"`
	}
	if invokePacket("discovery.body_encode", v, &out) == nil {
		_, _ = w.Write(out.Data)
	}
}

// Body readers retain only length-prefixed I/O in Go. Guard nested lengths
// before allocation; field decoding and validation live in Rust.
func readBody(r io.Reader, id uint16) (packetWire, error) {
	var out packetWire
	prefix := 0
	switch id {
	case IDRequestPacket:
		return out, nil
	case IDResponsePacket:
		prefix = 4
	case IDMessagePacket:
		prefix = 12
	default:
		return out, errors.New("unknown discovery packet")
	}
	h := make([]byte, prefix)
	if _, err := io.ReadFull(r, h); err != nil {
		return out, err
	}
	n := binary.LittleEndian.Uint32(h[prefix-4:])
	if n > maxPacketPayloadLength {
		return out, errors.New("invalid length")
	}
	if l, ok := r.(interface{ Len() int }); ok && int(n) > l.Len() {
		return out, errors.New("invalid length")
	}
	if int(n) > maxPacketPayloadLength-prefix {
		return out, errors.New("invalid length")
	}
	b := make([]byte, prefix+int(n))
	copy(b, h)
	if _, err := io.ReadFull(r, b[prefix:]); err != nil {
		return out, err
	}
	if id == IDMessagePacket {
		if buf, ok := r.(*bytes.Buffer); ok {
			if buf.Len() > maxPacketPayloadLength-len(b) {
				return out, errors.New("invalid length")
			}
			b = append(b, buf.Next(buf.Len())...)
		}
	}
	err := rustcore.Invoke("discovery.body_decode", struct {
		ID   uint16 `json:"id"`
		Data []byte `json:"data"`
	}{id, b}, &out)
	return out, err
}

// Header is the fixed discovery packet header.
type Header struct {
	PacketID uint16
	SenderID uint64
}

func (h *Header) Read(r io.Reader) error {
	var b [18]byte
	if _, err := io.ReadFull(r, b[:]); err != nil {
		return err
	}
	var v packetWire
	if err := rustcore.Invoke("discovery.header_decode", struct {
		Data []byte `json:"data"`
	}{b[:]}, &v); err != nil {
		return err
	}
	h.PacketID = v.ID
	h.SenderID = v.Sender
	return nil
}
func (h *Header) Write(w io.Writer) {
	var out struct {
		Data []byte `json:"data"`
	}
	if invokePacket("discovery.header_encode", packetWire{ID: h.PacketID, Sender: h.SenderID}, &out) == nil {
		_, _ = w.Write(out.Data)
	}
}
