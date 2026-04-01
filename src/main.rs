use clap::Parser;
use colored::Colorize;
use git2::{Repository, Status, StatusOptions};
use serde::Deserialize;
use std::fmt::Write as _;

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
struct Cli {}

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
}

#[derive(Deserialize)]
struct ContextWindow {
    used_percentage: Option<f64>,
}

#[derive(Deserialize)]
struct Cost {
    total_cost_usd: Option<f64>,
    total_lines_added: Option<i64>,
    total_lines_removed: Option<i64>,
}

#[derive(Deserialize)]
struct RateLimits {
    five_hour: Option<RateLimit>,
}

#[derive(Deserialize)]
struct RateLimit {
    used_percentage: Option<f64>,
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

fn git_part(dir: &str) -> String {
    let mut repo = match Repository::discover(dir) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };

    let (is_branch, branch) = {
        let head = match repo.head() {
            Ok(h) => h,
            Err(_) => return String::new(),
        };
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
        return String::new();
    }

    let mut flags = String::new();

    let mut opts = StatusOptions::new();
    opts.include_untracked(true).exclude_submodules(true);
    if let Ok(statuses) = repo.statuses(Some(&mut opts)) {
        let mut staged = 0u32;
        let mut modified = 0u32;
        let mut deleted = 0u32;
        let mut untracked = 0u32;
        let mut conflicted = 0u32;

        for entry in statuses.iter() {
            let s = entry.status();
            if s.contains(Status::CONFLICTED) {
                conflicted += 1;
            }
            if s.intersects(STAGED) {
                staged += 1;
            }
            if s.intersects(WT_MODIFIED) {
                modified += 1;
            }
            if s.intersects(Status::WT_DELETED) || s.intersects(Status::INDEX_DELETED) {
                deleted += 1;
            }
            if s.contains(Status::WT_NEW) {
                untracked += 1;
            }
        }
        if conflicted > 0 {
            write!(flags, " {}", format!("={conflicted}").red()).ok();
        }
        if staged > 0 {
            write!(flags, " {}", format!("+{staged}").green()).ok();
        }
        if modified > 0 {
            write!(flags, " {}", format!("!{modified}").yellow()).ok();
        }
        if deleted > 0 {
            write!(flags, " {}", format!("✘{deleted}").red()).ok();
        }
        if untracked > 0 {
            write!(flags, " {}", format!("?{untracked}").blue()).ok();
        }
    }

    if let Ok(stashes) = stash_count(&mut repo)
        && stashes > 0
    {
        write!(flags, " {}", format!("${stashes}").cyan()).ok();
    }

    if is_branch {
        let upstream_ref = format!("refs/heads/{branch}");
        if let Ok(local_oid) = repo.refname_to_id("HEAD")
            && let Ok(upstream_name) = repo.branch_upstream_name(&upstream_ref)
            && let Some(name) = upstream_name.as_str()
            && let Ok(upstream_oid) = repo.refname_to_id(name)
            && let Ok((ahead, behind)) = repo.graph_ahead_behind(local_oid, upstream_oid)
        {
            if ahead > 0 {
                write!(flags, " {}", format!("⇡{ahead}").green()).ok();
            }
            if behind > 0 {
                write!(flags, " {}", format!("⇣{behind}").red()).ok();
            }
        }
    }

    format!(" {} {}{}", "\u{2387}".dimmed(), branch.magenta(), flags,)
}

fn stash_count(repo: &mut Repository) -> Result<u32, git2::Error> {
    let mut count = 0u32;
    repo.stash_foreach(|_, _, _| {
        count += 1;
        true
    })?;
    Ok(count)
}

fn pct_color(pct: f64, label: &str) -> String {
    let text = format!("{label}:{pct:.0}%");
    if pct >= 80.0 {
        text.red().to_string()
    } else if pct >= 50.0 {
        text.yellow().to_string()
    } else {
        text.green().to_string()
    }
}

fn main() {
    let _cli = Cli::parse();
    colored::control::set_override(true);

    let data: Input = match serde_json::from_reader(std::io::stdin().lock()) {
        Ok(d) => d,
        Err(_) => return,
    };

    let dir = shorten_home(&data.workspace.current_dir);
    let git = git_part(&data.workspace.current_dir);

    let ctx = data
        .context_window
        .used_percentage
        .map(|p| format!(" {}", pct_color(p, "ctx")))
        .unwrap_or_default();

    let rate = data
        .rate_limits
        .and_then(|r| r.five_hour)
        .and_then(|r| r.used_percentage)
        .map(|p| format!(" {}", pct_color(p, "rate")))
        .unwrap_or_default();

    let cost_usd = data.cost.total_cost_usd.unwrap_or(0.0);
    let cost = if cost_usd > 0.0 {
        format!(" {}", format!("${cost_usd:.2}").green())
    } else {
        String::new()
    };

    let added = data.cost.total_lines_added.unwrap_or(0);
    let removed = data.cost.total_lines_removed.unwrap_or(0);
    let lines = if added > 0 || removed > 0 {
        format!(
            " {} {}",
            format!("+{added}").green(),
            format!("-{removed}").red(),
        )
    } else {
        String::new()
    };

    let model = &data.model.display_name;
    println!(
        "{}{} {} {}{}{}{}{lines}",
        dir.bold().blue(),
        git,
        "|".dimmed(),
        model.cyan(),
        ctx,
        rate,
        cost,
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
        assert!(data.context_window.used_percentage.is_none());
        assert!(data.cost.total_cost_usd.is_none());
        assert!(data.rate_limits.is_none());
    }

    #[test]
    fn deserializes_full_input() {
        let json = r#"{
            "model": {"display_name": "Claude Opus 4.6"},
            "workspace": {"current_dir": "/home/user/project"},
            "context_window": {"used_percentage": 42.5},
            "cost": {"total_cost_usd": 1.23, "total_lines_added": 10, "total_lines_removed": 5},
            "rate_limits": {"five_hour": {"used_percentage": 15.0}}
        }"#;
        let data: Input = serde_json::from_str(json).unwrap();
        assert_eq!(data.context_window.used_percentage, Some(42.5));
        assert_eq!(data.cost.total_cost_usd, Some(1.23));
        assert_eq!(data.cost.total_lines_added, Some(10));
        assert_eq!(
            data.rate_limits.unwrap().five_hour.unwrap().used_percentage,
            Some(15.0)
        );
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
    fn git_part_detached_head_shows_sha() {
        force_colors();
        let (tmp, repo, oid) = init_test_repo();
        repo.set_head_detached(oid).unwrap();
        let result = git_part(tmp.path().to_str().unwrap());
        assert!(result.contains(&oid.to_string()[..7]));
    }
}
