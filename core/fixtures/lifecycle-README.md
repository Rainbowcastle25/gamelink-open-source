`lifecycle-go-6c89f64.json` was generated before changing manager.go by running
`go test ./internal/client -run TestFreezeOriginalLifecycle -count=1` against
6c89f64. The generator is preserved as `lifecycle-freeze_original_go_test.go.txt` (copy
back into internal/client only on the original revision). Inputs are synthetic;
the timestamp, status counters, errors and transition pairs are fixed. No
networking, keys or addresses other than synthetic loopback endpoints are used.

These files lived in `tests/testsupport/client/testdata/` until the legacy Go
hosts were deleted; the bytes of the JSON and generator are unchanged.
