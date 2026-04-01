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
<dir> [<branch> <flags>] | <model> ctx:<N>% rate:<N>% $<cost> +<added> -<removed>
```

### Dependencies

- `serde` / `serde_json` — JSON deserialization
- `git2` — git status via libgit2 (no subprocess spawning)
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
