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
<dir> ⎇ <branch> +<staged> !<modified> ✘<deleted> ?<untracked> $<stashes> ⇡<ahead> ⇣<behind> +<added> -<removed> | <model> ctx:<N>% 5h:<N>% t:<N>% 7d:<N>% wk:<N>% <pace> fb:<N>% $<cost>
```

Rate limit time percentages: `t:` shows elapsed time in the 5-hour window, `wk:` shows elapsed time in the 7-day window. Both use the `resets_at` timestamp from Claude Code and inherit their color from the corresponding usage percentage.

Pace indicator: `▲` (over) or `▼` (under) sustainable usage pace for the 7-day window. Color reflects severity: bright green (well under), green (under), yellow (slightly over), red (significantly over).

Fable usage (`fb:`): read from `rate_limits.model_scoped[]`, the per-model weekly windows, matching the entry whose `display_name` contains "fable". Claude Code does not emit this field to statusline commands as of 2.1.227 — it only reaches the Agent SDK's `get_usage` response, where the schema is marked experimental — so the field is parsed with `#[serde(default)]` and the segment is hidden when absent. `model_scoped` carries the server's raw `utilization` rather than a pre-scaled `used_percentage`, so `as_percentage()` accepts either a 0-1 fraction or a 0-100 percentage.

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
- `git2` — git status via libgit2 (no subprocess spawning)
- `chrono` — local time for rate limit window elapsed percentages
- `colored` — ANSI terminal colors (forced on since stdout is piped)

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
