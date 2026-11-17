use serde_json::Value;
#[repr(C)]
struct Buffer {
    ptr: *mut u8,
    len: usize,
}
extern "C" {
    #[cfg(target_env = "musl")]
    fn gamelink_agent_runtime_start();
    fn gamelink_agent_call(ptr: *const u8, len: usize) -> Buffer;
    fn gamelink_agent_free(buf: Buffer);
}
// Test seam: a test may install a per-thread stand-in for the Go archive, so the
// serialized effect loop can be driven without real networking. Compiled out of
// production builds.
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
    pub(super) fn intercept(input: &Value) -> Option<Result<Value, String>> {
        HANDLER.with(|h| h.borrow_mut().as_mut().map(|handler| handler(input)))
    }
}

pub fn call(input: Value) -> Result<Value, String> {
    #[cfg(test)]
    if let Some(result) = fake::intercept(&input) {
        return result;
    }
    #[cfg(target_env = "musl")]
    unsafe {
        gamelink_agent_runtime_start();
    }
    let bytes = serde_json::to_vec(&input).map_err(|e| e.to_string())?;
    if bytes.len() > 32 * 1024 {
        return Err("agent request exceeds size limit".into());
    }
    // The archive copies inputs before returning. Output is C malloc memory,
    // never a Go pointer; this owner frees and clears it exactly once.
    let buf = unsafe { gamelink_agent_call(bytes.as_ptr(), bytes.len()) };
    struct Owned(Buffer);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                gamelink_agent_free(Buffer {
                    ptr: self.0.ptr,
                    len: self.0.len,
                })
            };
        }
    }
    let owned = Owned(buf);
    if owned.0.ptr.is_null() || owned.0.len > 4 * 1024 * 1024 {
        return Err("invalid agent response".into());
    }
    let output: Value =
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(owned.0.ptr, owned.0.len) })
            .map_err(|_| "invalid agent response")?;
    if let Some(error) = output["error"].as_str() {
        Err(error.into())
    } else {
        Ok(output["ok"].clone())
    }
}
// Go's c-archive requires foreign signal handlers to use an alternate stack,
// including when the process delivers SIGINT/SIGTERM on a Go runtime thread.
// ctrlc installs the handler; retain it and add the flag Go requires.
pub fn enable_signal_stack() -> Result<(), String> {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
        if unsafe { libc::sigaction(signal, std::ptr::null(), &mut action) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        action.sa_flags |= libc::SA_ONSTACK;
        if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}
pub struct Host;
impl Host {
    pub fn init() -> Result<Self, String> {
        call(serde_json::json!({"method":"init"}))?;
        Ok(Self)
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = call(serde_json::json!({"method":"shutdown"}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_c_abi_rejects_invalid_size_and_returns_owned_memory() {
        #[cfg(target_env = "musl")]
        unsafe {
            gamelink_agent_runtime_start();
        }
        for length in [1, 32 * 1024 + 1] {
            let buffer = unsafe { gamelink_agent_call(std::ptr::null(), length) };
            assert!(!buffer.ptr.is_null());
            let result: Value = serde_json::from_slice(unsafe {
                std::slice::from_raw_parts(buffer.ptr, buffer.len)
            })
            .unwrap();
            assert_eq!(result["error"], "invalid agent request size");
            unsafe {
                gamelink_agent_free(buffer);
            }
        }
        assert!(call(serde_json::json!({"method":"init","padding":"x".repeat(32768)})).is_err());
        let host = Host::init().unwrap();
        assert!(Host::init().is_err());
        assert!(call(serde_json::json!({"method":"generate"})).is_ok());
        drop(host);
        assert!(call(serde_json::json!({"method":"generate"})).is_err());
    }
}
