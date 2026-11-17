`pure.json` records the original Go APIs at phase-1 commit `f607be0`.
`internal/purefixtures/generate.go` produced it before the APIs switched to Rust.
Additional cases were recorded against that same commit using Go's `-overlay`
feature to map original security/invite/games/identity sources into the current
checkout. Never regenerate expected behavior against the Rust-backed adapters.

The addresses, keys and secret strings are synthetic test inputs, generated only
for this file and never used to authenticate any real service. No private node
key is retained. Fixtures cover display Unicode, error quoting, IP parsing,
relay policy, invitation encoding/decoding, game metadata, public-key formats,
and redaction. Rust's pure module tests and Go's public adapter tests both read
this exact JSON. The checked-in Go Unicode tables preserve Go 1.26's Unicode
17.0.0 display semantics independently of Rust's Unicode version.
