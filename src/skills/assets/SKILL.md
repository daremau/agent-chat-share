---
name: agent-chat-share
description: Share the current chat with another coding agent (Claude Code, Codex, or OpenCode) using the `acs` CLI. Use when the user says things like "share this chat with codex" or "hand this conversation to opencode".
---

# Share this chat with another agent

When the user asks to share, hand off, or continue the current conversation in
another agent, use the `acs` CLI to do it. `acs` reads the current session,
renders a portable transcript, and prints the exact command to continue the
conversation in the target agent. **`acs` never launches the other agent — you
relay the command for the user to run.**

## Steps

1. Identify the **source** agent (the one you are running in) and the **target**
   agent the user named (`claude`, `codex`, or `opencode`).

2. Run the one-shot share command:

   ```bash
   acs share --from <source> --to <target>
   ```

   Example — sharing the current Claude Code chat with Codex:

   ```bash
   acs share --from claude --to codex
   ```

   `acs` selects the current session automatically (for Claude Code it reads
   `$CLAUDE_CODE_SESSION_ID`; for Codex/OpenCode it uses the newest session for
   the current directory). To share a specific session instead, pass `--session
   <id>`; use `acs list --agent <source>` to find session ids.

3. `acs` writes a transcript file and prints a seed command, e.g.:

   ```
   Wrote .agents/acs/transcripts/shared-chat.md (42 turns)
   Run this to continue in codex:

     codex "Continue this prior conversation… $(cat '.agents/acs/transcripts/shared-chat.md')"
   ```

4. **Relay to the user** the transcript path and the exact seed command, and tell
   them to run it to continue the conversation in the target agent. Do not try to
   run the seed command yourself.

## Notes

- All directions between `claude`, `codex`, and `opencode` are supported when
  the source agent's local session storage is available. OpenCode read support
  expects the `sqlite3` CLI to be available.
- `acs` only writes a transcript file; it never modifies any agent's sessions.
