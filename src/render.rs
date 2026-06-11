//! Render a canonical conversation into a portable Markdown transcript suitable
//! for seeding a target agent. Tool activity is flattened to readable prose so
//! the transcript does not depend on the target having the source's tools;
//! oversized tool output is truncated, and the whole transcript can be reduced
//! to fit a target context-window budget.

use crate::model::{Block, Conversation, Message};

/// Rendering options.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Maximum characters for a single tool result / passthrough payload.
    pub per_item_chars: usize,
    /// Optional overall transcript budget (characters). When exceeded, the
    /// oldest turns are omitted (with a marker), keeping the most recent.
    pub budget_chars: Option<usize>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            per_item_chars: 2000,
            budget_chars: None,
        }
    }
}

/// Render a conversation to a Markdown transcript.
pub fn render(conv: &Conversation, opts: &RenderOptions) -> String {
    let header = render_header(conv);

    if conv.messages.is_empty() {
        return format!("{header}\n_(no messages in this conversation)_\n");
    }

    let blocks: Vec<String> = conv
        .messages
        .iter()
        .map(|m| render_message(m, opts))
        .collect();

    let body = match opts.budget_chars {
        Some(budget) => fit_to_budget(&blocks, budget),
        None => blocks.join("\n\n"),
    };

    format!("{header}\n{body}\n")
}

fn render_header(conv: &Conversation) -> String {
    format!(
        "# Shared conversation (from {})\n\n\
         The following is a prior conversation you should continue. \
         Treat it as context and pick up where it left off.\n",
        conv.metadata.source_agent
    )
}

fn render_message(msg: &Message, opts: &RenderOptions) -> String {
    let mut out = format!("## {}\n", msg.role.label());
    for block in &msg.blocks {
        out.push('\n');
        out.push_str(&render_block(block, opts));
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn render_block(block: &Block, opts: &RenderOptions) -> String {
    match block {
        Block::Text { text } => text.clone(),
        Block::ToolCall { name, input, .. } => {
            let summary = summarize_input(input);
            if summary.is_empty() {
                format!("> 🔧 **Tool call:** `{name}`")
            } else {
                format!("> 🔧 **Tool call:** `{name}` — {summary}")
            }
        }
        Block::ToolResult { output, .. } => {
            let shown = truncate(output, opts.per_item_chars);
            format!("> ↳ **Result:** {shown}")
        }
        Block::Passthrough { source_type, raw } => {
            let shown = truncate(&raw.to_string(), opts.per_item_chars);
            format!("> _[{source_type}]_ {shown}")
        }
    }
}

/// Summarize a tool-call input object into a short, readable key list.
fn summarize_input(input: &serde_json::Value) -> String {
    match input {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, v)| {
                let val = match v {
                    serde_json::Value::String(s) => truncate(s, 120),
                    other => truncate(&other.to_string(), 120),
                };
                format!("{k}: {val}")
            })
            .collect::<Vec<_>>()
            .join(", "),
        serde_json::Value::Null => String::new(),
        other => truncate(&other.to_string(), 120),
    }
}

/// Truncate a string to `max` characters, appending an explicit marker noting
/// how many characters were omitted.
fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max).collect();
    let omitted = count - max;
    format!("{kept}\n…[truncated {omitted} chars]")
}

/// Keep the most recent rendered turns that fit within `budget`, omitting older
/// ones with a marker. The most recent turn is always included (even if it alone
/// exceeds the budget) so the continuation has its immediate context.
fn fit_to_budget(blocks: &[String], budget: usize) -> String {
    let mut kept_rev: Vec<&String> = Vec::new();
    let mut total = 0usize;
    for block in blocks.iter().rev() {
        let cost = block.chars().count() + 2; // +2 for the joining separator
        if kept_rev.is_empty() || total + cost <= budget {
            kept_rev.push(block);
            total += cost;
        } else {
            break;
        }
    }
    let omitted = blocks.len() - kept_rev.len();
    kept_rev.reverse();
    let body = kept_rev
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if omitted > 0 {
        format!("_…[{omitted} earlier turn(s) omitted to fit the context budget]…_\n\n{body}")
    } else {
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Block, Conversation, Message, Metadata, Role};

    fn conv(messages: Vec<Message>) -> Conversation {
        Conversation {
            metadata: Metadata::new("id", "claude", None, None, None),
            messages,
        }
    }

    fn text_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            blocks: vec![Block::Text { text: text.into() }],
        }
    }

    #[test]
    fn ordered_and_role_labeled() {
        let c = conv(vec![
            text_msg(Role::User, "first"),
            text_msg(Role::Assistant, "second"),
        ]);
        let out = render(&c, &RenderOptions::default());
        assert!(out.contains("prior conversation"));
        assert!(out.contains("## User"));
        assert!(out.contains("## Assistant"));
        // Order preserved: User appears before Assistant.
        assert!(out.find("## User").unwrap() < out.find("## Assistant").unwrap());
    }

    #[test]
    fn tool_activity_flattened_to_prose() {
        let c = conv(vec![Message {
            role: Role::Assistant,
            blocks: vec![
                Block::ToolCall {
                    id: "t1".into(),
                    name: "Edit".into(),
                    input: serde_json::json!({ "file": "auth.ts" }),
                },
                Block::ToolResult {
                    call_id: "t1".into(),
                    output: "ok".into(),
                },
            ],
        }]);
        let out = render(&c, &RenderOptions::default());
        assert!(out.contains("Tool call:"));
        assert!(out.contains("Edit"));
        assert!(out.contains("file: auth.ts"));
        assert!(out.contains("Result:"));
        // No raw structured tool-call JSON leaks into the transcript.
        assert!(!out.contains("\"tool_use\""));
    }

    #[test]
    fn large_output_truncated_with_marker() {
        let big = "x".repeat(5000);
        let c = conv(vec![Message {
            role: Role::User,
            blocks: vec![Block::ToolResult {
                call_id: "t".into(),
                output: big,
            }],
        }]);
        let out = render(
            &c,
            &RenderOptions {
                per_item_chars: 100,
                budget_chars: None,
            },
        );
        assert!(out.contains("[truncated"));
    }

    #[test]
    fn over_budget_reduced_with_marker() {
        let msgs: Vec<Message> = (0..10)
            .map(|i| {
                text_msg(
                    Role::User,
                    &format!("turn number {i} with some padding text"),
                )
            })
            .collect();
        let c = conv(msgs);
        let out = render(
            &c,
            &RenderOptions {
                per_item_chars: 2000,
                budget_chars: Some(120),
            },
        );
        assert!(out.contains("earlier turn(s) omitted"));
        // Most recent turn retained.
        assert!(out.contains("turn number 9"));
    }

    #[test]
    fn within_budget_full_render() {
        let c = conv(vec![text_msg(Role::User, "short")]);
        let out = render(
            &c,
            &RenderOptions {
                per_item_chars: 2000,
                budget_chars: Some(10_000),
            },
        );
        assert!(!out.contains("omitted"));
        assert!(out.contains("short"));
    }

    #[test]
    fn empty_conversation_is_valid() {
        let c = conv(vec![]);
        let out = render(&c, &RenderOptions::default());
        assert!(out.contains("no messages"));
    }
}
