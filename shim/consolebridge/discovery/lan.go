package discovery

import (
	"errors"
	"golang.org/x/net/ipv4"
	"golang.org/x/time/rate"
	"net"
	"net/netip"
)

// ListenLAN receives broadcast discovery using IP_PKTINFO. A wildcard receive
// socket is necessary for broadcasts on Linux/Android; every packet is checked
// against the explicitly selected interface and subnet before decoding, and
// replies use the selected source address/interface. No root or VPN is needed.
func ListenLAN(ip netip.Addr, prefix netip.Prefix, index int, port uint16) (*Listener, error) {
	if !ip.Is4() || !prefix.IsValid() || !prefix.Contains(ip) || index <= 0 {
		return nil, errors.New("invalid selected LAN")
	}
	l, err := Listen(netip.AddrPortFrom(netip.IPv4Unspecified(), port), netip.AddrPort{})
	if err != nil {
		return nil, err
	}
	l.lan = &lanSocket{pc: ipv4.NewPacketConn(l.conn), ip: ip, prefix: prefix.Masked(), index: index, packets: rate.NewLimiter(500, 128)}
	if err = l.lan.pc.SetControlMessage(ipv4.FlagInterface|ipv4.FlagDst, true); err != nil {
		l.Close()
		return nil, err
	}
	return l, nil
}

type lanSocket struct {
	pc      *ipv4.PacketConn
	ip      netip.Addr
	prefix  netip.Prefix
	index   int
	packets *rate.Limiter
}

func (l *Listener) readPacket(b []byte) (int, netip.AddrPort, error) {
	if l.lan == nil {
		return l.conn.ReadFromUDPAddrPort(b)
	}
	for {
		n, cm, src, err := l.lan.pc.ReadFrom(b)
		if err != nil {
			return 0, netip.AddrPort{}, err
		}
		address, ok := src.(*net.UDPAddr)
		if !ok || cm == nil || cm.IfIndex != l.lan.index {
			continue
		}
		ap := address.AddrPort()
		ap = netip.AddrPortFrom(ap.Addr().Unmap(), ap.Port())
		if !l.lan.prefix.Contains(ap.Addr()) || ap.Addr() == l.lan.ip || ap.Addr().IsMulticast() || ap.Addr().IsUnspecified() || !l.lan.packets.Allow() {
			continue
		}
		return n, ap, nil
	}
}
func (l *Listener) writePacket(b []byte, target netip.AddrPort) error {
	if l.lan == nil {
		_, err := l.conn.WriteToUDPAddrPort(b, target)
		return err
	}
	if !l.lan.prefix.Contains(target.Addr()) || target.Addr() == l.lan.ip {
		return errors.New("target outside selected LAN")
	}
	_, err := l.lan.pc.WriteTo(b, &ipv4.ControlMessage{Src: net.IP(l.lan.ip.AsSlice()), IfIndex: l.lan.index}, net.UDPAddrFromAddrPort(target))
	return err
}
