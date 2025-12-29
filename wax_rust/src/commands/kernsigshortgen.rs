//! Kernel Signature Short Gen command - Generate short kernel signature list

use anyhow::{bail, Result};
use clap::Args;
use std::path::PathBuf;
use std::fs;
use std::io::{BufRead, BufReader};

use crate::utils::log_info;

/// Arguments for the kernsigshortgen command
#[derive(Args, Debug)]
pub struct KernSigShortGenArgs {
    /// Optional output file (default: stdout)
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

/// Run the kernsigshortgen command
pub fn run(args: KernSigShortGenArgs, _debug: bool) -> Result<()> {
    let keyfile = find_keyfile()?;
    
    let file = fs::File::open(&keyfile)?;
    let reader = BufReader::new(file);
    
    let mut output_lines = Vec::new();
    
    for line in reader.lines() {
        let line = line?;
        // Strip the last field (the base64 key data)
        if let Some(stripped) = line.rsplit_once(';') {
            output_lines.push(stripped.0.to_string());
        }
    }
    
    if let Some(output_path) = args.output {
        fs::write(&output_path, output_lines.join("\n") + "\n")?;
        log_info(&format!("Written to {}", output_path.display()));
    } else {
        for line in output_lines {
            println!("{}", line);
        }
    }
    
    Ok(())
}

/// Find the key file
fn find_keyfile() -> Result<PathBuf> {
    // Check relative to executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let keyfile = parent.join("lib/mp-reco-2024-05-03.txt");
            if keyfile.exists() {
                return Ok(keyfile);
            }
            let keyfile = parent.join("mp-reco-2024-05-03.txt");
            if keyfile.exists() {
                return Ok(keyfile);
            }
        }
    }
    
    // Check current directory
    let cwd = std::env::current_dir()?;
    let keyfile = cwd.join("lib/mp-reco-2024-05-03.txt");
    if keyfile.exists() {
        return Ok(keyfile);
    }
    
    let keyfile = cwd.join("wax/lib/mp-reco-2024-05-03.txt");
    if keyfile.exists() {
        return Ok(keyfile);
    }
    
    bail!("Could not find required key list");
}
