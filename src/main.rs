//! Thin binary entry point: run the CLI and map any error to a non-zero exit
//! code, printing a human-readable message to stderr.

use std::process::ExitCode;

fn main() -> ExitCode {
    match acs::cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
