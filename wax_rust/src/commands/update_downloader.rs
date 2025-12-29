//! Update Downloader command - Download ChromeOS update payloads

use anyhow::{bail, Result};
use clap::Args;
use std::path::PathBuf;
use std::fs;
use std::io::{BufRead, BufReader};

use crate::utils::{self, log_info};

/// Arguments for the update_downloader command
#[derive(Args, Debug)]
pub struct UpdateDownloaderArgs {
    /// Board name to download update for
    pub board: String,
    
    /// Output directory (optional)
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

/// Run the update_downloader command
pub fn run(args: UpdateDownloaderArgs, debug: bool) -> Result<()> {
    let missing_deps = utils::check_deps(&["curl", "git", "python3", "protoc", "gzip"]);
    if !missing_deps.is_empty() {
        bail!(
            "The following required commands weren't found in PATH:\n{}",
            missing_deps.join("\n")
        );
    }

    // Check Python dependencies
    let py_check = utils::shell("python3", &["-c", "import google.protobuf"], false, true);
    if py_check.is_err() {
        bail!("Please install the python package 'protobuf'");
    }
    
    let py_check = utils::shell("python3", &["-c", "import argparse"], false, true);
    if py_check.is_err() {
        bail!("Please install the python package 'argparse'");
    }
    
    let py_check = utils::shell("python3", &["-c", "from six.moves import zip"], false, true);
    if py_check.is_err() {
        bail!("Please install the python package 'six'");
    }

    let workdir = tempfile::tempdir()?;
    let script_dir = find_script_dir()?;
    
    // Find URL file
    let url_file = script_dir.join("lib/latest_r132.txt");
    if !url_file.exists() {
        bail!("Could not find required URL list at {}", url_file.display());
    }

    // Set up output directory
    let out_dir = args.output.unwrap_or_else(|| script_dir.join("mounted_payloads/updates"));
    fs::create_dir_all(&out_dir)?;

    // Find board in URL file
    let file = fs::File::open(&url_file)?;
    let reader = BufReader::new(file);
    
    let mut file_line = None;
    for line in reader.lines() {
        let line = line?;
        if line.starts_with(&format!("{},", args.board)) {
            file_line = Some(line);
            break;
        }
    }
    
    let file_line = file_line.ok_or_else(|| anyhow::anyhow!("board '{}' is not in board list", args.board))?;
    
    let parts: Vec<&str> = file_line.split(',').collect();
    if parts.len() < 2 {
        bail!("Invalid line format in URL file");
    }
    
    let file_name = parts[1];
    let file_version = file_name.split('_').nth(1).unwrap_or("");
    let major_version = file_version.split('.').next().unwrap_or("");
    let file_channel = file_name.split('_').nth(3).unwrap_or("");
    
    let file_url = format!(
        "https://dl.google.com/chromeos/{}/{}/{}/{}",
        args.board, file_version, file_channel, file_name
    );

    log_info("Downloading update payload...");
    let download_path = workdir.path().join(file_name);
    utils::shell("curl", &[&file_url, "-o", download_path.to_str().unwrap()], false, !debug)?;

    // Check for update_engine
    let update_engine = script_dir.join("lib/update_engine");
    if !update_engine.is_dir() {
        log_info("Downloading update_engine...");
        utils::shell("git", &[
            "clone", "-n",
            "https://chromium.googlesource.com/aosp/platform/system/update_engine",
            update_engine.to_str().unwrap(),
        ], false, !debug)?;
        
        // Checkout specific commit
        let prev_dir = std::env::current_dir()?;
        std::env::set_current_dir(&update_engine)?;
        utils::shell("git", &["checkout", "c5a026d8c9ad881ee7834eb245a447455b2788c7"], false, !debug)?;
        std::env::set_current_dir(prev_dir)?;
    }

    // Generate protobuf Python files
    utils::shell("protoc", &[
        &format!("--proto_path={}", update_engine.display()),
        &format!("--python_out={}/scripts/update_payload", update_engine.display()),
        &format!("{}/update_metadata.proto", update_engine.display()),
    ], false, !debug)?;

    log_info("Extracting update payload...");
    let kern_path = workdir.path().join("kern");
    let root_path = workdir.path().join("root");
    
    utils::shell("python3", &[
        &format!("{}/scripts/paycheck.py", update_engine.display()),
        download_path.to_str().unwrap(),
        "--part_names", "kernel", "root",
        "--out_dst_part_paths", kern_path.to_str().unwrap(), root_path.to_str().unwrap(),
    ], false, !debug)?;
    
    // Remove downloaded payload to save space
    fs::remove_file(&download_path)?;

    log_info("Compressing update payload...");
    let final_dir = out_dir.join(major_version).join(&args.board);
    fs::create_dir_all(&final_dir)?;
    
    utils::shell("gzip", &["-c", kern_path.to_str().unwrap()], false, true)?;
    utils::shell("sh", &["-c", &format!(
        "gzip -c '{}' > '{}'",
        kern_path.display(),
        final_dir.join("kern.gz").display()
    )], false, !debug)?;
    
    utils::shell("sh", &["-c", &format!(
        "gzip -c '{}' > '{}'",
        root_path.display(),
        final_dir.join("root.gz").display()
    )], false, !debug)?;

    log_info(&format!("Done. Files saved to {}", final_dir.display()));
    Ok(())
}

/// Find the script directory
fn find_script_dir() -> Result<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let wax_dir = parent.join("wax");
            if wax_dir.exists() {
                return Ok(wax_dir);
            }
            if parent.join("lib").exists() {
                return Ok(parent.to_path_buf());
            }
        }
    }
    
    let cwd = std::env::current_dir()?;
    if cwd.join("lib").exists() {
        return Ok(cwd);
    }
    
    if let Some(parent) = cwd.parent() {
        let wax_dir = parent.join("wax");
        if wax_dir.exists() {
            return Ok(wax_dir);
        }
    }
    
    Ok(cwd)
}
