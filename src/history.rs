//! Local history of everything the statusline sees, for querying later.
//!
//! Each render records a row in an SQLite database (default
//! `~/.local/share/claude-statusline/history.db`), but only when something changed
//! since the session's last row (or after [`HEARTBEAT`] of nothing changing, so idle
//! time is visible). Claude Code re-runs the statusline every few hundred
//! milliseconds while active, so this collapses thousands of identical renders into
//! roughly one row per turn. The per-model weekly limits arrive on their own cadence
//! from the `--refresh-usage` child and land in a separate table.
//!
//! Recording is best-effort: a locked database, an unwritable directory or any
//! other error is ignored so the render is never delayed or broken. Multiple
//! concurrent Claude Code sessions share the file; WAL mode plus a short busy
//! timeout serialises their writes and a sample dropped under contention is harmless.

use crate::usage::ScopedLimit;
use crate::{GitStatus, now_secs, xdg_dir};
use rusqlite::{Connection, OptionalExtension, params};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Applied in order, each in its own transaction that also bumps `user_version`,
/// so a new step is a new entry here and never an edit to an earlier one.
const MIGRATIONS: &[&str] = &[SCHEMA];
/// A sample is written even when nothing changed once this long has passed.
const HEARTBEAT: i64 = 5 * 60;
/// How often `sessions.last_seen` is bumped when nothing else changed.
const TOUCH_INTERVAL: i64 = 60;
/// How long to wait for another session's write before giving up on this sample.
const BUSY_TIMEOUT: Duration = Duration::from_millis(50);

const SCHEMA: &str = "
CREATE TABLE sessions (
    session_id  TEXT PRIMARY KEY,
    first_seen  INTEGER NOT NULL,
    last_seen   INTEGER NOT NULL,
    last_sample INTEGER NOT NULL,
    cwd         TEXT,
    worktree    TEXT,
    cc_version  TEXT,
    last_fp     INTEGER NOT NULL
);
CREATE TABLE samples (
    ts               INTEGER NOT NULL,
    session_id       TEXT NOT NULL,
    cwd              TEXT,
    model            TEXT,
    model_id         TEXT,
    ctx_pct          REAL,
    input_tokens     INTEGER,
    output_tokens    INTEGER,
    ctx_size         INTEGER,
    cost_usd         REAL,
    duration_ms      INTEGER,
    api_ms           INTEGER,
    cc_lines_added   INTEGER,
    cc_lines_removed INTEGER,
    diff_added       INTEGER,
    diff_removed     INTEGER,
    branch           TEXT,
    staged           INTEGER,
    modified         INTEGER,
    deleted          INTEGER,
    untracked        INTEGER,
    conflicted       INTEGER,
    stashes          INTEGER,
    ahead            INTEGER,
    behind           INTEGER,
    five_hour_pct    REAL,
    five_hour_resets INTEGER,
    seven_day_pct    REAL,
    seven_day_resets INTEGER
);
CREATE INDEX samples_session ON samples(session_id, ts);
CREATE INDEX samples_ts ON samples(ts);
CREATE TABLE usage_limits (
    ts        INTEGER NOT NULL,
    model     TEXT NOT NULL,
    used_pct  REAL NOT NULL,
    resets_at INTEGER,
    PRIMARY KEY (ts, model)
);
";

/// Everything recorded about one render.
#[derive(Debug, Default)]
pub struct Sample {
    pub session_id: String,
    pub cwd: String,
    pub worktree: Option<String>,
    pub cc_version: Option<String>,
    pub model: String,
    pub model_id: Option<String>,
    pub ctx_pct: Option<f64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub ctx_size: Option<i64>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<i64>,
    pub api_ms: Option<i64>,
    pub cc_lines_added: Option<i64>,
    pub cc_lines_removed: Option<i64>,
    pub git: Option<GitStatus>,
    pub five_hour_pct: Option<f64>,
    pub five_hour_resets: Option<i64>,
    pub seven_day_pct: Option<f64>,
    pub seven_day_resets: Option<i64>,
}

impl Sample {
    /// Hash of the fields that describe session state, so two renders with the
    /// same state compare equal regardless of when they happened. The exhaustive
    /// destructure makes adding a field a compile error until it is placed either
    /// in the hash or, like `duration_ms` (wall-clock time that ticks on every
    /// render and would defeat change detection), explicitly outside it.
    fn fingerprint(&self) -> i64 {
        let Sample {
            session_id: _,
            worktree: _,
            cc_version: _,
            duration_ms: _,
            cwd,
            model,
            model_id,
            ctx_pct,
            input_tokens,
            output_tokens,
            ctx_size,
            cost_usd,
            api_ms,
            cc_lines_added,
            cc_lines_removed,
            git,
            five_hour_pct,
            five_hour_resets,
            seven_day_pct,
            seven_day_resets,
        } = self;
        let mut h = DefaultHasher::new();
        (
            cwd,
            model,
            model_id,
            input_tokens,
            output_tokens,
            ctx_size,
            api_ms,
        )
            .hash(&mut h);
        (cc_lines_added, cc_lines_removed, git).hash(&mut h);
        (five_hour_resets, seven_day_resets).hash(&mut h);
        for pct in [ctx_pct, cost_usd, five_hour_pct, seven_day_pct] {
            pct.map(f64::to_bits).hash(&mut h);
        }
        h.finish() as i64
    }
}

pub fn db_path() -> Option<PathBuf> {
    xdg_dir("XDG_DATA_HOME", ".local/share").map(|d| d.join("history.db"))
}

fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|_| rusqlite::Error::InvalidPath(dir.to_path_buf()))?;
    }
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    init_schema(&mut conn)?;
    Ok(conn)
}

/// Applies any [`MIGRATIONS`] the database has not seen. Each step commits together
/// with its `user_version` bump, so an interrupted step is retried whole rather
/// than half-applied and then refused ("duplicate column") forever after.
fn init_schema(conn: &mut Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let version = usize::try_from(version).unwrap_or(0);
    if version == 0 {
        // Persistent in the file, so only set when creating it.
        conn.pragma_update(None, "journal_mode", "WAL")?;
    }
    for (i, step) in MIGRATIONS.iter().enumerate().skip(version) {
        let tx = conn.transaction()?;
        tx.execute_batch(step)?;
        tx.pragma_update(None, "user_version", i as i64 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

/// Records a render. Errors are swallowed; see the module docs.
pub fn record(sample: &Sample) {
    let Some(path) = db_path() else {
        return;
    };
    let _ = open(&path).and_then(|mut conn| record_in(&mut conn, sample, now_secs()));
}

/// Returns whether a `samples` row was written.
fn record_in(conn: &mut Connection, sample: &Sample, now: i64) -> rusqlite::Result<bool> {
    let fp = sample.fingerprint();
    let tx = conn.transaction()?;
    let last: Option<(i64, i64, i64)> = tx
        .query_row(
            "SELECT last_seen, last_sample, last_fp FROM sessions WHERE session_id = ?1",
            [&sample.session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (changed, last_seen, last_sample) = match last {
        None => (true, now, now),
        Some((seen, ts, prev_fp)) => (prev_fp != fp || now - ts >= HEARTBEAT, seen, ts),
    };
    if !changed && now - last_seen < TOUCH_INTERVAL {
        return Ok(false);
    }
    let last_sample = if changed { now } else { last_sample };
    if changed {
        let git = sample.git.as_ref();
        tx.execute(
            "INSERT INTO samples (ts, session_id, cwd, model, model_id, ctx_pct, input_tokens,
                output_tokens, ctx_size, cost_usd, duration_ms, api_ms, cc_lines_added,
                cc_lines_removed, diff_added, diff_removed, branch, staged, modified, deleted,
                untracked, conflicted, stashes, ahead, behind, five_hour_pct, five_hour_resets,
                seven_day_pct, seven_day_resets)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29)",
            params![
                now,
                sample.session_id,
                sample.cwd,
                sample.model,
                sample.model_id,
                sample.ctx_pct,
                sample.input_tokens,
                sample.output_tokens,
                sample.ctx_size,
                sample.cost_usd,
                sample.duration_ms,
                sample.api_ms,
                sample.cc_lines_added,
                sample.cc_lines_removed,
                git.map(|g| g.diff_added),
                git.map(|g| g.diff_removed),
                git.map(|g| &g.branch),
                git.map(|g| g.staged),
                git.map(|g| g.modified),
                git.map(|g| g.deleted),
                git.map(|g| g.untracked),
                git.map(|g| g.conflicted),
                git.map(|g| g.stashes),
                git.map(|g| g.ahead),
                git.map(|g| g.behind),
                sample.five_hour_pct,
                sample.five_hour_resets,
                sample.seven_day_pct,
                sample.seven_day_resets,
            ],
        )?;
    }
    tx.execute(
        "INSERT INTO sessions
            (session_id, first_seen, last_seen, last_sample, cwd, worktree, cc_version, last_fp)
         VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(session_id) DO UPDATE SET
            last_seen = excluded.last_seen,
            last_sample = excluded.last_sample,
            cwd = excluded.cwd,
            worktree = excluded.worktree,
            cc_version = excluded.cc_version,
            last_fp = excluded.last_fp",
        params![
            sample.session_id,
            now,
            last_sample,
            sample.cwd,
            sample.worktree,
            sample.cc_version,
            fp
        ],
    )?;
    tx.commit()?;
    Ok(changed)
}

/// Records a successful usage API fetch. Called from the `--refresh-usage` child.
pub fn record_limits(limits: &[ScopedLimit]) {
    let Some(path) = db_path() else {
        return;
    };
    let _ = open(&path).and_then(|mut conn| record_limits_in(&mut conn, limits, now_secs()));
}

fn record_limits_in(
    conn: &mut Connection,
    limits: &[ScopedLimit],
    now: i64,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for l in limits {
        tx.execute(
            "INSERT OR REPLACE INTO usage_limits (ts, model, used_pct, resets_at) VALUES (?1, ?2, ?3, ?4)",
            params![now, l.name, l.used_percentage, l.resets_at],
        )?;
    }
    tx.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        init_schema(&mut c).unwrap();
        c
    }

    fn count(c: &Connection, table: &str) -> i64 {
        c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    fn sample() -> Sample {
        Sample {
            session_id: "s1".into(),
            cwd: "/tmp/proj".into(),
            model: "Fable".into(),
            ctx_pct: Some(12.5),
            cost_usd: Some(0.42),
            git: Some(GitStatus {
                branch: "main".into(),
                modified: 2,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn unchanged_renders_collapse_into_one_row() {
        let mut c = conn();
        assert!(record_in(&mut c, &sample(), 1000).unwrap());
        assert!(!record_in(&mut c, &sample(), 1001).unwrap());
        assert!(!record_in(&mut c, &sample(), 1002).unwrap());
        assert_eq!(count(&c, "samples"), 1);
        assert_eq!(count(&c, "sessions"), 1);
    }

    #[test]
    fn changed_state_writes_a_row() {
        let mut c = conn();
        record_in(&mut c, &sample(), 1000).unwrap();
        let mut s = sample();
        s.cost_usd = Some(0.50);
        assert!(record_in(&mut c, &s, 1001).unwrap());
        let mut s = sample();
        s.git.as_mut().unwrap().modified = 3;
        assert!(record_in(&mut c, &s, 1002).unwrap());
        let mut s = sample();
        s.cwd = "/tmp/other".into();
        assert!(record_in(&mut c, &s, 1003).unwrap());
        assert_eq!(count(&c, "samples"), 4);
    }

    #[test]
    fn session_duration_alone_does_not_write_a_row() {
        let mut c = conn();
        let mut s = sample();
        s.duration_ms = Some(1000);
        record_in(&mut c, &s, 1000).unwrap();
        s.duration_ms = Some(3000);
        assert!(!record_in(&mut c, &s, 1002).unwrap());
    }

    #[test]
    fn heartbeat_writes_a_row_when_idle() {
        let mut c = conn();
        record_in(&mut c, &sample(), 1000).unwrap();
        assert!(!record_in(&mut c, &sample(), 1000 + HEARTBEAT - 1).unwrap());
        assert!(record_in(&mut c, &sample(), 1000 + HEARTBEAT).unwrap());
    }

    #[test]
    fn session_last_seen_is_touched_after_interval() {
        let mut c = conn();
        record_in(&mut c, &sample(), 1000).unwrap();
        record_in(&mut c, &sample(), 1000 + TOUCH_INTERVAL - 1).unwrap();
        let seen = |c: &Connection| -> i64 {
            c.query_row("SELECT last_seen FROM sessions", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(seen(&c), 1000);
        record_in(&mut c, &sample(), 1000 + TOUCH_INTERVAL).unwrap();
        assert_eq!(seen(&c), 1000 + TOUCH_INTERVAL);
        let first: i64 = c
            .query_row("SELECT first_seen FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(first, 1000);
    }

    #[test]
    fn sessions_are_independent() {
        let mut c = conn();
        record_in(&mut c, &sample(), 1000).unwrap();
        let mut s = sample();
        s.session_id = "s2".into();
        assert!(record_in(&mut c, &s, 1000).unwrap());
        assert_eq!(count(&c, "sessions"), 2);
    }

    #[test]
    fn git_and_cwd_columns_round_trip() {
        let mut c = conn();
        record_in(&mut c, &sample(), 1000).unwrap();
        let (cwd, branch, modified): (String, String, i64) = c
            .query_row("SELECT cwd, branch, modified FROM samples", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(cwd, "/tmp/proj");
        assert_eq!(branch, "main");
        assert_eq!(modified, 2);
    }

    #[test]
    fn limits_are_recorded_per_model() {
        let mut c = conn();
        let limits = vec![
            ScopedLimit {
                name: "Fable".into(),
                used_percentage: 7.0,
                resets_at: Some(2000),
            },
            ScopedLimit {
                name: "Opus".into(),
                used_percentage: 1.0,
                resets_at: None,
            },
        ];
        record_limits_in(&mut c, &limits, 1000).unwrap();
        assert_eq!(count(&c, "usage_limits"), 2);
    }

    #[test]
    fn schema_init_is_idempotent() {
        let mut c = conn();
        init_schema(&mut c).unwrap();
        let v: i64 = c
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
    }

    #[test]
    fn open_creates_directory_and_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("nested/history.db");
        let mut c = open(&path).unwrap();
        assert!(record_in(&mut c, &sample(), 1000).unwrap());
        assert!(path.exists());
    }
}
