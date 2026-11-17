package discovery

import (
	"net/netip"
	"testing"
	"time"
)

func TestPinnedPeerAndActualSource(t *testing.T) {
	a, err := Listen(netip.MustParseAddrPort("127.0.0.1:0"), netip.AddrPort{})
	if err != nil {
		t.Fatal(err)
	}
	defer a.Close()
	b, err := Listen(netip.MustParseAddrPort("127.0.0.1:0"), a.LocalAddr())
	if err != nil {
		t.Fatal(err)
	}
	defer b.Close()
	c, err := Listen(netip.MustParseAddrPort("127.0.0.1:0"), netip.AddrPort{})
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	if err := c.Send(&RequestPacket{}, b.LocalAddr()); err != nil {
		t.Fatal(err)
	}
	if err := a.Send(&RequestPacket{}, b.LocalAddr()); err != nil {
		t.Fatal(err)
	}
	_ = b.conn.SetReadDeadline(time.Now().Add(time.Second))
	d, err := b.Receive()
	if err != nil {
		t.Fatal(err)
	}
	if d.Source != a.LocalAddr() || d.SenderID != a.NetworkID() {
		t.Fatal(d)
	}
	if err := b.Send(&RequestPacket{}, c.LocalAddr()); err == nil {
		t.Fatal("sent to non-pinned endpoint")
	}
}
