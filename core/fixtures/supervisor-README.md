# Supervisor compatibility evidence

`supervisor-go-original.txt` is the unmodified `internal/transport/supervisor.go`
from commit `6c89f64`, retained as non-executable provenance for this migration.
`supervisor-fingerprints.json` records outputs obtained by running that Go
fingerprint implementation before replacing the policy. The key-file timestamp
was set with `os.Chtimes` to 1700000000123456789 nanoseconds since the Unix epoch.
The bridge case has no discoveryPort; the next case adds discoveryPort=19132.

The Rust reducer tests freeze the corresponding decision branches: unchanged
and display-only edits, stop-before-start revocation, empty/invalid approvals,
removed games, invalid config and recovery, ten-second retry at nanosecond
precision, retained retry deadlines across removal, final-state runtime write
comparison, generation exhaustion and shutdown. Go integration tests remain the
proof that closing a game actually terminates existing revoked sessions.

The state contains control-plane metadata only. Go validates Tailcat public
keys, supplies configured key strings and key-file mtime, and executes ordered
resource effects. The reducer's Commit event compares final visible metadata
with the last published snapshot; transient start states and live counters do
not trigger extra runtime writes. Both FFI boundaries bound supervisor input at
8 MiB, matching configuration operations.
