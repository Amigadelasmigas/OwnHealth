//! The keyring loader (ownhealth-core keyfile.rs). keyring.json comes back from an off-box copy at
//! restore, so its bytes are not ours to trust. Whatever the bytes: no panic, and a keyring it
//! accepts writes back as the same keyring.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ownhealth_core::keyfile;

fuzz_target!(|data: &[u8]| {
    if let Ok(ring) = keyfile::decode(data) {
        let again = keyfile::decode(keyfile::encode(&ring).as_bytes()).expect("a keyring we write reads back");
        assert_eq!(again, ring);
        for id in ring.shredded.iter().chain(ring.profiles.keys()) {
            let _ = ring.closes(*id, None);
        }
    }
});
