These are frozen compatibility results from the Go implementation before
phase 2. Both Rust and the public Go adapters consume them.

`pure.json` was generated using `internal/purefixtures/generate.go` with the
original Go packages at commit `f607be0`, including Go's Unicode 17 tables.
The invitation addresses use freshly generated test keys and synthetic relay
metadata. No production invitation or private key is included. Do not replace
them with real user data.

The secret scanner exempts only the reviewed SHA-256 of this generated file.
Any edit removes that exemption; regenerating it requires inspecting the data
and updating the digest deliberately. Private-key patterns remain forbidden
everywhere else.

`storage-go-original.json` captures original Go defaults, slug conversion,
and non-Windows secret-envelope behavior.
