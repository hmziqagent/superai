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
