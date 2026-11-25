//! C-CDA import (slice 20): the HL7 C-CDA-Examples samples (CC0, `fixtures/ccda/`) give exact FHIR
//! resources per section; a section or entry this version does not read lands in review, never in
//! an error; malformed, hostile or oversized XML is refused without a panic.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use common::{Env, R, family, store_doc};
use ownhealth_core::Error;
use ownhealth_core::documents::{StoredFile, dispatch_upload, sniff};
use ownhealth_core::jobs::{Job, JobContext};
use ownhealth_core::review::{self, ItemQuery};
use ownhealth_ingest_structured::ccda::{self, Converted, Section, convert, held_items, import_ccda};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/ccda")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
}

/// A minimal ClinicalDocument for Isabella Jones (born 2005-05-01) around `body`, the `component`
/// elements of its structured body.
fn document(body: &str) -> Vec<u8> {
    document_with(
        r#"<id root="2.16.840.1.113883.19.5.99999.2" extension="998991"/>"#,
        body,
    )
}

/// [`document`] with `ids` as the `id` elements of its patientRole.
fn document_with(ids: &str, body: &str) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ClinicalDocument xmlns="urn:hl7-org:v3" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:sdtc="urn:hl7-org:sdtc">
  <templateId root="2.16.840.1.113883.10.20.22.1.2"/>
  <id root="2.16.840.1.113883.19.5.99999.1" extension="ownhealth-test"/>
  <code code="34133-9" codeSystem="2.16.840.1.113883.6.1" displayName="Summary of episode note"/>
  <title>Test document</title>
  <effectiveTime value="20151115"/>
  <recordTarget><patientRole>
    {ids}
    <patient><name><given>Isabella</given><family>Jones</family></name>
      <administrativeGenderCode code="F" codeSystem="2.16.840.1.113883.5.1"/><birthTime value="20050501"/></patient>
  </patientRole></recordTarget>
  <component><structuredBody>{body}</structuredBody></component>
</ClinicalDocument>"#
    )
    .into_bytes()
}

/// A `fixtures/ccda/sections/` file (one section, no document around it) inside [`document`].
fn section_file(name: &str) -> Vec<u8> {
    let raw = String::from_utf8(fixture(&format!("sections/{name}"))).unwrap_or_default();
    let body = match raw.find("<?xml") {
        Some(at) => raw[at..]
            .split_once("?>")
            .map_or(raw.as_str(), |(_, rest)| rest)
            .to_string(),
        None => raw.clone(),
    };
    let first = body
        .match_indices('<')
        .map(|(i, _)| &body[i..])
        .find(|t| !t.starts_with("<!--"))
        .unwrap_or_default();
    if first.starts_with("<component") {
        document(&body)
    } else {
        document(&format!("<component>{body}</component>"))
    }
}

/// Resources by type made from the sections of `kind`.
fn made(c: &Converted, kind: Section) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for s in c.sections.iter().filter(|s| s.kind == Some(kind)) {
        for (t, n) in &s.resources {
            *out.entry(t.clone()).or_default() += n;
        }
    }
    out
}

fn counts(pairs: &[(&str, usize)]) -> BTreeMap<String, usize> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn resources<'a>(c: &'a Converted, rtype: &str) -> Vec<&'a Value> {
    c.entries
        .iter()
        .map(|e| &e.resource)
        .filter(|r| r["resourceType"] == json!(rtype))
        .collect()
}

#[test]
fn the_history_and_physical_sample_gives_exact_resources_per_section() -> R {
    let c = convert(&fixture("history-and-physical.xml"))?;
    assert_eq!(made(&c, Section::Allergies), counts(&[("AllergyIntolerance", 3)]));
    assert_eq!(made(&c, Section::Problems), counts(&[("Condition", 2)]));
    assert_eq!(made(&c, Section::Medications), counts(&[("MedicationStatement", 1)]));
    assert_eq!(made(&c, Section::Immunizations), counts(&[("Immunization", 4)]));
    assert_eq!(
        made(&c, Section::Results),
        counts(&[("DiagnosticReport", 1), ("Observation", 3)])
    );
    assert_eq!(made(&c, Section::VitalSigns), counts(&[("Observation", 6)]));
    // the aborted procedure observation waits in review, never skipped
    assert_eq!(made(&c, Section::Procedures), counts(&[("Procedure", 2)]));
    assert_eq!(made(&c, Section::SocialHistory), counts(&[("Observation", 3)]));
    assert_eq!(c.entries.len(), 27);
    assert_eq!(resources(&c, "Patient").len(), 1);
    assert_eq!(resources(&c, "Composition").len(), 1);
    // Each section counts the text retained only in XML. The medicine's supply order and
    // dispense and the aborted procedure observation also wait in review.
    let original = "code (original text kept in the uploaded file)";
    let unread: Vec<_> = c
        .sections
        .iter()
        .filter_map(|s| {
            let kind = s.kind?;
            (!s.not_read.is_empty()).then(|| (kind, s.not_read.iter().map(String::as_str).collect::<Vec<_>>()))
        })
        .collect();
    assert_eq!(
        unread,
        [
            (Section::Allergies, vec![original; 3]),
            (Section::Immunizations, vec![original; 4]),
            (
                Section::Medications,
                vec![
                    "supply 2.16.840.1.113883.10.20.22.4.17",
                    "supply 2.16.840.1.113883.10.20.22.4.18",
                    original
                ]
            ),
            (
                Section::Procedures,
                vec!["observation 2.16.840.1.113883.10.20.22.4.13 (status aborted)", original]
            )
        ]
    );
    // 17 sections: 8 supported, 2 more with entries, 7 with narrative only.
    assert_eq!(c.sections.len(), 17);
    let other: Vec<(&str, usize)> = c
        .sections
        .iter()
        .filter(|s| s.kind.is_none() && s.entries > 0)
        .map(|s| (s.title.as_str(), s.entries))
        .collect();
    assert_eq!(other, [("FAMILY HISTORY", 1), ("PLAN OF CARE", 4)]);
    assert_eq!(
        c.sections.iter().filter(|s| s.kind.is_none() && s.entries == 0).count(),
        7
    );

    let patient = resources(&c, "Patient")[0];
    assert_eq!(patient["birthDate"], "2005-05-01");
    assert_eq!(patient["name"][0]["given"], json!(["Isabella", "Isa"]));
    assert_eq!(patient["name"][0]["family"], "Jones");
    // the social security number stays in the file only
    assert!(!patient.to_string().contains("111-00-2330"), "{patient}");

    let pneumonia = resources(&c, "Condition")
        .into_iter()
        .find(|r| r["code"]["coding"][0]["code"] == "233604007")
        .ok_or_else(|| anyhow::anyhow!("no pneumonia"))?;
    assert_eq!(pneumonia["code"]["coding"][0]["system"], "http://snomed.info/sct");
    assert_eq!(pneumonia["code"]["text"], "Pneumonia");
    let hgb = resources(&c, "Observation")
        .into_iter()
        .find(|r| r["code"]["coding"][0]["code"] == "30313-1")
        .ok_or_else(|| anyhow::anyhow!("no HGB"))?;
    assert_eq!(hgb["code"]["coding"][0]["system"], "http://loinc.org");
    assert_eq!(hgb["valueQuantity"]["value"], json!(13.2));
    assert_eq!(hgb["valueQuantity"]["code"], "g/dl");
    assert_eq!(hgb["category"][0]["coding"][0]["code"], "laboratory");
    // the report names its three results
    let report = resources(&c, "DiagnosticReport")[0];
    assert_eq!(report["result"].as_array().map(Vec::len), Some(3));
    assert!(hgb["effectiveDateTime"].is_string(), "{hgb}");
    // one immunization was not given (negationInd)
    let not_done: Vec<_> = resources(&c, "Immunization")
        .into_iter()
        .filter(|r| r["status"] == "not-done")
        .collect();
    assert_eq!(not_done.len(), 1);
    // ids are unique in the file even where the sample repeats one C-CDA id
    let mut ids: Vec<String> = c
        .entries
        .iter()
        .map(|e| format!("{}/{}", e.resource["resourceType"], e.resource["id"]))
        .collect();
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n);
    Ok(())
}

#[test]
fn the_ccd_with_no_information_sections_reads_what_it_has() -> R {
    let c = convert(&fixture("ccd-2.xml"))?;
    assert_eq!(made(&c, Section::Allergies), counts(&[("AllergyIntolerance", 1)]));
    assert_eq!(made(&c, Section::Problems), counts(&[]));
    assert_eq!(made(&c, Section::Medications), counts(&[]));
    assert_eq!(
        made(&c, Section::Results),
        counts(&[("DiagnosticReport", 1), ("Observation", 1)])
    );
    assert_eq!(made(&c, Section::VitalSigns), counts(&[("Observation", 9)]));
    // "no known allergies": the IPS absent-unknown code, which the importer does not keep as a record
    let allergy = resources(&c, "AllergyIntolerance")[0];
    assert_eq!(
        allergy["code"]["coding"][0],
        json!({"system": ownhealth_ingest_structured::fhir::mapping::ABSENT_UNKNOWN,
            "code": "no-known-allergies", "display": "no known allergies"})
    );
    // the pending leukocyte count has no value yet
    let pending = resources(&c, "Observation")
        .into_iter()
        .find(|r| r["code"]["coding"][0]["code"] == "804-5")
        .ok_or_else(|| anyhow::anyhow!("no leukocytes"))?;
    assert_eq!(pending["status"], "preliminary");
    assert!(pending.get("valueQuantity").is_none(), "{pending}");
    assert_eq!(resources(&c, "DiagnosticReport")[0]["status"], "partial");
    let bmi = resources(&c, "Observation")
        .into_iter()
        .find(|r| r["code"]["coding"][0]["code"] == "39156-5")
        .ok_or_else(|| anyhow::anyhow!("no BMI"))?;
    assert_eq!(bmi["valueQuantity"]["value"], json!(37.58));
    assert_eq!(bmi["category"][0]["coding"][0]["code"], "vital-signs");
    assert_eq!(
        made(&c, Section::Procedures),
        counts(&[("Procedure", 2), ("Observation", 1)])
    );
    assert_eq!(made(&c, Section::SocialHistory), counts(&[("Observation", 1)]));
    let held = held_items(&c, "ccda-x", "ccd-2.xml");
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0]["section"]["code"], "47519-4");
    assert_eq!(
        held[0]["notes"],
        json!({"not read: code (original text kept in the uploaded file)": 2})
    );
    Ok(())
}

#[test]
fn negated_entries_never_become_plain_records() -> R {
    let absent = |c: &Converted, rtype: &str, key: &str| -> String {
        let r = resources(c, rtype);
        assert_eq!(r.len(), 1, "{rtype}");
        let concept = &r[0][key]["coding"][0];
        assert_eq!(
            concept["system"],
            ownhealth_ingest_structured::fhir::mapping::ABSENT_UNKNOWN
        );
        concept["code"].as_str().unwrap_or_default().to_string()
    };
    let c = convert(&section_file("allergies-none-known.xml"))?;
    assert_eq!(absent(&c, "AllergyIntolerance", "code"), "no-known-allergies");
    let c = convert(&section_file("problems-none-known.xml"))?;
    assert_eq!(absent(&c, "Condition", "code"), "no-known-problems");
    let c = convert(&section_file("medications-none.xml"))?;
    assert_eq!(
        absent(&c, "MedicationStatement", "medicationCodeableConcept"),
        "no-known-medications"
    );

    // not allergic to peanuts: no substance code, the peanut sits in the exposure-risk extension
    let c = convert(&section_file("allergies-not-peanuts.xml"))?;
    let peanut = resources(&c, "AllergyIntolerance")[0];
    assert!(peanut.get("code").is_none(), "{peanut}");
    assert_eq!(
        peanut["extension"][0]["extension"][0]["valueCodeableConcept"]["coding"][0]["code"],
        "762952008"
    );
    // does not have diabetes: refuted
    let c = convert(&section_file("problems-not-diabetes.xml"))?;
    let diabetes = resources(&c, "Condition")[0];
    assert_eq!(diabetes["code"]["coding"][0]["code"], "44054006");
    assert_eq!(diabetes["verificationStatus"]["coding"][0]["code"], "refuted");
    // a vaccine the patient refused: not-done, with the reason
    let c = convert(&section_file("immunization-refused.xml"))?;
    let refused = resources(&c, "Immunization")[0];
    assert_eq!(refused["status"], "not-done");
    assert_eq!(refused["statusReason"]["coding"][0]["code"], "PATOBJ");
    assert_eq!(refused["vaccineCode"]["coding"][0]["code"], "166");
    Ok(())
}

#[test]
fn a_result_below_a_limit_keeps_its_comparator() -> R {
    let c = convert(&section_file("results-less-than.xml"))?;
    let bnp = resources(&c, "Observation")[0];
    assert_eq!(
        bnp["valueQuantity"],
        json!({"value": 5, "unit": "pg/mL", "system": "http://unitsofmeasure.org", "code": "pg/mL", "comparator": "<="})
    );
    assert_eq!(bnp["referenceRange"][0]["high"]["value"], json!(100));
    Ok(())
}

#[test]
fn an_unknown_template_is_held_for_review_never_an_error() -> R {
    let c = convert(&document(
        r#"<component><section>
             <templateId root="1.2.3.4.5.6"/><title>Mystery</title><text>a b</text>
             <entry><observation classCode="OBS" moodCode="EVN"><templateId root="1.2.3.4.5.7"/>
               <code code="X1" codeSystem="1.2.3"/></observation></entry>
             <entry><act classCode="ACT" moodCode="EVN"><code code="X2" codeSystem="1.2.3"/></act></entry>
           </section></component>
           <component><section>
             <templateId root="2.16.840.1.113883.10.20.22.2.5.1"/><title>PROBLEMS</title>
             <entry><encounter classCode="ENC" moodCode="EVN"><templateId root="9.9.9"/></encounter></entry>
             <entry><act classCode="ACT" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.3"/></act></entry>
           </section></component>"#,
    ))?;
    assert_eq!(c.sections.len(), 2);
    assert_eq!(c.sections[0].kind, None);
    assert_eq!(c.sections[1].kind, Some(Section::Problems));
    assert_eq!(
        c.sections[1].not_read,
        [
            "encounter 9.9.9".to_string(),
            "act 2.16.840.1.113883.10.20.22.4.3".to_string()
        ]
    );
    let held = held_items(&c, "ccda-x", "mystery.xml");
    assert_eq!(held.len(), 2);
    assert_eq!(held[0]["resource_type"], "Mystery (C-CDA)");
    assert_eq!(held[0]["count"], 2);
    assert_eq!(held[0]["notes"], json!({"section 1.2.3.4.5.6 is not read yet": 2}));
    assert_eq!(held[1]["resource_type"], "PROBLEMS (C-CDA)");
    assert_eq!(held[1]["count"], 2);
    Ok(())
}

/// Every hostile or broken file is refused as `Error::Invalid`, with a reason, and nothing panics.
#[test]
fn malformed_hostile_or_huge_xml_is_refused_without_a_panic() {
    let laughs = r#"<?xml version="1.0"?>
<!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">
<!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">]>
<ClinicalDocument xmlns="urn:hl7-org:v3"><title>&lol3;</title></ClinicalDocument>"#;
    let external = r#"<?xml version="1.0"?>
<!DOCTYPE ClinicalDocument [<!ENTITY x SYSTEM "file:///etc/passwd">]>
<ClinicalDocument xmlns="urn:hl7-org:v3"><title>&x;</title></ClinicalDocument>"#;
    let deep = format!(
        r#"<ClinicalDocument xmlns="urn:hl7-org:v3">{}{}</ClinicalDocument>"#,
        "<a>".repeat(200_000),
        "</a>".repeat(200_000)
    );
    let many = format!(
        r#"<ClinicalDocument xmlns="urn:hl7-org:v3">{}</ClinicalDocument>"#,
        "<a/>".repeat(ccda::xml::MAX_NODES as usize + 10)
    );
    let mut huge = b"<ClinicalDocument xmlns=\"urn:hl7-org:v3\">".to_vec();
    huge.resize(ccda::xml::MAX_BYTES + 1, b' ');
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("billion laughs", laughs.into(), "DOCTYPE"),
        ("external entity", external.into(), "DOCTYPE"),
        (
            "undeclared entity",
            br#"<ClinicalDocument xmlns="urn:hl7-org:v3">&x;</ClinicalDocument>"#.to_vec(),
            "not well formed",
        ),
        ("deep nesting", deep.into_bytes(), "nests deeper"),
        ("too many nodes", many.into_bytes(), "nodes"),
        ("too large", huge, "MB"),
        (
            "unclosed",
            br#"<ClinicalDocument xmlns="urn:hl7-org:v3"><title>x</ClinicalDocument>"#.to_vec(),
            "not well formed",
        ),
        ("truncated", fixture("ccd-2.xml")[..20_000].to_vec(), "not well formed"),
        ("empty", Vec::new(), "not well formed"),
        ("binary", vec![0u8, 159, 146, 150, 0, 1], "not well formed"),
        (
            "another root",
            br#"<Bundle xmlns="urn:hl7-org:v3"/>"#.to_vec(),
            "ClinicalDocument",
        ),
        (
            "no namespace",
            b"<ClinicalDocument><title>x</title></ClinicalDocument>".to_vec(),
            "ClinicalDocument",
        ),
    ];
    for (name, bytes, why) in cases {
        match std::panic::catch_unwind(|| convert(&bytes)) {
            Ok(Err(Error::Invalid(m))) => assert!(m.contains(why), "{name}: {m}"),
            Ok(other) => panic!("{name}: {other:?}"),
            Err(_) => panic!("{name}: the converter panicked"),
        }
    }
}

// ------------------------------------------------------------------ import ----

async fn open_items(conn: &mut PgConnection, doc: Uuid, kind: &str) -> anyhow::Result<Vec<review::ReviewItem>> {
    Ok(review::list_items(
        conn,
        &ItemQuery {
            document_id: Some(doc),
            status: Some("open".into()),
            kind: Some(kind.into()),
            ..ItemQuery::default()
        },
    )
    .await?)
}

async fn count(conn: &mut PgConnection, table: &str, profile: Uuid) -> anyhow::Result<i64> {
    let sql = format!("select count(*) from {table} where profile_id = $1");
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(profile)
        .fetch_one(conn)
        .await?)
}

#[tokio::test]
async fn the_history_and_physical_imports_into_the_matching_profile_and_holds_the_rest() -> R {
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    let p = fam.profile("lucia");
    sqlx::query("update profile set display_name = 'Isabella Jones', birth_date = '2005-05-01' where id = $1")
        .bind(p)
        .execute(&mut *tx)
        .await?;
    let bytes = fixture("history-and-physical.xml");
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &bytes, "hp.xml", ccda::MIME).await?;
    let s = import_ccda(&mut tx, &env.blobs, doc).await?;
    // The conditions, allergies, medicine, completed procedures and given vaccines are records.
    // The refused vaccine waits in review.
    assert_eq!(count(&mut tx, "condition", p).await?, 2);
    assert_eq!(count(&mut tx, "allergy", p).await?, 3);
    assert_eq!(count(&mut tx, "medication_statement", p).await?, 1);
    assert_eq!(count(&mut tx, "immunization", p).await?, 3);
    assert_eq!(count(&mut tx, "procedure", p).await?, 2);
    let held = open_items(&mut tx, doc, "fhir_unmapped_summary").await?;
    let mut types: Vec<String> = held
        .iter()
        .map(|h| h.payload["resource_type"].as_str().unwrap_or_default().to_string())
        .collect();
    types.sort();
    assert_eq!(
        types,
        [
            "ALLERGIES, ADVERSE REACTIONS, ALERTS (C-CDA)",
            "C-CDA narrative section",
            "FAMILY HISTORY (C-CDA)",
            "IMMUNIZATIONS (C-CDA)",
            "MEDICATIONS (C-CDA)",
            "PLAN OF CARE (C-CDA)",
            "PROCEDURES (C-CDA)"
        ]
    );
    let values = count(&mut tx, "observation", p).await?;
    let rows = open_items(&mut tx, doc, "candidate_row").await?.len() as i64;
    assert_eq!(
        values + rows,
        12,
        "3 labs, 6 vital signs and 3 social history observations"
    );
    // Grouped items account for unread parts, including the aborted procedure observation.
    assert_eq!(s.skipped, 0, "{s:?}");
    assert_eq!(s.accepted + s.queued, 12 + 12 + 7, "{s:?}");
    let flags: Value = sqlx::query_scalar("select flags from document where id = $1")
        .bind(doc)
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(flags["ccda"]["sections"].as_array().map(Vec::len), Some(17));
    assert_eq!(flags["import"]["accepted"], json!(s.accepted));
    // a second run reads the stored summary and adds nothing
    let again = import_ccda(&mut tx, &env.blobs, doc).await?;
    assert_eq!(again, s);
    assert_eq!(open_items(&mut tx, doc, "fhir_unmapped_summary").await?.len(), 7);
    Ok(())
}

#[tokio::test]
async fn a_document_for_someone_else_is_held_whole() -> R {
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    let p = fam.profile("carmen");
    let doc = store_doc(
        &mut tx,
        &env.blobs,
        &fam,
        "carmen",
        &fixture("history-and-physical.xml"),
        "hp.xml",
        ccda::MIME,
    )
    .await?;
    let s = import_ccda(&mut tx, &env.blobs, doc).await?;
    assert_eq!(s.accepted, 0, "{s:?}");
    assert_eq!(count(&mut tx, "condition", p).await?, 0);
    Ok(())
}

fn job(payload: Value) -> Job {
    Job {
        id: 0,
        queue: "core".into(),
        kind: ccda::JOB.into(),
        payload,
        attempts: 1,
        max_attempts: 5,
        dedupe_key: None,
    }
}

#[tokio::test]
async fn an_uploaded_ccd_is_sniffed_and_imported_by_its_job_and_a_broken_one_fails_for_good() -> R {
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    let p = fam.profile("lucia");
    let bytes = fixture("ccd-2.xml");
    assert_eq!(sniff("record.xml", &bytes[..4096], None), "ccda");
    let sha = env
        .blobs
        .put_bytes(ownhealth_core::blobs::BlobOwner::Profile(p), &bytes)?;
    let out = dispatch_upload(
        &mut tx,
        &env.blobs,
        fam.household_id,
        p,
        &[StoredFile {
            filename: "record.xml".into(),
            sha256: sha.clone(),
            size: bytes.len() as i64,
        }],
        None,
    )
    .await?;
    assert_eq!(out["items"][0]["kind"], "ccda");
    let queued: Vec<(String, Value)> =
        sqlx::query_as("select kind, payload from job where payload->>'blob_sha256' = $1")
            .bind(&sha)
            .fetch_all(&mut *tx)
            .await?;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].0, ccda::JOB);
    tx.commit().await?;

    let settings = Arc::new(env.db.settings(env.dir.path())?);
    let ctx = JobContext::new(env.db.pool.clone(), settings, env.blobs.clone());
    let result = ccda::import_ccda_job(ctx.clone(), job(queued[0].1.clone())).await?;
    let result = result.ok_or_else(|| anyhow::anyhow!("no result"))?;
    let doc = Uuid::parse_str(result["document_id"].as_str().unwrap_or_default())?;
    let (mime, channel): (String, String) = sqlx::query_as(
        "select d.mime, s.channel from document d join document_source s on s.document_id = d.id where d.id = $1",
    )
    .bind(doc)
    .fetch_one(&env.db.pool)
    .await?;
    assert_eq!((mime.as_str(), channel.as_str()), (ccda::MIME, "ccda"));

    // the same root, then broken XML: a retry reads the same bytes, so it fails for good
    let broken = b"<?xml version=\"1.0\"?><ClinicalDocument xmlns=\"urn:hl7-org:v3\"><title>cut".to_vec();
    assert_eq!(sniff("cut.xml", &broken, None), "ccda");
    let sha = env
        .blobs
        .put_bytes(ownhealth_core::blobs::BlobOwner::Profile(p), &broken)?;
    let payload = json!({"profile_id": p.to_string(), "household_id": fam.household_id.to_string(),
        "blob_sha256": sha, "filename": "cut.xml"});
    let Err(e) = ccda::import_ccda_job(ctx, job(payload)).await else {
        panic!("a broken C-CDA file imported");
    };
    assert!(e.permanent, "{e}");
    assert!(e.to_string().contains("not a readable C-CDA document"), "{e}");
    Ok(())
}

// ------------------------------------------------------------- round 1 ----

/// A problem act around one Problem Observation with `value`, `times` (the observation's
/// effectiveTime children) and `extra` (more children of the observation).
fn problem_entry(id: &str, value: &str, times: &str, extra: &str) -> String {
    format!(
        r#"<entry><act classCode="ACT" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.3"/>
          <id root="1.2.3.4" extension="act-{id}"/>
          <entryRelationship typeCode="SUBJ"><observation classCode="OBS" moodCode="EVN">
            <templateId root="2.16.840.1.113883.10.20.22.4.4"/><id root="1.2.3.4" extension="obs-{id}"/>
            <code code="55607006" codeSystem="2.16.840.1.113883.6.96"/>
            <effectiveTime>{times}</effectiveTime>
            <value xsi:type="CD" codeSystem="2.16.840.1.113883.6.96" {value}/>{extra}
          </observation></entryRelationship></act></entry>"#
    )
}

fn problems_section(entries: &str) -> String {
    format!(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.5.1"/><title>PROBLEMS</title>{entries}</section></component>"#
    )
}

/// A DS4P Security Observation whose value is `code` (v3 ActCode).
fn security_label(code: &str) -> String {
    format!(
        r#"<observation classCode="OBS" moodCode="EVN"><templateId root="2.16.840.1.113883.3.3445.21"/>
          <code code="SECCLASSOBS" codeSystem="2.16.840.1.113883.5.4"/>
          <value xsi:type="CE" code="{code}" codeSystem="2.16.840.1.113883.5.4"/></observation>"#
    )
}

async fn as_isabella(conn: &mut PgConnection, fam: &ownhealth_core::demo::Family, slug: &str) -> anyhow::Result<Uuid> {
    let p = fam.profile(slug);
    sqlx::query("update profile set display_name = 'Isabella Jones', birth_date = '2005-05-01' where id = $1")
        .bind(p)
        .execute(&mut *conn)
        .await?;
    Ok(p)
}

/// C-F1: a narrative that many entries point at is read once, and only up to its first characters.
#[test]
fn a_large_narrative_referenced_many_times_converts_in_bounded_time() -> R {
    let narrative = "word ".repeat(4 * 1024 * 1024 / 5);
    let entry = r##"<entry><observation classCode="OBS" moodCode="EVN"><code><originalText><reference value="#n"/></originalText></code></observation></entry>"##;
    let body = format!(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.3.1"/><title>RESULTS</title>
          <text><paragraph ID="n">{narrative}</paragraph></text>{}</section></component>"#,
        entry.repeat(500)
    );
    let bytes = document(&body);
    let started = std::time::Instant::now();
    let c = convert(&bytes)?;
    let took = started.elapsed();
    let observations = resources(&c, "Observation");
    assert_eq!(observations.len(), 500);
    assert!(
        observations
            .iter()
            .all(|o| o["code"]["text"].as_str().is_some_and(|t| t.chars().count() <= 2_000))
    );
    // each reference used to walk the whole 4 MB text node: about 90 ms each in a debug build
    assert!(took < std::time::Duration::from_secs(5), "{took:?}");
    Ok(())
}

/// C-F2: the social security number gives no id, wherever the patientRole lists it.
#[test]
fn no_id_derives_from_the_social_security_number() -> R {
    use sha2::{Digest, Sha256};
    let ssn = r#"<id root="2.16.840.1.113883.4.1" extension="111-00-2330"/>"#;
    let mrn = r#"<id root="2.16.840.1.113883.19.5.99999.2" extension="998991"/>"#;
    let hash = hex::encode(Sha256::digest(b"2.16.840.1.113883.4.1|111-00-2330"));
    let body = problems_section(&problem_entry(
        "p",
        r#"code="233604007" displayName="Pneumonia""#,
        "",
        "",
    ));
    let patient_id = |c: &Converted| resources(c, "Patient")[0]["id"].clone();
    let plain = convert(&document_with(mrn, &body))?;
    for ids in [format!("{ssn}{mrn}"), ssn.to_string()] {
        let c = convert(&document_with(&ids, &body))?;
        for e in &c.entries {
            let text = e.resource.to_string();
            assert!(
                !text.contains(&hash[..16]) && !text.contains("111-00-2330"),
                "{ids}: {text}"
            );
        }
        if ids.contains("998991") {
            // the other id names the patient, as in a file without the number
            assert_eq!(patient_id(&c), patient_id(&plain));
        }
    }
    Ok(())
}

/// C-F4: a core section that holds only narrative (Medications as a table, no entries) is listed
/// with the other narrative-only sections; one that says "no information" (nullFlavor) is not.
#[test]
fn a_core_section_with_only_narrative_is_held_with_the_narrative() -> R {
    let c = convert(&document(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.1"/>
             <code code="10160-0" codeSystem="2.16.840.1.113883.6.1"/><title>MEDICATIONS</title>
             <text><table><tr><td>Metformin 500 mg twice a day</td></tr><tr><td>Lisinopril 10 mg</td></tr></table></text>
           </section></component>
           <component><section nullFlavor="NI"><templateId root="2.16.840.1.113883.10.20.22.2.5.1"/>
             <title>PROBLEMS</title><text>No information</text></section></component>
           <component><section><templateId root="1.2.3.9"/><title>NOTES</title><text>free note</text></section></component>"#,
    ))?;
    let held = held_items(&c, "ccda-x", "meds.xml");
    assert_eq!(held.len(), 1, "{}", json!(held));
    assert_eq!(held[0]["resource_type"], ccda::NARRATIVE);
    assert_eq!(
        held[0]["notes"],
        json!({"MEDICATIONS (2.16.840.1.113883.10.20.22.2.1)": 1, "NOTES (1.2.3.9)": 1})
    );
    assert_eq!(held[0]["count"], 2);
    Ok(())
}

/// C-F5: "did not take aspirin" is never a medicine taken: no row, one item in review.
#[tokio::test]
async fn a_medicine_not_taken_is_never_recorded_as_taken() -> R {
    let body = r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.1.1"/><title>MEDICATIONS</title>
      <entry><substanceAdministration classCode="SBADM" moodCode="EVN" negationInd="true">
        <templateId root="2.16.840.1.113883.10.20.22.4.16"/><id root="1.2.3.4" extension="med-neg-1"/>
        <statusCode code="completed"/><effectiveTime xsi:type="IVL_TS"><low value="20230101"/></effectiveTime>
        <consumable><manufacturedProduct><templateId root="2.16.840.1.113883.10.20.22.4.23"/>
          <manufacturedMaterial><code code="1191" codeSystem="2.16.840.1.113883.6.88" displayName="Aspirin"/></manufacturedMaterial>
        </manufacturedProduct></consumable></substanceAdministration></entry>
    </section></component>"#;
    let bytes = document(body);
    let c = convert(&bytes)?;
    let aspirin = resources(&c, "MedicationStatement")[0];
    assert_eq!(aspirin["status"], "not-taken");
    assert_eq!(aspirin["medicationCodeableConcept"]["coding"][0]["code"], "1191");
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    let p = as_isabella(&mut tx, &fam, "lucia").await?;
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &bytes, "neg.xml", ccda::MIME).await?;
    let s = import_ccda(&mut tx, &env.blobs, doc).await?;
    assert_eq!((s.accepted, s.queued), (0, 1), "{s:?}");
    assert_eq!(count(&mut tx, "medication_statement", p).await?, 0);
    Ok(())
}

/// C-F7: a problem whose observation has an end date and no status observation is resolved.
#[test]
fn a_problem_with_an_end_date_is_resolved() -> R {
    let c = convert(&document(&problems_section(&format!(
        "{}{}",
        problem_entry(
            "ended",
            r#"code="233604007" displayName="Pneumonia""#,
            r#"<low value="20200110"/><high value="20200301"/>"#,
            ""
        ),
        problem_entry(
            "open",
            r#"code="38341003" displayName="Hypertension""#,
            r#"<low value="20190101"/>"#,
            ""
        )
    ))))?;
    let conditions = resources(&c, "Condition");
    let status: Vec<(&str, &str)> = conditions
        .iter()
        .map(|r| {
            (
                r["code"]["coding"][0]["code"].as_str().unwrap_or_default(),
                r["clinicalStatus"]["coding"][0]["code"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(status, [("233604007", "resolved"), ("38341003", "active")]);
    assert_eq!(conditions[0]["abatementDateTime"], "2020-03-01");
    Ok(())
}

/// C-F7: a Medication Activity in INT mood is a MedicationRequest (an order), not a medicine taken.
#[test]
fn an_intended_medicine_is_a_request() -> R {
    let c = convert(&document(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.1.1"/><title>MEDICATIONS</title>
          <entry><substanceAdministration classCode="SBADM" moodCode="INT">
            <templateId root="2.16.840.1.113883.10.20.22.4.16"/><id root="1.2.3.4" extension="med-int-1"/>
            <statusCode code="active"/>
            <consumable><manufacturedProduct><manufacturedMaterial>
              <code code="860975" codeSystem="2.16.840.1.113883.6.88" displayName="Metformin 500 MG Oral Tablet"/>
            </manufacturedMaterial></manufacturedProduct></consumable></substanceAdministration></entry>
        </section></component>"#,
    ))?;
    assert_eq!(made(&c, Section::Medications), counts(&[("MedicationRequest", 1)]));
    let order = resources(&c, "MedicationRequest")[0];
    assert_eq!(
        (&order["intent"], &order["status"]),
        (&json!("order"), &json!("active"))
    );
    Ok(())
}

/// C-F7: 256 levels of elements are read, 257 are refused.
#[test]
fn nesting_is_refused_one_level_past_the_limit() -> R {
    // `levels` elements deep, the ClinicalDocument root being the first
    let nested = |levels: usize| {
        format!(
            r#"<ClinicalDocument xmlns="urn:hl7-org:v3">{}{}</ClinicalDocument>"#,
            "<a>".repeat(levels - 1),
            "</a>".repeat(levels - 1)
        )
        .into_bytes()
    };
    assert_eq!(ccda::xml::MAX_DEPTH, 256);
    convert(&nested(256))?;
    match convert(&nested(257)) {
        Err(Error::Invalid(m)) => assert!(m.contains("nests deeper"), "{m}"),
        other => panic!("257 levels: {other:?}"),
    }
    Ok(())
}

/// C-F3: a stored file over the limit is refused by the import, as the converter refuses it.
#[tokio::test]
async fn an_oversized_stored_file_is_refused_by_the_import() -> R {
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    let mut huge = b"<ClinicalDocument xmlns=\"urn:hl7-org:v3\">".to_vec();
    huge.resize(ccda::xml::MAX_BYTES + 1, b' ');
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &huge, "huge.xml", ccda::MIME).await?;
    match import_ccda(&mut tx, &env.blobs, doc).await {
        Err(Error::Invalid(m)) => assert!(m.contains("MB"), "{m}"),
        other => panic!("{other:?}"),
    }
    Ok(())
}

/// C-F10: the import holds its document row from its first read, so a move waits for the import to
/// commit instead of slipping in while it converts.
#[tokio::test]
async fn the_import_locks_its_document_before_it_reads_the_profile() -> R {
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    as_isabella(&mut tx, &fam, "lucia").await?;
    let bytes = document(&problems_section(&problem_entry(
        "p",
        r#"code="233604007" displayName="Pneumonia""#,
        "",
        "",
    )));
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &bytes, "p.xml", ccda::MIME).await?;
    tx.commit().await?;
    // the profile table is held, so the import stops at its profile read
    let mut gate = env.tx().await?;
    sqlx::query("lock table profile in access exclusive mode")
        .execute(&mut *gate)
        .await?;
    let (pool, blobs) = (env.db.pool.clone(), env.blobs.clone());
    let run = tokio::spawn(async move {
        let mut t = pool.begin().await?;
        let s = import_ccda(&mut t, &blobs, doc).await?;
        t.commit().await?;
        anyhow::Ok(s)
    });
    let mut waiting = 0i64;
    for _ in 0..200 {
        waiting = sqlx::query_scalar(
            "select count(*) from pg_locks l join pg_class c on c.oid = l.relation \
             where c.relname = 'profile' and not l.granted",
        )
        .fetch_one(&env.db.pool)
        .await?;
        if waiting > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(waiting, 1, "the import never reached its profile read");
    let mut mover = env.tx().await?;
    sqlx::query("set local lock_timeout = '500ms'")
        .execute(&mut *mover)
        .await?;
    let moved = sqlx::query("update document set flags = flags where id = $1")
        .bind(doc)
        .execute(&mut *mover)
        .await;
    mover.rollback().await?;
    gate.rollback().await?;
    let s = run.await??;
    assert_eq!(s.accepted, 1, "{s:?}");
    let e = moved
        .err()
        .ok_or_else(|| anyhow::anyhow!("the document row was free while the import ran"))?;
    assert!(e.to_string().contains("lock timeout"), "{e}");
    Ok(())
}

/// U-F1: a DS4P security label (an entry's Security Observation, a section's) or a confidentiality
/// code reaches meta.security, so the record is as sensitive as the same label in a FHIR file.
#[tokio::test]
async fn ds4p_security_labels_make_the_records_sensitive() -> R {
    let hiv = problem_entry(
        "hiv",
        r#"code="86406008" displayName="Human immunodeficiency virus infection""#,
        r#"<low value="20200110"/>"#,
        &format!(
            r#"<entryRelationship typeCode="COMP">{}</entryRelationship>"#,
            security_label("HIV")
        ),
    );
    let flu = problem_entry(
        "flu",
        r#"code="6142004" displayName="Influenza""#,
        r#"<low value="20200110"/>"#,
        "",
    );
    let depression = problem_entry(
        "dep",
        r#"code="35489007" displayName="Depressive disorder""#,
        r#"<low value="20200110"/>"#,
        "",
    );
    let body = format!(
        r#"{}<component><section><templateId root="2.16.840.1.113883.10.20.22.2.5.1"/>
             <templateId root="2.16.840.1.113883.3.3445.3"/><title>PROBLEMS (restricted)</title>
             <confidentialityCode code="R" codeSystem="2.16.840.1.113883.5.25"/>
             <entry>{}</entry>{depression}</section></component>"#,
        problems_section(&format!("{hiv}{flu}")),
        security_label("PSY")
    );
    let bytes = document(&body);
    let c = convert(&bytes)?;
    let labels: BTreeMap<String, Vec<String>> = resources(&c, "Condition")
        .iter()
        .map(|r| {
            let codes = r["meta"]["security"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s["code"].as_str().map(String::from))
                .collect();
            (
                r["code"]["coding"][0]["code"].as_str().unwrap_or_default().to_string(),
                codes,
            )
        })
        .collect();
    // the section's own label is no problem of its own
    assert_eq!(
        labels,
        BTreeMap::from([
            ("86406008".to_string(), vec!["HIV".to_string()]),
            ("6142004".to_string(), vec![]),
            ("35489007".to_string(), vec!["R".to_string(), "PSY".to_string()]),
        ])
    );
    assert!(c.sections.iter().all(|s| s.not_read.is_empty()), "{:?}", c.sections);

    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    let p = as_isabella(&mut tx, &fam, "lucia").await?;
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &bytes, "ds4p.xml", ccda::MIME).await?;
    import_ccda(&mut tx, &env.blobs, doc).await?;
    let rows: Vec<(String, Vec<String>)> =
        sqlx::query_as("select fhir->'code'->'coding'->0->>'code', sensitivity from condition where profile_id = $1")
            .bind(p)
            .fetch_all(&mut *tx)
            .await?;
    let rows: BTreeMap<String, Vec<String>> = rows.into_iter().collect();
    assert_eq!(
        rows,
        BTreeMap::from([
            ("86406008".to_string(), vec!["sexual_health".to_string()]),
            ("6142004".to_string(), vec![]),
            ("35489007".to_string(), vec!["mental_health".to_string()]),
        ])
    );
    assert!(ownhealth_core::documents::document_is_sensitive(&mut tx, doc).await?);
    Ok(())
}

/// R1-H1: a DS4P label in the document's Privacy Markings Section (templateId
/// 2.16.840.1.113883.3.3445.4) labels the whole document, so every record made from it is sensitive,
/// even one that carries no label of its own.
#[tokio::test]
async fn a_privacy_markings_section_labels_every_record_of_the_document() -> R {
    let body = format!(
        r#"<component><section><templateId root="2.16.840.1.113883.3.3445.4"/><title>Privacy Markings</title>
             <text>This document contains HIV and mental health information</text><entry>{}</entry><entry>{}</entry>
           </section></component>{}"#,
        security_label("HIV"),
        security_label("PSY"),
        problems_section(&problem_entry(
            "hiv",
            r#"code="86406008" displayName="Human immunodeficiency virus infection""#,
            r#"<low value="20200110"/>"#,
            "",
        ))
    );
    let bytes = document(&body);
    let c = convert(&bytes)?;
    let labels: Vec<Vec<String>> = resources(&c, "Condition")
        .iter()
        .map(|r| {
            r["meta"]["security"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s["code"].as_str().map(String::from))
                .collect()
        })
        .collect();
    // every entry of the section labels the document, not only the first
    assert_eq!(labels, vec![vec!["HIV".to_string(), "PSY".to_string()]]);

    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    let p = as_isabella(&mut tx, &fam, "lucia").await?;
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &bytes, "ds4p-doc.xml", ccda::MIME).await?;
    import_ccda(&mut tx, &env.blobs, doc).await?;
    let rows: Vec<(String, Vec<String>)> =
        sqlx::query_as("select fhir->'code'->'coding'->0->>'code', sensitivity from condition where profile_id = $1")
            .bind(p)
            .fetch_all(&mut *tx)
            .await?;
    assert_eq!(
        rows,
        vec![(
            "86406008".to_string(),
            vec!["mental_health".to_string(), "sexual_health".to_string()]
        )]
    );
    Ok(())
}

/// U-F2: what no reader read (sections and parts left for review, narrative-only sections) keeps the
/// stored file behind read:sensitive, as a connector pull's unprojected resources do. A file read in
/// full stays open.
#[tokio::test]
async fn a_ccd_with_unread_sections_keeps_its_file_closed() -> R {
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    as_isabella(&mut tx, &fam, "lucia").await?;
    let hp = fixture("history-and-physical.xml");
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &hp, "hp.xml", ccda::MIME).await?;
    import_ccda(&mut tx, &env.blobs, doc).await?;
    let held: i64 = open_items(&mut tx, doc, "fhir_unmapped_summary")
        .await?
        .iter()
        .map(|i| i.payload["count"].as_i64().unwrap_or(0))
        .sum();
    let flags: Value = sqlx::query_scalar("select flags from document where id = $1")
        .bind(doc)
        .fetch_one(&mut *tx)
        .await?;
    assert!(held > 0);
    assert_eq!(flags["unprojected"], json!(held));
    assert!(ownhealth_core::documents::document_is_sensitive(&mut tx, doc).await?);

    let read = document(&problems_section(&problem_entry(
        "p",
        r#"code="233604007" displayName="Pneumonia""#,
        "",
        "",
    )));
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &read, "p.xml", ccda::MIME).await?;
    import_ccda(&mut tx, &env.blobs, doc).await?;
    let flags: Value = sqlx::query_scalar("select flags from document where id = $1")
        .bind(doc)
        .fetch_one(&mut *tx)
        .await?;
    assert!(flags.get("unprojected").is_none(), "{flags}");
    assert!(!ownhealth_core::documents::document_is_sensitive(&mut tx, doc).await?);
    Ok(())
}

/// U-F6: HL7 CCD 1 puts a Severity Observation on an allergy concern act. It is no allergy of its
/// own; a severity the converter cannot map is listed for review.
#[test]
fn the_ccd_1_sample_reads_each_allergy_once() -> R {
    let c = convert(&fixture("ccd-1.xml"))?;
    assert_eq!(made(&c, Section::Allergies), counts(&[("AllergyIntolerance", 2)]));
    let substances: Vec<&str> = resources(&c, "AllergyIntolerance")
        .iter()
        .map(|r| r["code"]["coding"][0]["code"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(substances, ["70618", "2670"]);
    let allergies = c
        .sections
        .iter()
        .find(|s| s.kind == Some(Section::Allergies))
        .ok_or_else(|| anyhow::anyhow!("no allergies section"))?;
    // "Moderate to severe" has no FHIR severity
    assert_eq!(allergies.not_read, ["observation 2.16.840.1.113883.10.20.22.4.8"]);
    Ok(())
}

// ------------------------------------------------------------- round 3 ----

/// CR1: a CDA document whose body is a `nonXMLBody` (a scan or an attached file, never coded) is
/// refused with its own reason. It is never imported as a document that holds nothing, which would
/// leave its text unread and unflagged. A structured body still reads.
#[test]
fn an_unstructured_body_is_refused_never_read_as_empty() -> R {
    let structured = String::from_utf8(document(""))?;
    let unstructured = structured.replace(
        "<component><structuredBody></structuredBody></component>",
        r#"<component><nonXMLBody><text mediaType="text/plain">HIV test positive, see the attached letter.</text></nonXMLBody></component>"#,
    );
    assert_ne!(structured, unstructured);
    match convert(unstructured.as_bytes()) {
        Err(Error::Invalid(m)) => assert!(m.contains("nonXMLBody"), "{m}"),
        other => panic!("a nonXMLBody was read: {other:?}"),
    }
    convert(structured.as_bytes())?;
    Ok(())
}

/// CR5: a C-CDA document saved as UTF-16 (either byte order) reaches the importer, which refuses it
/// with its own reason (save it as UTF-8) instead of an unknown file.
#[test]
fn a_utf16_document_is_refused_with_its_own_reason() -> R {
    let text = String::from_utf8(fixture("ccd-2.xml"))?.replacen("encoding=\"UTF-8\"", "encoding=\"UTF-16\"", 1);
    for le in [true, false] {
        let bom: &[u8] = if le { b"\xff\xfe" } else { b"\xfe\xff" };
        let bytes: Vec<u8> = bom
            .iter()
            .copied()
            .chain(
                text.encode_utf16()
                    .flat_map(|u| if le { u.to_le_bytes() } else { u.to_be_bytes() }),
            )
            .collect();
        assert_eq!(sniff("record.xml", &bytes[..4096], None), "ccda", "le {le}");
        match convert(&bytes) {
            Err(Error::Invalid(m)) => assert!(m.contains("UTF-16") && m.contains("UTF-8"), "{m}"),
            other => panic!("le {le}: {other:?}"),
        }
    }
    Ok(())
}

/// CR6: CCD 1's first medicine has a supply order and a dispense; neither is read, so both are listed
/// for review. The MedicationStatements stay.
#[test]
fn medication_supply_and_dispense_are_listed_as_not_read() -> R {
    let c = convert(&fixture("ccd-1.xml"))?;
    assert_eq!(made(&c, Section::Medications), counts(&[("MedicationStatement", 2)]));
    let meds = c
        .sections
        .iter()
        .find(|s| s.kind == Some(Section::Medications))
        .ok_or_else(|| anyhow::anyhow!("no medications section"))?;
    assert_eq!(
        meds.not_read,
        [
            "supply 2.16.840.1.113883.10.20.22.4.17",
            "supply 2.16.840.1.113883.10.20.22.4.18"
        ]
    );
    Ok(())
}

/// A Problem Status Observation saying `code` (SNOMED: 55561003 active, 413322009 resolved).
fn problem_status(code: &str) -> String {
    format!(
        r#"<entryRelationship typeCode="REFR"><observation classCode="OBS" moodCode="EVN">
          <templateId root="2.16.840.1.113883.10.20.22.4.6"/><code code="33999-4" codeSystem="2.16.840.1.113883.6.1"/>
          <statusCode code="completed"/><value xsi:type="CD" code="{code}" codeSystem="2.16.840.1.113883.6.96"/>
        </observation></entryRelationship>"#
    )
}

/// An allergy concern act (status `act_status`) around one penicillin allergy.
fn allergy_entry(id: &str, act_status: &str) -> String {
    format!(
        r#"<entry><act classCode="ACT" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.30"/>
          <id root="1.2.3.4" extension="alg-act-{id}"/><code code="CONC" codeSystem="2.16.840.1.113883.5.6"/>
          <statusCode code="{act_status}"/>
          <entryRelationship typeCode="SUBJ"><observation classCode="OBS" moodCode="EVN">
            <templateId root="2.16.840.1.113883.10.20.22.4.7"/><id root="1.2.3.4" extension="alg-{id}"/>
            <code code="ASSERTION" codeSystem="2.16.840.1.113883.5.4"/><statusCode code="completed"/>
            <effectiveTime><low value="20200101"/></effectiveTime>
            <value xsi:type="CD" code="419511003" codeSystem="2.16.840.1.113883.6.96"/>
            <participant typeCode="CSM"><participantRole classCode="MANU"><playingEntity classCode="MMAT">
              <code code="7980" codeSystem="2.16.840.1.113883.6.88" displayName="Penicillin G"/>
            </playingEntity></participantRole></participant>
          </observation></entryRelationship></act></entry>"#
    )
}

/// CR7: FHIR R4 con-5 and ait-2 (no clinicalStatus on an entered-in-error record) and con-4 (an
/// abated condition is never active). A problem whose status observation says active while its
/// effectiveTime has an end is held for review with that reason; one that says resolved keeps its end.
#[test]
fn entered_in_error_has_no_clinical_status_and_an_active_problem_with_an_end_is_held() -> R {
    let c = convert(&document(&format!(
        r#"{}<component><section><templateId root="2.16.840.1.113883.10.20.22.2.6.1"/><title>ALLERGIES</title>{}{}</section></component>"#,
        problems_section(&format!(
            "{}{}{}",
            problem_entry(
                "void",
                r#"code="233604007" displayName="Pneumonia""#,
                r#"<low value="20200110"/><high value="20200301"/>"#,
                r#"<statusCode code="nullified"/>"#
            ),
            problem_entry(
                "conflict",
                r#"code="38341003" displayName="Hypertension""#,
                r#"<low value="20190101"/><high value="20200301"/>"#,
                &problem_status("55561003")
            ),
            problem_entry(
                "ended",
                r#"code="195967001" displayName="Asthma""#,
                r#"<low value="20100101"/><high value="20200301"/>"#,
                &problem_status("413322009")
            )
        )),
        allergy_entry("void", "nullified"),
        allergy_entry("kept", "active")
    )))?;
    let conditions: Vec<(&str, &Value, Option<&Value>, &Value)> = resources(&c, "Condition")
        .into_iter()
        .map(|r| {
            (
                r["code"]["coding"][0]["code"].as_str().unwrap_or_default(),
                &r["verificationStatus"]["coding"][0]["code"],
                r.get("clinicalStatus"),
                &r["abatementDateTime"],
            )
        })
        .collect();
    assert_eq!(
        conditions,
        [
            ("233604007", &json!("entered-in-error"), None, &Value::Null),
            (
                "195967001",
                &json!("confirmed"),
                Some(
                    &json!({"coding": [{"system": "http://terminology.hl7.org/CodeSystem/condition-clinical", "code": "resolved"}]})
                ),
                &json!("2020-03-01")
            ),
        ]
    );
    let problems = c
        .sections
        .iter()
        .find(|s| s.kind == Some(Section::Problems))
        .ok_or_else(|| anyhow::anyhow!("no problems section"))?;
    assert_eq!(
        problems.not_read,
        ["observation 2.16.840.1.113883.10.20.22.4.4 (status active with an end date)"]
    );
    let allergies: Vec<(&Value, bool)> = resources(&c, "AllergyIntolerance")
        .into_iter()
        .map(|r| {
            (
                &r["verificationStatus"]["coding"][0]["code"],
                r.get("clinicalStatus").is_some(),
            )
        })
        .collect();
    assert_eq!(
        allergies,
        [(&json!("entered-in-error"), false), (&json!("confirmed"), true)]
    );
    Ok(())
}

/// XML text with `&`, `<`, `>` and `"` escaped.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every string inside a JSON value.
fn strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        Value::Object(o) => o.values().for_each(|x| strings(x, out)),
        _ => {}
    }
}

/// The planted instructions of one injection fixture: each literal string of a PDF (its page and its
/// metadata), each paragraph of a text file (HTML tags taken off) and each string of a JSON file that
/// holds the marker, and the `.truth` file's `injection_text`.
fn planted(dir: &std::path::Path, fixture: &Value, truth: &Value, marker: &str) -> Vec<String> {
    let mut out = Vec::new();
    for f in fixture["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let bytes = std::fs::read(dir.join(f)).unwrap_or_else(|e| panic!("injection fixture {f}: {e}"));
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut pieces = Vec::new();
        if f.ends_with(".pdf") {
            pieces.extend(
                text.split('(')
                    .skip(1)
                    .filter_map(|s| s.split_once(')').map(|(a, _)| a.to_string())),
            );
        } else if f.ends_with(".json") {
            strings(&serde_json::from_str(&text).unwrap_or_default(), &mut pieces);
        } else {
            let mut para = String::new();
            for line in text.lines().chain([""]) {
                if line.trim().is_empty() {
                    pieces.push(std::mem::take(&mut para));
                } else {
                    para.push_str(line);
                    para.push('\n');
                }
            }
        }
        for p in pieces {
            // tags off, so a hidden HTML div gives its words
            let mut words = String::new();
            let mut tag = false;
            for ch in p.chars() {
                match ch {
                    '<' => tag = true,
                    '>' if tag => {
                        tag = false;
                        words.push(' ');
                    }
                    _ if !tag => words.push(ch),
                    _ => {}
                }
            }
            let words = collapse(&words);
            if words.contains(marker) {
                out.push(words);
            }
        }
    }
    if let Some(t) = truth["expect"]["injection_text"].as_str() {
        out.push(collapse(t));
    }
    out.sort();
    out.dedup();
    out
}

/// A document that carries `text` as its title, in a narrative cell a result's name and value point
/// at, as the result's value, as a medicine's sig (Instruction), as a visit's type, performer and
/// place, and as the note of a procedure and of a procedure observation.
fn carrying(text: &str) -> Vec<u8> {
    let e = escape(text);
    let body = format!(
        r##"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.1.1"/><title>MEDICATIONS</title>
          <entry><substanceAdministration classCode="SBADM" moodCode="EVN">
            <templateId root="2.16.840.1.113883.10.20.22.4.16"/><id root="1.2.3.4" extension="med-inj"/>
            <statusCode code="active"/>
            <consumable><manufacturedProduct><manufacturedMaterial>
              <code code="860975" codeSystem="2.16.840.1.113883.6.88" displayName="Metformin 500 MG Oral Tablet"/>
            </manufacturedMaterial></manufacturedProduct></consumable>
            <entryRelationship typeCode="COMP"><substanceAdministration classCode="SBADM" moodCode="INT">
              <templateId root="2.16.840.1.113883.10.20.22.4.147"/><code code="76662-6" codeSystem="2.16.840.1.113883.6.1"/>
              <text>{e}</text>
              <consumable><manufacturedProduct><manufacturedMaterial nullFlavor="NA"/></manufacturedProduct></consumable>
            </substanceAdministration></entryRelationship>
          </substanceAdministration></entry></section></component>
        <component><section><templateId root="2.16.840.1.113883.10.20.22.2.3.1"/><title>RESULTS</title>
          <text><table><tbody><tr><td ID="res-inj">{e}</td></tr></tbody></table></text>
          <entry><organizer classCode="BATTERY" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.1"/>
            <id root="1.2.3.4" extension="org-inj"/>
            <code code="24323-8" codeSystem="2.16.840.1.113883.6.1" displayName="Metabolic panel"/>
            <statusCode code="completed"/><effectiveTime value="20261020"/>
            <component><observation classCode="OBS" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.2"/>
              <id root="1.2.3.4" extension="obs-inj"/>
              <code code="2345-7" codeSystem="2.16.840.1.113883.6.1"><originalText><reference value="#res-inj"/></originalText></code>
              <statusCode code="completed"/><effectiveTime value="20261020"/>
              <value xsi:type="ST">{e}</value>
            </observation></component>
          </organizer></entry></section></component>
        <component><section><templateId root="2.16.840.1.113883.10.20.22.2.22.1"/><title>ENCOUNTERS</title>
          <entry><encounter classCode="ENC" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.49"/>
            <id root="1.2.3.4" extension="enc-inj"/>
            <code code="99213" codeSystem="2.16.840.1.113883.6.12"><originalText>{e}</originalText></code>
            <effectiveTime value="20261020"/>
            <performer><assignedEntity><id root="2.16.840.1.113883.4.6" extension="1234567893"/>
              <assignedPerson><name>{e}</name></assignedPerson></assignedEntity></performer>
            <participant typeCode="LOC"><participantRole classCode="SDLOC">
              <playingEntity classCode="PLC"><name>{e}</name></playingEntity></participantRole></participant>
          </encounter></entry></section></component>
        <component><section><templateId root="2.16.840.1.113883.10.20.22.2.7.1"/><title>PROCEDURES</title>
          <entry><procedure classCode="PROC" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.14"/>
            <id root="1.2.3.4" extension="proc-inj"/>
            <code code="73761001" codeSystem="2.16.840.1.113883.6.96" displayName="Colonoscopy"/>
            <text>{e}</text><statusCode code="completed"/><effectiveTime value="20261020"/>
          </procedure></entry>
          <entry><observation classCode="OBS" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.13"/>
            <id root="1.2.3.4" extension="pobs-inj"/>
            <code code="274025005" codeSystem="2.16.840.1.113883.6.96" displayName="Colonic polypectomy"/>
            <text>{e}</text><statusCode code="completed"/><effectiveTime value="20261020"/>
          </observation></entry></section></component>"##
    );
    String::from_utf8(document(&body))
        .unwrap_or_default()
        .replacen("<title>Test document</title>", &format!("<title>{e}</title>"), 1)
        .into_bytes()
}

/// What breaks the injection rule in converted output: the marker anywhere but a free-text field
/// (`title`, `text`, `valueString`, the display of the visit's performer or place), or a number that
/// the `.truth` says must never be a value. `path` is the keys from the resource down, dot-joined.
fn injection_breaches(v: &Value, path: &str, marker: &str, never: &[String], out: &mut Vec<String>) {
    let key = path.rsplit('.').next().unwrap_or_default();
    let free_text = matches!(key, "title" | "text" | "valueString")
        || matches!(path, "participant.individual.display" | "location.location.display");
    match v {
        Value::String(s) if s.contains(marker) && !free_text => out.push(format!("{path}: {s}")),
        Value::Number(n) if never.contains(&n.to_string()) => out.push(format!("{path}: {n}")),
        Value::Array(a) => a.iter().for_each(|x| injection_breaches(x, path, marker, never, out)),
        Value::Object(o) => o.iter().for_each(|(k, x)| {
            let at = if path.is_empty() {
                k.clone()
            } else {
                format!("{path}.{k}")
            };
            injection_breaches(x, &at, marker, never, out);
        }),
        _ => {}
    }
}

/// CR9: the injection fixtures of `fixtures/labslips/injection/` on the C-CDA free-text paths. Each
/// planted instruction rides in a document's title, narrative (a result's name and value), sig,
/// visit (type, performer, place) and procedure notes.
/// It is stored as written, as data: the output equals the same document with plain text in those
/// places, the marker sits only in free-text fields, and no number the `.truth` names becomes a value.
#[test]
fn injection_fixtures_on_the_ccda_free_text_paths_stay_data() -> R {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/labslips/injection");
    let registry: Value = serde_json::from_slice(&std::fs::read(dir.join("registry.json"))?)?;
    let marker = registry["marker"].as_str().unwrap_or("CANARY");
    // the checker sees a breach: a positive control
    let mut seen = Vec::new();
    // (a coding display is no free-text field; the visit's performer display is one)
    injection_breaches(
        &json!({"code": {"coding": [{"code": marker, "display": marker}]}, "valueQuantity": {"value": 45},
            "participant": [{"individual": {"display": marker}}]}),
        "",
        marker,
        &["45".to_string()],
        &mut seen,
    );
    assert_eq!(
        seen,
        [
            format!("code.coding.code: {marker}"),
            format!("code.coding.display: {marker}"),
            "valueQuantity.value: 45".to_string()
        ]
    );
    let plain = "Take one tablet by mouth daily";
    let control = convert(&carrying(plain))?;
    let fixtures = registry["fixtures"].as_array().cloned().unwrap_or_default();
    assert!(fixtures.len() >= 7, "{registry}");
    for f in &fixtures {
        let name = f["name"].as_str().unwrap_or_default();
        let truth: Value = serde_json::from_slice(&std::fs::read(dir.join(format!("{name}.truth")))?)?;
        let never: Vec<String> = truth["expect"]["never_a_value"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        let payloads = planted(&dir, f, &truth, marker);
        assert!(!payloads.is_empty(), "{name}: no planted text found");
        for p in payloads {
            let c = convert(&carrying(&p))?;
            let composition = resources(&c, "Composition")[0];
            let statement = resources(&c, "MedicationStatement")[0];
            let result = resources(&c, "Observation")[0];
            assert_eq!(composition["title"], json!(p), "{name}");
            assert_eq!(statement["dosage"][0]["text"], json!(p), "{name}");
            assert_eq!(result["code"]["text"], json!(p), "{name}");
            assert_eq!(result["valueString"], json!(p), "{name}");
            let visit = resources(&c, "Encounter")[0];
            assert_eq!(visit["type"][0]["text"], json!(p), "{name}");
            assert_eq!(visit["participant"][0]["individual"]["display"], json!(p), "{name}");
            assert_eq!(visit["location"][0]["location"]["display"], json!(p), "{name}");
            assert_eq!(resources(&c, "Procedure")[0]["note"][0]["text"], json!(p), "{name}");
            assert_eq!(resources(&c, "Observation")[1]["note"][0]["text"], json!(p), "{name}");
            let mut breaches = Vec::new();
            for e in &c.entries {
                injection_breaches(&e.resource, "", marker, &never, &mut breaches);
            }
            assert!(breaches.is_empty(), "{name}: {breaches:?}");
            // the text changes nothing but the fields that hold it
            let quoted = |s: &str| serde_json::to_string(s).unwrap_or_default();
            let same = serde_json::to_string(&c.entries.iter().map(|e| &e.resource).collect::<Vec<_>>())?
                .replace(&quoted(&p), &quoted(plain));
            let want = serde_json::to_string(&control.entries.iter().map(|e| &e.resource).collect::<Vec<_>>())?;
            assert_eq!(same, want, "{name}");
            assert_eq!(c.sections, control.sections, "{name}");
        }
    }
    Ok(())
}

// ------------------------------------------------------------- round 4 ----

/// The codes of `meta.security` of each resource of `rtype`, in document order.
fn label_codes(c: &Converted, rtype: &str) -> Vec<Vec<String>> {
    resources(c, rtype)
        .iter()
        .map(|r| {
            r["meta"]["security"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s["code"].as_str().map(String::from))
                .collect()
        })
        .collect()
}

/// A heart rate entry of a Vital Signs section.
fn vital_entry(id: &str) -> String {
    format!(
        r#"<entry><observation classCode="OBS" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.27"/>
          <id root="1.2.3" extension="{id}"/><code code="8867-4" codeSystem="2.16.840.1.113883.6.1" displayName="Heart rate"/>
          <statusCode code="completed"/><effectiveTime value="20240101"/><value xsi:type="PQ" value="72" unit="/min"/></observation></entry>"#
    )
}

const VITALS_HEAD: &str = r#"<templateId root="2.16.840.1.113883.10.20.22.2.4.1"/><code code="8716-3" codeSystem="2.16.840.1.113883.6.1"/><title>VITAL SIGNS</title>"#;
const RESTRICTED: &str = r#"<confidentialityCode code="R" codeSystem="2.16.840.1.113883.5.25"/>"#;

/// T6: a restricted confidentialityCode on a section reaches the records of every section nested in
/// it, at any depth, and one on the structuredBody reaches every record of the document.
#[test]
fn a_restricted_parent_section_or_body_labels_every_record_inside_it() -> R {
    let nested = format!(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.65"/><title>Behavioral health</title>
             {RESTRICTED}<text>restricted</text>
             <component><section><title>Notes</title>
               <component><section>{VITALS_HEAD}{}</section></component></section></component>
           </section></component>
           <component><section>{VITALS_HEAD}{}</section></component>"#,
        vital_entry("inside"),
        vital_entry("outside")
    );
    let c = convert(&document(&nested))?;
    assert_eq!(label_codes(&c, "Observation"), vec![vec!["R".to_string()], vec![]]);

    let body = format!(
        r#"{RESTRICTED}<component><section>{VITALS_HEAD}{}</section></component>"#,
        vital_entry("v")
    );
    let c = convert(&document(&body))?;
    assert_eq!(label_codes(&c, "Observation"), vec![vec!["R".to_string()]]);
    assert_eq!(label_codes(&c, "Composition"), vec![vec!["R".to_string()]]);
    Ok(())
}

#[test]
fn round_6_referenced_original_text_carries_its_source_entry_labels() -> R {
    let c = convert(&document(&format!(
        r##"<component><section>{VITALS_HEAD}
          <entry><observation classCode="OBS" moodCode="EVN">
            <code code="8867-4" codeSystem="2.16.840.1.113883.6.1">
              <originalText ID="pulse-name">Pulse at rest</originalText>
            </code>
            {RESTRICTED}
          </observation></entry>
          <entry><observation classCode="OBS" moodCode="EVN">
            <code code="8867-4" codeSystem="2.16.840.1.113883.6.1">
              <originalText><reference value="#pulse-name"/></originalText>
            </code>
          </observation></entry>
          {}
        </section></component>"##,
        vital_entry("unrelated")
    )))?;
    let observations = resources(&c, "Observation");
    assert_eq!(observations.len(), 3);
    assert_eq!(observations[1]["code"]["text"], "Pulse at rest");
    assert_eq!(
        label_codes(&c, "Observation"),
        vec![vec!["R".to_string()], vec!["R".to_string()], vec![]]
    );
    Ok(())
}

/// An Immunization Activity in `mood` with `status`.
fn immunization_section(mood: &str, status: &str) -> String {
    format!(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.2.1"/><title>IMMUNIZATIONS</title>
          <entry><substanceAdministration classCode="SBADM" moodCode="{mood}" negationInd="false">
            <templateId root="2.16.840.1.113883.10.20.22.4.52"/><id root="1.2.3" extension="imm-{mood}-{status}"/>
            <statusCode code="{status}"/><effectiveTime value="20261201"/>
            <consumable><manufacturedProduct><templateId root="2.16.840.1.113883.10.20.22.4.54"/><manufacturedMaterial>
              <code code="88" codeSystem="2.16.840.1.113883.12.292" displayName="Influenza virus vaccine"/>
            </manufacturedMaterial></manufacturedProduct></consumable>
          </substanceAdministration></entry></section></component>"#
    )
}

/// T8: only a given vaccine (`EVN`, not active) is a completed Immunization. A planned one (`INT`)
/// and one still active are listed as not read, never recorded as given.
#[test]
fn a_planned_or_active_immunization_is_never_recorded_as_given() -> R {
    for (mood, status) in [
        ("INT", "active"),
        ("INT", "completed"),
        ("EVN", "active"),
        ("PRP", "completed"),
    ] {
        let c = convert(&document(&immunization_section(mood, status)))?;
        assert!(resources(&c, "Immunization").is_empty(), "{mood} {status}");
        let s = c
            .sections
            .iter()
            .find(|s| s.kind == Some(Section::Immunizations))
            .ok_or_else(|| anyhow::anyhow!("no immunizations section"))?;
        assert_eq!(
            s.not_read,
            ["substanceAdministration 2.16.840.1.113883.10.20.22.4.52"],
            "{mood} {status}"
        );
    }
    let c = convert(&document(&immunization_section("EVN", "completed")))?;
    let given = resources(&c, "Immunization");
    assert_eq!(given.len(), 1);
    assert_eq!(given[0]["status"], "completed");
    assert!(c.sections.iter().all(|s| s.not_read.is_empty()), "{:?}", c.sections);
    Ok(())
}

/// A medicine of `ccd-1.xml` by a word of its name.
fn ccd1_medicine(c: &Converted, word: &str) -> anyhow::Result<Value> {
    resources(c, "MedicationStatement")
        .into_iter()
        .find(|r| {
            r["medicationCodeableConcept"]["text"]
                .as_str()
                .is_some_and(|t| t.contains(word))
        })
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no {word}"))
}

/// A medicine with a referenced sig, optional inline words, and other children.
fn medicine_with(narrative: &str, inline: &str, extra: &str) -> Vec<u8> {
    document(&format!(
        r##"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.1.1"/><title>MEDICATIONS</title>
          <text><content ID="med-n">{narrative}</content></text>
          <entry><substanceAdministration classCode="SBADM" moodCode="EVN">
            <templateId root="2.16.840.1.113883.10.20.22.4.16"/><id root="1.2.3.4" extension="med-x"/>
            <text><reference value="#med-n"/>{inline}</text>
            <statusCode code="active"/>
            <consumable><manufacturedProduct><manufacturedMaterial>
              <code code="573621" codeSystem="2.16.840.1.113883.6.88" displayName="albuterol 0.09 MG/ACTUAT [Proventil]"/>
            </manufacturedMaterial></manufacturedProduct></consumable>
            {extra}
          </substanceAdministration></entry></section></component>"##,
    ))
}

/// T7: an as-needed precondition ("when wheezing", CCD 1's albuterol) becomes the dosage's
/// asNeededCodeableConcept; a precondition that names nothing is listed as not read.
#[test]
fn an_as_needed_precondition_is_kept_or_listed() -> R {
    let c = convert(&fixture("ccd-1.xml"))?;
    let albuterol = ccd1_medicine(&c, "albuterol")?;
    let as_needed = &albuterol["dosage"][0]["asNeededCodeableConcept"];
    assert_eq!(as_needed["coding"][0]["code"], "56018004", "{albuterol}");
    assert_eq!(as_needed["text"], "Wheezing");
    let atenolol = ccd1_medicine(&c, "atenolol")?;
    assert!(
        atenolol["dosage"][0].get("asNeededCodeableConcept").is_none(),
        "{atenolol}"
    );

    let empty = r#"<precondition typeCode="PRCN"><criterion><templateId root="2.16.840.1.113883.10.20.22.4.25"/>
        <code code="ASSERTION" codeSystem="2.16.840.1.113883.5.4"/></criterion></precondition>"#;
    let c = convert(&medicine_with("2 puffs", "", empty))?;
    let med = resources(&c, "MedicationStatement")[0];
    assert!(med["dosage"][0].get("asNeededCodeableConcept").is_none(), "{med}");
    let s = c
        .sections
        .iter()
        .find(|s| s.kind == Some(Section::Medications))
        .ok_or_else(|| anyhow::anyhow!("no medications section"))?;
    assert_eq!(s.not_read, ["criterion 2.16.840.1.113883.10.20.22.4.25"]);
    Ok(())
}

/// T4: a sig that points at the narrative and goes on with its own words keeps both (the H&P's
/// albuterol: "2 puffs QID PRN wheezing" is written inline), within the 2,000 characters a text has.
#[test]
fn a_sig_keeps_its_own_words_beside_the_narrative_it_points_at() -> R {
    let c = convert(&fixture("history-and-physical.xml"))?;
    let albuterol = ccd1_medicine(&c, "albuterol")?;
    let text = albuterol["dosage"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("Proventil 0.09 MG/ACTUAT inhalant solution"), "{text}");
    assert!(text.contains("2 puffs QID PRN wheezing"), "{text}");

    let c = convert(&medicine_with(&"narrative ".repeat(300), &"inline ".repeat(400), ""))?;
    let text = resources(&c, "MedicationStatement")[0]["dosage"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(text.starts_with("narrative "), "{text}");
    assert!(text.contains("inline"), "{text}");
    assert!(text.chars().count() <= 2_000, "{}", text.chars().count());
    assert_eq!(
        c.sections[0].not_read,
        ["medication instructions (text scan limit reached)"]
    );
    Ok(())
}

/// A document whose Privacy Markings Section carries `n` different DS4P labels, and one problem.
fn labelled(n: usize) -> Vec<u8> {
    let marks: String = (0..n)
        .map(|i| format!("<entry>{}</entry>", security_label(&format!("L{i}"))))
        .collect();
    document(&format!(
        r#"<component><section><templateId root="2.16.840.1.113883.3.3445.4"/><title>Privacy Markings</title>{marks}</section></component>{}"#,
        problems_section(&problem_entry(
            "p",
            r#"code="233604007" displayName="Pneumonia""#,
            r#"<low value="20200110"/>"#,
            "",
        ))
    ))
}

/// T2: every record gets every label of its document, so the labels a file may carry are capped: 64
/// different ones are read in full, 65 refuse the file. No label is ever dropped.
#[test]
fn more_than_64_security_labels_refuse_the_file() -> R {
    let c = convert(&labelled(64))?;
    assert_eq!(
        label_codes(&c, "Condition").iter().map(Vec::len).collect::<Vec<_>>(),
        [64]
    );
    match convert(&labelled(65)) {
        Err(Error::Invalid(m)) => assert!(m.contains("more than 64 different security labels"), "{m}"),
        other => panic!("65 labels were read: {other:?}"),
    }
    // the same label many times is one label
    let c = convert(
        &String::from_utf8(labelled(1))?
            .replace(
                "</section></component>",
                &(format!("<entry>{}</entry>", security_label("L0")).repeat(100) + "</section></component>"),
            )
            .into_bytes(),
    )?;
    assert_eq!(label_codes(&c, "Condition"), vec![vec!["L0".to_string()]]);
    Ok(())
}

/// A document whose Privacy Markings Section carries one label with `code` and `system`, and one
/// problem.
fn one_label(code: &str, system: &str) -> Vec<u8> {
    let value = security_label(code).replace(
        "2.16.840.1.113883.5.4\"/></observation>",
        &format!("{system}\"/></observation>"),
    );
    String::from_utf8(labelled(1))
        .unwrap_or_default()
        .replace(&security_label("L0"), &value)
        .into_bytes()
}

/// A document whose Privacy Markings Section carries `labels` different labels, and `vitals` vital
/// signs: every vital sign gets every label.
fn labelled_vitals(labels: usize, vitals: usize) -> Vec<u8> {
    let marks: String = (0..labels)
        .map(|i| format!("<entry>{}</entry>", security_label(&format!("L{i}"))))
        .collect();
    let entries: String = (0..vitals).map(|i| vital_entry(&format!("v{i}"))).collect();
    document(&format!(
        r#"<component><section><templateId root="2.16.840.1.113883.3.3445.4"/><title>Privacy Markings</title>{marks}</section></component>
           <component><section>{VITALS_HEAD}{entries}</section></component>"#
    ))
}

/// R4-M1: a label's code and codeSystem are as short as real ones, because every record holds its
/// own copy: a code over 64 characters or a codeSystem over 128 refuses the file, in a DS4P value
/// and in a confidentialityCode alike. 64 and 128 are read.
#[test]
fn a_security_label_longer_than_a_real_one_refuses_the_file() -> R {
    let c = convert(&one_label(&"C".repeat(64), &"1".repeat(128)))?;
    assert_eq!(label_codes(&c, "Condition"), vec![vec!["C".repeat(64)]]);
    let problem = problems_section(&problem_entry(
        "p",
        r#"code="233604007" displayName="Pneumonia""#,
        r#"<low value="20200110"/>"#,
        "",
    ));
    let confidential = |code: &str| {
        document(&format!(
            r#"<confidentialityCode code="{code}" codeSystem="2.16.840.1.113883.5.25"/>{problem}"#
        ))
    };
    let c = convert(&confidential(&"R".repeat(64)))?;
    assert_eq!(label_codes(&c, "Condition"), vec![vec!["R".repeat(64)]]);
    for file in [
        one_label(&"C".repeat(65), "2.16.840.1.113883.5.4"),
        one_label("C", &"1".repeat(129)),
        confidential(&"R".repeat(65)),
    ] {
        match convert(&file) {
            Err(Error::Invalid(m)) => assert!(m.contains("a security label longer than a real one"), "{m}"),
            other => panic!("a long label was read: {}", other.is_ok()),
        }
    }
    Ok(())
}

/// R4-M1: every record holds its own copy of its labels, so the copies one file makes are capped at
/// 250,000: 64 labels on 3,000 vital signs (192,000 copies) are read in full, on 4,000 (256,000)
/// the file is refused. No label is ever dropped.
#[test]
fn label_copies_over_the_budget_refuse_the_file() -> R {
    let c = convert(&labelled_vitals(64, 3_000))?;
    let codes = label_codes(&c, "Observation");
    assert_eq!(codes.len(), 3_000);
    assert!(codes.iter().all(|l| l.len() == 64));
    match convert(&labelled_vitals(64, 4_000)) {
        Err(Error::Invalid(m)) => assert!(m.contains("more than 250000 security labels in all"), "{m}"),
        other => panic!("256,000 label copies were read: {}", other.is_ok()),
    }
    Ok(())
}

/// T2, R4-M1: a file refused for its labels (too many, too long, or too many copies) writes
/// nothing: no document, no record, no review item.
#[tokio::test]
async fn a_file_with_too_many_security_labels_writes_nothing() -> R {
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    let p = as_isabella(&mut tx, &fam, "lucia").await?;
    tx.commit().await?;
    for (bytes, why) in [
        (labelled(65), "the file carries more than 64 different security labels"),
        (
            one_label(&"C".repeat(65_536), "2.16.840.1.113883.5.4"),
            "the file carries a security label longer than a real one",
        ),
        (
            labelled_vitals(64, 4_000),
            "the file's records would carry more than 250000 security labels in all",
        ),
    ] {
        let sha = env
            .blobs
            .put_bytes(ownhealth_core::blobs::BlobOwner::Profile(p), &bytes)?;
        let settings = Arc::new(env.db.settings(env.dir.path())?);
        let ctx = JobContext::new(env.db.pool.clone(), settings, env.blobs.clone());
        let payload = json!({"profile_id": p.to_string(), "household_id": fam.household_id.to_string(),
            "blob_sha256": sha, "filename": "labels.xml"});
        let Err(e) = ccda::import_ccda_job(ctx, job(payload)).await else {
            panic!("a file refused for its labels imported: {why}");
        };
        assert!(e.permanent, "{e}");
        assert!(
            e.to_string().contains(&format!("not a readable C-CDA document: {why}")),
            "{e}"
        );
        let mut conn = env.db.pool.acquire().await?;
        let docs: i64 = sqlx::query_scalar("select count(*) from document where blob_sha256 = decode($1, 'hex')")
            .bind(&sha)
            .fetch_one(&mut *conn)
            .await?;
        assert_eq!(docs, 0, "{why}");
        for table in ["condition", "observation", "review_item"] {
            assert_eq!(count(&mut conn, table, p).await?, 0, "{why}: {table}");
        }
    }
    Ok(())
}

/// T1: the Medplum port ships the full Apache License 2.0 text (section 4(a)): in the repository,
/// named in NOTICE, and copied into the image beside NOTICE.
#[test]
fn the_medplum_port_ships_the_apache_license_text() -> R {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let text = std::fs::read_to_string(root.join("LICENSES/Apache-2.0.txt"))?;
    assert!(text.contains("Version 2.0, January 2004"));
    assert!(text.contains("TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION"));
    assert!(text.contains("END OF TERMS AND CONDITIONS"));
    let notice = std::fs::read_to_string(root.join("NOTICE"))?;
    assert!(notice.contains("LICENSES/Apache-2.0.txt"), "{notice}");
    let dockerfile = std::fs::read_to_string(root.join("Dockerfile"))?;
    let copies = dockerfile
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .any(|l| l.starts_with("COPY ") && l.contains("LICENSES/") && l.contains("/usr/share/doc/ownhealth/"));
    assert!(copies, "the image does not carry LICENSES/");
    Ok(())
}
#[test]
fn round_5_a_bounded_medication_dose_is_listed_as_unread() -> R {
    let bounded = convert(&medicine_with(
        "2 puffs",
        "",
        r#"<doseQuantity><low value="1" unit="mL"/><high value="2" unit="mL"/></doseQuantity>"#,
    ))?;
    assert_eq!(bounded.sections[0].not_read, ["doseQuantity (bounded dose)"]);
    assert!(
        resources(&bounded, "MedicationStatement")[0]["dosage"][0]
            .get("doseAndRate")
            .is_none()
    );

    let scalar = convert(&medicine_with("2 puffs", "", r#"<doseQuantity value="1" unit="mL"/>"#))?;
    assert!(scalar.sections[0].not_read.is_empty());
    assert_eq!(
        resources(&scalar, "MedicationStatement")[0]["dosage"][0]["doseAndRate"][0]["doseQuantity"],
        json!({"value": 1, "unit": "mL", "system": "http://unitsofmeasure.org", "code": "mL"})
    );
    Ok(())
}

#[test]
fn round_5_an_unsupported_structured_result_value_is_listed_as_unread() -> R {
    let mut entries = String::new();
    for (id, value) in [
        (
            "ratio",
            r#"<value xsi:type="RTO_PQ_PQ"><numerator value="11" unit="mg"/><denominator value="3" unit="dL"/></value>"#,
        ),
        ("present", r#"<value xsi:type="PQ" value="12" unit="mg/dL"/>"#),
        ("missing", r#"<value xsi:type="PQ" nullFlavor="NI"/>"#),
    ] {
        entries.push_str(&format!(
            r#"<entry><observation classCode="OBS" moodCode="EVN">
              <id root="1.2.3" extension="{id}"/><code code="13457-7" codeSystem="2.16.840.1.113883.6.1"/>
              <statusCode code="completed"/><effectiveTime value="20240101"/>{value}
            </observation></entry>"#
        ));
    }
    let c = convert(&document(&format!(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.3.1"/><title>RESULTS</title>
          {entries}</section></component>"#
    )))?;
    assert_eq!(
        c.sections[0].not_read,
        ["observation (unsupported value type RTO_PQ_PQ)"]
    );
    let observations = resources(&c, "Observation");
    assert_eq!(observations.len(), 3);
    assert_eq!(observations[1]["valueQuantity"]["value"], json!(12));
    for obs in [observations[0], observations[2]] {
        assert!(
            !obs.as_object()
                .ok_or_else(|| anyhow::anyhow!("expected an Observation object: {obs}"))?
                .keys()
                .any(|key| key.starts_with("value")),
            "{obs}"
        );
    }
    Ok(())
}

/// R5-M1: a nullFlavor on a coded result value says the code is outside the value set, not that
/// there is no value. The translation and original text are still the result.
#[test]
fn round_6_a_coded_result_value_with_a_null_flavor_keeps_its_translation() -> R {
    let mut entries = String::new();
    for (id, value) in [
        (
            "local",
            r#"<value xsi:type="CD" nullFlavor="OTH"><originalText>Positive</originalText><translation code="POS" codeSystem="1.2.3.4.5" displayName="Positive (local)"/></value>"#,
        ),
        ("null-ratio", r#"<value xsi:type="RTO_PQ_PQ" nullFlavor="NI"/>"#),
        ("null-code", r#"<value xsi:type="CD" nullFlavor="UNK"/>"#),
        // a null quantity still names its unit: the unit alone is not a result
        (
            "null-quantity",
            r#"<value xsi:type="PQ" nullFlavor="NI" unit="mg/dL"/>"#,
        ),
    ] {
        entries.push_str(&format!(
            r#"<entry><observation classCode="OBS" moodCode="EVN">
              <id root="1.2.3" extension="{id}"/><code code="13457-7" codeSystem="2.16.840.1.113883.6.1"/>
              <statusCode code="completed"/><effectiveTime value="20240101"/>{value}
            </observation></entry>"#
        ));
    }
    let c = convert(&document(&format!(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.3.1"/><title>RESULTS</title>
          {entries}</section></component>"#
    )))?;
    assert!(c.sections[0].not_read.is_empty(), "{:?}", c.sections[0].not_read);
    let observations = resources(&c, "Observation");
    assert_eq!(observations.len(), 4);
    assert_eq!(
        observations[0]["valueCodeableConcept"],
        json!({
            "coding": [{"system": "urn:oid:1.2.3.4.5", "code": "POS", "display": "Positive (local)"}],
            "text": "Positive"
        })
    );
    for obs in [observations[1], observations[2], observations[3]] {
        assert!(
            !obs.as_object()
                .ok_or_else(|| anyhow::anyhow!("expected an Observation object: {obs}"))?
                .keys()
                .any(|key| key.starts_with("value")),
            "{obs}"
        );
    }
    Ok(())
}

#[test]
fn round_6_unread_typed_result_text_is_reported() -> R {
    for (value, unread) in [
        (r#"<value xsi:type="TS">not-a-timestamp</value>"#, true),
        (r#"<value xsi:type="TS" value="not-a-timestamp"/>"#, true),
        (r#"<value xsi:type="TS" nullFlavor="OTH">not-a-timestamp</value>"#, true),
        ("<value xsi:type=\"TS\">  \n\t  </value>", false),
    ] {
        let c = convert(&document(&format!(
            r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.3.1"/><title>RESULTS</title>
              <entry><observation classCode="OBS" moodCode="EVN">
                <code code="13457-7" codeSystem="2.16.840.1.113883.6.1"/>{value}
              </observation></entry>
            </section></component>"#
        )))?;
        let expected: &[&str] = if unread {
            &["observation (unsupported value type TS)"]
        } else {
            &[]
        };
        assert_eq!(c.sections[0].not_read, expected, "{value}");
        let observations = resources(&c, "Observation");
        assert_eq!(observations.len(), 1);
        assert!(observations[0].get("valueDateTime").is_none());
        assert!(observations[0].get("valueString").is_none());
    }
    Ok(())
}

#[test]
fn slice_21_encounters_keep_class_period_and_reason() -> R {
    let c = convert(&fixture("ccd-1.xml"))?;
    let encounters = resources(&c, "Encounter");
    assert_eq!(encounters.len(), 1);
    let encounter = encounters[0];
    assert_eq!(encounter["class"]["code"], "AMB");
    assert_eq!(encounter["type"][0]["coding"][0]["code"], "99213");
    // the words the document shows for the visit, not the CPT paragraph
    assert_eq!(encounter["type"][0]["text"], "Checkup Examination");
    assert_eq!(encounter["period"]["start"], "2012-09-27T13:00:00-05:00");
    assert_eq!(
        encounter["location"][0]["location"]["display"],
        "Good Health Urgent Care"
    );
    // the performer has an NPI and a role, no name: the role names no provider
    let individual = &encounter["participant"][0]["individual"];
    assert_eq!(individual["identifier"]["value"], "333444555");
    assert!(individual.get("display").is_none(), "{individual}");
    // the indication is why the visit happened, never a condition of its own
    assert_eq!(encounter["reasonCode"][0]["coding"][0]["code"], "233604007");
    assert_eq!(encounter["reasonCode"][0]["text"], "Pneumonia");
    assert!(encounter.get("diagnosis").is_none(), "{encounter}");
    assert!(!made(&c, Section::Encounters).contains_key("Condition"));
    let pneumonia: Vec<&Value> = resources(&c, "Condition")
        .into_iter()
        .filter(|r| r["code"]["coding"][0]["code"] == "233604007")
        .collect();
    assert!(!pneumonia.is_empty());
    assert!(
        pneumonia
            .iter()
            .all(|r| r["clinicalStatus"]["coding"][0]["code"] != "active"),
        "{pneumonia:?}"
    );
    Ok(())
}

#[test]
fn slice_21_procedures_keep_the_act_procedure_and_result() -> R {
    let c = convert(&fixture("ccd-2.xml"))?;
    let procedures = resources(&c, "Procedure");
    assert_eq!(procedures.len(), 2);
    let appendectomy = procedures
        .iter()
        .find(|p| p["code"]["coding"][0]["code"] == "6025007")
        .context("missing laparoscopic appendectomy")?;
    assert_eq!(appendectomy["performedPeriod"]["start"], "2014-10-02T10:30:26-05:00");
    assert_eq!(appendectomy["performedPeriod"]["end"], "2014-10-02T12:42:45-05:00");
    assert_eq!(appendectomy["bodySite"][0]["coding"][0]["code"], "181255000");
    assert!(procedures.iter().any(|p| p["code"]["coding"][0]["code"] == "61310001"));
    let observations = resources(&c, "Observation");
    let ecg = observations
        .iter()
        .find(|p| p["code"]["coding"][0]["code"] == "268400002")
        .context("missing electrocardiogram")?;
    assert_eq!(ecg["valueCodeableConcept"]["coding"][0]["code"], "251135002");
    Ok(())
}

#[test]
fn slice_21_notes_are_kept_without_creating_a_clinical_resource() -> R {
    let c = convert(&section_file("notes.xml"))?;
    let note = &c.sections[0];
    assert_eq!(note.kind.map(Section::name), Some("notes"));
    assert_eq!(note.entries, 1);
    assert_eq!(note.stored_notes, 1);
    assert!(note.resources.is_empty(), "{note:?}");
    // the review item is where a person learns the note is in the file
    let held = held_items(&c, "notes-test", "notes.xml");
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0]["count"], 1);
    assert_eq!(held[0]["notes"], json!({"clinical note kept in the uploaded file": 1}));
    Ok(())
}

#[test]
fn slice_21_smoking_status_is_a_social_history_observation() -> R {
    let c = convert(&section_file("smoking-current.xml"))?;
    let observations = resources(&c, "Observation");
    assert_eq!(observations.len(), 2);
    let smoking = observations
        .iter()
        .find(|r| r["code"]["coding"][0]["code"] == "72166-2")
        .context("missing smoking status")?;
    assert_eq!(smoking["category"][0]["coding"][0]["code"], "social-history");
    assert_eq!(smoking["valueCodeableConcept"]["coding"][0]["code"], "449868002");
    assert_eq!(smoking["effectiveDateTime"], "2014-06-06T10:32:00-05:00");
    let detail = observations
        .iter()
        .find(|r| r["code"]["coding"][0]["code"] == "11367-0")
        .context("missing tobacco use history")?;
    assert_eq!(detail["effectivePeriod"]["start"], "1992");
    assert!(held_items(&c, "smoking-test", "smoking.xml").is_empty());
    Ok(())
}

#[test]
fn slice_21_full_ccd_accounts_for_every_section_and_entry() -> R {
    use ownhealth_ingest_structured::ccda::xml::{child, children, parse, text_of_file};
    let bytes = fixture("ccd-1.xml");
    let text = text_of_file(&bytes)?;
    let doc = parse(&text)?;
    let top = doc
        .root_element()
        .descendants()
        .find(|n| n.has_tag_name("structuredBody"))
        .context("missing CCD body")?;
    let c = convert(&bytes)?;
    let held = held_items(&c, "full", "ccd-1.xml");
    let sections: Vec<_> = children(top, "component").filter_map(|n| child(n, "section")).collect();
    assert_eq!(sections.len(), 15);
    assert_eq!(c.sections.len(), sections.len());
    assert_eq!(c.sections.iter().filter(|s| s.kind.is_some()).count(), 9);
    let mut entries = 0;
    let mut without_resource = Vec::new();
    for (section, report) in sections.into_iter().zip(&c.sections) {
        let input = children(section, "entry").count();
        entries += input;
        assert_eq!(report.entries, input, "{}", report.title);
        if report.kind.is_none() {
            let item = held
                .iter()
                .find(|h| h["section"]["pointer"] == report.pointer)
                .with_context(|| format!("missing review item for {}", report.title))?;
            assert_eq!(item["count"], input, "{}", report.title);
            continue;
        }
        for i in 0..input {
            let pointer = format!("{}/entry/{i}", report.pointer);
            if !c.entries.iter().any(|e| {
                e.pointer == pointer
                    || e.pointer.starts_with(&format!("{pointer}#"))
                    || e.pointer.starts_with(&format!("{pointer}/"))
            }) {
                without_resource.push(report.title.clone());
            }
        }
    }
    assert_eq!(entries, 31);
    // the one entry with no resource is the aborted procedure observation, and it waits in review
    assert_eq!(without_resource, ["PROCEDURES"]);
    let procedures = held
        .iter()
        .find(|h| h["resource_type"] == "PROCEDURES (C-CDA)")
        .context("missing review item for the aborted procedure")?;
    assert_eq!(
        procedures["notes"],
        json!({
            "not read: observation 2.16.840.1.113883.10.20.22.4.13 (status aborted)": 1,
            "not read: code (original text kept in the uploaded file)": 1,
        })
    );
    Ok(())
}

#[tokio::test]
async fn slice_21_the_note_count_and_original_are_kept_on_the_document() -> R {
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    let p = as_isabella(&mut tx, &fam, "lucia").await?;
    let bytes = section_file("notes.xml");
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &bytes, "notes.xml", ccda::MIME).await?;
    let s = import_ccda(&mut tx, &env.blobs, doc).await?;
    let (flags, sha): (Value, String) =
        sqlx::query_as("select flags, encode(blob_sha256, 'hex') from document where id = $1")
            .bind(doc)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(flags["ccda"]["sections"][0]["stored_notes"], 1);
    assert_eq!(flags["unprojected"], 1);
    assert_eq!((s.accepted, s.queued, s.skipped), (0, 1, 0));
    let held = open_items(&mut tx, doc, "fhir_unmapped_summary").await?;
    assert_eq!(held.len(), 1);
    assert_eq!(
        held[0].payload["notes"],
        json!({"clinical note kept in the uploaded file": 1})
    );
    assert_eq!(
        bytes,
        env.blobs
            .get_bytes(ownhealth_core::blobs::BlobOwner::Profile(p), &sha)?
    );
    assert_eq!(import_ccda(&mut tx, &env.blobs, doc).await?, s);
    Ok(())
}

#[test]
fn slice_21_typed_social_history_value_reports_unread_inline_text() -> R {
    let original = String::from_utf8(section_file("smoking-current.xml"))?;
    let edited = original.replace(
        r#"displayName="Smokes tobacco daily"/>"#,
        r#"displayName="Smokes tobacco daily">Additional untyped description</value>"#,
    );
    assert_ne!(original, edited);
    let c = convert(edited.as_bytes())?;
    assert_eq!(c.sections[0].not_read, ["observation (text beside a typed value)"]);
    assert_eq!(
        resources(&c, "Observation")[0]["valueCodeableConcept"]["coding"][0]["code"],
        "449868002"
    );
    Ok(())
}

#[test]
fn a_note_inside_an_encounter_stays_in_the_file_and_an_unknown_part_waits_in_review() -> R {
    let c = convert(&document(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.22.1"/><title>Encounters</title>
          <entry><encounter moodCode="EVN">
            <effectiveTime><low value="20210302"/><high value="20210303"/></effectiveTime>
            <entryRelationship typeCode="COMP"><act moodCode="EVN">
              <templateId root="2.16.840.1.113883.10.20.22.4.202"/><text>Clinical note</text>
            </act></entryRelationship>
            <entryRelationship typeCode="RSON"><observation moodCode="EVN">
              <templateId root="9.9.9"/><value xsi:type="CD" code="233604007" codeSystem="2.16.840.1.113883.6.96"/>
            </observation></entryRelationship>
          </encounter></entry>
          <entry><encounter moodCode="INT"><effectiveTime value="20210401"/></encounter></entry>
          <entry/>
        </section></component>"#,
    ))?;
    let section = &c.sections[0];
    assert_eq!(section.stored_notes, 1);
    assert_eq!(
        section.not_read,
        [
            "observation 9.9.9",
            "encounter without a templateId",
            "entry (no activity)"
        ]
    );
    assert_eq!(resources(&c, "Encounter").len(), 1);
    assert!(resources(&c, "Condition").is_empty());
    assert_eq!(held_items(&c, "test", "record.xml")[0]["count"], 4);
    assert!(!serde_json::to_string(&c.entries[2].resource)?.contains("Clinical note"));
    Ok(())
}

#[test]
fn an_encounter_carries_its_referenced_narrative_labels_to_its_own_resource() -> R {
    let c = convert(&document(&format!(
        r##"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.22.1"/>
          <entry><encounter moodCode="EVN">
            <code><originalText><reference value="#visit"/></originalText></code>
            <effectiveTime value="20210302"/>
            <entryRelationship typeCode="RSON"><observation moodCode="EVN">
              <value xsi:type="CD" code="233604007" codeSystem="2.16.840.1.113883.6.96"/>
            </observation></entryRelationship>
          </encounter></entry>
          <entry><encounter moodCode="EVN"><effectiveTime value="20210303"/></encounter></entry>
        </section></component>
        <component><section><text><paragraph ID="visit">Visit description</paragraph></text><entry>{}</entry></section></component>"##,
        security_label("PSY")
    )))?;
    assert_eq!(resources(&c, "Encounter")[0]["type"][0]["text"], "Visit description");
    assert_eq!(label_codes(&c, "Encounter"), [vec!["PSY".to_string()], Vec::new()]);
    assert_eq!(
        resources(&c, "Encounter")[0]["reasonCode"][0]["coding"][0]["code"],
        "233604007"
    );
    assert!(resources(&c, "Condition").is_empty());
    Ok(())
}

// ------------------------------------------------------ slice 21, round 1 ----

fn encounters(entries: &str) -> Vec<u8> {
    document(&format!(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.22.1"/><title>Encounters</title>
          {entries}</section></component>"#
    ))
}

fn procedures(entries: &str) -> Vec<u8> {
    document(&format!(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.7.1"/><title>Procedures</title>
          {entries}</section></component>"#
    ))
}

/// C-L2, M10: the class comes from a CPT code alone; a visit with no code has an unknown class, as
/// one with a code we do not know has, never ambulatory.
#[test]
fn round_1_an_encounter_class_comes_from_its_code_or_is_unknown() -> R {
    let c = convert(&encounters(
        r#"<entry><encounter moodCode="EVN"><code code="99283" codeSystem="2.16.840.1.113883.6.12"/>
            <effectiveTime value="20210302"/></encounter></entry>
          <entry><encounter moodCode="EVN"><effectiveTime value="20210303"/></encounter></entry>"#,
    ))?;
    let visits = resources(&c, "Encounter");
    assert_eq!(visits.len(), 2);
    assert_eq!(visits[0]["class"]["code"], "EMER");
    assert!(visits[1]["class"].get("code").is_none(), "{}", visits[1]);
    assert_eq!(visits[1]["class"]["extension"][0]["valueCode"], "unknown");
    Ok(())
}

/// C-H1, M1, M16: only a reason (RSON) Indication that is not negated is the visit's reason, and it
/// is never a Condition. Any other part, and a null activity, waits in review.
#[test]
fn round_1_only_a_reason_indication_becomes_the_visit_reason() -> R {
    let indication = |type_code: &str, rel_extra: &str, extra: &str, code: &str| {
        format!(
            r#"<entryRelationship typeCode="{type_code}" {rel_extra}><observation classCode="OBS" moodCode="EVN" {extra}>
              <templateId root="2.16.840.1.113883.10.20.22.4.19"/><statusCode code="completed"/>
              <effectiveTime><low value="20210301"/></effectiveTime>
              <value xsi:type="CD" code="{code}" codeSystem="2.16.840.1.113883.6.96"/>
            </observation></entryRelationship>"#
        )
    };
    let c = convert(&encounters(&format!(
        r#"<entry><encounter moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.49"/>
            <effectiveTime value="20210302"/>{}{}{}{}{}</encounter></entry>
          <entry><encounter moodCode="EVN" nullFlavor="NI"><templateId root="2.16.840.1.113883.10.20.22.4.49"/>
            <effectiveTime value="20210303"/></encounter></entry>"#,
        indication("RSON", r#"inversionInd="false" negationInd="false""#, "", "233604007"),
        indication("COMP", "", "", "195967001"),
        indication("RSON", "", r#"negationInd="true""#, "38341003"),
        // round 9 (K21R8-M2, K21R8-T4): "not the reason" and a reversed relationship are no reason
        indication("RSON", r#"negationInd="true""#, "", "44054006"),
        indication("RSON", r#"inversionInd="true""#, "", "386661006"),
    )))?;
    let visits = resources(&c, "Encounter");
    assert_eq!(visits.len(), 1);
    assert_eq!(visits[0]["reasonCode"].as_array().map(Vec::len), Some(1));
    assert_eq!(visits[0]["reasonCode"][0]["coding"][0]["code"], "233604007");
    assert!(resources(&c, "Condition").is_empty());
    let indication = "observation 2.16.840.1.113883.10.20.22.4.19";
    assert_eq!(
        c.sections[0].not_read,
        [
            indication,
            indication,
            indication,
            indication,
            "encounter 2.16.840.1.113883.10.20.22.4.49"
        ]
    );
    Ok(())
}

/// C-M2, U-M4, C-L3, M3, M13, M17, M18: a procedure not done, cancelled or new and an aborted
/// procedure observation wait in review, never skipped; an end before the start is dropped and
/// listed; an unknown part waits in review; a note stays on its procedure or observation.
#[test]
fn round_1_a_procedure_not_done_or_an_aborted_observation_waits_in_review() -> R {
    let procedure = |attrs: &str, status: &str, time: &str, inner: &str| {
        format!(
            r#"<entry><procedure classCode="PROC" moodCode="EVN" {attrs}>
              <templateId root="2.16.840.1.113883.10.20.22.4.14"/>
              <code code="73761001" codeSystem="2.16.840.1.113883.6.96" displayName="Colonoscopy"/>
              <statusCode code="{status}"/>{time}{inner}</procedure></entry>"#
        )
    };
    let observation = |status: &str, inner: &str| {
        format!(
            r#"<entry><observation classCode="OBS" moodCode="EVN">
              <templateId root="2.16.840.1.113883.10.20.22.4.13"/>
              <code code="274025005" codeSystem="2.16.840.1.113883.6.96" displayName="Colonic polypectomy"/>
              <statusCode code="{status}"/><effectiveTime value="20110203"/>{inner}</observation></entry>"#
        )
    };
    let unknown_part = r#"<entryRelationship typeCode="COMP"><observation moodCode="EVN"><templateId root="9.9.9"/></observation></entryRelationship>"#;
    let c = convert(&procedures(
        &[
            procedure(
                r#"negationInd="true""#,
                "completed",
                r#"<effectiveTime value="20200101"/>"#,
                "",
            ),
            procedure("", "cancelled", "", ""),
            procedure("", "new", "", ""),
            observation("aborted", ""),
            procedure(
                "",
                "completed",
                r#"<effectiveTime><low value="2010"/><high value="2009"/></effectiveTime>"#,
                "<text>Done under sedation</text>",
            ),
            procedure("", "completed", r#"<effectiveTime value="20200102"/>"#, unknown_part),
            observation("completed", "<text>Two polyps removed</text>"),
        ]
        .concat(),
    ))?;
    let p = "procedure 2.16.840.1.113883.10.20.22.4.14";
    assert_eq!(
        c.sections[0].not_read,
        [
            format!("{p} (not done)"),
            format!("{p} (status cancelled)"),
            format!("{p} (status new)"),
            "observation 2.16.840.1.113883.10.20.22.4.13 (status aborted)".into(),
            format!("{p} (ends before it starts)"),
            "observation 9.9.9".into(),
        ]
    );
    let kept = resources(&c, "Procedure");
    assert_eq!(kept.len(), 2);
    assert_eq!(kept[0]["performedPeriod"], json!({"start": "2010"}));
    assert_eq!(kept[0]["note"][0]["text"], "Done under sedation");
    assert_eq!(kept[1]["status"], "completed");
    let observations = resources(&c, "Observation");
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0]["note"][0]["text"], "Two polyps removed");
    assert_eq!(held_items(&c, "test", "p.xml")[0]["count"], 6);
    Ok(())
}

/// C-M1: Pregnancy and Sexual Orientation are reproductive and sexual health. Unlabelled, they are
/// not read, so the file stays behind read:sensitive and nothing becomes a value.
#[tokio::test]
async fn round_1_an_unlabelled_pregnancy_or_sexual_orientation_keeps_the_file_closed() -> R {
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    let p = as_isabella(&mut tx, &fam, "lucia").await?;
    let bytes = document(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.17"/><title>Social History</title>
          <entry><observation classCode="OBS" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.15.3.8"/>
            <code code="ASSERTION" codeSystem="2.16.840.1.113883.5.4"/><statusCode code="completed"/>
            <effectiveTime value="20210301"/>
            <value xsi:type="CD" code="77386006" codeSystem="2.16.840.1.113883.6.96" displayName="Pregnant"/>
          </observation></entry>
          <entry><observation classCode="OBS" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.501"/>
            <code code="76690-7" codeSystem="2.16.840.1.113883.6.1"/><statusCode code="completed"/>
            <effectiveTime value="20210301"/>
            <value xsi:type="CD" code="38628009" codeSystem="2.16.840.1.113883.6.96" displayName="Homosexual"/>
          </observation></entry>
        </section></component>"#,
    );
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &bytes, "social.xml", ccda::MIME).await?;
    import_ccda(&mut tx, &env.blobs, doc).await?;
    assert_eq!(count(&mut tx, "observation", p).await?, 0);
    assert!(open_items(&mut tx, doc, "candidate_row").await?.is_empty());
    let flags: Value = sqlx::query_scalar("select flags from document where id = $1")
        .bind(doc)
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(flags["unprojected"], 2, "{flags}");
    assert!(ownhealth_core::documents::document_is_sensitive(&mut tx, doc).await?);
    Ok(())
}

/// U-M2, M5, M12: a Notes section with text and no entry is one note kept in the file, and its
/// review item says so (not the narrative-only item).
#[test]
fn round_1_a_notes_section_with_text_only_is_a_stored_note_in_review() -> R {
    let c = convert(&document(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.65"/><title>Notes</title>
          <text><paragraph>Seen for a cough.</paragraph></text></section></component>"#,
    ))?;
    let notes = &c.sections[0];
    assert_eq!((notes.entries, notes.stored_notes), (0, 1));
    let held = held_items(&c, "test", "n.xml");
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0]["resource_type"], "Notes (C-CDA)");
    assert_eq!(held[0]["notes"], json!({"clinical note kept in the uploaded file": 1}));
    Ok(())
}

/// CodeRabbit round 7 (Notes): a Notes section whose text exhausts the scan before its first word is
/// one incomplete note in review, like a narrative-only section; an empty Notes section stays out.
#[test]
fn round_7_a_notes_section_cut_before_its_first_word_is_a_stored_note_in_review() -> R {
    let beyond = text_past_the_node_limit();
    for (template, title) in [
        ("2.16.840.1.113883.10.20.22.2.65", "Notes"),
        ("2.16.840.1.113883.10.20.22.2.999", "Other"),
    ] {
        let c = convert(&document(&format!(
            r#"<component><section><templateId root="{template}"/><title>{title}</title>
              <text>{beyond}</text></section></component>"#
        )))?;
        assert!(c.sections[0].narrative, "{title}");
        let held = held_items(&c, "test", "n.xml");
        assert_eq!(held.len(), 1, "{title}: {held:?}");
        if title == "Notes" {
            assert_eq!(c.sections[0].stored_notes, 1);
            assert_eq!(held[0]["notes"], json!({"clinical note kept in the uploaded file": 1}));
        } else {
            assert_eq!(c.sections[0].stored_notes, 0);
            assert_eq!(held[0]["resource_type"], ccda::NARRATIVE);
        }
    }
    let c = convert(&document(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.65"/><title>Notes</title>
          <text><paragraph> </paragraph></text></section></component>"#,
    ))?;
    assert_eq!((c.sections[0].narrative, c.sections[0].stored_notes), (false, 0));
    assert!(held_items(&c, "test", "n.xml").is_empty());
    Ok(())
}

/// Round 8 (K21R7-M1): an Encounters, Procedures or Social History section with no entry whose text
/// exhausts the scan before its first word still reaches review with the narrative-only sections.
#[test]
fn round_8_a_no_entry_section_cut_before_its_first_word_is_held_as_narrative() -> R {
    let beyond = text_past_the_node_limit();
    for (template, title) in [
        ("2.16.840.1.113883.10.20.22.2.22.1", "Encounters"),
        ("2.16.840.1.113883.10.20.22.2.7.1", "Procedures"),
        ("2.16.840.1.113883.10.20.22.2.17", "Social History"),
    ] {
        let c = convert(&document(&format!(
            r#"<component><section><templateId root="{template}"/><title>{title}</title>
              <text>{beyond}</text></section></component>"#
        )))?;
        assert!(c.sections[0].kind.is_some(), "{title}");
        assert_eq!((c.sections[0].entries, c.sections[0].narrative), (0, true), "{title}");
        let held = held_items(&c, "test", "s.xml");
        assert_eq!(held.len(), 1, "{title}: {held:?}");
        assert_eq!(held[0]["resource_type"], ccda::NARRATIVE, "{title}");
        assert_eq!(held[0]["notes"], json!({format!("{title} ({template})"): 1}), "{title}");
    }
    Ok(())
}

// ------------------------------------------------------ slice 21, round 2 ----

fn social_history(entries: &str) -> Vec<u8> {
    document(&format!(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.17"/><title>Social History</title>
           {entries}</section></component>"#
    ))
}

/// Round 9 (K21R8-T2): an observation held whole because its code's original text was cut before its
/// first word is one line in review; its original text is counted with the hold, not again.
#[test]
fn round_9_original_text_of_an_observation_held_whole_is_counted_once() -> R {
    let beyond = text_past_the_node_limit();
    let c = convert(&social_history(&format!(
        r#"<entry><observation classCode="OBS" moodCode="EVN">
             <templateId root="2.16.840.1.113883.10.20.22.4.38"/>
             <code><originalText>{beyond}</originalText></code>
             <statusCode code="completed"/><effectiveTime value="20210301"/>
             <value xsi:type="ST">daily</value>
           </observation></entry>"#
    )))?;
    assert!(resources(&c, "Observation").is_empty());
    assert_eq!(c.sections[0].not_read, ["observation 2.16.840.1.113883.10.20.22.4.38"]);
    Ok(())
}

/// Round 9 (K21R8-T6): a procedure whose status is unknown (a nullFlavor, or no statusCode) is not
/// recorded as completed.
#[test]
fn round_9_a_procedure_with_no_usable_status_is_unknown() -> R {
    for status in [
        r#"<statusCode nullFlavor="UNK"/>"#,
        "",
        r#"<statusCode code="completed"/>"#,
    ] {
        let c = convert(&procedures(&format!(
            r#"<entry><procedure classCode="PROC" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.14"/>
              <code code="73761001" codeSystem="2.16.840.1.113883.6.96" displayName="Colonoscopy"/>{status}
              <effectiveTime value="20200101"/></procedure></entry>"#
        )))?;
        let p = resources(&c, "Procedure");
        assert_eq!(p.len(), 1, "{status}");
        let want = if status.contains("completed") {
            "completed"
        } else {
            "unknown"
        };
        assert_eq!(p[0]["status"], want, "{status}");
        assert!(
            c.sections[0].not_read.is_empty(),
            "{status}: {:?}",
            c.sections[0].not_read
        );
    }
    Ok(())
}

/// Round 9 (K21R8-M1): a local original text that is only part of a word of the referenced narrative
/// ("reactive" inside "nonreactive") is a label of its own. It joins the projected text, or review lists
/// it, never neither. The same label in another case or spacing is still one label.
#[test]
fn round_9_a_local_original_text_inside_a_narrative_word_is_not_dropped() -> R {
    let observation = |display: &str, local: &str| {
        format!(
            r##"<text><paragraph ID="t1">nonreactive</paragraph></text>
           <entry><observation classCode="OBS" moodCode="EVN">
             <templateId root="2.16.840.1.113883.10.20.22.4.38"/>
             <code code="72166-2" codeSystem="2.16.840.1.113883.6.1"/>
             <statusCode code="completed"/><effectiveTime value="20210301"/>
             <value xsi:type="CD" code="266919005" codeSystem="2.16.840.1.113883.6.96" {display}>
               <originalText><reference value="#t1"/>{local}</originalText></value>
           </observation></entry>"##
        )
    };
    let read = |xml: String| -> anyhow::Result<(Value, Vec<String>)> {
        let c = convert(&social_history(&xml))?;
        let o = resources(&c, "Observation");
        anyhow::ensure!(o.len() == 1, "{o:?}");
        Ok((
            o[0]["valueCodeableConcept"]["text"].clone(),
            c.sections[0].not_read.clone(),
        ))
    };
    let unread = "value (original text kept in the uploaded file)";
    // the display keeps the narrative word only: review lists the local label
    assert_eq!(
        read(observation(r#"displayName="nonreactive""#, "reactive"))?,
        (json!("nonreactive"), vec![unread.to_string()])
    );
    // no display: the projected text keeps both labels
    assert_eq!(
        read(observation("", "reactive"))?,
        (json!("nonreactive; reactive"), vec![])
    );
    // the same label in another case and spacing adds nothing
    assert_eq!(
        read(observation(r#"displayName="nonreactive""#, " NonReactive "))?,
        (json!("nonreactive"), vec![])
    );
    Ok(())
}

#[tokio::test]
async fn round_2_excluded_observation_templates_override_social_history() -> R {
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    let p = as_isabella(&mut tx, &fam, "lucia").await?;
    let entries = [
        ("2.16.840.1.113883.10.20.22.4.501", "76690-7"),
        ("2.16.840.1.113883.10.20.15.3.8", "82810-3"),
        ("2.16.840.1.113883.10.20.22.4.281", "86645-9"),
        ("2.16.840.1.113883.10.20.15.3.1", "11778-8"),
    ]
    .map(|(excluded, code)| {
        format!(
            r#"<entry><observation classCode="OBS" moodCode="EVN">
                 <templateId root="{excluded}"/>
                 <templateId root="2.16.840.1.113883.10.20.22.4.38"/>
                 <code code="{code}" codeSystem="2.16.840.1.113883.6.1"/>
                 <statusCode code="completed"/><effectiveTime value="20210301"/>
                 <value xsi:type="ST">Synthetic answer</value>
               </observation></entry>"#
        )
    })
    .concat();
    let bytes = social_history(&entries);
    let converted = convert(&bytes)?;
    assert_eq!(resources(&converted, "Observation").len(), 0);
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &bytes, "social.xml", ccda::MIME).await?;
    import_ccda(&mut tx, &env.blobs, doc).await?;
    assert_eq!(count(&mut tx, "observation", p).await?, 0);
    assert!(open_items(&mut tx, doc, "candidate_row").await?.is_empty());
    let flags: Value = sqlx::query_scalar("select flags from document where id = $1")
        .bind(doc)
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(flags["unprojected"], 4, "{flags}");
    assert!(ownhealth_core::documents::document_is_sensitive(&mut tx, doc).await?);
    Ok(())
}

#[tokio::test]
async fn round_2_sensitive_loinc_codes_protect_unlabelled_social_history_and_its_file() -> R {
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    as_isabella(&mut tx, &fam, "lucia").await?;
    let entries = [
        r#"<code code="86647-5" codeSystem="2.16.840.1.113883.6.1"/>"#,
        r#"<code code="local" codeSystem="1.2.3"><translation code="8665-2" codeSystem="2.16.840.1.113883.6.1"/></code>"#,
        r#"<code code="44261-6" codeSystem="2.16.840.1.113883.6.1"/>"#,
        r#"<code code="72166-2" codeSystem="2.16.840.1.113883.6.1"/>"#,
        r#"<code code="86647-5" codeSystem="2.16.840.1.113883.6.96"/>"#,
        r#"<code code="86649-1" codeSystem="2.16.840.1.113883.6.1"/>"#,
    ]
    .map(|code| {
        format!(
            r#"<entry><observation classCode="OBS" moodCode="EVN">
                 <templateId root="2.16.840.1.113883.10.20.22.4.38"/>
                 {code}<statusCode code="completed"/><effectiveTime value="20210301"/>
                 <value xsi:type="ST">Synthetic answer</value>
               </observation></entry>"#
        )
    })
    .concat();
    let bytes = social_history(&entries);
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &bytes, "coded.xml", ccda::MIME).await?;
    import_ccda(&mut tx, &env.blobs, doc).await?;
    let flags: Value = sqlx::query_scalar("select flags from document where id = $1")
        .bind(doc)
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(flags["sensitive"], 4, "{flags}");
    assert!(flags.get("unprojected").is_none(), "{flags}");
    assert!(ownhealth_core::documents::document_is_sensitive(&mut tx, doc).await?);

    let candidates = open_items(&mut tx, doc, "candidate_row").await?;
    let by_loinc: BTreeMap<_, _> = candidates
        .iter()
        .map(|item| {
            let obs = &item.payload["observation"];
            (obs["loinc"].as_str(), obs["sensitivity"].clone())
        })
        .collect();
    assert_eq!(
        by_loinc,
        BTreeMap::from([
            (None, json!([])),
            (Some("44261-6"), json!(["mental_health"])),
            (Some("72166-2"), json!([])),
            (Some("8665-2"), json!(["reproductive"])),
            (Some("86647-5"), json!(["sexual_health"])),
            (Some("86649-1"), json!(["reproductive"])),
        ])
    );
    Ok(())
}

#[test]
fn round_2_notes_with_a_security_entry_are_counted_in_review() -> R {
    let c = convert(&document(&format!(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.65"/><title>Notes</title>
             <text><paragraph>Seen for a cough.</paragraph></text><entry>{}</entry>
           </section></component>"#,
        security_label("PSY")
    )))?;
    let notes = &c.sections[0];
    assert_eq!((notes.entries, notes.stored_notes), (1, 1));
    let held = held_items(&c, "test", "n.xml");
    assert_eq!(held.len(), 1);
    assert_eq!(held[0]["notes"], json!({"clinical note kept in the uploaded file": 1}));
    Ok(())
}

#[test]
fn round_2_an_encounter_with_reversed_dates_keeps_only_its_start() -> R {
    let c = convert(&encounters(
        r#"<entry><encounter moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.49"/>
             <effectiveTime><low value="2010"/><high value="2009"/></effectiveTime>
           </encounter></entry>"#,
    ))?;
    assert_eq!(resources(&c, "Encounter")[0]["period"], json!({"start": "2010"}));
    let held = held_items(&c, "test", "enc.xml");
    assert_eq!(held.len(), 1);
    assert_eq!(
        held[0]["notes"],
        json!({"not read: encounter 2.16.840.1.113883.10.20.22.4.49 (ends before it starts)": 1})
    );
    Ok(())
}

#[test]
fn round_2_an_activity_period_compares_full_instants_when_available() -> R {
    let entries = [
        ("202103021200+0000", "202103021100+0000"),
        ("202103021200+0200", "202103021100+0000"),
        ("202103020030+0200", "202103012330+0000"),
    ]
    .map(|(low, high)| {
        format!(
            r#"<entry><procedure moodCode="EVN">
                 <code code="73761001" codeSystem="2.16.840.1.113883.6.96"/>
                 <effectiveTime><low value="{low}"/><high value="{high}"/></effectiveTime>
               </procedure></entry>"#
        )
    })
    .concat();
    let c = convert(&procedures(&entries))?;
    let visits = resources(&c, "Procedure");
    assert_eq!(visits.len(), 3);
    assert_eq!(visits[0]["performedPeriod"], json!({"start": "2021-03-02T12:00:00Z"}));
    assert_eq!(visits[1]["performedPeriod"]["end"], "2021-03-02T11:00:00Z");
    assert_eq!(visits[2]["performedPeriod"]["end"], "2021-03-01T23:30:00Z");
    assert_eq!(
        c.sections[0].not_read,
        ["procedure without a templateId (ends before it starts)"]
    );
    Ok(())
}

// ------------------------------------------------------ slice 21, round 3 ----

#[tokio::test]
async fn round_3_encounter_text_kept_only_in_xml_is_counted() -> R {
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    let p = as_isabella(&mut tx, &fam, "lucia").await?;
    let mut unread = Vec::new();
    for (i, (entry_text, reference)) in [
        ("<text>Synthetic visit detail</text>", None),
        (
            r##"<text><reference value="#visit"/>Synthetic visit detail</text>"##,
            Some("#visit"),
        ),
        (r##"<text><reference value="#visit"/></text>"##, Some("#visit")),
        ("<text><content>Synthetic visit detail</content></text>", None),
    ]
    .into_iter()
    .enumerate()
    {
        let day = i + 1;
        let bytes = encounters(&format!(
            r#"<text><paragraph ID="visit">Office visit</paragraph></text>
               <entry><encounter moodCode="EVN">
                 <code code="99213" codeSystem="2.16.840.1.113883.6.12"/>
                 {entry_text}<statusCode code="completed"/><effectiveTime value="2021030{day}"/>
               </encounter></entry>"#
        ));
        let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &bytes, "encounter.xml", ccda::MIME).await?;
        import_ccda(&mut tx, &env.blobs, doc).await?;
        let flags: Value = sqlx::query_scalar("select flags from document where id = $1")
            .bind(doc)
            .fetch_one(&mut *tx)
            .await?;
        unread.push(flags["unprojected"].as_i64());
        let visit: Value = sqlx::query_scalar(
            "select e.fhir from encounter e join src s on s.id = e.src_id where e.profile_id = $1 and s.document_id = $2",
        )
        .bind(p)
        .bind(doc)
        .fetch_one(&mut *tx)
        .await?;
        assert_eq!(visit["extension"][0]["valueString"].as_str(), reference);
        assert!(!visit.to_string().contains("Synthetic visit detail"));
    }
    assert_eq!(unread, [Some(1), Some(1), None, Some(1)]);
    Ok(())
}

#[tokio::test]
async fn round_3_social_history_counts_original_text_missing_from_the_coded_answer() -> R {
    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    as_isabella(&mut tx, &fam, "lucia").await?;
    let mut unread = Vec::new();
    let long_answer = "a".repeat(2_000);
    let long_display = format!(r#"displayName="{long_answer}""#);
    let empty_elements = "<content/>".repeat(20_000);
    for (display, original_text, expected_text) in [
        (
            r#"displayName="Never smoker""#,
            "<originalText>Additional detail</originalText>",
            "Never smoker",
        ),
        (
            r#"displayName="Never smoker""#,
            r##"<originalText><reference value="#extra"/></originalText>"##,
            "Never smoker",
        ),
        (
            r#"displayName="Never smoker""#,
            r##"<originalText><reference value="#answer"/>Additional detail</originalText>"##,
            "Never smoker",
        ),
        (
            r#"displayName="Never smoker""#,
            "<originalText> Never  smoker </originalText>",
            "Never smoker",
        ),
        (
            r#"displayName="Never smoker""#,
            r##"<originalText><reference value="#answer"/></originalText>"##,
            "Never smoker",
        ),
        ("", "<originalText>Never smoker</originalText>", "Never smoker"),
        (
            long_display.as_str(),
            r##"<originalText><reference value="#long"/></originalText>"##,
            long_answer.as_str(),
        ),
        (
            r#"displayName="Never smoker""#,
            r##"<originalText><reference value="#many-nodes"/></originalText>"##,
            "Never smoker",
        ),
        (
            r#"displayName="Never smoker""#,
            r#"<originalText>Never smoked tobacco</originalText>
               <translation code="X1" codeSystem="1.2.3" displayName="Never smoked tobacco"/>"#,
            "Never smoker",
        ),
    ] {
        let bytes = social_history(&format!(
            r#"<text>
                 <paragraph ID="answer">Never smoker</paragraph>
                 <paragraph ID="extra">Additional detail</paragraph>
                 <paragraph ID="long">{long_answer}Additional detail</paragraph>
                 <paragraph ID="many-nodes">Never smoker{empty_elements}Additional detail</paragraph>
               </text>
               <entry><observation classCode="OBS" moodCode="EVN">
                 <code code="72166-2" codeSystem="2.16.840.1.113883.6.1"/>
                 <statusCode code="completed"/><effectiveTime value="20210301"/>
                 <value xsi:type="CD" code="266919005" codeSystem="2.16.840.1.113883.6.96" {display}>
                   {original_text}
                 </value>
               </observation></entry>"#
        ));
        let converted = convert(&bytes)?;
        let value = &resources(&converted, "Observation")[0]["valueCodeableConcept"];
        assert_eq!(value["coding"][0]["code"], "266919005");
        assert_eq!(value["text"], expected_text);
        let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &bytes, "social.xml", ccda::MIME).await?;
        import_ccda(&mut tx, &env.blobs, doc).await?;
        let flags: Value = sqlx::query_scalar("select flags from document where id = $1")
            .bind(doc)
            .fetch_one(&mut *tx)
            .await?;
        unread.push(flags["unprojected"].as_i64());
    }
    assert_eq!(
        unread,
        [Some(1), Some(1), Some(1), None, None, None, Some(1), Some(1), None]
    );
    Ok(())
}

// ------------------------------------------------------ slice 21, round 4 ----

#[test]
fn round_4_original_text_omitted_from_coded_concepts_goes_to_review() -> R {
    let history = social_history(
        r#"<entry><observation classCode="OBS" moodCode="EVN">
             <code code="72166-2" codeSystem="2.16.840.1.113883.6.1" displayName="Tobacco smoking status">
               <originalText>Smokes cigars at weekend parties</originalText>
             </code>
             <statusCode code="completed"/><effectiveTime value="20210301"/>
             <value xsi:type="CD" code="8517006" codeSystem="2.16.840.1.113883.6.96"/>
           </observation></entry>"#,
    );
    let procedure = procedures(
        r#"<entry><procedure classCode="PROC" moodCode="EVN">
             <code code="73761001" codeSystem="2.16.840.1.113883.6.96" displayName="Colonoscopy">
               <originalText>Colonoscopy, two polyps removed, biopsy sent</originalText>
             </code>
             <statusCode code="completed"/><effectiveTime value="20210301"/>
           </procedure></entry>"#,
    );
    let encounter = encounters(
        r#"<entry><encounter moodCode="EVN">
             <code code="99213" codeSystem="2.16.840.1.113883.6.12" displayName="Office visit">
               <originalText>Visit at the afternoon clinic</originalText>
             </code>
             <statusCode code="completed"/><effectiveTime value="20210301"/>
             <entryRelationship typeCode="RSON"><observation classCode="OBS" moodCode="EVN">
               <templateId root="2.16.840.1.113883.10.20.22.4.19"/>
               <code code="404684003" codeSystem="2.16.840.1.113883.6.96"/><statusCode code="completed"/>
               <value xsi:type="CD" code="233604007" codeSystem="2.16.840.1.113883.6.96" displayName="Pneumonia">
                 <originalText>Pneumonia after contact with a partner diagnosed with HIV</originalText>
               </value>
             </observation></entryRelationship>
           </encounter></entry>"#,
    );
    let mut unread = Vec::new();
    for (bytes, resource_type, pointer, display) in [
        (history, "Observation", "/code/text", "Tobacco smoking status"),
        (procedure, "Procedure", "/code/text", "Colonoscopy"),
        (encounter, "Encounter", "/reasonCode/0/text", "Pneumonia"),
    ] {
        let c = convert(&bytes)?;
        let resource = resources(&c, resource_type)[0];
        assert_eq!(resource.pointer(pointer).and_then(Value::as_str), Some(display));
        if resource_type == "Encounter" {
            assert_eq!(resource["type"][0]["text"], "Visit at the afternoon clinic");
        }
        let held = held_items(&c, "test", "coded.xml");
        unread.push(held.iter().map(|h| h["count"].as_u64().unwrap_or(0)).sum::<u64>());
    }
    assert_eq!(unread, [1, 1, 1]);
    Ok(())
}

#[test]
fn round_4_coded_result_original_text_matches_display_ignoring_case_and_whitespace() -> R {
    let mut unread = Vec::new();
    for (display, narrative) in [
        ("Negative", "negative"),
        ("DÉTECTÉ&#x9; &#xA0; FAIBLEMENT", "détecté faiblement"),
        ("Negative", "Not negative"),
    ] {
        let c = convert(&document(&format!(
            r##"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.3.1"/><title>Results</title>
              <text><content ID="result">{narrative}</content></text>
              <entry><observation classCode="OBS" moodCode="EVN">
                <code code="5195-3" codeSystem="2.16.840.1.113883.6.1"/>
                <statusCode code="completed"/><effectiveTime value="20210301"/>
                <value xsi:type="CD" code="260385009" codeSystem="2.16.840.1.113883.6.96" displayName="{display}">
                  <originalText><reference value="#result"/></originalText>
                </value>
              </observation></entry></section></component>"##
        )))?;
        unread.push(c.sections[0].not_read.len());
    }
    assert_eq!(unread, [0, 0, 1]);
    Ok(())
}

#[test]
fn round_4_original_text_omitted_from_the_document_type_or_a_translation_goes_to_review() -> R {
    let bytes = procedures(
        r#"<entry><procedure classCode="PROC" moodCode="EVN">
             <code code="73761001" codeSystem="2.16.840.1.113883.6.96">
               <translation code="X1" codeSystem="1.2.3" displayName="Colonoscopy">
                 <originalText>Additional details from the local procedure record</originalText>
               </translation>
             </code>
             <statusCode code="completed"/><effectiveTime value="20210301"/>
           </procedure></entry>"#,
    );
    let text = String::from_utf8(bytes)?.replace(
        r#"displayName="Summary of episode note"/>"#,
        r#"displayName="Summary of episode note"><originalText>Private referral details</originalText></code>"#,
    );
    let c = convert(text.as_bytes())?;
    assert_eq!(
        resources(&c, "Composition")[0]["type"]["text"],
        "Summary of episode note"
    );
    assert_eq!(
        resources(&c, "Procedure")[0]["code"]["coding"][1]["display"],
        "Colonoscopy"
    );
    let held = held_items(&c, "test", "coded.xml");
    assert_eq!(held.iter().map(|h| h["count"].as_u64().unwrap_or(0)).sum::<u64>(), 2);
    Ok(())
}

#[test]
fn round_4_truncated_original_text_goes_to_review_even_when_used_as_the_label() -> R {
    let long = "a".repeat(2_001);
    let bytes = procedures(&format!(
        r#"<entry><procedure classCode="PROC" moodCode="EVN">
             <code code="73761001" codeSystem="2.16.840.1.113883.6.96"><originalText>{long}</originalText></code>
             <statusCode code="completed"/><effectiveTime value="20210301"/>
           </procedure></entry>"#
    ));
    let c = convert(&bytes)?;
    assert_eq!(resources(&c, "Procedure")[0]["code"]["text"], &long[..2_000]);
    assert_eq!(c.sections[0].not_read.len(), 1);
    Ok(())
}

// ------------------------------------------------------ slice 21, round 5 ----

/// What the converter lists as not read in every section, in document order.
fn unread_lines(bytes: &[u8]) -> anyhow::Result<Vec<String>> {
    let c = convert(bytes)?;
    Ok(c.sections.iter().flat_map(|s| s.not_read.clone()).collect())
}

/// R4-M1: original text of a coded element the converter never reads (a method, a target site, an
/// indication's own code, an interpretation) exists only in the XML, so it goes to review. Text
/// inside a part already held whole, a bare reference to the section narrative and blank text add
/// no line.
#[test]
fn round_5_original_text_of_unread_coded_elements_goes_to_review() -> R {
    let kept = |tag: &str| format!("{tag} (original text kept in the uploaded file)");
    let method = procedures(
        r#"<entry><procedure classCode="PROC" moodCode="EVN">
             <code code="73761001" codeSystem="2.16.840.1.113883.6.96" displayName="Colonoscopy"/>
             <statusCode code="completed"/><effectiveTime value="20210301"/>
             <methodCode code="X" codeSystem="1.2.3" displayName="Method"><originalText>Done under sedation after HIV test</originalText></methodCode>
             <approachSiteCode code="Y" codeSystem="1.2.3" displayName="Approach"/>
             <priorityCode code="EM" codeSystem="2.16.840.1.113883.5.7" displayName="Emergency"/>
           </procedure></entry>"#,
    );
    let site = procedures(
        r#"<entry><observation classCode="OBS" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.13"/>
             <code code="274025005" codeSystem="2.16.840.1.113883.6.96" displayName="Colonic polypectomy"/>
             <statusCode code="completed"/><effectiveTime value="20210301"/>
             <targetSiteCode code="71854001" codeSystem="2.16.840.1.113883.6.96" displayName="Colon"><originalText>Rectum near prior anal fistula</originalText></targetSiteCode>
             <methodCode code="X" codeSystem="1.2.3" displayName="Method"/>
           </observation></entry>"#,
    );
    let indication = encounters(
        r#"<entry><encounter moodCode="EVN"><code code="99213" codeSystem="2.16.840.1.113883.6.12"/>
             <statusCode code="completed"/><effectiveTime value="20210301"/>
             <entryRelationship typeCode="RSON"><observation classCode="OBS" moodCode="EVN">
               <templateId root="2.16.840.1.113883.10.20.22.4.19"/>
               <code code="404684003" codeSystem="2.16.840.1.113883.6.96" displayName="Finding"><originalText>Seen after assault by partner</originalText></code>
               <statusCode code="completed"/>
               <value xsi:type="CD" code="233604007" codeSystem="2.16.840.1.113883.6.96" displayName="Pneumonia"/>
             </observation></entryRelationship>
           </encounter></entry>"#,
    );
    let interpretation = document(
        r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.3.1"/><title>Results</title>
             <entry><observation classCode="OBS" moodCode="EVN">
               <code code="2345-7" codeSystem="2.16.840.1.113883.6.1" displayName="Glucose"/>
               <statusCode code="completed"/><effectiveTime value="20210301"/><value xsi:type="PQ" value="300" unit="mg/dL"/>
               <interpretationCode code="HH" codeSystem="2.16.840.1.113883.5.83" displayName="Critical high"><originalText>Critical, phoned to Dr Ruiz who said patient is pregnant</originalText></interpretationCode>
             </observation></entry></section></component>"#,
    );
    let held_part = procedures(
        r#"<entry><procedure classCode="PROC" moodCode="EVN">
             <code code="73761001" codeSystem="2.16.840.1.113883.6.96" displayName="Colonoscopy"/>
             <statusCode code="completed"/><effectiveTime value="20210301"/>
             <entryRelationship typeCode="COMP"><observation classCode="OBS" moodCode="EVN">
               <code code="X" codeSystem="1.2.3" displayName="Finding"><originalText>Held with its observation</originalText></code>
             </observation></entryRelationship>
           </procedure></entry>
           <entry><procedure classCode="PROC" moodCode="EVN">
             <code code="73761001" codeSystem="2.16.840.1.113883.6.96" displayName="Colonoscopy"/>
             <statusCode code="cancelled"/><effectiveTime value="20210301"/>
             <methodCode code="X" codeSystem="1.2.3"><originalText>Held with its procedure</originalText></methodCode>
           </procedure></entry>"#,
    );
    let no_words = procedures(
        r##"<text><paragraph ID="m1">Under sedation</paragraph></text>
           <entry><procedure classCode="PROC" moodCode="EVN">
             <code code="73761001" codeSystem="2.16.840.1.113883.6.96" displayName="Colonoscopy"/>
             <statusCode code="completed"/><effectiveTime value="20210301"/>
             <methodCode code="X" codeSystem="1.2.3"><originalText><reference value="#m1"/></originalText></methodCode>
             <approachSiteCode code="Y" codeSystem="1.2.3"><originalText>
             </originalText></approachSiteCode>
           </procedure></entry>"##,
    );
    assert_eq!(unread_lines(&method)?, [kept("methodCode")]);
    assert_eq!(unread_lines(&site)?, [kept("targetSiteCode")]);
    assert_eq!(unread_lines(&indication)?, [kept("code")]);
    assert_eq!(unread_lines(&interpretation)?, [kept("interpretationCode")]);
    assert_eq!(
        unread_lines(&held_part)?,
        [
            "observation without a templateId",
            "procedure without a templateId (status cancelled)"
        ]
    );
    assert_eq!(unread_lines(&no_words)?, Vec::<String>::new());
    // the record still carries what it read
    let c = convert(&indication)?;
    assert_eq!(resources(&c, "Encounter")[0]["reasonCode"][0]["text"], "Pneumonia");
    let c = convert(&interpretation)?;
    assert_eq!(
        resources(&c, "Observation")[0]["interpretation"][0]["coding"][0]["code"],
        "HH"
    );
    Ok(())
}

// ------------------------------------------------------ slice 21, round 6 ----

/// An Encounter whose only unread content is `inner`, inside an eligible RSON Indication with a
/// coded value (its reason is read).
fn encounter_with_indication(narrative: &str, inner: &str) -> Vec<u8> {
    encounters(&format!(
        r#"<text>{narrative}</text>
           <entry><encounter moodCode="EVN"><code code="99213" codeSystem="2.16.840.1.113883.6.12"/>
             <statusCode code="completed"/><effectiveTime value="20210301"/>
             <entryRelationship typeCode="RSON"><observation classCode="OBS" moodCode="EVN">
               <templateId root="2.16.840.1.113883.10.20.22.4.19"/>
               <code code="404684003" codeSystem="2.16.840.1.113883.6.96"/>{inner}<statusCode code="completed"/>
               <value xsi:type="CD" code="233604007" codeSystem="2.16.840.1.113883.6.96" displayName="Pneumonia"/>
             </observation></entryRelationship>
           </encounter></entry>"#
    ))
}

/// CR-IND: an Indication's own `<text>` is no part of the reason the Encounter keeps, so words it
/// holds (alone or beside a reference) go to review and flag the document. A bare reference adds no
/// words of its own (they are in the section narrative), as for the Encounter's own text.
#[tokio::test]
async fn round_6_an_indications_own_text_goes_to_review() -> R {
    let line = "indication (text kept in the uploaded file)";
    let narrative = r#"<paragraph ID="why">Pneumonia</paragraph>"#;
    for (inner, expected) in [
        ("<text>Seen after assault by partner</text>", vec![line]),
        (
            r##"<text><reference value="#why"/>Seen after assault by partner</text>"##,
            vec![line],
        ),
        ("<text><content>Seen after assault</content></text>", vec![line]),
        (r##"<text><reference value="#why"/></text>"##, vec![]),
        ("<text>  </text>", vec![]),
    ] {
        let bytes = encounter_with_indication(narrative, inner);
        assert_eq!(unread_lines(&bytes)?, expected, "{inner}");
        let c = convert(&bytes)?;
        let visit = resources(&c, "Encounter")[0];
        assert_eq!(visit["reasonCode"][0]["text"], "Pneumonia");
        assert!(!visit.to_string().contains("assault"));
    }

    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    as_isabella(&mut tx, &fam, "lucia").await?;
    let bytes = encounter_with_indication(narrative, "<text>Seen after assault by partner</text>");
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &bytes, "encounter.xml", ccda::MIME).await?;
    import_ccda(&mut tx, &env.blobs, doc).await?;
    let flags: Value = sqlx::query_scalar("select flags from document where id = $1")
        .bind(doc)
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(flags["unprojected"], 1, "{flags}");
    Ok(())
}

/// CR-PROC: a Procedure or procedure Observation note is kept up to 2,000 characters. When the
/// note's own text, the narrative it points at, or the two joined pass that bound, the cut part is
/// only in the file, so the note goes to review. A note within the bound adds nothing.
#[test]
fn round_6_a_note_cut_at_the_text_bound_goes_to_review() -> R {
    let under = "a".repeat(1_999);
    let at = "b".repeat(2_000);
    let over = "c".repeat(2_001);
    let near = "d".repeat(1_995);
    let narrative = format!(
        r#"<paragraph ID="under">{under}</paragraph><paragraph ID="over">{over}</paragraph><paragraph ID="near">{near}</paragraph>"#
    );
    let procedure = |text: &str| {
        procedures(&format!(
            r#"<text>{narrative}</text>
               <entry><procedure classCode="PROC" moodCode="EVN">
                 <code code="73761001" codeSystem="2.16.840.1.113883.6.96" displayName="Colonoscopy"/>
                 <text>{text}</text>
                 <statusCode code="completed"/><effectiveTime value="20210301"/>
               </procedure></entry>"#
        ))
    };
    let observation = |text: &str| {
        procedures(&format!(
            r#"<text>{narrative}</text>
               <entry><observation classCode="OBS" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.13"/>
                 <code code="274025005" codeSystem="2.16.840.1.113883.6.96" displayName="Colonic polypectomy"/>
                 <text>{text}</text>
                 <statusCode code="completed"/><effectiveTime value="20210301"/>
               </observation></entry>"#
        ))
    };
    let cut = |what: &str| format!("{what} (note scan limit reached)");
    let cases = [
        (under.clone(), under.clone(), false),
        (at.clone(), at.clone(), false),
        (r##"<reference value="#under"/>"##.to_string(), under.clone(), false),
        (over.clone(), over[..2_000].to_string(), true),
        (
            r##"<reference value="#over"/>"##.to_string(),
            over[..2_000].to_string(),
            true,
        ),
        (
            r##"<reference value="#near"/>Done under sedation"##.to_string(),
            format!("{}; Done under sedation", "d".repeat(1_979)),
            true,
        ),
        // the narrative and the words beside the reference joined fill the bound exactly
        (
            r##"<reference value="#near"/>abc"##.to_string(),
            format!("{near}; abc"),
            false,
        ),
    ];
    for (rtype, make, what) in [
        (
            "Procedure",
            &procedure as &dyn Fn(&str) -> Vec<u8>,
            "procedure without a templateId",
        ),
        (
            "Observation",
            &observation,
            "observation 2.16.840.1.113883.10.20.22.4.13",
        ),
    ] {
        for (text, note, flagged) in &cases {
            let bytes = make(text);
            let c = convert(&bytes)?;
            let kept = resources(&c, rtype)[0]["note"][0]["text"].as_str();
            assert_eq!(kept, Some(note.as_str()), "{rtype} {}", &text[..text.len().min(40)]);
            let expected = if *flagged { vec![cut(what)] } else { vec![] };
            assert_eq!(
                unread_lines(&bytes)?,
                expected,
                "{rtype} {}",
                &text[..text.len().min(40)]
            );
        }
    }
    Ok(())
}

// ------------------------------------------------------ slice 21, round 7 ----

const NOTE_ACTIVITIES: [(&str, &str, &str); 2] = [
    ("Procedure", "procedure", "2.16.840.1.113883.10.20.22.4.14"),
    ("Observation", "observation", "2.16.840.1.113883.10.20.22.4.13"),
];

fn activity_with_note(tag: &str, template: &str, status: &str, narrative: &str, text: &str) -> Vec<u8> {
    procedures(&format!(
        r#"<text>{narrative}</text>
           <entry><{tag} moodCode="EVN">
             <templateId root="{template}"/>
             <code code="73761001" codeSystem="2.16.840.1.113883.6.96" displayName="Colonoscopy"/>
             <text>{text}</text>
             <statusCode code="{status}"/><effectiveTime value="20210301"/>
           </{tag}></entry>"#
    ))
}

#[test]
fn round_7_nullified_procedure_activities_are_held() -> R {
    for (rtype, tag, template) in NOTE_ACTIVITIES {
        let bytes = activity_with_note(tag, template, "nullified", "", "Done under sedation");
        let c = convert(&bytes)?;
        assert_eq!(c.sections[0].not_read, [format!("{tag} {template} (status nullified)")]);
        assert!(resources(&c, rtype).is_empty(), "{rtype}");
    }
    Ok(())
}

fn text_past_the_node_limit() -> String {
    format!("{}<content>Unread detail</content>", "<content/>".repeat(20_000))
}

#[tokio::test]
async fn round_7_an_incomplete_note_scan_is_listed_even_without_words() -> R {
    let beyond = text_past_the_node_limit();
    let narrative =
        format!(r#"<paragraph ID="late">{beyond}</paragraph><paragraph ID="short">Target words</paragraph>"#);
    let reference_and_late_local = format!(r##"<reference value="#short"/>{beyond}"##);
    for (rtype, tag, template) in NOTE_ACTIVITIES {
        for (text, retained, cut) in [
            (beyond.as_str(), None, true),
            (r##"<reference value="#late"/>"##, None, true),
            (r##"<reference value="#late"/>Local words"##, Some("Local words"), true),
            (reference_and_late_local.as_str(), Some("Target words"), true),
            ("<paragraph> </paragraph>", None, false),
        ] {
            let bytes = activity_with_note(tag, template, "completed", &narrative, text);
            let c = convert(&bytes)?;
            let activity = resources(&c, rtype)[0];
            assert_eq!(activity["note"][0]["text"].as_str(), retained, "{rtype}");
            assert_eq!(activity.get("note").is_some(), retained.is_some(), "{rtype}");
            let expected = if cut {
                vec![format!("{tag} {template} (note scan limit reached)")]
            } else {
                vec![]
            };
            assert_eq!(c.sections[0].not_read, expected, "{rtype}, retained: {retained:?}");
        }
    }

    let env = Env::new().await?;
    let mut tx = env.tx().await?;
    let fam = family(&mut tx).await?;
    as_isabella(&mut tx, &fam, "lucia").await?;
    let (_, tag, template) = NOTE_ACTIVITIES[0];
    let bytes = activity_with_note(tag, template, "completed", "", &beyond);
    let doc = store_doc(&mut tx, &env.blobs, &fam, "lucia", &bytes, "procedure.xml", ccda::MIME).await?;
    import_ccda(&mut tx, &env.blobs, doc).await?;
    let flags: Value = sqlx::query_scalar("select flags from document where id = $1")
        .bind(doc)
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(flags["unprojected"], 1, "{flags}");
    assert!(ownhealth_core::documents::document_is_sensitive(&mut tx, doc).await?);
    Ok(())
}

fn observation_with_narrative(social: bool, extra: &str) -> Vec<u8> {
    let template = if social {
        "2.16.840.1.113883.10.20.22.4.38"
    } else {
        "2.16.840.1.113883.10.20.22.4.2"
    };
    let beyond = text_past_the_node_limit();
    let contents = format!(
        r#"<text><paragraph ID="late">{beyond}</paragraph></text>
           <entry><observation classCode="OBS" moodCode="EVN">
             <templateId root="{template}"/>
             <code code="48767-8" codeSystem="2.16.840.1.113883.6.1"/>
             <statusCode code="completed"/><effectiveTime value="20210301"/>{extra}
           </observation></entry>"#
    );
    if social {
        social_history(&contents)
    } else {
        document(&format!(
            r#"<component><section><templateId root="2.16.840.1.113883.10.20.22.2.3.1"/><title>Results</title>
               {contents}</section></component>"#
        ))
    }
}

#[test]
fn round_7_an_incomplete_social_history_text_value_is_listed() -> R {
    let bytes = observation_with_narrative(
        true,
        r##"<value xsi:type="ST"><reference value="#late"/>Local answer</value>"##,
    );
    let c = convert(&bytes)?;
    assert_eq!(resources(&c, "Observation")[0]["valueString"], "Local answer");
    assert_eq!(c.sections[0].not_read, ["observation value (text scan limit reached)"]);
    Ok(())
}

#[test]
fn round_7_an_incomplete_result_text_value_is_listed_even_without_words() -> R {
    let bytes = observation_with_narrative(false, r##"<value xsi:type="ST"><reference value="#late"/></value>"##);
    let c = convert(&bytes)?;
    assert!(resources(&c, "Observation")[0].get("valueString").is_none());
    assert_eq!(c.sections[0].not_read, ["observation value (text scan limit reached)"]);
    Ok(())
}

#[test]
fn round_7_an_incomplete_medication_sig_is_listed_even_without_words() -> R {
    let bytes = medicine_with(&text_past_the_node_limit(), "", "");
    let c = convert(&bytes)?;
    assert!(resources(&c, "MedicationStatement")[0].get("dosage").is_none());
    assert_eq!(
        c.sections[0].not_read,
        ["medication instructions (text scan limit reached)"]
    );
    Ok(())
}

#[test]
fn round_7_an_incomplete_reference_range_text_is_listed_even_without_words() -> R {
    let bytes = observation_with_narrative(
        false,
        r##"<value xsi:type="PQ" value="5"/>
            <referenceRange><observationRange><text><reference value="#late"/></text></observationRange></referenceRange>"##,
    );
    let c = convert(&bytes)?;
    assert!(resources(&c, "Observation")[0].get("referenceRange").is_none());
    assert_eq!(c.sections[0].not_read, ["reference range (text scan limit reached)"]);
    Ok(())
}

#[test]
fn round_7_incomplete_original_text_is_listed_even_without_a_coded_value() -> R {
    let bytes = observation_with_narrative(
        false,
        r##"<value xsi:type="CD" nullFlavor="OTH"><originalText><reference value="#late"/></originalText></value>"##,
    );
    let c = convert(&bytes)?;
    assert!(resources(&c, "Observation")[0].get("valueCodeableConcept").is_none());
    assert_eq!(
        c.sections[0].not_read,
        ["value (original text kept in the uploaded file)"]
    );
    Ok(())
}

#[test]
fn round_7_trailing_whitespace_and_empty_markup_do_not_cut_a_note() -> R {
    let full = "é".repeat(2_000);
    for (rtype, tag, template) in NOTE_ACTIVITIES {
        for (tail, unread) in [
            (" ", 0),
            ("\n<paragraph> &#160; </paragraph><!-- end -->", 0),
            ("<content>z</content>", 1),
            (" z", 1),
        ] {
            let bytes = activity_with_note(tag, template, "completed", "", &format!("{full}{tail}"));
            let c = convert(&bytes)?;
            assert_eq!(
                resources(&c, rtype)[0]["note"][0]["text"].as_str(),
                Some(full.as_str()),
                "{rtype}"
            );
            assert_eq!(c.sections[0].not_read.len(), unread, "{rtype}: {tail:?}");
        }
    }
    Ok(())
}

// ------------------------------------------------------ slice 21, round 10 ----

/// CR10-SENSITIVE: pregnancy status and sexual orientation wait in review by their LOINC code too, in
/// the code or a translation, when the observation declares only the generic Social History
/// template. Smoking status keeps its path.
#[test]
fn round_10_pregnancy_and_sexual_orientation_codes_wait_in_review_on_a_generic_template() -> R {
    let entry = |code: &str| {
        format!(
            r#"<entry><observation classCode="OBS" moodCode="EVN"><templateId root="2.16.840.1.113883.10.20.22.4.38"/>
                 {code}<statusCode code="completed"/><effectiveTime value="20210301"/>
                 <value xsi:type="ST">Synthetic answer</value></observation></entry>"#
        )
    };
    for loinc in ["82810-3", "76690-7"] {
        let plain = format!(r#"<code code="{loinc}" codeSystem="2.16.840.1.113883.6.1"/>"#);
        let translated = format!(
            r#"<code code="X1" codeSystem="1.2.3.4.5"><translation code="{loinc}" codeSystem="2.16.840.1.113883.6.1"/></code>"#
        );
        for code in [plain, translated] {
            let c = convert(&social_history(&entry(&code)))?;
            assert!(resources(&c, "Observation").is_empty(), "{code}");
            assert_eq!(
                c.sections[0].not_read,
                ["observation 2.16.840.1.113883.10.20.22.4.38"],
                "{code}"
            );
        }
    }
    let c = convert(&social_history(&entry(
        r#"<code code="72166-2" codeSystem="2.16.840.1.113883.6.1"/>"#,
    )))?;
    assert_eq!(resources(&c, "Observation").len(), 1);
    assert!(c.sections[0].not_read.is_empty());
    Ok(())
}
