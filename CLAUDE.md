# claude-statusline

A custom statusline for Claude Code, written in Rust.

## Key Commands

```bash
# Build
cargo build --release

# Install to ~/.cargo/bin
cargo install --path .

# Lint and format
cargo clippy && cargo fmt
```

## Architecture

Single-binary CLI that reads Claude Code's statusline JSON from stdin and outputs a formatted, colored line to stdout.

### Input

Claude Code pipes a JSON object to stdin with fields: `model`, `workspace`, `context_window`, `cost`, `rate_limits`.

### Output Format

```
<dir> ⑂<worktree> ⎇ <branch> +<staged> !<modified> ✘<deleted> ?<untracked> $<stashes> ⇡<ahead> ⇣<behind> +<added> -<removed> | <model> ctx:<N>% 5h:<N>% t:<N>% 7d:<N>% wk:<N>% <pace> <model>:<N>% $<cost>
```

Worktree (`⑂<worktree>`): shown only when Claude Code sends `workspace.git_worktree`, which it does
when the session's cwd is a linked git worktree (the value is the worktree's name). For worktrees
Claude Code creates itself, `current_dir` is `<repo>/.claude/worktrees/<name>`; that suffix is
stripped so the directory reads as the repo root. Worktrees located elsewhere keep their full path.

Rate limit time percentages: `t:` shows elapsed time in the 5-hour window, `wk:` shows elapsed time in the 7-day window. Both use the `resets_at` timestamp from Claude Code and inherit their color from the corresponding usage percentage.

Pace indicator: `▲` (over) or `▼` (under) sustainable usage pace for the 7-day window. Color reflects severity: bright green (well under), green (under), yellow (slightly over), red (significantly over).

Per-model weekly limits (`<model>:<N>%`, e.g. `fable:7%`): Claude Code's statusline JSON omits these,
so `src/usage.rs` fetches `GET https://api.anthropic.com/api/oauth/usage` (what `/usage` uses) with the
OAuth token from the macOS Keychain item `Claude Code-credentials` (falling back to
`$CLAUDE_CONFIG_DIR/.credentials.json`). Only `limits[]` entries with `kind: "weekly_scoped"` are shown,
labelled by `scope.model.display_name` lower-cased. The result is cached in
`$XDG_CACHE_HOME/claude-statusline/usage.json` (default `~/.cache/...`) for 5 minutes. A stale cache
makes the render spawn a detached `claude-statusline --refresh-usage` child (guarded by `usage.lock`)
and continue with the cached values, so the render path never touches the network. Expired tokens are
never refreshed here since Claude Code owns and rotates them. `--no-usage` disables the feature; the
lookup is also skipped when the input has no `rate_limits` (API-key users have no such buckets).

Git status symbols (starship-style):
- `+N` — staged files
- `!N` — modified files
- `✘N` — deleted files
- `?N` — untracked files
- `$N` — stashes
- `=N` — conflicted files
- `⇡N` / `⇣N` — ahead / behind upstream

Lines changed (`+<added> -<removed>`): total insertions and deletions in uncommitted changes (staged + unstaged) vs HEAD, computed via `git2::Diff::stats()`.

### Dependencies

- `serde` / `serde_json` — JSON deserialization
- `git2` — git status via libgit2 (no subprocess spawning). Uses `vendored-libgit2` so libgit2 is
  built from source and statically linked, rather than picking up a system copy whose path breaks
  when the package manager upgrades it.
- `chrono` — local time for rate limit window elapsed percentages
- `colored` — ANSI terminal colors (forced on since stdout is piped)
- `ureq` (rustls) — the one HTTP call, made only from the background refresh child
- `security-framework` (macOS only) — reads the Keychain without spawning `security`

### Configuration

Referenced in `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "claude-statusline"
  }
}
```
