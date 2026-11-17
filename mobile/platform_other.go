//go:build !android

package mobile

import "net/netip"

func setPlatformLANInterface(netip.Addr, netip.Prefix, int) func() { return func() {} }
