//! Serves the claude-statusline history report over HTTP.
//!
//! `GET /` renders the report: `template.html` with the database contents embedded
//! as JSON (the SQL aliases below are the field names the template expects).
//! `GET /data.json` returns just the JSON and `GET /healthz` checks the database
//! opens. The database is opened read-only per request, so the page always
//! reflects what the statusline has recorded. Both `/` and `/data.json` take
//! `?days=N` (default 30, `all` for everything) to bound the samples returned.

use axum::{
    Router,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The report page. It is a body fragment (no doctype/head/body) so the same file
/// can be published as a Claude artifact, which supplies its own skeleton; we do
/// the same here.
const TEMPLATE: &str = include_str!("../template.html");
const HEAD: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1,viewport-fit=cover\">\
<style>:root{color-scheme:light dark}body{margin:0}</style></head><body>";
const FOOT: &str = "</body></html>";

const DEFAULT_DAYS: f64 = 30.0;

const SAMPLES: &str =
    "SELECT ts, substr(session_id,1,8) sid, cwd, model_id, model, ctx_pct, input_tokens,
    output_tokens, cost_usd, duration_ms, api_ms, cc_lines_added cca, cc_lines_removed ccr,
    diff_added da, diff_removed dr, branch, five_hour_pct h5, five_hour_resets h5r,
    seven_day_pct d7, seven_day_resets d7r FROM samples WHERE ts >= ?1 ORDER BY ts";
const LIMITS: &str =
    "SELECT ts, model, used_pct, resets_at FROM usage_limits WHERE ts >= ?1 ORDER BY ts";
const SESSIONS: &str = "SELECT substr(session_id,1,8) sid, first_seen, last_seen, cwd, cc_version
    FROM sessions WHERE last_seen >= ?1 ORDER BY first_seen";

fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_millis(500))?;
    Ok(conn)
}

/// Runs `sql` and returns its rows as JSON objects keyed by column name.
fn query(conn: &Connection, sql: &str, since: i64) -> rusqlite::Result<Value> {
    let mut stmt = conn.prepare(sql)?;
    let names: Vec<String> = stmt.column_names().iter().map(|n| n.to_string()).collect();
    let rows = stmt
        .query_map([since], |row| {
            let mut obj = Map::with_capacity(names.len());
            for (i, name) in names.iter().enumerate() {
                let v = match row.get_ref(i)? {
                    ValueRef::Null | ValueRef::Blob(_) => Value::Null,
                    ValueRef::Integer(n) => json!(n),
                    ValueRef::Real(f) => json!(f),
                    ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                };
                obj.insert(name.clone(), v);
            }
            Ok(Value::Object(obj))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Value::Array(rows))
}

fn load(conn: &Connection, since: i64) -> rusqlite::Result<Value> {
    Ok(json!({
        "samples": query(conn, SAMPLES, since)?,
        "limits": query(conn, LIMITS, since)?,
        "sessions": query(conn, SESSIONS, since)?,
    }))
}

/// Opens the database on a blocking thread and runs `f` against it.
async fn with_db<T: Send + 'static>(
    db: PathBuf,
    f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
) -> Result<T, Response> {
    tokio::task::spawn_blocking(move || open(&db).and_then(|c| f(&c)))
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("task failed: {e}"),
            )
                .into_response()
        })?
        .map_err(|e| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("database error: {e}"),
            )
                .into_response()
        })
}

/// Unix timestamp the `?days=` window starts at.
fn since(params: &HashMap<String, String>) -> i64 {
    let days = match params.get("days").map(String::as_str) {
        Some("all") => return 0,
        Some(d) => d.parse().unwrap_or(DEFAULT_DAYS),
        None => DEFAULT_DAYS,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    (now - days * 86_400.0) as i64
}

async fn index(
    State(db): State<PathBuf>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, Response> {
    let s = since(&params);
    let v = with_db(db, move |c| load(c, s)).await?;
    // No `<` may appear in inline JSON or a value could end the script block;
    // `<` is the same string once parsed.
    let page = TEMPLATE.replacen("__DATA__", &v.to_string().replace('<', "\\u003c"), 1);
    Ok(Html(format!("{HEAD}{page}{FOOT}")))
}

async fn data_json(
    State(db): State<PathBuf>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, Response> {
    let s = since(&params);
    let v = with_db(db, move |c| load(c, s)).await?;
    Ok(([(header::CONTENT_TYPE, "application/json")], v.to_string()))
}

async fn healthz(State(db): State<PathBuf>) -> Result<impl IntoResponse, Response> {
    let n = with_db(db, |c| {
        c.query_row("SELECT count(*) FROM samples", [], |r| r.get::<_, i64>(0))
    })
    .await?;
    Ok(format!("ok {n} samples\n"))
}

async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
}

/// `$DB_PATH`, else the statusline's own default location.
fn db_path() -> PathBuf {
    if let Some(p) = std::env::var_os("DB_PATH") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_default();
    base.join("claude-statusline/history.db")
}

#[tokio::main]
async fn main() {
    let db = db_path();
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let app = Router::new()
        .route("/", get(index))
        .route("/data.json", get(data_json))
        .route("/healthz", get(healthz))
        .with_state(db.clone());
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .unwrap_or_else(|e| panic!("bind port {port}: {e}"));
    eprintln!("serving {} on http://0.0.0.0:{port}", db.display());
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
        .expect("server");
}
