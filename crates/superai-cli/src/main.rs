//! Layer 4: interface. Placeholder CLI until the GPUI interface exists;
//! nothing below this crate knows an interface exists.

mod usage;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use superai_core::Registry;

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Core(#[from] superai_core::CoreError),
    #[error("{reason}")]
    Args { reason: String },
    #[error("usage scan failed: {reason}")]
    Scan { reason: String },
    #[error("usage window math failed: {reason}")]
    Window { reason: String },
    #[error("usage pricing failed: {reason}")]
    Pricing { reason: String },
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("superai: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Dispatch: the first argument names a subcommand; no argument keeps the
/// instance printer. New top-level commands (for example `superai vps`)
/// join this match, never a rewrite of it.
fn run(args: &[OsString]) -> Result<(), CliError> {
    let home = home_dir().ok_or_else(|| {
        CliError::Core(superai_core::CoreError::Validation {
            field: "home".to_owned(),
            reason: "neither HOME nor USERPROFILE is set".to_owned(),
        })
    })?;
    let recovery = superai_core::failure::recover_pending(&home)?;
    for journal in &recovery.journals {
        println!("recovered: {}", journal.outcome);
    }

    match args.split_first() {
        None => list_instances(),
        Some((command, rest)) if command == "usage" => usage::run(rest, &home),
        Some((command, _)) => Err(CliError::Args {
            reason: format!(
                "unknown command `{}`; available: usage",
                bounded(command.to_string_lossy().as_ref())
            ),
        }),
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn list_instances() -> Result<(), CliError> {
    let path = Registry::default_path()?;
    let registry = Registry::load(&path)?;

    println!("records: {}", path.display());
    if registry.instances().is_empty() {
        println!("no instances recorded");
        return Ok(());
    }
    for instance in registry.instances() {
        let template = instance
            .template
            .as_ref()
            .map_or_else(|| "-".to_owned(), |t| format!("{}@{}", t.name, t.version));
        println!(
            "{}  {}  {}  {}",
            instance.name,
            instance.harness,
            instance.config_root.as_path().display(),
            template
        );
    }
    Ok(())
}

/// Cap any user-supplied text echoed back in an error so nothing unbounded
/// is printed; the cap respects UTF-8 char boundaries.
pub(crate) fn bounded(text: &str) -> String {
    const LIMIT: usize = 64;
    text.chars().take(LIMIT).collect()
}
