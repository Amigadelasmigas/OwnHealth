//! v0.2 Ops: the health endpoint per source, with the last successful sync for alerting. JSON for the
//! app, the Prometheus text format for a monitor, and each source readable with its own token.

#[path = "auth_support.rs"]
mod support;

use axum::http::{Method, StatusCode};
use ownhealth_core::demo::create_rivera_family;
use ownhealth_core::sources::{self, NewSource, RunOutcome, SourcePatch};
use serde_json::Value;
use sqlx::PgConnection;
use support::Env;
use uuid::Uuid;

type R = anyhow::Result<()>;

struct Ids {
    alex_email: Uuid,
    carmen_nightscout: Uuid,
    folder: Uuid,
    lucia_hae: Uuid,
    other_household: Uuid,
}

async fn source(
    conn: &mut PgConnection,
    household: Uuid,
    profile: Option<Uuid>,
    kind: &str,
    label: &str,
    poll: Option<i32>,
) -> anyhow::Result<Uuid> {
    Ok(sources::create_source(
        conn,
        &NewSource {
            profile_id: profile,
            poll_seconds: poll,
            ..NewSource::new(household, kind, label)
        },
    )
    .await?)
}

async fn run(conn: &mut PgConnection, id: Uuid, ok: bool) -> R {
    let run = sources::begin_run(conn, id, "poll", None).await?;
    let outcome = RunOutcome::default();
    sources::finish_run(conn, run, if ok { Ok(&outcome) } else { Err("connection refused") }).await?;
    Ok(())
}

async fn setup(env: &Env) -> anyhow::Result<Ids> {
    let mut c = env.pool().acquire().await?;
    let h = env.fam().household_id;
    let ids = Ids {
        alex_email: source(
            &mut c,
            h,
            Some(env.profile("alex")),
            "email",
            "Correo de Alex",
            Some(300),
        )
        .await?,
        carmen_nightscout: source(
            &mut c,
            h,
            Some(env.profile("carmen")),
            "nightscout",
            "Nightscout de Carmen",
            Some(300),
        )
        .await?,
        folder: source(&mut c, h, None, "folder", "Escáner", Some(60)).await?,
        lucia_hae: source(
            &mut c,
            h,
            Some(env.profile("lucia")),
            "health_auto_export",
            "iPad",
            None,
        )
        .await?,
        other_household: {
            let other = create_rivera_family(&mut c, "Otra").await?;
            source(&mut c, other.household_id, None, "folder", "Otra casa", Some(60)).await?
        },
    };
    run(&mut c, ids.alex_email, false).await?;
    run(&mut c, ids.alex_email, false).await?;
    run(&mut c, ids.carmen_nightscout, true).await?;
    // Alex, Carmen's caregiver, also reads her wearables, so her Nightscout shows to him (a reader
    // without them: a_source_shows_only_to_a_reader_of_its_data)
    sqlx::query(
        "update profile_grant set categories = array_append(categories, 'read:wearables') \
         where user_id = $1 and profile_id = $2",
    )
    .bind(env.user("alex"))
    .bind(env.profile("carmen"))
    .execute(&mut *c)
    .await?;
    Ok(ids)
}

fn ids_of(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["id"].as_str().map(str::to_string))
        .collect()
}

fn row(v: &Value, id: Uuid) -> &Value {
    v.as_array()
        .and_then(|a| a.iter().find(|r| r["id"] == id.to_string()))
        .unwrap_or(&Value::Null)
}

#[tokio::test]
async fn each_person_sees_the_sources_of_the_profiles_they_can_open() -> R {
    let env = Env::new(true).await?;
    let ids = setup(&env).await?;

    let mut alex = env.as_user(env.user("alex")).await?;
    let r = alex.get("/api/inboxes/health").await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    let mut got = ids_of(&r.json);
    got.sort();
    let mut want: Vec<String> = [ids.alex_email, ids.carmen_nightscout, ids.folder, ids.lucia_hae]
        .iter()
        .map(Uuid::to_string)
        .collect();
    want.sort();
    assert_eq!(
        got, want,
        "alex: own, carmen (caregiver), lucía (guardian), household-wide"
    );

    let email = row(&r.json, ids.alex_email);
    assert_eq!(email["kind"], "email");
    assert_eq!(email["consecutive_failures"], 2);
    assert_eq!(email["last_error"], "connection refused");
    assert!(email["last_success_at"].is_null());
    assert_eq!(email["stale"], true, "polled and never synced");
    let ns = row(&r.json, ids.carmen_nightscout);
    assert!(ns["last_success_at"].is_string());
    assert_eq!(
        (ns["stale"].as_bool(), ns["consecutive_failures"].as_i64()),
        (Some(false), Some(0))
    );
    assert_eq!(row(&r.json, ids.folder)["stale"], true);
    assert_eq!(
        row(&r.json, ids.lucia_hae)["stale"],
        false,
        "a push source is never stale"
    );
    for r in r.json.as_array().into_iter().flatten() {
        let keys: Vec<&String> = r.as_object().into_iter().flatten().map(|(k, _)| k).collect();
        assert!(
            !keys.iter().any(|k| k.contains("secret") || k.contains("token")),
            "{keys:?}"
        );
    }

    // sam has no grant on carmen, and his viewer grant on alex (read:summary) shows no mailbox
    let mut sam = env.as_user(env.user("sam")).await?;
    let r = sam.get("/api/inboxes/health").await?;
    let got = ids_of(&r.json);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(!got.contains(&ids.carmen_nightscout.to_string()));
    assert!(!got.contains(&ids.alex_email.to_string()));
    assert!(!got.contains(&ids.other_household.to_string()));
    let r = sam
        .get(&format!("/api/inboxes/{}/health", ids.carmen_nightscout))
        .await?;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = alex
        .get(&format!("/api/inboxes/{}/health", ids.carmen_nightscout))
        .await?;
    assert_eq!(
        (r.status, r.json["kind"].as_str()),
        (StatusCode::OK, Some("nightscout"))
    );
    let r = alex
        .get(&format!("/api/inboxes/{}/health", ids.other_household))
        .await?;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = alex.get("/api/inboxes/not-a-uuid/health").await?;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    let r = env.anon().get("/api/inboxes/health").await?;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    Ok(())
}

/// Sam's categories on Alex's profile (sam is a viewer there).
async fn sam_on_alex(env: &Env, categories: &[&str]) -> R {
    sqlx::query("update profile_grant set categories = $3 where user_id = $1 and profile_id = $2")
        .bind(env.user("sam"))
        .bind(env.profile("alex"))
        .bind(categories.iter().map(|c| c.to_string()).collect::<Vec<_>>())
        .execute(env.pool())
        .await?;
    Ok(())
}

/// The two sources `user` is shown, by kind: in the list, and the single row answering 200 for the same.
async fn shown(env: &Env, user: &str, dexcom: Uuid, mail: Uuid) -> anyhow::Result<Vec<&'static str>> {
    let mut c = env.as_user(env.user(user)).await?;
    let r = c.get("/api/inboxes/health").await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    let listed = ids_of(&r.json);
    let mut out = Vec::new();
    for (id, name) in [(dexcom, "dexcom"), (mail, "email")] {
        let one = c.get(&format!("/api/inboxes/{id}/health")).await?;
        let in_list = listed.contains(&id.to_string());
        assert_eq!(
            in_list,
            one.status == StatusCode::OK,
            "{name}: the list and the single row disagree ({:?})",
            one.status
        );
        if in_list {
            out.push(name);
        }
    }
    Ok(out)
}

/// A source's kind and label say what the profile holds: a Dexcom source means a glucose sensor. So a
/// source shows only to a reader who may read the data it brings in: a device's readings need
/// read:wearables, the documents a mailbox files need a scope the document list reads with. The owner
/// sees both, a member with the grant sees that source, a member without it sees neither the row nor
/// the single source.
#[tokio::test]
async fn a_source_shows_only_to_a_reader_of_its_data() -> R {
    let env = Env::new(true).await?;
    let (dexcom, mail) = {
        let mut c = env.pool().acquire().await?;
        let (h, alex) = (env.fam().household_id, Some(env.profile("alex")));
        (
            source(&mut c, h, alex, "dexcom", "Dexcom G7 de Alex", Some(900)).await?,
            source(&mut c, h, alex, "email", "Correo de Alex", Some(300)).await?,
        )
    };
    assert_eq!(
        shown(&env, "alex", dexcom, mail).await?,
        ["dexcom", "email"],
        "the owner"
    );
    // sam is a viewer on alex: read:summary only
    assert!(
        shown(&env, "sam", dexcom, mail).await?.is_empty(),
        "a member without the grant"
    );
    sam_on_alex(&env, &["read:summary", "read:wearables"]).await?;
    assert_eq!(
        shown(&env, "sam", dexcom, mail).await?,
        ["dexcom"],
        "a member with read:wearables"
    );
    sam_on_alex(&env, &["read:summary", "read:labs"]).await?;
    assert_eq!(
        shown(&env, "sam", dexcom, mail).await?,
        ["email"],
        "a member with read:labs"
    );
    Ok(())
}

/// The scopes that read what each source kind brings in, written out here on purpose (not taken from
/// the route), so a change to the route's table shows as a failure.
const KIND_SCOPES: [(&str, &[&str]); 14] = [
    ("email", &["read:labs", "read:records"]),
    ("folder", &["read:labs", "read:records"]),
    ("paperless", &["read:labs", "read:records"]),
    ("share", &["read:labs", "read:records"]),
    ("shortcut", &["read:labs", "read:records"]),
    ("health_auto_export", &["read:wearables"]),
    ("gadgetbridge", &["read:wearables"]),
    ("nightscout", &["read:wearables"]),
    ("openwearables", &["read:wearables"]),
    ("dexcom", &["read:wearables"]),
    ("smart", &["read:records"]),
    ("bluebutton", &["read:records"]),
    ("brands", &["read:records"]),
    ("clinvar", &["read:genome"]),
];

/// Every source kind, one scope at a time: a viewer holding only that scope (and read:summary) is
/// shown a source exactly when the scope reads what the kind brings in. read:summary alone shows
/// none, and so do the scopes no kind lists. A guardian of a child's profile manages it, so he is
/// shown every source of it whatever scopes he holds.
#[tokio::test]
async fn each_source_kind_shows_to_a_reader_of_its_own_scopes_only() -> R {
    let mut listed: Vec<&str> = KIND_SCOPES.iter().map(|(kind, _)| *kind).collect();
    let mut known = sources::SOURCE_KINDS.to_vec();
    listed.sort_unstable();
    known.sort_unstable();
    assert_eq!(listed, known, "a source kind is missing from the table");
    let env = Env::new(true).await?;
    let (mut kinds, mut lucia) = (Vec::new(), Vec::new());
    {
        let mut c = env.pool().acquire().await?;
        let h = env.fam().household_id;
        for (kind, _) in KIND_SCOPES {
            let id = source(
                &mut c,
                h,
                Some(env.profile("alex")),
                kind,
                &format!("{kind} de Alex"),
                Some(900),
            )
            .await?;
            kinds.push((id, kind));
            // a household holds one directory source (brands), which Alex's set has
            if kind != "brands" {
                let id = source(
                    &mut c,
                    h,
                    Some(env.profile("lucia")),
                    kind,
                    &format!("{kind} de Lucía"),
                    Some(900),
                )
                .await?;
                lucia.push((id, kind));
            }
        }
    }
    let shown_of = |sources: &[(Uuid, &'static str)], v: &Value| -> Vec<&'static str> {
        let ids = ids_of(v);
        sources
            .iter()
            .filter(|(id, _)| ids.contains(&id.to_string()))
            .map(|(_, kind)| *kind)
            .collect()
    };
    for scope in [
        "read:summary",
        "read:labs",
        "read:records",
        "read:wearables",
        "read:genome",
        "read:vitals",
        "read:sensitive",
        "read:documents_text",
    ] {
        sam_on_alex(&env, &["read:summary", scope]).await?;
        let mut sam = env.as_user(env.user("sam")).await?;
        let r = sam.get("/api/inboxes/health").await?;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
        let expected: Vec<&str> = KIND_SCOPES
            .iter()
            .filter(|(_, scopes)| scopes.contains(&scope))
            .map(|(kind, _)| *kind)
            .collect();
        assert_eq!(shown_of(&kinds, &r.json), expected, "a viewer with {scope}");
    }
    sqlx::query("update profile_grant set role = 'guardian', categories = $3 where user_id = $1 and profile_id = $2")
        .bind(env.user("sam"))
        .bind(env.profile("lucia"))
        .bind(vec!["read:summary".to_string()])
        .execute(env.pool())
        .await?;
    let mut sam = env.as_user(env.user("sam")).await?;
    let r = sam.get("/api/inboxes/health").await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    let every: Vec<&str> = lucia.iter().map(|(_, kind)| *kind).collect();
    assert_eq!(every.len(), KIND_SCOPES.len() - 1);
    assert_eq!(
        shown_of(&lucia, &r.json),
        every,
        "a guardian of Lucía with read:summary only"
    );
    Ok(())
}

/// C-M2: the registry (`GET /api/inboxes`) carries a source's settings: the mailbox host and user, the
/// token prefix, the last error, the held-mail count. It lists only the sources the person manages, as
/// `GET /api/inboxes/{id}` answers only for them. A reader of the data sees the source on the health
/// route, which sends none of that.
#[tokio::test]
async fn the_registry_lists_only_the_sources_the_person_manages() -> R {
    let env = Env::new(true).await?;
    let mail = {
        let mut c = env.pool().acquire().await?;
        let id = source(
            &mut c,
            env.fam().household_id,
            Some(env.profile("alex")),
            "email",
            "Correo de Alex",
            Some(300),
        )
        .await?;
        sqlx::query("update source set config = $2 where id = $1")
            .bind(id)
            .bind(serde_json::json!({"host": "imap.correo.example", "port": 993, "username": "alex.rivera@correo.example"}))
            .execute(&mut *c)
            .await?;
        id
    };
    let listed = |v: &Value| ids_of(v).contains(&mail.to_string());
    let mut alex = env.as_user(env.user("alex")).await?;
    let r = alex.get("/api/inboxes").await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    assert!(listed(&r.json), "the owner: {:?}", r.json);
    assert_eq!(row(&r.json, mail)["config"]["host"], "imap.correo.example");

    // sam reads alex's documents but does not manage the profile
    sam_on_alex(&env, &["read:summary", "read:labs"]).await?;
    let mut sam = env.as_user(env.user("sam")).await?;
    let health = sam.get("/api/inboxes/health").await?;
    assert!(listed(&health.json), "{:?}", health.json);
    assert!(row(&health.json, mail).get("config").is_none(), "{:?}", health.json);
    let r = sam.get("/api/inboxes").await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    assert!(!listed(&r.json), "a reader who does not manage: {:?}", r.json);
    let body = String::from_utf8_lossy(&r.bytes);
    assert!(
        !body.contains("imap.correo.example"),
        "the mailbox host reached a reader: {body}"
    );
    Ok(())
}

#[tokio::test]
async fn prometheus_format_for_alerting() -> R {
    let env = Env::new(true).await?;
    let ids = setup(&env).await?;
    let mut alex = env.as_user(env.user("alex")).await?;

    let r = alex.get("/api/inboxes/health?format=prometheus").await?;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("content-type"), "text/plain; version=0.0.4; charset=utf-8");
    let text = String::from_utf8(r.bytes)?;
    let labels = |id: Uuid, kind: &str| format!("{{source=\"{id}\",kind=\"{kind}\"}}");
    let ns = labels(ids.carmen_nightscout, "nightscout");
    let success: f64 = text
        .lines()
        .find_map(|l| l.strip_prefix(&format!("ownhealth_source_last_success_timestamp_seconds{ns} ")))
        .and_then(|v| v.parse().ok())
        .unwrap_or_default();
    let now = chrono::Utc::now().timestamp() as f64;
    assert!((now - success).abs() < 120.0, "last success {success}, now {now}");
    let email = labels(ids.alex_email, "email");
    for want in [
        format!("ownhealth_source_consecutive_failures{email} 2"),
        format!("ownhealth_source_stale{email} 1"),
        format!("ownhealth_source_last_success_timestamp_seconds{email} 0"),
        format!(
            "ownhealth_source_stale{} 0",
            labels(ids.lucia_hae, "health_auto_export")
        ),
    ] {
        assert!(text.lines().any(|l| l == want), "missing {want:?} in\n{text}");
    }
    // labels carry ids and kinds, never the user's words or the profile
    for private in [
        "Carmen",
        "Correo",
        "connection refused",
        &env.profile("alex").to_string(),
    ] {
        assert!(!text.contains(private), "{private} in\n{text}");
    }
    assert!(!text.contains(&ids.other_household.to_string()));

    let r = alex.get("/api/inboxes/health?format=xml").await?;
    assert_eq!((r.status, r.code()), (StatusCode::UNPROCESSABLE_ENTITY, "invalid"));
    Ok(())
}

/// A monitor holds only the source's own token: it reads that source and nothing else.
#[tokio::test]
async fn a_source_token_reads_its_own_health_only() -> R {
    let env = Env::new(true).await?;
    let ids = setup(&env).await?;
    let mut conn = env.pool().acquire().await?;
    let token = sources::issue_token(&mut conn, ids.alex_email).await?;
    let bearer = format!("Bearer {token}");
    let mut monitor = env.anon();
    let get = |path: String| (Method::GET, path);

    let (m, p) = get(format!("/api/inboxes/{}/health?format=prometheus", ids.alex_email));
    let r = monitor.send(m, &p, None, &[("authorization", &bearer)]).await?;
    assert_eq!(r.status, StatusCode::OK);
    let text = String::from_utf8(r.bytes)?;
    assert!(text.contains(&format!(
        "ownhealth_source_stale{{source=\"{}\",kind=\"email\"}} 1",
        ids.alex_email
    )));

    let (m, p) = get(format!("/api/inboxes/{}/health", ids.alex_email));
    let r = monitor.send(m, &p, None, &[("x-ownhealth-secret", &token)]).await?;
    assert_eq!(
        (r.status, r.json["id"].as_str()),
        (StatusCode::OK, Some(ids.alex_email.to_string().as_str()))
    );

    let (m, p) = get("/api/inboxes/health".into());
    let r = monitor.send(m, &p, None, &[("authorization", &bearer)]).await?;
    assert_eq!(ids_of(&r.json), vec![ids.alex_email.to_string()]);

    // another source of the same household: not found, so a token cannot probe ids
    let (m, p) = get(format!("/api/inboxes/{}/health", ids.carmen_nightscout));
    let r = monitor.send(m, &p, None, &[("authorization", &bearer)]).await?;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    let mut wrong = token.clone();
    wrong.pop();
    wrong.push(if token.ends_with('A') { 'B' } else { 'A' });
    let (m, p) = get(format!("/api/inboxes/{}/health", ids.alex_email));
    let r = monitor
        .send(m, &p, None, &[("authorization", &format!("Bearer {wrong}"))])
        .await?;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    sources::update_source(
        &mut conn,
        ids.alex_email,
        &SourcePatch {
            enabled: Some(false),
            ..SourcePatch::default()
        },
    )
    .await?;
    let (m, p) = get(format!("/api/inboxes/{}/health", ids.alex_email));
    let r = monitor.send(m, &p, None, &[("authorization", &bearer)]).await?;
    assert_eq!(
        r.status,
        StatusCode::UNAUTHORIZED,
        "a disabled source's token stops working"
    );
    Ok(())
}

fn problem(code: &str) -> sources::RunProblem {
    sources::RunProblem {
        code: code.into(),
        message: "attachment 1: synthetic failure".into(),
    }
}

#[tokio::test]
async fn a_run_where_every_item_failed_is_not_a_successful_sync() -> R {
    let env = Env::new(true).await?;
    let mut c = env.pool().acquire().await?;
    let h = env.fam().household_id;
    let id = source(
        &mut c,
        h,
        Some(env.profile("alex")),
        "email",
        "Correo de Alex",
        Some(300),
    )
    .await?;
    let ok = sources::begin_run(&mut c, id, "poll", None).await?;
    let cursor = serde_json::json!({"uid": 7});
    let first = RunOutcome {
        cursor: Some(cursor.clone()),
        ..RunOutcome::default()
    };
    sources::finish_run(&mut c, ok, Ok(&first)).await?;
    // the last success is long ago
    sqlx::query("update source set last_success_at = now() - interval '1 day' where id = $1")
        .bind(id)
        .execute(&mut *c)
        .await?;
    let before: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select last_success_at from source where id = $1")
            .bind(id)
            .fetch_one(&mut *c)
            .await?;
    let failed = RunOutcome {
        seen: 2,
        problems: vec![problem("message_failed"), problem("message_failed")],
        cursor: Some(serde_json::json!({"uid": 9})),
        ..RunOutcome::default()
    };
    for n in 1..=2 {
        let run = sources::begin_run(&mut c, id, "poll", None).await?;
        sources::finish_run(&mut c, run, Ok(&failed)).await?;
        let status: String = sqlx::query_scalar("select status from source_run where id = $1")
            .bind(run)
            .fetch_one(&mut *c)
            .await?;
        assert_eq!(status, "error");
        let s = sources::get_source(&mut c, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("gone"))?;
        assert_eq!(s.consecutive_failures, n);
        assert_eq!(s.last_success_at, before);
        assert_eq!(s.cursor, Some(cursor.clone()), "the failed items are retried");
        assert_eq!(
            s.last_error.as_deref(),
            Some("message_failed: attachment 1: synthetic failure")
        );
    }
    let health = sources::source_health(&mut c, h).await?;
    let me = health
        .iter()
        .find(|x| x.id == id)
        .ok_or_else(|| anyhow::anyhow!("no row"))?;
    assert!(me.stale, "{me:?}");
    assert_eq!(
        (me.last_run_status.as_deref(), me.last_run_problems),
        (Some("error"), 2)
    );

    // something arrived: partial, a success, but the last run says so
    let mixed = RunOutcome {
        seen: 3,
        new: 1,
        problems: vec![problem("attachment_skipped")],
        ..RunOutcome::default()
    };
    let run = sources::begin_run(&mut c, id, "poll", None).await?;
    sources::finish_run(&mut c, run, Ok(&mixed)).await?;
    let s = sources::get_source(&mut c, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("gone"))?;
    assert_eq!(s.consecutive_failures, 0);
    assert!(s.last_success_at > before);
    drop(c);

    let mut alex = env.as_user(env.user("alex")).await?;
    let r = alex.get("/api/inboxes").await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    let row = row(&r.json, id);
    assert_eq!(row["last_run_status"], "partial");
    assert_eq!(row["last_run_problems"], 1);
    assert_eq!(
        row["last_run_problem"],
        "attachment_skipped: attachment 1: synthetic failure"
    );
    let r = alex.get("/api/inboxes/health?format=prometheus").await?;
    let text = String::from_utf8(r.bytes)?;
    assert!(
        text.contains(&format!(
            "ownhealth_source_last_run_ok{{source=\"{id}\",kind=\"email\"}} 0"
        )),
        "{text}"
    );
    Ok(())
}

/// The `read` rows `tool` wrote for `actor` (None: no actor), as (profile, source ids), sorted.
async fn health_reads(env: &Env, tool: &str, actor: Option<Uuid>) -> anyhow::Result<Vec<(Option<Uuid>, Vec<String>)>> {
    let mut rows: Vec<(Option<Uuid>, Vec<String>)> = sqlx::query_as(
        "select profile_id, resource_ids from audit_event where action = 'read' and tool = $1 \
         and actor_id is not distinct from $2",
    )
    .bind(tool)
    .bind(actor)
    .fetch_all(env.pool())
    .await?;
    rows.sort();
    Ok(rows)
}

/// L-AUDITRULE (a): the health list tells a person which sources a profile has, and a source's kind
/// names a device, so a response that shows a source writes one read row per profile it shows (and one
/// with no profile for the household's own sources). The single row writes one for its profile. A
/// source's own token, the Prometheus scrape path, writes none.
#[tokio::test]
async fn a_health_read_that_shows_a_source_writes_one_read_row_per_profile() -> R {
    let env = Env::new(true).await?;
    let ids = setup(&env).await?;
    let alex = env.user("alex");
    let mut c = env.as_user(alex).await?;
    let r = c.get("/api/inboxes/health").await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    let mut expected = vec![
        (None, vec![ids.folder.to_string()]),
        (Some(env.profile("alex")), vec![ids.alex_email.to_string()]),
        (Some(env.profile("carmen")), vec![ids.carmen_nightscout.to_string()]),
        (Some(env.profile("lucia")), vec![ids.lucia_hae.to_string()]),
    ];
    expected.sort();
    assert_eq!(health_reads(&env, "GET /api/inboxes/health", Some(alex)).await?, expected);
    let r = c.get(&format!("/api/inboxes/{}/health", ids.alex_email)).await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    assert_eq!(
        health_reads(&env, "GET /api/inboxes/{id}/health", Some(alex)).await?,
        [(Some(env.profile("alex")), vec![ids.alex_email.to_string()])]
    );
    // a source's own token reads its own health and writes nothing
    let mut conn = env.pool().acquire().await?;
    let token = sources::issue_token(&mut conn, ids.alex_email).await?;
    let before: i64 = sqlx::query_scalar("select count(*) from audit_event")
        .fetch_one(env.pool())
        .await?;
    let mut monitor = env.anon();
    for path in [
        "/api/inboxes/health?format=prometheus".to_string(),
        format!("/api/inboxes/{}/health?format=prometheus", ids.alex_email),
    ] {
        let r = monitor
            .send(Method::GET, &path, None, &[("authorization", &format!("Bearer {token}"))])
            .await?;
        assert_eq!(r.status, StatusCode::OK, "{path}");
    }
    let after: i64 = sqlx::query_scalar("select count(*) from audit_event")
        .fetch_one(env.pool())
        .await?;
    assert_eq!(after, before, "the scrape path writes no row");
    Ok(())
}
