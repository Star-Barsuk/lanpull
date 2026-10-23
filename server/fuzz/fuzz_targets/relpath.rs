#![no_main]

//! Manifest-relative path validation, as used on request paths and on every
//! file the walker emits.

use lanpull_core::relpath;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = relpath::validate(text);
    let _ = relpath::is_reserved(text);
});
