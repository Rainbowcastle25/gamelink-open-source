package transport

import (
	"sync"
	"sync/atomic"
	"testing"
)

func TestTCPAdmissionReservesCapacityAtomically(t *testing.T) {
	tun := &Tunnel{tcpSlots: make(chan struct{}, maxLocalConns)}
	var accepted atomic.Int64
	var wg sync.WaitGroup
	for i := 0; i < maxLocalConns*8; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if tun.reserveTCP() {
				accepted.Add(1)
			}
		}()
	}
	wg.Wait()
	if got := accepted.Load(); got != maxLocalConns {
		t.Fatalf("reserved %d TCP handlers, want exactly the limit %d", got, maxLocalConns)
	}
	if tun.reserveTCP() {
		t.Fatal("reserved a TCP handler after reaching the limit")
	}
	for i := 0; i < maxLocalConns; i++ {
		tun.releaseTCP()
	}
	if !tun.reserveTCP() {
		t.Fatal("capacity was not released when a TCP handler completed")
	}
}
