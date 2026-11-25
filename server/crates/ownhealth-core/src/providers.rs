//! The provider policy (`data/connectors/providers.yaml`): per wearable provider
//! (`wearable_source.provider`), whether its samples are stored and whether MCP, the rule-pack markers
//! and the calculators may read them.
//!
//! The shared readers apply it: `ownhealth_rules::engine::wearables` (markers and calculators),
//! `ownhealth_ingest_structured::wearables::points` read as [`Reader::Shared`] (MCP rollups) and the
//! MCP freshness date. Only the Wearables page and its REST routes read every provider.
//! The device sample store checks [`require_ingest`] before writing a batch. Wearable source
//! registration checks it too, including for Apple Health's bulk importer. A provider the file
//! omits is withheld from shared readers and refused at ingest.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const SHIPPED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../data/connectors/providers.yaml"
));

/// Whether a provider's samples may be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ingest {
    Allow,
    Drop,
}

/// Whether agents and the shared readers see a provider's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mcp {
    Served,
    Withheld,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub ingest: Ingest,
    pub mcp: Mcp,
    /// How the data arrives; documentation only.
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub version: u32,
    pub providers: BTreeMap<String, Provider>,
}

/// Who reads wearable data. The Wearables page and its REST routes read every provider; every other
/// reader (MCP, the rule-pack markers, the calculators) only the providers the policy serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    WearablesPage,
    Shared,
}

impl Reader {
    /// Whether this reader may see `provider`'s data.
    pub fn sees(self, provider: &str) -> bool {
        self == Reader::WearablesPage || served(provider)
    }
}

/// Parse a policy file. A provider key follows the `wearable_source.provider` form: 1 to 40 lowercase
/// letters, digits or `_`.
pub fn parse(text: &str) -> Result<Policy, String> {
    let p: Policy = serde_yaml::from_str(text).map_err(|e| format!("providers.yaml: {e}"))?;
    if let Some(bad) = p.providers.keys().find(|k| {
        k.is_empty()
            || k.len() > 40
            || !k
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    }) {
        return Err(format!("providers.yaml: {bad:?} is not a provider key"));
    }
    Ok(p)
}

/// The shipped policy. The file is compiled in and its test parses it, so it cannot fail at run time;
/// if it ever did, every provider would be withheld and refused at ingest.
pub fn policy() -> &'static Policy {
    static P: OnceLock<Policy> = OnceLock::new();
    P.get_or_init(|| {
        parse(SHIPPED).unwrap_or_else(|e| {
            tracing::error!("{e}: every wearable provider is withheld");
            Policy {
                version: 0,
                providers: BTreeMap::new(),
            }
        })
    })
}

/// Whether MCP, the markers and the calculators may read `provider`'s data. Unknown: no.
pub fn served(provider: &str) -> bool {
    policy().providers.get(provider).is_some_and(|p| p.mcp == Mcp::Served)
}

/// Whether `provider`'s samples may be stored. Unknown: no.
pub fn ingest_allowed(provider: &str) -> bool {
    policy()
        .providers
        .get(provider)
        .is_some_and(|p| p.ingest == Ingest::Allow)
}

/// Reject a provider that the policy omits or marks as `ingest: drop`.
pub fn require_ingest(provider: &str) -> crate::Result<()> {
    if !ingest_allowed(provider) {
        return Err(crate::Error::WearableIngestDenied {
            provider: provider.into(),
        });
    }
    Ok(())
}

/// Every provider the policy serves, for a SQL filter (`provider = any($n)`).
pub fn served_providers() -> Vec<String> {
    policy()
        .providers
        .iter()
        .filter(|(_, p)| p.mcp == Mcp::Served)
        .map(|(k, _)| k.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_file_parses() {
        let p = parse(SHIPPED).expect("providers.yaml");
        assert_eq!(p.version, 1);
        assert_eq!(policy().providers.len(), p.providers.len());
    }

    #[test]
    fn the_vendors_whose_terms_ban_ai_or_are_not_public_are_withheld() {
        // Oura and WHOOP ban giving their API data to AI; Dexcom's data licence is shown only at
        // registration (docs/connectors/oura.md, whoop.md, dexcom.md)
        for p in ["oura", "whoop", "dexcom"] {
            assert!(!served(p), "{p}");
            assert!(ingest_allowed(p), "{p}");
            assert!(Reader::WearablesPage.sees(p) && !Reader::Shared.sees(p), "{p}");
        }
    }

    #[test]
    fn polar_and_withings_are_served_because_their_quoted_terms_allow_it() {
        // docs/connectors/polar.md and withings.md quote the terms
        for p in ["polar", "withings"] {
            assert!(served(p) && ingest_allowed(p), "{p}");
            assert!(Reader::Shared.sees(p), "{p}");
            assert!(served_providers().contains(&p.to_string()), "{p}");
        }
    }

    /// Providers whose data are the person's own files or pushes (account exports, their own
    /// Nightscout, an app on their own phone). No vendor API terms reach them (docs/design/connectors.md
    /// section 4), so they need no connector page. Every other provider marked served needs
    /// docs/connectors/<provider>.md with the vendor's terms quoted.
    const OWN_FILES: [&str; 10] = [
        "apple_health",
        "health_auto_export",
        "gadgetbridge",
        "nightscout",
        "garmin_export",
        "fitbit_takeout",
        "fit",
        "strava_export",
        "libreview",
        "dexcom_clarity",
    ];

    /// The US record vendors of data/connectors/connectors.yaml. Their records join the person's record,
    /// which agents read; providers.yaml does not name them, and each has a page that says `records`.
    fn record_vendors() -> Vec<String> {
        let profiles = crate::connector_profiles::profiles().expect("connectors.yaml");
        profiles.vendors.iter().map(|v| v.key.clone()).collect()
    }

    /// The connector pages (docs/connectors/*.md, the README index aside), by file stem. No folder is
    /// no pages, so the test lists every page that is missing.
    fn pages() -> BTreeMap<String, String> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/connectors");
        let mut out = BTreeMap::new();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return out;
        };
        for e in entries {
            let path = e.expect("entry").path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().is_some_and(|x| x == "md") && stem != "README" {
                out.insert(stem.to_string(), std::fs::read_to_string(&path).expect("page"));
            }
        }
        out
    }

    /// The policy word on a page's only `**MCP policy:**` line: served, withheld or records.
    fn stated(text: &str) -> Option<&str> {
        let mut lines = text.lines().filter_map(|l| l.strip_prefix("**MCP policy:**"));
        let word = lines
            .next()?
            .split_whitespace()
            .next()?
            .trim_end_matches(['.', ',', ':']);
        if lines.next().is_some() { None } else { Some(word) }
    }

    /// What a connector page lacks. Each quote (a run of `>` lines) is followed, before the next quote
    /// or heading, by `Source: <https://...>` and `Fetched: YYYY-MM-DD`. A page has at least one quote
    /// and one `**MCP policy:**` line, and a withheld page names the clause on a `**Withheld by:**` line.
    fn page_problems(name: &str, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let policy = stated(text);
        if !matches!(policy, Some("served" | "withheld" | "records")) {
            out.push(format!(
                "{name}: no single **MCP policy:** line saying served, withheld or records"
            ));
        }
        if policy == Some("withheld")
            && !text
                .lines()
                .any(|l| l.strip_prefix("**Withheld by:**").is_some_and(|r| r.trim().len() >= 10))
        {
            out.push(format!(
                "{name}: withheld, but no **Withheld by:** line names the clause"
            ));
        }
        let lines: Vec<&str> = text.lines().collect();
        let mut quotes = 0;
        let mut i = 0;
        while i < lines.len() {
            if !lines[i].starts_with('>') {
                i += 1;
                continue;
            }
            let mut quote = String::new();
            while i < lines.len() && lines[i].starts_with('>') {
                quote.push_str(lines[i].trim_start_matches('>').trim());
                i += 1;
            }
            quotes += 1;
            let tail: Vec<&str> = lines[i..]
                .iter()
                .take_while(|l| !l.starts_with('>') && !l.starts_with('#'))
                .copied()
                .collect();
            let source = tail
                .iter()
                .any(|l| l.strip_prefix("Source: <https://").is_some_and(|r| r.contains('>')));
            let fetched = tail.iter().any(|l| {
                l.strip_prefix("Fetched: ")
                    .and_then(|r| r.get(..10))
                    .is_some_and(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
            });
            if quote.len() < 20 {
                out.push(format!(
                    "{name}: quote {quotes} is empty or too short to be the vendor's words"
                ));
            }
            if !source {
                out.push(format!("{name}: quote {quotes} has no Source: <https://...> line"));
            }
            if !fetched {
                out.push(format!("{name}: quote {quotes} has no Fetched: YYYY-MM-DD line"));
            }
        }
        if quotes == 0 {
            out.push(format!("{name}: no quote of the vendor's terms"));
        }
        out
    }

    /// Every way the policy file, the record vendors and the pages disagree: a served provider (other
    /// than the person's own files) or a record vendor with no page, a page whose policy line differs
    /// from the files, a page that names neither, and every page problem.
    fn policy_problems(policy: &Policy, records: &[String], pages: &BTreeMap<String, String>) -> Vec<String> {
        let mut out = Vec::new();
        for (key, p) in &policy.providers {
            let want = match p.mcp {
                Mcp::Served => "served",
                Mcp::Withheld => "withheld",
            };
            match pages.get(key) {
                None if p.mcp == Mcp::Served && !OWN_FILES.contains(&key.as_str()) => {
                    out.push(format!("{key}: served, but docs/connectors/{key}.md does not exist"))
                }
                Some(text) if stated(text) != Some(want) => out.push(format!(
                    "{key}: providers.yaml says {want}, its page says {:?}",
                    stated(text)
                )),
                _ => {}
            }
        }
        for key in records {
            match pages.get(key) {
                None => out.push(format!(
                    "{key}: a record vendor, but docs/connectors/{key}.md does not exist"
                )),
                Some(text) if stated(text) != Some("records") => out.push(format!(
                    "{key}: connectors.yaml names a record vendor, its page says {:?}",
                    stated(text)
                )),
                _ => {}
            }
        }
        for (name, text) in pages {
            if !policy.providers.contains_key(name) && !records.contains(name) {
                out.push(format!(
                    "{name}: names no provider in providers.yaml and no vendor in connectors.yaml"
                ));
            }
            out.extend(page_problems(name, text));
        }
        out
    }

    #[test]
    fn every_served_vendor_has_a_connector_page_that_quotes_its_terms() {
        let problems = policy_problems(policy(), &record_vendors(), &pages());
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn a_served_vendor_with_no_page_or_a_page_without_its_quote_is_refused() {
        let file = |mcp: &str| {
            parse(&format!(
                "version: 1\nproviders:\n  acme: {{ingest: allow, mcp: {mcp}, path: x}}\n"
            ))
        };
        let served = file("served").expect("served");
        let withheld = file("withheld").expect("withheld");
        let none: &[String] = &[];
        let quote = "> You may pass the data to software the user chooses.\n\nSource: <https://acme.example/terms>\n";
        let good = format!("# Acme\n\n**MCP policy:** served.\n\n{quote}Fetched: 2026-11-25 (plain HTTP)\n");
        let page = |text: &str| BTreeMap::from([("acme".to_string(), text.to_string())]);
        let has = |problems: Vec<String>, what: &str| problems.iter().any(|p| p.contains(what));
        assert!(policy_problems(&served, none, &page(&good)).is_empty());
        // the control: no page at all
        assert!(has(policy_problems(&served, none, &BTreeMap::new()), "does not exist"));
        // a page with no quote, an empty quote, or a quote without its source or date
        let no_quote = "# Acme\n\n**MCP policy:** served\n\nThe terms allow it.\n";
        assert!(has(policy_problems(&served, none, &page(no_quote)), "no quote"));
        let empty = good.replace("> You may pass the data to software the user chooses.", ">");
        assert!(has(policy_problems(&served, none, &page(&empty)), "too short"));
        let no_source = good.replace("Source: <https://acme.example/terms>\n", "");
        assert!(has(policy_problems(&served, none, &page(&no_source)), "no Source"));
        let no_date = good.replace("Fetched: 2026-11-25 (plain HTTP)\n", "Fetched: last week\n");
        assert!(has(policy_problems(&served, none, &page(&no_date)), "no Fetched"));
        // a source after the next heading belongs to another section
        let far = good.replace("\nSource:", "\n## Elsewhere\n\nSource:");
        assert!(has(policy_problems(&served, none, &page(&far)), "no Source"));
        // the page and the file disagree, in either direction
        assert!(has(policy_problems(&withheld, none, &page(&good)), "its page says"));
        let says_withheld = good.replace("**MCP policy:** served", "**MCP policy:** withheld");
        assert!(has(
            policy_problems(&served, none, &page(&says_withheld)),
            "its page says"
        ));
        // two policy lines are no policy line
        let twice = format!("{good}\n**MCP policy:** served\n");
        assert!(has(policy_problems(&served, none, &page(&twice)), "no single"));
        // a withheld page names the clause that keeps it withheld
        assert!(has(
            policy_problems(&withheld, none, &page(&says_withheld)),
            "Withheld by"
        ));
        let named = says_withheld.replace("\n\n>", "\n\n**Withheld by:** section 4(d), no AI input.\n\n>");
        assert!(policy_problems(&withheld, none, &page(&named)).is_empty());
        // the person's own files need no page
        let own =
            parse("version: 1\nproviders:\n  apple_health: {ingest: allow, mcp: served, path: x}\n").expect("own");
        assert!(policy_problems(&own, none, &BTreeMap::new()).is_empty());
        // a record vendor needs a page that says records, and every page names a provider or a vendor
        let empty_policy = parse("version: 1\nproviders: {}\n").expect("empty");
        let acme = ["acme".to_string()];
        assert!(has(
            policy_problems(&empty_policy, &acme, &BTreeMap::new()),
            "a record vendor"
        ));
        assert!(has(
            policy_problems(&empty_policy, &acme, &page(&good)),
            "names a record vendor"
        ));
        let records = good.replace("**MCP policy:** served", "**MCP policy:** records");
        assert!(policy_problems(&empty_policy, &acme, &page(&records)).is_empty());
        assert!(has(
            policy_problems(&empty_policy, none, &page(&records)),
            "names no provider"
        ));
    }

    #[test]
    fn the_sources_served_before_stay_served() {
        for p in [
            "apple_health",
            "health_auto_export",
            "gadgetbridge",
            "nightscout",
            "strava_export",
            "fit",
            "libreview",
            "dexcom_clarity",
        ] {
            assert!(served(p) && ingest_allowed(p), "{p}");
            assert!(served_providers().contains(&p.to_string()), "{p}");
        }
    }

    #[test]
    fn an_unknown_provider_is_withheld_and_dropped() {
        for p in ["", "Oura", "new_vendor", "strava", "garmin"] {
            assert!(!served(p), "{p}");
            assert!(!Reader::Shared.sees(p), "{p}");
        }
        assert!(!ingest_allowed("new_vendor") && !ingest_allowed("strava") && !ingest_allowed("garmin"));
        assert!(Reader::WearablesPage.sees("new_vendor"));
    }

    #[test]
    fn a_bad_file_is_refused() {
        assert!(parse("version: 1\nproviders:\n  Oura: {ingest: allow, mcp: served, path: x}\n").is_err());
        assert!(parse("version: 1\nproviders:\n  oura: {ingest: allow, mcp: shared, path: x}\n").is_err());
        assert!(parse("version: 1\nproviders:\n  oura: {ingest: allow, mcp: served}\n").is_err());
        assert!(parse("version: 1\nproviders:\n  oura: {ingest: allow, mcp: served, path: x, ai: yes}\n").is_err());
        assert!(parse("version: 1\nproviders:\n  oura: {ingest: allow, mcp: served, path: x}\n").is_ok());
    }
}
