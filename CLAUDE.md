# claude-statusline

A custom statusline for Claude Code, written in Rust.

## Key Commands

```bash
# Build
cargo build --release

# Install to ~/.cargo/bin
cargo install --path .

# Lint and format
cargo clippy --workspace && cargo fmt

# Report service
docker compose up -d --build   # http://localhost:8787
cargo run -p claude-statusline-report
```

## Architecture

Single-binary CLI that reads Claude Code's statusline JSON from stdin and outputs a formatted, colored line to stdout.

### Input

Claude Code pipes a JSON object to stdin with fields: `model`, `workspace`, `context_window`, `cost`, `rate_limits`.

### Output Format

```
<dir> ⑂<worktree> ⎇ <branch> +<staged> !<modified> ✘<deleted> ?<untracked> $<stashes> ⇡<ahead> ⇣<behind> +<added> -<removed> | <model> ctx:<N>% 5h:<N>% t:<N>% 7d:<N>% wk:<N>% <pace> <model>:<N>% <pace> $<cost>
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
`$CLAUDE_CONFIG_DIR/.credentials.json`). The Keychain read shells out to `security` on purpose:
Claude Code writes the item with that tool, so it is permanently on the item's access list, whereas
a grant to this binary is tied to its ad-hoc per-build signature and is wiped each time Claude Code
recreates the item on token refresh, which produced a Keychain prompt on every fetch. Only `limits[]` entries with `kind: "weekly_scoped"` are shown,
labelled by `scope.model.display_name` lower-cased, each followed by its own pace arrow computed
from that bucket's `resets_at` over a 7-day window. The result is cached in
`$XDG_CACHE_HOME/claude-statusline/usage.json` (default `~/.cache/...`) for 5 minutes. A stale cache
makes the render spawn a detached `claude-statusline --refresh-usage` child (guarded by `usage.lock`)
and continue with the cached values, so the render path never touches the network. Expired tokens are
never refreshed here since Claude Code owns and rotates them. `--no-usage` disables the feature; the
lookup is also skipped when the input has no `rate_limits` (API-key users have no such buckets).

History (`src/history.rs`): every render is recorded to an SQLite database at
`$XDG_DATA_HOME/claude-statusline/history.db` (default `~/.local/share/...`) via `rusqlite` with the
`bundled` feature (statically linked, like libgit2). Three tables: `samples` (one row per observed
state change per session, plus a heartbeat row every 5 idle minutes), `sessions` (first/last seen,
cwd, worktree, Claude Code version, and the fingerprint of the last sample used for change
detection) and `usage_limits` (one row per model per successful usage API fetch, written by the
`--refresh-usage` child). Change detection is a hash of every sample field except timestamps, stored
in `sessions.last_fp`, so the thousands of identical renders per turn collapse into one row. Recording
runs after the line is printed, uses WAL + a 50 ms busy timeout so concurrent sessions can share the
file, and swallows every error so the render is never broken. Schema changes are appended to `MIGRATIONS`; each step is
applied in its own transaction together with the `PRAGMA user_version` bump so an interrupted step
is retried whole. `GitStatus` (in `main.rs`, including the uncommitted diff line counts) is hashed
into the fingerprint; `Sample::fingerprint` destructures every field so adding one is a compile
error until it is placed in or explicitly outside the hash. `samples.cwd` is
recorded per row (not just the latest on `sessions`) so a branch is always attributable to a repo. `--no-history` disables recording and is forwarded to the refresh child.

Report (`report/`, workspace member `claude-statusline-report`): an axum service that serves the
history as a chart page. `report/template.html` is the page; it has no document skeleton because
the same file is published as a Claude artifact (which wraps it) and the server adds its own
`<!doctype>`/`<head>`/`<body>`. The server replaces the single `__DATA__` token with JSON from three
queries (`samples`, `usage_limits`, `sessions`, aliases as the template expects) and escapes every `<` in
the JSON as `\u003c` so no value can end the script block. The database is opened `SQLITE_OPEN_READ_ONLY` per
request with a 500 ms busy timeout; `?days=N` (default 30, `all`) bounds the rows so the page does
not grow without limit. `report/Dockerfile` builds on `rust:1-alpine` (musl, so the release binary
is static) into a `scratch` image (a stub `src/main.rs` stands in for the statusline package so the
workspace resolves without libgit2); `docker-compose.yml` bind-mounts the data directory at
`/data`, which must be the directory rather than the file so SQLite can use the `-wal`/`-shm`
files. Chart colours follow the dataviz palette: the six largest sessions by spend take fixed
categorical slots and the rest are muted "other", tables only.

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
- `rusqlite` (bundled) — the history database; SQLite is built from source and statically linked
- `git2` — git status via libgit2 (no subprocess spawning). Uses `vendored-libgit2` so libgit2 is
  built from source and statically linked, rather than picking up a system copy whose path breaks
  when the package manager upgrades it.
- `chrono` — local time for rate limit window elapsed percentages
- `colored` — ANSI terminal colors (forced on since stdout is piped)
- `ureq` (rustls) — the one HTTP call, made only from the background refresh child

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
