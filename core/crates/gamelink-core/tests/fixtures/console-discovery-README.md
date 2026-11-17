# Console/discovery wire fixtures

`gamelink-console-fixtures.json` and `gamelink-discovery-fixtures.json` were
captured from the original Go implementation at commit `6c89f64`, before its
parsers were removed. A temporary Go module copied the original source files
and invoked the exported functions; the generator and duplicate implementation
were kept outside the repository (`/tmp/gamelink-console-freeze`).

The fixtures record deterministic valid wire bytes and malformed-input outcomes,
including exact uint64 IDs, candidate whitespace, CRLF, filtered interfaces,
remote-candidate rejection, signal numeric suffixes, raw non-UTF-8 signal bytes,
Go JSON HTML/separator escapes, unknown fields, nulls and unpaired surrogates,
discovery encryption/checksum, inclusive and legacy-exclusive length prefixes,
nested length limits, and advertisement varints. Error text and partially mutated
outputs on errors are not a compatibility contract: failure is asserted.

All wire strings are base64. Advertisement string fields use normal JSON in the
frozen Go output; the Rust fixture runner translates these fields to the byte-safe
base64 transport representation before comparing the Rust codec results.
