//! Codex target adapter. Emits a `codex` seed command that frames a rendered
//! transcript as a prior conversation to continue. It never writes into Codex's
//! session storage or its SQLite session index. Reading Codex is not implemented.

use std::path::{Path, PathBuf};

use super::{Adapter, Error, Result, Roles, Scope, SeedCommand, SessionRef};
use crate::model::Conversation;

/// Prompt prefix that tells Codex to continue the attached transcript.
const SEED_PREFIX: &str =
    "Continue this prior conversation from another coding assistant. Pick up where it left off. Transcript follows:";

pub struct CodexAdapter;

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexAdapter {
    pub fn new() -> Self {
        CodexAdapter
    }
}

impl Adapter for CodexAdapter {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn roles(&self) -> Roles {
        Roles {
            read: false,
            seed: true,
        }
    }

    fn storage_root(&self) -> Result<PathBuf> {
        // Seed-only: storage is irrelevant and intentionally not touched.
        super::resolve_agent_dir("ACS_CODEX_HOME", ".codex")
            .ok_or_else(|| Error::Other(anyhow::anyhow!("cannot resolve codex home")))
    }

    fn discover(&self, _scope: &Scope) -> Result<Vec<SessionRef>> {
        Err(Error::Unsupported {
            agent: "codex".to_string(),
            role: "read",
        })
    }

    fn read(&self, _id: Option<&str>, _scope: &Scope) -> Result<Conversation> {
        Err(Error::Unsupported {
            agent: "codex".to_string(),
            role: "read",
        })
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
        let err = a.read(None, &Scope::default()).unwrap_err();
        assert!(matches!(err, Error::Unsupported { role: "read", .. }));
    }
}
