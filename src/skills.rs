//! Install/uninstall the agent-chat-share skill into each agent's skills
//! directory. The skill markdown is embedded in the binary, so installing needs
//! no external files.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Folder name the skill is installed under.
pub const SKILL_NAME: &str = "agent-chat-share";
/// The embedded skill markdown.
pub const SKILL_BODY: &str = include_str!("skills/assets/SKILL.md");

/// Where to install: the current project directory, or the agent's user home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Project,
    User,
}

/// Resolve the skills directory (parent of the skill folder) for an agent.
fn skills_dir(agent: &str, scope: Scope) -> Result<PathBuf> {
    match scope {
        Scope::Project => Ok(std::env::current_dir()?
            .join(format!(".{agent}"))
            .join("skills")),
        Scope::User => {
            let home = directories::BaseDirs::new()
                .context("cannot resolve home directory")?
                .home_dir()
                .to_path_buf();
            let dir = match agent {
                // OpenCode reads user config/skills from ~/.config/opencode.
                "opencode" => home.join(".config").join("opencode").join("skills"),
                other => home.join(format!(".{other}")).join("skills"),
            };
            Ok(dir)
        }
    }
}

/// Install the skill for the given agents at the given scope. Idempotent:
/// re-installing overwrites the skill body with the current version.
/// Returns the SKILL.md paths written.
pub fn install(agents: &[&str], scope: Scope) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for agent in agents {
        let dir = skills_dir(agent, scope)?;
        written.push(install_into(&dir)?);
    }
    Ok(written)
}

/// Install the skill into an explicit skills directory (project-style). Used by
/// the project-scope path and by tests.
pub fn install_into(skills_dir: &Path) -> Result<PathBuf> {
    let folder = skills_dir.join(SKILL_NAME);
    fs::create_dir_all(&folder)
        .with_context(|| format!("creating skill dir {}", folder.display()))?;
    let file = folder.join("SKILL.md");
    fs::write(&file, SKILL_BODY).with_context(|| format!("writing {}", file.display()))?;
    Ok(file)
}

/// Remove only the agent-chat-share skill for the given agents/scope. Returns
/// the folders actually removed (empty if nothing was installed).
pub fn uninstall(agents: &[&str], scope: Scope) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for agent in agents {
        let dir = skills_dir(agent, scope)?;
        if let Some(p) = uninstall_from(&dir)? {
            removed.push(p);
        }
    }
    Ok(removed)
}

/// Remove the skill folder from an explicit skills directory, if present.
pub fn uninstall_from(skills_dir: &Path) -> Result<Option<PathBuf>> {
    let folder = skills_dir.join(SKILL_NAME);
    if folder.exists() {
        fs::remove_dir_all(&folder).with_context(|| format!("removing {}", folder.display()))?;
        Ok(Some(folder))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "acs-skills-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    #[test]
    fn install_writes_skill_and_is_idempotent() {
        let base = tmp();
        let dir = base.join(".claude").join("skills");
        let f1 = install_into(&dir).unwrap();
        assert!(f1.exists());
        assert_eq!(fs::read_to_string(&f1).unwrap(), SKILL_BODY);
        // Re-install: still one file, content current.
        let f2 = install_into(&dir).unwrap();
        assert_eq!(f1, f2);
        let entries: Vec<_> = fs::read_dir(dir.join(SKILL_NAME)).unwrap().collect();
        assert_eq!(entries.len(), 1);
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn uninstall_removes_only_our_skill() {
        let base = tmp();
        let dir = base.join(".codex").join("skills");
        // A sibling skill that must be preserved.
        let other = dir.join("some-other-skill");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("SKILL.md"), "keep me").unwrap();

        install_into(&dir).unwrap();
        let removed = uninstall_from(&dir).unwrap();
        assert!(removed.is_some());
        assert!(!dir.join(SKILL_NAME).exists());
        // Sibling untouched.
        assert!(other.join("SKILL.md").exists());

        // Uninstalling again removes nothing.
        assert!(uninstall_from(&dir).unwrap().is_none());
        fs::remove_dir_all(&base).ok();
    }
}
