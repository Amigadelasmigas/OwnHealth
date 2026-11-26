//! Which open review items a reader is shown: the one rule behind REST `GET /api/review`, its `open`
//! total, `review_open` on `/api/me` and the dashboard, a document's `open_review`, and MCP
//! get_health_status `review_open`, so no count holds an item the list hides.

use std::collections::HashMap;

use ownhealth_auth::household::can_manage;
use ownhealth_core::Result;
use ownhealth_core::access::Permission;
use ownhealth_core::review::{self, ItemQuery, ReviewItem};
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::envelope::truthy;

/// A sync-gap alert's kind.
pub const SOURCE_GAP: &str = "source_gap";

fn obs_scope(category: Option<&str>) -> &'static str {
    match category {
        Some("vital-signs" | "survey" | "exam") => "read:vitals",
        Some("activity") => "read:wearables",
        _ => "read:labs",
    }
}

fn is_truthy(v: Option<&Value>) -> bool {
    v.is_some_and(truthy)
}

/// A held payload carries the values it would write: show it only to someone whose scopes would
/// show the committed fact (its category scope, and read:sensitive for a sensitive one). A held
/// imaging study names the patient and the study, which the timeline and the viewer show only with
/// read:records, and a proposed relative is family history, which needs read:records too. A sync-gap
/// alert names the device's vendor, and a dismissal closes it for the whole household, so it shows
/// only to who manages the profile and its connections, as the Connections screen does. A FHIR
/// resource a file import held unmapped carries the whole resource ([`resource_shows`]).
pub fn visible(item: &ReviewItem, perm: &Permission) -> bool {
    if item.kind == SOURCE_GAP && !can_manage(perm) {
        return false;
    }
    if item.kind == ownhealth_imaging::import::REVIEW_KIND && !perm.has("read:records") {
        return false;
    }
    // a failed DICOM import's error can name the upload and its studies, as its job details do
    if item.kind == "job_failed"
        && item.payload.get("kind").and_then(Value::as_str) == Some(ownhealth_imaging::import::JOB)
        && !perm.has("read:records")
    {
        return false;
    }
    // a relative from a family tree file: the family list shows relatives only with read:records
    if item.kind == ownhealth_core::gedcom::KIND && !perm.has("read:records") {
        return false;
    }
    let empty = Map::new();
    let p = item.payload.as_object().unwrap_or(&empty);
    let mut observations: Vec<&Map<String, Value>> = Vec::new();
    if let Some(Value::Object(o)) = p.get("observation") {
        observations.push(o);
    }
    if let Some(Value::Array(rows)) = p.get("rows") {
        observations.extend(
            rows.iter()
                .filter_map(|r| r.get("observation"))
                .filter_map(Value::as_object),
        );
    }
    if let Some(resource) = p.get("resource").filter(|r| r.is_object())
        && !resource_shows(resource, perm)
    {
        return false;
    }
    let record = p.get("record").and_then(Value::as_object);
    if let Some(record) = record
        && (!perm.has("read:records")
            || (is_truthy(record.get("sensitivity")) && !perm.may_read_sensitive("read:records")))
    {
        return false;
    }
    observations.iter().all(|o| {
        let category = o.get("category").and_then(Value::as_str).filter(|c| !c.is_empty());
        let scope = obs_scope(category);
        perm.has(scope) && (!is_truthy(o.get("sensitivity")) || perm.may_read_sensitive(scope))
    })
}

/// A held FHIR resource (`fhir_unmapped` from a file: the item holds the `resource` as received) shows
/// on the terms of the fact it would be: an Observation with the scope of each of its categories (as
/// [`obs_scope`] maps them; read:labs without one), any other type with read:records, and with
/// read:sensitive too when a security label or a social-history code makes it sensitive.
fn resource_shows(resource: &Value, perm: &Permission) -> bool {
    let mut scopes: Vec<&str> = Vec::new();
    if resource.get("resourceType").and_then(Value::as_str) == Some("Observation") {
        scopes.extend(
            resource
                .get("category")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|c| c.get("coding").and_then(Value::as_array))
                .flatten()
                .filter_map(|c| c.get("code").and_then(Value::as_str))
                .map(|code| obs_scope(Some(code))),
        );
        if scopes.is_empty() {
            scopes.push(obs_scope(None));
        }
    } else {
        scopes.push("read:records");
    }
    let sensitive = !ownhealth_ingest_structured::fhir::mapping::sensitivity(resource).is_empty();
    scopes
        .iter()
        .all(|s| perm.has(s) && (!sensitive || perm.may_read_sensitive(s)))
}

/// A pull's grouped item carries no values: it points at the bundle that holds them. It shows to
/// whoever can open that bundle, so its link never answers 403.
pub async fn opens_document(conn: &mut PgConnection, item: &ReviewItem, perm: &Permission) -> Result<bool> {
    match (item.kind.as_str(), item.document_id) {
        ("fhir_unmapped_summary", Some(doc)) => ownhealth_core::documents::file_opens(conn, perm, doc).await,
        _ => Ok(true),
    }
}

/// `review_open` on the dashboard and in MCP get_health_status: what the list for this one profile
/// shows the caller, the household's own items included. A reader who cannot review there is shown
/// none.
pub async fn count_shown_on_profile(conn: &mut PgConnection, perm: &Permission) -> Result<i64> {
    if !perm.can("review") {
        return Ok(0);
    }
    count_shown(conn, perm.profile.household_id, &[perm]).await
}

/// `open_review` of documents on the profile of `perm`, per document: the open items of the document
/// that the review list shows the caller, the household's own included. A reader who cannot review
/// there is shown none, so the count never holds an item the list hides (a sensitive held value
/// without read:sensitive, say).
pub async fn shown_per_document(
    conn: &mut PgConnection,
    perm: &Permission,
    documents: &[Uuid],
) -> Result<HashMap<Uuid, i64>> {
    let mut shown = HashMap::new();
    if !perm.can("review") || documents.is_empty() {
        return Ok(shown);
    }
    let items: Vec<ReviewItem> = sqlx::query_as(
        "select * from review_item where status = 'open' and document_id = any($1) and household_id = $2 \
         and (profile_id = $3 or profile_id is null)",
    )
    .bind(documents)
    .bind(perm.profile.household_id)
    .bind(perm.profile.id)
    .fetch_all(&mut *conn)
    .await?;
    for i in &items {
        let show = match i.profile_id {
            // the list takes a held study or a sync alert only on a profile it names
            None => i.kind != ownhealth_imaging::import::REVIEW_KIND && i.kind != SOURCE_GAP,
            Some(_) => {
                let show = visible(i, perm) && opens_document(&mut *conn, i, perm).await?;
                if show && i.kind == ownhealth_imaging::import::REVIEW_KIND {
                    perm.record_read(&["read:records"]);
                }
                show
            }
        };
        if let (true, Some(doc)) = (show, i.document_id) {
            *shown.entry(doc).or_insert(0) += 1;
        }
    }
    Ok(shown)
}

/// The items of the household itself show only to who can review on some profile.
pub fn shows_household_items<'a>(perms: impl IntoIterator<Item = &'a Permission>) -> bool {
    perms.into_iter().any(|p| p.can("review"))
}

/// How many open items the list shows the caller, ordinary and emergency alike: the list's `open` and
/// `review_open` on `/api/me`. `perms` are the caller's permissions on the profiles to count.
pub async fn count_shown(conn: &mut PgConnection, household: Uuid, perms: &[&Permission]) -> Result<i64> {
    let household_items = shows_household_items(perms.iter().copied());
    let by_profile: HashMap<Uuid, &Permission> = perms.iter().map(|p| (p.profile.id, *p)).collect();
    // the list's own query: a held study or a sync alert counts only where the list shows it
    let imaging_profiles = imaging_review_profiles(perms.iter().copied());
    let gap_profiles = gap_review_profiles(perms.iter().copied());
    let items = review::list_items_with_kind_profiles(
        &mut *conn,
        &ItemQuery {
            household_id: Some(household),
            profile_ids: Some(by_profile.keys().copied().collect()),
            status: Some("open".into()),
            limit: i64::MAX,
            ..ItemQuery::default()
        },
        &[
            (ownhealth_imaging::import::REVIEW_KIND, &imaging_profiles),
            (SOURCE_GAP, &gap_profiles),
        ],
    )
    .await?;
    let mut shown = 0;
    for i in &items {
        let show = match i.profile_id.map(|pid| by_profile.get(&pid)) {
            None => household_items,
            Some(Some(p)) => {
                let show = visible(i, p) && opens_document(&mut *conn, i, p).await?;
                if show && i.kind == ownhealth_imaging::import::REVIEW_KIND {
                    p.record_read(&["read:records"]);
                }
                show
            }
            Some(None) => false,
        };
        shown += i64::from(show);
    }
    Ok(shown)
}

/// The profiles whose held imaging studies the caller sees: review and read:records ([`visible`]).
pub fn imaging_review_profiles<'a>(perms: impl IntoIterator<Item = &'a Permission>) -> Vec<Uuid> {
    perms
        .into_iter()
        .filter(|p| p.can("review") && p.has("read:records"))
        .map(|p| p.profile.id)
        .collect()
}

/// The profiles whose sync alerts the caller sees: the ones it manages ([`visible`]).
pub fn gap_review_profiles<'a>(perms: impl IntoIterator<Item = &'a Permission>) -> Vec<Uuid> {
    perms
        .into_iter()
        .filter(|p| p.can("review") && can_manage(p))
        .map(|p| p.profile.id)
        .collect()
}
