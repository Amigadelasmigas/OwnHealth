//! scripts/cite-links.sh checks the links of the connector pages (docs/connectors) as well as the cited
//! data files. These tests run the script itself, with no network: `--list` prints the links a run would
//! fetch, and a page whose link a fake `curl` answers as a host that does not resolve must make a run
//! fail.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Run the script. `bin`, when given, goes first on its PATH, so a fake `curl` there answers instead of
/// the real one.
fn run(args: &[&str], bin: Option<&Path>) -> Output {
    let mut cmd = Command::new("bash");
    if let Some(bin) = bin {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![bin.to_path_buf()];
        dirs.extend(std::env::split_paths(&path));
        cmd.env("PATH", std::env::join_paths(dirs).expect("PATH"));
    }
    cmd.arg(repo().join("scripts/cite-links.sh"))
        .args(args)
        .output()
        .expect("bash runs scripts/cite-links.sh")
}

/// The https links of one text, read as the script reads them (`grep -o`, left to right): a link ends
/// at white space, a quote, a backtick or one of `, } < > |`, and loses a trailing `.` or `)`.
fn links(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("https://") {
        let tail = &rest[i..];
        let end = tail
            .find(|c: char| c.is_whitespace() || "\"'`,}<>|".contains(c))
            .unwrap_or(tail.len());
        out.push(tail[..end].trim_end_matches(['.', ')']).to_string());
        rest = &tail[end..];
    }
    out
}

/// The links of every page under docs/connectors.
fn page_links() -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(repo().join("docs/connectors")).expect("docs/connectors") {
        let path = e.expect("entry").path();
        if path.extension().is_some_and(|x| x == "md") {
            out.extend(links(&std::fs::read_to_string(&path).expect("page")));
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn the_link_reader_stops_where_the_script_stops() {
    let text = "See <https://a.example/x>, [b](https://b.example/y). `https://c.example/z` \
                https://web.archive.org/web/1/https://d.example/w.pdf\n";
    assert_eq!(
        links(text),
        [
            "https://a.example/x",
            "https://b.example/y",
            "https://c.example/z",
            "https://web.archive.org/web/1/https://d.example/w.pdf"
        ]
    );
}

#[test]
fn a_default_run_checks_every_link_of_the_connector_pages() {
    let out = run(&["--list"], None);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let listed = String::from_utf8_lossy(&out.stdout);
    let listed: Vec<&str> = listed.lines().collect();
    let pages = page_links();
    // 14 vendor pages, each with at least one source
    assert!(pages.len() >= 14, "every connector page cites its source: {pages:?}");
    for url in &pages {
        assert!(listed.contains(&url.as_str()), "{url} is not checked");
    }
    // the cited data files stay in the run
    assert!(listed.contains(&"https://pubmed.ncbi.nlm.nih.gov/34120177/"));
    assert!(listed.len() > pages.len() + 100, "{} links", listed.len());
}

#[test]
fn a_dead_link_in_a_connector_page_fails_the_run() {
    let tree = tempfile::tempdir().expect("tempdir");
    let dir = tree.path().join("docs/connectors");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("acme.md"),
        "# Acme\n\n> The vendor's words.\n\nSource: <https://terms.cite-links-test.invalid/api>\nFetched: 2026-11-25\n",
    )
    .expect("page");
    // A fake curl answers as curl does for a host that does not resolve (exit 6), and writes down the
    // link it was given, so the test asks no resolver and reaches no network.
    let bin = tree.path().join("bin");
    let asked = tree.path().join("asked");
    std::fs::create_dir_all(&bin).expect("mkdir bin");
    std::fs::write(
        bin.join("curl"),
        format!(
            "#!/bin/sh\nfor a; do last=$a; done\nprintf '%s\\n' \"$last\" >>'{}'\nexit 6\n",
            asked.display()
        ),
    )
    .expect("fake curl");
    std::fs::set_permissions(bin.join("curl"), std::fs::Permissions::from_mode(0o755)).expect("chmod fake curl");
    let out = run(&[dir.to_str().expect("utf-8 path")], Some(&bin));
    assert_eq!(
        std::fs::read_to_string(&asked).unwrap_or_default(),
        "https://terms.cite-links-test.invalid/api\n",
        "the fake curl, not a real one, was asked for the link"
    );
    let report = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{report}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        report.contains("1 links under") && report.contains("1 dead"),
        "{report}"
    );
    assert!(
        report.contains("| https://terms.cite-links-test.invalid/api |"),
        "{report}"
    );
    assert!(report.contains("acme.md |"), "{report}");
}
