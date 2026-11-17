package discovery

import (
	"bytes"
	"encoding/binary"
	"testing"
)

func TestRustAdvertisementAndSignalPreserveBytes(t *testing.T) {
	ad := testServerData("raw\xff", "world\x00\xfe")
	ad.Nonce = "\xffnonce"
	wire, err := ad.MarshalBinary()
	if err != nil {
		t.Fatal(err)
	}
	var got ServerData
	if err := got.UnmarshalBinary(wire); err != nil || got != *ad {
		t.Fatalf("advertisement bytes changed: %v", err)
	}
	message := &MessagePacket{RecipientID: ^uint64(0), Data: "CONNECTREQUEST 1 raw\x00\xff"}
	pk, id, err := Unmarshal(Marshal(message, ^uint64(0)))
	if err != nil || id != ^uint64(0) {
		t.Fatal(err)
	}
	if got, ok := pk.(*MessagePacket); !ok || *got != *message {
		t.Fatal("message bytes changed")
	}
	// Public body methods retain the same byte contract as sealed packets.
	var body bytes.Buffer
	message.Write(&body)
	var bodyMessage MessagePacket
	if err := bodyMessage.Read(&body); err != nil || bodyMessage != *message {
		t.Fatalf("body parity: %v", err)
	}
}
func TestRustDiscoveryFailClosedAndBodyLengths(t *testing.T) {
	wire := Marshal(&RequestPacket{}, 1)
	for _, index := range []int{0, 31, 32, len(wire) - 1} {
		bad := append([]byte(nil), wire...)
		bad[index] ^= 1
		if _, _, err := Unmarshal(bad); err == nil {
			t.Fatal("accepted corrupt discovery packet")
		}
	}
	for _, header := range [][]byte{binary.LittleEndian.AppendUint32(nil, ^uint32(0)), append(make([]byte, 8), 255, 255, 255, 255)} {
		var err error
		if len(header) == 4 {
			err = new(ResponsePacket).Read(bytes.NewReader(header))
		} else {
			err = new(MessagePacket).Read(bytes.NewReader(header))
		}
		if err == nil {
			t.Fatal("accepted hostile nested length")
		}
	}
}
