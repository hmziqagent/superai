//! Layer 3 — interface. A placeholder CLI until the GPUI interface exists;
//! nothing below this crate knows an interface exists at all.

use std::process::ExitCode;

use superai_core::Registry;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("superai: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> superai_core::Result<()> {
    // Startup crash recovery (MUT-09): finish or roll back any operation
    // abandoned mid-transaction before anything else runs. Recovery inspects
    // the actual filesystem state and never replays stale writes.
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .ok_or_else(|| superai_core::CoreError::Validation {
            field: "home".to_owned(),
            reason: "neither HOME nor USERPROFILE is set".to_owned(),
        })?;
    let recovery = superai_core::failure::recover_pending(&home)?;
    for journal in &recovery.journals {
        println!("recovered: {}", journal.outcome);
    }

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
