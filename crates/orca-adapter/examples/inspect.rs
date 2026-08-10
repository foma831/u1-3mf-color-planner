use std::path::PathBuf;
use u1_orca_adapter::{discover_installation, inspect_macos_application};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let application_path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(discover_installation)
        .ok_or("Snapmaker Orca was not found; pass the application path")?;
    let report = inspect_macos_application(&application_path)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
