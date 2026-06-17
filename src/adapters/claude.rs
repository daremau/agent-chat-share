//! Claude Code adapter. Reads `~/.claude/projects/<encoded-cwd>/<id>.jsonl`
//! session logs into the canonical model, and emits a transcript seed command
//! for Claude as a target.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};

use super::{Adapter, Error, Result, Roles, Scope, SeedCommand, SessionRef};
use crate::model::{Block, Conversation, Message, Metadata, Role};

/// Environment variable Claude Code sets to the current session id.
const ENV_SESSION: &str = "CLAUDE_CODE_SESSION_ID";
/// Environment override for the Claude home directory (testing).
const ENV_HOME: &str = "ACS_CLAUDE_HOME";
/// Prompt prefix that tells Claude to continue the attached transcript.
const SEED_PREFIX: &str =
    "Continue this prior conversation from another coding assistant. Pick up where it left off. Transcript follows:";

pub struct ClaudeAdapter {
    /// Claude home directory (`~/.claude` by default).
    home: PathBuf,
}

impl Default for ClaudeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeAdapter {
    pub fn new() -> Self {
        let home = super::resolve_agent_dir(ENV_HOME, ".claude")
            .unwrap_or_else(|| PathBuf::from(".claude"));
        Self { home }
    }

    /// Construct against an explicit home (used by tests).
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    fn projects_dir(&self) -> PathBuf {
        self.home.join("projects")
    }

    /// The per-project session directory for a working directory. Claude encodes
    /// the absolute path by replacing `/` and `.` with `-`.
    fn project_dir(&self, cwd: &Path) -> PathBuf {
        self.projects_dir().join(encode_cwd(cwd))
    }
}

/// Encode an absolute working directory the way Claude Code names its project
/// folders: every `/` and `.` becomes `-`.
pub fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

impl Adapter for ClaudeAdapter {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn roles(&self) -> Roles {
        Roles {
            read: true,
            seed: true,
        }
    }

    fn storage_root(&self) -> Result<PathBuf> {
        Ok(self.projects_dir())
    }

    fn discover(&self, scope: &Scope) -> Result<Vec<SessionRef>> {
        let cwd = scope.effective_cwd()?;
        let dir = self.project_dir(&cwd);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut refs = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let id = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            // Parse to build an accurate summary and message count.
            let conv = match parse_session_file(&path) {
                Ok(c) => c,
                Err(_) => continue, // skip unreadable session files
            };
            refs.push(SessionRef {
                id,
                summary: summarize(&conv),
                updated_at: conv.metadata.updated_at,
                message_count: conv.messages.len(),
            });
        }
        // Newest first.
        refs.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        Ok(refs)
    }

    fn read(&self, id: Option<&str>, scope: &Scope) -> Result<Conversation> {
        let cwd = scope.effective_cwd()?;
        let dir = self.project_dir(&cwd);

        // Resolve which session: explicit id > env current session > newest.
        let resolved: String = if let Some(id) = id {
            id.to_string()
        } else if let Ok(env_id) = std::env::var(ENV_SESSION) {
            if env_id.is_empty() {
                newest_id(self, scope)?
            } else {
                env_id
            }
        } else {
            newest_id(self, scope)?
        };

        let path = dir.join(format!("{resolved}.jsonl"));
        if !path.exists() {
            return Err(Error::Other(anyhow!(
                "claude session {resolved:?} not found at {}",
                path.display()
            )));
        }
        parse_session_file(&path).map_err(Error::Other)
    }

    fn seed_command(&self, transcript: &Path) -> Result<SeedCommand> {
        Ok(SeedCommand {
            program: "claude".to_string(),
            shell: build_seed_shell(transcript),
        })
    }
}

fn build_seed_shell(transcript: &Path) -> String {
    let path = transcript.display().to_string();
    let quoted_path = format!("'{}'", path.replace('\'', r"'\''"));
    format!("claude \"{SEED_PREFIX} $(cat {quoted_path})\"")
}

fn newest_id(adapter: &ClaudeAdapter, scope: &Scope) -> Result<String> {
    adapter
        .discover(scope)?
        .into_iter()
        .next()
        .map(|r| r.id)
        .ok_or_else(|| Error::NoSession("claude".to_string()))
}

fn summarize(conv: &Conversation) -> String {
    for msg in &conv.messages {
        if msg.role == Role::User {
            for block in &msg.blocks {
                if let Block::Text { text } = block {
                    let t = text.trim();
                    if !t.is_empty() {
                        return truncate_summary(t);
                    }
                }
            }
        }
    }
    "(no user text)".to_string()
}

fn truncate_summary(s: &str) -> String {
    const MAX: usize = 80;
    let one_line = s.replace('\n', " ");
    if one_line.chars().count() <= MAX {
        one_line
    } else {
        let prefix: String = one_line.chars().take(MAX - 1).collect();
        format!("{prefix}…")
    }
}

/// Parse a Claude Code session JSONL file into the canonical model.
pub fn parse_session_file(path: &Path) -> anyhow::Result<Conversation> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading claude session {}", path.display()))?;
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    parse_session_str(&text, &id)
}

/// Parse Claude session JSONL content (one event per line) into a conversation.
pub fn parse_session_str(text: &str, id: &str) -> anyhow::Result<Conversation> {
    let mut messages = Vec::new();
    let mut cwd: Option<String> = None;
    let mut first_ts: Option<DateTime<Utc>> = None;
    let mut last_ts: Option<DateTime<Utc>> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // tolerate non-JSON / partial lines
        };

        if cwd.is_none() {
            if let Some(c) = value.get("cwd").and_then(|v| v.as_str()) {
                cwd = Some(c.to_string());
            }
        }
        if let Some(ts) = value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        {
            let ts = ts.with_timezone(&Utc);
            first_ts.get_or_insert(ts);
            last_ts = Some(ts);
        }

        let msg = match value.get("message") {
            Some(m) if m.is_object() => m,
            _ => continue,
        };
        let role_str = match msg.get("role").and_then(|v| v.as_str()) {
            Some(r) => r,
            None => continue,
        };
        let role = match Role::from_source(role_str) {
            Ok(r) => r,
            Err(_) => continue, // ignore roles we don't model (keeps parsing robust)
        };

        let blocks = parse_content(msg.get("content"));
        if blocks.is_empty() {
            continue;
        }
        messages.push(Message { role, blocks });
    }

    let metadata = Metadata::new(id, "claude", cwd, first_ts, last_ts);
    Ok(Conversation { metadata, messages })
}

/// Convert a Claude `message.content` value (string or block array) into
/// canonical blocks.
fn parse_content(content: Option<&serde_json::Value>) -> Vec<Block> {
    let mut blocks = Vec::new();
    match content {
        Some(serde_json::Value::String(s)) => {
            if !s.trim().is_empty() {
                blocks.push(Block::Text { text: s.clone() });
            }
        }
        Some(serde_json::Value::Array(items)) => {
            for item in items {
                if let Some(block) = parse_block(item) {
                    blocks.push(block);
                }
            }
        }
        _ => {}
    }
    blocks
}

fn parse_block(item: &serde_json::Value) -> Option<Block> {
    let ty = item.get("type").and_then(|v| v.as_str())?;
    match ty {
        "text" => {
            let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if text.trim().is_empty() {
                None
            } else {
                Some(Block::Text {
                    text: text.to_string(),
                })
            }
        }
        "tool_use" => Some(Block::ToolCall {
            id: item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            name: item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("tool")
                .to_string(),
            input: item
                .get("input")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        }),
        "tool_result" => Some(Block::ToolResult {
            call_id: item
                .get("tool_use_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            output: stringify_result(item.get("content")),
        }),
        other => Some(Block::Passthrough {
            source_type: other.to_string(),
            raw: item.clone(),
        }),
    }
}

/// Flatten a tool_result `content` (string, or array of text blocks) to a string.
fn stringify_result(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|i| {
                i.get("text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        if i.is_string() {
                            i.as_str().map(|s| s.to_string())
                        } else {
                            None
                        }
                    })
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_cwd_matches_claude_scheme() {
        assert_eq!(
            encode_cwd(Path::new("/home/darem/dev/agent-chat-share")),
            "-home-darem-dev-agent-chat-share"
        );
        assert_eq!(
            encode_cwd(Path::new("/home/darem/.claude-mem")),
            "-home-darem--claude-mem"
        );
    }

    #[test]
    fn parses_messages_tools_and_links() {
        let jsonl = r#"
{"type":"user","timestamp":"2026-01-01T00:00:00Z","cwd":"/tmp/p","message":{"role":"user","content":"refactor auth"}}
{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"On it."},{"type":"tool_use","id":"toolu_1","name":"Edit","input":{"file":"auth.ts"}}]}}
{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"done"}]}]}}
"#;
        let conv = parse_session_str(jsonl, "sess1").unwrap();
        assert_eq!(conv.metadata.id, "sess1");
        assert_eq!(conv.metadata.source_agent, "claude");
        assert_eq!(conv.metadata.cwd.as_deref(), Some("/tmp/p"));
        assert_eq!(conv.messages.len(), 3);
        assert_eq!(conv.messages[0].role, Role::User);
        assert_eq!(conv.messages[1].role, Role::Assistant);

        // Tool call and its result share the call id (linking).
        let call_id = match &conv.messages[1].blocks[1] {
            Block::ToolCall { id, name, .. } => {
                assert_eq!(name, "Edit");
                id.clone()
            }
            other => panic!("expected tool call, got {other:?}"),
        };
        match &conv.messages[2].blocks[0] {
            Block::ToolResult {
                call_id: cid,
                output,
            } => {
                assert_eq!(cid, &call_id);
                assert_eq!(output, "done");
            }
            other => panic!("expected tool result, got {other:?}"),
        }
    }

    #[test]
    fn seed_command_references_transcript_and_uses_claude() {
        let a = ClaudeAdapter::with_home("/nonexistent");
        let cmd = a.seed_command(Path::new("/tmp/x.md")).unwrap();
        assert_eq!(cmd.program, "claude");
        assert!(cmd.shell.starts_with("claude "));
        assert!(cmd.shell.contains("/tmp/x.md"));
        assert!(cmd.shell.contains("$(cat"));
    }
}
