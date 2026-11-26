//! Lane I: `api/imports.py`. Uploads (PDF, image, Apple Health export.zip, FHIR bundle, OwnGenome
//! output) and the import jobs they start.
//!
//! ```text
//! POST /api/imports   multipart: profile, files (one or more), timezone (optional, the browser's IANA zone)
//! GET  /api/jobs      ?ids=1,2,3&limit=50   import jobs of the caller's profiles, newest first
//! ```
//!
//! `timezone` reads a CGM export's wall-clock times when the server has no `OWNHEALTH_TIMEZONE`, or one
//! Postgres does not know. An old name the tz database links to a current one (`Asia/Calcutta`) travels
//! as the current name; a zone still unknown travels as no zone, and the upload goes in all the same.
//!
//! Files are spooled to disk while the form is read, then land in the blob store once the caller may
//! import on the profile; the dispatcher creates documents and enqueues each lane's import job.
//!
//! A job's `result` counts what its import brought or lists files a failed import left out. The result of a wearables import
//! ([`WEARABLE_JOBS`]) is shown only to a caller who may read wearables on that profile; anyone else
//! with a grant there sees the job with `result: null`. A document's job (PDF, C-CDA, FHIR, Apple
//! Health) shows its filename, error and result on the terms of that document, where it is now
//! ([`jobs`]).

use std::collections::BTreeMap;
use std::path::PathBuf;

use axum::extract::{DefaultBodyLimit, FromRequest, Multipart, Query, Request, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use ownhealth_core::access::accessible_profiles;
use ownhealth_core::audit::{AuditEvent, write_audit};
use ownhealth_core::blobs::BlobOwner;
use ownhealth_core::documents::{StoredFile, dispatch_upload_in_zone};
use ownhealth_core::pyjson::row_to_json;
use ownhealth_core::{Error, Runtime, ids};
use ownhealth_ingest_structured::DOCUMENT_IMPORT_JOBS;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::Caller;
use crate::error::ApiError;
use crate::routes::auth::{Fail, R};
use crate::validate::{Errors, Params, param, pydantic_datetimes};

pub fn router() -> Router<Runtime> {
    Router::new()
        // uploads have no size cap, as in 0.1.1 (an Apple Health export runs to gigabytes)
        .route("/imports", post(upload).layer(DefaultBodyLimit::disable()))
        .route("/jobs", get(jobs))
}

/// A spooled upload part, removed from disk when dropped.
struct Spool {
    filename: String,
    path: PathBuf,
}

impl Drop for Spool {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn bad_body() -> Fail {
    Fail::Api(ApiError::new(
        StatusCode::BAD_REQUEST,
        "bad_request",
        "There was an error parsing the body",
    ))
}

/// Starlette's MultiPartParser limits in 0.1.1: a non-file part holds at most 1 MiB, and a form has
/// at most 1000 file parts and 1000 other parts. File parts have no size cap.
const MAX_PART: usize = 1024 * 1024;
const MAX_PARTS: usize = 1000;

/// A non-file part's bytes, refused past [`MAX_PART`].
async fn read_capped(field: &mut axum::extract::multipart::Field<'_>) -> R<Vec<u8>> {
    let mut buf = Vec::new();
    while let Some(c) = field.chunk().await.map_err(|_| bad_body())? {
        if buf.len() + c.len() > MAX_PART {
            return Err(bad_body());
        }
        buf.extend_from_slice(&c);
    }
    Ok(buf)
}

async fn spool(field: &mut axum::extract::multipart::Field<'_>, path: &std::path::Path) -> R<()> {
    // the upload is plaintext until the store takes it: readable by this user only
    let mut out = ownhealth_core::blobs::create_spool(path).await.map_err(Error::from)?;
    while let Some(chunk) = field.chunk().await.map_err(|_| bad_body())? {
        out.write_all(&chunk).await.map_err(Error::from)?;
    }
    out.flush().await.map_err(Error::from)?;
    Ok(())
}

async fn upload(State(rt): State<Runtime>, caller: Caller, req: Request) -> R<Json<Value>> {
    let principal = &caller.0;
    let mut profile: Option<String> = None;
    let mut timezone: Option<String> = None;
    let mut files: Vec<Spool> = Vec::new();
    // a body that is not multipart has neither field, which FastAPI reports as two missing fields
    if let Ok(mut form) = Multipart::from_request(req, &rt).await {
        let (mut file_parts, mut field_parts) = (0usize, 0usize);
        while let Some(mut field) = form.next_field().await.map_err(|_| bad_body())? {
            let is_file = field.file_name().is_some();
            if is_file {
                file_parts += 1;
            } else {
                field_parts += 1;
            }
            if file_parts > MAX_PARTS || field_parts > MAX_PARTS {
                return Err(bad_body());
            }
            match field.name() {
                Some("profile") => {
                    let bytes = read_capped(&mut field).await?;
                    profile = Some(String::from_utf8(bytes).map_err(|_| bad_body())?);
                }
                Some("timezone") if !is_file => {
                    let bytes = read_capped(&mut field).await?;
                    timezone = Some(String::from_utf8(bytes).map_err(|_| bad_body())?);
                }
                Some("files") => {
                    let filename = field
                        .file_name()
                        .filter(|f| !f.is_empty())
                        .unwrap_or("upload")
                        .to_string();
                    let path = rt
                        .blobs
                        .staging_path(&format!("upload-{}", ids::token_hex(16)))
                        .map_err(Error::from)?;
                    // the Spool owns the file before it exists, so a failed read still removes it
                    files.push(Spool {
                        filename,
                        path: path.clone(),
                    });
                    spool(&mut field, &path).await?;
                }
                _ if !is_file => {
                    read_capped(&mut field).await?;
                }
                _ => {}
            }
        }
    }
    let mut e = Errors::new();
    if profile.is_none() {
        e.missing(vec![json!("body"), json!("profile")], Value::Null);
    }
    if files.is_empty() {
        e.missing(vec![json!("body"), json!("files")], Value::Null);
    }
    e.done()?;
    let profile = profile.unwrap_or_default();

    let mut tx = rt.pool.begin().await?;
    let perm = caller.require(&mut tx, &profile, None, Some("import")).await?;
    // never a refusal: a zone Postgres does not know, even by its current name, is no zone
    let zone = ownhealth_devices::cgm_csv::known_zone(&mut tx, timezone.as_deref()).await?;
    let mut stored = Vec::with_capacity(files.len());
    for f in &files {
        let blobs = rt.blobs.clone();
        let path = f.path.clone();
        let owner = BlobOwner::Profile(perm.profile.id);
        let (sha256, size) = tokio::task::spawn_blocking(move || blobs.put_file(owner, &path))
            .await
            .map_err(|e| Error::Other(e.into()))?
            .map_err(Error::from)?;
        stored.push(StoredFile {
            filename: f.filename.clone(),
            sha256,
            size: i64::try_from(size).unwrap_or(i64::MAX),
        });
    }
    drop(files);
    let result = match dispatch_upload_in_zone(
        &mut tx,
        &rt.blobs,
        perm.profile.household_id,
        perm.profile.id,
        &stored,
        principal.user_id,
        zone.as_deref(),
    )
    .await
    {
        Ok(r) => r,
        Err(Error::Upload(m)) => {
            return Err(Fail::Api(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_file",
                m,
            )));
        }
        Err(e) => return Err(e.into()),
    };
    // a report uploaded again brings back the studies retracted with it that the caller may change
    let documents: Vec<Uuid> = result
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|i| i.get("document_id").and_then(Value::as_str))
        .filter_map(|id| Uuid::parse_str(id).ok())
        .collect();
    for document_id in documents {
        crate::routes::documents::restore_studies(&mut tx, principal, document_id).await?;
    }
    let shas: Vec<String> = result
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|i| i.get("sha256").and_then(Value::as_str).unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    let mut audit = AuditEvent::new(principal.actor_kind(), "ui", "import");
    audit.actor_id = principal.user_id;
    audit.tool = Some("POST /api/imports".into());
    audit.profile_id = Some(perm.profile.id);
    audit.resource_type = Some("upload".into());
    audit.resource_ids = Some(shas);
    write_audit(&mut tx, &audit).await?;
    tx.commit().await?;
    Ok(Json(result))
}

/// The import jobs whose result counts a profile's wearable samples and names its devices (a CGM
/// export's result also counts its Low and High readings).
pub const WEARABLE_JOBS: [&str; 6] = [
    "import.fit",
    "import.gadgetbridge",
    "import.cgm_csv",
    ownhealth_devices::garmin_zip::JOB,
    ownhealth_devices::strava_zip::JOB,
    ownhealth_devices::takeout_fitbit::JOB,
];

/// The PDF and image import job; its payload names its document.
const PDF_JOB: &str = "import.pdf";

/// A job's filename, error and result, left out of what the caller is shown.
fn withhold(j: &mut Value) {
    j["result"] = Value::Null;
    j["filename"] = Value::Null;
    j["last_error"] = Value::Null;
}

/// Import jobs of the caller's profiles, newest first; `ids=1,2,3` to poll specific ones.
///
/// A job's filename names its upload, its error can quote it, and its result can count health data, so
/// each goes out only on the terms of what the job read:
/// - a PDF job or a [`DOCUMENT_IMPORT_JOBS`] job, once its document exists: the scope that document
///   needs on the profile it is on now (a move does not rewrite a finished job), and read:sensitive
///   too for the result, as the document's import flags do (the summary's counts take in items the
///   review list hides). A job whose document is gone, or is on a profile the caller cannot read with
///   that scope, shows none of them; so does a PDF job that names no document. A structured job whose
///   document does not exist yet shows its filename and error with read:records on its profile.
/// - a [`WEARABLE_JOBS`] job: read:wearables on its profile; a DICOM import: read:records.
///
/// A response that shows a job's filename, error or result writes one read audit row per profile it
/// read (for a document's job, the profile the document is on now) naming those jobs, for every job
/// kind. A job that shows only its state, kind and times is not a read of health data and is not
/// named, with one exception: a DICOM import is named for every caller with read:records on its
/// profile, whatever it shows. A response that names no job writes no row, except an empty one, which
/// writes a row with no profile (RUST-CONTRACTS.md section 3). A document's job that shows none of
/// its fields does not name its document either (`document_id` is null), as `GET /api/review` hides
/// the document of a failed job.
async fn jobs(State(rt): State<Runtime>, caller: Caller, Query(q): Query<Params>) -> R<Json<Value>> {
    let mut e = Errors::new();
    let limit = e.int("query", &q, "limit", 50, Some(1), Some(200));
    e.done()?;
    let ids: Option<Vec<i64>> = param(&q, "ids").filter(|s| !s.is_empty()).map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|i| !i.is_empty() && i.bytes().all(|b| b.is_ascii_digit()))
            .filter_map(|i| i.parse().ok())
            .collect()
    });
    let mut tx = rt.pool.begin().await?;
    let perms = accessible_profiles(&mut tx, &caller.0).await?;
    let pids: Vec<String> = perms.iter().map(|p| p.profile.id.to_string()).collect();
    let rows = sqlx::query(
        "select id, kind, state, attempts, max_attempts, last_error, result, created_at, finished_at, \
         payload->>'profile_id' as profile_id, payload->>'filename' as filename, \
         payload->>'document_id' as document_id from job \
         where payload->>'profile_id' = any($1) and ($2::bigint[] is null or id = any($2)) \
         order by id desc limit $3",
    )
    .bind(&pids)
    .bind(&ids)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    let mut jobs = rows
        .iter()
        .map(|r| row_to_json(r).map(Value::Object))
        .collect::<Result<Vec<_>, _>>()?;
    // Audit, by profile, every job whose filename, error or result goes out.
    let mut read_jobs = BTreeMap::<_, Vec<_>>::new();
    for j in &mut jobs {
        pydantic_datetimes(j, &["created_at", "finished_at"]);
        let permission = j["profile_id"]
            .as_str()
            .and_then(|id| pids.iter().position(|pid| pid == id))
            .map(|i| &perms[i]);
        let kind = j["kind"].as_str().unwrap_or_default();
        let wearable = WEARABLE_JOBS.contains(&kind);
        let dicom = kind == ownhealth_imaging::import::JOB;
        let pdf = kind == PDF_JOB;
        let structured = DOCUMENT_IMPORT_JOBS.contains(&kind);
        if let Some(perm) = permission {
            perm.record_read(if wearable { &["read:wearables"] } else { &[] });
        }
        // the permission whose profile the shown fields belong to, when they may go out at all
        let mut reader = permission;
        if pdf || structured {
            let doc = j["document_id"]
                .as_str()
                .or_else(|| j["result"]["document_id"].as_str())
                .map(|d| Uuid::parse_str(d).ok());
            let shown = match doc {
                // a structured import before its document exists: the upload on the job's profile
                None if structured => permission
                    .filter(|p| p.has("read:records"))
                    .map(|p| (p, "read:records")),
                None => None,
                Some(None) => None,
                Some(Some(d)) => match ownhealth_core::documents::document_profile_scope(&mut tx, d).await? {
                    Some((Some(profile), scope)) => perms
                        .iter()
                        .find(|p| p.profile.id == profile && p.has(scope))
                        .map(|p| (p, scope)),
                    _ => None,
                },
            };
            reader = shown.map(|(p, _)| p);
            match shown {
                Some((perm, scope)) => {
                    perm.record_read(&[scope]);
                    if !perm.may_read_sensitive(scope) {
                        j["result"] = Value::Null;
                    }
                }
                None => {
                    withhold(j);
                    j["document_id"] = Value::Null;
                }
            }
        }
        if wearable && !permission.is_some_and(|p| p.has("read:wearables")) {
            withhold(j);
        }
        if dicom {
            if let Some(perm) = permission.filter(|p| p.has("read:records")) {
                perm.record_read(&["read:records"]);
                read_jobs
                    .entry(Some(perm.profile.id))
                    .or_default()
                    .push(j["id"].to_string());
            } else {
                withhold(j);
            }
        }
        // A filename names a document, an error can quote it, and a result can expose health data. A
        // job that shows none of them (an unfinished poll, a withheld job) is no read.
        let shown = ["filename", "last_error", "result"]
            .iter()
            .any(|field| !j[*field].is_null());
        if !dicom
            && shown
            && let Some(perm) = reader
        {
            read_jobs
                .entry(Some(perm.profile.id))
                .or_default()
                .push(j["id"].to_string());
        }
    }
    if jobs.is_empty() {
        read_jobs.insert(None, Vec::new());
    }
    for (profile_id, job_ids) in read_jobs {
        let mut audit = AuditEvent::new(caller.0.actor_kind(), "ui", "read");
        audit.actor_id = caller.0.user_id;
        audit.token_id = caller.0.token_id;
        audit.tool = Some("GET /api/jobs".into());
        audit.profile_id = profile_id;
        audit.resource_type = Some("job".into());
        audit.rows_out = i32::try_from(job_ids.len()).ok();
        audit.resource_ids = Some(job_ids);
        write_audit(&mut tx, &audit).await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"jobs": jobs})))
}
