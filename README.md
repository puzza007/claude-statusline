# claude-statusline

A fast, custom statusline for [Claude Code](https://claude.ai/code), written in Rust.

Replaces the default statusline with a compact, color-coded display showing your working directory, git status, model, context usage, rate limits, session cost, and lines changed.

## Output

```
~/src/my-project ⎇ main +2 !3 ?4 +42 -10 | Fable 5 ctx:24% 5h:12% t:40% 7d:5% wk:53% fable:7% ▼ $1.47
```

In a Claude Code worktree session the nested `.claude/worktrees/<name>` path collapses back to the
repo root, with the worktree name shown alongside it:

```
~/src/my-project ⑂fix-login ⎇ fix-login !1 | Fable 5 ctx:24% $0.12
```

| Segment | Description |
|---|---|
| Directory | Current working directory (bold blue) |
| `⑂name` | Git worktree name, shown only in a worktree session (magenta) |
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
| `fable:N%` | Per-model weekly limit usage (one per model bucket, e.g. Fable) — green/yellow/red at 50%/80% |
| `▼` / `▲` | Weekly pace indicator — under/over sustainable usage rate |
| `$N.NN` | Session cost in USD (green) |
| `+N -N` | Uncommitted lines added (green) / removed (red) vs HEAD |

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

### Per-model limits

Claude Code's statusline JSON does not include per-model weekly limits such as the Fable bucket, so
the statusline fetches them itself from the same usage endpoint the `/usage` command uses, with the
OAuth token Claude Code stores in the macOS Keychain (or `~/.claude/.credentials.json` elsewhere).
The response is cached in `~/.cache/claude-statusline/usage.json` for 5 minutes, and refreshes run in
a detached background process so rendering never waits on the network. Pass `--no-usage` in the
command above to turn this off.

## License

MIT
