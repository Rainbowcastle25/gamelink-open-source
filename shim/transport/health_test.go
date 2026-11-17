package transport

import (
	"net"
	"testing"
	"time"
)

func TestGameProbeDoesNotTreatPendingUpstreamDialAsOnline(t *testing.T) {
	// Tailcat accepts this side while the agent is still dialing upstream.
	// The pending dial outlives the 600ms probe, then fails and closes it.
	agent, client := net.Pipe()
	defer agent.Close()
	defer client.Close()
	start := time.Now()
	if got := probeGame(client); got != "unknown" {
		t.Fatalf("pending upstream dial reported %s", got)
	}
	if time.Since(start) < 500*time.Millisecond {
		t.Fatal("test did not exercise a pending dial")
	}
	agent.Close()
	if got := probeGame(client); got != "no" {
		t.Fatalf("failed upstream dial reported %s", got)
	}
}
