use serde_json::{json, Value};

pub const MAX_INPUT: usize = gamelink_core::MAX_INPUT_BYTES;
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
#[repr(C)]
struct Buffer {
    ptr: *mut u8,
    len: usize,
}
unsafe extern "C" {
    fn gamelink_desktop_call(ptr: *const u8, len: usize) -> Buffer;
    fn gamelink_desktop_free(buffer: Buffer);
}
struct Owned(Buffer);
impl Drop for Owned {
    fn drop(&mut self) {
        // The Go ABI allocates C memory; this is its sole release path.
        unsafe {
            gamelink_desktop_free(Buffer {
                ptr: self.0.ptr,
                len: self.0.len,
            })
        }
    }
}
// Test seam: a test may install a per-thread stand-in for the Go archive, so the
// serialized application host can be driven without real networking. Compiled
// out of production builds.
#[cfg(test)]
pub(crate) mod fake {
    use serde_json::Value;
    use std::cell::RefCell;
    pub type Handler = Box<dyn FnMut(&Value) -> Result<Value, String>>;
    thread_local! {
        static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
    }
    pub struct Installed;
    impl Drop for Installed {
        fn drop(&mut self) {
            HANDLER.with(|h| *h.borrow_mut() = None);
        }
    }
    pub fn install(handler: impl FnMut(&Value) -> Result<Value, String> + 'static) -> Installed {
        HANDLER.with(|h| *h.borrow_mut() = Some(Box::new(handler)));
        Installed
    }
    pub(super) fn intercept(request: &Value) -> Option<Result<Value, String>> {
        HANDLER.with(|h| h.borrow_mut().as_mut().map(|handler| handler(request)))
    }
}
pub fn call(request: Value) -> Result<Value, String> {
    #[cfg(test)]
    if let Some(result) = fake::intercept(&request) {
        return result;
    }
    let input = serde_json::to_vec(&request).map_err(|_| "invalid desktop request")?;
    if input.len() > MAX_INPUT {
        return Err("desktop request exceeds size limit".into());
    }
    // Go copies the borrowed input before returning. No callback or Go pointer
    // escapes this call. The result is copied before its allocator frees it.
    let out = Owned(unsafe { gamelink_desktop_call(input.as_ptr(), input.len()) });
    if out.0.ptr.is_null() || out.0.len > MAX_OUTPUT {
        return Err("invalid desktop response".into());
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.0.ptr, out.0.len) };
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "invalid desktop response")?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(error.to_owned());
    }
    value
        .get("ok")
        .cloned()
        .ok_or("invalid desktop response".into())
}
pub struct Session;
impl Session {
    pub fn open() -> Result<Self, String> {
        crate::host::call(json!({"method":"init"}))?;
        Ok(Self)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = crate::host::call(json!({"method":"shutdown"}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_and_owned_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("GAMELINK_HOME", dir.path());
        assert!(crate::host::call(json!({"method":"version"})).is_err());
        let session = Session::open().unwrap();
        assert!(Session::open().is_err());
        assert_eq!(
            crate::host::call(json!({"method":"version"})).unwrap()["app"],
            "GameLink"
        );
        assert!(crate::host::call(json!({"method":"identity"}))
            .unwrap()
            .get("privateKey")
            .is_none());
        assert!(crate::host::call(json!({"method":"x", "text":"x".repeat(MAX_INPUT)})).is_err());
        assert!(crate::host::call(json!({"method":"unknown"})).is_err());
        for _ in 0..100 {
            crate::host::call(json!({"method":"settings"})).unwrap();
        }
        let mut settings = crate::host::call(json!({"method":"settings"})).unwrap();
        settings["demoMode"] = json!(true);
        settings["checkOnStartup"] = json!(false);
        crate::host::call(json!({"method":"updateSettings","settings":settings})).unwrap();
        let servers = crate::host::call(json!({"method":"servers"})).unwrap();
        assert_eq!(servers.as_array().unwrap().len(), 3);
        crate::host::call(json!({"method":"join","id":"demo-mc"})).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(950));
        let servers = crate::host::call(json!({"method":"servers"})).unwrap();
        assert_eq!(servers[0]["connection"]["state"], "connected");
        assert_eq!(servers[0]["connection"]["localAddr"], "127.0.0.1:25565");
        crate::host::call(json!({"method":"disconnect","id":"demo-mc"})).unwrap();
        let servers = crate::host::call(json!({"method":"servers"})).unwrap();
        assert_eq!(servers[0]["connection"]["state"], "disconnected");
        assert!(crate::host::call(
            json!({"method":"updateSettings","settings":{"demoMode":"yes"}})
        )
        .is_err());
        assert!(crate::host::call(json!({"method":"join","id":"demo-invalid"})).is_err());
        drop(session);
        assert!(crate::host::call(json!({"method":"version"})).is_err());
        let reopened = Session::open().unwrap();
        assert_eq!(
            crate::host::call(json!({"method":"settings"})).unwrap()["demoMode"],
            true
        );
        drop(reopened);
    }
}
