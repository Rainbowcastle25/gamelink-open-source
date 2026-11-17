//go:build cgo

// Package rustcore calls the GameLink Rust core (core/crates/gamelink-ffi)
// through its C ABI. Every call is JSON in, JSON out.
//
// The Rust static library must be built first:
//
//	bash scripts/build-rust-ffi.sh
//
// Shared invitation, security, identity and configuration adapters depend on it.
package rustcore

/*
#cgo CFLAGS: -I${SRCDIR}/../../core/crates/gamelink-ffi/include
// Link the staticlib by full path so the linker can never pick a .so. The
// archive under core/target/cgo is produced by scripts/build-rust-ffi.sh with
// LLVM bitcode sections removed (cmd/go rejects archives containing them).
// Extra libraries follow rustc's --print native-static-libs for the target,
// with two windows-gnu changes for zig's MinGW toolchain (docs/building.md):
//   - -lsynchronization is spelled -lapi-ms-win-core-synch-l1-2-0. MinGW-w64's
//     libsynchronization.a is that same import library under another name;
//     zig ships only the API-set name.
//   - -lunwind (zig's LLVM libunwind) stands in for -lgcc_eh, which rustc adds
//     through the target spec rather than native-static-libs. It provides the
//     same SEH _GCC_specific_handler/_Unwind_* ABI that Rust's panic runtime
//     uses. Compiler builtins (___chkstk_ms, __udivti3, ...) are defined by
//     both Rust's compiler_builtins (strong, one symbol per archive member)
//     and zig's compiler-rt (weak), so there is no duplicate definition and
//     no -lgcc is needed.
#cgo linux,amd64,!android,!rust_musl LDFLAGS: ${SRCDIR}/../../core/target/cgo/x86_64-unknown-linux-gnu/libgamelink_ffi.a -lgcc_s -lutil -lrt -lpthread -lm -ldl -lc
#cgo linux,amd64,!android,rust_musl LDFLAGS: ${SRCDIR}/../../core/target/cgo/x86_64-unknown-linux-musl/libgamelink_ffi.a -lunwind -lpthread -lm -ldl -lc
#cgo linux,arm64,!android,rust_musl LDFLAGS: ${SRCDIR}/../../core/target/cgo/aarch64-unknown-linux-musl/libgamelink_ffi.a -lunwind -lpthread -lm -ldl -lc
#cgo windows,amd64 LDFLAGS: ${SRCDIR}/../../core/target/cgo/x86_64-pc-windows-gnu/libgamelink_ffi.a -lcrypt32 -ladvapi32 -lkernel32 -lntdll -luserenv -lws2_32 -ldbghelp -lapi-ms-win-core-synch-l1-2-0 -lunwind
#cgo android,arm64 LDFLAGS: ${SRCDIR}/../../core/target/cgo/aarch64-linux-android/libgamelink_ffi.a -ldl -llog -lunwind -lm -lc
#cgo android,amd64 LDFLAGS: ${SRCDIR}/../../core/target/cgo/x86_64-linux-android/libgamelink_ffi.a -ldl -llog -lunwind -lm -lc
#include "gamelink_ffi.h"
*/
import "C"

import (
	"encoding/json"
	"unsafe"
)

// Call invokes a Rust core method with JSON input and returns the method's
// JSON result. Failures are returned as *Error. Inputs larger than
// the method's input limit and method names longer than MaxMethodBytes are
// rejected before crossing into Rust. The default input limit is MaxInputBytes.
func Call(method string, input []byte) (json.RawMessage, error) {
	if err := precheck(method, input); err != nil {
		return nil, err
	}
	return callRust(method, input)
}

// callRust crosses into Rust without the Go-side prechecks, so tests can
// confirm that Rust enforces the same limits.
func callRust(method string, input []byte) (json.RawMessage, error) {
	buf := C.gamelink_call(
		(*C.uint8_t)(unsafe.Pointer(unsafe.StringData(method))), C.size_t(len(method)),
		(*C.uint8_t)(unsafe.Pointer(unsafe.SliceData(input))), C.size_t(len(input)),
	)
	defer C.gamelink_buffer_free(buf)
	if buf.ptr == nil {
		return nil, errMalformed
	}
	// Copy into Go memory before the deferred free releases the Rust buffer.
	out := C.GoBytes(unsafe.Pointer(buf.ptr), C.int(buf.len))
	return decodeEnvelope(out)
}
