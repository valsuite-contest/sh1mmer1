//! Kernel Archive command - Archive kernel partitions from a shim image

use anyhow::{bail, Result};
use clap::Args;
use std::path::PathBuf;
use std::fs;

use crate::gpt::{self, Gpt, partition_types};
use crate::utils::{self, log_info};

/// Arguments for the kernarchive command
#[derive(Args, Debug)]
pub struct KernArchiveArgs {
    /// Source device or image
    pub source: PathBuf,
    
    /// Output archive file
    pub output: PathBuf,
}

/// Run the kernarchive command
pub fn run(args: KernArchiveArgs, debug: bool) -> Result<()> {
    let missing_deps = utils::check_deps(&["sfdisk", "sgdisk", "tar"]);
    if !missing_deps.is_empty() {
        bail!(
            "The following required commands weren't found in PATH:\n{}",
            missing_deps.join("\n")
        );
    }

    let source = &args.source;
    
    if !source.exists() || (!source.is_file() && !utils::is_block_device(source)) {
        bail!("{} doesn't exist or is not a file or block device", source.display());
    }
    
    // Check read permissions
    if fs::metadata(source).is_err() {
        bail!("Cannot read {}, try running as root?", source.display());
    }
    
    if !gpt::check_gpt_image(source)? {
        bail!("{} is not GPT, or is corrupted", source.display());
    }

    let gpt = Gpt::load_from_file(source)?;
    let sector_size = gpt.block_size;

    // Create temp directory for extracted kernels
    let tmp_dir = tempfile::tempdir()?;
    
    // Set ownership if running as sudo
    let user = std::env::var("SUDO_USER").ok();

    // Extract kernel partitions
    for (part_num, partition) in gpt.partitions.iter().enumerate() {
        if partition.is_unused() {
            continue;
        }
        
        let type_str = partition.type_guid.to_string().to_uppercase();
        if type_str != partition_types::CHROMEOS_KERNEL {
            continue;
        }
        
        let sectors = partition.size_in_sectors();
        if sectors <= 1 {
            continue;
        }
        
        let start = partition.first_lba;
        let part_number = part_num + 1;
        
        let name = if partition.name.is_empty() {
            format!("{}.bin", part_number)
        } else {
            format!("{}.{}.bin", part_number, partition.name)
        };
        
        let filename = tmp_dir.path().join(&name);
        
        // Create file
        fs::File::create(&filename)?;
        if let Some(ref user) = user {
            utils::shell("chown", &[&format!("{}:{}", user, user), filename.to_str().unwrap()], true, true)?;
        }
        
        log_info(&format!("Extracting kernel partition {} -> {}", part_number, name));
        
        // Extract using dd
        utils::shell(
            "dd",
            &[
                &format!("if={}", source.display()),
                &format!("of={}", filename.display()),
                &format!("bs={}", sector_size),
                &format!("skip={}", start),
                &format!("count={}", sectors),
                "status=progress",
            ],
            true,
            !debug,
        )?;
    }

    // Create tar archive
    log_info(&format!("Creating archive: {}", args.output.display()));
    utils::shell(
        "tar",
        &["-cf", args.output.to_str().unwrap(), "-C", tmp_dir.path().to_str().unwrap(), "."],
        false,
        !debug,
    )?;

    log_info("Done.");
    Ok(())
}
