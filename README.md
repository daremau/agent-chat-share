# acs — agent-chat-share

Share an in-progress chat between coding agents (Claude Code, Codex, OpenCode).

`acs` reads a source agent's session, converts it to a canonical model, renders a
portable **transcript**, and prints the exact command to **continue that
conversation in a target agent**. It does not write into any agent's session
storage, and it never launches the other agent — it emits a command for you to
run.

## Why transcript-seed (not native sessions)

Codex and OpenCode index their sessions in private SQLite databases, and the
agents' tool vocabularies don't overlap. Rather than forge native, resumable
sessions (fragile and version-coupled), `acs` hands the target a readable
transcript as seed context. See `openspec/changes/add-chat-share-cli/design.md`.

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

# One-shot: read the current chat and print a command to continue it in Codex
acs share --from claude --to codex
```

`share` writes a transcript file and prints a seed command, e.g.:

```
Wrote shared-chat-<id>.md (42 turns)
Run this to continue in codex:

  codex "Continue this prior conversation… $(cat 'shared-chat-<id>.md')"
```

Run that command to continue the conversation in Codex.

Session selection: with no `--session`, `acs` uses the current session — for
Claude Code it reads `$CLAUDE_CODE_SESSION_ID`, otherwise it picks the most
recently updated session for the current directory. Use `acs list` to find ids.

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

- **Supported end-to-end:** Claude Code (read) → Codex (seed).
- **Scaffolded:** Codex/OpenCode read, OpenCode seed — report a clear "not yet
  supported" message. The OpenCode read adapter will query its SQLite database.

## Development

```bash
cargo test
cargo fmt
cargo clippy --all-targets
```
