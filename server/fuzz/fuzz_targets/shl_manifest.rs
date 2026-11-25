//! The SMART Health Link receiver (ownhealth-ingest-structured shl_receive.rs): the link a person
//! pastes and the manifest its server answers with, both written by someone else. Whatever the
//! bytes: no panic, and what is accepted keeps the rules the importer relies on.
#![no_main]

use chrono::DateTime;
use libfuzzer_sys::fuzz_target;
use ownhealth_ingest_structured::shl_receive;

fuzz_target!(|data: &[u8]| {
    let now = DateTime::from_timestamp(1_800_000_000, 0).expect("a date");
    if let Ok(text) = std::str::from_utf8(data)
        && let Ok(link) = shl_receive::parse(text, now)
    {
        assert!(!link.host.is_empty(), "a link with no host");
        assert!(!(link.needs_passcode() && link.direct()), "a link with both U and P");
        assert!(link.exp.is_none_or(|exp| exp >= now), "an expired link");
        assert!(
            link.label
                .as_ref()
                .is_none_or(|l| l.chars().count() <= 80 && !l.chars().any(char::is_control)),
            "a label over 80 characters or with a control character"
        );
    }
    if let Ok(files) = shl_receive::manifest_files(data) {
        assert!(
            (1..=shl_receive::MAX_FILES).contains(&files.len()),
            "a manifest of {} files",
            files.len()
        );
    }
});
