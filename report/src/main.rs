//! Serves the claude-statusline history report over HTTP.
//!
//! `GET /` renders the report: `template.html` with the database contents embedded
//! as JSON (the SQL aliases below are the field names the template expects).
//! `GET /data.json` returns just the JSON and `GET /healthz` checks the database
//! opens. The database is opened read-only per request, so the page always
//! reflects what the statusline has recorded; `index::Index` keeps what downsampling
//! needs in memory between requests and reads only the rows added since. Both `/` and `/data.json` take
//! `?days=N` (default 30, `all` for everything) to bound the samples returned, and
//! `?bucket=N` (seconds, default 60, `0` for raw) to downsample them. Per-request API
//! times are always derived from the raw samples.

use axum::{
    Router,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use index::Index;
use rusqlite::{
    Connection, OpenFlags,
    types::{Value as SqlValue, ValueRef},
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

mod index;

/// The report page. It is a body fragment (no doctype/head/body) so the same file
/// can be published as a Claude artifact, which supplies its own skeleton; we do
/// the same here.
const TEMPLATE: &str = include_str!("../template.html");
const HEAD: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1,viewport-fit=cover\">\
<style>:root{color-scheme:light dark}body{margin:0}</style></head><body>";
const FOOT: &str = "</body></html>";

const DEFAULT_DAYS: f64 = 30.0;
const DEFAULT_BUCKET: i64 = 60;

/// The samples `Index::window` kept, in full.
const SAMPLES: &str = "SELECT ts, substr(session_id,1,8) sid, cwd, model_id, model, ctx_pct,
    input_tokens, output_tokens, cost_usd, duration_ms, api_ms, cc_lines_added cca,
    cc_lines_removed ccr, diff_added da, diff_removed dr, branch, five_hour_pct h5,
    five_hour_resets h5r, seven_day_pct d7, seven_day_resets d7r, uncached_in ui,
    cache_write cw, cache_read cr
    FROM samples WHERE rowid IN rarray(?1)
    ORDER BY ts, rowid";
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

/// Appends to a buffer with every `<` written as `\u003c`. Inline JSON must not contain
/// `<` or a value could end the script block, and it is the same string once parsed.
/// Only JSON strings can contain one, so this is applied to strings alone.
struct NoLt<'a>(&'a mut Vec<u8>);

impl std::io::Write for NoLt<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for part in buf.split_inclusive(|&b| b == b'<') {
            match part.split_last() {
                Some((b'<', rest)) => {
                    self.0.extend_from_slice(rest);
                    self.0.extend_from_slice(b"\\u003c");
                }
                _ => self.0.extend_from_slice(part),
            }
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serialises `v` onto `out`; writing to a `Vec` cannot fail.
fn put(out: &mut Vec<u8>, v: &impl serde::Serialize) {
    serde_json::to_writer(NoLt(out), v).expect("serialise to a Vec");
}

/// Runs `sql` and appends its rows to `out` as a JSON array of objects keyed by column
/// name, written straight from the SQLite rows without building them in memory first.
fn query(
    out: &mut Vec<u8>,
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let keys: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|n| serde_json::to_string(n).expect("serialise a str"))
        .collect();
    let mut rows = stmt.query(params)?;
    out.push(b'[');
    let mut first = true;
    while let Some(row) = rows.next()? {
        if !std::mem::take(&mut first) {
            out.push(b',');
        }
        for (i, key) in keys.iter().enumerate() {
            out.push(if i == 0 { b'{' } else { b',' });
            out.extend_from_slice(key.as_bytes());
            out.push(b':');
            match row.get_ref(i)? {
                ValueRef::Null | ValueRef::Blob(_) => out.extend_from_slice(b"null"),
                ValueRef::Integer(n) => put(out, &n),
                // Non-finite values come out as `null`.
                ValueRef::Real(f) => put(out, &f),
                ValueRef::Text(t) => put(out, &String::from_utf8_lossy(t)),
            }
        }
        out.push(b'}');
    }
    out.push(b']');
    Ok(())
}

/// Appends the report's JSON to `out`.
fn load(
    out: &mut Vec<u8>,
    conn: &Connection,
    index: &Mutex<Index>,
    since: i64,
    bucket: i64,
) -> rusqlite::Result<()> {
    let w = {
        // A panic mid-refresh can only have left rows unapplied, which the next
        // refresh's count check catches, so a poisoned lock is still usable.
        let mut index = index.lock().unwrap_or_else(|e| e.into_inner());
        index.refresh(conn)?;
        index.window(since, bucket)
    };
    rusqlite::vtab::array::load_module(conn)?;
    let keep = Rc::new(w.keep.into_iter().map(SqlValue::from).collect::<Vec<_>>());
    out.extend_from_slice(b"{\"samples\":");
    query(out, conn, SAMPLES, [keep])?;
    out.extend_from_slice(b",\"limits\":");
    query(out, conn, LIMITS, [since])?;
    out.extend_from_slice(b",\"sessions\":");
    query(out, conn, SESSIONS, [since])?;
    out.extend_from_slice(b",\"api\":");
    put(out, &w.api);
    out.extend_from_slice(b",\"apiPct\":");
    put(out, &w.api_pct);
    out.push(b'}');
    Ok(())
}

#[derive(Clone)]
struct AppState {
    db: PathBuf,
    index: Arc<Mutex<Index>>,
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

/// Downsampling bucket in seconds from `?bucket=`; `0` is treated as 1 s, effectively raw.
fn bucket(params: &HashMap<String, String>) -> i64 {
    params
        .get("bucket")
        .and_then(|b| b.parse().ok())
        .unwrap_or(DEFAULT_BUCKET)
        .max(1)
}

async fn index(
    State(st): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, Response> {
    let (s, b) = (since(&params), bucket(&params));
    let page = with_db(st.db, move |c| {
        let (pre, post) = TEMPLATE
            .split_once("__DATA__")
            .expect("template has __DATA__");
        // Sized for a month of samples, so the buffer rarely has to grow and copy.
        let mut out = Vec::with_capacity(8 << 20);
        out.extend_from_slice(HEAD.as_bytes());
        out.extend_from_slice(pre.as_bytes());
        load(&mut out, c, &st.index, s, b)?;
        out.extend_from_slice(post.as_bytes());
        out.extend_from_slice(FOOT.as_bytes());
        Ok(out)
    })
    .await?;
    Ok(Html(page))
}

async fn data_json(
    State(st): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, Response> {
    let (s, b) = (since(&params), bucket(&params));
    let v = with_db(st.db, move |c| {
        let mut out = Vec::new();
        load(&mut out, c, &st.index, s, b)?;
        Ok(out)
    })
    .await?;
    Ok(([(header::CONTENT_TYPE, "application/json")], v))
}

async fn healthz(
    State(AppState { db, .. }): State<AppState>,
) -> Result<impl IntoResponse, Response> {
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
        .with_state(AppState {
            db: db.clone(),
            index: Arc::default(),
        });
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .unwrap_or_else(|e| panic!("bind port {port}: {e}"));
    eprintln!("serving {} on http://0.0.0.0:{port}", db.display());
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
        .expect("server");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_cannot_end_the_script_block() {
        let mut out = Vec::new();
        put(&mut out, &"a</script><b");
        let s = String::from_utf8(out).unwrap();
        assert!(!s.contains('<'), "{s}");
        assert_eq!(serde_json::from_str::<String>(&s).unwrap(), "a</script><b");
    }
}
