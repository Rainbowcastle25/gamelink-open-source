package main

import (
	"net"
	"net/netip"
	"os"

	"golang.org/x/net/ipv4"
	"golang.org/x/sys/unix"
)

type groupSocket struct {
	conn *net.UDPConn
}

// Go's net.ListenUDP substitutes a wildcard bind for multicast addresses.
// Bind the group directly so the socket never listens for unicast traffic.
func bindMulticast() (lanSocket, error) {
	fd, err := unix.Socket(unix.AF_INET, unix.SOCK_DGRAM|unix.SOCK_CLOEXEC, unix.IPPROTO_UDP)
	if err != nil {
		return nil, err
	}
	file := os.NewFile(uintptr(fd), "gamelink-lan-multicast")
	defer file.Close()
	if err := unix.SetsockoptInt(fd, unix.SOL_SOCKET, unix.SO_REUSEADDR, 1); err != nil {
		return nil, err
	}
	addr := &unix.SockaddrInet4{Port: int(lanGroup.Port()), Addr: lanGroup.Addr().As4()}
	if err := unix.Bind(fd, addr); err != nil {
		return nil, err
	}
	conn, err := net.FilePacketConn(file)
	if err != nil {
		return nil, err
	}
	return groupSocket{conn: conn.(*net.UDPConn)}, nil
}

func (s groupSocket) join(iface *net.Interface) error {
	return ipv4.NewPacketConn(s.conn).JoinGroup(iface, net.UDPAddrFromAddrPort(lanGroup))
}

func (s groupSocket) read(buf []byte) (int, netip.Addr, error) {
	n, src, err := s.conn.ReadFromUDPAddrPort(buf)
	return n, src.Addr(), err
}

func (s groupSocket) close() error { return s.conn.Close() }
