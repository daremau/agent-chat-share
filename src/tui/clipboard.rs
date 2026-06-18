//! Best-effort copy to the system clipboard by shelling out to whatever
//! tool is installed. Returns the tool that succeeded, or an error naming
//! everything we tried so the status line can report it honestly.

use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{anyhow, Result};

/// Clipboard writers to try, in priority order: Wayland, X11, then macOS.
const CANDIDATES: &[(&str, &[&str])] = &[
    ("wl-copy", &[]),
    ("xclip", &["-selection", "clipboard"]),
    ("xsel", &["--clipboard", "--input"]),
    ("pbcopy", &[]),
];

/// Copy `text` to the system clipboard. Returns the name of the tool used.
pub fn copy(text: &str) -> Result<&'static str> {
    let mut last_err: Option<String> = None;
    for (tool, args) in CANDIDATES {
        match try_copy(tool, args, text) {
            Ok(()) => return Ok(tool),
            Err(e) => last_err = Some(e.to_string()),
        }
    }
    Err(anyhow!(
        "no clipboard tool found (tried wl-copy, xclip, xsel, pbcopy){}",
        last_err
            .map(|e| format!(": {e}"))
            .unwrap_or_default()
    ))
}

fn try_copy(tool: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(tool)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Dropping the borrowed stdin at the end of this statement closes the
    // pipe, signalling EOF so the tool stops reading and exits.
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("{tool} exposed no stdin"))?
        .write_all(text.as_bytes())?;
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{tool} exited with {status}"))
    }
}
