//go:build android

package mobile

import (
	"net"
	"net/netip"
	"sync"

	"tailscale.com/net/netmon"
)

var androidInterfaces struct {
	sync.RWMutex
	index int
	gen   uint64
	list  []netmon.Interface
}

func init() {
	// Android app UIDs cannot enumerate interfaces with net.Interfaces. Supply
	// the selected Wi-Fi interface details from ConnectivityManager instead.
	netmon.RegisterInterfaceGetter(androidInterfaceSnapshot)
}

func setPlatformLANInterface(ip netip.Addr, prefix netip.Prefix, index int) func() {
	if !ip.Is4() || !prefix.IsValid() || !prefix.Contains(ip) || index <= 0 {
		return func() {}
	}
	iface := &net.Interface{Index: index, Name: "gamelink-wifi", Flags: net.FlagUp | net.FlagBroadcast | net.FlagMulticast}
	addr := net.IP(ip.AsSlice())
	mask := net.CIDRMask(prefix.Bits(), 32)
	androidInterfaces.Lock()
	androidInterfaces.gen++
	gen := androidInterfaces.gen
	androidInterfaces.index = index
	androidInterfaces.list = []netmon.Interface{{Interface: iface, AltAddrs: []net.Addr{&net.IPNet{IP: addr, Mask: mask}}}}
	androidInterfaces.Unlock()
	return func() {
		androidInterfaces.Lock()
		if androidInterfaces.gen == gen {
			androidInterfaces.index = 0
			androidInterfaces.list = nil
			androidInterfaces.gen++
		}
		androidInterfaces.Unlock()
	}
}

func androidInterfaceSnapshot() ([]netmon.Interface, error) {
	androidInterfaces.RLock()
	defer androidInterfaces.RUnlock()
	return append([]netmon.Interface(nil), androidInterfaces.list...), nil
}
