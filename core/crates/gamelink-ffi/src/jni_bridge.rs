//! Android JNI entrypoint. Behavior is unchanged from the original
//! `gamelink-core` cdylib; only the library name moved to `gamelink_ffi`.

use gamelink_core::reduce_json;

const JNI_ERROR: &str = r#"{"state":{"phase":"error","generation":0,"message":"GameLink could not read the bridge state.","name":null,"serverId":null},"effects":[]}"#;

#[no_mangle]
pub extern "system" fn Java_io_gamelink_app_Core_nativeReduce(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    input: jni::objects::JString,
) -> jni::sys::jstring {
    let output = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let input: String = env.get_string(&input).ok()?.into();
        Some(reduce_json(&input).unwrap_or_else(|_| JNI_ERROR.to_owned()))
    }))
    .ok()
    .flatten()
    .unwrap_or_else(|| JNI_ERROR.to_owned());
    env.new_string(output)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
