# claude-statusline

A fast, custom statusline for [Claude Code](https://claude.ai/code), written in Rust.

Replaces the default statusline with a compact, color-coded display showing your working directory, git status, model, context usage, rate limits, session cost, and lines changed.

## Output

```
~/src/my-project ⎇ main +2 !3 ?4 | Opus 4.6 (1M context) ctx:24% rate:12% $1.47 +42 -10
```

| Segment | Description |
|---|---|
| Directory | Current working directory (bold blue) |
| `⎇` | Git branch icon (dimmed) |
| Branch | Git branch name or short SHA when detached (magenta) |
| `=N` | Conflicted files (red) |
| `+N` | Staged files (green) |
| `!N` | Modified files (yellow) |
| `✘N` | Deleted files (red) |
| `?N` | Untracked files (blue) |
| `$N` | Stashes (cyan) |
| `⇡N` / `⇣N` | Commits ahead (green) / behind (red) upstream |
| Model | Active Claude model (cyan) |
| `ctx:N%` | Context window usage — green/yellow/red at 50%/80% |
| `rate:N%` | 5-hour rate limit usage — green/yellow/red at 50%/80% |
| `$N.NN` | Session cost in USD (green) |
| `+N -N` | Lines added (green) / removed (red) |

## Install

```bash
cargo install --path .
```

## Configure

Add to `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "claude-statusline"
  }
}
```

## License

MIT
