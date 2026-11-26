//! The mapping: the header to a Patient and a Composition, and coded sections to FHIR R4
//! resources the FHIR importer already reads (`crate::fhir::mapping`).
//!
//! | Section (templateId `2.16.840.1.113883.10.20.22.2.x`) | Entry | FHIR |
//! |---|---|---|
//! | Allergies (`6`, `6.1`) | Allergy Concern Act, one per Allergy-Intolerance Observation | AllergyIntolerance |
//! | Problems (`5`, `5.1`) | Problem Concern Act, one per Problem Observation | Condition |
//! | Medications (`1`, `1.1`) | Medication Activity | MedicationStatement (`EVN`) or MedicationRequest (`INT`) |
//! | Immunizations (`2`, `2.1`) | Immunization Activity (`EVN`, not active) | Immunization |
//! | Results (`3`, `3.1`) | Result Organizer and its Result Observations | DiagnosticReport plus Observations |
//! | Vital Signs (`4`, `4.1`) | Vital Signs Organizer and its observations | Observations |
//! | Encounters (`22`, `22.1`) | Encounter Activity and its indications | Encounter (indications as `reasonCode`) |
//! | Procedures (`7`, `7.1`) | Procedure Activity Procedure, Act and Observation | Procedure or Observation |
//! | Notes (`65`) | Note Activity and narrative-only Notes sections | Kept in the XML, counted in review |
//! | Social History (`17`, `17.2`) | Smoking status and other Social History observations, not pregnancy or sexual orientation | Observations |
//!
//! Differences from Medplum, each on purpose:
//! - Resource ids come from the C-CDA `id` (a hash of root and extension), or from the entry's bytes
//!   when it has none, so a later download of the same record dedupes against this one. Two entries
//!   of one file with the same `id` get `-2`, `-3`.
//! - A `<reference value="#x"/>` is read as the narrative text it points at.
//! - "No known allergies, problems or medicines" (a negated entry that names nothing) becomes the IPS
//!   absent-unknown code, which the importer counts and does not keep as a record. A negated entry
//!   that names something (not allergic to peanuts, does not have diabetes, did not take a medicine)
//!   never becomes a plain record: a problem is `refuted`, an allergy keeps no substance code and a
//!   medicine is `not-taken`, so the importer holds the last two for review.
//! - A Medication Activity in `EVN` mood is a MedicationStatement (what the person takes), and every
//!   SUBJ observation of a concern act is read, not only the first. In an allergy concern act only an
//!   Allergy-Intolerance Observation (or one without a templateId) is an allergy; a Severity
//!   Observation there fills the severity of the reactions that have none, and any other observation
//!   is listed as not read.
//! - The Patient id never comes from the social security number: from the first other patientRole
//!   `id`, else from the name, birth date and gender.
//! - DS4P labels (a Security Observation, templateId `2.16.840.1.113883.3.3445.*`, in an entry or as
//!   an entry of its section) and a confidentialityCode other than `N` (of the document, the section or
//!   inside the entry) go to every resource made there as `meta.security`. The FHIR importer applies
//!   the same sensitivity rules as for a labelled FHIR file. A section's labels reach its subsections
//!   at any depth. Narrative copied by reference carries the labels of its source entry and sections.
//!   The labels in a Privacy Markings Section (templateId `2.16.840.1.113883.3.3445.4`) and a
//!   confidentialityCode on the structuredBody go to every
//!   resource of the document. A Security Observation is never a record of its own. Each resource
//!   holds its own copy of its labels, so a file is refused when it carries more than 64 different
//!   labels, a label whose code is over 64 characters or whose codeSystem is over 128, or more than
//!   250,000 label copies over all its resources. No label is ever dropped.
//! - An entered-in-error allergy or problem has no `clinicalStatus` (FHIR R4 ait-2, con-5). A problem
//!   whose status observation says active while its effectiveTime has an end is listed for review
//!   with that reason, never imported as active or as resolved (con-4).
//! - A supply order, dispense or bounded dose in a Medication Activity is listed as not read. Its first
//!   precondition that names something ("as needed for wheezing") is the dosage's
//!   `asNeededCodeableConcept`; any other is listed as not read. A `<reference>` with words written
//!   beside it keeps both (the narrative first).
//! - Only an Immunization Activity in `EVN` mood that is not active is a vaccine given; a planned one
//!   (`INT`), another mood or an active one is listed as not read.
//! - A Results organizer is a DiagnosticReport with the Observations as its results; a Vital Signs
//!   organizer only groups, so its observations come alone.
//! - An Encounter's indications are its `reasonCode`, never Conditions: a Condition would claim a
//!   clinical status the visit does not give. A procedure that is negated, cancelled or `new`, a
//!   visit or activity observation that is aborted, cancelled or nullified, an activity whose status
//!   code is no ActStatus this reads, and a Social History observation coded as a pregnancy or
//!   sexual orientation question (in its code or a translation) are listed as not read. So is an
//!   activity bound that is no date, or an end before the start (the period keeps what reads).
//! - Section text that no `<reference>` points into (a paragraph, list, table or loose words at the
//!   top of the section's `text`) is listed with its word count, even when every entry reads.
//! - Every section, entry or part the converter does not read is listed in the section's report, and
//!   the importer turns it into a review item. Nothing is thrown away and nothing throws.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use ownhealth_core::Result;
use roxmltree::{Node, NodeId};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::xml::{self, attr, child, children, elements, has_template, is_true, path, templates, xsi_type};
use crate::fhir::Entry;
use crate::fhir::mapping::{ABSENT_UNKNOWN, LOINC, UCUM};

mod activities;

/// One of the sections this version reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Allergies,
    Medications,
    Problems,
    Results,
    VitalSigns,
    Immunizations,
    Encounters,
    Procedures,
    Notes,
    SocialHistory,
}

/// (section, templateId roots, LOINC section code). A section without one of those templateIds is
/// still read when its code is the LOINC code of the section.
const SECTIONS: &[(Section, &[&str], &str)] = &[
    (
        Section::Allergies,
        &["2.16.840.1.113883.10.20.22.2.6", "2.16.840.1.113883.10.20.22.2.6.1"],
        "48765-2",
    ),
    (
        Section::Medications,
        &["2.16.840.1.113883.10.20.22.2.1", "2.16.840.1.113883.10.20.22.2.1.1"],
        "10160-0",
    ),
    (
        Section::Problems,
        &["2.16.840.1.113883.10.20.22.2.5", "2.16.840.1.113883.10.20.22.2.5.1"],
        "11450-4",
    ),
    (
        Section::Results,
        &["2.16.840.1.113883.10.20.22.2.3", "2.16.840.1.113883.10.20.22.2.3.1"],
        "30954-2",
    ),
    (
        Section::VitalSigns,
        &["2.16.840.1.113883.10.20.22.2.4", "2.16.840.1.113883.10.20.22.2.4.1"],
        "8716-3",
    ),
    (
        Section::Immunizations,
        &["2.16.840.1.113883.10.20.22.2.2", "2.16.840.1.113883.10.20.22.2.2.1"],
        "11369-6",
    ),
    (
        Section::Encounters,
        &["2.16.840.1.113883.10.20.22.2.22", "2.16.840.1.113883.10.20.22.2.22.1"],
        "46240-8",
    ),
    (
        Section::Procedures,
        &["2.16.840.1.113883.10.20.22.2.7", "2.16.840.1.113883.10.20.22.2.7.1"],
        "47519-4",
    ),
    (Section::Notes, &["2.16.840.1.113883.10.20.22.2.65"], "11488-4"),
    (
        Section::SocialHistory,
        &["2.16.840.1.113883.10.20.22.2.17", "2.16.840.1.113883.10.20.22.2.17.2"],
        "29762-2",
    ),
];

impl Section {
    fn of(section: Node<'_, '_>) -> Option<Section> {
        let roots = templates(section);
        if let Some((s, _, _)) = SECTIONS.iter().find(|(_, t, _)| roots.iter().any(|r| t.contains(r))) {
            return Some(*s);
        }
        let code = child(section, "code").filter(|c| attr(*c, "codeSystem") == Some(OID_LOINC));
        let code = code.and_then(|c| attr(c, "code"))?;
        SECTIONS.iter().find(|(_, _, loinc)| *loinc == code).map(|(s, _, _)| *s)
    }

    pub fn name(self) -> &'static str {
        match self {
            Section::Allergies => "allergies",
            Section::Medications => "medications",
            Section::Problems => "problems",
            Section::Results => "results",
            Section::VitalSigns => "vital_signs",
            Section::Immunizations => "immunizations",
            Section::Encounters => "encounters",
            Section::Procedures => "procedures",
            Section::Notes => "notes",
            Section::SocialHistory => "social_history",
        }
    }
}

/// What the converter did with one section of the document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SectionReport {
    /// Where it sits: `/section/3`, `/section/3/section/0` for a subsection.
    pub pointer: String,
    pub title: String,
    pub templates: Vec<String>,
    /// The section's LOINC code, when it has one.
    pub code: Option<String>,
    /// None: a section this version does not read.
    pub kind: Option<Section>,
    /// `entry` elements in it.
    pub entries: usize,
    /// FHIR resources made from it, by type.
    pub resources: BTreeMap<String, usize>,
    /// Notes available in the stored XML, with no clinical resource made from them.
    pub stored_notes: usize,
    /// One line per entry, or part of an entry, that was not read (`observation 2.16.840...`).
    pub not_read: Vec<String>,
    /// The section has narrative text of its own and does not say it has no information (no
    /// `nullFlavor`).
    pub narrative: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Converted {
    /// The document's own title.
    pub title: Option<String>,
    /// Parts of the document header kept only in the uploaded file.
    pub header_not_read: Vec<String>,
    /// Patient and Composition first, then every resource in document order.
    pub entries: Vec<Entry>,
    pub sections: Vec<SectionReport>,
}

impl Converted {
    /// Resources of `rtype` made from sections of `kind`.
    pub fn count(&self, kind: Section, rtype: &str) -> usize {
        self.sections
            .iter()
            .filter(|s| s.kind == Some(kind))
            .map(|s| s.resources.get(rtype).copied().unwrap_or(0))
            .sum()
    }
}

/// Convert one C-CDA file. Refuses (`Error::Invalid`) what is not a well-formed C-CDA document within
/// the limits of [`super::xml`]; never panics.
pub fn convert(bytes: &[u8]) -> Result<Converted> {
    let text = xml::text_of_file(bytes)?;
    let doc = xml::parse(&text)?;
    let root = doc.root_element();
    // a body that is a scan or an attached file holds no coded section: refused, its reference never
    // followed, so it is never imported as a document with nothing unread
    if path(root, &["component", "structuredBody"]).is_none() && path(root, &["component", "nonXMLBody"]).is_some() {
        return Err(xml::unstructured());
    }
    // every record carries every label of its document, its sections and its entry, so the labels a
    // file may hold are capped in number and length: a file with more is refused, never read with
    // labels dropped
    check_labels(root)?;
    let top = path(root, &["component", "structuredBody"]);
    // the document's confidentialityCode, and the structuredBody's, label every record
    let mut doc_labels = confidentiality(root).into_iter().collect::<Vec<_>>();
    add_labels(&mut doc_labels, top.and_then(confidentiality));
    // a Privacy Markings Section labels the whole document (DS4P document-level labels)
    for sec in root.descendants().filter(|n| is_privacy_markings(*n)) {
        for entry in children(sec, "entry") {
            add_labels(&mut doc_labels, entry_labels(entry));
        }
    }
    let narrative = xml::narrative_ids(&doc);
    // the narrative elements a `<reference>` points at, so text no entry points at is counted
    let referenced = root
        .descendants()
        .filter(|n| n.has_tag_name((xml::NS, "reference")))
        .filter_map(|n| attr(n, "value"))
        .filter_map(|v| narrative.get(v.trim_start_matches('#')))
        .map(|n| n.id())
        .collect();
    let mut c = Conv {
        text: &text,
        narrative,
        referenced,
        read: HashMap::new(),
        source_labels: HashMap::new(),
        narrative_labels: Vec::new(),
        unread_concepts: Vec::new(),
        read_originals: HashSet::new(),
        held: HashSet::new(),
        ids: HashMap::new(),
        patient: None,
        labels: doc_labels.clone(),
        doc_labels,
        label_copies: 0,
        out: Vec::new(),
    };
    c.header(root);
    let header_not_read: Vec<String> = std::mem::take(&mut c.unread_concepts)
        .into_iter()
        .map(|(_, line)| line)
        .collect();
    let mut sections = Vec::new();
    // sections and their subsections in document order, with a stack instead of recursion; each
    // subsection inherits the labels of the section around it
    let mut stack: Vec<(Node<'_, '_>, String, Rc<Vec<Value>>)> = Vec::new();
    let inherited = Rc::new(c.doc_labels.clone());
    for (i, s) in top
        .map(|b| subsections(b).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .rev()
    {
        stack.push((s, format!("/section/{i}"), Rc::clone(&inherited)));
    }
    // past the label copy budget nothing more is read: the file is refused below
    while c.label_copies <= MAX_LABEL_COPIES
        && let Some((sec, pointer, inherited)) = stack.pop()
    {
        let (report, labels) = c.section(sec, pointer, &inherited);
        let labels = Rc::new(labels);
        let nested: Vec<_> = subsections(sec).collect();
        for (i, s) in nested.into_iter().enumerate().rev() {
            stack.push((s, format!("{}/section/{i}", report.pointer), Rc::clone(&labels)));
        }
        sections.push(report);
    }
    if c.label_copies > MAX_LABEL_COPIES {
        return Err(xml::too_many_label_copies(MAX_LABEL_COPIES));
    }
    Ok(Converted {
        title: child(root, "title").and_then(xml::text),
        header_not_read,
        entries: c.out,
        sections,
    })
}

fn subsections<'a, 'i>(n: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    children(n, "component").filter_map(|c| child(c, "section"))
}

// ------------------------------------------------------------------ systems ----

const OID_LOINC: &str = "2.16.840.1.113883.6.1";
const OID_SNOMED: &str = "2.16.840.1.113883.6.96";
const OID_SSN: &str = "2.16.840.1.113883.4.1";
const OID_CONFIDENTIALITY: &str = "2.16.840.1.113883.5.25";
const SNOMED: &str = "http://snomed.info/sct";
/// The templateId root of every DS4P template (Security Observation `.21` and its kin).
const DS4P: &str = "2.16.840.1.113883.3.3445.";
/// The templateId of the DS4P Privacy Markings Section, which labels the whole document.
const PRIVACY_MARKINGS: &str = "2.16.840.1.113883.3.3445.4";
const ALLERGY_OBSERVATION: &str = "2.16.840.1.113883.10.20.22.4.7";
const SEVERITY_OBSERVATION: &str = "2.16.840.1.113883.10.20.22.4.8";
/// The most different security labels a file may carry.
const MAX_LABELS: usize = 64;
/// The longest code of a security label, in characters (real ones are a few letters).
const MAX_LABEL_CODE: usize = 64;
/// The longest codeSystem of a security label, in characters (real ones are OIDs of about 25).
const MAX_LABEL_SYSTEM: usize = 128;
/// The most label copies one file may make: each resource holds its own copy of each of its labels.
/// At most a few hundred bytes each: about 160 MB at the longest labels allowed.
const MAX_LABEL_COPIES: usize = 250_000;

/// C-CDA code system OIDs and the FHIR system URL each one has (Medplum `systems.ts`, with CVX and
/// the ICD and NDC systems at their HL7 URLs). Any other OID becomes `urn:oid:<oid>`.
const SYSTEMS: [(&str, &str); 23] = [
    (OID_LOINC, LOINC),
    (OID_SNOMED, SNOMED),
    ("2.16.840.1.113883.6.88", "http://www.nlm.nih.gov/research/umls/rxnorm"),
    ("2.16.840.1.113883.12.292", "http://hl7.org/fhir/sid/cvx"),
    ("2.16.840.1.113883.6.90", "http://hl7.org/fhir/sid/icd-10-cm"),
    ("2.16.840.1.113883.6.3", "http://hl7.org/fhir/sid/icd-10"),
    ("2.16.840.1.113883.6.103", "http://hl7.org/fhir/sid/icd-9-cm"),
    ("2.16.840.1.113883.6.69", "http://hl7.org/fhir/sid/ndc"),
    ("2.16.840.1.113883.6.12", "http://www.ama-assn.org/go/cpt"),
    ("2.16.840.1.113883.4.9", "http://fdasis.nlm.nih.gov"),
    ("2.16.840.1.113883.3.26.1.1", "http://ncithesaurus-stage.nci.nih.gov"),
    ("2.16.840.1.113883.6.24", "urn:iso:std:iso:11073:10101"),
    ("2.16.840.1.113883.6.8", UCUM),
    ("2.16.840.1.113883.6.101", "http://nucc.org/provider-taxonomy"),
    ("2.16.840.1.113883.6.209", "http://hl7.org/fhir/ndfrt"),
    ("2.16.840.1.113883.6.345", "http://va.gov/terminology/medrt"),
    (
        "2.16.840.1.113883.5.83",
        "http://terminology.hl7.org/CodeSystem/v3-ObservationInterpretation",
    ),
    (
        "2.16.840.1.113883.5.4",
        "http://terminology.hl7.org/CodeSystem/v3-ActCode",
    ),
    (
        "2.16.840.1.113883.5.1",
        "http://terminology.hl7.org/CodeSystem/v3-AdministrativeGender",
    ),
    (
        OID_CONFIDENTIALITY,
        "http://terminology.hl7.org/CodeSystem/v3-Confidentiality",
    ),
    (
        "2.16.840.1.113883.5.111",
        "http://terminology.hl7.org/CodeSystem/v3-RoleCode",
    ),
    ("2.16.840.1.113883.4.6", "http://hl7.org/fhir/sid/us-npi"),
    (OID_SSN, "http://hl7.org/fhir/sid/us-ssn"),
];

pub fn system_url(oid: &str) -> String {
    SYSTEMS
        .iter()
        .find(|(o, _)| *o == oid)
        .map_or_else(|| format!("urn:oid:{oid}"), |(_, url)| (*url).to_string())
}

// -------------------------------------------------------------------- dates ----

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn in_range(s: &str, lo: u32, hi: u32) -> bool {
    s.parse::<u32>().is_ok_and(|v| (lo..=hi).contains(&v))
}

/// A C-CDA `TS` (`YYYYMMDDHHMMSS.UUUU[+|-ZZzz]`, any tail left off) as a FHIR date or dateTime
/// (Medplum `mapCcdaToFhirDateTime`). A time without a zone is read as UTC, as Medplum does. A value
/// that is not a timestamp gives None.
pub fn fhir_datetime(ts: &str) -> Option<String> {
    let ts = ts.trim();
    let (value, zone) = match ts.len().checked_sub(5).and_then(|i| ts.get(i..).map(|z| (i, z))) {
        Some((i, z)) if (z.starts_with('+') || z.starts_with('-')) && digits(&z[1..]) => (&ts[..i], Some(z)),
        _ => (ts, None),
    };
    let (value, fraction) = match value.split_once('.') {
        Some((v, f)) if digits(f) => (v, Some(f)),
        Some(_) => return None,
        None => (value, None),
    };
    // every component whole (year, month, day, hour, minute, second), a fraction only after seconds
    if !digits(value) || ![4, 6, 8, 10, 12, 14].contains(&value.len()) || (fraction.is_some() && value.len() != 14) {
        return None;
    }
    let part = |a: usize, b: usize| value.get(a..b);
    let year = part(0, 4)?;
    if value.len() < 6 {
        return Some(year.to_string());
    }
    let month = part(4, 6).filter(|m| in_range(m, 1, 12))?;
    if value.len() < 8 {
        return Some(format!("{year}-{month}"));
    }
    let day = part(6, 8).filter(|d| in_range(d, 1, 31))?;
    // a day the month does not have (February 31) is no date
    chrono::NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, day.parse().ok()?)?;
    let date = format!("{year}-{month}-{day}");
    if value.len() < 10 {
        return Some(date);
    }
    let hour = part(8, 10).filter(|h| in_range(h, 0, 23))?;
    let minute = part(10, 12).unwrap_or("00");
    let second = part(12, 14).unwrap_or("00");
    if !in_range(minute, 0, 59) || !in_range(second, 0, 60) {
        return None;
    }
    let zone = match zone {
        None | Some("+0000" | "-0000") => "Z".to_string(),
        Some(z) => {
            let (h, m) = (z.get(1..3)?, z.get(3..5)?);
            // FHIR offsets run to 14:00 at most
            if !in_range(h, 0, 14) || !in_range(m, 0, 59) || (h == "14" && m != "00") {
                return None;
            }
            format!("{}:{}", z.get(..3)?, m)
        }
    };
    let fraction = fraction.map(|f| format!(".{f}")).unwrap_or_default();
    Some(format!("{date}T{hour}:{minute}:{second}{fraction}{zone}"))
}

/// The date part of [`fhir_datetime`].
pub fn fhir_date(ts: &str) -> Option<String> {
    fhir_datetime(ts).map(|d| d.chars().take(10).collect())
}

/// An `effectiveTime` (or `time`) element as one instant: its `value`, else `low`, else `center`.
fn ts(n: Option<Node<'_, '_>>) -> Option<String> {
    let n = n?;
    attr(n, "value")
        .or_else(|| child(n, "low").and_then(|l| attr(l, "value")))
        .or_else(|| child(n, "center").and_then(|c| attr(c, "value")))
        .and_then(fhir_datetime)
}

/// A point is the start of a period. An interval can have either bound.
fn period(n: Option<Node<'_, '_>>) -> Option<Value> {
    let mut r = Map::new();
    put(&mut r, "start", ts(n).map(Value::String));
    put(&mut r, "end", bound(n, "high").map(Value::String));
    (!r.is_empty()).then_some(Value::Object(r))
}

/// An activity period with any reversed end dropped and listed for review. A bound written but no
/// date is listed too, and the period keeps the bound that reads.
fn ordered_period(n: Option<Node<'_, '_>>, of: Node<'_, '_>, rep: &mut SectionReport) -> Option<Value> {
    let no_date = |b: Option<Node<'_, '_>>| {
        b.and_then(|b| attr(b, "value"))
            .is_some_and(|v| fhir_datetime(v).is_none())
    };
    if no_date(n) || no_date(n.and_then(|n| child(n, "low"))) {
        rep.not_read.push(format!("{} (start is not a date)", describe(of)));
    }
    if no_date(n.and_then(|n| child(n, "high"))) {
        rep.not_read.push(format!("{} (end is not a date)", describe(of)));
    }
    let mut p = period(n)?;
    let date = |key: &str| p.get(key).and_then(Value::as_str);
    if let (Some(start), Some(end)) = (date("start"), date("end")) {
        let reversed = match (
            chrono::DateTime::parse_from_rfc3339(start),
            chrono::DateTime::parse_from_rfc3339(end),
        ) {
            (Ok(start), Ok(end)) => end < start,
            _ => {
                // Compare the date parts at the precision both bounds have.
                let len = start.len().min(end.len()).min(10);
                end.as_bytes()[..len] < start.as_bytes()[..len]
            }
        };
        if reversed && let Value::Object(m) = &mut p {
            m.shift_remove("end");
            rep.not_read.push(format!("{} (ends before it starts)", describe(of)));
        }
    }
    Some(p)
}

fn bound(n: Option<Node<'_, '_>>, which: &'static str) -> Option<String> {
    child(n?, which).and_then(|b| attr(b, "value")).and_then(fhir_datetime)
}

// ------------------------------------------------------------------ helpers ----

fn put(r: &mut Map<String, Value>, key: &str, v: Option<Value>) {
    if let Some(v) = v.filter(|v| !v.is_null()) {
        r.insert(key.to_string(), v);
    }
}

fn coded(system: &str, code: &str) -> Value {
    json!({"coding": [{"system": system, "code": code}]})
}

fn normalized_text(text: &str) -> String {
    let mut normalized = String::new();
    for word in text.split_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(&word.to_lowercase());
    }
    normalized
}

fn status_of<'a>(n: Node<'a, '_>) -> Option<&'a str> {
    child(n, "statusCode").and_then(|s| attr(s, "code"))
}

/// The observations under `entryRelationship` elements of `n`, those of `type_code` when given.
fn related<'a, 'i>(n: Node<'a, 'i>, type_code: Option<&'static str>) -> impl Iterator<Item = Node<'a, 'i>> {
    children(n, "entryRelationship")
        .filter(move |er| type_code.is_none_or(|t| attr(*er, "typeCode") == Some(t)))
        .flat_map(|er| children(er, "observation"))
}

/// The value code of the first related observation with one of `roots`.
fn related_value<'a>(n: Node<'a, '_>, roots: &[&str]) -> Option<&'a str> {
    related(n, None)
        .find(|o| has_template(*o, roots))
        .and_then(|o| child(o, "value"))
        .and_then(|v| attr(v, "code"))
}

const STATUS_OBSERVATIONS: [&str; 2] = ["2.16.840.1.113883.10.20.22.4.6", "2.16.840.1.113883.10.20.22.4.28"];
const CLINICAL_STATUS: [(&str, &str); 3] = [
    ("55561003", "active"),
    ("73425007", "inactive"),
    ("413322009", "resolved"),
];

/// active | inactive | resolved: the status observation when there is one, else resolved when the
/// observation's effectiveTime has an end.
fn clinical_status(obs: Node<'_, '_>) -> &'static str {
    if let Some(s) = lookup(&CLINICAL_STATUS, related_value(obs, &STATUS_OBSERVATIONS)) {
        return s;
    }
    if bound(child(obs, "effectiveTime"), "high").is_some() {
        return "resolved";
    }
    "active"
}

fn author_time(n: Node<'_, '_>) -> Option<String> {
    path(n, &["author", "time"])
        .and_then(|t| attr(t, "value"))
        .and_then(fhir_datetime)
}

/// A PQ as a FHIR Quantity, or None when its value is not a number.
fn quantity(n: Node<'_, '_>) -> Option<Value> {
    let raw = attr(n, "value")?;
    let raw = raw.strip_prefix('+').unwrap_or(raw);
    let fixed = if raw.starts_with('.') {
        format!("0{raw}")
    } else if let Some(r) = raw.strip_prefix("-.") {
        format!("-0.{r}")
    } else {
        raw.trim_end_matches('.').to_string()
    };
    let number: serde_json::Number = fixed.parse().ok()?;
    let mut q = Map::new();
    q.insert("value".into(), Value::Number(number));
    if let Some(unit) = attr(n, "unit") {
        q.insert("unit".into(), json!(unit));
        q.insert("system".into(), json!(UCUM));
        q.insert("code".into(), json!(unit));
    }
    Some(Value::Object(q))
}

/// Generic "substance", "problem" and "medicine" concepts: a negated entry naming only one of these
/// names nothing.
const GENERIC: [&str; 6] = [
    "105590001",
    "55607006",
    "404684003",
    "64572001",
    "410942007",
    "418038007",
];

/// IPS absent-unknown codes for a negated allergy that names no substance, by its type.
fn no_known_allergy(value_code: Option<&str>) -> &'static str {
    match value_code {
        Some("414285001" | "235719002") => "no-known-food-allergies",
        Some("416098002" | "419511003" | "59037007") => "no-known-medication-allergies",
        _ => "no-known-allergies",
    }
}

fn absent(code: &str, display: &str) -> Value {
    json!({"coding": [{"system": ABSENT_UNKNOWN, "code": code, "display": display}], "text": display})
}

const ALLERGY_CATEGORY: [(&str, &str); 7] = [
    ("414285001", "food"),
    ("235719002", "food"),
    ("416098002", "medication"),
    ("419511003", "medication"),
    ("59037007", "medication"),
    ("426232007", "environment"),
    ("418038007", "biologic"),
];
const SEVERITY: [(&str, &str); 3] = [("255604002", "mild"), ("6736007", "moderate"), ("24484000", "severe")];
const CRITICALITY: [(&str, &str); 3] = [("CRITL", "low"), ("CRITH", "high"), ("CRITU", "unable-to-assess")];

fn lookup(table: &[(&str, &'static str)], code: Option<&str>) -> Option<&'static str> {
    let code = code?;
    table.iter().find(|(c, _)| *c == code).map(|(_, v)| *v)
}

fn observation_status(code: Option<&str>) -> &'static str {
    match code {
        Some("active") => "preliminary",
        Some("aborted" | "cancelled") => "cancelled",
        Some("nullified") => "entered-in-error",
        Some("held" | "suspended" | "new") => "registered",
        _ => "final",
    }
}

const OBSERVATION_CATEGORY: &str = "http://terminology.hl7.org/CodeSystem/observation-category";

// ---------------------------------------------------------------- converter ----

enum ObservationValue {
    Read(&'static str, Value, bool),
    Missing,
    Unsupported,
}

struct Conv<'a, 'i> {
    /// The whole file, so an entry without an `id` is known by its bytes.
    text: &'i str,
    narrative: HashMap<&'a str, Node<'a, 'i>>,
    /// The narrative elements a `<reference>` of the document points at.
    referenced: HashSet<NodeId>,
    /// The text of each narrative element a reference read, so a narrative many entries point at is
    /// read once.
    read: HashMap<NodeId, xml::NarrativeText>,
    /// Each referenced section or entry's own labels, shared by all targets inside it.
    source_labels: HashMap<NodeId, Vec<Value>>,
    /// Labels of the narrative used to build the current resource.
    narrative_labels: Vec<Value>,
    /// Omitted original text in the header or the current section, with the `originalText` and the
    /// elements around it: a notice inside an element held whole is counted with that hold.
    unread_concepts: Vec<(Vec<NodeId>, String)>,
    /// Every `originalText` a coded concept read or compared with what it kept.
    read_originals: HashSet<NodeId>,
    /// Elements sent to review whole: the original text inside them is counted with them.
    held: HashSet<NodeId>,
    /// `<type>/<id>` given so far, to tell apart entries of one file that share an `id`.
    ids: HashMap<String, usize>,
    patient: Option<String>,
    /// The document's security labels, and the labels of the entry being read (the document's, its
    /// section's and its own): each resource made gets them as `meta.security`.
    doc_labels: Vec<Value>,
    labels: Vec<Value>,
    /// The labels copied into resources so far. Past [`MAX_LABEL_COPIES`] no more are copied and the
    /// file is refused once read, so the copies stay bounded and no label is ever dropped.
    label_copies: usize,
    out: Vec<Entry>,
}

impl<'a, 'i> Conv<'a, 'i> {
    fn push(&mut self, rep: Option<&mut SectionReport>, pointer: &str, mut resource: Value) {
        let narrative_labels = std::mem::take(&mut self.narrative_labels);
        let combined;
        let labels = if narrative_labels.is_empty() {
            &self.labels
        } else {
            combined = {
                let mut labels = self.labels.clone();
                add_labels(&mut labels, narrative_labels);
                labels
            };
            &combined
        };
        if !labels.is_empty()
            && let Value::Object(r) = &mut resource
        {
            self.label_copies = self.label_copies.saturating_add(labels.len());
            if self.label_copies <= MAX_LABEL_COPIES {
                r.insert("meta".into(), json!({"security": labels}));
            }
        }
        let rtype = resource["resourceType"].as_str().unwrap_or_default().to_string();
        if let Some(rep) = rep {
            *rep.resources.entry(rtype).or_default() += 1;
        }
        self.out.push(Entry {
            pointer: pointer.to_string(),
            resource,
            full_url: None,
        });
    }

    /// The first `id` of the given elements with a root, as `root|extension`.
    fn first_id(nodes: &[Node<'_, '_>]) -> Option<String> {
        nodes.iter().find_map(|n| {
            children(*n, "id")
                .filter(|i| attr(*i, "nullFlavor").is_none())
                .find_map(|i| attr(i, "root").map(|r| format!("{r}|{}", attr(i, "extension").unwrap_or_default())))
        })
    }

    /// A FHIR id: `ccda-` and 16 hex of the C-CDA id (or of the element's bytes), `-n` when this
    /// file already gave it to another resource of the type.
    fn new_id(&mut self, rtype: &str, ids_of: &[Node<'_, '_>], element: Node<'_, '_>) -> String {
        let basis = Self::first_id(ids_of)
            .unwrap_or_else(|| format!("bytes:{}", self.text.get(element.range()).unwrap_or_default()));
        self.id_from(rtype, &basis)
    }

    /// [`Self::new_id`] from a given basis.
    fn id_from(&mut self, rtype: &str, basis: &str) -> String {
        let mut id = format!("ccda-{}", &hex::encode(Sha256::digest(basis.as_bytes()))[..16]);
        let seen = self.ids.entry(format!("{rtype}/{id}")).or_default();
        *seen += 1;
        if *seen > 1 {
            id = format!("{id}-{seen}");
        }
        id
    }

    fn identifiers(nodes: &[Node<'_, '_>]) -> Option<Value> {
        let mut out = Vec::new();
        for n in nodes {
            for i in children(*n, "id").filter(|i| attr(*i, "nullFlavor").is_none()) {
                let Some(root) = attr(i, "root") else { continue };
                let v = match attr(i, "extension") {
                    Some(ext) => json!({"system": system_url(root), "value": ext}),
                    None if root.len() == 36 && root.matches('-').count() == 4 => {
                        json!({"system": "urn:ietf:rfc:3986", "value": format!("urn:uuid:{}", root.to_lowercase())})
                    }
                    None => json!({"system": "urn:ietf:rfc:3986", "value": format!("urn:oid:{root}")}),
                };
                if !out.contains(&v) {
                    out.push(v);
                }
            }
        }
        (!out.is_empty()).then_some(Value::Array(out))
    }

    /// Sends a whole element to review: the line that lists it.
    fn hold(&mut self, el: Node<'_, '_>) -> String {
        self.held.insert(el.id());
        describe(el)
    }

    /// Lists the original text written inside an entry element that no coded concept read: a method,
    /// a target site, an interpretation. Its words exist only in the uploaded file. A bare reference
    /// adds no words of its own (they are in the section narrative), and text inside a part already
    /// held whole is counted with that part.
    fn unread_original_texts(&mut self, el: Node<'_, '_>, rep: &mut SectionReport) {
        for original in el.descendants().filter(|n| n.has_tag_name((xml::NS, "originalText"))) {
            if self.read_originals.contains(&original.id())
                || !original
                    .descendants()
                    .any(|n| n.is_text() && n.text().is_some_and(|t| !t.trim().is_empty()))
            {
                continue;
            }
            let mut counted = false;
            for n in std::iter::successors(Some(original), Node::parent_element) {
                if self.held.contains(&n.id()) || activities::is_note_activity(n) || is_security_observation(n) {
                    counted = true;
                }
                if counted || n == el {
                    break;
                }
            }
            if !counted {
                let parent = original
                    .parent_element()
                    .map_or("originalText", |p| p.tag_name().name());
                rep.not_read
                    .push(format!("{parent} (original text kept in the uploaded file)"));
            }
        }
    }

    fn subject(&self) -> Option<Value> {
        self.patient.as_ref().map(|p| json!({"reference": p}))
    }

    fn referenced_narrative(&mut self, n: Node<'_, '_>) -> Option<(Node<'a, 'i>, &xml::NarrativeText)> {
        let reference = child(n, "reference").and_then(|r| attr(r, "value"))?;
        let &target = self.narrative.get(reference.trim_start_matches('#'))?;
        let text = self.read.entry(target.id()).or_insert_with(|| xml::read_text(target));
        Some((target, text))
    }

    /// Bounded text from the referenced narrative followed by any local words. Either scan can
    /// hit a limit before finding text. The truncation flag also covers a cut when joining them.
    fn narrative_of(&mut self, n: Node<'_, '_>) -> xml::NarrativeText {
        let mut local = xml::read_text(n);
        let Some((target, text)) = self.referenced_narrative(n) else {
            return local;
        };
        local.truncated |= text.truncated;
        let Some(referenced) = text.value.clone() else {
            return local;
        };
        // The target can be in a later section or under a restricted parent section.
        for sec in
            std::iter::successors(Some(target), Node::parent_element).filter(|n| n.has_tag_name((xml::NS, "section")))
        {
            let labels = self
                .source_labels
                .entry(sec.id())
                .or_insert_with(|| section_labels(sec));
            if !labels.is_empty() {
                add_labels(&mut self.narrative_labels, labels.iter().cloned());
            }
        }
        if let Some(entry) =
            std::iter::successors(Some(target), Node::parent_element).find(|n| n.has_tag_name((xml::NS, "entry")))
        {
            let labels = self
                .source_labels
                .entry(entry.id())
                .or_insert_with(|| entry_labels(entry));
            if !labels.is_empty() {
                add_labels(&mut self.narrative_labels, labels.iter().cloned());
            }
        }
        local.value = Some(match local.value {
            // a local label inside a narrative word ("reactive" in "nonreactive") is a label of its own
            Some(own) if normalized_text(&referenced) != normalized_text(&own) => {
                local.truncated |= xml::joined_cuts(&referenced, &own);
                xml::joined(&referenced, &own)
            }
            _ => referenced,
        });
        local
    }

    fn reported_narrative(&mut self, n: Node<'_, '_>, field: &str, rep: &mut SectionReport) -> xml::NarrativeText {
        let text = self.narrative_of(n);
        if text.truncated {
            rep.not_read.push(format!("{field} (text scan limit reached)"));
        }
        text
    }

    fn coding(n: Node<'_, '_>) -> Option<Value> {
        let code = attr(n, "code");
        let display = attr(n, "displayName");
        if code.is_none() && display.is_none() {
            return None;
        }
        let mut c = Map::new();
        put(&mut c, "system", attr(n, "codeSystem").map(|s| json!(system_url(s))));
        put(&mut c, "code", code.map(|s| json!(s)));
        put(&mut c, "display", display.map(|s| json!(s)));
        Some(Value::Object(c))
    }

    /// A CD as a CodeableConcept: its code and translations, and a text. `skip` leaves codes out
    /// (the generic "substance" of a negated allergy). Only encounter types prefer original text.
    fn concept_data(&mut self, n: Node<'_, '_>, skip: &[&str], prefer_original: bool) -> Option<Value> {
        let keep = |c: &Node<'_, '_>| attr(*c, "code").is_none_or(|code| !skip.contains(&code));
        let mut codings: Vec<Value> = std::iter::once(n)
            .filter(keep)
            .chain(children(n, "translation").filter(keep))
            .filter_map(Self::coding)
            .collect();
        codings.dedup();
        let own_display = attr(n, "displayName").filter(|_| keep(&n));
        let text = own_display
            .filter(|_| !prefer_original)
            .map(String::from)
            .or_else(|| child(n, "originalText").and_then(|o| self.narrative_of(o).value))
            .or_else(|| own_display.map(String::from));
        if codings.is_empty() && text.is_none() {
            return None;
        }
        let mut cc = Map::new();
        if !codings.is_empty() {
            cc.insert("coding".into(), Value::Array(codings));
        }
        put(&mut cc, "text", text.map(Value::String));
        Some(Value::Object(cc))
    }

    fn concept(&mut self, n: Option<Node<'_, '_>>) -> Option<Value> {
        self.reported_concept(n, &[], false)
    }

    fn reported_concept(&mut self, n: Option<Node<'_, '_>>, skip: &[&str], prefer_original: bool) -> Option<Value> {
        let n = n?;
        let concept = self.concept_data(n, skip, prefer_original);
        let originals: Vec<_> = std::iter::once(n)
            .chain(children(n, "translation"))
            .filter_map(|code| Some((code.tag_name().name(), child(code, "originalText")?)))
            .collect();
        if !originals.is_empty() {
            // Compare the original even if its scan found no words and there is no concept.
            let kept = concept.as_ref().unwrap_or(&Value::Null);
            let displays: HashSet<_> = kept["text"]
                .as_str()
                .into_iter()
                .chain(
                    kept["coding"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|coding| coding["display"].as_str()),
                )
                .map(normalized_text)
                .collect();
            for (field, original) in originals {
                self.read_originals.insert(original.id());
                if self.unread_original_text(original, &displays) {
                    let around = std::iter::successors(Some(original), Node::parent_element)
                        .map(|n| n.id())
                        .collect();
                    self.unread_concepts
                        .push((around, format!("{field} (original text kept in the uploaded file)")));
                }
            }
        }
        concept
    }

    /// Whether a negated entry's concept names nothing: absent, null, or only a generic concept. The
    /// original text of a generic code ("Problem" pointing at "no known problems") names nothing either.
    fn names_nothing(&mut self, n: Option<Node<'_, '_>>) -> bool {
        let generic = n.and_then(|n| attr(n, "code")).is_some_and(|c| GENERIC.contains(&c));
        match n.and_then(|n| self.concept_data(n, &GENERIC, false)) {
            None => true,
            Some(cc) => generic && cc.get("coding").is_none(),
        }
    }

    // ------------------------------------------------------------- header ----

    fn header(&mut self, root: Node<'_, '_>) {
        if let Some(role) = path(root, &["recordTarget", "patientRole"]) {
            let patient = child(role, "patient");
            let names: Vec<Value> = patient
                .map(|p| children(p, "name").filter_map(Self::name).collect())
                .unwrap_or_default();
            let birth = patient
                .and_then(|p| child(p, "birthTime"))
                .and_then(|b| attr(b, "value"))
                .and_then(fhir_date);
            let gender = patient
                .and_then(|p| child(p, "administrativeGenderCode"))
                .and_then(|g| attr(g, "code"))
                .and_then(|g| match g {
                    "F" => Some("female"),
                    "M" => Some("male"),
                    "UN" => Some("other"),
                    _ => None,
                });
            // the social security number identifies no record: it stays in the file only, and the
            // id comes from another id of the patient, else from who the patient is
            let basis = children(role, "id")
                .filter(|i| attr(*i, "nullFlavor").is_none() && attr(*i, "root") != Some(OID_SSN))
                .find_map(|i| attr(i, "root").map(|r| format!("{r}|{}", attr(i, "extension").unwrap_or_default())))
                .unwrap_or_else(|| {
                    format!(
                        "patient:{}|{}|{}",
                        Value::Array(names.clone()),
                        birth.as_deref().unwrap_or_default(),
                        gender.unwrap_or_default()
                    )
                });
            let id = self.id_from("Patient", &basis);
            let mut r = Map::new();
            r.insert("resourceType".into(), json!("Patient"));
            r.insert("id".into(), json!(id));
            if let Some(Value::Array(list)) = Self::identifiers(&[role]) {
                let ssn = system_url(OID_SSN);
                let kept: Vec<Value> = list.into_iter().filter(|i| i["system"] != json!(ssn)).collect();
                if !kept.is_empty() {
                    r.insert("identifier".into(), Value::Array(kept));
                }
            }
            if !names.is_empty() {
                r.insert("name".into(), Value::Array(names));
            }
            put(&mut r, "gender", gender.map(|g| json!(g)));
            put(&mut r, "birthDate", birth.map(Value::String));
            self.patient = Some(format!("Patient/{id}"));
            self.push(None, "/recordTarget", Value::Object(r));
        }
        let id = self.new_id("Composition", &[root], root);
        let mut c = Map::new();
        c.insert("resourceType".into(), json!("Composition"));
        c.insert("id".into(), json!(id));
        c.insert("status".into(), json!("final"));
        put(&mut c, "type", self.concept(child(root, "code")));
        put(&mut c, "subject", self.subject());
        put(&mut c, "date", ts(child(root, "effectiveTime")).map(Value::String));
        put(
            &mut c,
            "title",
            child(root, "title").and_then(xml::text).map(Value::String),
        );
        self.push(None, "/", Value::Object(c));
    }

    fn name(n: Node<'_, '_>) -> Option<Value> {
        let parts =
            |tag: &'static str| -> Vec<Value> { children(n, tag).filter_map(xml::text).map(Value::String).collect() };
        let mut h = Map::new();
        let given = parts("given");
        if !given.is_empty() {
            h.insert("given".into(), Value::Array(given));
        }
        put(
            &mut h,
            "family",
            child(n, "family").and_then(xml::text).map(Value::String),
        );
        for (tag, key) in [("prefix", "prefix"), ("suffix", "suffix")] {
            let p = parts(tag);
            if !p.is_empty() {
                h.insert(key.into(), Value::Array(p));
            }
        }
        if h.is_empty() {
            // a name written as plain text
            put(&mut h, "text", xml::text(n).map(Value::String));
        }
        (!h.is_empty()).then_some(Value::Object(h))
    }

    // ----------------------------------------------------------- sections ----

    /// Read one section. `inherited` are the labels of the document and of the sections around it;
    /// the section's own labels are returned with them, for its subsections.
    fn section(&mut self, sec: Node<'_, '_>, pointer: String, inherited: &[Value]) -> (SectionReport, Vec<Value>) {
        let entries: Vec<Node<'_, '_>> = children(sec, "entry").collect();
        let mut rep = SectionReport {
            title: child(sec, "title").and_then(xml::text).unwrap_or_default(),
            templates: templates(sec).into_iter().map(String::from).collect(),
            code: child(sec, "code").and_then(|c| attr(c, "code")).map(String::from),
            kind: Section::of(sec),
            entries: entries.len(),
            // A text cut before its first word still holds narrative the converter did not read.
            narrative: attr(sec, "nullFlavor").is_none()
                && child(sec, "text").is_some_and(|t| {
                    let text = xml::read_text(t);
                    text.value.is_some() || text.truncated
                }),
            pointer,
            ..SectionReport::default()
        };
        rep.templates.dedup();
        let mut labels = inherited.to_vec();
        add_labels(&mut labels, section_labels(sec));
        let Some(kind) = rep.kind else {
            return (rep, labels);
        };
        for (j, entry) in entries.into_iter().enumerate() {
            if self.label_copies > MAX_LABEL_COPIES {
                break; // the file is refused once its sections stop
            }
            let at = format!("{}/entry/{j}", rep.pointer);
            if elements(entry).next().is_none() {
                rep.not_read.push("entry (no activity)".into());
                continue;
            }
            rep.stored_notes += entry.descendants().filter(|n| activities::is_note_activity(*n)).count();
            self.labels = labels.clone();
            add_labels(&mut self.labels, entry_labels(entry));
            for (k, el) in elements(entry).enumerate() {
                let at = if k == 0 { at.clone() } else { format!("{at}/{k}") };
                self.narrative_labels.clear();
                if is_security_observation(el) || activities::is_note_activity(el) {
                    continue; // counted above as a label or a stored note
                }
                let read = match (kind, el.tag_name().name()) {
                    (Section::Allergies, "act") => self.allergy_concern(el, &at, &mut rep),
                    (Section::Allergies, "observation") => {
                        self.allergy(el, None, &at, &mut rep);
                        true
                    }
                    (Section::Problems, "act") => self.problem_concern(el, &at, &mut rep),
                    (Section::Problems, "observation") => {
                        self.problem(el, None, &at, &mut rep);
                        true
                    }
                    (Section::Medications, "substanceAdministration") => {
                        self.medication(el, &at, &mut rep);
                        true
                    }
                    (Section::Immunizations, "substanceAdministration") => self.immunization(el, &at, &mut rep),
                    (Section::Results, "organizer") => self.results(el, &at, &mut rep),
                    (Section::Results, "observation") => {
                        self.observation(el, None, "laboratory", &at, &mut rep).is_some()
                    }
                    (Section::VitalSigns, "organizer") => self.vitals(el, &at, &mut rep),
                    (Section::VitalSigns, "observation") => {
                        self.observation(el, None, "vital-signs", &at, &mut rep).is_some()
                    }
                    (Section::Encounters, "encounter") => self.encounter(el, &at, &mut rep),
                    (Section::Procedures, "procedure" | "act") => self.procedure(el, &at, &mut rep),
                    (Section::Procedures | Section::SocialHistory, "observation") => {
                        self.activity_observation(el, &at, &mut rep)
                    }
                    _ => false,
                };
                if read {
                    self.unread_original_texts(el, &mut rep);
                } else {
                    rep.not_read.push(self.hold(el));
                }
            }
        }
        if kind == Section::Notes && rep.narrative && rep.stored_notes == 0 {
            rep.stored_notes = 1;
        } else if rep.narrative
            && rep.entries > 0
            && let Some(words) = child(sec, "text")
                .map(|t| self.unreferenced_words(t))
                .filter(|w| *w > 0)
        {
            let unit = if words == 1 { "word" } else { "words" };
            rep.not_read
                .push(format!("section text no entry points at ({words} {unit})"));
        }
        self.labels = self.doc_labels.clone();
        for (around, line) in std::mem::take(&mut self.unread_concepts) {
            if !around.iter().any(|id| self.held.contains(id)) {
                rep.not_read.push(line);
            }
        }
        (rep, labels)
    }

    /// Words in the blocks of a section's text that no `<reference>` points into: a paragraph, list,
    /// table or loose words at the top of the text that hold no referenced element. A block that holds
    /// one shows an entry read with it; any other block was read by nothing.
    fn unreferenced_words(&self, text: Node<'_, '_>) -> usize {
        if self.referenced.contains(&text.id()) {
            return 0;
        }
        text.children()
            .filter(|block| !block.descendants().any(|n| self.referenced.contains(&n.id())))
            .flat_map(|block| block.descendants())
            .filter_map(|n| n.is_text().then(|| n.text()).flatten())
            .map(|t| t.split_whitespace().count())
            .sum()
    }

    /// A problem concern act: one Condition per SUBJ observation. False when it holds none.
    fn problem_concern(&mut self, act: Node<'_, '_>, at: &str, rep: &mut SectionReport) -> bool {
        let subjects: Vec<Node<'_, '_>> = related(act, Some("SUBJ")).collect();
        for (i, obs) in subjects.iter().enumerate() {
            let at = if i == 0 { at.to_string() } else { format!("{at}#{i}") };
            self.problem(*obs, Some(act), &at, rep);
        }
        !subjects.is_empty()
    }

    /// An allergy concern act: one AllergyIntolerance per Allergy-Intolerance Observation (or SUBJ
    /// observation without a templateId). A Severity Observation of the act gives the severity of
    /// reactions that state none; one the converter cannot map, and any other observation, is listed
    /// as not read. False when the act holds no allergy.
    fn allergy_concern(&mut self, act: Node<'_, '_>, at: &str, rep: &mut SectionReport) -> bool {
        let mut allergies = Vec::new();
        let mut left = Vec::new();
        for obs in related(act, Some("SUBJ")) {
            let roots = templates(obs);
            if roots.is_empty() || roots.contains(&ALLERGY_OBSERVATION) {
                allergies.push(obs);
            } else if !(roots.contains(&SEVERITY_OBSERVATION) && severity_of(obs).is_some()) {
                left.push(self.hold(obs));
            }
        }
        for (i, obs) in allergies.iter().enumerate() {
            let at = if i == 0 { at.to_string() } else { format!("{at}#{i}") };
            self.allergy(*obs, Some(act), &at, rep);
        }
        if allergies.is_empty() {
            return false;
        }
        rep.not_read.extend(left);
        true
    }

    fn allergy(&mut self, obs: Node<'_, '_>, act: Option<Node<'_, '_>>, at: &str, rep: &mut SectionReport) {
        let negated = is_true(obs, "negationInd");
        let value_code = child(obs, "value").and_then(|v| attr(v, "code"));
        let participant = children(obs, "participant")
            .find(|p| attr(*p, "typeCode") == Some("CSM"))
            .or_else(|| child(obs, "participant"));
        let allergen = participant.and_then(|p| path(p, &["participantRole", "playingEntity", "code"]));
        let id = self.new_id("AllergyIntolerance", &[obs, act.unwrap_or(obs)], obs);
        let mut r = Map::new();
        r.insert("resourceType".into(), json!("AllergyIntolerance"));
        r.insert("id".into(), json!(id));
        let nodes: Vec<Node<'_, '_>> = act.into_iter().chain([obs]).collect();
        put(&mut r, "identifier", Self::identifiers(&nodes));
        let nullified = nodes.iter().any(|n| status_of(*n) == Some("nullified"));
        // FHIR R4 ait-2: an entered-in-error allergy has no clinical status
        if !nullified {
            r.insert(
                "clinicalStatus".into(),
                coded(
                    "http://terminology.hl7.org/CodeSystem/allergyintolerance-clinical",
                    clinical_status(obs),
                ),
            );
        }
        r.insert(
            "verificationStatus".into(),
            coded(
                "http://terminology.hl7.org/CodeSystem/allergyintolerance-verification",
                if nullified { "entered-in-error" } else { "confirmed" },
            ),
        );
        let intolerance = matches!(value_code, Some("235719002" | "59037007"));
        r.insert(
            "type".into(),
            json!(if intolerance { "intolerance" } else { "allergy" }),
        );
        put(
            &mut r,
            "category",
            lookup(&ALLERGY_CATEGORY, value_code).map(|c| json!([c])),
        );
        if negated && self.names_nothing(allergen) {
            let code = no_known_allergy(value_code);
            r.insert("code".into(), absent(code, &code.replace('-', " ")));
        } else if negated {
            // not allergic to a named substance: FHIR has no negated code for it, so the substance
            // goes in the exposure-risk extension (Medplum, C-CDA on FHIR) and no code is set
            r.insert(
                "extension".into(),
                json!([{
                    "url": "http://hl7.org/fhir/StructureDefinition/allergyintolerance-substanceExposureRisk",
                    "extension": [
                        {"url": "substance", "valueCodeableConcept": self.reported_concept(allergen, &GENERIC, false)},
                        {"url": "exposureRisk", "valueCodeableConcept": coded(
                            "http://terminology.hl7.org/CodeSystem/allerg-intol-substance-exp-risk",
                            "no-known-reaction-risk")},
                    ],
                }]),
            );
        } else {
            put(&mut r, "code", self.concept(allergen));
        }
        put(&mut r, "patient", self.subject());
        put(
            &mut r,
            "onsetDateTime",
            bound(child(obs, "effectiveTime"), "low").map(Value::String),
        );
        let recorded = act
            .and_then(|a| ts(child(a, "effectiveTime")).or_else(|| author_time(a)))
            .or_else(|| author_time(obs));
        put(&mut r, "recordedDate", recorded.map(Value::String));
        put(
            &mut r,
            "criticality",
            lookup(&CRITICALITY, related_value(obs, &["2.16.840.1.113883.10.20.22.4.145"])).map(|c| json!(c)),
        );
        // the act's own Severity Observation, for the reactions that state none
        let act_severity = act
            .and_then(|a| related(a, Some("SUBJ")).find(|o| has_template(*o, &[SEVERITY_OBSERVATION])))
            .and_then(severity_of);
        let reactions: Vec<Value> = related(obs, Some("MFST"))
            .map(|ro| {
                let mut rx = Map::new();
                let manifestation = self.concept(child(ro, "value")).unwrap_or_else(|| {
                    json!({"extension": [{"url": "http://hl7.org/fhir/StructureDefinition/data-absent-reason", "valueCode": "unknown"}]})
                });
                rx.insert("manifestation".into(), json!([manifestation]));
                put(&mut rx, "onset", bound(child(ro, "effectiveTime"), "low").map(Value::String));
                let severity = related(ro, None)
                    .find(|o| has_template(*o, &[SEVERITY_OBSERVATION]))
                    .and_then(severity_of)
                    .or(act_severity);
                put(&mut rx, "severity", severity.map(|s| json!(s)));
                Value::Object(rx)
            })
            .collect();
        if !reactions.is_empty() {
            r.insert("reaction".into(), Value::Array(reactions));
        }
        self.push(Some(rep), at, Value::Object(r));
    }

    fn problem(
        &mut self,
        obs: Node<'_, '_>,
        act: Option<Node<'_, '_>>,
        at: &str,
        rep: &mut SectionReport,
    ) -> Option<String> {
        let negated = is_true(obs, "negationInd");
        let value = child(obs, "value");
        let nodes: Vec<Node<'_, '_>> = act.into_iter().chain([obs]).collect();
        let nullified = nodes.iter().any(|n| status_of(*n) == Some("nullified"));
        let effective = child(obs, "effectiveTime");
        // FHIR R4 con-5: an entered-in-error condition has no clinical status (and so, by con-4, no
        // abatement)
        let status = (!nullified).then(|| clinical_status(obs));
        let abatement = bound(effective, "high").filter(|_| status.is_some());
        if status == Some("active") && abatement.is_some() {
            // con-4: an abated condition is never active. The status observation and the end date
            // disagree, so a person decides
            rep.not_read
                .push(format!("{} (status active with an end date)", self.hold(obs)));
            return None;
        }
        let nothing = negated && self.names_nothing(value);
        let verification = if nullified {
            "entered-in-error"
        } else if negated && !nothing {
            "refuted"
        } else {
            "confirmed"
        };
        let id = self.new_id("Condition", &[obs, act.unwrap_or(obs)], obs);
        let mut r = Map::new();
        r.insert("resourceType".into(), json!("Condition"));
        r.insert("id".into(), json!(id));
        put(&mut r, "identifier", Self::identifiers(&nodes));
        put(
            &mut r,
            "clinicalStatus",
            status.map(|s| coded("http://terminology.hl7.org/CodeSystem/condition-clinical", s)),
        );
        r.insert(
            "verificationStatus".into(),
            coded(
                "http://terminology.hl7.org/CodeSystem/condition-ver-status",
                verification,
            ),
        );
        let (category, display) = if rep.kind == Some(Section::Encounters) {
            ("encounter-diagnosis", "Encounter Diagnosis")
        } else {
            ("problem-list-item", "Problem List Item")
        };
        r.insert(
            "category".into(),
            json!([{"coding": [{"system": "http://terminology.hl7.org/CodeSystem/condition-category",
                "code": category, "display": display}]}]),
        );
        if nothing {
            r.insert("code".into(), absent("no-known-problems", "no known problems"));
        } else {
            put(&mut r, "code", self.concept(value));
        }
        put(&mut r, "subject", self.subject());
        put(
            &mut r,
            "onsetDateTime",
            bound(effective, "low")
                .or_else(|| attr_ts(effective))
                .map(Value::String),
        );
        put(&mut r, "abatementDateTime", abatement.map(Value::String));
        let recorded = act
            .and_then(|a| ts(child(a, "effectiveTime")).or_else(|| author_time(a)))
            .or_else(|| author_time(obs));
        put(&mut r, "recordedDate", recorded.map(Value::String));
        self.push(Some(rep), at, Value::Object(r));
        Some(id)
    }

    fn material<'b, 'j>(sa: Node<'b, 'j>) -> Option<Node<'b, 'j>> {
        path(sa, &["consumable", "manufacturedProduct", "manufacturedMaterial"])
    }

    fn medication(&mut self, sa: Node<'_, '_>, at: &str, rep: &mut SectionReport) {
        let negated = is_true(sa, "negationInd");
        let code = Self::material(sa).and_then(|m| child(m, "code"));
        let request = !negated && matches!(attr(sa, "moodCode"), Some("INT" | "RQO" | "PRMS" | "PRP"));
        let rtype = if request {
            "MedicationRequest"
        } else {
            "MedicationStatement"
        };
        let id = self.new_id(rtype, &[sa], sa);
        let mut r = Map::new();
        r.insert("resourceType".into(), json!(rtype));
        r.insert("id".into(), json!(id));
        put(&mut r, "identifier", Self::identifiers(&[sa]));
        let state = status_of(sa);
        let status = if negated {
            "not-taken"
        } else {
            match (request, state) {
                (_, Some("active")) => "active",
                (_, Some("completed")) => "completed",
                (_, Some("aborted")) => "stopped",
                (true, Some("cancelled")) => "cancelled",
                (false, Some("cancelled")) => "stopped",
                (_, Some("suspended" | "held")) => "on-hold",
                (_, Some("nullified")) => "entered-in-error",
                (true, Some("new")) => "draft",
                (false, Some("new")) => "intended",
                _ => "unknown",
            }
        };
        if negated && self.names_nothing(code) {
            // "no medicines": the IPS code says the list is empty
            r.insert("status".into(), json!("unknown"));
            r.insert(
                "medicationCodeableConcept".into(),
                absent("no-known-medications", "no known medications"),
            );
        } else {
            r.insert("status".into(), json!(status));
            if request {
                r.insert("intent".into(), json!("order"));
            }
            put(&mut r, "medicationCodeableConcept", self.concept(code));
        }
        put(&mut r, "subject", self.subject());
        let (mut period, mut single, mut repeat) = (None, None, None);
        for et in children(sa, "effectiveTime") {
            match xsi_type(et) {
                Some("PIVL_TS" | "EIVL_TS") => {
                    if let Some(p) = child(et, "period")
                        && let Some(Value::Object(q)) = quantity(p)
                    {
                        repeat = Some(json!({"period": q.get("value"), "periodUnit": attr(p, "unit")}));
                    }
                }
                _ if child(et, "low").is_some() || child(et, "high").is_some() => {
                    let mut p = Map::new();
                    put(&mut p, "start", bound(Some(et), "low").map(Value::String));
                    put(&mut p, "end", bound(Some(et), "high").map(Value::String));
                    if !p.is_empty() && period.is_none() {
                        period = Some(Value::Object(p));
                    }
                }
                _ => single = single.or_else(|| attr_ts(Some(et))),
            }
        }
        if request {
            put(&mut r, "authoredOn", author_time(sa).map(Value::String));
            put(&mut r, "dispenseRequest", period.map(|p| json!({"validityPeriod": p})));
        } else {
            put(&mut r, "effectivePeriod", period);
            if !r.contains_key("effectivePeriod") {
                put(&mut r, "effectiveDateTime", single.map(Value::String));
            }
            put(&mut r, "dateAsserted", author_time(sa).map(Value::String));
        }
        let mut dosage = Map::new();
        // the free text sig (Instruction 4.147), else the entry's own text
        let sig = children(sa, "entryRelationship")
            .flat_map(|er| children(er, "substanceAdministration"))
            .find(|s| has_template(*s, &["2.16.840.1.113883.10.20.22.4.147"]))
            .and_then(|s| child(s, "text"))
            .or_else(|| child(sa, "text"))
            .and_then(|t| self.reported_narrative(t, "medication instructions", rep).value);
        put(&mut dosage, "text", sig.map(Value::String));
        put(&mut dosage, "timing", repeat.map(|rp| json!({"repeat": rp})));
        put(&mut dosage, "route", self.concept(child(sa, "routeCode")));
        if let Some(dose) = child(sa, "doseQuantity") {
            if let Some(q) = quantity(dose) {
                dosage.insert("doseAndRate".into(), json!([{"doseQuantity": q}]));
            } else if attr(dose, "nullFlavor").is_none()
                && (child(dose, "low").is_some() || child(dose, "high").is_some())
            {
                rep.not_read.push("doseQuantity (bounded dose)".into());
            }
        }
        // "as needed for wheezing" (Precondition for Substance Administration 4.25): the first
        // criterion that names something; any other is listed as not read
        let mut as_needed = None;
        let mut unread = Vec::new();
        for criterion in children(sa, "precondition").flat_map(|p| children(p, "criterion")) {
            if as_needed.is_none()
                && let Some(cc) = self.concept(child(criterion, "value"))
            {
                as_needed = Some(cc);
            } else {
                unread.push(self.hold(criterion));
            }
        }
        put(&mut dosage, "asNeededCodeableConcept", as_needed);
        if !dosage.is_empty() {
            r.insert(
                if request { "dosageInstruction" } else { "dosage" }.into(),
                json!([dosage]),
            );
        }
        let reasons: Vec<Value> = related(sa, Some("RSON"))
            .filter_map(|o| self.concept(child(o, "value")))
            .collect();
        if !reasons.is_empty() {
            r.insert("reasonCode".into(), Value::Array(reasons));
        }
        self.push(Some(rep), at, Value::Object(r));
        // a supply order or a dispense of the medicine is no record this version reads
        for supply in children(sa, "entryRelationship").flat_map(|er| children(er, "supply")) {
            let line = self.hold(supply);
            rep.not_read.push(line);
        }
        rep.not_read.extend(unread);
    }

    /// A vaccine given (an Immunization Activity in `EVN` mood that is not still active). A planned
    /// one (`INT`), any other mood and an active one are no record of a vaccine given: false, so the
    /// section lists it as not read.
    fn immunization(&mut self, sa: Node<'_, '_>, at: &str, rep: &mut SectionReport) -> bool {
        if attr(sa, "moodCode") != Some("EVN") || status_of(sa) == Some("active") {
            return false;
        }
        let negated = is_true(sa, "negationInd");
        let material = Self::material(sa);
        let id = self.new_id("Immunization", &[sa], sa);
        let mut r = Map::new();
        r.insert("resourceType".into(), json!("Immunization"));
        r.insert("id".into(), json!(id));
        put(&mut r, "identifier", Self::identifiers(&[sa]));
        let status = if status_of(sa) == Some("nullified") {
            "entered-in-error"
        } else if negated {
            "not-done"
        } else {
            "completed"
        };
        r.insert("status".into(), json!(status));
        if negated {
            let reason = related(sa, Some("RSON")).find_map(|o| self.concept(child(o, "code")));
            put(&mut r, "statusReason", reason);
        }
        put(
            &mut r,
            "vaccineCode",
            self.concept(material.and_then(|m| child(m, "code"))),
        );
        put(&mut r, "patient", self.subject());
        put(
            &mut r,
            "occurrenceDateTime",
            ts(child(sa, "effectiveTime")).map(Value::String),
        );
        put(
            &mut r,
            "lotNumber",
            material
                .and_then(|m| child(m, "lotNumberText"))
                .and_then(xml::text)
                .map(Value::String),
        );
        let maker = path(
            sa,
            &["consumable", "manufacturedProduct", "manufacturerOrganization", "name"],
        )
        .and_then(xml::text);
        put(&mut r, "manufacturer", maker.map(|m| json!({"display": m})));
        put(&mut r, "route", self.concept(child(sa, "routeCode")));
        put(&mut r, "doseQuantity", child(sa, "doseQuantity").and_then(quantity));
        let dose = child(sa, "repeatNumber")
            .and_then(|n| attr(n, "value"))
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|n| *n > 0);
        put(
            &mut r,
            "protocolApplied",
            dose.map(|d| json!([{"doseNumberPositiveInt": d}])),
        );
        self.push(Some(rep), at, Value::Object(r));
        true
    }

    /// A Results organizer: its observations, then a DiagnosticReport that lists them.
    fn results(&mut self, org: Node<'_, '_>, at: &str, rep: &mut SectionReport) -> bool {
        let mut refs = Vec::new();
        for (i, comp) in children(org, "component").enumerate() {
            for (k, el) in elements(comp).enumerate() {
                let at = format!(
                    "{at}/component/{i}{}",
                    if k == 0 { String::new() } else { format!("/{k}") }
                );
                if is_security_observation(el) {
                    continue; // a label of the entry
                }
                let id = match el.tag_name().name() {
                    "observation" => self.observation(el, Some(org), "laboratory", &at, rep),
                    _ => None,
                };
                match id {
                    Some(id) => refs.push(json!({"reference": format!("Observation/{id}")})),
                    None => rep.not_read.push(self.hold(el)),
                }
            }
        }
        if refs.is_empty() {
            return false;
        }
        let id = self.new_id("DiagnosticReport", &[org], org);
        let mut r = Map::new();
        r.insert("resourceType".into(), json!("DiagnosticReport"));
        r.insert("id".into(), json!(id));
        put(&mut r, "identifier", Self::identifiers(&[org]));
        let status = match status_of(org) {
            Some("active") => "partial",
            Some("aborted" | "cancelled") => "cancelled",
            Some("nullified") => "entered-in-error",
            _ => "final",
        };
        r.insert("status".into(), json!(status));
        r.insert(
            "category".into(),
            json!([coded("http://terminology.hl7.org/CodeSystem/v2-0074", "LAB")]),
        );
        let code = self
            .concept(child(org, "code"))
            .unwrap_or_else(|| json!({"text": if rep.title.is_empty() { "Results" } else { rep.title.as_str() }}));
        r.insert("code".into(), code);
        put(&mut r, "subject", self.subject());
        put(
            &mut r,
            "effectiveDateTime",
            ts(child(org, "effectiveTime")).map(Value::String),
        );
        r.insert("result".into(), Value::Array(refs));
        self.push(Some(rep), at, Value::Object(r));
        true
    }

    /// A Vital Signs organizer only groups: its observations come alone, each with the organizer's
    /// time when it has none of its own.
    fn vitals(&mut self, org: Node<'_, '_>, at: &str, rep: &mut SectionReport) -> bool {
        let mut any = false;
        for (i, comp) in children(org, "component").enumerate() {
            for (k, el) in elements(comp).enumerate() {
                let at = format!(
                    "{at}/component/{i}{}",
                    if k == 0 { String::new() } else { format!("/{k}") }
                );
                if is_security_observation(el) {
                    continue; // a label of the entry
                }
                let read = el.tag_name().name() == "observation"
                    && self.observation(el, Some(org), "vital-signs", &at, rep).is_some();
                if read {
                    any = true;
                } else {
                    rep.not_read.push(self.hold(el));
                }
            }
        }
        any
    }

    /// One result or vital sign. None when it has no code, as Medplum skips it; the caller lists it.
    fn observation(
        &mut self,
        obs: Node<'_, '_>,
        organizer: Option<Node<'_, '_>>,
        category: &str,
        at: &str,
        rep: &mut SectionReport,
    ) -> Option<String> {
        let code = self.concept(child(obs, "code"))?;
        let id = self.new_id("Observation", &[obs], obs);
        let mut r = Map::new();
        r.insert("resourceType".into(), json!("Observation"));
        r.insert("id".into(), json!(id));
        put(&mut r, "identifier", Self::identifiers(&[obs]));
        r.insert("status".into(), json!(observation_status(status_of(obs))));
        r.insert("category".into(), json!([coded(OBSERVATION_CATEGORY, category)]));
        r.insert("code".into(), code);
        put(&mut r, "subject", self.subject());
        let time = child(obs, "effectiveTime");
        let activity = matches!(rep.kind, Some(Section::Procedures | Section::SocialHistory));
        if activity && time.is_some_and(|t| child(t, "low").is_some() || child(t, "high").is_some()) {
            put(&mut r, "effectivePeriod", ordered_period(time, obs, rep));
        } else {
            let when = ts(time).or_else(|| organizer.and_then(|o| ts(child(o, "effectiveTime"))));
            put(&mut r, "effectiveDateTime", when.map(Value::String));
        }
        if activity {
            put(&mut r, "note", self.activity_note(obs, rep));
        }
        if let Some(v) = child(obs, "value") {
            match self.value(v, rep) {
                ObservationValue::Read(field, value, unread_text) => {
                    r.insert(field.into(), value);
                    if unread_text {
                        rep.not_read.push("observation (text beside a typed value)".into());
                    }
                }
                ObservationValue::Missing => {}
                ObservationValue::Unsupported => {
                    let kind = xsi_type(v).filter(|kind| kind.len() <= 32).unwrap_or("unknown");
                    rep.not_read
                        .push(format!("observation (unsupported value type {kind})"));
                }
            }
        }
        let interpretation: Vec<Value> = children(obs, "interpretationCode")
            .filter_map(|i| attr(i, "code"))
            .map(|c| coded("http://terminology.hl7.org/CodeSystem/v3-ObservationInterpretation", c))
            .collect();
        if !interpretation.is_empty() {
            r.insert("interpretation".into(), Value::Array(interpretation));
        }
        let ranges: Vec<Value> = children(obs, "referenceRange")
            .filter_map(|rr| child(rr, "observationRange"))
            .filter_map(|range| {
                let mut out = Map::new();
                if let Some(v) = child(range, "value") {
                    put(&mut out, "low", child(v, "low").and_then(quantity));
                    put(&mut out, "high", child(v, "high").and_then(quantity));
                }
                put(
                    &mut out,
                    "text",
                    child(range, "text")
                        .and_then(|t| self.reported_narrative(t, "reference range", rep).value)
                        .map(Value::String),
                );
                (!out.is_empty()).then_some(Value::Object(out))
            })
            .collect();
        if !ranges.is_empty() {
            r.insert("referenceRange".into(), Value::Array(ranges));
        }
        self.push(Some(rep), at, Value::Object(r));
        Some(id)
    }

    /// Compare originalText with the retained text and coding displays without copying source labels.
    /// A truncated originalText always has a part the converter could not read.
    fn unread_original_text(&mut self, original: Node<'_, '_>, displays: &HashSet<String>) -> bool {
        let retained = |text: &str| displays.contains(&normalized_text(text));
        let own = xml::read_text(original);
        let target = self.referenced_narrative(original).map(|(_, text)| text);
        if own.truncated || target.is_some_and(|text| text.truncated) {
            return true;
        }
        match (target.and_then(|text| text.value.as_deref()), own.value.as_deref()) {
            (Some(target), Some(text)) if normalized_text(target) != normalized_text(text) => {
                !retained(&format!("{target}; {text}"))
            }
            (Some(target), _) => !retained(target),
            (None, Some(text)) => !retained(text),
            (None, None) => false,
        }
    }

    /// Read an observation's value by its CDA type, distinguishing absent and unread values.
    fn value(&mut self, v: Node<'_, '_>, rep: &mut SectionReport) -> ObservationValue {
        let kind = xsi_type(v).unwrap_or(if attr(v, "code").is_some() {
            "CD"
        } else if attr(v, "value").is_some() {
            "PQ"
        } else {
            "ST"
        });
        let field = match kind {
            "PQ" | "REAL" | "MO" => match quantity(v) {
                Some(q) => Some(("valueQuantity", q)),
                // A value like "positive" or "see note" is kept as written. A unit with no value is not a
                // result: it falls through to the missing or unread value below.
                None => attr(v, "value").map(|value| {
                    let written = [Some(value), attr(v, "unit")]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ");
                    ("valueString", Value::String(written))
                }),
            },
            "IVL_PQ" => {
                let (mut low, high) = (child(v, "low"), child(v, "high"));
                // "0 to 5" is how C-CDA writes "5 or less": the 0 says nothing.
                if high.is_some() && low.and_then(|l| attr(l, "value")).and_then(|x| x.parse::<f64>().ok()) == Some(0.0)
                {
                    low = None;
                }
                let open = |b: Node<'_, '_>| attr(b, "inclusive") == Some("false");
                match (low.and_then(quantity), high.and_then(quantity)) {
                    (None, Some(Value::Object(mut q))) => {
                        let cmp = if high.is_some_and(open) { "<" } else { "<=" };
                        q.insert("comparator".into(), json!(cmp));
                        Some(("valueQuantity", Value::Object(q)))
                    }
                    (Some(Value::Object(mut q)), None) => {
                        let cmp = if low.is_some_and(open) { ">" } else { ">=" };
                        q.insert("comparator".into(), json!(cmp));
                        Some(("valueQuantity", Value::Object(q)))
                    }
                    (Some(_), Some(_)) => {
                        let part = |b: Option<Node<'_, '_>>| {
                            b.map(|b| {
                                [attr(b, "value"), attr(b, "unit")]
                                    .into_iter()
                                    .flatten()
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            })
                            .unwrap_or_default()
                        };
                        Some(("valueString", json!(format!("{} - {}", part(low), part(high)))))
                    }
                    _ => None,
                }
            }
            "CD" | "CE" | "CV" | "CO" | "CS" => self.concept(Some(v)).map(|cc| ("valueCodeableConcept", cc)),
            "INT" => match attr(v, "value").and_then(|s| s.parse::<i32>().ok()) {
                Some(i) => Some(("valueInteger", json!(i))),
                None => quantity(v).map(|q| ("valueQuantity", q)),
            },
            "BL" => attr(v, "value")
                .and_then(|b| b.parse::<bool>().ok())
                .map(|b| ("valueBoolean", Value::Bool(b))),
            "TS" => attr(v, "value")
                .and_then(fhir_datetime)
                .map(|date| ("valueDateTime", Value::String(date))),
            // A ratio is not read; the arms below list it unless it is null.
            "RTO" | "RTO_PQ_PQ" => None,
            _ => {
                let text = self.reported_narrative(v, "observation value", rep);
                if text.truncated && text.value.is_none() {
                    return ObservationValue::Missing; // the incomplete value is already listed
                }
                text.value.map(|text| ("valueString", Value::String(text)))
            }
        };
        let inline_text = v
            .children()
            .any(|n| n.is_text() && n.text().is_some_and(|text| !text.trim().is_empty()));
        let structured = matches!(
            kind,
            "PQ" | "REAL" | "MO" | "IVL_PQ" | "CD" | "CE" | "CV" | "CO" | "CS" | "INT" | "BL" | "TS"
        );
        match field {
            Some((key, value)) => ObservationValue::Read(key, value, structured && inline_text),
            None if inline_text => ObservationValue::Unsupported,
            // Checked after the type: a coded value outside the value set is `nullFlavor="OTH"`
            // with its translation or original text, and that is still the result.
            None if attr(v, "nullFlavor").is_some() => ObservationValue::Missing,
            None if v.attributes().any(|a| a.namespace().is_none())
                || elements(v).any(|el| attr(el, "nullFlavor").is_none()) =>
            {
                ObservationValue::Unsupported
            }
            None => ObservationValue::Missing,
        }
    }
}

/// The value of an `effectiveTime` that is a point (`value`), not an interval.
fn attr_ts(n: Option<Node<'_, '_>>) -> Option<String> {
    n.and_then(|n| attr(n, "value")).and_then(fhir_datetime)
}

/// The FHIR severity of a Severity Observation, when its value maps.
fn severity_of(obs: Node<'_, '_>) -> Option<&'static str> {
    lookup(&SEVERITY, child(obs, "value").and_then(|v| attr(v, "code")))
}

// ------------------------------------------------------------------ labels ----

/// A DS4P Privacy Markings Section: its labels are the labels of the whole document.
fn is_privacy_markings(n: Node<'_, '_>) -> bool {
    n.is_element()
        && n.tag_name().namespace() == Some(xml::NS)
        && n.tag_name().name() == "section"
        && has_template(n, &[PRIVACY_MARKINGS])
}

/// A DS4P observation (Security Observation and its kin): a label, never a record.
fn is_security_observation(n: Node<'_, '_>) -> bool {
    n.tag_name().name() == "observation" && templates(n).iter().any(|t| t.starts_with(DS4P))
}

fn label(code_node: Node<'_, '_>) -> Option<Value> {
    let code = attr(code_node, "code")?;
    let mut c = Map::new();
    put(
        &mut c,
        "system",
        attr(code_node, "codeSystem").map(|s| json!(system_url(s))),
    );
    c.insert("code".into(), json!(code));
    Some(Value::Object(c))
}

/// The confidentialityCode child of `n` as a label, unless it is `N` (normal).
fn confidentiality(n: Node<'_, '_>) -> Option<Value> {
    child(n, "confidentialityCode")
        .filter(|c| attr(*c, "code").is_some_and(|code| !code.eq_ignore_ascii_case("N")))
        .and_then(label)
}

/// The values of a DS4P observation as labels.
fn security_values(obs: Node<'_, '_>) -> Vec<Value> {
    children(obs, "value").filter_map(label).collect()
}

/// A section's own confidentiality code and Security Observations.
fn section_labels(sec: Node<'_, '_>) -> Vec<Value> {
    let mut labels = confidentiality(sec).into_iter().collect();
    for obs in children(sec, "entry")
        .flat_map(elements)
        .filter(|el| is_security_observation(*el))
    {
        add_labels(&mut labels, security_values(obs));
    }
    labels
}

/// The labels inside one entry: every confidentialityCode (other than `N`) and every DS4P
/// observation's values, at any depth.
fn entry_labels(entry: Node<'_, '_>) -> Vec<Value> {
    let mut out = Vec::new();
    for d in entry
        .descendants()
        .filter(|d| d.is_element() && d.tag_name().namespace() == Some(xml::NS))
    {
        if d.tag_name().name() == "confidentialityCode" {
            if let Some(parent) = d.parent_element() {
                add_labels(&mut out, confidentiality(parent));
            }
        } else if is_security_observation(d) {
            add_labels(&mut out, security_values(d));
        }
    }
    out
}

/// Add the labels `to` does not hold yet, in one pass over both.
fn add_labels(to: &mut Vec<Value>, labels: impl IntoIterator<Item = Value>) {
    let mut seen: HashSet<String> = to.iter().map(Value::to_string).collect();
    for l in labels {
        if seen.insert(l.to_string()) {
            to.push(l);
        }
    }
}

/// Refuses a document whose labels (confidentialityCodes other than `N` and DS4P observation values,
/// anywhere in it) are more than [`MAX_LABELS`] different ones, or hold a code longer than
/// [`MAX_LABEL_CODE`] or a codeSystem longer than [`MAX_LABEL_SYSTEM`]. One pass, stopped at the
/// first fault.
fn check_labels(root: Node<'_, '_>) -> Result<()> {
    let mut seen = HashSet::new();
    for d in root
        .descendants()
        .filter(|d| d.is_element() && d.tag_name().namespace() == Some(xml::NS))
    {
        // the nodes `confidentiality` and `security_values` read labels from
        let found: Vec<Node<'_, '_>> = if d.tag_name().name() == "confidentialityCode" {
            d.parent_element()
                .and_then(|p| child(p, "confidentialityCode"))
                .filter(|c| attr(*c, "code").is_some_and(|code| !code.eq_ignore_ascii_case("N")))
                .into_iter()
                .collect()
        } else if is_security_observation(d) {
            children(d, "value").collect()
        } else {
            continue;
        };
        for n in found {
            let longer = |name: &str, max: usize| attr(n, name).is_some_and(|v| v.chars().count() > max);
            if longer("code", MAX_LABEL_CODE) || longer("codeSystem", MAX_LABEL_SYSTEM) {
                return Err(xml::label_too_long(MAX_LABEL_CODE, MAX_LABEL_SYSTEM));
            }
            if let Some(l) = label(n) {
                seen.insert(l.to_string());
                if seen.len() > MAX_LABELS {
                    return Err(xml::too_many_labels(MAX_LABELS));
                }
            }
        }
    }
    Ok(())
}

/// How a part that was not read is listed: its element and first templateId.
fn describe(el: Node<'_, '_>) -> String {
    let t = templates(el);
    match t.first() {
        Some(root) => format!("{} {root}", el.tag_name().name()),
        None => format!("{} without a templateId", el.tag_name().name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_become_fhir_dates_as_medplum_writes_them() {
        assert_eq!(fhir_datetime("2014").as_deref(), Some("2014"));
        assert_eq!(fhir_datetime("201410").as_deref(), Some("2014-10"));
        assert_eq!(fhir_datetime("20141015").as_deref(), Some("2014-10-15"));
        assert_eq!(fhir_datetime("2014101510").as_deref(), Some("2014-10-15T10:00:00Z"));
        assert_eq!(
            fhir_datetime("20141015103026-0500").as_deref(),
            Some("2014-10-15T10:30:26-05:00")
        );
        assert_eq!(
            fhir_datetime("20141015103026.123+0000").as_deref(),
            Some("2014-10-15T10:30:26.123Z")
        );
        for bad in [
            "",
            "abc",
            "201",
            "20141315",
            "2014-10-15",
            "20141015253000",
            "20141015.x",
        ] {
            assert_eq!(fhir_datetime(bad), None, "{bad}");
        }
        assert_eq!(fhir_date("20141015103026-0500").as_deref(), Some("2014-10-15"));
    }

    /// F69-m2: FHIR offsets end at 14:00; +1430 is no time zone, +1400 still reads.
    #[test]
    fn an_offset_past_fourteen_hours_is_no_time_zone() {
        assert_eq!(fhir_datetime("20141015103026+1430"), None);
        assert_eq!(fhir_datetime("20141015103026-1401"), None);
        assert_eq!(
            fhir_datetime("20141015103026+1400").as_deref(),
            Some("2014-10-15T10:30:26+14:00")
        );
        assert_eq!(
            fhir_datetime("20141015103026-1359").as_deref(),
            Some("2014-10-15T10:30:26-13:59")
        );
    }

    /// C-F8: a day the month does not have is no date.
    #[test]
    fn an_impossible_day_is_no_date() {
        for bad in ["20230231", "20230229", "20230431", "2023023110"] {
            assert_eq!(fhir_datetime(bad), None, "{bad}");
        }
        assert_eq!(fhir_datetime("20240229").as_deref(), Some("2024-02-29"));
        assert_eq!(fhir_datetime("20230131").as_deref(), Some("2023-01-31"));
    }

    /// CR8 (round 3): a timestamp with a cut or an extra component is no timestamp; every precision
    /// from the year to the second still reads, with a fraction after the seconds and a zone.
    #[test]
    fn a_timestamp_with_a_partial_component_is_no_date() {
        for bad in [
            "20241",
            "2024011",
            "202401011",
            "20240101123",
            "2024010112300",
            "202401011230001",
            "2024010112300012",
            "2024.5",
            "20240101.5",
            "202401011230.5",
            "20241-0500",
        ] {
            assert_eq!(fhir_datetime(bad), None, "{bad}");
        }
        for (good, want) in [
            ("2024", "2024"),
            ("202401", "2024-01"),
            ("20240101", "2024-01-01"),
            ("2024010112", "2024-01-01T12:00:00Z"),
            ("202401011230", "2024-01-01T12:30:00Z"),
            ("202401011230-0800", "2024-01-01T12:30:00-08:00"),
            ("20240101123001", "2024-01-01T12:30:01Z"),
            ("20240101123001.5+0100", "2024-01-01T12:30:01.5+01:00"),
        ] {
            assert_eq!(fhir_datetime(good).as_deref(), Some(want), "{good}");
        }
    }

    #[test]
    fn code_systems_map_to_their_fhir_urls() {
        assert_eq!(system_url("2.16.840.1.113883.6.1"), LOINC);
        assert_eq!(system_url("2.16.840.1.113883.12.292"), "http://hl7.org/fhir/sid/cvx");
        assert_eq!(system_url("1.2.3"), "urn:oid:1.2.3");
    }
}
