//! Stripper command - Remove payloads from shim images
//!
//! Deletes any and all payloads from partition 1 (stateful) on an RMA image
//! and squashes partitions.

use anyhow::{bail, Result};
use clap::Args;
use std::fs;
use std::path::PathBuf;

use crate::ext2;
use crate::gpt::{self, Gpt};
use crate::utils::{self, log_info, log_debug};

const STATEFUL_SIZE: u64 = 4 * 1024 * 1024; // 4 MiB

/// Arguments for the stripper command
#[derive(Args, Debug)]
pub struct StripperArgs {
    /// Path to factory shim image
    #[arg(short, long)]
    pub image: PathBuf,
}

/// Run the stripper command
pub fn run(args: StripperArgs, debug: bool) -> Result<()> {
    utils::require_root()?;
    
    let missing_deps = utils::check_deps(&["partx", "sgdisk", "mkfs.ext4"]);
    if !missing_deps.is_empty() {
        bail!(
            "The following required commands weren't found in PATH:\n{}",
            missing_deps.join("\n")
        );
    }

    let image = &args.image;
    
    if !utils::check_file_rw(image) {
        bail!("{} doesn't exist, isn't a file, or isn't RW", image.display());
    }
    
    if !gpt::check_gpt_image(image)? {
        bail!("{} is not GPT, or is corrupted", image.display());
    }
    
    utils::check_slow_fs(image)?;

    // Fix backup GPT table
    utils::shell("sgdisk", &["-e", image.to_str().unwrap()], true, !debug)?;

    // Create loop device
    log_info("Creating loop device");
    let loopdev = utils::create_loop_device(image)?;
    utils::safesync();

    let _cleanup = scopeguard::guard(loopdev.clone(), |dev: String| {
        let _ = cleanup_loop_device(&dev);
    });

    // Check if stateful has cros_payloads
    let has_cros_payloads = check_stateful(&loopdev)?;
    
    if !has_cros_payloads {
        // Shrink partitions 4-7 to 1 sector each
        for i in 4..=7 {
            utils::shell("cgpt", &["add", &loopdev, "-i", &i.to_string(), "-s", "1"], true, true)?;
            utils::shell("partx", &["-u", "-n", &i.to_string(), &loopdev], true, true)?;
        }
    }

    // Delete partition 1
    utils::shell("sfdisk", &["--delete", image.to_str().unwrap(), "1"], true, !debug)?;

    // Squash partitions
    squash_partitions(&loopdev)?;
    utils::safesync();

    // Recreate stateful partition
    recreate_stateful(&loopdev)?;
    utils::safesync();

    // Detach loop device
    utils::detach_loop_device(&loopdev)?;
    utils::safesync();

    // Truncate image
    truncate_image(image)?;
    utils::safesync();

    log_info("Done!");
    Ok(())
}

/// Check if stateful partition has cros_payloads
fn check_stateful(loopdev: &str) -> Result<bool> {
    let mnt = tempfile::tempdir()?;
    let part1 = format!("{}p1", loopdev);
    
    // Try to mount read-only with noload
    let result = utils::shell(
        "mount",
        &["-o", "ro,noload", &part1, mnt.path().to_str().unwrap()],
        true,
        true,
    );
    
    let has_cros_payloads = if result.is_ok() {
        let cros_payloads_dir = mnt.path().join("cros_payloads");
        let exists = cros_payloads_dir.is_dir();
        let _ = utils::unmount_partition(mnt.path());
        exists
    } else {
        false
    };
    
    Ok(has_cros_payloads)
}

/// Squash partitions to remove gaps
fn squash_partitions(loopdev: &str) -> Result<()> {
    log_info("Squashing partitions");
    
    let gpt = Gpt::load_from_file(std::path::Path::new(loopdev))?;
    
    for (part_num, _) in gpt.get_partitions_physical_order() {
        log_info(&format!("Squashing {}p{}", loopdev, part_num));
        
        let input = "+,-\n";
        let mut child = std::process::Command::new("sudo")
            .args(&["sfdisk", "-N", &part_num.to_string(), "--move-data", loopdev])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        
        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(input.as_bytes())?;
        }
        
        let _ = child.wait();
    }
    
    Ok(())
}

/// Recreate stateful partition with minimal content
fn recreate_stateful(loopdev: &str) -> Result<()> {
    log_info("Recreating STATE");
    
    let gpt = Gpt::load_from_file(std::path::Path::new(loopdev))?;
    let sector_size = gpt.block_size;
    let final_sector = gpt.get_final_sector();
    
    let sectors = STATEFUL_SIZE / sector_size;
    
    // Add partition 1
    utils::shell(
        "cgpt",
        &[
            "add", loopdev,
            "-i", "1",
            "-b", &(final_sector + 1).to_string(),
            "-s", &sectors.to_string(),
            "-t", "data",
            "-l", "STATE",
        ],
        true,
        false,
    )?;
    
    utils::shell("partx", &["-u", "-n", "1", loopdev], true, false)?;
    
    // Create ext4 filesystem
    let part1 = format!("{}p1", loopdev);
    ext2::mkfs_ext4(&part1, "STATE")?;
    
    utils::safesync();
    
    // Mount and create required directories
    let mnt = tempfile::tempdir()?;
    utils::mount_partition(&part1, mnt.path(), false)?;
    
    fs::create_dir_all(mnt.path().join("dev_image/etc"))?;
    fs::create_dir_all(mnt.path().join("dev_image/factory/sh"))?;
    fs::File::create(mnt.path().join("dev_image/etc/lsb-factory"))?;
    
    utils::unmount_partition(mnt.path())?;
    
    Ok(())
}

/// Truncate image to minimal size
fn truncate_image(image: &std::path::Path) -> Result<()> {
    const BUFFER_SECTORS: u64 = 35;
    
    let gpt = Gpt::load_from_file(image)?;
    let sector_size = gpt.block_size;
    let final_sector = gpt.get_final_sector();
    let end_bytes = (final_sector + BUFFER_SECTORS) * sector_size;
    
    log_info(&format!("Truncating image to {}", utils::format_bytes(end_bytes)));
    
    utils::truncate_file(image, end_bytes)?;
    utils::shell("sgdisk", &["-e", image.to_str().unwrap()], true, false)?;
    
    Ok(())
}

/// Cleanup loop device
fn cleanup_loop_device(loopdev: &str) -> Result<()> {
    for i in 1..=12 {
        let part = format!("{}p{}", loopdev, i);
        let _ = utils::shell("umount", &[&part], true, true);
    }
    let _ = utils::detach_loop_device(loopdev);
    Ok(())
}
