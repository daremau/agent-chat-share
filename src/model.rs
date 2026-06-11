//! The canonical, agent-neutral conversation model and its versioned JSON
//! interchange format. This is the hub every adapter reads into and the
//! transcript renderer reads from.

use anyhow::{bail, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// Current interchange schema version. Bumped on breaking model changes.
pub const SCHEMA_VERSION: u32 = 1;

/// Deterministic default timestamp used when a source omits one. Recorded in
/// [`Metadata::inferred`] so callers can tell it was not real.
pub fn default_timestamp() -> DateTime<Utc> {
    Utc.timestamp_opt(0, 0).single().expect("epoch is valid")
}

/// Speaker role of a message. Roles outside this set are rejected at parse time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

impl Role {
    /// Map a source-provided role string to a canonical [`Role`], erroring on
    /// anything unsupported rather than silently dropping the message.
    pub fn from_source(raw: &str) -> Result<Role> {
        match raw {
            "user" => Ok(Role::User),
            "assistant" => Ok(Role::Assistant),
            "system" => Ok(Role::System),
            other => bail!("unsupported message role: {other:?}"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System => "System",
        }
    }
}

/// A typed unit of message content. Anything the model cannot type is preserved
/// as [`Block::Passthrough`] so no source data is silently lost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        call_id: String,
        output: String,
    },
    Passthrough {
        source_type: String,
        raw: serde_json::Value,
    },
}

/// One message: a role plus ordered content blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub blocks: Vec<Block>,
}

/// Conversation-level metadata. Optional fields that the source omits are
/// filled with deterministic defaults and noted in `inferred`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub id: String,
    pub source_agent: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Names of fields whose value was defaulted/inferred rather than sourced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inferred: Vec<String>,
}

impl Metadata {
    /// Construct metadata, defaulting timestamps deterministically and recording
    /// which were inferred.
    pub fn new(
        id: impl Into<String>,
        source_agent: impl Into<String>,
        cwd: Option<String>,
        created_at: Option<DateTime<Utc>>,
        updated_at: Option<DateTime<Utc>>,
    ) -> Metadata {
        let mut inferred = Vec::new();
        let created_at = created_at.unwrap_or_else(|| {
            inferred.push("created_at".to_string());
            default_timestamp()
        });
        let updated_at = updated_at.unwrap_or_else(|| {
            inferred.push("updated_at".to_string());
            default_timestamp()
        });
        Metadata {
            id: id.into(),
            source_agent: source_agent.into(),
            cwd,
            created_at,
            updated_at,
            inferred,
        }
    }
}

/// A full conversation: metadata + ordered messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub metadata: Metadata,
    pub messages: Vec<Message>,
}

/// On-disk interchange wrapper carrying the schema version.
#[derive(Debug, Serialize, Deserialize)]
struct Interchange {
    schema_version: u32,
    #[serde(flatten)]
    conversation: Conversation,
}

impl Conversation {
    /// Serialize to the versioned JSON interchange format.
    pub fn to_json(&self) -> Result<String> {
        let wrapped = Interchange {
            schema_version: SCHEMA_VERSION,
            conversation: self.clone(),
        };
        Ok(serde_json::to_string_pretty(&wrapped)?)
    }

    /// Parse from the versioned JSON interchange format, rejecting unknown
    /// schema versions and unsupported roles (the latter via serde).
    pub fn from_json(input: &str) -> Result<Conversation> {
        // Read the version first so we can give a precise error.
        let probe: serde_json::Value = serde_json::from_str(input)?;
        match probe.get("schema_version").and_then(|v| v.as_u64()) {
            Some(v) if v == SCHEMA_VERSION as u64 => {}
            Some(found) => bail!(
                "unsupported interchange schema_version {found}; this build supports {SCHEMA_VERSION}"
            ),
            None => bail!("interchange is missing required field `schema_version`"),
        }
        let wrapped: Interchange = serde_json::from_str(input)?;
        Ok(wrapped.conversation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Conversation {
        Conversation {
            metadata: Metadata::new(
                "abc",
                "claude",
                Some("/tmp/proj".to_string()),
                Some(Utc.timestamp_opt(1000, 0).single().unwrap()),
                Some(Utc.timestamp_opt(2000, 0).single().unwrap()),
            ),
            messages: vec![
                Message {
                    role: Role::User,
                    blocks: vec![Block::Text {
                        text: "hi".to_string(),
                    }],
                },
                Message {
                    role: Role::Assistant,
                    blocks: vec![
                        Block::ToolCall {
                            id: "t1".to_string(),
                            name: "Bash".to_string(),
                            input: serde_json::json!({ "command": "ls" }),
                        },
                        Block::Passthrough {
                            source_type: "thinking".to_string(),
                            raw: serde_json::json!({ "text": "hmm" }),
                        },
                    ],
                },
                Message {
                    role: Role::User,
                    blocks: vec![Block::ToolResult {
                        call_id: "t1".to_string(),
                        output: "a\nb".to_string(),
                    }],
                },
            ],
        }
    }

    #[test]
    fn round_trip_preserves_conversation() {
        let conv = sample();
        let json = conv.to_json().unwrap();
        let back = Conversation::from_json(&json).unwrap();
        assert_eq!(conv, back);
    }

    #[test]
    fn passthrough_preserved() {
        let conv = sample();
        let back = Conversation::from_json(&conv.to_json().unwrap()).unwrap();
        let has_passthrough = back.messages[1].blocks.iter().any(
            |b| matches!(b, Block::Passthrough { source_type, .. } if source_type == "thinking"),
        );
        assert!(has_passthrough);
    }

    #[test]
    fn unknown_role_rejected() {
        let mut v: serde_json::Value = serde_json::from_str(&sample().to_json().unwrap()).unwrap();
        v["messages"][0]["role"] = serde_json::json!("robot");
        let err = Conversation::from_json(&v.to_string()).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("robot") || err.to_string().contains("variant")
        );
    }

    #[test]
    fn unsupported_version_rejected() {
        let mut v: serde_json::Value = serde_json::from_str(&sample().to_json().unwrap()).unwrap();
        v["schema_version"] = serde_json::json!(999);
        let err = Conversation::from_json(&v.to_string()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("999"),
            "error should name found version: {msg}"
        );
        assert!(
            msg.contains(&SCHEMA_VERSION.to_string()),
            "error should name supported version: {msg}"
        );
    }

    #[test]
    fn missing_timestamp_inferred_deterministically() {
        let m = Metadata::new("x", "claude", None, None, None);
        assert_eq!(m.created_at, default_timestamp());
        assert!(m.inferred.contains(&"created_at".to_string()));
    }
}
