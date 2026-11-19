package main

import (
	"slices"
	"testing"
	"time"

	"golang.org/x/sys/unix"
)

func TestLANExpiryAfterSuspendWithBackwardWallClock(t *testing.T) {
	clocks := map[int32]time.Duration{
		unix.CLOCK_BOOTTIME:  20 * time.Second,
		unix.CLOCK_MONOTONIC: 10 * time.Second,
		unix.CLOCK_REALTIME:  30 * time.Second,
	}
	discovery := newLANDiscovery()
	discovery.clock = linuxLANClock(func(id int32, ts *unix.Timespec) error {
		now, ok := clocks[id]
		if !ok {
			t.Fatalf("unexpected clock ID %d", id)
		}
		*ts = unix.NsecToTimespec(int64(now))
		return nil
	})
	world := lanWorld{Name: "Cabin", Port: 51000}
	discovery.record(world)
	if !discovery.live(world) {
		t.Fatal("a fresh announcement did not allow forwarding")
	}

	clocks[unix.CLOCK_BOOTTIME] += lanShareTTL
	clocks[unix.CLOCK_REALTIME] -= time.Second
	if discovery.live(world) {
		t.Fatal("a suspended share with a backward wall-clock jump still allowed forwarding")
	}
	worlds, err := discovery.snapshot(lanShareTTL)
	if err != nil || slices.Contains(worlds, world) {
		t.Fatalf("discovery still reported the expired world: %v, %v", worlds, err)
	}
}
