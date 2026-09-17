//! Per-model weekly limits (e.g. Fable) that Claude Code does not include in its
//! statusline JSON.
//!
//! Claude Code's `/usage` command fetches `GET /api/oauth/usage` with the user's
//! OAuth token. We do the same, but never on the statusline's critical path: the
//! result is cached on disk for [`TTL`], and when the cache is stale the statusline
//! spawns a detached copy of itself (`--refresh-usage`) to refresh it while the
//! current render proceeds with whatever the cache already holds.

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::{now_secs, xdg_dir};

/// How long a cached response is considered fresh.
pub const TTL: Duration = Duration::from_secs(5 * 60);

/// A refresh lock older than this is assumed abandoned (crashed child) and ignored.
const LOCK_STALE_AFTER: Duration = Duration::from_secs(60);

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopedLimit {
    /// Server-supplied label for the bucket, e.g. "Fable".
    pub name: String,
    pub used_percentage: f64,
    /// Unix epoch seconds when the window resets.
    pub resets_at: Option<i64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    /// Unix epoch seconds of the last fetch attempt, successful or not.
    fetched_at: i64,
    #[serde(default)]
    limits: Vec<ScopedLimit>,
}

fn cache_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CACHE_HOME", ".cache")
}

fn read_cache(path: &Path) -> Option<Cache> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_cache(path: &Path, cache: &Cache) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec(cache)?)?;
    fs::rename(tmp, path)
}

fn is_fresh(cache: &Cache, now: i64) -> bool {
    now - cache.fetched_at < TTL.as_secs() as i64
}

/// Returns the cached per-model limits, kicking off a background refresh when the
/// cache is missing or older than [`TTL`]. Never blocks on the network.
///
/// `record_history` is forwarded to the refresh child so it knows whether to log
/// what it fetches.
pub fn scoped_limits(record_history: bool) -> Vec<ScopedLimit> {
    let Some(dir) = cache_dir() else {
        return Vec::new();
    };
    let cache = read_cache(&dir.join("usage.json")).unwrap_or_default();
    if !is_fresh(&cache, now_secs()) {
        spawn_refresh(&dir, record_history);
    }
    cache.limits
}

/// Guard that holds the refresh lock file and removes it on drop.
struct RefreshLock(PathBuf);

impl RefreshLock {
    fn acquire(dir: &Path) -> Option<Self> {
        fs::create_dir_all(dir).ok()?;
        let path = lock_path(dir);
        if !lock_is_live(&path) {
            let _ = fs::remove_file(&path);
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .ok()?;
        Some(Self(path))
    }
}

fn lock_path(dir: &Path) -> PathBuf {
    dir.join("usage.lock")
}

/// True if a lock file exists and is recent enough to belong to a running refresh.
fn lock_is_live(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| m.elapsed().ok())
        .is_some_and(|age| age <= LOCK_STALE_AFTER)
}

impl Drop for RefreshLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn spawn_refresh(dir: &Path, record_history: bool) {
    // Another render (possibly from a different session) already has a refresh
    // in flight; the child takes the lock itself, this is just to avoid a pile-up.
    if lock_is_live(&lock_path(dir)) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = Command::new(exe);
    cmd.arg("--refresh-usage");
    if !record_history {
        cmd.arg("--no-history");
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Detach into its own process group so it outlives this render.
        cmd.process_group(0);
    }
    let _ = cmd.spawn();
}

/// Fetches the usage endpoint and rewrites the cache, returning what was fetched.
/// Run in the detached child.
///
/// On any failure the cache's timestamp is still bumped (keeping the previous
/// limits) so a persistent error is retried once per [`TTL`] rather than on every
/// render.
pub fn refresh() -> Option<Vec<ScopedLimit>> {
    let dir = cache_dir()?;
    let _lock = RefreshLock::acquire(&dir)?;
    let path = dir.join("usage.json");
    let previous = read_cache(&path).unwrap_or_default();
    let fetched = fetch_scoped_limits();
    let _ = write_cache(
        &path,
        &Cache {
            fetched_at: now_secs(),
            limits: fetched.clone().unwrap_or(previous.limits),
        },
    );
    fetched
}

fn fetch_scoped_limits() -> Option<Vec<ScopedLimit>> {
    let token = access_token()?;
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .new_agent();
    let body = agent
        .get(USAGE_URL)
        .header("Authorization", &format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("Content-Type", "application/json")
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    parse_scoped_limits(&body)
}

#[derive(Deserialize)]
struct UsageResponse {
    #[serde(default)]
    limits: Vec<LimitEntry>,
}

#[derive(Deserialize)]
struct LimitEntry {
    kind: String,
    percent: Option<f64>,
    resets_at: Option<String>,
    scope: Option<Scope>,
}

#[derive(Deserialize)]
struct Scope {
    model: Option<ScopeModel>,
}

#[derive(Deserialize)]
struct ScopeModel {
    display_name: Option<String>,
}

fn parse_scoped_limits(body: &str) -> Option<Vec<ScopedLimit>> {
    let resp: UsageResponse = serde_json::from_str(body).ok()?;
    Some(
        resp.limits
            .into_iter()
            .filter(|l| l.kind == "weekly_scoped")
            .filter_map(|l| {
                let name = l.scope?.model?.display_name?;
                Some(ScopedLimit {
                    name,
                    used_percentage: l.percent?,
                    resets_at: l
                        .resets_at
                        .as_deref()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|t| t.timestamp()),
                })
            })
            .collect(),
    )
}

#[derive(Deserialize)]
struct Credentials {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OAuth>,
}

#[derive(Deserialize)]
struct OAuth {
    #[serde(rename = "accessToken")]
    access_token: String,
    /// Unix epoch milliseconds.
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
}

/// Returns the OAuth access token Claude Code is currently using, or `None` if it
/// is missing or expired. Expired tokens are never refreshed here: Claude Code owns
/// that token and rotates it itself.
fn access_token() -> Option<String> {
    let raw = read_credentials()?;
    token_from_credentials(&raw, now_secs() * 1000)
}

fn token_from_credentials(raw: &str, now_ms: i64) -> Option<String> {
    let creds: Credentials = serde_json::from_str(raw).ok()?;
    let oauth = creds.claude_ai_oauth?;
    if oauth.expires_at.is_some_and(|exp| exp <= now_ms) {
        return None;
    }
    Some(oauth.access_token)
}

fn read_credentials() -> Option<String> {
    keychain_credentials().or_else(file_credentials)
}

/// Reads the credentials JSON Claude Code stores in the macOS Keychain.
///
/// This deliberately shells out to `security` rather than using the Keychain API
/// directly: Claude Code writes the item via `security`, so that tool is on the
/// item's access list permanently, whereas a grant to this binary is tied to its
/// (ad-hoc, per-build) code signature and is wiped whenever Claude Code recreates
/// the item on token refresh, causing a password prompt on every fetch.
#[cfg(target_os = "macos")]
fn keychain_credentials() -> Option<String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim_end().to_string())
}

#[cfg(not(target_os = "macos"))]
fn keychain_credentials() -> Option<String> {
    let _ = KEYCHAIN_SERVICE;
    None
}

fn file_credentials() -> Option<String> {
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))?;
    fs::read_to_string(config_dir.join(".credentials.json")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "five_hour": {"utilization": 16.0},
        "limits": [
            {"kind": "session", "group": "session", "percent": 16, "resets_at": "2026-09-01T23:10:00.218704+00:00", "scope": null},
            {"kind": "weekly_all", "group": "weekly", "percent": 3, "resets_at": "2026-09-05T11:00:00.218726+00:00", "scope": null},
            {"kind": "weekly_scoped", "group": "weekly", "percent": 6, "resets_at": "2026-09-05T11:00:00.218947+00:00",
             "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}}
        ]
    }"#;

    #[test]
    fn parses_only_model_scoped_weekly_limits() {
        let limits = parse_scoped_limits(SAMPLE).unwrap();
        assert_eq!(
            limits,
            vec![ScopedLimit {
                name: "Fable".into(),
                used_percentage: 6.0,
                resets_at: Some(1788606000),
            }]
        );
    }

    #[test]
    fn parses_response_without_limits_array() {
        assert_eq!(
            parse_scoped_limits(r#"{"five_hour": null}"#).unwrap(),
            vec![]
        );
        assert!(parse_scoped_limits("not json").is_none());
    }

    #[test]
    fn token_rejected_when_expired() {
        let raw = r#"{"claudeAiOauth": {"accessToken": "tok", "expiresAt": 1000}}"#;
        assert_eq!(token_from_credentials(raw, 999).as_deref(), Some("tok"));
        assert!(token_from_credentials(raw, 1000).is_none());
        assert!(token_from_credentials(r#"{"mcpOAuth": {}}"#, 0).is_none());
    }

    #[test]
    fn cache_freshness_uses_ttl() {
        let cache = Cache {
            fetched_at: 1000,
            limits: vec![],
        };
        assert!(is_fresh(&cache, 1000 + TTL.as_secs() as i64 - 1));
        assert!(!is_fresh(&cache, 1000 + TTL.as_secs() as i64));
        assert!(!is_fresh(&Cache::default(), 1000));
    }

    #[test]
    fn cache_round_trips_through_disk() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("nested").join("usage.json");
        let cache = Cache {
            fetched_at: 42,
            limits: parse_scoped_limits(SAMPLE).unwrap(),
        };
        write_cache(&path, &cache).unwrap();
        let back = read_cache(&path).unwrap();
        assert_eq!(back.fetched_at, 42);
        assert_eq!(back.limits, cache.limits);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn refresh_lock_is_exclusive_and_released_on_drop() {
        let tmp = tempfile::TempDir::new().unwrap();
        let lock = RefreshLock::acquire(tmp.path()).unwrap();
        assert!(RefreshLock::acquire(tmp.path()).is_none());
        drop(lock);
        assert!(RefreshLock::acquire(tmp.path()).is_some());
    }
}
