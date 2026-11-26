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
    // a failed import's error can name the upload, as its job details do: a DICOM import's (and its
    // studies), and a structured import's that failed before its document existed, whose upload
    // `GET /api/jobs` names only with read:records on the profile
    if item.kind == "job_failed"
        && let Some(job) = item.payload.get("kind").and_then(Value::as_str)
        && (job == ownhealth_imaging::import::JOB
            || (item.document_id.is_none() && ownhealth_ingest_structured::DOCUMENT_IMPORT_JOBS.contains(&job)))
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
    // a FHIR import's held record, or a held relative (`member`), which the family list shows only
    // with read:records, and a relative with a sensitivity tag only with read:sensitive too
    for held in ["record", "member"] {
        if let Some(held) = p.get(held).and_then(Value::as_object)
            && (!perm.has("read:records")
                || (is_truthy(held.get("sensitivity")) && !perm.may_read_sensitive("read:records")))
        {
            return false;
        }
    }
    observations.iter().all(|o| {
        let category = o.get("category").and_then(Value::as_str).filter(|c| !c.is_empty());
        let scope = obs_scope(category);
        perm.has(scope) && (!is_truthy(o.get("sensitivity")) || perm.may_read_sensitive(scope))
    })
}

/// A held FHIR resource (`fhir_unmapped` from a file: the item holds the `resource` as received) shows
/// on the terms of the fact it would be: an Observation with the scope of each of its categories, read
/// as the importer reads them (`mapping::category_of`: `social-history` is a survey, `imaging` and
/// `procedure` are exams) and mapped by [`obs_scope`], read:labs when none is one we read; any other
/// type with read:records; and with read:sensitive too when a security label or a social-history code
/// makes it sensitive.
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
                .filter_map(ownhealth_ingest_structured::fhir::mapping::category_of)
                .map(|category| obs_scope(Some(category))),
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

/// [`opens_document`] decided once per document and profile within one count: many grouped items of
/// one bundle cost one lookup.
type Opens = HashMap<(Uuid, Uuid), bool>;

async fn opens_document_once(
    conn: &mut PgConnection,
    item: &ReviewItem,
    perm: &Permission,
    decided: &mut Opens,
) -> Result<bool> {
    let Some(doc) = item.document_id.filter(|_| item.kind == "fhir_unmapped_summary") else {
        return Ok(true);
    };
    if let Some(opens) = decided.get(&(perm.profile.id, doc)) {
        return Ok(*opens);
    }
    let opens = opens_document(conn, item, perm).await?;
    decided.insert((perm.profile.id, doc), opens);
    Ok(opens)
}

/// Whether item `i` of the profile of `perm` counts for the caller, as the list shows it. A count that
/// holds an item only emergency access shows reads through that access, so it records the use as the
/// list does; an item the caller's own grant shows records none.
async fn shown_on(conn: &mut PgConnection, i: &ReviewItem, perm: &Permission, decided: &mut Opens) -> Result<bool> {
    let show = visible(i, perm) && opens_document_once(&mut *conn, i, perm, decided).await?;
    if show {
        if i.kind == ownhealth_imaging::import::REVIEW_KIND {
            perm.record_read(&["read:records"]);
        }
        if !visible(i, &perm.own_only()) {
            perm.read_scopes();
        }
    }
    Ok(show)
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
    let mut decided = Opens::new();
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
            Some(_) => shown_on(&mut *conn, i, perm, &mut decided).await?,
        };
        if let (true, Some(doc)) = (show, i.document_id) {
            *shown.entry(doc).or_insert(0) += 1;
        }
    }
    Ok(shown)
}

/// How many rows a [`Scan`] reads at a time.
pub const SCAN_BATCH: i64 = 200;

/// The review queue read a batch of [`SCAN_BATCH`] rows at a time, in the list's order (`created_at`,
/// `id`), so a list page or a count holds one batch of rows (and their payloads) in memory at most.
/// Each batch seeks past the last row of the one before, so a row closed between two batches moves no
/// open row out of reach, as a raw offset would.
pub struct Scan<'k> {
    query: ItemQuery,
    kinds: &'k [(&'k str, &'k [Uuid])],
    done: bool,
}

impl<'k> Scan<'k> {
    /// A scan of `query` (its `limit`, `offset` and `after` are the scan's own) with
    /// [`review::list_items_with_kind_profiles`]'s narrower profiles per kind.
    pub fn new(query: ItemQuery, kinds: &'k [(&'k str, &'k [Uuid])]) -> Self {
        Scan {
            query: ItemQuery {
                limit: SCAN_BATCH,
                offset: 0,
                after: None,
                ..query
            },
            kinds,
            done: false,
        }
    }

    /// The next batch; empty once the queue has no more rows.
    pub async fn next(&mut self, conn: &mut PgConnection) -> Result<Vec<ReviewItem>> {
        if self.done {
            return Ok(Vec::new());
        }
        let batch = review::list_items_with_kind_profiles(conn, &self.query, self.kinds).await?;
        self.done = (batch.len() as i64) < SCAN_BATCH;
        self.query.after = batch.last().map(|i| (i.created_at, i.id));
        Ok(batch)
    }
}

/// The items of the household itself show only to who can review on some profile.
pub fn shows_household_items<'a>(perms: impl IntoIterator<Item = &'a Permission>) -> bool {
    perms.into_iter().any(|p| p.can("review"))
}

/// How many open items the list shows the caller, ordinary and emergency alike: the list's `open` and
/// `review_open` on `/api/me`. `perms` are the caller's permissions on the profiles to count. It reads
/// the queue a [`Scan`] batch at a time, so a large backlog never sits in memory whole.
pub async fn count_shown(conn: &mut PgConnection, household: Uuid, perms: &[&Permission]) -> Result<i64> {
    let household_items = shows_household_items(perms.iter().copied());
    let by_profile: HashMap<Uuid, &Permission> = perms.iter().map(|p| (p.profile.id, *p)).collect();
    // the list's own query: a held study or a sync alert counts only where the list shows it
    let imaging_profiles = imaging_review_profiles(perms.iter().copied());
    let gap_profiles = gap_review_profiles(perms.iter().copied());
    let kinds = [
        (ownhealth_imaging::import::REVIEW_KIND, imaging_profiles.as_slice()),
        (SOURCE_GAP, gap_profiles.as_slice()),
    ];
    let mut scan = Scan::new(
        ItemQuery {
            household_id: Some(household),
            profile_ids: Some(by_profile.keys().copied().collect()),
            status: Some("open".into()),
            ..ItemQuery::default()
        },
        &kinds,
    );
    let mut decided = Opens::new();
    let mut shown = 0;
    loop {
        let batch = scan.next(&mut *conn).await?;
        if batch.is_empty() {
            break;
        }
        for i in &batch {
            let show = match i.profile_id.map(|pid| by_profile.get(&pid)) {
                None => household_items,
                Some(Some(p)) => shown_on(&mut *conn, i, p, &mut decided).await?,
                Some(None) => false,
            };
            shown += i64::from(show);
        }
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
