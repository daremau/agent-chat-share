//! Per-agent adapters. An adapter can act as a *source* (read its native
//! session storage into the canonical model) and/or a *target* (emit a seed
//! command that continues a rendered transcript in that agent). The target half
//! never writes into any agent's private session storage.

pub mod claude;
pub mod codex;
pub mod opencode;

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::model::Conversation;

pub use claude::ClaudeAdapter;
pub use codex::CodexAdapter;
pub use opencode::OpenCodeAdapter;

/// Stable identifiers of all known agents, in registry order.
pub const KNOWN_AGENTS: &[&str] = &["claude", "codex", "opencode"];

/// Which roles an adapter supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Roles {
    /// Can act as a source: discover + read native sessions.
    pub read: bool,
    /// Can act as a target: emit a seed command for a transcript.
    pub seed: bool,
}

/// Optional scoping for discovery/read (e.g. restrict to a working directory).
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub cwd: Option<PathBuf>,
}

impl Scope {
    /// Resolve the effective working directory: the explicit scope, else the
    /// process current directory.
    pub fn effective_cwd(&self) -> std::io::Result<PathBuf> {
        match &self.cwd {
            Some(p) => Ok(p.clone()),
            None => std::env::current_dir(),
        }
    }
}

/// A discovered session, summarized for listing/selection.
#[derive(Debug, Clone)]
pub struct SessionRef {
    pub id: String,
    pub summary: String,
    pub updated_at: DateTime<Utc>,
    pub message_count: usize,
}

/// A runnable command (rendered as a shell line) that seeds a transcript into a
/// target agent. `acs` prints this for the user to run; it never executes it.
#[derive(Debug, Clone)]
pub struct SeedCommand {
    pub program: String,
    pub shell: String,
}

/// Shared wording for every target's seed command. The transcript is referenced
/// by *path*, not inlined: an earlier design pasted the whole file into argv via
/// `$(cat …)`, which overflows the kernel's `ARG_MAX` on long conversations
/// (`bash: … Argument list too long`). Since every target is itself a coding
/// agent with file-read tools, we instead ask it to open the file, which keeps
/// argv tiny regardless of transcript size.
const SEED_PREFIX: &str =
    "Continue this prior conversation from another coding assistant. Pick up where it left off. Read the transcript at";

/// Build a seed shell line of the form `<invocation> '<prompt>'`, where the
/// prompt references `transcript` by path and the whole prompt is POSIX
/// single-quoted as one argument. `invocation` is the program plus any flags
/// (e.g. `"opencode --prompt"`).
pub fn seed_shell(invocation: &str, transcript: &Path) -> String {
    let path = transcript.display().to_string();
    let prompt = format!("{SEED_PREFIX} {path}, then continue from where it left off.");
    format!("{invocation} {}", single_quote(&prompt))
}

/// POSIX single-quote a string so it is safe as a single shell word, even if it
/// contains spaces or quotes.
fn single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Typed adapter errors. `Unsupported` and `UnknownAgent` are matched by the CLI
/// to produce clear messages and non-zero exit codes; incidental I/O and parse
/// failures are carried via `Other`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("agent '{agent}' does not support {role} (not yet supported)")]
    Unsupported { agent: String, role: &'static str },

    #[error("unknown agent '{0}'; supported agents: {1}")]
    UnknownAgent(String, String),

    #[error("no session found for agent '{0}'")]
    NoSession(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Other(e.into())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Other(e.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// The capability surface every agent implements.
pub trait Adapter {
    /// Stable agent identifier (e.g. `"claude"`).
    fn id(&self) -> &'static str;

    /// Which roles this adapter supports.
    fn roles(&self) -> Roles;

    /// Resolve the agent's on-disk storage root.
    fn storage_root(&self) -> Result<PathBuf>;

    /// Discover sessions, optionally scoped to a working directory, newest first.
    fn discover(&self, scope: &Scope) -> Result<Vec<SessionRef>>;

    /// Read a session into the canonical model. With `id == None`, resolve the
    /// current session (agent-specific; falls back to newest in scope).
    fn read(&self, id: Option<&str>, scope: &Scope) -> Result<Conversation>;

    /// Emit a seed command that continues `transcript` in this agent.
    fn seed_command(&self, transcript: &Path) -> Result<SeedCommand>;
}

/// Build the default adapter for an identifier, resolving storage from the
/// environment/home directory.
pub fn get(id: &str) -> Result<Box<dyn Adapter>> {
    match id {
        "claude" => Ok(Box::new(ClaudeAdapter::new())),
        "codex" => Ok(Box::new(CodexAdapter::new())),
        "opencode" => Ok(Box::new(OpenCodeAdapter::new())),
        other => Err(Error::UnknownAgent(
            other.to_string(),
            KNOWN_AGENTS.join(", "),
        )),
    }
}

/// Resolve an agent's home directory: an env override if set, else
/// `$HOME/<rel>`. Used so tests can point adapters at a fixture tree.
pub fn resolve_agent_dir(env_var: &str, rel: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(env_var) {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    directories::BaseDirs::new().map(|b| b.home_dir().join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_agent_lists_supported() {
        let err = match get("nope") {
            Ok(_) => panic!("expected unknown-agent error"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(msg.contains("nope"));
        assert!(msg.contains("claude") && msg.contains("codex") && msg.contains("opencode"));
    }

    #[test]
    fn known_agents_resolve() {
        for id in KNOWN_AGENTS {
            assert_eq!(get(id).unwrap().id(), *id);
        }
    }
}
