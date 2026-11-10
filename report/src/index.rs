//! An in-memory copy of the few `samples` columns that downsampling and per-request
//! API time need, kept across requests and topped up with only the rows added since
//! the last one. The statusline only ever inserts samples, so rows already read never
//! change; anything else (the file replaced, rows deleted) shows up as a count
//! mismatch and the index is rebuilt.

use rusqlite::Connection;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

struct Row {
    id: i64,
    ts: i64,
    api_ms: Option<i64>,
    input: Option<i64>,
    /// Interned, so equality is string equality without a string per row.
    branch: Option<u32>,
}

#[derive(Default)]
pub struct Index {
    /// Each session's rows ordered by (ts, rowid).
    sessions: HashMap<String, Vec<Row>>,
    branches: HashMap<String, u32>,
    last_id: i64,
    count: i64,
}

/// One request: the increase in a session's cumulative API time. `merged` is `None`
/// for a session's first request in the window, which has nothing to compare with.
struct Request {
    ts: i64,
    ms: i64,
    merged: Option<bool>,
}

pub struct Window {
    /// Row ids of the samples to send, in no particular order.
    pub keep: Vec<i64>,
    pub api: Value,
    pub api_pct: Value,
}

impl Index {
    /// Reads the rows added since the last call.
    pub fn refresh(&mut self, conn: &Connection) -> rusqlite::Result<()> {
        let seen: i64 = conn.query_row(
            "SELECT count(*) FROM samples WHERE rowid <= ?1",
            [self.last_id],
            |r| r.get(0),
        )?;
        if seen != self.count {
            *self = Index::default();
        }
        let mut stmt = conn.prepare(
            "SELECT rowid, session_id, ts, api_ms, input_tokens, branch
             FROM samples WHERE rowid > ?1 ORDER BY rowid",
        )?;
        // Collected before anything is applied, so a failed read leaves the index as it was.
        let new = stmt
            .query_map([self.last_id], |r| {
                Ok((
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(5)?,
                    (r.get(0)?, r.get(2)?, r.get(3)?, r.get(4)?),
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (sid, branch, (id, ts, api_ms, input)) in new {
            let next = self.branches.len() as u32;
            let branch = branch.map(|b| *self.branches.entry(b).or_insert(next));
            let rows = self.sessions.entry(sid).or_default();
            let row = Row {
                id,
                ts,
                api_ms,
                input,
                branch,
            };
            // Rows arrive in rowid order, which is time order unless a clock stepped back.
            let at = rows.partition_point(|r| (r.ts, r.id) <= (ts, id));
            rows.insert(at, row);
            self.last_id = id;
            self.count += 1;
        }
        Ok(())
    }

    /// The samples to send for the window starting at `since` and per-request API time.
    ///
    /// Keeps the last sample of each session per `bucket`-second bucket, plus the
    /// samples the page derives events from: each session's first in the window (spend
    /// is measured from it), any followed by a compaction (input tokens falling below
    /// 70%, so the peak can be marked) and any branch change. Cumulative columns (cost,
    /// tokens, lines) stay exact at the kept points; only intermediate steps are dropped.
    ///
    /// Claude Code's cumulative `total_api_duration_ms` moves once per completed API
    /// call, so each increase is a request's duration. Overlapping requests (parallel
    /// subagents) land as one increase longer than the wall time since the previous one
    /// finished; those are `merged` and left out of the latencies. Always from the raw
    /// samples, since downsampling would merge them too.
    pub fn window(&self, since: i64, bucket: i64) -> Window {
        let mut keep = Vec::new();
        let mut reqs = Vec::new();
        for rows in self.sessions.values() {
            let w = &rows[rows.partition_point(|r| r.ts < since)..];
            let mut prev_req = None;
            for (i, r) in w.iter().enumerate() {
                let prev = i.checked_sub(1).map(|j| &w[j]);
                let next = w.get(i + 1);
                let compacted = matches!((next.and_then(|n| n.input), r.input),
                    (Some(n), Some(c)) if (n as f64) < c as f64 * 0.7);
                if prev.is_none_or(|p| p.branch != r.branch)
                    || next.is_none_or(|n| n.ts / bucket != r.ts / bucket)
                    || compacted
                {
                    keep.push(r.id);
                }
                let ms = match (r.api_ms, prev.and_then(|p| p.api_ms)) {
                    (Some(c), Some(p)) if c > p => c - p,
                    _ => continue,
                };
                let merged = prev_req.map(|t: i64| ms > (r.ts - t) * 1000 + 1000);
                reqs.push(Request {
                    ts: r.ts,
                    ms,
                    merged,
                });
                prev_req = Some(r.ts);
            }
        }
        let (api, api_pct) = api_time(&reqs);
        Window { keep, api, api_pct }
    }
}

/// Single-request counts and total ms per hour and power-of-two second bucket (0 is
/// under 1 s, 7 is 64 s and over), plus the exact median and 90th percentile and how
/// many were left out. A session's first request counts as left out, like a merged one.
fn api_time(reqs: &[Request]) -> (Value, Value) {
    let mut single: Vec<i64> = reqs
        .iter()
        .filter(|r| r.merged == Some(false))
        .map(|r| r.ms)
        .collect();
    let mut hours: BTreeMap<(i64, u32), (i64, i64)> = BTreeMap::new();
    for r in reqs.iter().filter(|r| r.merged == Some(false)) {
        let b = if r.ms < 1000 {
            0
        } else {
            ((r.ms / 1000).ilog2() + 1).min(7)
        };
        let e = hours.entry((r.ts / 3600 * 3600, b)).or_default();
        e.0 += 1;
        e.1 += r.ms;
    }
    let api = hours
        .into_iter()
        .map(|((hour, b), (n, ms))| json!({"hour": hour, "b": b, "n": n, "ms": ms}))
        .collect();
    single.sort_unstable();
    let n = single.len();
    // 1-based ranks, as the page's numbers have always been computed.
    let at = |i: usize| single.get(i.wrapping_sub(1)).copied();
    let pct = json!({
        "n": (n > 0).then_some(n),
        "p50": at(n.div_ceil(2)),
        "p90": at((9 * n).div_ceil(10)),
        "merged": reqs.len() - n,
    });
    (Value::Array(api), pct)
}
