# claude-statusline

A fast, custom statusline for [Claude Code](https://claude.ai/code), written in Rust.

Replaces the default statusline with a compact, color-coded display showing your working directory, git status, model, context usage, rate limits, session cost, and lines changed.

## Output

```
~/src/my-project ⎇ main +2 !3 ?4 | Opus 4.6 ctx:24% 5h:12% t:40% 7d:5% wk:53% ▼ $1.47 +42 -10
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
| `5h:N%` | 5-hour rate limit usage — green/yellow/red at 50%/80% |
| `t:N%` | Elapsed time in the 5-hour rate limit window |
| `7d:N%` | 7-day rate limit usage — green/yellow/red at 50%/80% |
| `wk:N%` | Elapsed time in the 7-day rate limit window |
| `▼` / `▲` | Weekly pace indicator — under/over sustainable usage rate |
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
