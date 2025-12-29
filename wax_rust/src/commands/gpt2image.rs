//! GPT to Image command - Convert block device to GPT image file

use anyhow::{bail, Result};
use clap::Args;
use std::fs;
use std::path::PathBuf;

use crate::gpt::{self, Gpt};
use crate::utils::{self, log_info};

const BLOCK_SIZE: u64 = 4 * 1024 * 1024; // 4 MiB

/// Arguments for the gpt2image command
#[derive(Args, Debug)]
pub struct Gpt2ImageArgs {
    /// Source block device
    pub device: PathBuf,
    
    /// Output image file
    pub output: PathBuf,
    
    /// Skip confirmation
    #[arg(short = 'y', long)]
    pub yes: bool,
}

/// Run the gpt2image command
pub fn run(args: Gpt2ImageArgs, debug: bool) -> Result<()> {
    let missing_deps = utils::check_deps(&["sfdisk", "sgdisk"]);
    if !missing_deps.is_empty() {
        bail!(
            "The following required commands weren't found in PATH:\n{}",
            missing_deps.join("\n")
        );
    }

    let device = &args.device;
    
    if !utils::is_block_device(device) {
        bail!("{} doesn't exist or is not a block device", device.display());
    }
    
    // Check read permissions
    if fs::metadata(device).is_err() {
        bail!("Cannot read {}, try running as root?", device.display());
    }
    
    if !gpt::check_gpt_image(device)? {
        bail!("{} is not GPT, or is corrupted", device.display());
    }

    // Calculate image size
    let gpt = Gpt::load_from_file(device)?;
    let sector_size = gpt.block_size;
    let final_sector = gpt.get_final_sector();
    let buffer: u64 = 35;
    let end_bytes = (final_sector + buffer) * sector_size;
    let dd_count = ((final_sector + 1) * sector_size + BLOCK_SIZE - 1) / BLOCK_SIZE;

    log_info(&format!("Image will be {}", utils::format_bytes(end_bytes)));

    if !args.yes {
        print!("Is this ok? (y/N): ");
        use std::io::Write;
        std::io::stdout().flush()?;
        
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        
        if input.trim().to_lowercase() != "y" {
            println!("Abort.");
            return Ok(());
        }
    }

    // Create output file
    fs::File::create(&args.output)?;
    
    // Set ownership if running as sudo
    if let Ok(user) = std::env::var("SUDO_USER") {
        utils::shell("chown", &[&format!("{}:{}", user, user), args.output.to_str().unwrap()], true, true)?;
    }

    // Copy image using dd
    log_info("Copying image...");
    utils::shell(
        "dd",
        &[
            &format!("if={}", device.display()),
            &format!("of={}", args.output.display()),
            &format!("bs={}", BLOCK_SIZE),
            &format!("count={}", dd_count),
            "conv=sync",
            "status=progress",
        ],
        true,
        false,
    )?;

    // Truncate to exact size
    utils::truncate_file(&args.output, end_bytes)?;
    
    // Fix GPT backup table
    utils::shell("sgdisk", &["-e", args.output.to_str().unwrap()], true, !debug)?;

    log_info("Done.");
    Ok(())
}
