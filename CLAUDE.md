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
<dir> ⎇ <branch> +<staged> !<modified> ✘<deleted> ?<untracked> $<stashes> ⇡<ahead> ⇣<behind> | <model> ctx:<N>% 5h:<N>% t:<bar> 7d:<N>% wk:<bar> $<cost> +<added> -<removed>
```

Rate limit time bars: 3-character block bars using fractional Unicode blocks (`░▏▎▍▌▋▊▉█`) showing elapsed time in the rate limit window. `t:` tracks the 5-hour window, `wk:` tracks the 7-day window. Both use the `resets_at` timestamp from Claude Code.

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
- `chrono` — local time for rate limit time bars
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
