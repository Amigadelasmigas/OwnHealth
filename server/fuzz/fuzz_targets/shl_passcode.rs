//! The public passcode route, POST /api/shl/{id} (ownhealth-api routes/shl.rs): the manifest id and
//! the request body, from anyone on the internet, read before any database row. The input is the id,
//! a line break, then the body. Whatever the bytes: no panic, and the recipient the access log keeps
//! is clean text.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ownhealth_ops::shl;

fuzz_target!(|data: &[u8]| {
    let split = data.iter().position(|&b| b == b'\n').unwrap_or(data.len());
    let (id, body) = data.split_at(split);
    if let Ok(id) = std::str::from_utf8(id)
        && shl::is_segment(id)
    {
        assert_eq!(id.len(), 43);
    }
    let asked = shl::ManifestRequest::read(body);
    if let Some(r) = &asked.recipient {
        assert!(!r.is_empty(), "an empty recipient");
        assert!(r.chars().count() <= shl::RECIPIENT_MAX, "a recipient over its cap");
        assert!(!r.chars().any(char::is_control), "a control character in a recipient");
        assert!(!r.starts_with(char::is_whitespace), "an untrimmed recipient");
    }
    assert!(asked.passcode.as_deref() != Some(""), "an empty passcode");
});
