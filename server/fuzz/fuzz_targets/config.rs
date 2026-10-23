#![no_main]

//! The `KEY=VALUE` configuration parser, as used on operator input.

use lanpull_core::config::{parse_kv, unknown_keys_in, Config};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let map = parse_kv(text);
    let _ = unknown_keys_in(&map);
    let _ = Config::from_map(&map);
});
