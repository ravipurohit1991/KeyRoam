//! JNI entry point for the Android app's Kotlin code: the autofill service,
//! the Credential Manager provider, background sync, biometric unlock and
//! the sign-in redirect. It serves the same JSON requests as
//! [`crate::keyroam_call`] from the same process-wide engine, so the Flutter
//! UI and the system services share one session and one unlocked vault.

use std::panic::{catch_unwind, AssertUnwindSafe};

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;
use zeroize::Zeroize;

/// `io.github.ravipurohit1991.keyroam.KeyRoamEngine.nativeCall(String): String`
#[no_mangle]
pub extern "system" fn Java_io_github_ravipurohit1991_keyroam_KeyRoamEngine_nativeCall<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    request: JString<'local>,
) -> jstring {
    let mut response = match env.get_string(&request) {
        Ok(java) => {
            let mut request: String = java.into();
            let response = catch_unwind(AssertUnwindSafe(|| crate::engine().handle_json(&request)))
                .unwrap_or_else(|_| crate::error_json("internal", "internal error"));
            request.zeroize();
            response
        }
        Err(_) => crate::error_json("invalid_request", "request is not a string"),
    };
    let out = env
        .new_string(&response)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut());
    response.zeroize();
    out
}
