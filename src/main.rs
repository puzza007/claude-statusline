use chrono::Local;
use clap::Parser;
use colored::Colorize;
use git2::{Repository, Status, StatusOptions};
use serde::Deserialize;
use std::fmt::Write as _;
use std::path::PathBuf;

mod usage;

/// A fast, custom statusline for Claude Code.
///
/// Reads Claude Code's statusline JSON from stdin and outputs a formatted,
/// color-coded status line showing directory, git status, model, context
/// usage, rate limits, session cost, and lines changed.
///
/// Configure in ~/.claude/settings.json:
///
///   { "statusLine": { "type": "command", "command": "claude-statusline" } }
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Skip fetching per-model weekly limits (e.g. Fable) from the usage API.
    #[arg(long)]
    no_usage: bool,

    /// Refresh the cached usage API response and exit. Spawned in the background
    /// by the statusline itself; not meant to be run by hand.
    #[arg(long, hide = true)]
    refresh_usage: bool,
}

#[derive(Deserialize)]
struct Input {
    model: Model,
    workspace: Workspace,
    context_window: ContextWindow,
    cost: Cost,
    rate_limits: Option<RateLimits>,
}

#[derive(Deserialize)]
struct Model {
    display_name: String,
}

#[derive(Deserialize)]
struct Workspace {
    current_dir: String,
    /// Name of the linked git worktree, sent only when the session is inside one.
    git_worktree: Option<String>,
}

#[derive(Deserialize)]
struct ContextWindow {
    used_percentage: Option<f64>,
}

#[derive(Deserialize)]
struct Cost {
    total_cost_usd: Option<f64>,
}

#[derive(Deserialize)]
struct RateLimits {
    five_hour: Option<RateLimit>,
    seven_day: Option<RateLimit>,
}

#[derive(Deserialize)]
struct RateLimit {
    used_percentage: Option<f64>,
    resets_at: Option<i64>,
}

/// Unix epoch seconds.
pub(crate) fn now_secs() -> i64 {
    Local::now().timestamp()
}

/// `$VAR/claude-statusline`, or `$HOME/<fallback>/claude-statusline` when the XDG
/// variable is unset.
pub(crate) fn xdg_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    let base = std::env::var_os(var)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(fallback)))?;
    Some(base.join("claude-statusline"))
}

fn shorten_home(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// Trims the `.claude/worktrees/<name>` suffix Claude Code appends when it creates
/// a worktree, leaving the repo root. Worktrees placed elsewhere are left untouched.
fn strip_worktree_suffix<'a>(path: &'a str, name: &str) -> &'a str {
    path.strip_suffix(&format!("/.claude/worktrees/{name}"))
        .unwrap_or(path)
}

const STAGED: Status = Status::from_bits_truncate(
    Status::INDEX_NEW.bits()
        | Status::INDEX_MODIFIED.bits()
        | Status::INDEX_DELETED.bits()
        | Status::INDEX_RENAMED.bits()
        | Status::INDEX_TYPECHANGE.bits(),
);

const WT_MODIFIED: Status = Status::from_bits_truncate(
    Status::WT_MODIFIED.bits() | Status::WT_RENAMED.bits() | Status::WT_TYPECHANGE.bits(),
);

/// Git state of the working tree as the statusline counts it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub(crate) struct GitStatus {
    pub branch: String,
    pub staged: u32,
    pub modified: u32,
    pub deleted: u32,
    pub untracked: u32,
    pub conflicted: u32,
    pub stashes: u32,
    pub ahead: u32,
    pub behind: u32,
    /// Insertions and deletions in uncommitted changes (staged + unstaged) vs HEAD.
    pub diff_added: u32,
    pub diff_removed: u32,
}

fn git_status(dir: &str) -> Option<GitStatus> {
    let mut repo = Repository::discover(dir).ok()?;

    let (is_branch, branch) = {
        let head = repo.head().ok()?;
        let is_branch = head.is_branch();
        let branch = if is_branch {
            head.shorthand().unwrap_or("").to_string()
        } else {
            head.target()
                .map(|oid| oid.to_string()[..7].to_string())
                .unwrap_or_default()
        };
        (is_branch, branch)
    };

    if branch.is_empty() {
        return None;
    }

    let mut status = GitStatus {
        branch,
        ..Default::default()
    };

    let mut opts = StatusOptions::new();
    opts.include_untracked(true).exclude_submodules(true);
    if let Ok(statuses) = repo.statuses(Some(&mut opts)) {
        for entry in statuses.iter() {
            let s = entry.status();
            if s.contains(Status::CONFLICTED) {
                status.conflicted += 1;
            }
            if s.intersects(STAGED) {
                status.staged += 1;
            }
            if s.intersects(WT_MODIFIED) {
                status.modified += 1;
            }
            if s.intersects(Status::WT_DELETED) || s.intersects(Status::INDEX_DELETED) {
                status.deleted += 1;
            }
            if s.contains(Status::WT_NEW) {
                status.untracked += 1;
            }
        }
    }

    status.stashes = stash_count(&mut repo).unwrap_or(0);

    if let Some((added, removed)) = diff_lines(&repo) {
        status.diff_added = added as u32;
        status.diff_removed = removed as u32;
    }

    if is_branch {
        let upstream_ref = format!("refs/heads/{}", status.branch);
        if let Ok(local_oid) = repo.refname_to_id("HEAD")
            && let Ok(upstream_name) = repo.branch_upstream_name(&upstream_ref)
            && let Some(name) = upstream_name.as_str()
            && let Ok(upstream_oid) = repo.refname_to_id(name)
            && let Ok((ahead, behind)) = repo.graph_ahead_behind(local_oid, upstream_oid)
        {
            status.ahead = ahead as u32;
            status.behind = behind as u32;
        }
    }

    Some(status)
}

fn render_git(status: &GitStatus) -> String {
    let mut flags = String::new();
    if status.conflicted > 0 {
        write!(flags, " {}", format!("={}", status.conflicted).red()).ok();
    }
    if status.staged > 0 {
        write!(flags, " {}", format!("+{}", status.staged).green()).ok();
    }
    if status.modified > 0 {
        write!(flags, " {}", format!("!{}", status.modified).yellow()).ok();
    }
    if status.deleted > 0 {
        write!(flags, " {}", format!("✘{}", status.deleted).red()).ok();
    }
    if status.untracked > 0 {
        write!(flags, " {}", format!("?{}", status.untracked).blue()).ok();
    }
    if status.stashes > 0 {
        write!(flags, " {}", format!("${}", status.stashes).cyan()).ok();
    }
    if status.ahead > 0 {
        write!(flags, " {}", format!("⇡{}", status.ahead).green()).ok();
    }
    if status.behind > 0 {
        write!(flags, " {}", format!("⇣{}", status.behind).red()).ok();
    }
    format!(
        " {} {}{}",
        "\u{2387}".dimmed(),
        status.branch.magenta(),
        flags
    )
}

fn stash_count(repo: &mut Repository) -> Result<u32, git2::Error> {
    let mut count = 0;
    repo.stash_foreach(|_, _, _| {
        count += 1;
        true
    })?;
    Ok(count)
}

/// Returns (insertions, deletions) for uncommitted changes (staged + unstaged) vs HEAD.
fn diff_lines(repo: &Repository) -> Option<(usize, usize)> {
    let head = repo.head().ok()?.peel_to_tree().ok()?;
    let diff = repo
        .diff_tree_to_workdir_with_index(Some(&head), None)
        .ok()?;
    let stats = diff.stats().ok()?;
    Some((stats.insertions(), stats.deletions()))
}

fn pct_color(pct: f64, label: &str) -> String {
    colorize_by_pct(pct, &format!("{label}:{pct:.0}%"))
}

fn colorize_by_pct(pct: f64, text: &str) -> String {
    if pct >= 80.0 {
        text.red().to_string()
    } else if pct >= 50.0 {
        text.yellow().to_string()
    } else {
        text.green().to_string()
    }
}

/// Returns the elapsed percentage of a rate limit window.
fn window_pct(resets_at: i64, window_secs: f64) -> f64 {
    let now = Local::now().timestamp();
    let start = resets_at as f64 - window_secs;
    let elapsed = (now as f64 - start).clamp(0.0, window_secs);
    elapsed / window_secs * 100.0
}

/// Sustainable pace indicator for a weekly window: `▲` when usage is ahead of
/// elapsed time, `▼` when behind. Color reflects how far off pace it is.
fn pace_arrow(usage_pct: f64, time_pct: f64) -> String {
    let delta = usage_pct - time_pct;
    let arrow = if delta > 20.0 {
        "▲".red()
    } else if delta > 0.0 {
        "▲".yellow()
    } else if delta > -20.0 {
        "▼".green()
    } else {
        "▼".bright_green()
    };
    format!(" {arrow}")
}

const WEEK_SECS: f64 = 7.0 * 24.0 * 3600.0;

fn main() {
    let cli = Cli::parse();
    if cli.refresh_usage {
        usage::refresh();
        return;
    }
    colored::control::set_override(true);

    let data: Input = match serde_json::from_reader(std::io::stdin().lock()) {
        Ok(d) => d,
        Err(_) => return,
    };

    let (dir, worktree) = match data.workspace.git_worktree.as_deref() {
        Some(name) => (
            shorten_home(strip_worktree_suffix(&data.workspace.current_dir, name)),
            format!(" {}{}", "\u{2442}".dimmed(), name.magenta()),
        ),
        None => (shorten_home(&data.workspace.current_dir), String::new()),
    };
    let git_status = git_status(&data.workspace.current_dir);
    let git = git_status.as_ref().map(render_git).unwrap_or_default();

    let ctx = data
        .context_window
        .used_percentage
        .map(|p| format!(" {}", pct_color(p, "ctx")))
        .unwrap_or_default();

    let five_hour = data.rate_limits.as_ref().and_then(|r| r.five_hour.as_ref());
    let five_hour_pct = five_hour.and_then(|r| r.used_percentage);

    let rate = five_hour_pct
        .map(|p| format!(" {}", pct_color(p, "5h")))
        .unwrap_or_default();

    let rate_5h_time = five_hour
        .and_then(|r| r.resets_at)
        .map(|ts| {
            let time_pct = window_pct(ts, 5.0 * 3600.0);
            let color_pct = five_hour_pct.unwrap_or(0.0);
            format!(
                " {}",
                colorize_by_pct(color_pct, &format!("t:{time_pct:.0}%"))
            )
        })
        .unwrap_or_default();

    let seven_day = data.rate_limits.as_ref().and_then(|r| r.seven_day.as_ref());
    let seven_day_pct = seven_day.and_then(|r| r.used_percentage);

    let weekly = seven_day_pct
        .map(|p| format!(" {}", pct_color(p, "7d")))
        .unwrap_or_default();

    let cost_usd = data.cost.total_cost_usd.unwrap_or(0.0);
    let cost = if cost_usd > 0.0 {
        format!(" {}", format!("${cost_usd:.2}").green())
    } else {
        String::new()
    };

    let lines = match git_status.as_ref().map(|g| (g.diff_added, g.diff_removed)) {
        Some((added, removed)) if added > 0 || removed > 0 => format!(
            " {} {}",
            format!("+{added}").green(),
            format!("-{removed}").red(),
        ),
        _ => String::new(),
    };

    let week = seven_day
        .and_then(|r| r.resets_at)
        .map(|ts| {
            let time_pct = window_pct(ts, WEEK_SECS);
            let color_pct = seven_day_pct.unwrap_or(0.0);
            let wk_text = colorize_by_pct(color_pct, &format!("wk:{time_pct:.0}%"));
            let pace = seven_day_pct
                .map(|usage| pace_arrow(usage, time_pct))
                .unwrap_or_default();
            format!(" {wk_text}{pace}")
        })
        .unwrap_or_default();

    // Per-model weekly limits (e.g. Fable) are not in the statusline JSON; only
    // subscribers (who get `rate_limits`) have them, so skip the lookup otherwise.
    let scoped = if cli.no_usage || data.rate_limits.is_none() {
        String::new()
    } else {
        usage::scoped_limits()
            .iter()
            .map(|l| {
                let text = pct_color(l.used_percentage, &l.name.to_lowercase());
                let pace = l
                    .resets_at
                    .map(|ts| pace_arrow(l.used_percentage, window_pct(ts, WEEK_SECS)))
                    .unwrap_or_default();
                format!(" {text}{pace}")
            })
            .collect()
    };

    let model = data
        .model
        .display_name
        .split_once(" (")
        .map(|(name, _)| name)
        .unwrap_or(&data.model.display_name);
    let dir_fmt = dir.bold().blue();
    let sep = "|".dimmed();
    let model_fmt = model.cyan();
    println!(
        "{dir_fmt}{worktree}{git}{lines} {sep} {model_fmt}{ctx}{rate}{rate_5h_time}{weekly}{week}{scoped}{cost}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Oid;
    use std::fs;
    use tempfile::TempDir;

    fn force_colors() {
        colored::control::set_override(true);
    }

    fn git_part(dir: &str) -> String {
        git_status(dir).as_ref().map(render_git).unwrap_or_default()
    }

    fn init_test_repo() -> (TempDir, Repository, Oid) {
        let tmp = TempDir::new().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        let oid = {
            let sig = repo.signature().unwrap();
            let tree_id = repo.index().unwrap().write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap()
        };
        (tmp, repo, oid)
    }

    #[test]
    fn shorten_home_replaces_home_prefix() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(shorten_home(&format!("{home}/src/foo")), "~/src/foo");
    }

    #[test]
    fn shorten_home_leaves_other_paths() {
        assert_eq!(shorten_home("/tmp/something"), "/tmp/something");
    }

    #[test]
    fn pct_color_green_below_50() {
        force_colors();
        let result = pct_color(25.0, "ctx");
        assert!(result.contains("ctx:25%"));
        assert!(result.contains("\x1b[32m"));
    }

    #[test]
    fn pct_color_yellow_at_50() {
        force_colors();
        let result = pct_color(50.0, "rate");
        assert!(result.contains("rate:50%"));
        assert!(result.contains("\x1b[33m"));
    }

    #[test]
    fn pct_color_red_at_80() {
        force_colors();
        let result = pct_color(80.0, "ctx");
        assert!(result.contains("ctx:80%"));
        assert!(result.contains("\x1b[31m"));
    }

    #[test]
    fn deserializes_minimal_input() {
        let json = r#"{
            "model": {"display_name": "Test"},
            "workspace": {"current_dir": "/tmp"},
            "context_window": {},
            "cost": {}
        }"#;
        let data: Input = serde_json::from_str(json).unwrap();
        assert_eq!(data.model.display_name, "Test");
        assert!(data.workspace.git_worktree.is_none());
        assert!(data.context_window.used_percentage.is_none());
        assert!(data.cost.total_cost_usd.is_none());
        assert!(data.rate_limits.is_none());
    }

    #[test]
    fn strip_worktree_suffix_trims_claude_worktree_path() {
        assert_eq!(
            strip_worktree_suffix("/home/user/proj/.claude/worktrees/feat", "feat"),
            "/home/user/proj"
        );
    }

    #[test]
    fn strip_worktree_suffix_leaves_other_locations() {
        assert_eq!(
            strip_worktree_suffix("/home/user/proj-feat", "feat"),
            "/home/user/proj-feat"
        );
    }

    #[test]
    fn deserializes_git_worktree() {
        let json = r#"{
            "model": {"display_name": "Test"},
            "workspace": {"current_dir": "/tmp/proj/.claude/worktrees/feat", "git_worktree": "feat"},
            "context_window": {},
            "cost": {}
        }"#;
        let data: Input = serde_json::from_str(json).unwrap();
        assert_eq!(data.workspace.git_worktree.as_deref(), Some("feat"));
    }

    #[test]
    fn deserializes_full_input() {
        let json = r#"{
            "model": {"display_name": "Claude Fable 5"},
            "workspace": {"current_dir": "/home/user/project"},
            "context_window": {"used_percentage": 42.5},
            "cost": {"total_cost_usd": 1.23},
            "rate_limits": {"five_hour": {"used_percentage": 15.0}, "seven_day": {"used_percentage": 42.0}}
        }"#;
        let data: Input = serde_json::from_str(json).unwrap();
        assert_eq!(data.context_window.used_percentage, Some(42.5));
        assert_eq!(data.cost.total_cost_usd, Some(1.23));
        let rl = data.rate_limits.unwrap();
        assert_eq!(rl.five_hour.unwrap().used_percentage, Some(15.0));
        assert_eq!(rl.seven_day.unwrap().used_percentage, Some(42.0));
    }

    #[test]
    fn git_part_non_repo_returns_empty() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(git_part(tmp.path().to_str().unwrap()), "");
    }

    #[test]
    fn git_part_clean_repo_shows_branch() {
        force_colors();
        let (tmp, _repo, _oid) = init_test_repo();
        let result = git_part(tmp.path().to_str().unwrap());
        assert!(result.contains("main") || result.contains("master"));
    }

    #[test]
    fn git_part_dirty_repo_shows_bang() {
        force_colors();
        let (tmp, repo, _oid) = init_test_repo();

        let file_path = tmp.path().join("file.txt");
        fs::write(&file_path, "hello").unwrap();
        let sig = repo.signature().unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("file.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "add file", &tree, &[&head])
            .unwrap();

        fs::write(&file_path, "changed").unwrap();

        let result = git_part(tmp.path().to_str().unwrap());
        assert!(result.contains("!"));
    }

    #[test]
    fn git_part_untracked_file_shows_question() {
        force_colors();
        let (tmp, _repo, _oid) = init_test_repo();
        fs::write(tmp.path().join("untracked.txt"), "new").unwrap();
        let result = git_part(tmp.path().to_str().unwrap());
        assert!(result.contains("?"));
    }

    #[test]
    fn window_pct_halfway() {
        // resets_at 2.5 hours from now → halfway through a 5h window
        let resets_at = Local::now().timestamp() + (2.5 * 3600.0) as i64;
        let pct = window_pct(resets_at, 5.0 * 3600.0);
        assert!((pct - 50.0).abs() < 1.0, "expected ~50%, got: {pct}");
    }

    #[test]
    fn pace_arrow_reflects_usage_vs_time() {
        force_colors();
        assert!(
            pace_arrow(80.0, 50.0).contains("▲") && pace_arrow(80.0, 50.0).contains("\x1b[31m")
        );
        assert!(
            pace_arrow(55.0, 50.0).contains("▲") && pace_arrow(55.0, 50.0).contains("\x1b[33m")
        );
        assert!(
            pace_arrow(45.0, 50.0).contains("▼") && pace_arrow(45.0, 50.0).contains("\x1b[32m")
        );
        assert!(
            pace_arrow(10.0, 50.0).contains("▼") && pace_arrow(10.0, 50.0).contains("\x1b[92m")
        );
    }

    #[test]
    fn git_part_detached_head_shows_sha() {
        force_colors();
        let (tmp, repo, oid) = init_test_repo();
        repo.set_head_detached(oid).unwrap();
        let result = git_part(tmp.path().to_str().unwrap());
        assert!(result.contains(&oid.to_string()[..7]));
    }
}
