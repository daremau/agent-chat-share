//! Command-line interface: `list`, `export`, `share`, `tui`, and
//! `skills install|uninstall`.

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand, ValueEnum};

use crate::adapters::{self, Scope};
use crate::share::{self, ExportFormat};
use crate::skills;
use crate::tui;

#[derive(Parser)]
#[command(
    name = "acs",
    version,
    about = "Share a chat between Claude Code, Codex, and OpenCode"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the sessions available for an agent.
    List {
        #[arg(long)]
        agent: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Export a session as a transcript (default) or canonical JSON.
    Export {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long, value_enum, default_value_t = Format::Transcript)]
        format: Format,
        /// Output path; omit to write to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Read a source session and print a command to continue it in a target agent.
    Share {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        session: Option<String>,
        /// Transcript output path (default: ./.agents/acs/transcripts/shared-chat-<id>.md).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Open the interactive terminal UI.
    Tui,
    /// Install or remove the agent-chat-share skill.
    Skills {
        #[command(subcommand)]
        action: SkillsAction,
    },
}

#[derive(Subcommand)]
enum SkillsAction {
    /// Install the skill into agents' skills directories.
    Install {
        /// Limit to a single agent (default: all).
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, value_enum, default_value_t = ScopeArg::Project)]
        scope: ScopeArg,
    },
    /// Remove the agent-chat-share skill.
    Uninstall {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, value_enum, default_value_t = ScopeArg::Project)]
        scope: ScopeArg,
    },
}

#[derive(Copy, Clone, ValueEnum)]
enum Format {
    Transcript,
    Json,
}

#[derive(Copy, Clone, ValueEnum)]
enum ScopeArg {
    Project,
    User,
}

impl From<ScopeArg> for skills::Scope {
    fn from(s: ScopeArg) -> Self {
        match s {
            ScopeArg::Project => skills::Scope::Project,
            ScopeArg::User => skills::Scope::User,
        }
    }
}

/// Parse arguments and run. Returns `Err` on any failure (mapped to a non-zero
/// exit code by `main`).
pub fn run() -> Result<()> {
    dispatch(Cli::parse())
}

fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Command::List { agent, json } => cmd_list(&agent, json),
        Command::Export {
            agent,
            session,
            format,
            out,
        } => cmd_export(&agent, session.as_deref(), format, out),
        Command::Share {
            from,
            to,
            session,
            out,
        } => cmd_share(&from, &to, session.as_deref(), out),
        Command::Tui => tui::run(),
        Command::Skills { action } => cmd_skills(action),
    }
}

fn cmd_list(agent: &str, json: bool) -> Result<()> {
    let adapter = adapters::get(agent)?;
    let scope = Scope::default();
    let sessions = adapter.discover(&scope)?;

    if json {
        let arr: Vec<serde_json::Value> = sessions
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "updated_at": s.updated_at.to_rfc3339(),
                    "message_count": s.message_count,
                    "summary": s.summary,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
    } else if sessions.is_empty() {
        println!("No sessions found for agent '{agent}'.");
    } else {
        for s in &sessions {
            println!(
                "{}  {}  ({} msgs)  {}",
                s.id,
                s.updated_at.to_rfc3339(),
                s.message_count,
                s.summary
            );
        }
    }
    Ok(())
}

fn cmd_export(
    agent: &str,
    session: Option<&str>,
    format: Format,
    out: Option<PathBuf>,
) -> Result<()> {
    let shared_format = match format {
        Format::Transcript => ExportFormat::Transcript,
        Format::Json => ExportFormat::Json,
    };
    if let Some(path) = share::export(agent, session, shared_format, out)? {
        println!("Wrote {}", path.display());
    }
    Ok(())
}

fn cmd_share(from: &str, to: &str, session: Option<&str>, out: Option<PathBuf>) -> Result<()> {
    let result = share::run(from, to, session, out)?;
    println!(
        "Wrote {} ({} turns)",
        result.transcript_path.display(),
        result.message_count
    );
    println!("Run this to continue in {to}:\n");
    println!("  {}", result.seed_shell);
    Ok(())
}

fn cmd_skills(action: SkillsAction) -> Result<()> {
    let (agent, scope, install) = match action {
        SkillsAction::Install { agent, scope } => (agent, scope, true),
        SkillsAction::Uninstall { agent, scope } => (agent, scope, false),
    };

    // Validate an explicit agent name against the known set.
    let agents: Vec<&str> = match &agent {
        Some(a) => {
            if !adapters::KNOWN_AGENTS.contains(&a.as_str()) {
                bail!(
                    "unknown agent '{a}'; supported agents: {}",
                    adapters::KNOWN_AGENTS.join(", ")
                );
            }
            vec![a.as_str()]
        }
        None => adapters::KNOWN_AGENTS.to_vec(),
    };

    if install {
        let written = skills::install(&agents, scope.into())?;
        println!("Installed the {} skill:", skills::SKILL_NAME);
        for p in written {
            println!("  {}", p.display());
        }
    } else {
        let removed = skills::uninstall(&agents, scope.into())?;
        if removed.is_empty() {
            println!(
                "Nothing to remove; the {} skill was not installed.",
                skills::SKILL_NAME
            );
        } else {
            println!("Removed the {} skill:", skills::SKILL_NAME);
            for p in removed {
                println!("  {}", p.display());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // default_share_out_path coverage lives next to the helper in
    // crate::share. Keeping the test here would require re-exporting a
    // private function, so we just keep this module as a placeholder for
    // future CLI-shape tests.
}
