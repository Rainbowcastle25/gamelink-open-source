# Performance measurements

GameLink has no battery measurement or target-hardware baseline yet. The
hermetic profile measures two processes: the test process, which hosts the
clients and the in-process DERP/STUN relay, and the shipped Rust agent running
as a child process. It provides comparable local resource snapshots without
claiming they represent a deployed server or a laptop's battery use.

Run the profile from the repository root:

```sh
python3 scripts/measure-resources.py --go /path/to/go --output /tmp/gamelink-resources.json
```

The profile starts real Tailcat servers and clients over the same local relay
setup as `tests/integration`. It records disconnected idle, connected TCP
idle, active TCP, connected UDP idle, active UDP, forced-relay idle, three
client cleanup cycles, and post-cleanup states. The direct/default-path
sample reports the path Tailcat actually measured; it may be `relay` on a
machine where direct UDP is unavailable. `connected-idle-relay` is forced
through DERP. The test fails if any client cannot connect or echo traffic.

Every phase emits one snapshot per process (`"process": "test-process"` and
`"process": "agent"`; the final phase samples the agent before it is stopped).
Each JSON snapshot includes Go heap allocation (test process only), goroutine and OS-thread counts,
RSS, open file descriptors, process CPU ticks, `/proc` voluntary and
involuntary context switches, and `/proc/<pid>/io` byte counts. Context switches
are a practical wakeup proxy, not a count of timer wakeups. CPU ticks and I/O
are cumulative process counters; compare adjacent phases for deltas. RSS and
heap are snapshots. Results include the relay and Go test runner overhead, so
use the same Go version, host, and command when comparing runs.

## Measurements

> **The numbers below are not comparable with new runs.** They were taken when
> the agent ran in the same process as the clients and relay (using the legacy Go
> host that has since been deleted), so each row was one combined process. The
> profile now reports the test process and the Rust agent separately, and the
> agent polls its configuration every second rather than every 100 ms in tests.
> The `BenchmarkStableReconcile` benchmark referred to below went away with that
> legacy host; the reconcile loop is now Rust. Treat this section as history.


Measurements below were collected on Linux amd64, Go 1.27.4, Intel i7-11800H.
The before run used the original supervisor snapshot; the final run used the
changed supervisor. Each end-to-end profile ran once, and other transport
changes landed between them. Treat the end-to-end table as observed context,
not a controlled before/after attribution. The direct path was observed on
this host; DERP was forced for the relay row. The sanitized final snapshot is
in [2026-11-14-final.json](measurements/2026-11-14-final.json), with the prior
supervisor snapshot in [2026-11-14-before.json](measurements/2026-11-14-before.json).

| Profile phase | Path | RSS MiB before → after | Heap MiB before → after | Goroutines before → after | FDs before → after |
|---|---|---:|---:|---:|---:|
| Disconnected idle | — | 32.8 → 34.5 | 2.0 → 2.0 | 7 → 7 | 11 → 11 |
| Connected idle TCP | direct | 46.7 → 44.3 | 6.2 → 6.5 | 302 → 302 | 31 → 31 |
| Active TCP | direct | 52.9 → 52.8 | 10.6 → 11.0 | 305 → 305 | 40 → 40 |
| Connected idle UDP | direct | 55.1 → 52.8 | 10.4 → 12.7 | 399 → 398 | 43 → 44 |
| Active UDP | direct | 55.1 → 52.9 | 11.4 → 14.3 | 403 → 403 | 45 → 45 |
| Connected idle | relay | 59.3 → 57.2 | 13.4 → 17.6 | 508 → 509 | 41 → 47 |
| Three cleanup cycles | relay | 63.5 → 65.3 | 20.0 → 21.7 | 446 → 415 | 39 → 37 |
| Disconnected after cleanup | — | 63.5 → 65.3 | 20.1 → 21.8 | 10 → 10 | 20 → 18 |

The final run's cumulative process counters at each phase were:

| Phase | Path | CPU user/system ticks | Voluntary/involuntary context switches | `/proc/self/io` read/write bytes |
|---|---|---:|---:|---:|
| Disconnected idle | — | 0 / 0 | 7 / 0 | 0 / 0 |
| Connected idle TCP | direct | 3 / 1 | 58 / 2 | 0 / 0 |
| Active TCP | direct | 7 / 3 | 162 / 2 | 0 / 0 |
| Connected idle UDP | direct | 8 / 3 | 162 / 2 | 0 / 0 |
| Active UDP | direct | 8 / 3 | 162 / 2 | 0 / 0 |
| Connected idle | relay | 16 / 6 | 368 / 3 | 0 / 0 |
| Three cleanup cycles | relay | 27 / 11 | 472 / 9 | 0 / 0 |
| Disconnected after cleanup | — | 27 / 11 | 472 / 9 | 0 / 0 |

CPU ticks, context switches and `/proc/self/io` are cumulative process
counters, not per-phase deltas. Context switches are a wakeup proxy, not a
timer-wakeup count. Linux `/proc/self/io` physical bytes do not include socket
traffic or all filesystem writes; the zero read/write bytes here mean no
physical block I/O was charged to this process in the sample.

The focused stable-reconcile benchmark isolates the supervisor against one
unchanged inactive game. On the same host and toolchain, before → final repeat
was 2,005,574 ns/op → 3,807 ns/op, 3,837 → 1,160 B/op, and 28 → 7
allocations/op. The runtime file changed during the before benchmark and stayed
byte-for-byte unchanged during the final benchmark.

```sh
go test -run '^$' -bench BenchmarkStableReconcile -benchmem -count=1 ./shim/transport
```

This isolates the effect of avoiding JSON decode and atomic runtime-file
replacement on unchanged polls. It does not establish a battery or deployed
server improvement. Hardware battery impact and target-server resource use
remain unmeasured.

The Windows desktop frontend refreshes live counters every 15 seconds instead
of every two seconds. Backend state/path events still trigger immediate UI
refreshes. The transport's five-second health check remains unchanged for
reliable reconnect behavior. On the host, supervisor config is still checked
every poll; unchanged JSON is not decoded, and runtime status writes happen on
state changes plus the five-second heartbeat. No battery claim is inferred
from these changes.
