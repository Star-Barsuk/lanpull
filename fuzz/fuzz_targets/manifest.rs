#![no_main]

//! The manifest deserializer the server uses when it loads a manifest from
//! disk. The client has its own validator; this target covers the server side.

use lanpull_store::manifest::Manifest;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(manifest) = serde_json::from_slice::<Manifest>(data) {
        let _ = manifest.files.len();
        let _ = manifest.scheme.len();
        let _ = manifest.generated_at.len();
    }
});
