`storage-go-original.json` was captured by running the original Go `slugify`,
`security.Unseal`, and `DefaultSettings` functions in an isolated `git archive`
checkout before replacing them with Rust adapters. Inputs contain synthetic
values only. The unseal success rows describe the non-Windows format; Windows
DPAPI round trips are covered by the existing Go security and identity tests.

Rust `config::tests::original_go_fixtures` consumes this snapshot. The existing
Go config, identity, and security lifecycle tests now exercise Rust through the
C ABI. `internal/config/rust_storage_test.go` adds large snapshots, concurrent
mutations, corruption protection, and rollback checks.
