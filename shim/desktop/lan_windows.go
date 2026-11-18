package main

import (
	"context"
	"errors"
	"net"
	"net/netip"
	"syscall"
	"time"
	"unsafe"

	"golang.org/x/net/ipv4"
	"golang.org/x/sys/windows"
)

// Go's monotonic clock on Windows advances across suspend.
func lanNow() time.Time { return time.Now() }

type groupSocket struct {
	conn *net.UDPConn
}

// Winsock delivers multicast only to sockets bound to the wildcard address or
// to the receiving interface's address. Minecraft announces on the default
// interface, so a loopback bind receives nothing. Bind the wildcard and accept
// only datagrams whose destination, from Winsock's packet metadata, is the
// group; listenLAN also requires a source address of this PC. Never sends.
func bindMulticast() (lanSocket, error) {
	config := net.ListenConfig{Control: func(_, _ string, raw syscall.RawConn) error {
		var optionError error
		if err := raw.Control(func(fd uintptr) {
			optionError = windows.SetsockoptInt(windows.Handle(fd), windows.IPPROTO_IP, windows.IP_PKTINFO, 1)
		}); err != nil {
			return err
		}
		return optionError
	}}
	addr := netip.AddrPortFrom(netip.IPv4Unspecified(), lanGroup.Port())
	conn, err := config.ListenPacket(context.Background(), "udp4", addr.String())
	if err != nil {
		return nil, err
	}
	return groupSocket{conn: conn.(*net.UDPConn)}, nil
}

func (s groupSocket) join(iface *net.Interface) error {
	return ipv4.NewPacketConn(s.conn).JoinGroup(iface, net.UDPAddrFromAddrPort(lanGroup))
}

func (s groupSocket) read(buf []byte) (int, netip.Addr, error) {
	var info struct {
		header  windows.WSACMSGHDR
		addr    [4]byte
		ifIndex uint32
	}
	control := unsafe.Slice((*byte)(unsafe.Pointer(&info)), int(unsafe.Sizeof(info)))
	n, controlLen, flags, src, err := s.conn.ReadMsgUDPAddrPort(buf, control)
	if errors.Is(err, windows.WSAEMSGSIZE) {
		return 0, netip.Addr{}, nil // discard truncated datagrams
	}
	if err != nil {
		return 0, netip.Addr{}, err
	}
	if controlLen != len(control) || info.header.Len != uintptr(controlLen) ||
		info.header.Level != windows.IPPROTO_IP || info.header.Type != windows.IP_PKTINFO ||
		flags&(windows.MSG_TRUNC|windows.MSG_CTRUNC) != 0 || info.addr != lanGroup.Addr().As4() {
		return 0, netip.Addr{}, nil
	}
	return n, src.Addr(), nil
}

func (s groupSocket) close() error { return s.conn.Close() }
