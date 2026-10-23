#![no_main]

//! The share-relative glob engine, as used by the access policy, the ignore
//! rules, and path validation.

use lanpull_core::glob::Glob;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(glob) = Glob::parse(text) {
        let _ = glob.covers(text);
        let _ = glob.covers("a/b/c.pdf");
        let _ = glob.covers("_lanpull/x");
    }
});
