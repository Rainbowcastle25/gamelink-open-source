# Desktop lifecycle boundary

`client_lifecycle.reduce` is independent of Android's unchanged
`lifecycle.reduce`. Each desktop server has an opaque JSON state, bounded to
32 KiB along with the serialized event input. The state contains generations,
observed status, check results and presentation metadata; Tailcat addresses,
private keys and Go resource handles are not state fields.

Rust decides Join/Retry eligibility, pending/success/error/cancellation state,
status-transition logs, disconnect/removal/shutdown, check suppression and
results, demo transitions, local-port edit guards and connection projections.
Go serializes reducer calls with resource registration under `Manager.mu`.
It keeps generation-scoped handles and executes all context cancellation,
client closure, tunnel stopping, timer work and waits outside that mutex.

A successful start effect first awaits all older resources for the same server,
including resources already retiring after a concurrent Disconnect. A Join marks
itself pending before cancelling a probe, and cannot start its Tailcat client
until the probe has actually closed. A stale successful Join must close its
returned tunnel before its ownership completion channel closes. Displaying
`unreachable` permits Retry while retaining the tunnel's local-port edit guard.
A live status snapshot updates measurements; a separate last-callback phase
preserves transition logs when UI polling races a callback.

Stale callbacks, probe results and demo timers cannot mutate current state.
Removal invalidates and awaits probes. Close prevents resource registration,
invalidates all states, cancels resources and waits for registered workers and
timers. UI notifications run asynchronously so a reentrant callback can request
Disconnect/Close without waiting on the resource callback that emitted it.
Activity is recorded synchronously before its notification is dispatched.

`core/fixtures/lifecycle-*` preserves the original Go fixture generator and its
fixed outputs from 6c89f64. Rust replays those projections and status transitions.
Go ownership tests use synthetic handles and scheduling gates; the desktop
integration suite exercises actual Tailcat clients against its local DERP relay.
