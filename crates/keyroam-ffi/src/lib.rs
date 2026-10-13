//! C ABI for the KeyRoam Flutter apps.
//!
//! Two functions form the whole boundary:
//!
//! * `keyroam_call(request)` takes a NUL-terminated UTF-8 JSON request and
//!   returns a newly allocated JSON response (see [`api`]).
//! * `keyroam_free_string(response)` wipes and frees a response.
//!
//! Calls block (Argon2, network), so Dart invokes them from background isolates.

pub mod api;

use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::OnceLock;

use zeroize::Zeroize;

static ENGINE: OnceLock<api::Engine> = OnceLock::new();

fn engine() -> &'static api::Engine {
    ENGINE.get_or_init(api::Engine::default)
}

fn error_json(code: &str, message: &str) -> String {
    serde_json::json!({ "error": { "code": code, "message": message } }).to_string()
}

/// Handle one JSON request.
///
/// # Safety
/// `request` must be null or point to a NUL-terminated string that stays valid
/// for the duration of the call. The returned pointer must be released with
/// [`keyroam_free_string`].
#[no_mangle]
pub unsafe extern "C" fn keyroam_call(request: *const c_char) -> *mut c_char {
    let response = if request.is_null() {
        error_json("invalid_request", "null request")
    } else {
        // SAFETY: caller guarantees a valid NUL-terminated string.
        let req = unsafe { CStr::from_ptr(request) };
        match req.to_str() {
            Ok(s) => catch_unwind(AssertUnwindSafe(|| engine().handle_json(s)))
                .unwrap_or_else(|_| error_json("internal", "internal error")),
            Err(_) => error_json("invalid_request", "request is not UTF-8"),
        }
    };
    // JSON output never contains interior NULs (they are escaped).
    CString::new(response)
        .unwrap_or_else(|_| CString::new(error_json("internal", "bad response")).unwrap())
        .into_raw()
}

/// Wipe and free a string returned by [`keyroam_call`].
///
/// # Safety
/// `s` must be null or a pointer obtained from [`keyroam_call`] that has not
/// been freed yet.
#[no_mangle]
pub unsafe extern "C" fn keyroam_free_string(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    // SAFETY: caller guarantees the pointer came from CString::into_raw.
    let mut bytes = unsafe { CString::from_raw(s) }.into_bytes();
    bytes.zeroize();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_abi_roundtrip() {
        let req = CString::new(r#"{"method":"generate_password","params":{"length":16}}"#).unwrap();
        unsafe {
            let out = keyroam_call(req.as_ptr());
            let s = CStr::from_ptr(out).to_str().unwrap().to_string();
            keyroam_free_string(out);
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v["ok"].as_str().unwrap().chars().count(), 16);

            let out = keyroam_call(std::ptr::null());
            let s = CStr::from_ptr(out).to_str().unwrap().to_string();
            keyroam_free_string(out);
            assert!(s.contains("invalid_request"));
            keyroam_free_string(std::ptr::null_mut());
        }
    }
}
