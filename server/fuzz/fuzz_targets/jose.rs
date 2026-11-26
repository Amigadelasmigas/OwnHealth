//! jose.rs, the JWE and JWS of every SMART Health Link and Card the instance receives. Whatever the
//! bytes: no panic, no payload over its cap, a JWE opens only as `dir` + `A256GCM`, and what
//! `encrypt` seals opens again to the same bytes.
#![no_main]

use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;
use ownhealth_core::jose;
use p256::ecdsa::VerifyingKey;

/// A small cap, so a DEFLATE bomb past it is quick to find.
const MAX: usize = 4096;
const KEY: [u8; jose::KEY_LEN] = [7; jose::KEY_LEN];
/// The generator of P-256, a valid point: signatures over it can be checked, never forged.
static ISSUER: LazyLock<VerifyingKey> = LazyLock::new(|| {
    jose::es256_key(&serde_json::json!({
        "kty": "EC",
        "crv": "P-256",
        "x": "axfR8uEsQkf4vOblY6RA8ncDfYEt6zOg9KE5RdiYwpY",
        "y": "T-NC4v4af5uO5-tKfA-eFivOM1drMV7Oy7ZAaDe_UfU",
    }))
    .expect("the P-256 generator is a valid key")
});

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(opened) = jose::decrypt(&KEY, text, MAX) {
            assert!(opened.plaintext.len() <= MAX, "a JWE plaintext over its cap");
            assert_eq!(opened.header.get("alg").and_then(|v| v.as_str()), Some("dir"));
            assert_eq!(opened.header.get("enc").and_then(|v| v.as_str()), Some("A256GCM"));
        }
        if let Ok((_, payload)) = jose::jws_unverified(text, MAX) {
            assert!(payload.len() <= MAX, "a JWS payload over its cap");
        }
        if let Ok(verified) = jose::verify_es256(text, &ISSUER, MAX) {
            assert!(verified.payload.len() <= MAX, "a verified JWS payload over its cap");
        }
        let _ = jose::jws_header(text);
        if let Ok(jwk) = serde_json::from_str(text) {
            let _ = jose::es256_key(&jwk);
        }
    }
    // what encrypt seals opens with the same key, deflated or not
    let deflate = data.first().is_some_and(|b| b & 1 == 1);
    let sealed = jose::encrypt(&KEY, data, Some("application/fhir+json"), deflate).expect("seals");
    let opened = jose::decrypt(&KEY, &sealed, data.len()).expect("opens what it sealed");
    assert_eq!(opened.plaintext, data);
});
