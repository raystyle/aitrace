# aitrace

Rust TUI for AI coding-session observability. Records every file edit into `.aitrace/`, correlates Claude Code hook metadata, and exposes the timeline over MCP so the agent can inspect and fix its own regressions.

This checkout is the project Claude Code should operate in. Config is project-scoped only: do not write hooks, MCP servers, or skills into `~/.claude/` or `~/.claude.json` user scope.

## Commands

```bash
cargo test
cargo build
cargo clippy
aitrace daemon start
aitrace daemon status
aitrace sessions
aitrace mcp
```

Project install lives under `.aitrace/` (gitignored): sessions, snapshots, `daemon.sock`, and `bin/aitrace.exe`. Hook and MCP must call that project binary, not a user-scope `~/.claude` config and not a global `aitrace` on PATH. `aitrace init` and the daemon copy the running exe into `.aitrace/bin/`. A PATH install is only a convenience so you can type `aitrace` in a shell.

## Claude Code surfaces (this repo)

| Surface | File | How to use |
| --- | --- | --- |
| Hook | `.claude/settings.json` | `PostToolUse` on `Write\|Edit` runs `aitrace hook-send` |
| MCP | `.mcp.json` (repo root, not under `.claude/`) | Server name `aitrace`; tools listed below |
| Skill | `.claude/skills/aitrace-review/SKILL.md` | `/aitrace-review` after a regression |
| Local overrides | `.claude/settings.local.json` | gitignored; do not commit |

On first launch in this folder: accept the workspace trust dialog, then approve the `aitrace` MCP server if it is still pending (`/mcp`).

When tests fail or an edit sequence went wrong, invoke `/aitrace-review` (or follow that skill) instead of guessing. Use MCP tools, not a re-read of the whole tree, to find the bad frame.

## MCP tools

`list_sessions`, `get_timeline`, `get_frame`, `diff_frames`, `search_edits`, `get_regression_window`, `subscribe_edits`.

The stdio server reads `.aitrace/` in `CLAUDE_PROJECT_DIR`. Start the daemon so new edits are recorded; MCP can still read past sessions if data is already on disk.

## Layout

- `src/main.rs` — CLI (`daemon`, `mcp`, `hook-send`, TUI)
- `src/hook/` — Claude Code hook registration + `hook-send`
- `src/mcp/` — stdio JSON-RPC
- `src/daemon/` — background recorder, Unix-domain socket (Windows: `uds_windows`)
- `src/tui/` — ratatui UI
- Data: `.aitrace/` (gitignored) — snapshots, `edits.jsonl`, `daemon.sock`, sessions

## Constraints

- Crate/binary name is `aitrace`. Do not rename back to vibetracer.
- `publish = false`. No Homebrew, no crates.io.
- Windows 10 1809+ is supported (AF_UNIX via `uds_windows`). Do not introduce named pipes, TCP, or nightly std AF_UNIX.
- Do not change the user's rustup/cargo home or `D:\ohmypwsh`. Rust is already installed.
- Keep CLAUDE.md short. Put long procedures in `.claude/skills/`.
