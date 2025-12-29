//! GPT Truncate command - Truncate GPT images to minimal size

use anyhow::{bail, Result};
use clap::Args;
use std::path::PathBuf;

use crate::gpt::{self, Gpt};
use crate::utils::{self, log_info, log_warn};

/// Arguments for the gpttruncate command
#[derive(Args, Debug)]
pub struct GptTruncateArgs {
    /// GPT image file(s) to truncate
    #[arg(required = true)]
    pub images: Vec<PathBuf>,
}

/// Run the gpttruncate command
pub fn run(args: GptTruncateArgs, debug: bool) -> Result<()> {
    let missing_deps = utils::check_deps(&["sfdisk", "sgdisk"]);
    if !missing_deps.is_empty() {
        bail!(
            "The following required commands weren't found in PATH:\n{}",
            missing_deps.join("\n")
        );
    }

    for image in &args.images {
        if let Err(e) = truncate_image_verbose(image, debug) {
            log_warn(&format!("ERROR truncating {}: {}", image.display(), e));
        }
    }

    log_info("Done.");
    Ok(())
}

/// Truncate a single image with verbose output
fn truncate_image_verbose(image: &PathBuf, debug: bool) -> Result<()> {
    if !utils::check_file_rw(image) {
        bail!("{} doesn't exist, isn't a file, or isn't RW", image.display());
    }
    
    if !gpt::check_gpt_image(image)? {
        bail!("{} is not GPT, or is corrupted", image.display());
    }

    let old_bytes = std::fs::metadata(image)?.len();
    
    let gpt = Gpt::load_from_file(image)?;
    let sector_size = gpt.block_size;
    let final_sector = gpt.get_final_sector();
    let buffer: u64 = 35;
    let end_bytes = (final_sector + buffer) * sector_size;

    if old_bytes == end_bytes {
        log_info(&format!("{} is already adequately truncated.", image.display()));
        return Ok(());
    }

    if old_bytes == end_bytes - sector_size {
        log_info(&format!(
            "{} is one sector smaller than the ideal size. Still safe.",
            image.display()
        ));
        return Ok(());
    }

    if old_bytes < end_bytes {
        log_warn(&format!(
            "WARNING: the image is smaller than the ideal size. It may be corrupted."
        ));
    }

    log_info(&format!(
        "Truncating {} to {}",
        image.display(),
        utils::format_bytes(end_bytes)
    ));

    utils::truncate_file(image, end_bytes)?;
    utils::shell("sgdisk", &["-e", image.to_str().unwrap()], true, !debug)?;

    Ok(())
}
