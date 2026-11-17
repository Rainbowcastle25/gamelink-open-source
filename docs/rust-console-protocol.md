# Console protocol boundary

Rust owns candidate filtering/rewriting, textual signal parsing/formatting,
control-envelope validation/JSON, discovery headers and encrypted packets, and
advertisement serialization. Frozen original Go wire fixtures cover valid and
malformed inputs, inclusive and legacy discovery lengths, byte-level escaping,
64-bit IDs, arbitrary signal bytes, checksum failures, and nested length bounds.
The Rust discovery implementation retains the original upstream license and
notice, and uses pinned RustCrypto crates for its existing protocol crypto.

Go keeps socket reads/writes and enforces control length prefixes before body
allocation. It retains source pinning, session/resource maps, and media loops.
Datagram framing, fragmentation, and reassembly remain in Go, as chosen for the
migration: no media packet makes a per-packet FFI call. Rust validates the host
address and bounded media-port range used when a session is established.

Explicit console/discovery ABI methods accept at most 1 MiB of serialized input
on both sides of the boundary. Control frames still have their 128 KiB wire
limit and 64 KiB data limit. Binary data crosses the JSON boundary as base64;
transport errors never include raw payloads. The existing Go public APIs are
adapters and contain no second protocol parser.
