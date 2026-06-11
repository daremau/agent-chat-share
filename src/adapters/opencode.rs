//! OpenCode adapter scaffold. OpenCode now stores sessions in a SQLite database
//! (`~/.local/share/opencode/opencode.db`, Drizzle-migrated `session`/`message`/
//! `part` tables); the old file-based `storage/` tree is legacy. A real read
//! adapter must query that database. Until implemented, all roles are
//! unsupported and this scaffold touches no OpenCode storage.

use std::path::{Path, PathBuf};

use super::{Adapter, Error, Result, Roles, Scope, SeedCommand, SessionRef};
use crate::model::Conversation;

pub struct OpenCodeAdapter;

impl Default for OpenCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenCodeAdapter {
    pub fn new() -> Self {
        OpenCodeAdapter
    }
}

impl Adapter for OpenCodeAdapter {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn roles(&self) -> Roles {
        Roles {
            read: false,
            seed: false,
        }
    }

    fn storage_root(&self) -> Result<PathBuf> {
        super::resolve_agent_dir("ACS_OPENCODE_HOME", ".local/share/opencode")
            .ok_or_else(|| Error::Other(anyhow::anyhow!("cannot resolve opencode storage")))
    }

    fn discover(&self, _scope: &Scope) -> Result<Vec<SessionRef>> {
        Err(self.unsupported("read"))
    }

    fn read(&self, _id: Option<&str>, _scope: &Scope) -> Result<Conversation> {
        Err(self.unsupported("read"))
    }

    fn seed_command(&self, _transcript: &Path) -> Result<SeedCommand> {
        Err(self.unsupported("seed"))
    }
}

impl OpenCodeAdapter {
    fn unsupported(&self, role: &'static str) -> Error {
        Error::Unsupported {
            agent: "opencode".to_string(),
            role,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_roles_unsupported() {
        let a = OpenCodeAdapter::new();
        assert!(matches!(
            a.read(None, &Scope::default()).unwrap_err(),
            Error::Unsupported { role: "read", .. }
        ));
        assert!(matches!(
            a.seed_command(Path::new("/tmp/x.md")).unwrap_err(),
            Error::Unsupported { role: "seed", .. }
        ));
    }
}
