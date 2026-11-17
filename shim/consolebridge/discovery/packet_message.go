package discovery

import "io"

// MessagePacket negotiates a NetherNet connection. RecipientID is the network
// identity; Data contains the signal's separate connection ID.
type MessagePacket struct {
	RecipientID uint64
	Data        string
}

func (*MessagePacket) ID() uint16 { return IDMessagePacket }
func (pk *MessagePacket) Read(r io.Reader) error {
	v, err := readBody(r, IDMessagePacket)
	if err == nil {
		pk.RecipientID = v.Recipient
		pk.Data = string(v.Data)
	}
	return err
}
func (pk *MessagePacket) Write(w io.Writer) { writeBody(w, pk) }
