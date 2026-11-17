package integration

import (
	"net"
	"net/netip"
	"time"
)

func parseAP(s string) (netip.AddrPort, error) { return netip.ParseAddrPort(s) }

func dialTimeout(addr string) (net.Conn, error) { return net.DialTimeout("tcp", addr, 2*time.Second) }
