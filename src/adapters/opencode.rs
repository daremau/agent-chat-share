//! OpenCode adapter. OpenCode stores sessions in a SQLite database
//! (`~/.local/share/opencode/opencode.db`, Drizzle-migrated `session`/`message`/
//! `part` tables). This adapter reads those rows through the installed
//! `sqlite3` CLI and emits an `opencode --prompt` seed command as a target. It
//! never imports transcripts or writes OpenCode storage.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::anyhow;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

use super::{Adapter, Error, Result, Roles, Scope, SeedCommand, SessionRef};
use crate::model::{default_timestamp, Block, Conversation, Message, Metadata, Role};

/// Environment override for the OpenCode home directory (testing).
const ENV_HOME: &str = "ACS_OPENCODE_HOME";
/// Prompt prefix that tells OpenCode to continue the attached transcript.
const SEED_PREFIX: &str =
    "Continue this prior conversation from another coding assistant. Pick up where it left off. Transcript follows:";

pub struct OpenCodeAdapter {
    /// OpenCode data directory (`~/.local/share/opencode` by default).
    home: PathBuf,
}

impl Default for OpenCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenCodeAdapter {
    pub fn new() -> Self {
        let home = super::resolve_agent_dir(ENV_HOME, ".local/share/opencode")
            .unwrap_or_else(|| PathBuf::from(".local/share/opencode"));
        OpenCodeAdapter { home }
    }

    /// Construct against an explicit home (used by tests).
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    fn db_path(&self) -> PathBuf {
        self.home.join("opencode.db")
    }
}

impl Adapter for OpenCodeAdapter {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn roles(&self) -> Roles {
        Roles {
            read: true,
            seed: true,
        }
    }

    fn storage_root(&self) -> Result<PathBuf> {
        Ok(self.home.clone())
    }

    fn discover(&self, scope: &Scope) -> Result<Vec<SessionRef>> {
        let cwd = scope.effective_cwd()?.to_string_lossy().to_string();
        let sql = format!(
            "SELECT s.id, s.directory, s.title, s.time_created, s.time_updated, \
             COUNT(DISTINCT m.id) AS message_count \
             FROM session s \
             LEFT JOIN message m ON m.session_id = s.id \
             WHERE s.directory = {} \
             GROUP BY s.id \
             ORDER BY s.time_updated DESC",
            sql_quote(&cwd)
        );
        let rows: Vec<SessionRow> = sqlite_json(&self.db_path(), &sql)?;
        Ok(rows
            .into_iter()
            .map(|row| SessionRef {
                id: row.id,
                summary: row.title,
                updated_at: millis_to_datetime(row.time_updated),
                message_count: row.message_count.unwrap_or(0) as usize,
            })
            .collect())
    }

    fn read(&self, id: Option<&str>, scope: &Scope) -> Result<Conversation> {
        let session_id = match id {
            Some(id) => id.to_string(),
            None => self
                .discover(scope)?
                .into_iter()
                .next()
                .map(|r| r.id)
                .ok_or_else(|| Error::NoSession("opencode".to_string()))?,
        };

        let session_sql = format!(
            "SELECT id, directory, title, time_created, time_updated, 0 AS message_count \
             FROM session WHERE id = {} LIMIT 1",
            sql_quote(&session_id)
        );
        let mut sessions: Vec<SessionRow> = sqlite_json(&self.db_path(), &session_sql)?;
        let session = sessions
            .pop()
            .ok_or_else(|| Error::Other(anyhow!("opencode session {session_id:?} not found")))?;

        let message_sql = format!(
            "SELECT m.id AS message_id, m.data AS message_data, p.data AS part_data \
             FROM message m \
             LEFT JOIN part p ON p.message_id = m.id \
             WHERE m.session_id = {} \
             ORDER BY m.time_created, p.time_created, p.id",
            sql_quote(&session_id)
        );
        let rows: Vec<MessagePartRow> = sqlite_json(&self.db_path(), &message_sql)?;
        conversation_from_rows(session, rows).map_err(Error::Other)
    }

    fn seed_command(&self, transcript: &Path) -> Result<SeedCommand> {
        Ok(SeedCommand {
            program: "opencode".to_string(),
            shell: build_seed_shell(transcript),
        })
    }
}

fn build_seed_shell(transcript: &Path) -> String {
    let path = transcript.display().to_string();
    let quoted_path = format!("'{}'", path.replace('\'', r"'\''"));
    format!("opencode --prompt \"{SEED_PREFIX} $(cat {quoted_path})\"")
}

#[derive(Debug, Deserialize)]
struct SessionRow {
    id: String,
    directory: String,
    title: String,
    time_created: i64,
    time_updated: i64,
    #[serde(default)]
    message_count: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MessagePartRow {
    message_id: String,
    message_data: String,
    part_data: Option<String>,
}

fn sqlite_json<T>(db: &Path, sql: &str) -> Result<Vec<T>>
where
    T: for<'de> Deserialize<'de>,
{
    let output = Command::new("sqlite3")
        .arg("-json")
        .arg(db)
        .arg(sql)
        .output()
        .map_err(|e| Error::Other(anyhow!("running sqlite3: {e}")))?;
    if !output.status.success() {
        return Err(Error::Other(anyhow!(
            "sqlite3 failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    if output.stdout.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_slice(&output.stdout).map_err(Error::from)
}

fn sql_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn millis_to_datetime(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(default_timestamp)
}

fn conversation_from_rows(
    session: SessionRow,
    rows: Vec<MessagePartRow>,
) -> anyhow::Result<Conversation> {
    let mut messages = Vec::new();
    let mut current_id: Option<String> = None;
    let mut current_role = Role::User;
    let mut current_blocks: Vec<Block> = Vec::new();

    for row in rows {
        if current_id.as_deref() != Some(row.message_id.as_str()) {
            push_message(&mut messages, current_role, &mut current_blocks);
            current_id = Some(row.message_id.clone());
            let data: serde_json::Value = serde_json::from_str(&row.message_data)?;
            current_role = role_from_opencode(data.get("role").and_then(|v| v.as_str()))
                .unwrap_or(Role::System);
        }

        if let Some(part_data) = row.part_data {
            let part: serde_json::Value = serde_json::from_str(&part_data)?;
            current_blocks.extend(parse_part(&part));
        }
    }
    push_message(&mut messages, current_role, &mut current_blocks);

    let metadata = Metadata::new(
        session.id,
        "opencode",
        Some(session.directory),
        Some(millis_to_datetime(session.time_created)),
        Some(millis_to_datetime(session.time_updated)),
    );
    Ok(Conversation { metadata, messages })
}

fn push_message(messages: &mut Vec<Message>, role: Role, blocks: &mut Vec<Block>) {
    if !blocks.is_empty() {
        messages.push(Message {
            role,
            blocks: std::mem::take(blocks),
        });
    }
}

fn role_from_opencode(raw: Option<&str>) -> Option<Role> {
    match raw {
        Some("user") => Some(Role::User),
        Some("assistant") => Some(Role::Assistant),
        Some("system") => Some(Role::System),
        _ => None,
    }
}

fn parse_part(part: &serde_json::Value) -> Vec<Block> {
    match part.get("type").and_then(|v| v.as_str()) {
        Some("text") => part
            .get("text")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|text| {
                vec![Block::Text {
                    text: text.to_string(),
                }]
            })
            .unwrap_or_default(),
        Some("tool") => parse_tool_part(part),
        Some("reasoning") => vec![Block::Passthrough {
            source_type: "reasoning".to_string(),
            raw: part.clone(),
        }],
        Some("step-start") | Some("step-finish") => Vec::new(),
        Some(other) => vec![Block::Passthrough {
            source_type: other.to_string(),
            raw: part.clone(),
        }],
        None => Vec::new(),
    }
}

fn parse_tool_part(part: &serde_json::Value) -> Vec<Block> {
    let call_id = part
        .get("callID")
        .or_else(|| part.get("call_id"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name = part
        .get("tool")
        .or_else(|| part.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("tool")
        .to_string();
    let state = part.get("state").unwrap_or(&serde_json::Value::Null);
    let input = state
        .get("input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let mut blocks = vec![Block::ToolCall {
        id: call_id.clone(),
        name,
        input,
    }];

    if let Some(output) = state.get("output").or_else(|| state.get("error")) {
        blocks.push(Block::ToolResult {
            call_id,
            output: stringify_json_or_string(output),
        });
    }
    blocks
}

fn stringify_json_or_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_read_and_seed() {
        let a = OpenCodeAdapter::new();
        assert!(a.roles().read);
        assert!(a.roles().seed);
    }

    #[test]
    fn seed_command_references_transcript_and_uses_opencode() {
        let a = OpenCodeAdapter::new();
        let cmd = a.seed_command(Path::new("/tmp/shared-chat.md")).unwrap();
        assert_eq!(cmd.program, "opencode");
        assert!(cmd.shell.starts_with("opencode "));
        assert!(cmd.shell.contains("--prompt"));
        assert!(cmd.shell.contains("/tmp/shared-chat.md"));
        assert!(cmd.shell.contains("$(cat"));
    }

    #[test]
    fn parses_message_and_part_rows() {
        let session = SessionRow {
            id: "ses_1".to_string(),
            directory: "/tmp/p".to_string(),
            title: "Title".to_string(),
            time_created: 1_704_067_200_000,
            time_updated: 1_704_067_201_000,
            message_count: Some(2),
        };
        let rows = vec![
            MessagePartRow {
                message_id: "m1".to_string(),
                message_data: serde_json::json!({"role":"user"}).to_string(),
                part_data: Some(serde_json::json!({"type":"text","text":"hello"}).to_string()),
            },
            MessagePartRow {
                message_id: "m2".to_string(),
                message_data: serde_json::json!({"role":"assistant"}).to_string(),
                part_data: Some(
                    serde_json::json!({
                        "type":"tool",
                        "tool":"bash",
                        "callID":"call_1",
                        "state":{"input":{"cmd":"ls"},"output":"ok"}
                    })
                    .to_string(),
                ),
            },
        ];
        let conv = conversation_from_rows(session, rows).unwrap();
        assert_eq!(conv.metadata.id, "ses_1");
        assert_eq!(conv.metadata.source_agent, "opencode");
        assert_eq!(conv.messages.len(), 2);
        assert_eq!(conv.messages[0].role, Role::User);
        assert!(matches!(
            conv.messages[0].blocks[0],
            Block::Text { ref text } if text == "hello"
        ));
        assert!(matches!(
            conv.messages[1].blocks[0],
            Block::ToolCall { ref name, .. } if name == "bash"
        ));
        assert!(matches!(
            conv.messages[1].blocks[1],
            Block::ToolResult { ref output, .. } if output == "ok"
        ));
    }
}
