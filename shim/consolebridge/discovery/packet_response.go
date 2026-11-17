package discovery

import "io"

// ResponsePacket advertises an application's world data.
type ResponsePacket struct{ ApplicationData []byte }

func (*ResponsePacket) ID() uint16 { return IDResponsePacket }
func (pk *ResponsePacket) Read(r io.Reader) error {
	v, err := readBody(r, IDResponsePacket)
	if err == nil {
		pk.ApplicationData = v.Data
	}
	return err
}
func (pk *ResponsePacket) Write(w io.Writer) { writeBody(w, pk) }
