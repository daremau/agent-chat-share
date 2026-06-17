//! Codex adapter. Reads Codex rollout JSONL files from
//! `~/.codex/sessions/**/rollout-*.jsonl` into the canonical model, and emits a
//! `codex` seed command that frames a rendered transcript as a prior
//! conversation to continue. The target half never writes into Codex's session
//! storage or SQLite indexes.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};

use super::{Adapter, Error, Result, Roles, Scope, SeedCommand, SessionRef};
use crate::model::{Block, Conversation, Message, Metadata, Role};

/// Prompt prefix that tells Codex to continue the attached transcript.
const SEED_PREFIX: &str =
    "Continue this prior conversation from another coding assistant. Pick up where it left off. Transcript follows:";
/// Environment override for the Codex home directory (testing).
const ENV_HOME: &str = "ACS_CODEX_HOME";

pub struct CodexAdapter {
    /// Codex home directory (`~/.codex` by default).
    home: PathBuf,
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexAdapter {
    pub fn new() -> Self {
        let home =
            super::resolve_agent_dir(ENV_HOME, ".codex").unwrap_or_else(|| PathBuf::from(".codex"));
        CodexAdapter { home }
    }

    /// Construct against an explicit home (used by tests).
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    fn sessions_dir(&self) -> PathBuf {
        self.home.join("sessions")
    }
}

impl Adapter for CodexAdapter {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn roles(&self) -> Roles {
        Roles {
            read: true,
            seed: true,
        }
    }

    fn storage_root(&self) -> Result<PathBuf> {
        Ok(self.sessions_dir())
    }

    fn discover(&self, scope: &Scope) -> Result<Vec<SessionRef>> {
        let cwd = scope.effective_cwd()?;
        let cwd = cwd.to_string_lossy().to_string();
        let mut refs = Vec::new();
        for path in session_files(&self.sessions_dir())? {
            let conv = match parse_session_file(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if !cwd_matches(conv.metadata.cwd.as_deref(), &cwd) {
                continue;
            }
            refs.push(SessionRef {
                id: conv.metadata.id.clone(),
                summary: summarize(&conv),
                updated_at: conv.metadata.updated_at,
                message_count: conv.messages.len(),
            });
        }
        refs.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        Ok(refs)
    }

    fn read(&self, id: Option<&str>, scope: &Scope) -> Result<Conversation> {
        let resolved = match id {
            Some(id) => id.to_string(),
            None => self
                .discover(scope)?
                .into_iter()
                .next()
                .map(|r| r.id)
                .ok_or_else(|| Error::NoSession("codex".to_string()))?,
        };

        for path in session_files(&self.sessions_dir())? {
            if file_matches_id(&path, &resolved) {
                return parse_session_file(&path).map_err(Error::Other);
            }
        }
        for path in session_files(&self.sessions_dir())? {
            let conv = match parse_session_file(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if conv.metadata.id == resolved {
                return Ok(conv);
            }
        }
        Err(Error::Other(anyhow!(
            "codex session {resolved:?} not found under {}",
            self.sessions_dir().display()
        )))
    }

    fn seed_command(&self, transcript: &Path) -> Result<SeedCommand> {
        let shell = build_seed_shell(transcript);
        Ok(SeedCommand {
            program: "codex".to_string(),
            shell,
        })
    }
}

/// Build the shell line that starts Codex with the transcript inlined via
/// `$(cat …)`, so the (potentially large) transcript is not baked into argv.
fn build_seed_shell(transcript: &Path) -> String {
    // Single-quote the path for `cat`; embed in a double-quoted prompt so the
    // command substitution expands at run time.
    let path = transcript.display().to_string();
    let quoted_path = format!("'{}'", path.replace('\'', r"'\''"));
    format!("codex \"{SEED_PREFIX} $(cat {quoted_path})\"")
}

fn cwd_matches(session_cwd: Option<&str>, cwd: &str) -> bool {
    match session_cwd {
        Some(c) => c == cwd,
        None => false,
    }
}

fn file_matches_id(path: &Path, id: &str) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|stem| stem == id || stem.ends_with(id))
        .unwrap_or(false)
}

fn session_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect_session_files(root, &mut out)?;
    Ok(out)
}

fn collect_session_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_session_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    Ok(())
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
        format!("{prefix}...")
    }
}

/// Parse a Codex rollout JSONL file into the canonical model.
pub fn parse_session_file(path: &Path) -> anyhow::Result<Conversation> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading codex session {}", path.display()))?;
    let fallback_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    parse_session_str(&text, fallback_id)
}

/// Parse Codex rollout JSONL content (one event per line) into a conversation.
pub fn parse_session_str(text: &str, fallback_id: &str) -> anyhow::Result<Conversation> {
    let mut id = fallback_id.to_string();
    let mut cwd: Option<String> = None;
    let mut first_ts: Option<DateTime<Utc>> = None;
    let mut last_ts: Option<DateTime<Utc>> = None;
    let mut messages = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(ts) = parse_top_level_timestamp(&value) {
            first_ts.get_or_insert(ts);
            last_ts = Some(ts);
        }

        let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let payload = match value.get("payload") {
            Some(p) if p.is_object() => p,
            _ => continue,
        };

        match event_type {
            "session_meta" => {
                if let Some(raw_id) = payload.get("id").and_then(|v| v.as_str()) {
                    id = raw_id.to_string();
                }
                if cwd.is_none() {
                    cwd = payload
                        .get("cwd")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
            }
            "turn_context" => {
                if cwd.is_none() {
                    cwd = payload
                        .get("cwd")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
            }
            "response_item" => {
                if let Some(msg) = parse_response_item(payload) {
                    messages.push(msg);
                }
            }
            _ => {}
        }
    }

    let metadata = Metadata::new(id, "codex", cwd, first_ts, last_ts);
    Ok(Conversation { metadata, messages })
}

fn parse_top_level_timestamp(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|ts| ts.with_timezone(&Utc))
}

fn parse_response_item(payload: &serde_json::Value) -> Option<Message> {
    match payload.get("type").and_then(|v| v.as_str()) {
        Some("message") => {
            let role = role_from_codex(payload.get("role").and_then(|v| v.as_str())?)?;
            let blocks = parse_content(payload.get("content"));
            if blocks.is_empty() {
                None
            } else {
                Some(Message { role, blocks })
            }
        }
        Some("function_call") => Some(Message {
            role: Role::Assistant,
            blocks: vec![Block::ToolCall {
                id: payload
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                name: payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool")
                    .to_string(),
                input: parse_arguments(payload.get("arguments")),
            }],
        }),
        Some("function_call_output") => Some(Message {
            role: Role::User,
            blocks: vec![Block::ToolResult {
                call_id: payload
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                output: stringify_output(payload.get("output")),
            }],
        }),
        Some("reasoning") => parse_reasoning(payload),
        _ => None,
    }
}

fn role_from_codex(raw: &str) -> Option<Role> {
    match raw {
        "user" => Some(Role::User),
        "assistant" => Some(Role::Assistant),
        "system" | "developer" => Some(Role::System),
        _ => None,
    }
}

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
                if let Some(block) = parse_content_item(item) {
                    blocks.push(block);
                }
            }
        }
        _ => {}
    }
    blocks
}

fn parse_content_item(item: &serde_json::Value) -> Option<Block> {
    let ty = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match ty {
        "input_text" | "output_text" | "text" => item
            .get("text")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|text| Block::Text {
                text: text.to_string(),
            }),
        other if !other.is_empty() => Some(Block::Passthrough {
            source_type: other.to_string(),
            raw: item.clone(),
        }),
        _ => None,
    }
}

fn parse_arguments(value: Option<&serde_json::Value>) -> serde_json::Value {
    match value {
        Some(serde_json::Value::String(s)) => {
            serde_json::from_str(s).unwrap_or_else(|_| serde_json::Value::String(s.clone()))
        }
        Some(v) => v.clone(),
        None => serde_json::Value::Null,
    }
}

fn stringify_output(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

fn parse_reasoning(payload: &serde_json::Value) -> Option<Message> {
    let summary = payload.get("summary")?;
    Some(Message {
        role: Role::Assistant,
        blocks: vec![Block::Passthrough {
            source_type: "reasoning".to_string(),
            raw: summary.clone(),
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_command_references_transcript_and_uses_codex() {
        let a = CodexAdapter::new();
        let cmd = a.seed_command(Path::new("/tmp/shared-chat.md")).unwrap();
        assert_eq!(cmd.program, "codex");
        assert!(cmd.shell.starts_with("codex "));
        assert!(cmd.shell.contains("/tmp/shared-chat.md"));
        assert!(cmd.shell.contains("$(cat"));
    }

    #[test]
    fn read_is_unsupported() {
        let a = CodexAdapter::new();
        assert!(a.roles().read);
        assert!(a.roles().seed);
    }

    #[test]
    fn parses_codex_rollout_messages_and_tools() {
        let jsonl = r#"
{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"sess-codex","cwd":"/tmp/p"}}
{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"port this"}]}}
{"timestamp":"2026-01-01T00:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"reading"}]}}
{"timestamp":"2026-01-01T00:00:03Z","type":"response_item","payload":{"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"ls\"}"}}
{"timestamp":"2026-01-01T00:00:04Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":"ok"}}
"#;
        let conv = parse_session_str(jsonl, "fallback").unwrap();
        assert_eq!(conv.metadata.id, "sess-codex");
        assert_eq!(conv.metadata.cwd.as_deref(), Some("/tmp/p"));
        assert_eq!(conv.messages.len(), 4);
        assert_eq!(conv.messages[0].role, Role::User);
        assert_eq!(conv.messages[1].role, Role::Assistant);
        assert!(matches!(
            conv.messages[2].blocks[0],
            Block::ToolCall { ref name, .. } if name == "exec_command"
        ));
        assert!(matches!(
            conv.messages[3].blocks[0],
            Block::ToolResult { ref output, .. } if output == "ok"
        ));
    }
}
