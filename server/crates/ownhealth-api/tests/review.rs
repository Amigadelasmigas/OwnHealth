//! Lane I: `api/review.py` (tests/security/test_review_scoping.py and the review parts of
//! tests/test_app.py): accept never writes outside the item's profile, held payloads follow the
//! read scopes, and the request validation answers as FastAPI did.

mod lane_i;

use http::StatusCode;
use lane_i::{App, R, app, item, lab_document};
use ownhealth_core::documents::{NewDocument, create_document};
use ownhealth_core::review::{ItemQuery, ReviewItemIn, create_item};
use ownhealth_core::sources::{NewSource, create_source};
use ownhealth_ingest_structured::fhir::import_pull;
use ownhealth_mcp::review_shown::Scan;
use serde_json::{Value, json};
use uuid::Uuid;

async fn count(a: &App, table: &str, pid: Uuid) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "select count(*) from {table} where profile_id = $1"
    )))
    .bind(pid)
    .fetch_one(&a.rt.pool)
    .await?)
}

async fn status_of(a: &App, id: Uuid) -> anyhow::Result<String> {
    Ok(sqlx::query_scalar("select status from review_item where id = $1")
        .bind(id)
        .fetch_one(&a.rt.pool)
        .await?)
}

#[tokio::test]
async fn test_accept_edits_cannot_move_a_fact_to_another_profile() -> R {
    let a = app().await?;
    let f = &a.family;
    let sam = f.profile("sam"); // alex is only a viewer on sam
    let cand = item(&a, "alex", "candidate_row", None, json!({})).await?;
    let alex = f.profile("alex").to_string();
    let fhir = item(
        &a,
        "alex",
        "fhir_unmapped",
        Some(json!({
            "projection": "record",
            "record_kind": "condition",
            "record": {"profile_id": alex, "text": "Planted condition"},
            "src": {"kind": "fhir_import", "profile_id": alex, "import_ref": "bundle#1"},
        })),
        json!({}),
    )
    .await?;
    for id in [cand, fhir] {
        let (s, v) = a
            .post(
                "alex",
                &format!("/review/{id}/accept"),
                Some(json!({"edits": {"profile_id": sam}})),
            )
            .await?;
        assert_eq!(s, StatusCode::CONFLICT, "{v}");
        assert_eq!(v["detail"]["code"], "conflict");
    }
    assert_eq!(count(&a, "observation", sam).await?, 0);
    assert_eq!(count(&a, "condition", sam).await?, 0);
    let (s, v) = a
        .post(
            "alex",
            &format!("/review/{cand}/accept"),
            Some(json!({"edits": {"value_num": "98"}})),
        )
        .await?;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(count(&a, "observation", f.profile("alex")).await?, 1);
    Ok(())
}

#[tokio::test]
async fn test_document_identity_target_needs_import() -> R {
    let a = app().await?;
    let payload = json!({"suggested_profile_id": a.family.profile("sam").to_string()});
    let id = item(&a, "alex", "document_identity", Some(payload), json!({})).await?;
    let (s, v) = a.post("alex", &format!("/review/{id}/accept"), Some(json!({}))).await?;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(status_of(&a, id).await?, "open");
    Ok(())
}

#[tokio::test]
async fn test_caregiver_without_read_sensitive_cannot_see_a_sensitive_held_value() -> R {
    let a = app().await?;
    let hidden = item(
        &a,
        "carmen",
        "candidate_row",
        None,
        json!({"value_num": "1", "printed_value": "1", "value_canonical": "1", "printed_name": "VIH Ac",
               "sensitivity": ["sexual_health"]}),
    )
    .await?;
    let plain = item(&a, "carmen", "candidate_row", None, json!({})).await?;
    // alex is a caregiver on carmen: no read:sensitive
    let (s, listed) = a.get("alex", "/review?profile=carmen").await?;
    assert_eq!(s, StatusCode::OK, "{listed}");
    let ids: Vec<Value> = listed["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|i| i["id"].clone())
        .collect();
    assert!(ids.contains(&json!(plain)) && !ids.contains(&json!(hidden)), "{ids:?}");
    // nor how many items it hides from him
    assert!(listed.get("withheld").is_none(), "{listed}");
    // the count holds what the list shows: the hidden item is not one Alex can review
    assert_eq!(listed["open"], json!(1));
    let (s, v) = a.get("alex", &format!("/review/{hidden}")).await?;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(
        v,
        json!({"detail": {"code": "not_found", "message": "No such review item."}})
    );
    assert_eq!(
        a.post("alex", &format!("/review/{hidden}/accept"), Some(json!({})))
            .await?
            .0,
        StatusCode::NOT_FOUND
    );
    let (s, one) = a.get("alex", &format!("/review/{plain}")).await?;
    assert_eq!(s, StatusCode::OK);
    // FastAPI: the list (`-> dict`) went through pydantic (`Z`), the single item through
    // jsonable_encoder (`+00:00`)
    let listed_at = listed["items"][0]["created_at"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let one_at = one["created_at"].as_str().unwrap_or_default().to_string();
    assert!(
        listed_at.ends_with('Z') && one_at.ends_with("+00:00"),
        "{listed_at} {one_at}"
    );
    assert_eq!(listed_at.trim_end_matches('Z'), one_at.trim_end_matches("+00:00"));
    let keys: Vec<&String> = one.as_object().map(|m| m.keys().collect()).unwrap_or_default();
    assert_eq!(
        keys,
        [
            "id",
            "kind",
            "profile_id",
            "status",
            "reasons",
            "document_id",
            "candidate_row_id",
            "observation_id",
            "payload",
            "row_conf",
            "proposed_by_token",
            "context_src_ids",
            "decided_by",
            "decided_at",
            "decision_reason",
            "result_ids",
            "created_at"
        ]
    );
    Ok(())
}

#[tokio::test]
async fn test_viewer_cannot_review_and_others_see_nothing() -> R {
    let a = app().await?;
    let id = item(&a, "alex", "candidate_row", None, json!({})).await?;
    // sam is a viewer on alex: the item is not listed, and deciding it is refused
    let (_, listed) = a.get("sam", "/review").await?;
    assert!(!listed.to_string().contains(&id.to_string()));
    let (s, v) = a.get("sam", &format!("/review/{id}")).await?;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(v["detail"]["code"], "forbidden");
    assert_eq!(v["detail"]["message"], "The role viewer cannot review on this profile.");
    let (s, _) = a.get("alex", &format!("/review/{}", Uuid::new_v4())).await?;
    assert_eq!(s, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn test_reject_reopen_dismiss_and_audit() -> R {
    let a = app().await?;
    let id = item(&a, "alex", "candidate_row", None, json!({})).await?;
    let (s, v) = a.post("alex", &format!("/review/{id}/reject"), Some(json!({}))).await?;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        v,
        json!({"detail": [{"type": "missing", "loc": ["body", "reason"], "msg": "Field required", "input": {}}]})
    );
    let (s, v) = a.post("alex", &format!("/review/{id}/reject"), None).await?;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        v,
        json!({"detail": [{"type": "missing", "loc": ["body"], "msg": "Field required", "input": null}]})
    );
    let (s, v) = a
        .post("alex", &format!("/review/{id}/reject"), Some(json!({"reason": 5})))
        .await?;
    assert_eq!(
        (s, v["detail"][0]["type"].clone()),
        (StatusCode::UNPROCESSABLE_ENTITY, json!("string_type"))
    );
    let (s, v) = a
        .post(
            "alex",
            &format!("/review/{id}/reject"),
            Some(json!({"reason": "misread"})),
        )
        .await?;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v, json!({"id": id, "status": "rejected"}));
    let (s, v) = a.post("alex", &format!("/review/{id}/reopen"), None).await?;
    assert_eq!((s, v), (StatusCode::OK, json!({"id": id, "status": "open"})));
    // dismiss takes an optional body
    let (s, v) = a.post("alex", &format!("/review/{id}/dismiss"), None).await?;
    assert_eq!((s, v), (StatusCode::OK, json!({"id": id, "status": "dismissed"})));
    let (s, _) = a.post("alex", &format!("/review/{id}/dismiss"), None).await?;
    assert_eq!(s, StatusCode::CONFLICT);
    let actions: Vec<String> = sqlx::query_scalar(
        "select action from audit_event where resource_type = 'review_item' and resource_ids = array[$1] order by id",
    )
    .bind(id.to_string())
    .fetch_all(&a.rt.pool)
    .await?;
    assert_eq!(actions, ["review_reject", "review_reopen", "review_dismiss"]);
    Ok(())
}

#[tokio::test]
async fn test_validation_answers_like_fastapi() -> R {
    let a = app().await?;
    let (s, v) = a.get("alex", "/review/x").await?;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v["detail"][0]["type"], "uuid_parsing");
    assert_eq!(v["detail"][0]["loc"], json!(["path", "item_id"]));
    assert_eq!(v["detail"][0]["input"], "x");
    let (s, v) = a.get("alex", "/review?limit=501&document_id=nope").await?;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let types: Vec<Value> = v["detail"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|e| e["type"].clone())
        .collect();
    assert_eq!(types, [json!("less_than_equal"), json!("uuid_parsing")]);
    assert_eq!(v["detail"][0]["ctx"], json!({"le": 500}));
    // a negative limit or offset is a 422, not a database error (500)
    let (s, v) = a.get("alex", "/review?limit=-1&offset=-5").await?;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(v["detail"][0]["type"], "greater_than_equal");
    assert_eq!(v["detail"][1]["loc"], json!(["query", "offset"]));
    let (s, v) = a
        .post(
            "alex",
            "/review/00000000-0000-0000-0000-000000000000/accept",
            Some(json!({"edits": [1]})),
        )
        .await?;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        v,
        json!({"detail": [{"type": "dict_type", "loc": ["body", "edits"], "msg": "Input should be a valid dictionary",
                           "input": [1]}]})
    );
    let (s, v) = a
        .post("alex", "/review/bulk-accept", Some(json!({"document_id": "nope"})))
        .await?;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v["detail"][0]["loc"], json!(["body", "document_id"]));
    Ok(())
}

#[tokio::test]
async fn test_bulk_accept_takes_only_clean_rows_of_a_document() -> R {
    use ownhealth_core::acceptance::Check;
    use ownhealth_core::review::{self, CandidateIn};
    let a = app().await?;
    let mut tx = a.rt.pool.begin().await?;
    let (doc, boxes) = lab_document(&mut tx, &a, "alex").await?;
    let run = review::create_extraction_run(&mut tx, doc, "rules:test", "1", b"x", None, None).await?;
    let mut queued = Vec::new();
    for (value, checks) in [
        ("97", vec![Check::pass("loinc")]),
        ("98", vec![Check::fail("loinc", "loinc_unknown")]),
    ] {
        let r = review::submit_candidate(
            &mut tx,
            run,
            &CandidateIn {
                page_no: 1,
                bbox: boxes[0],
                cells: json!({}),
                row_conf: 0.5,
                observation: lane_i::obs(a.family.profile("alex"), value, json!({})),
                checks,
            },
            "rules_text",
            "matched",
            0.95,
            false,
        )
        .await?;
        queued.push(r.review_item_id);
    }
    tx.commit().await?;
    let (s, v) = a
        .post("sam", "/review/bulk-accept", Some(json!({"document_id": doc})))
        .await?;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    let (s, v) = a
        .post(
            "alex",
            "/review/bulk-accept",
            Some(json!({"document_id": doc, "min_row_conf": 0.9})),
        )
        .await?;
    assert_eq!((s, v), (StatusCode::OK, json!({"accepted": []})));
    let (s, v) = a
        .post("alex", "/review/bulk-accept", Some(json!({"document_id": doc})))
        .await?;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v, json!({"accepted": [queued[0]]}));
    let (s, v) = a
        .post(
            "alex",
            "/review/bulk-accept",
            Some(json!({"document_id": Uuid::new_v4()})),
        )
        .await?;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(
        v,
        json!({"detail": {"code": "not_found", "message": "No such document."}})
    );
    let rows: i32 = sqlx::query_scalar(
        "select rows_out from audit_event where action = 'review_bulk_accept' order by id desc limit 1",
    )
    .fetch_one(&a.rt.pool)
    .await?;
    assert_eq!(rows, 1);
    Ok(())
}

/// Carmen's connection to her hospital.
async fn carmen_source(a: &App) -> anyhow::Result<Uuid> {
    let fam = &a.family;
    let mut c = a.conn().await?;
    Ok(create_source(
        &mut c,
        &NewSource {
            profile_id: Some(fam.profile("carmen")),
            ..NewSource::new(fam.household_id, "smart", "Hospital LabNorte")
        },
    )
    .await?)
}

/// What a connector poller does for Carmen: the answer stored as her document, then the import.
async fn carmen_pull(a: &App, resources: Vec<Value>) -> anyhow::Result<Uuid> {
    let source = carmen_source(a).await?;
    carmen_pull_from(a, source, "pull-1", resources).await
}

/// One pull of `source` whose Bundle.id is `page`.
async fn carmen_pull_from(a: &App, source: Uuid, page: &str, resources: Vec<Value>) -> anyhow::Result<Uuid> {
    let fam = &a.family;
    let carmen = fam.profile("carmen");
    let mut c = a.conn().await?;
    let entries: Vec<Value> = resources
        .into_iter()
        .map(|r| {
            let url = format!(
                "https://fhir.labnorte.example/r4/{}/{}",
                r["resourceType"].as_str().unwrap_or_default(),
                r["id"].as_str().unwrap_or_default()
            );
            json!({"fullUrl": url, "resource": r})
        })
        .collect();
    let bytes =
        serde_json::to_vec(&json!({"resourceType": "Bundle", "id": page, "type": "searchset", "entry": entries}))?;
    let sha =
        a.rt.blobs
            .put_bytes(ownhealth_core::blobs::BlobOwner::Profile(carmen), &bytes)?;
    let doc = create_document(
        &mut c,
        &NewDocument {
            filename: Some("Hospital LabNorte · pull.json"),
            channel: "smart",
            source_id: Some(source),
            ..NewDocument::upload(
                fam.household_id,
                Some(carmen),
                &sha,
                "application/fhir+json",
                bytes.len() as i64,
            )
        },
    )
    .await?;
    import_pull(&mut c, &a.rt.blobs, source, doc.id).await?;
    Ok(doc.id)
}

/// Alex's categories on Carmen's profile (alex is her caregiver).
async fn alex_on_carmen(a: &App, categories: &[&str]) -> R {
    sqlx::query("update profile_grant set categories = $3 where user_id = $1 and profile_id = $2")
        .bind(a.family.user("alex"))
        .bind(a.family.profile("carmen"))
        .bind(categories.iter().map(|c| c.to_string()).collect::<Vec<_>>())
        .execute(&a.rt.pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn test_a_pull_summary_shows_only_to_who_can_open_its_bundle() -> R {
    let a = app().await?;
    // a claim: OwnHealth projects no claims, so nothing read its diagnosis for sensitivity
    let claim = json!({"resourceType": "ExplanationOfBenefit", "id": "eob-1", "status": "active", "use": "claim",
        "patient": {"reference": "Patient/carmen"},
        "diagnosis": [{"sequence": 1, "diagnosisCodeableConcept": {"text": "VIH, control"}}]});
    let doc = carmen_pull(&a, vec![claim]).await?;
    let summary: Uuid =
        sqlx::query_scalar("select id from review_item where document_id = $1 and kind = 'fhir_unmapped_summary'")
            .bind(doc)
            .fetch_one(&a.rt.pool)
            .await?;
    let file = format!("/documents/{doc}/file");
    let listed = |v: &Value| {
        v["items"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|i| i["id"] == json!(summary))
    };
    // a caregiver with read:records and no read:sensitive: the bundle and its item stay closed
    let (s, v) = a.get("alex", &file).await?;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(v["detail"]["code"], "insufficient_scope");
    let (_, queue) = a.get("alex", "/review?profile=carmen").await?;
    assert!(!listed(&queue), "{queue}");
    assert!(queue.get("withheld").is_none(), "{queue}");
    assert_eq!(
        a.get("alex", &format!("/review/{summary}")).await?.0,
        StatusCode::NOT_FOUND
    );
    // read:sensitive without read:records (the scope of the bundle): still closed
    alex_on_carmen(&a, &["read:summary", "read:labs", "read:sensitive"]).await?;
    assert_eq!(a.get("alex", &file).await?.0, StatusCode::FORBIDDEN);
    assert!(!listed(&a.get("alex", "/review?profile=carmen").await?.1));
    // both: the bundle opens, and the item shows and can be dismissed
    alex_on_carmen(&a, &["read:summary", "read:records", "read:sensitive"]).await?;
    let (s, v) = a.get("alex", &file).await?;
    assert_eq!((s, &v["id"]), (StatusCode::OK, &json!("pull-1")), "{v}");
    assert!(listed(&a.get("alex", "/review?profile=carmen").await?.1));
    let (s, v) = a.post("alex", &format!("/review/{summary}/dismiss"), None).await?;
    assert_eq!(s, StatusCode::OK, "{v}");
    Ok(())
}

#[tokio::test]
async fn test_a_daily_pull_of_a_sensitive_fact_stays_closed_without_read_sensitive() -> R {
    let a = app().await?;
    let hiv = json!({"resourceType": "Condition", "id": "hiv",
        "meta": {"versionId": "1", "security": [{"system": "http://terminology.hl7.org/CodeSystem/v3-ActCode", "code": "HIV"}]},
        "clinicalStatus": {"coding": [{"system": "http://terminology.hl7.org/CodeSystem/condition-clinical", "code": "active"}]},
        "verificationStatus": {"coding": [{"system": "http://terminology.hl7.org/CodeSystem/condition-ver-status", "code": "confirmed"}]},
        "code": {"coding": [{"system": "http://snomed.info/sct", "code": "86406008", "display": "Human immunodeficiency virus infection"}]},
        "subject": {"reference": "Patient/carmen"}, "onsetDateTime": "2019-04"});
    let patient = json!({"resourceType": "Patient", "id": "carmen", "name": [{"family": "Ortega", "given": ["Carmen"]}],
        "birthDate": "1955-01-30", "gender": "female"});
    let source = carmen_source(&a).await?;
    let day1 = carmen_pull_from(&a, source, "day-1", vec![patient.clone(), hiv.clone()]).await?;
    let day2 = carmen_pull_from(&a, source, "day-2", vec![patient, hiv]).await?;
    // alex, her caregiver, has no read:sensitive: neither day's bundle opens
    for doc in [day1, day2] {
        let (s, v) = a.get("alex", &format!("/documents/{doc}/file")).await?;
        assert_eq!(s, StatusCode::FORBIDDEN, "{doc}: {v}");
    }
    alex_on_carmen(&a, &["read:summary", "read:records", "read:sensitive"]).await?;
    assert_eq!(
        a.get("alex", &format!("/documents/{day2}/file")).await?.0,
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn test_the_newest_fhir_import_is_named_however_many_dated_documents_come_first() -> R {
    let a = app().await?;
    let fam = &a.family;
    let carmen = fam.profile("carmen");
    let source = carmen_source(&a).await?;
    let first = carmen_pull_from(&a, source, "day-1", vec![]).await?;
    let second = carmen_pull_from(&a, source, "day-2", vec![]).await?;
    // a dated report, stored after the pulls: the list puts it first, and pulls carry no date
    let mut c = a.conn().await?;
    let sha =
        a.rt.blobs
            .put_bytes(ownhealth_core::blobs::BlobOwner::Profile(carmen), b"%PDF-1.7 report")?;
    let report = create_document(
        &mut c,
        &NewDocument::upload(fam.household_id, Some(carmen), &sha, "application/pdf", 15),
    )
    .await?;
    sqlx::query("update document set collected_at = now() where id = $1")
        .bind(report.id)
        .execute(&mut *c)
        .await?;
    drop(c);
    let ids = |v: &Value| -> Vec<Value> {
        v["items"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|i| i["id"].clone())
            .collect()
    };
    let (s, v) = a.get("alex", "/profiles/carmen/documents?limit=1").await?;
    assert_eq!((s, ids(&v)), (StatusCode::OK, vec![json!(report.id)]), "{v}");
    // latest=fhir: FHIR imports only, the newest stored first
    let (s, v) = a.get("alex", "/profiles/carmen/documents?latest=fhir&limit=1").await?;
    assert_eq!((s, ids(&v)), (StatusCode::OK, vec![json!(second)]), "{v}");
    let (_, v) = a.get("alex", "/profiles/carmen/documents?latest=fhir").await?;
    assert_eq!(ids(&v), [json!(second), json!(first)], "{v}");
    let (s, v) = a.get("alex", "/profiles/carmen/documents?latest=pdf").await?;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    Ok(())
}

/// A sync-gap alert names the vendor (a Dexcom alert says the person wears a CGM) and is closed for the
/// whole household by a dismissal. Only who manages the profile, and so its connections, sees it.
#[tokio::test]
async fn test_a_sync_gap_alert_shows_only_to_who_manages_the_connection() -> R {
    let a = app().await?;
    let message = json!({"message": {"en": "Sync has stopped for Dexcom.", "es": "Se ha interrumpido la sincronización de Dexcom."}});
    // alex is a caregiver on carmen: reads and reviews, never manages
    let carmen = item(&a, "carmen", "source_gap", Some(message.clone()), json!({})).await?;
    // alex guards lucía, who has no login: alex manages her connections
    let lucia = item(&a, "lucia", "source_gap", Some(message), json!({})).await?;
    let listed = |v: &Value, id: Uuid| {
        v["items"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|i| i["id"] == json!(id))
    };
    let (s, v) = a.get("alex", "/review?profile=carmen").await?;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(!listed(&v, carmen), "{v}");
    // the list gives no count of hidden items, so it does not reveal that the alert exists
    let (s, v) = a.get("alex", "/review?profile=carmen&kind=source_gap").await?;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["items"], json!([]), "{v}");
    assert!(v.get("withheld").is_none(), "{v}");
    // nor does it take a page slot from an item Alex reviews
    let row = item(&a, "carmen", "candidate_row", None, json!({})).await?;
    let (s, v) = a.get("alex", "/review?profile=carmen&limit=1").await?;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(listed(&v, row), "{v}");
    assert!(v.get("withheld").is_none(), "{v}");
    assert_eq!(
        a.get("alex", &format!("/review/{carmen}")).await?.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        a.post("alex", &format!("/review/{carmen}/dismiss"), None).await?.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(status_of(&a, carmen).await?, "open");
    let (s, v) = a.get("alex", "/review?profile=lucia").await?;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(listed(&v, lucia), "{v}");
    let (s, v) = a.post("alex", &format!("/review/{lucia}/dismiss"), None).await?;
    assert_eq!(s, StatusCode::OK, "{v}");
    Ok(())
}

/// The dashboard's `review_open` counts only what the review list shows the reader. Alex is Carmen's
/// caregiver without read:records: a sync-gap alert (only who manages the profile sees it), a held
/// imaging study and a proposed relative (both read:records) are not counted, a lab row is. Alex's
/// own profile still counts all three, since Alex manages it with every scope; Sam, who only views
/// it, is shown none. A household item counts as the list for the profile shows it.
#[tokio::test]
async fn test_the_dashboard_counts_only_the_review_items_the_reader_sees() -> R {
    let a = app().await?;
    alex_on_carmen(&a, &["read:summary", "read:labs"]).await?;
    let read = |reader: &'static str, who: &'static str| {
        let a = &a;
        async move {
            let (s, v) = a
                .get(reader, &format!("/profiles/{who}/dashboard?as_of=2026-11-07&lang=en"))
                .await?;
            assert_eq!(s, StatusCode::OK, "{v}");
            anyhow::Ok(v["review_open"].clone())
        }
    };
    let dashboard = |who: &'static str| read("alex", who);
    let message = json!({"message": {"en": "Sync has stopped for Dexcom.", "es": "Se ha interrumpido la sincronización de Dexcom."}});
    let study = json!({"patient_name": "ORTEGA^CARMEN", "study_description": "CT HEAD"});
    let relative = json!({"name": "Rosa Ortega", "relationship": "mother"});
    for (kind, payload) in [
        ("source_gap", message),
        (ownhealth_imaging::import::REVIEW_KIND, study),
        (ownhealth_core::gedcom::KIND, relative),
    ] {
        item(&a, "carmen", kind, Some(payload.clone()), json!({})).await?;
        assert_eq!(
            dashboard("carmen").await?,
            json!(0),
            "{kind} counted for a reader who cannot see it"
        );
        item(&a, "alex", kind, Some(payload), json!({})).await?;
    }
    assert_eq!(dashboard("alex").await?, json!(3), "the owner sees every kind");
    // sam only views Alex's profile and reviews nothing there, not even an item her scopes would show
    let note = json!({"kind": "note", "text": "Ask about the LDL trend"});
    item(&a, "alex", "agent_proposal", Some(note), json!({})).await?;
    assert_eq!(read("sam", "alex").await?, json!(0));
    assert_eq!(dashboard("alex").await?, json!(4));
    // a household item counts where the review list for the profile shows it: for who reviews there
    household_item(&a).await?;
    assert_eq!(read("sam", "alex").await?, json!(0));
    let (s, listed) = a.get("alex", "/review?profile=alex").await?;
    assert_eq!(s, StatusCode::OK, "{listed}");
    assert_eq!(listed["open"], json!(5));
    assert_eq!(
        dashboard("alex").await?,
        listed["open"],
        "the dashboard and the list disagree"
    );
    // positive control: a lab row Alex may read on Carmen counts
    item(&a, "carmen", "candidate_row", None, json!({})).await?;
    assert_eq!(dashboard("carmen").await?, json!(2));
    Ok(())
}

/// An open item of the household itself, as a failed job with no profile files it.
async fn household_item(a: &App) -> anyhow::Result<Uuid> {
    let mut c = a.conn().await?;
    Ok(create_item(
        &mut c,
        &ReviewItemIn {
            kind: "job_failed".into(),
            household_id: Some(a.family.household_id),
            profile_id: None,
            payload: json!({"kind": "extract", "error": "The file did not read."}),
            reasons: vec!["x".into()],
            ..Default::default()
        },
    )
    .await?)
}

/// A household item shows only to who can review on some profile. Sam, left with her viewer grant on
/// Alex's profile alone, is not shown it, cannot open or dismiss it, and does not count it.
#[tokio::test]
async fn test_a_household_item_shows_only_to_who_reviews_somewhere() -> R {
    let a = app().await?;
    let id = household_item(&a).await?;
    sqlx::query("delete from profile_grant where user_id = $1 and profile_id <> $2")
        .bind(a.family.user("sam"))
        .bind(a.family.profile("alex"))
        .execute(&a.rt.pool)
        .await?;
    let (s, listed) = a.get("sam", "/review").await?;
    assert_eq!(s, StatusCode::OK, "{listed}");
    assert!(!listed.to_string().contains(&id.to_string()), "{listed}");
    assert_eq!(listed["open"], json!(0));
    let (s, me) = a.get("sam", "/me").await?;
    assert_eq!(s, StatusCode::OK, "{me}");
    assert_eq!(me["review_open"], json!(0));
    assert_eq!(a.get("sam", &format!("/review/{id}")).await?.0, StatusCode::NOT_FOUND);
    let (s, v) = a.post("sam", &format!("/review/{id}/dismiss"), None).await?;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
    assert_eq!(status_of(&a, id).await?, "open");
    // positive control: Alex reviews on his profiles and is shown it
    let (_, listed) = a.get("alex", "/review").await?;
    assert!(listed.to_string().contains(&id.to_string()), "{listed}");
    assert_eq!(a.get("alex", &format!("/review/{id}")).await?.0, StatusCode::OK);
    Ok(())
}

/// A C-CDA document of Carmen's whose import held a sensitive value and a plain one, with the flags
/// the import writes: the section titles, how many resources were sensitive or not projected, and how
/// many items it queued for review.
async fn carmen_ccda_with_held_values(a: &App) -> anyhow::Result<Uuid> {
    let doc: Uuid = sqlx::query_scalar(
        "insert into document (household_id, profile_id, blob_sha256, filename, mime, size_bytes, flags) \
         values ($1, $2, decode($3, 'hex'), 'resumen.xml', 'application/hl7-cda+xml', 3, $4) returning id",
    )
    .bind(a.family.household_id)
    .bind(a.family.profile("carmen"))
    .bind(ownhealth_core::ids::token_hex(32))
    .bind(json!({
        "ccda": {"title": "Resumen", "sections": [{"title": "Salud sexual", "kind": "social_history", "entries": 1}]},
        "sensitive": 2,
        "unprojected": 1,
        "import": {"queued": 2, "reasons": {"unmapped": 2}},
    }))
    .fetch_one(&a.rt.pool)
    .await?;
    let hidden = item(
        a,
        "carmen",
        "candidate_row",
        None,
        json!({"value_num": "1", "printed_value": "1", "value_canonical": "1", "printed_name": "VIH Ac",
               "sensitivity": ["sexual_health"]}),
    )
    .await?;
    let plain = item(a, "carmen", "candidate_row", None, json!({})).await?;
    sqlx::query("update review_item set document_id = $1 where id = any($2)")
        .bind(doc)
        .bind(vec![hidden, plain])
        .execute(&a.rt.pool)
        .await?;
    Ok(doc)
}

/// Alex's view of Carmen's document `doc`: its detail, its row in the document list, and how many of its
/// items the review list shows him.
async fn read_carmen_document(a: &App, doc: Uuid) -> anyhow::Result<(Value, Value, usize)> {
    let (s, one) = a.get("alex", &format!("/documents/{doc}")).await?;
    assert_eq!(s, StatusCode::OK, "{one}");
    let (s, list) = a.get("alex", "/profiles/carmen/documents").await?;
    assert_eq!(s, StatusCode::OK, "{list}");
    let listed = list["items"]
        .as_array()
        .and_then(|items| items.iter().find(|i| i["id"] == json!(doc)))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("the list leaves the document out: {list}"))?;
    let (s, queue) = a
        .get("alex", &format!("/review?profile=carmen&document_id={doc}"))
        .await?;
    assert_eq!(s, StatusCode::OK, "{queue}");
    Ok((one, listed, queue["items"].as_array().map_or(0, Vec::len)))
}

/// U-L4: a document's C-CDA section titles and its sensitive and unprojected counts reach only a reader
/// with read:sensitive, and its pending-review count holds only the items the review list shows the
/// reader, in the document detail and in the document list alike. Alex is Carmen's caregiver, first
/// without read:sensitive, then with it.
#[tokio::test]
async fn test_a_documents_review_flags_and_count_follow_what_the_reader_sees() -> R {
    let a = app().await?;
    let doc = carmen_ccda_with_held_values(&a).await?;
    let flags = |v: &Value| -> Vec<String> {
        let mut keys: Vec<String> = v["flags"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, _)| k.clone())
            .collect();
        keys.sort();
        keys
    };
    let (one, listed, shown) = read_carmen_document(&a, doc).await?;
    assert_eq!(shown, 1, "the review list hides the sensitive item from a caregiver");
    for v in [&one, &listed] {
        assert!(flags(v).is_empty(), "flags without read:sensitive: {v}");
        assert_eq!(
            v["open_review"],
            json!(shown),
            "the count holds an item the list hides: {v}"
        );
    }
    // a SMART Health Card's rows are the file's content: from a sensitive card they show only with
    // read:sensitive, as the file does; a plain card shows them to every reader of the document. An
    // Apple Health export's record types it did not import (sexual activity here) need read:sensitive.
    let shc = |text: &str| {
        json!({"shc": [{"iss": "https://issuer.labnorte.example", "host": "issuer.labnorte.example",
            "kid": "k", "issued": "2026-10-22T12:00:00Z", "expires": null, "jws": "ab",
            "entries": [{"type": "Observation", "date": "2026-03-02", "text": text}]}]})
    };
    let sensitive_card = carmen_document(&a, "tarjeta-vih.json", shc("VIH 1+2 Ac: Positivo")).await?;
    let held = item(
        &a,
        "carmen",
        "candidate_row",
        None,
        json!({"printed_name": "VIH Ac", "sensitivity": ["sexual_health"]}),
    )
    .await?;
    sqlx::query("update review_item set document_id = $1 where id = $2")
        .bind(sensitive_card)
        .bind(held)
        .execute(&a.rt.pool)
        .await?;
    let plain_card = carmen_document(&a, "tarjeta-gripe.json", shc("Gripe 2026")).await?;
    let apple = carmen_document(
        &a,
        "export.zip",
        json!({"apple_health": {"samples": 3, "not_imported": {"skipped:HKCategoryTypeIdentifierSexualActivity": 2}}}),
    )
    .await?;
    let (card, card_listed) = carmen_flags(&a, sensitive_card).await?;
    for f in [&card, &card_listed] {
        assert!(f.get("shc").is_none(), "card rows of a sensitive card: {f}");
        // F-SHCPHOTO: the card is still a card, so the page shows that its rows are withheld and never
        // renders (or requests) its photo as a plain scan
        assert_eq!(f["shc_withheld"], json!(true), "{f}");
    }
    let (card, card_listed) = carmen_flags(&a, plain_card).await?;
    for f in [&card, &card_listed] {
        assert_eq!(f["shc"][0]["entries"][0]["text"], "Gripe 2026", "{f}");
        assert!(f.get("shc_withheld").is_none(), "{f}");
    }
    let (export, export_listed) = carmen_flags(&a, apple).await?;
    for f in [&export, &export_listed] {
        assert_eq!(f["apple_health"], json!({"samples": 3}), "{f}");
    }

    alex_on_carmen(&a, &["read:summary", "read:labs", "read:records", "read:sensitive"]).await?;
    let (one, listed, shown) = read_carmen_document(&a, doc).await?;
    assert_eq!(shown, 2);
    for v in [&one, &listed] {
        assert_eq!(flags(v), ["ccda", "import", "sensitive", "unprojected"], "{v}");
        assert_eq!(v["open_review"], json!(2), "{v}");
    }
    let (card, card_listed) = carmen_flags(&a, sensitive_card).await?;
    for f in [&card, &card_listed] {
        assert_eq!(f["shc"][0]["entries"][0]["text"], "VIH 1+2 Ac: Positivo", "{f}");
        assert!(f.get("shc_withheld").is_none(), "{f}");
    }
    let (export, export_listed) = carmen_flags(&a, apple).await?;
    for f in [&export, &export_listed] {
        assert_eq!(
            f["apple_health"]["not_imported"]["skipped:HKCategoryTypeIdentifierSexualActivity"], 2,
            "{f}"
        );
    }
    // read:sensitive goes with the document's own scope: a C-CDA document is read with read:records,
    // so read:labs plays no part
    alex_on_carmen(&a, &["read:summary", "read:records", "read:sensitive"]).await?;
    let (one, listed) = carmen_flags(&a, doc).await?;
    for f in [&one, &listed] {
        assert!(f.get("ccda").is_some() && f.get("import").is_some(), "{f}");
    }
    Ok(())
}

/// A document of Carmen's (no type: read with read:records) with these flags.
async fn carmen_document(a: &App, filename: &str, flags: Value) -> anyhow::Result<Uuid> {
    Ok(sqlx::query_scalar(
        "insert into document (household_id, profile_id, blob_sha256, filename, mime, size_bytes, flags) \
         values ($1, $2, decode($3, 'hex'), $4, 'application/octet-stream', 3, $5) returning id",
    )
    .bind(a.family.household_id)
    .bind(a.family.profile("carmen"))
    .bind(ownhealth_core::ids::token_hex(32))
    .bind(filename)
    .bind(flags)
    .fetch_one(&a.rt.pool)
    .await?)
}

/// The flags Alex is shown of Carmen's document `doc`: in its detail, and in its row of the list.
async fn carmen_flags(a: &App, doc: Uuid) -> anyhow::Result<(Value, Value)> {
    let (s, one) = a.get("alex", &format!("/documents/{doc}")).await?;
    assert_eq!(s, StatusCode::OK, "{one}");
    let (s, list) = a.get("alex", "/profiles/carmen/documents").await?;
    assert_eq!(s, StatusCode::OK, "{list}");
    let listed = list["items"]
        .as_array()
        .and_then(|items| items.iter().find(|i| i["id"] == json!(doc)))
        .map(|i| i["flags"].clone())
        .ok_or_else(|| anyhow::anyhow!("the list leaves the document out: {list}"))?;
    Ok((one["flags"].clone(), listed))
}

/// An open item of the household itself (no profile) on document `doc`.
async fn household_item_on(a: &App, kind: &str, doc: Uuid, payload: Value) -> anyhow::Result<Uuid> {
    let mut c = a.conn().await?;
    Ok(create_item(
        &mut c,
        &ReviewItemIn {
            kind: kind.into(),
            household_id: Some(a.family.household_id),
            profile_id: None,
            document_id: Some(doc),
            payload,
            ..ReviewItemIn::default()
        },
    )
    .await?)
}

/// `open_review` of each document holds what the review list shows the reader for that document: its
/// own items the reader may see, plus the household's own items on it (a failed job, not a sync alert,
/// which the list takes only on a profile it names). A pull's grouped item on a sensitive bundle stays
/// out for a reader the bundle would not open for. Two documents with different counts keep their
/// own. A member who cannot review on the profile is shown 0.
#[tokio::test]
async fn test_open_review_counts_per_document_what_the_list_shows() -> R {
    let a = app().await?;
    let first = carmen_document(&a, "informe.xml", json!({})).await?;
    let bundle = carmen_document(&a, "bundle.json", json!({"sensitive": 1})).await?;
    for doc in [first, first, bundle] {
        let id = item(&a, "carmen", "candidate_row", None, json!({})).await?;
        sqlx::query("update review_item set document_id = $1 where id = $2")
            .bind(doc)
            .bind(id)
            .execute(&a.rt.pool)
            .await?;
    }
    household_item_on(
        &a,
        "job_failed",
        first,
        json!({"kind": "import.pdf", "error": "unreadable"}),
    )
    .await?;
    household_item_on(&a, "source_gap", first, json!({})).await?;
    let grouped = item(
        &a,
        "carmen",
        "fhir_unmapped_summary",
        Some(json!({"resource_type": "ExplanationOfBenefit", "count": 1})),
        json!({}),
    )
    .await?;
    sqlx::query("update review_item set document_id = $1 where id = $2")
        .bind(bundle)
        .bind(grouped)
        .execute(&a.rt.pool)
        .await?;
    let counts = |list: &Value| -> Vec<(Value, Value)> {
        let mut out: Vec<(Value, Value)> = list["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["id"] == json!(first) || d["id"] == json!(bundle))
            .map(|d| (d["filename"]["text"].clone(), d["open_review"].clone()))
            .collect();
        out.sort_by_key(|(name, _)| name.to_string());
        out
    };
    // Alex is Carmen's caregiver: he reviews, without read:sensitive
    let (s, list) = a.get("alex", "/profiles/carmen/documents").await?;
    assert_eq!(s, StatusCode::OK, "{list}");
    assert_eq!(
        counts(&list),
        [(json!("bundle.json"), json!(1)), (json!("informe.xml"), json!(3))],
        "{list}"
    );
    for (doc, n) in [(first, 3), (bundle, 1)] {
        let (s, queue) = a
            .get("alex", &format!("/review?profile=carmen&document_id={doc}"))
            .await?;
        assert_eq!(s, StatusCode::OK, "{queue}");
        assert_eq!(queue["items"].as_array().map(Vec::len), Some(n), "{queue}");
        let (s, one) = a.get("alex", &format!("/documents/{doc}")).await?;
        assert_eq!(s, StatusCode::OK, "{one}");
        assert_eq!(one["open_review"], json!(n), "{one}");
    }
    // as a viewer he reads the same documents and reviews none of them
    sqlx::query("update profile_grant set role = 'viewer' where user_id = $1 and profile_id = $2")
        .bind(a.family.user("alex"))
        .bind(a.family.profile("carmen"))
        .execute(&a.rt.pool)
        .await?;
    let (s, list) = a.get("alex", "/profiles/carmen/documents").await?;
    assert_eq!(s, StatusCode::OK, "{list}");
    assert_eq!(
        counts(&list),
        [(json!("bundle.json"), json!(0)), (json!("informe.xml"), json!(0))],
        "{list}"
    );
    Ok(())
}

/// U-F1: a FHIR resource a file import held unmapped (`fhir_unmapped` with the whole `resource`) shows
/// on the terms of the fact it would be: an Observation with the scope of its category, any other type
/// with read:records, and a sensitive one (a security label) with read:sensitive too. The same rule
/// holds for the document's count.
#[tokio::test]
async fn test_a_held_fhir_resource_shows_only_with_its_scope_and_read_sensitive() -> R {
    let a = app().await?;
    let doc = carmen_document(&a, "vacunas.json", json!({})).await?;
    let held = |resource: Value| {
        json!({"projection": null, "note": "not-done", "resource_type": resource["resourceType"],
               "json_pointer": "/entry/0/resource", "src": {"import_ref": "vacunas#1"}, "resource": resource})
    };
    let mut ids = Vec::new();
    for resource in [
        json!({"resourceType": "Immunization", "status": "not-done",
               "vaccineCode": {"text": "HIV vaccine trial dose (synthetic)"},
               "meta": {"security": [{"code": "R"}, {"code": "HIV"}]}}),
        json!({"resourceType": "Immunization", "status": "not-done", "vaccineCode": {"text": "Influenza"}}),
        json!({"resourceType": "Observation", "status": "cancelled",
               "category": [{"coding": [{"code": "laboratory"}]}], "code": {"text": "Glucosa"}}),
        json!({"resourceType": "Observation", "status": "cancelled",
               "category": [{"coding": [{"code": "vital-signs"}]}], "code": {"text": "Tension arterial"}}),
    ] {
        let id = item(&a, "carmen", "fhir_unmapped", Some(held(resource)), json!({})).await?;
        sqlx::query("update review_item set document_id = $1 where id = $2")
            .bind(doc)
            .bind(id)
            .execute(&a.rt.pool)
            .await?;
        ids.push(id);
    }
    let (hiv, flu, glucose, pressure) = (ids[0], ids[1], ids[2], ids[3]);
    let shown = |grant: &'static [&'static str]| {
        let a = &a;
        async move {
            alex_on_carmen(a, grant).await?;
            let (s, queue) = a.get("alex", "/review?profile=carmen").await?;
            assert_eq!(s, StatusCode::OK, "{queue}");
            let mut listed: Vec<Uuid> = Vec::new();
            for id in [hiv, flu, glucose, pressure] {
                let in_list = queue["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|i| i["id"] == json!(id));
                let one = a.get("alex", &format!("/review/{id}")).await?.0;
                assert_eq!(
                    in_list,
                    one == StatusCode::OK,
                    "{grant:?}: list and item disagree on {id}"
                );
                if in_list {
                    listed.push(id);
                }
            }
            if grant.contains(&"read:records") {
                let (s, one) = a.get("alex", &format!("/documents/{doc}")).await?;
                assert_eq!(s, StatusCode::OK, "{one}");
                assert_eq!(one["open_review"], json!(listed.len()), "{grant:?}: {one}");
            }
            anyhow::Ok(listed)
        }
    };
    assert_eq!(shown(&["read:summary", "read:labs"]).await?, [glucose], "labs only");
    assert_eq!(
        shown(&["read:summary", "read:records"]).await?,
        [flu],
        "records, no read:sensitive"
    );
    assert_eq!(
        shown(&["read:summary", "read:labs", "read:records", "read:sensitive"]).await?,
        [hiv, flu, glucose],
        "every scope but read:vitals"
    );
    assert_eq!(
        shown(&["read:summary", "read:vitals"]).await?,
        [pressure],
        "a vital-signs Observation needs read:vitals, not read:labs"
    );
    Ok(())
}

/// CR-REVIEWPAGE: a page holds only items the reader may see. With 450 older held activity values a
/// labs-only reviewer may not see and one newer lab value they may, the first page shows the lab value
/// and `open` counts it. 450 is more than two read batches (review_shown::SCAN_BATCH, 200), so the lab
/// value is found only if the list reads on past every batch that held nothing visible. `offset` skips
/// visible items only, and `limit` counts visible items.
#[tokio::test]
async fn test_a_review_page_holds_the_visible_items_past_any_number_of_hidden_ones() -> R {
    let a = app().await?;
    alex_on_carmen(&a, &["read:summary", "read:labs"]).await?;
    let hidden = item(&a, "carmen", "candidate_row", None, json!({"category": "activity"})).await?;
    sqlx::query(
        "insert into review_item (household_id, profile_id, kind, payload, reasons, created_at) \
         select household_id, profile_id, kind, payload, reasons, created_at - make_interval(secs => g) \
         from review_item, generate_series(1, 449) g where id = $1",
    )
    .bind(hidden)
    .execute(&a.rt.pool)
    .await?;
    let lab = item(&a, "carmen", "candidate_row", None, json!({})).await?;
    let page = |query: &'static str| {
        let a = &a;
        async move {
            let (s, v) = a.get("alex", &format!("/review?profile=carmen{query}")).await?;
            assert_eq!(s, StatusCode::OK, "{v}");
            let ids: Vec<Value> = v["items"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| i["id"].clone())
                .collect();
            anyhow::Ok((ids, v["open"].clone()))
        }
    };
    assert_eq!(page("").await?, (vec![json!(lab)], json!(1)));
    assert_eq!(page("&limit=1").await?, (vec![json!(lab)], json!(1)));
    assert_eq!(page("&offset=1").await?, (vec![], json!(1)));
    // positive control: with read:wearables he sees all 451, oldest first, a page at a time
    sqlx::query("update profile_grant set categories = $3 where user_id = $1 and profile_id = $2")
        .bind(a.family.user("alex"))
        .bind(a.family.profile("carmen"))
        .bind(vec![
            "read:summary".to_string(),
            "read:labs".to_string(),
            "read:wearables".to_string(),
        ])
        .execute(&a.rt.pool)
        .await?;
    let (ids, open) = page("&limit=100").await?;
    assert_eq!((ids.len(), open), (100, json!(451)));
    assert_ne!(ids[0], json!(lab));
    let (ids, open) = page("&offset=400").await?;
    assert_eq!((ids.len(), ids.last(), open), (51, Some(&json!(lab)), json!(451)));
    assert_eq!(page("&offset=450").await?, (vec![json!(lab)], json!(451)));
    Ok(())
}

/// CR-FHIRALIAS: a held FHIR Observation shows on the terms of the category the importer would give
/// it. The importer reads `social-history` as `survey` and `imaging` or `procedure` as `exam`, which
/// need read:vitals; an Observation with no category is a lab value.
#[tokio::test]
async fn test_a_held_fhir_observation_takes_the_scope_of_its_imported_category() -> R {
    let a = app().await?;
    let doc = carmen_document(&a, "observaciones.json", json!({})).await?;
    let held = |code: Option<&str>| {
        let mut resource = json!({"resourceType": "Observation", "status": "cancelled", "code": {"text": "Peso"}});
        if let Some(code) = code {
            resource["category"] = json!([{"coding": [{"code": code}]}]);
        }
        json!({"projection": null, "note": "not-done", "resource_type": "Observation",
               "json_pointer": "/entry/0/resource", "src": {"import_ref": "obs#1"}, "resource": resource})
    };
    let mut ids = Vec::new();
    for code in [Some("social-history"), Some("imaging"), Some("procedure"), None] {
        let id = item(&a, "carmen", "fhir_unmapped", Some(held(code)), json!({})).await?;
        sqlx::query("update review_item set document_id = $1 where id = $2")
            .bind(doc)
            .bind(id)
            .execute(&a.rt.pool)
            .await?;
        ids.push(id);
    }
    let shown = |grant: &'static [&'static str]| {
        let a = &a;
        let ids = ids.clone();
        async move {
            alex_on_carmen(a, grant).await?;
            let mut listed = Vec::new();
            for id in ids {
                let one = a.get("alex", &format!("/review/{id}")).await?.0;
                if one == StatusCode::OK {
                    listed.push(id);
                }
            }
            anyhow::Ok(listed)
        }
    };
    assert_eq!(shown(&["read:summary", "read:labs"]).await?, [ids[3]], "labs only");
    assert_eq!(shown(&["read:summary", "read:vitals"]).await?, ids[..3], "vitals only");
    Ok(())
}

/// L-JOBFAILDOC: a failed job of the household itself can name a document of a profile the reviewing
/// reader cannot read. Sam reviews on her own profile, so she sees the household's item, but not the
/// document of Carmen's it names, nor the error its import wrote. Alex, who reads Carmen's documents,
/// gets both.
#[tokio::test]
async fn test_a_household_item_names_its_document_only_to_a_reader_of_that_document() -> R {
    let a = app().await?;
    let doc = carmen_document(&a, "informe_carmen.pdf", json!({})).await?;
    let id = household_item_on(
        &a,
        "job_failed",
        doc,
        json!({"kind": "import.pdf", "error": "informe_carmen.pdf: unreadable"}),
    )
    .await?;
    for (who, sees) in [("sam", false), ("alex", true)] {
        let (s, listed) = a.get(who, "/review").await?;
        assert_eq!(s, StatusCode::OK, "{listed}");
        let row = listed["items"]
            .as_array()
            .and_then(|items| items.iter().find(|i| i["id"] == json!(id)).cloned())
            .ok_or_else(|| anyhow::anyhow!("{who}: the household item is not listed: {listed}"))?;
        let (s, one) = a.get(who, &format!("/review/{id}")).await?;
        assert_eq!(s, StatusCode::OK, "{one}");
        for got in [&row, &one] {
            assert_eq!(got["document_id"] == json!(doc), sees, "{who}: {got}");
            assert_eq!(
                got.to_string().contains("informe_carmen"),
                sees,
                "{who}: the error quotes the document: {got}"
            );
            assert_eq!(got["payload"]["kind"], json!("import.pdf"), "{who}: {got}");
        }
    }
    Ok(())
}

/// F-FAILEDJOBDOC: a failed job on a profile can name a document the reviewing reader cannot read: a
/// lab report needs read:labs. Alex reviews on Carmen's profile with read:records alone, so he sees the
/// failed job but not the document it names nor the error its import wrote, in the list and in the
/// detail. With read:labs he gets both.
#[tokio::test]
async fn test_a_failed_job_on_a_profile_names_its_document_only_to_a_reader_of_that_document() -> R {
    let a = app().await?;
    let doc: Uuid = sqlx::query_scalar(
        "insert into document (household_id, profile_id, blob_sha256, filename, mime, size_bytes, doc_type) \
         values ($1, $2, decode($3, 'hex'), 'analitica_carmen.pdf', 'application/pdf', 3, 'lab_report') returning id",
    )
    .bind(a.family.household_id)
    .bind(a.family.profile("carmen"))
    .bind(ownhealth_core::ids::token_hex(32))
    .fetch_one(&a.rt.pool)
    .await?;
    let id = {
        let mut c = a.conn().await?;
        create_item(
            &mut c,
            &ReviewItemIn {
                kind: "job_failed".into(),
                household_id: Some(a.family.household_id),
                profile_id: Some(a.family.profile("carmen")),
                document_id: Some(doc),
                payload: json!({"kind": "import.pdf", "error": "analitica_carmen.pdf: unreadable"}),
                reasons: vec!["x".into()],
                ..ReviewItemIn::default()
            },
        )
        .await?
    };
    for (grant, sees) in [
        (&["read:summary", "read:records"][..], false),
        (&["read:summary", "read:records", "read:labs"][..], true),
    ] {
        alex_on_carmen(&a, grant).await?;
        let (s, listed) = a.get("alex", "/review?profile=carmen").await?;
        assert_eq!(s, StatusCode::OK, "{listed}");
        let row = listed["items"]
            .as_array()
            .and_then(|items| items.iter().find(|i| i["id"] == json!(id)).cloned())
            .ok_or_else(|| anyhow::anyhow!("{grant:?}: the failed job is not listed: {listed}"))?;
        let (s, one) = a.get("alex", &format!("/review/{id}")).await?;
        assert_eq!(s, StatusCode::OK, "{one}");
        for got in [&row, &one] {
            assert_eq!(got["document_id"] == json!(doc), sees, "{grant:?}: {got}");
            assert_eq!(
                got.to_string().contains("analitica_carmen"),
                sees,
                "{grant:?}: the error quotes the document: {got}"
            );
            assert_eq!(got["payload"]["kind"], json!("import.pdf"), "{grant:?}: {got}");
        }
    }
    Ok(())
}

/// F-DOCFILTER: a `document_id` filter on a document the reader cannot read answers as one on a
/// document that does not exist: no items. Sam reviews on her own profile, so the household's failed
/// job shows to her, but filtering on Carmen's document it names must not tell her that it does.
/// Alex, who reads Carmen's documents, gets the item through the same filter.
#[tokio::test]
async fn test_a_document_filter_on_an_unreadable_document_finds_nothing() -> R {
    let a = app().await?;
    let doc = carmen_document(&a, "informe_carmen.pdf", json!({})).await?;
    let id = household_item_on(
        &a,
        "job_failed",
        doc,
        json!({"kind": "import.pdf", "error": "unreadable"}),
    )
    .await?;
    let filtered = |who: &'static str, doc: Uuid| {
        let a = &a;
        async move {
            let (s, v) = a.get(who, &format!("/review?document_id={doc}")).await?;
            assert_eq!(s, StatusCode::OK, "{v}");
            anyhow::Ok(
                v["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|i| i["id"].clone())
                    .collect::<Vec<_>>(),
            )
        }
    };
    assert_eq!(
        filtered("sam", doc).await?,
        Vec::<Value>::new(),
        "sam cannot read the document"
    );
    assert_eq!(
        filtered("sam", Uuid::new_v4()).await?,
        Vec::<Value>::new(),
        "no such document"
    );
    assert_eq!(filtered("alex", doc).await?, [json!(id)], "a reader of the document");
    // without the filter Sam still sees the household's item, its document left out
    let (_, listed) = a.get("sam", "/review").await?;
    assert!(listed.to_string().contains(&id.to_string()), "{listed}");
    Ok(())
}

/// F-GROUPEDPROFILE: a pull's grouped item shows to whoever the file route would serve its bundle. A
/// move leaves the item, dismissed, on the profile the bundle came from, so the check reads the
/// profile the bundle is on now: once Carmen's bundle moves to Sam's profile, Alex's dismissed list
/// on Carmen no longer shows the item, and its detail answers 404.
#[tokio::test]
async fn test_a_grouped_item_follows_the_profile_its_bundle_is_on_now() -> R {
    let a = app().await?;
    alex_on_carmen(&a, &["read:summary", "read:records"]).await?;
    let doc = carmen_document(&a, "Hospital LabNorte · pull.json", json!({})).await?;
    let summary = json!({"projection": null, "resource_type": "Coverage", "count": 1});
    let id = item(&a, "carmen", "fhir_unmapped_summary", Some(summary), json!({})).await?;
    sqlx::query("update review_item set document_id = $1, status = 'dismissed' where id = $2")
        .bind(doc)
        .bind(id)
        .execute(&a.rt.pool)
        .await?;
    let shown = || async {
        let (s, v) = a.get("alex", "/review?profile=carmen&status=dismissed").await?;
        assert_eq!(s, StatusCode::OK, "{v}");
        let listed = v["items"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|i| i["id"] == json!(id));
        anyhow::Ok((listed, a.get("alex", &format!("/review/{id}")).await?.0))
    };
    assert_eq!(shown().await?, (true, StatusCode::OK), "on Carmen's profile");
    sqlx::query("update document set profile_id = $1 where id = $2")
        .bind(a.family.profile("sam"))
        .bind(doc)
        .execute(&a.rt.pool)
        .await?;
    assert_eq!(shown().await?, (false, StatusCode::NOT_FOUND), "moved to Sam's profile");
    Ok(())
}

/// F-BATCHSNAP: the review list and its counts read the queue a batch at a time, each batch in its own
/// statement. A row closed between two batches must not move an open row out of reach: with 201 open
/// rows, the first batch reads 200; one of them closes; the next batch still reads the 201st.
#[tokio::test]
async fn test_a_review_scan_reaches_every_open_row_when_one_closes_between_batches() -> R {
    let a = app().await?;
    let older = item(&a, "carmen", "candidate_row", None, json!({})).await?;
    sqlx::query(
        "insert into review_item (household_id, profile_id, kind, payload, reasons, created_at) \
         select household_id, profile_id, kind, payload, reasons, created_at - make_interval(secs => g) \
         from review_item, generate_series(1, 199) g where id = $1",
    )
    .bind(older)
    .execute(&a.rt.pool)
    .await?;
    let newest = item(&a, "carmen", "candidate_row", None, json!({})).await?;
    let mut conn = a.conn().await?;
    let mut scan = Scan::new(
        ItemQuery {
            household_id: Some(a.family.household_id),
            profile_ids: Some(vec![a.family.profile("carmen")]),
            status: Some("open".into()),
            ..ItemQuery::default()
        },
        &[],
    );
    let first = scan.next(&mut conn).await?;
    assert_eq!(first.len(), 200);
    assert!(first.iter().all(|i| i.id != newest));
    sqlx::query("update review_item set status = 'dismissed' where id = $1")
        .bind(first[0].id)
        .execute(&a.rt.pool)
        .await?;
    let second: Vec<Uuid> = scan.next(&mut conn).await?.iter().map(|i| i.id).collect();
    assert_eq!(second, [newest], "the 201st open row");
    assert!(scan.next(&mut conn).await?.is_empty());
    Ok(())
}

/// Which of `ids` Alex may open on Carmen's profile with `grant`, by `GET /review/{id}`, and the
/// review list's `open` total for Carmen.
async fn alex_opens(a: &App, grant: &[&str], ids: &[Uuid]) -> anyhow::Result<(Vec<Uuid>, Value)> {
    alex_on_carmen(a, grant).await?;
    let mut opened = Vec::new();
    for id in ids {
        if a.get("alex", &format!("/review/{id}")).await?.0 == StatusCode::OK {
            opened.push(*id);
        }
    }
    let (s, listed) = a.get("alex", "/review?profile=carmen").await?;
    assert_eq!(s, StatusCode::OK, "{listed}");
    let in_list: Vec<Uuid> = listed["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i["id"].as_str().and_then(|id| Uuid::parse_str(id).ok()))
        .filter(|id| ids.contains(id))
        .collect();
    assert_eq!(in_list, opened, "the list shows what the item route opens");
    Ok((opened, listed["open"].clone()))
}

/// U7-IMPORTFAIL: a structured import that failed before its document existed keeps the upload's name
/// in its error. `GET /api/jobs` shows that error only with read:records on the job's profile, so the
/// review item shows on the same terms, in the list, its `open` total and the item route.
#[tokio::test]
async fn test_a_structured_import_that_failed_before_its_document_needs_read_records() -> R {
    let a = app().await?;
    let mut ids = Vec::new();
    for kind in ["import.fhir_bundle", "import.apple_health", "import.ccda"] {
        let payload = json!({"job_id": 1, "kind": kind, "attempts": 3, "payload_document_id": null,
                             "error": "historia_carmen.json: the uploaded file is not in the blob store"});
        ids.push(item(&a, "carmen", "job_failed", Some(payload), json!({})).await?);
    }
    let (opened, labs_open) = alex_opens(&a, &["read:summary", "read:labs"], &ids).await?;
    assert_eq!(opened, Vec::<Uuid>::new(), "labs only");
    let (opened, records_open) = alex_opens(&a, &["read:summary", "read:labs", "read:records"], &ids).await?;
    assert_eq!(opened, ids, "with read:records");
    assert_eq!(
        records_open.as_i64(),
        labs_open.as_i64().map(|n| n + 3),
        "the open total follows"
    );
    Ok(())
}

/// U7-FAMILYMEMBER: a held relative (a FamilyMemberHistory under `payload.member`) shows on the family
/// list's terms: read:records, and read:sensitive too for a relative with a sensitivity tag.
#[tokio::test]
async fn test_a_held_relative_shows_only_with_read_records_and_read_sensitive() -> R {
    let a = app().await?;
    let carmen = a.family.profile("carmen");
    let held = |sensitivity: &[&str]| {
        json!({"projection": "family_member", "resource_type": "FamilyMemberHistory",
               "json_pointer": "/entry/0/resource", "src": {"import_ref": "familia#1"},
               "member": {"profile_id": carmen, "relationship": "mother", "display_name": "Madre de Carmen",
                          "deceased": false, "conditions": [], "sensitivity": sensitivity}})
    };
    let plain = item(&a, "carmen", "fhir_unmapped", Some(held(&[])), json!({})).await?;
    let tagged = item(&a, "carmen", "fhir_unmapped", Some(held(&["mental_health"])), json!({})).await?;
    let ids = [plain, tagged];
    assert_eq!(
        alex_opens(&a, &["read:summary", "read:labs"], &ids).await?.0,
        Vec::<Uuid>::new(),
        "labs only"
    );
    assert_eq!(
        alex_opens(&a, &["read:summary", "read:records"], &ids).await?.0,
        [plain],
        "records"
    );
    assert_eq!(
        alex_opens(&a, &["read:summary", "read:records", "read:sensitive"], &ids)
            .await?
            .0,
        ids,
        "records and read:sensitive"
    );
    Ok(())
}
