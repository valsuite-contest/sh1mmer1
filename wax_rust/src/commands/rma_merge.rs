//! RMA Merge command - Merge multiple RMA shim images

use anyhow::{bail, Result};
use clap::Args;
use std::path::PathBuf;

use crate::utils::{self, log_info};

/// Arguments for the rma_merge command
#[derive(Args, Debug)]
pub struct RmaMergeArgs {
    /// Output merged image path
    #[arg(short, long)]
    pub output: PathBuf,
    
    /// Input RMA images to merge
    #[arg(short, long, required = true, num_args = 2..)]
    pub images: Vec<PathBuf>,
    
    /// Automatically resolve duplicate boards
    #[arg(short = 'a', long)]
    pub auto_select: bool,
    
    /// Force overwrite existing output
    #[arg(short, long)]
    pub force: bool,
}

/// Run the rma_merge command
pub fn run(args: RmaMergeArgs, debug: bool) -> Result<()> {
    // Check if output exists
    if args.output.exists() && !args.force {
        bail!("Output already exists (add -f to overwrite): {}", args.output.display());
    }
    
    if args.images.len() < 2 {
        bail!("Need > 1 input image files to merge.");
    }

    // Verify all input images exist
    for image in &args.images {
        if !image.exists() {
            bail!("Input image does not exist: {}", image.display());
        }
    }

    log_info(&format!("Scanning {} input image files...", args.images.len()));

    // This command delegates to the Python image_tool.py
    // Find the image_tool script
    let script_dir = find_script_dir()?;
    let image_tool = script_dir.join("lib/py/image_tool.py");
    
    if !image_tool.exists() {
        bail!("Could not find image_tool.py at {}", image_tool.display());
    }

    // Build command arguments
    let mut cmd_args = vec![
        image_tool.to_str().unwrap().to_string(),
        "rma".to_string(),
        "merge".to_string(),
        "-o".to_string(),
        args.output.to_str().unwrap().to_string(),
    ];
    
    if args.force {
        cmd_args.push("-f".to_string());
    }
    
    if args.auto_select {
        cmd_args.push("-a".to_string());
    }
    
    cmd_args.push("-i".to_string());
    for image in &args.images {
        cmd_args.push(image.to_str().unwrap().to_string());
    }

    // Execute Python script
    let args_str: Vec<&str> = cmd_args.iter().map(|s| s.as_str()).collect();
    utils::shell("python3", &args_str[..], true, !debug)?;

    log_info(&format!("OK: Merged successfully in new image: {}", args.output.display()));
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
