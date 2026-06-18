# acs — agent-chat-share

Share an in-progress chat between coding agents (Claude Code, Codex, OpenCode).

`acs` reads a source agent's session, converts it to a canonical model, renders a
portable **transcript**, and prints the exact command to **continue that
conversation in a target agent**. It does not write into any agent's session
storage, and it never launches the other agent — it emits a command for you to
run.

## Why transcript-seed (not native sessions)

Agents store/index sessions differently, and their tool vocabularies don't
overlap. Rather than forge native, resumable sessions (fragile and
version-coupled), `acs` hands the target a readable transcript as seed context.
See `openspec/changes/add-chat-share-cli/design.md`.

## Install

```bash
cargo build --release
# binary at target/release/acs
```

## Usage

```bash
# List sessions for an agent (newest first)
acs list --agent claude
acs list --agent claude --json

# Export a session as a transcript (default) or canonical JSON
acs export --agent claude --out shared-chat.md
acs export --agent claude --format json --out chat.json

# One-shot: read the current chat and print a command to continue it in another agent
acs share --from claude --to codex
acs share --from codex --to claude
acs share --from opencode --to codex

# Browse sessions and share/export interactively
acs tui
```

`share` writes a transcript file and prints a seed command, e.g.:

```
Wrote .agents/acs/transcripts/shared-chat-<id>.md (42 turns)
Run this to continue in codex:

  codex "Continue this prior conversation… $(cat '.agents/acs/transcripts/shared-chat-<id>.md')"
```

Run that command to continue the conversation in the target agent.

Session selection: with no `--session`, `acs` uses the current session — for
Claude Code it reads `$CLAUDE_CODE_SESSION_ID`, otherwise it picks the most
recently updated session for the current directory. Use `acs list` to find ids.

## Interactive TUI

`acs tui` opens a terminal UI for browsing an agent's sessions, previewing the
transcript, and running the same `share`/`export` pipelines without retyping
flags. The selected session's transcript is previewed automatically. As with the
CLI, the TUI never launches the target agent — sharing produces a transcript and
a seed command you copy and run yourself.

Two panes: a session list on the left and a transcript preview on the right.
`Tab` switches which pane is focused (the focused pane has a highlighted border).

| Key | Action |
|-----|--------|
| `←` / `→` | Cycle the source agent |
| `Space` | Cycle the target agent |
| `Tab` | Switch focus between the session list and the transcript |
| `↑` / `↓` or `j` / `k` | Focused pane: move the session cursor, or scroll the transcript |
| `Ctrl-U` / `Ctrl-D` | Fast-scroll the transcript (from either pane) |
| `Enter` | Open the highlighted session and focus the transcript |
| `s` | Share — write a transcript and show the seed command |
| `e` | Export — write the transcript or JSON to a path |
| `c` | Copy the open modal's command/path to the clipboard |
| `r` | Reload the session list |
| `?` | Toggle help |
| `q` / `Ctrl-C` | Quit |
| `Esc` | Dismiss the current modal |

Clipboard copy shells out to the first available tool: `wl-copy`, `xclip`,
`xsel`, or `pbcopy`. If none are installed, the status line reports the failure.

## Skills

Install the skill so an agent can drive `acs` from a natural-language request
("share this chat with Codex"):

```bash
acs skills install                 # all agents, project scope (./.<agent>/skills)
acs skills install --agent claude  # one agent
acs skills install --scope user    # into your home agent dirs
acs skills uninstall
```

## Status

- **Supported end-to-end:** all directions between `claude`, `codex`, and
  `opencode`.
- **Read sources:** Claude Code JSONL sessions, Codex rollout JSONL sessions,
  and OpenCode's SQLite session database.
- **Seed targets:** Claude Code, Codex, and OpenCode initial-prompt commands.
- **Interfaces:** one-shot CLI subcommands and an interactive TUI (`acs tui`).
- OpenCode read support expects the `sqlite3` CLI to be available.

## Development

```bash
cargo test
cargo fmt
cargo clippy --all-targets
```
