//! C ABI (for Go via cgo) and JNI (for Android) over `gamelink-core`.
//!
//! The C ABI is a single JSON-in/JSON-out entrypoint, [`gamelink_call`], that
//! always returns an owned [`GamelinkBuffer`] holding a JSON envelope:
//! `{"ok": <value>}` or `{"error": {"code": "...", "message": "..."}}`.
//! The caller must release every returned buffer with [`gamelink_buffer_free`].
//!
//! Transport errors are fixed strings and never echo malformed input. Domain
//! operations preserve validated user-facing errors. Invitations and private
//! keys cross this boundary; owned response buffers are zeroed on release.

use std::panic::{catch_unwind, AssertUnwindSafe};

use gamelink_core::ReduceError;
use serde::Serialize;
use zeroize::Zeroize;

#[cfg(feature = "jni")]
mod jni_bridge;

/// Version of this C ABI. Bump on any incompatible change to the envelope,
/// the buffer layout or a method's contract.
pub const GAMELINK_ABI_VERSION: u32 = 1;
/// Maximum accepted input size in bytes. Larger inputs are rejected with
/// `input_too_large` before any parsing.
pub const GAMELINK_MAX_INPUT_BYTES: usize = 32 * 1024;
/// Maximum accepted method name size in bytes.
pub const GAMELINK_MAX_METHOD_BYTES: usize = 64;

/// Rust-side alias of [`GAMELINK_MAX_INPUT_BYTES`].
pub const MAX_INPUT_BYTES: usize = GAMELINK_MAX_INPUT_BYTES;

const _: () = assert!(MAX_INPUT_BYTES == gamelink_core::MAX_INPUT_BYTES);

/// An owned byte buffer allocated by Rust. Release it with
/// [`gamelink_buffer_free`] exactly once. A zeroed buffer is valid and empty.
#[repr(C)]
#[derive(Debug)]
pub struct GamelinkBuffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub cap: usize,
}

impl GamelinkBuffer {
    fn from_vec(bytes: Vec<u8>) -> Self {
        let mut bytes = std::mem::ManuallyDrop::new(bytes);
        Self {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
            cap: bytes.capacity(),
        }
    }
}

/// A failed call: stable machine-readable `code` and a fixed, input-free
/// human-readable `message`.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct CallError {
    pub code: &'static str,
    pub message: &'static str,
}

impl CallError {
    const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

const ERR_INVALID_ARGUMENT: CallError = CallError::new(
    "invalid_argument",
    "A pointer was null with a non-zero length.",
);
const ERR_METHOD_TOO_LONG: CallError =
    CallError::new("method_too_long", "The method name is too long.");
const ERR_INPUT_TOO_LARGE: CallError =
    CallError::new("input_too_large", "The input exceeds the size limit.");
const ERR_INVALID_UTF8: CallError =
    CallError::new("invalid_utf8", "The method or input is not valid UTF-8.");
const ERR_UNKNOWN_METHOD: CallError = CallError::new("unknown_method", "The method is not known.");
const ERR_INVALID_JSON: CallError =
    CallError::new("invalid_json", "The input is not valid for this method.");
const ERR_GENERATION_EXHAUSTED: CallError = CallError::new(
    "generation_exhausted",
    "The lifecycle generation counter is exhausted.",
);
/// The `internal` error, hand-written so the fallback can never fail to
/// serialize. Returned when a call panics.
const INTERNAL_ENVELOPE: &str =
    r#"{"error":{"code":"internal","message":"An internal error occurred."}}"#;

impl From<ReduceError> for CallError {
    fn from(error: ReduceError) -> Self {
        match error {
            ReduceError::InputTooLarge => ERR_INPUT_TOO_LARGE,
            ReduceError::InvalidJson => ERR_INVALID_JSON,
            ReduceError::GenerationExhausted => ERR_GENERATION_EXHAUSTED,
        }
    }
}

/// Successful result: a complete serialized JSON value.
type CallResult = Result<String, CallError>;

fn domain_result(result: Result<serde_json::Value, String>) -> String {
    match result {
        Ok(value) => serde_json::json!({"value":value,"error":""}).to_string(),
        Err(error) => serde_json::json!({"value":null,"error":error}).to_string(),
    }
}
fn dispatch(method: &str, input: &str) -> CallResult {
    match method {
        "version" => Ok(serde_json::json!({
            "abi": GAMELINK_ABI_VERSION,
            "crate": env!("CARGO_PKG_VERSION"),
        })
        .to_string()),
        "supervisor.reduce" => Ok(domain_result(
            gamelink_core::supervisor::reduce_json(input).and_then(|result| {
                serde_json::from_str(&result).map_err(|_| "cannot encode supervisor result".into())
            }),
        )),
        "launch.program" | "launch.uri" => {
            let value = serde_json::from_str(input).map_err(|_| ERR_INVALID_JSON)?;
            Ok(domain_result(gamelink_core::launch::dispatch(
                method, value,
            )))
        }
        "desktop.executable"
        | "desktop.settings_view"
        | "desktop.settings_prepare"
        | "desktop.preview"
        | "desktop.server_input"
        | "desktop.launch_plan"
        | "desktop.overview"
        | "desktop.join_parameters" => {
            let value = serde_json::from_str(input).map_err(|_| ERR_INVALID_JSON)?;
            Ok(domain_result(gamelink_core::desktop::dispatch(
                method, value,
            )))
        }
        "lifecycle.reduce" => Ok(gamelink_core::reduce_json(input)?),
        "client_lifecycle.reduce" => {
            let value = serde_json::from_str(input).map_err(|_| ERR_INVALID_JSON)?;
            Ok(domain_result(gamelink_core::client_lifecycle::reduce(
                value,
            )))
        }
        "security.port"
        | "security.slug"
        | "security.hostname"
        | "security.display"
        | "security.destination"
        | "security.public_ip"
        | "security.redact"
        | "security.redact_add"
        | "identity.public_key"
        | "games.all"
        | "games.valid_protocol"
        | "games.check"
        | "games.expand"
        | "games.placeholder"
        | "invite.decode"
        | "invite.encode"
        | "invite.fields"
        | "invite.address_format"
        | "invite.relay" => {
            let value = serde_json::from_str(input).map_err(|_| ERR_INVALID_JSON)?;
            Ok(domain_result(gamelink_core::pure::dispatch(method, value)))
        }
        "storage.ensure_dir"
        | "storage.write"
        | "storage.read"
        | "storage.seal"
        | "storage.unseal"
        | "storage.identity_load"
        | "storage.identity_save" => {
            let value = serde_json::from_str(input).map_err(|_| ERR_INVALID_JSON)?;
            Ok(domain_result(gamelink_core::storage::dispatch(
                method, value,
            )))
        }
        "config.bridge"
        | "config.default_settings"
        | "config.client_open"
        | "config.agent_load"
        | "config.agent_decode"
        | "config.save"
        | "config.client_mutate"
        | "config.game_prepare"
        | "config.new_id"
        | "config.keys_load"
        | "config.agent_mutate" => {
            let value = serde_json::from_str(input).map_err(|_| ERR_INVALID_JSON)?;
            Ok(domain_result(gamelink_core::config::dispatch(
                method, value,
            )))
        }
        "console.candidates_rewrite"
        | "console.candidates_map"
        | "console.signal_parse"
        | "console.signal_format"
        | "console.control_encode"
        | "console.control_decode"
        | "console.host_ip"
        | "console.host_config" => {
            let value = serde_json::from_str(input).map_err(|_| ERR_INVALID_JSON)?;
            Ok(domain_result(gamelink_core::console::dispatch(
                method, value,
            )))
        }
        "discovery.packet_encode"
        | "discovery.packet_decode"
        | "discovery.body_encode"
        | "discovery.body_decode"
        | "discovery.header_encode"
        | "discovery.header_decode"
        | "discovery.ad_encode"
        | "discovery.ad_decode" => {
            let value = serde_json::from_str(input).map_err(|_| ERR_INVALID_JSON)?;
            Ok(domain_result(gamelink_core::discovery::dispatch(
                method, value,
            )))
        }
        #[cfg(feature = "panic-probe")]
        "test.panic" => panic!("panic-probe"),
        _ => Err(ERR_UNKNOWN_METHOD),
    }
}

fn envelope(result: CallResult) -> Vec<u8> {
    match result {
        Ok(value) => {
            let mut out = String::with_capacity(value.len() + 7);
            out.push_str(r#"{"ok":"#);
            out.push_str(&value);
            out.push('}');
            out.into_bytes()
        }
        Err(error) => {
            #[derive(Serialize)]
            struct Envelope {
                error: CallError,
            }
            serde_json::to_vec(&Envelope { error })
                .unwrap_or_else(|_| INTERNAL_ENVELOPE.as_bytes().to_vec())
        }
    }
}

/// Borrow `len` bytes at `ptr`, treating a null pointer with length 0 as empty.
///
/// # Safety
/// If `ptr` is non-null it must be valid for reads of `len` bytes for `'a`.
unsafe fn borrow<'a>(ptr: *const u8, len: usize) -> Result<&'a [u8], CallError> {
    if ptr.is_null() {
        return if len == 0 {
            Ok(&[])
        } else {
            Err(ERR_INVALID_ARGUMENT)
        };
    }
    // SAFETY: caller guarantees `ptr` is valid for `len` bytes; callers bound
    // `len` well below `isize::MAX` before calling.
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
}

fn input_limit(method: &str) -> usize {
    match method {
        // Control JSON can expand sixfold; wire bytes use base64. Domain
        // codecs separately enforce their exact decoded protocol limits.
        "console.candidates_rewrite"
        | "console.candidates_map"
        | "console.signal_parse"
        | "console.signal_format"
        | "console.control_encode"
        | "console.control_decode"
        | "console.host_ip"
        | "console.host_config"
        | "discovery.packet_encode"
        | "discovery.packet_decode"
        | "discovery.body_encode"
        | "discovery.body_decode"
        | "discovery.header_encode"
        | "discovery.header_decode"
        | "discovery.ad_encode"
        | "discovery.ad_decode" => 1024 * 1024,
        "security.redact" => 256 * 1024,
        "storage.ensure_dir"
        | "storage.write"
        | "storage.read"
        | "storage.seal"
        | "storage.unseal"
        | "storage.identity_load"
        | "storage.identity_save"
        | "config.bridge"
        | "config.default_settings"
        | "config.client_open"
        | "config.agent_load"
        | "config.agent_decode"
        | "config.save"
        | "config.client_mutate"
        | "config.game_prepare"
        | "config.new_id"
        | "config.keys_load"
        | "config.agent_mutate"
        | "supervisor.reduce" => 8 * 1024 * 1024,
        _ => MAX_INPUT_BYTES,
    }
}

/// # Safety
/// See [`gamelink_call`].
unsafe fn call_with(
    dispatch: fn(&str, &str) -> CallResult,
    method: *const u8,
    method_len: usize,
    input: *const u8,
    input_len: usize,
) -> Vec<u8> {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // Bounds are checked before any byte is read or parsed.
        if method_len > GAMELINK_MAX_METHOD_BYTES {
            return envelope(Err(ERR_METHOD_TOO_LONG));
        }
        // Read only the already bounded method before deciding the input limit.
        let method_bytes = match unsafe { borrow(method, method_len) } {
            Ok(bytes) => bytes,
            Err(error) => return envelope(Err(error)),
        };
        let method_name = match std::str::from_utf8(method_bytes) {
            Ok(name) => name,
            Err(_) => return envelope(Err(ERR_INVALID_UTF8)),
        };
        if input_len > input_limit(method_name) {
            return envelope(Err(ERR_INPUT_TOO_LARGE));
        }
        // SAFETY: forwarded from the caller's contract.
        let borrowed = unsafe { (borrow(method, method_len), borrow(input, input_len)) };
        let (method, input) = match borrowed {
            (Ok(method), Ok(input)) => (method, input),
            (Err(error), _) | (_, Err(error)) => return envelope(Err(error)),
        };
        let (Ok(method), Ok(input)) = (std::str::from_utf8(method), std::str::from_utf8(input))
        else {
            return envelope(Err(ERR_INVALID_UTF8));
        };
        envelope(dispatch(method, input))
    }));
    result.unwrap_or_else(|_| INTERNAL_ENVELOPE.as_bytes().to_vec())
}

/// Call a GameLink core method.
///
/// `method` is a UTF-8 method name (at most 64 bytes) and `input` is UTF-8
/// JSON (normally at most 32 KiB; redaction accepts 256 KiB and explicit
/// config/storage operations accept 8 MiB). A null pointer with length 0 means empty. Always
/// returns an owned buffer holding a JSON envelope, `{"ok": <value>}` or
/// `{"error": {"code": "...", "message": "..."}}`, which must be released
/// with [`gamelink_buffer_free`]. Never unwinds.
///
/// # Safety
/// Each non-null pointer must be valid for reads of its length for the
/// duration of the call. The function does not retain either pointer.
#[no_mangle]
pub unsafe extern "C" fn gamelink_call(
    method: *const u8,
    method_len: usize,
    input: *const u8,
    input_len: usize,
) -> GamelinkBuffer {
    // SAFETY: forwarded from this function's contract.
    let bytes = unsafe { call_with(dispatch, method, method_len, input, input_len) };
    GamelinkBuffer::from_vec(bytes)
}

/// Release a buffer returned by [`gamelink_call`]. A zeroed buffer (null
/// `ptr`) is a no-op. Never unwinds.
///
/// # Safety
/// A non-null `buf` must come unmodified from [`gamelink_call`] and must not
/// be freed twice.
#[no_mangle]
pub unsafe extern "C" fn gamelink_buffer_free(buf: GamelinkBuffer) {
    if buf.ptr.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the buffer came from `GamelinkBuffer::from_vec` per contract.
        let mut bytes = unsafe { Vec::from_raw_parts(buf.ptr, buf.len, buf.cap) };
        bytes.zeroize();
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn call_raw(method: *const u8, method_len: usize, input: *const u8, input_len: usize) -> Value {
        // SAFETY: tests pass valid (or null with zero/checked length) pointers.
        let buf = unsafe { gamelink_call(method, method_len, input, input_len) };
        assert!(!buf.ptr.is_null());
        // SAFETY: buffer is owned and live until freed below.
        let bytes = unsafe { std::slice::from_raw_parts(buf.ptr, buf.len) }.to_vec();
        // SAFETY: freeing a buffer from gamelink_call once.
        unsafe { gamelink_buffer_free(buf) };
        serde_json::from_slice(&bytes).expect("envelope is JSON")
    }

    fn call(method: &[u8], input: &[u8]) -> Value {
        call_raw(method.as_ptr(), method.len(), input.as_ptr(), input.len())
    }

    fn error_code(envelope: &Value) -> &str {
        envelope["error"]["code"].as_str().expect("error envelope")
    }

    const START: &str = r#"{"state":{"phase":"idle","generation":0,"message":null,"name":null,"serverId":null},"event":{"type":"start","server":{"id":"world-1","name":"World","game":"minecraft-bedrock","bridgeVersion":1},"localIpv4":"192.168.1.25"}}"#;

    #[test]
    fn version_reports_abi_and_crate() {
        let got = call(b"version", b"");
        assert_eq!(got["ok"]["abi"], 1);
        assert_eq!(got["ok"]["crate"], env!("CARGO_PKG_VERSION"));
        let null_input = call_raw(b"version".as_ptr(), 7, std::ptr::null(), 0);
        assert_eq!(null_input, got);
    }

    #[test]
    fn lifecycle_reduce_round_trips() {
        let got = call(b"lifecycle.reduce", START.as_bytes());
        assert_eq!(got["ok"]["state"]["phase"], "connecting");
        assert_eq!(got["ok"]["state"]["generation"], 1);
        assert_eq!(got["ok"]["effects"][0]["type"], "start_transport");
        assert_eq!(got["ok"]["effects"][0]["server_id"], "world-1");
        let direct: Value =
            serde_json::from_str(&gamelink_core::reduce_json(START).unwrap()).unwrap();
        assert_eq!(got["ok"], direct);
    }

    #[test]
    fn invalid_reducer_input_is_invalid_json() {
        assert_eq!(error_code(&call(b"lifecycle.reduce", b"{")), "invalid_json");
        assert_eq!(error_code(&call(b"lifecycle.reduce", b"")), "invalid_json");
    }

    #[test]
    fn oversized_input_is_rejected_before_reading() {
        let big = vec![b' '; MAX_INPUT_BYTES + 1];
        assert_eq!(
            error_code(&call(b"lifecycle.reduce", &big)),
            "input_too_large"
        );
        // Length is checked before the pointer is read: a bogus pointer with
        // an oversized length must not be dereferenced.
        let bogus = std::ptr::NonNull::<u8>::dangling().as_ptr();
        let got = call_raw(b"version".as_ptr(), 7, bogus, usize::MAX);
        assert_eq!(error_code(&got), "input_too_large");
        // Exactly at the limit is accepted and parsed.
        let at_limit = vec![b' '; MAX_INPUT_BYTES];
        assert_eq!(
            error_code(&call(b"lifecycle.reduce", &at_limit)),
            "invalid_json"
        );
    }

    #[test]
    fn dedicated_limits_are_method_specific_and_checked_before_input_reads() {
        for (method, limit) in [
            ("version", 32 * 1024),
            ("security.redact", 256 * 1024),
            ("config.save", 8 * 1024 * 1024),
            ("supervisor.reduce", 8 * 1024 * 1024),
            ("supervisor.unknown", 32 * 1024),
            ("storage.write", 8 * 1024 * 1024),
            ("config.unknown", 32 * 1024),
            ("storage.unknown", 32 * 1024),
            ("console.control_encode", 1024 * 1024),
            ("discovery.packet_decode", 1024 * 1024),
            ("console.unknown", 32 * 1024),
            ("discovery.unknown", 32 * 1024),
        ] {
            assert_eq!(input_limit(method), limit);
            let bogus = std::ptr::NonNull::<u8>::dangling().as_ptr();
            assert_eq!(
                error_code(&call_raw(method.as_ptr(), method.len(), bogus, limit + 1)),
                "input_too_large"
            );
        }
        let redaction = vec![b' '; 256 * 1024];
        assert_eq!(
            error_code(&call(b"security.redact", &redaction)),
            "invalid_json"
        );
        assert_eq!(
            error_code(&call(b"invite.decode", &redaction)),
            "input_too_large"
        );
        let storage = vec![b' '; 8 * 1024 * 1024];
        assert_eq!(
            error_code(&call(b"storage.write", &storage)),
            "invalid_json"
        );
        assert_eq!(
            error_code(&call(b"security.redact", &storage)),
            "input_too_large"
        );
    }

    #[test]
    fn method_name_is_bounded() {
        let long = vec![b'a'; GAMELINK_MAX_METHOD_BYTES + 1];
        assert_eq!(error_code(&call(&long, b"")), "method_too_long");
        let at_limit = vec![b'a'; GAMELINK_MAX_METHOD_BYTES];
        assert_eq!(error_code(&call(&at_limit, b"")), "unknown_method");
    }

    #[test]
    fn null_pointers_are_safe() {
        let got = call_raw(std::ptr::null(), 0, std::ptr::null(), 0);
        assert_eq!(error_code(&got), "unknown_method");
        let got = call_raw(std::ptr::null(), 3, std::ptr::null(), 0);
        assert_eq!(error_code(&got), "invalid_argument");
        let got = call_raw(b"version".as_ptr(), 7, std::ptr::null(), 10);
        assert_eq!(error_code(&got), "invalid_argument");
    }

    #[test]
    fn non_utf8_is_rejected() {
        assert_eq!(error_code(&call(b"vers\xffion", b"")), "invalid_utf8");
        assert_eq!(
            error_code(&call(b"lifecycle.reduce", b"{\"a\":\"\xc3\x28\"}")),
            "invalid_utf8"
        );
    }

    #[test]
    fn unknown_method_is_rejected() {
        assert_eq!(error_code(&call(b"nope", b"{}")), "unknown_method");
        assert_eq!(error_code(&call(b"Version", b"")), "unknown_method");
    }

    #[test]
    fn error_messages_never_echo_input() {
        let secret = "tskey-SECRET-do-not-echo";
        let cases: Vec<(Vec<u8>, Vec<u8>)> = vec![
            (
                b"lifecycle.reduce".to_vec(),
                format!(r#"{{"{secret}":1"#).into_bytes(),
            ),
            (
                b"lifecycle.reduce".to_vec(),
                format!(r#"{{"state":"{secret}"}}"#).into_bytes(),
            ),
            (secret.as_bytes().to_vec(), secret.as_bytes().to_vec()),
            (
                b"lifecycle.reduce".to_vec(),
                [secret.as_bytes(), b"\xff"].concat(),
            ),
            (
                b"lifecycle.reduce".to_vec(),
                secret.repeat(2000).into_bytes(),
            ),
        ];
        for (method, input) in cases {
            let got = call(&method, &input);
            assert!(got.get("error").is_some(), "expected error: {got}");
            assert!(!got.to_string().contains("SECRET"), "echoed input: {got}");
        }
    }

    #[test]
    fn panics_become_internal_errors() {
        fn panicking(_: &str, _: &str) -> CallResult {
            panic!("boom");
        }
        // SAFETY: valid slices.
        let bytes = unsafe { call_with(panicking, b"x".as_ptr(), 1, b"".as_ptr(), 0) };
        let got: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(error_code(&got), "internal");
        assert_eq!(bytes, INTERNAL_ENVELOPE.as_bytes());
        assert_eq!(got["error"]["message"], "An internal error occurred.");
    }

    #[test]
    fn free_accepts_zeroed_buffer() {
        // SAFETY: a zeroed buffer is documented as a no-op.
        unsafe {
            gamelink_buffer_free(GamelinkBuffer {
                ptr: std::ptr::null_mut(),
                len: 0,
                cap: 0,
            })
        };
    }

    #[test]
    fn every_envelope_is_ok_xor_error() {
        for (method, input) in [
            (&b"version"[..], &b""[..]),
            (b"lifecycle.reduce", START.as_bytes()),
            (b"nope", b""),
        ] {
            let got = call(method, input);
            let object = got.as_object().unwrap();
            assert_eq!(object.len(), 1);
            assert!(object.contains_key("ok") ^ object.contains_key("error"));
        }
    }

    #[cfg(feature = "panic-probe")]
    #[test]
    fn panic_probe_is_contained() {
        assert_eq!(error_code(&call(b"test.panic", b"")), "internal");
        assert!(call(b"version", b"").get("ok").is_some());
    }
}
