# claude-statusline

A fast, custom statusline for [Claude Code](https://claude.ai/code), written in Rust.

Replaces the default statusline with a compact, color-coded display showing your working directory, git status, model, context usage, rate limits, session cost, and lines changed.

## Output

```
~/src/my-project ⎇ main +2 !3 ?4 +42 -10 | Fable 5 ctx:24% 5h:12% t:40% 7d:5% wk:53% ▼ $1.47
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
| `fb:N%` | Fable weekly allowance — only shown when Claude Code reports it (see below) |
| `$N.NN` | Session cost in USD (green) |
| `+N -N` | Uncommitted lines added (green) / removed (red) vs HEAD |

### Fable usage

Fable draws on its own weekly allowance, separate from the all-models `7d:` limit, so
`7d:` alone does not tell you how much Fable you have left.

Claude Code does not currently put that bucket in the statusline payload. As of 2.1.227 it
builds `rate_limits` from `five_hour` and `seven_day` only; the per-model windows exist in
the CLI, but reach the Agent SDK's `get_usage` response (as `rate_limits.model_scoped`)
rather than the statusline. That schema is marked experimental.

`fb:N%` is therefore parsed optionally and hidden when absent. If Anthropic adds
`model_scoped` to the statusline payload, the segment starts working with no change here.
Until then, `/usage` is the way to see the Fable window.

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
