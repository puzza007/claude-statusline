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
<dir> ⎇ <branch> +<staged> !<modified> ✘<deleted> ?<untracked> $<stashes> ⇡<ahead> ⇣<behind> | <model> ctx:<N>% 5h:<N>% t:<N>% 7d:<N>% wk:<N>% <pace> $<cost> +<added> -<removed>
```

Rate limit time percentages: `t:` shows elapsed time in the 5-hour window, `wk:` shows elapsed time in the 7-day window. Both use the `resets_at` timestamp from Claude Code and inherit their color from the corresponding usage percentage.

Pace indicator: `▲` (over) or `▼` (under) sustainable usage pace for the 7-day window. Color reflects severity: bright green (well under), green (under), yellow (slightly over), red (significantly over).

Git status symbols (starship-style):
- `+N` — staged files
- `!N` — modified files
- `✘N` — deleted files
- `?N` — untracked files
- `$N` — stashes
- `=N` — conflicted files
- `⇡N` / `⇣N` — ahead / behind upstream

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
