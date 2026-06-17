//! End-to-end: read a Claude fixture session, render a transcript, and produce a
//! Codex seed command — without spawning any agent or touching agent storage.

use std::fs;
use std::path::PathBuf;

use acs::adapters::{Adapter, ClaudeAdapter, CodexAdapter, Scope};
use acs::render::{render, RenderOptions};

/// Create a temporary Claude home with one session under the encoded project dir.
fn setup_claude_home(cwd: &std::path::Path) -> (PathBuf, String) {
    let mut home = std::env::temp_dir();
    home.push(format!(
        "acs-it-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let encoded = acs::adapters::claude::encode_cwd(cwd);
    let proj = home.join("projects").join(&encoded);
    fs::create_dir_all(&proj).unwrap();

    let session_id = "sess-int-1";
    let jsonl = r#"{"type":"user","timestamp":"2026-02-01T10:00:00Z","cwd":"CWD","message":{"role":"user","content":"refactor the auth module"}}
{"type":"assistant","timestamp":"2026-02-01T10:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"Reading the file."},{"type":"tool_use","id":"toolu_9","name":"Edit","input":{"file":"auth.ts","change":"add guard"}}]}}
{"type":"user","timestamp":"2026-02-01T10:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_9","content":[{"type":"text","text":"edit applied"}]}]}}
{"type":"assistant","timestamp":"2026-02-01T10:00:03Z","message":{"role":"assistant","content":"Done — added the guard clause."}}
"#
    .replace("CWD", &cwd.to_string_lossy());
    fs::write(proj.join(format!("{session_id}.jsonl")), jsonl).unwrap();
    (home, session_id.to_string())
}

#[test]
fn share_claude_to_codex_end_to_end() {
    let cwd = PathBuf::from("/tmp/acs-int-project");
    let (home, session_id) = setup_claude_home(&cwd);

    // Source: read the Claude session into the canonical model.
    let claude = ClaudeAdapter::with_home(&home);
    let scope = Scope {
        cwd: Some(cwd.clone()),
    };
    let conv = claude.read(Some(&session_id), &scope).unwrap();
    assert_eq!(conv.metadata.source_agent, "claude");
    assert_eq!(conv.messages.len(), 4);

    // Render a transcript and write it out.
    let transcript = render(&conv, &RenderOptions::default());
    assert!(transcript.contains("## User"));
    assert!(transcript.contains("refactor the auth module"));
    assert!(transcript.contains("Tool call:") && transcript.contains("Edit"));
    assert!(transcript.contains("edit applied"));

    let mut out = std::env::temp_dir();
    out.push(format!("acs-transcript-{}.md", std::process::id()));
    fs::write(&out, &transcript).unwrap();

    // Target: Codex emits a runnable seed command referencing the transcript.
    let codex_home = home.join("codex-home");
    let codex = CodexAdapter::with_home(&codex_home);
    let seed = codex.seed_command(&out).unwrap();
    assert!(seed.shell.starts_with("codex "));
    assert!(seed.shell.contains(&out.display().to_string()));

    // Codex storage was never created/written by acs.
    assert!(!codex_home.exists());

    fs::remove_dir_all(&home).ok();
    fs::remove_file(&out).ok();
}
