//! Wax4Web Bundler command - Bundle wax4web for distribution

use anyhow::{bail, Result};
use clap::Args;
use std::path::PathBuf;
use std::fs;

use crate::utils::{self, log_info};

/// Arguments for the wax4web_bundler command
#[derive(Args, Debug)]
pub struct Wax4WebBundlerArgs {
    /// Working directory (default: wax directory)
    #[arg(short, long)]
    pub workdir: Option<PathBuf>,
}

/// Run the wax4web_bundler command
pub fn run(args: Wax4WebBundlerArgs, debug: bool) -> Result<()> {
    let missing_deps = utils::check_deps(&["tar", "zip", "split"]);
    if !missing_deps.is_empty() {
        bail!(
            "The following required commands weren't found in PATH:\n{}",
            missing_deps.join("\n")
        );
    }

    let wax_dir = if let Some(dir) = args.workdir {
        dir
    } else {
        find_wax_dir()?
    };
    
    if !wax_dir.exists() {
        bail!("Wax directory not found: {}", wax_dir.display());
    }

    let wax4web_dir = wax_dir.parent()
        .map(|p| p.join("wax4web"))
        .unwrap_or_else(|| wax_dir.join("../wax4web"));

    // Remove old files
    let tar_file = wax4web_dir.join("wax4web.tar");
    let zip_file = wax4web_dir.join("wax4web.tar.zip");
    let split_dir = wax4web_dir.join("wax4web_tar_zip");
    
    let _ = fs::remove_file(&tar_file);
    let _ = fs::remove_file(&zip_file);
    let _ = fs::remove_dir_all(&split_dir);

    log_info("Creating wax4web bundle...");

    // Create tar archive
    let files_to_bundle = vec![
        "wax4web_entry.sh",
        "wax.sh",
        "str1pper.sh",
        "bootstrap",
        "m1nsh1m_legacy",  // renamed from sh1mmer_legacy
        "m1nsh1m_bw",      // renamed from sh1mmer_bw
        "payloads",
        "firmware",
        "lib/wax_common.sh",
        "lib/shflags",
        "lib/bin/i386/cgpt",
    ];
    
    // Check which files exist
    let existing_files: Vec<String> = files_to_bundle
        .iter()
        .filter(|f| wax_dir.join(f).exists())
        .map(|s| s.to_string())
        .collect();

    if existing_files.is_empty() {
        bail!("No bundleable files found in {}", wax_dir.display());
    }

    // Change to wax directory for tar
    let prev_dir = std::env::current_dir()?;
    std::env::set_current_dir(&wax_dir)?;

    // Build tar command
    let mut tar_args = vec!["-cvf", tar_file.to_str().unwrap()];
    for f in &existing_files {
        tar_args.push(f);
    }
    tar_args.push("--owner=0");
    tar_args.push("--group=0");
    
    utils::shell("tar", &tar_args, false, !debug)?;

    std::env::set_current_dir(prev_dir)?;

    // Create wax4web directory if needed
    fs::create_dir_all(&wax4web_dir)?;
    fs::create_dir_all(&split_dir)?;

    // Create zip
    std::env::set_current_dir(&wax4web_dir)?;
    utils::shell("zip", &["wax4web.tar.zip", "wax4web.tar"], false, !debug)?;

    // Split into chunks
    utils::shell("split", &["-da1", "--additional-suffix=.bin", "-b", "24M", "wax4web.tar.zip", "wax4web_tar_zip/"], false, !debug)?;

    // Cleanup intermediate files
    let _ = fs::remove_file(&tar_file);
    let _ = fs::remove_file(&zip_file);

    log_info(&format!("Bundle created in {}", split_dir.display()));
    Ok(())
}

/// Find the wax directory
fn find_wax_dir() -> Result<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let wax_dir = parent.join("wax");
            if wax_dir.exists() {
                return Ok(wax_dir);
            }
            if parent.join("wax.sh").exists() {
                return Ok(parent.to_path_buf());
            }
        }
    }
    
    let cwd = std::env::current_dir()?;
    if cwd.join("wax.sh").exists() {
        return Ok(cwd);
    }
    
    if let Some(parent) = cwd.parent() {
        let wax_dir = parent.join("wax");
        if wax_dir.exists() {
            return Ok(wax_dir);
        }
    }
    
    bail!("Could not find wax directory")
}
