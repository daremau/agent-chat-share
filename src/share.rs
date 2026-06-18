//! Shared implementation of the `share` and `export` flows. The CLI
//! subcommands and the TUI both call into here so they cannot drift in
//! their write behavior, the on-disk transcript shape, or the seed command
//! they emit.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::adapters::{self, Scope};
use crate::render::{self, RenderOptions};

/// Output of a successful `share` run. Carries everything the CLI prints
/// and everything the TUI needs to render a `ShareModal`.
#[derive(Debug, Clone)]
pub struct ShareResult {
    pub transcript_path: PathBuf,
    pub seed_shell: String,
    pub message_count: usize,
}

fn default_share_out_path(session_id: &str) -> PathBuf {
    PathBuf::from(".agents")
        .join("acs")
        .join("transcripts")
        .join(format!("shared-chat-{session_id}.md"))
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Read the source session, render a transcript, write it to `out` (or the
/// default path), and build the target agent's seed command. Validation of
/// adapter roles happens inside this function so callers get a typed
/// `adapters::Error` for unsupported directions.
pub fn run(
    from: &str,
    to: &str,
    session: Option<&str>,
    out: Option<PathBuf>,
) -> Result<ShareResult> {
    let source = adapters::get(from)?;
    let target = adapters::get(to)?;

    if !source.roles().read {
        return Err(adapters::Error::Unsupported {
            agent: from.to_string(),
            role: "read",
        }
        .into());
    }
    if !target.roles().seed {
        return Err(adapters::Error::Unsupported {
            agent: to.to_string(),
            role: "seed",
        }
        .into());
    }

    let scope = Scope::default();
    let conv = source.read(session, &scope)?;
    let transcript = render::render(&conv, &RenderOptions::default());

    let out_path = out.unwrap_or_else(|| default_share_out_path(&conv.metadata.id));
    ensure_parent_dir(&out_path)?;
    std::fs::write(&out_path, &transcript)?;

    let seed = target.seed_command(&out_path)?;

    Ok(ShareResult {
        transcript_path: out_path,
        seed_shell: seed.shell,
        message_count: conv.messages.len(),
    })
}

/// Output format for the `export` flow. Mirrors the CLI's `Format` enum
/// without depending on `clap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Transcript,
    Json,
}

/// Render a session and write it to `out` (or stdout if `None`). Returns
/// the resolved output path; `None` means the content went to stdout.
pub fn export(
    agent: &str,
    session: Option<&str>,
    format: ExportFormat,
    out: Option<PathBuf>,
) -> Result<Option<PathBuf>> {
    let adapter = adapters::get(agent)?;
    let scope = Scope::default();
    let conv = adapter.read(session, &scope)?;

    let content = match format {
        ExportFormat::Transcript => render::render(&conv, &RenderOptions::default()),
        ExportFormat::Json => conv.to_json()?,
    };

    match out {
        Some(path) => {
            ensure_parent_dir(&path)?;
            std::fs::write(&path, &content)?;
            Ok(Some(path))
        }
        None => {
            print!("{content}");
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::default_share_out_path;
    use std::path::PathBuf;

    #[test]
    fn default_share_output_goes_under_agent_state() {
        assert_eq!(
            default_share_out_path("abc123"),
            PathBuf::from(".agents/acs/transcripts/shared-chat-abc123.md")
        );
    }
}
