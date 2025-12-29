//! Core functionality for shim modification
//!
//! This module provides reusable components for shim modification operations.
//! It is designed to be modular and composable for different use cases.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ext2;
use crate::gpt::{self, Gpt, partition_types};
use crate::utils::{self, log_info, log_debug};

/// Configuration for shim modification operations
#[derive(Debug, Clone)]
pub struct ShimConfig {
    /// Path to the shim image
    pub image: PathBuf,
    /// Target architecture (x86_64 or aarch64)
    pub arch: Option<String>,
    /// Enable fast mode (skip shrinking/squashing)
    pub fast: bool,
    /// Debug mode
    pub debug: bool,
}

/// Partition configuration for adding new partitions
#[derive(Debug, Clone)]
pub struct PartitionConfig {
    /// Partition number (1-based)
    pub number: u32,
    /// Partition size in bytes
    pub size: u64,
    /// Partition type (rootfs, data, kernel)
    pub partition_type: String,
    /// Partition label
    pub label: String,
    /// Filesystem type (ext2, ext4)
    pub filesystem: String,
}

/// Payload configuration for copying files to partitions
#[derive(Debug, Clone)]
pub struct PayloadConfig {
    /// Source directory for payload files
    pub source_dir: PathBuf,
    /// Optional architecture-specific subdirectory
    pub arch_subdir: bool,
    /// Directories to create before copying
    pub create_dirs: Vec<String>,
    /// Files to create (empty)
    pub create_files: Vec<String>,
    /// Make all files executable
    pub make_executable: bool,
}

/// Manages loop device lifecycle
pub struct LoopDeviceManager {
    pub device: String,
}

impl LoopDeviceManager {
    /// Create a new loop device for the given image
    pub fn new(image: &Path) -> Result<Self> {
        log_info("Creating loop device");
        let device = utils::create_loop_device(image)?;
        log_debug(&format!("Loop device: {}", device));
        Ok(Self { device })
    }

    /// Get the partition device path
    pub fn partition(&self, num: u32) -> String {
        format!("{}p{}", self.device, num)
    }
}

impl Drop for LoopDeviceManager {
    fn drop(&mut self) {
        // Attempt to unmount any mounted partitions
        for i in 1..=12 {
            let part = format!("{}p{}", self.device, i);
            let _ = utils::shell("umount", &[&part], true, true);
        }
        // Detach the loop device
        let _ = utils::detach_loop_device(&self.device);
    }
}

/// Validate that an image is a valid GPT shim image
pub fn validate_shim_image(image: &Path) -> Result<()> {
    if utils::is_block_device(image) {
        log_info("Image is a block device, performance may suffer...");
    } else {
        if !utils::check_file_rw(image) {
            bail!("{} doesn't exist, isn't a file, or isn't RW", image.display());
        }
        utils::check_slow_fs(image)?;
    }
    
    if !gpt::check_gpt_image(image)? {
        bail!("{} is not GPT, or is corrupted", image.display());
    }
    
    Ok(())
}

/// Fix the backup GPT table
pub fn fix_gpt_backup(image: &Path, debug: bool) -> Result<()> {
    log_info("Fixing backup GPT table...");
    utils::shell("sgdisk", &["-e", image.to_str().unwrap()], true, !debug)?;
    Ok(())
}

/// Delete all partitions except the specified ones
pub fn delete_partitions_except(image: &Path, keep: &[u32]) -> Result<()> {
    let gpt = Gpt::load_from_file(image)?;
    let used_parts = gpt.get_used_partitions();
    
    let to_delete: Vec<String> = used_parts
        .iter()
        .filter(|p| !keep.contains(p))
        .map(|p| p.to_string())
        .collect();
    
    if !to_delete.is_empty() {
        log_info(&format!("Deleting partitions: {}", to_delete.join(", ")));
        let mut args = vec!["--delete".to_string(), image.to_str().unwrap().to_string()];
        args.extend(to_delete);
        let args_str: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        utils::shell("sfdisk", &args_str, true, false)?;
    }
    
    Ok(())
}

/// Detect target architecture from rootfs partition
pub fn detect_architecture(loopdev: &str) -> Result<String> {
    let mnt = tempfile::tempdir()?;
    let part3 = format!("{}p3", loopdev);
    
    utils::mount_partition(&part3, mnt.path(), true)?;
    
    let arch = if mnt.path().join("bin/bash").exists() {
        let file_info = utils::get_file_type(&mnt.path().join("bin/bash"))?;
        let file_info_lower = file_info.to_lowercase();
        
        if file_info_lower.contains("aarch64") 
            || file_info_lower.contains("armv8") 
            || file_info_lower.contains("arm") 
        {
            "aarch64".to_string()
        } else {
            "x86_64".to_string()
        }
    } else {
        "x86_64".to_string()
    };
    
    utils::unmount_partition(mnt.path())?;
    
    log_info(&format!("Detected architecture: {}", arch));
    Ok(arch)
}

/// Shrink the ROOT partition (partition 3)
pub fn shrink_root_partition(loopdev: &str) -> Result<()> {
    log_info("Shrinking ROOT");
    
    let part3 = format!("{}p3", loopdev);
    
    // Enable RW mount
    ext2::enable_rw_mount(Path::new(&part3), 0)?;
    
    // Run e2fsck
    ext2::check_filesystem(&part3)?;
    
    // Resize to minimum
    ext2::shrink_filesystem_to_minimum(&part3)?;
    
    // Disable RW mount
    ext2::disable_rw_mount(Path::new(&part3), 0)?;
    
    // Get new size info
    let fs_info = ext2::get_fs_info(&part3)?;
    let sector_size = gpt::get_sector_size(Path::new(loopdev))?;
    
    log_debug(&format!(
        "sector size: {}, block size: {}, block count: {}",
        sector_size, fs_info.block_size, fs_info.block_count
    ));
    
    // Calculate new partition size
    let gpt = Gpt::load_from_file(Path::new(loopdev))?;
    let part = gpt.get_partition(3).context("Partition 3 not found")?;
    
    let original_bytes = part.size_in_bytes(sector_size);
    let resized_bytes = fs_info.size();
    let resized_sectors = resized_bytes / sector_size;
    
    log_info(&format!(
        "Resizing ROOT from {} to {}",
        utils::format_bytes(original_bytes),
        utils::format_bytes(resized_bytes)
    ));
    
    // Update partition size using cgpt
    utils::shell(
        "cgpt",
        &["add", "-i", "3", "-s", &resized_sectors.to_string(), loopdev],
        true,
        false,
    )?;
    
    // Update kernel about partition changes
    utils::shell("partx", &["-u", "-n", "3", loopdev], true, false)?;
    
    Ok(())
}

/// Squash all partitions to remove gaps
pub fn squash_partitions(loopdev: &str) -> Result<()> {
    log_info("Squashing partitions");
    
    let gpt = Gpt::load_from_file(Path::new(loopdev))?;
    
    for (part_num, _) in gpt.get_partitions_physical_order() {
        log_info(&format!("Squashing {}p{}", loopdev, part_num));
        utils::sfdisk_squash_partition(loopdev, part_num)?;
    }
    
    Ok(())
}

/// Swap partition table entries (e.g., swap partition 3 to partition 4)
pub fn swap_partitions(loopdev: &str, from: u32, to: u32, debug: bool) -> Result<()> {
    log_info(&format!("Swapping partition {} to {}", from, to));
    utils::shell("sgdisk", &["-r", &format!("{}:{}", from, to), loopdev], true, !debug)?;
    Ok(())
}

/// Add a partition with automatic resizing if needed
pub fn add_partition(
    image: &Path,
    loopdev: &str,
    config: &PartitionConfig,
) -> Result<()> {
    let sector_size = gpt::get_sector_size(Path::new(loopdev))?;
    let sectors = config.size / sector_size;
    
    log_info(&format!(
        "Creating partition {} ({}) as {}",
        config.number,
        utils::format_bytes(config.size),
        config.label
    ));
    
    let gpt = Gpt::load_from_file(Path::new(loopdev))?;
    let final_sector = gpt.get_final_sector();
    let backup_sector = gpt.get_backup_table_sector();
    
    // Check if we need to resize the image
    if final_sector + sectors + 1 > backup_sector {
        let current_sectors = gpt.get_size() / sector_size;
        let needed_sectors = final_sector + sectors + 35;
        
        if needed_sectors > current_sectors {
            let new_size = needed_sectors * sector_size;
            log_info(&format!("Resizing image to {}", utils::format_bytes(new_size)));
            utils::truncate_file(image, new_size)?;
            utils::shell("sgdisk", &["-e", image.to_str().unwrap()], true, false)?;
            utils::shell("losetup", &["-c", loopdev], true, false)?;
        }
    }
    
    // Determine partition type GUID
    let type_guid = match config.partition_type.as_str() {
        "rootfs" => partition_types::CHROMEOS_ROOTFS,
        "data" => partition_types::LINUX_DATA,
        "kernel" => partition_types::CHROMEOS_KERNEL,
        _ => partition_types::LINUX_DATA,
    };
    
    // Add the partition
    let start_sector = final_sector + 1;
    utils::shell(
        "cgpt",
        &[
            "add",
            loopdev,
            "-i", &config.number.to_string(),
            "-b", &start_sector.to_string(),
            "-s", &sectors.to_string(),
            "-t", type_guid,
            "-l", &config.label,
        ],
        true,
        false,
    )?;
    
    // Update kernel partition table
    utils::shell("partx", &["-u", "-n", &config.number.to_string(), loopdev], true, false)?;
    
    // Create filesystem
    let part_dev = format!("{}p{}", loopdev, config.number);
    match config.filesystem.as_str() {
        "ext2" => ext2::mkfs_ext2(&part_dev, &config.label)?,
        "ext4" => ext2::mkfs_ext4(&part_dev, &config.label)?,
        _ => bail!("Unknown filesystem type: {}", config.filesystem),
    }
    
    utils::safesync();
    
    Ok(())
}

/// Copy payload files to a partition
pub fn copy_payload_to_partition(
    loopdev: &str,
    partition_num: u32,
    config: &PayloadConfig,
    arch: Option<&str>,
) -> Result<()> {
    let part_dev = format!("{}p{}", loopdev, partition_num);
    let mnt = tempfile::tempdir()?;
    utils::mount_partition(&part_dev, mnt.path(), false)?;
    
    // Create required directories
    for dir in &config.create_dirs {
        fs::create_dir_all(mnt.path().join(dir))?;
    }
    
    // Create required files
    for file in &config.create_files {
        let path = mnt.path().join(file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::File::create(path)?;
    }
    
    log_info(&format!("Copying payload from {}", config.source_dir.display()));
    
    // Copy noarch files if arch subdirectory is enabled
    if config.arch_subdir {
        let noarch_dir = config.source_dir.join("noarch");
        if noarch_dir.is_dir() {
            utils::copy_dir_recursive(&noarch_dir, mnt.path())?;
        }
        
        // Copy arch-specific files
        if let Some(arch_name) = arch {
            let arch_dir = config.source_dir.join(arch_name);
            if arch_dir.is_dir() {
                utils::copy_dir_recursive(&arch_dir, mnt.path())?;
            }
        }
    } else {
        utils::copy_dir_recursive(&config.source_dir, mnt.path())?;
    }
    
    // Make files executable if requested
    if config.make_executable {
        make_executable_recursive(mnt.path())?;
    }
    
    utils::unmount_partition(mnt.path())?;
    
    Ok(())
}

/// Copy a single directory to a specific path on a mounted partition
pub fn copy_to_partition_path(
    loopdev: &str,
    partition_num: u32,
    source: &Path,
    dest_path: &str,
) -> Result<()> {
    let part_dev = format!("{}p{}", loopdev, partition_num);
    let mnt = tempfile::tempdir()?;
    utils::mount_partition(&part_dev, mnt.path(), false)?;
    
    let dest = mnt.path().join(dest_path);
    fs::create_dir_all(&dest)?;
    
    log_info(&format!("Copying {} to partition {} at {}", source.display(), partition_num, dest_path));
    utils::copy_dir_recursive(source, &dest)?;
    
    utils::unmount_partition(mnt.path())?;
    
    Ok(())
}

/// Truncate image to minimal size
pub fn truncate_image(image: &Path, size_file: Option<&Path>) -> Result<()> {
    const BUFFER_SECTORS: u64 = 35;
    
    let gpt = Gpt::load_from_file(image)?;
    let sector_size = gpt.block_size;
    let final_sector = gpt.get_final_sector();
    let end_bytes = (final_sector + BUFFER_SECTORS) * sector_size;
    
    log_info(&format!("Truncating image to {}", utils::format_bytes(end_bytes)));
    
    if utils::is_block_device(image) {
        let loopdev = utils::shell_output(
            "losetup",
            &["--show", "-f", "-P", image.to_str().unwrap(), "--sizelimit", &end_bytes.to_string()],
            true,
        )?;
        let loopdev = loopdev.trim();
        
        utils::shell("sgdisk", &["-e", loopdev], true, false)?;
        utils::shell("losetup", &["-d", loopdev], true, true)?;
    } else {
        utils::truncate_file(image, end_bytes)?;
        utils::shell("sgdisk", &["-e", image.to_str().unwrap()], true, false)?;
    }
    
    if let Some(size_path) = size_file {
        fs::write(size_path, end_bytes.to_string())?;
    }
    
    Ok(())
}

/// Make all files in a directory executable
pub fn make_executable_recursive(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    
    for entry in walkdir::WalkDir::new(path) {
        let entry = entry?;
        if entry.file_type().is_file() {
            let mut perms = fs::metadata(entry.path())?.permissions();
            let mode = perms.mode();
            perms.set_mode(mode | 0o111);
            fs::set_permissions(entry.path(), perms)?;
        }
    }
    Ok(())
}

/// Find the script/payload directory
pub fn find_script_dir() -> Result<PathBuf> {
    // Try to find relative to executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let wax_dir = parent.join("wax");
            if wax_dir.exists() {
                return Ok(wax_dir);
            }
            if parent.join("m1nsh1m_bw").exists() || parent.join("sh1mmer_bw").exists() {
                return Ok(parent.to_path_buf());
            }
        }
    }
    
    let cwd = std::env::current_dir()?;
    if cwd.join("m1nsh1m_bw").exists() || cwd.join("sh1mmer_bw").exists() {
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
