//! Wax command - Main shim modification tool
//!
//! This command modifies factory shim images with M1NSH1M payloads,
//! enabling unenrollment and other features.

use anyhow::{bail, Context, Result};
use clap::Args;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::ext2;
use crate::gpt::{self, Gpt, partition_types};
use crate::utils::{self, log_info, log_debug, log_warn, log_error};

/// Arguments for the wax command
#[derive(Args, Debug)]
pub struct WaxArgs {
    /// Path to factory shim image
    #[arg(short, long)]
    pub image: PathBuf,

    /// Main payload type ('bw' for Beautiful World or 'legacy')
    #[arg(short, long, default_value = "bw")]
    pub payload: String,

    /// Custom main payload directory
    #[arg(long)]
    pub payload_dir: Option<PathBuf>,

    /// Partition size for payload(s) (e.g., "72M", "4G")
    #[arg(short = 's', long, default_value = "72M")]
    pub m1nsh1m_part_size: String,

    /// Extra payload directory
    #[arg(short = 'e', long)]
    pub extra_payload_dir: Option<PathBuf>,

    /// Firmware directory to include
    #[arg(long)]
    pub firmware_dir: Option<PathBuf>,

    /// Mounted payload directory
    #[arg(short = 'm', long)]
    pub mounted_payload_dir: Option<PathBuf>,

    /// Chromebrew tarball path
    #[arg(long)]
    pub chromebrew: Option<PathBuf>,

    /// Bootloader data directory
    #[arg(long)]
    pub bootloader_dir: Option<PathBuf>,

    /// Bootloader rootfs partition size
    #[arg(long, default_value = "4M")]
    pub bootloader_part_size: String,

    /// Force architecture for target device
    #[arg(long)]
    pub arch: Option<String>,

    /// Fast/dirty build (larger image size)
    #[arg(long)]
    pub fast: bool,

    /// Write final image size in bytes to this file
    #[arg(long)]
    pub finalsizefile: Option<PathBuf>,
}

/// Run the wax command
pub fn run(args: WaxArgs, debug: bool) -> Result<()> {
    // Check root privileges
    utils::require_root()?;
    
    // Check required dependencies
    let missing_deps = utils::check_deps(&[
        "partx", "sgdisk", "mkfs.ext4", "mkfs.ext2", "tune2fs", 
        "e2fsck", "resize2fs", "file", "numfmt", "pv", "tar"
    ]);
    if !missing_deps.is_empty() {
        bail!(
            "The following required commands weren't found in PATH:\n{}",
            missing_deps.join("\n")
        );
    }

    // Validate image
    let image = &args.image;
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

    // Find the script directory (for payload dirs)
    let script_dir = find_script_dir()?;
    
    // Set up bootloader directory
    let bootloader_dir = args.bootloader_dir
        .unwrap_or_else(|| script_dir.join("bootstrap"));
    if !bootloader_dir.is_dir() {
        bail!("{} is not a directory", bootloader_dir.display());
    }
    log_info(&format!("Using bootloader: {}", bootloader_dir.display()));

    // Set up payload directory
    let payload_dir = if let Some(dir) = args.payload_dir {
        dir
    } else {
        match args.payload.as_str() {
            "legacy" => script_dir.join("m1nsh1m_legacy"),
            "bw" => script_dir.join("m1nsh1m_bw"),
            _ => bail!("Invalid payload '{}'", args.payload),
        }
    };
    if !payload_dir.is_dir() {
        bail!("{} is not a directory", payload_dir.display());
    }
    log_info(&format!("Using main payload: {}", payload_dir.display()));

    // Set up extra payload directory
    let extra_payload_dir = args.extra_payload_dir
        .or_else(|| {
            let dir = script_dir.join("payloads");
            if dir.is_dir() { Some(dir) } else { None }
        });
    if let Some(ref dir) = extra_payload_dir {
        if !dir.is_dir() {
            bail!("{} is not a directory", dir.display());
        }
        log_info(&format!("Using extra payload: {}", dir.display()));
    }

    // Set up firmware directory
    let firmware_dir = args.firmware_dir
        .or_else(|| {
            let dir = script_dir.join("firmware");
            if dir.is_dir() { Some(dir) } else { None }
        });
    if let Some(ref dir) = firmware_dir {
        if !dir.is_dir() {
            bail!("{} is not a directory", dir.display());
        }
        log_info(&format!("Using firmware: {}", dir.display()));
    }

    // Set up mounted payload directory
    let mounted_payload_dir = args.mounted_payload_dir
        .or_else(|| {
            let dir = script_dir.join("mounted_payloads");
            if dir.is_dir() { Some(dir) } else { None }
        });
    if let Some(ref dir) = mounted_payload_dir {
        if !dir.is_dir() {
            bail!("{} is not a directory", dir.display());
        }
        log_info(&format!("Using mounted payload: {}", dir.display()));
    }

    // Validate chromebrew if provided
    if let Some(ref chromebrew) = args.chromebrew {
        if !chromebrew.is_file() {
            bail!("{} doesn't exist or isn't a file", chromebrew.display());
        }
        log_info(&format!("Using chromebrew: {}", chromebrew.display()));
    }

    // Parse sizes
    let m1nsh1m_part_size = utils::parse_bytes(&args.m1nsh1m_part_size)
        .context(format!("Could not parse size '{}'", args.m1nsh1m_part_size))?;
    let bootloader_part_size = utils::parse_bytes(&args.bootloader_part_size)
        .context(format!("Could not parse size '{}'", args.bootloader_part_size))?;

    // Fix backup GPT table
    log_info("Fixing backup GPT table...");
    utils::shell("sgdisk", &["-e", image.to_str().unwrap()], true, !debug)?;
    
    // Delete all partitions except kernel and rootfs (2 and 3)
    delete_partitions_except(image, &[2, 3])?;
    utils::safesync();

    // Create loop device
    log_info("Creating loop device");
    let loopdev = utils::create_loop_device(image)?;
    log_debug(&format!("Loop device: {}", loopdev));
    
    // Set up cleanup
    let _cleanup = scopeguard::guard(loopdev.clone(), |dev| {
        let _ = cleanup_loop_device(&dev);
    });
    utils::safesync();

    // Detect or use specified architecture
    let target_arch = if let Some(arch) = args.arch {
        log_info(&format!("Using specified architecture: {}", arch));
        arch
    } else {
        detect_architecture(&loopdev)?
    };
    utils::safesync();

    // Shrink root partition (unless fast mode)
    if !args.fast {
        shrink_root(&loopdev)?;
        utils::safesync();
        
        squash_partitions(&loopdev)?;
        utils::safesync();
    } else {
        log_info("Fast mode on, skipping shrink/squash");
    }

    // Patch bootloader partition
    patch_bootloader(&loopdev, image, &bootloader_dir, &target_arch, bootloader_part_size)?;
    utils::safesync();

    // Swap partition 3 to partition 4
    log_info("Swapping partition table entries...");
    utils::shell("sgdisk", &["-r", "3:4", &loopdev], true, !debug)?;
    utils::safesync();

    // Patch M1NSH1M partition
    patch_m1nsh1m(
        &loopdev,
        image,
        &payload_dir,
        extra_payload_dir.as_deref(),
        firmware_dir.as_deref(),
        mounted_payload_dir.as_deref(),
        args.chromebrew.as_deref(),
        m1nsh1m_part_size,
    )?;
    utils::safesync();

    // Detach loop device
    utils::detach_loop_device(&loopdev)?;
    utils::safesync();

    // Truncate image
    truncate_image(image, args.finalsizefile.as_deref())?;
    utils::safesync();

    log_info("Done. Have fun!");
    Ok(())
}

/// Find the script directory (where wax/payloads are located)
fn find_script_dir() -> Result<PathBuf> {
    // Try to find relative to executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            // Check for wax directory structure
            let wax_dir = parent.join("wax");
            if wax_dir.exists() {
                return Ok(wax_dir);
            }
            // Check if we're already in wax directory
            if parent.join("m1nsh1m_bw").exists() || parent.join("sh1mmer_bw").exists() {
                return Ok(parent.to_path_buf());
            }
        }
    }
    
    // Try current directory
    let cwd = std::env::current_dir()?;
    if cwd.join("m1nsh1m_bw").exists() || cwd.join("sh1mmer_bw").exists() {
        return Ok(cwd);
    }
    
    // Try parent directory
    if let Some(parent) = cwd.parent() {
        let wax_dir = parent.join("wax");
        if wax_dir.exists() {
            return Ok(wax_dir);
        }
    }
    
    // Default to current directory
    Ok(cwd)
}

/// Delete all partitions except the specified ones
fn delete_partitions_except(image: &Path, keep: &[u32]) -> Result<()> {
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

/// Detect target architecture from rootfs
fn detect_architecture(loopdev: &str) -> Result<String> {
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

/// Shrink the ROOT partition
fn shrink_root(loopdev: &str) -> Result<()> {
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

/// Squash partitions to remove gaps
fn squash_partitions(loopdev: &str) -> Result<()> {
    log_info("Squashing partitions");
    
    let gpt = Gpt::load_from_file(Path::new(loopdev))?;
    
    for (part_num, _) in gpt.get_partitions_physical_order() {
        log_info(&format!("Squashing {}p{}", loopdev, part_num));
        
        // Use sfdisk to move partition data
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

/// Patch the bootloader partition (partition 4)
fn patch_bootloader(
    loopdev: &str,
    image: &Path,
    bootloader_dir: &Path,
    target_arch: &str,
    part_size: u64,
) -> Result<()> {
    log_info(&format!("Creating bootloader partition ({})", utils::format_bytes(part_size)));
    
    let sector_size = gpt::get_sector_size(Path::new(loopdev))?;
    let sectors = part_size / sector_size;
    
    // Add partition using cgpt
    cgpt_add_auto(image, loopdev, 4, sectors, "rootfs", "ROOT-A")?;
    
    // Create ext2 filesystem
    let part4 = format!("{}p4", loopdev);
    ext2::mkfs_ext2(&part4, "ROOT-A")?;
    
    utils::safesync();
    
    // Mount and copy bootloader files
    let mnt = tempfile::tempdir()?;
    utils::mount_partition(&part4, mnt.path(), false)?;
    
    // Create required directories
    fs::create_dir_all(mnt.path().join("bin"))?;
    fs::create_dir_all(mnt.path().join("root"))?;
    fs::create_dir_all(mnt.path().join("etc/init"))?;
    
    log_info("Copying bootloader payload");
    
    // Copy noarch files
    let noarch_dir = bootloader_dir.join("noarch");
    if noarch_dir.is_dir() {
        utils::copy_dir_recursive(&noarch_dir, mnt.path())?;
    }
    
    // Copy arch-specific files
    let arch_dir = bootloader_dir.join(target_arch);
    if arch_dir.is_dir() {
        utils::copy_dir_recursive(&arch_dir, mnt.path())?;
    }
    
    // Make everything executable
    make_executable_recursive(mnt.path())?;
    
    utils::unmount_partition(mnt.path())?;
    
    Ok(())
}

/// Patch the M1NSH1M partition (partition 1)
fn patch_m1nsh1m(
    loopdev: &str,
    image: &Path,
    payload_dir: &Path,
    extra_payload_dir: Option<&Path>,
    firmware_dir: Option<&Path>,
    mounted_payload_dir: Option<&Path>,
    chromebrew: Option<&Path>,
    part_size: u64,
) -> Result<()> {
    log_info(&format!("Creating M1NSH1M partition ({})", utils::format_bytes(part_size)));
    
    let sector_size = gpt::get_sector_size(Path::new(loopdev))?;
    let sectors = part_size / sector_size;
    
    // Add partition using cgpt
    cgpt_add_auto(image, loopdev, 1, sectors, "data", "M1NSH1M")?;
    
    // Create ext4 filesystem
    let part1 = format!("{}p1", loopdev);
    ext2::mkfs_ext4(&part1, "M1NSH1M")?;
    
    utils::safesync();
    
    // Mount and copy payload files
    let mnt = tempfile::tempdir()?;
    utils::mount_partition(&part1, mnt.path(), false)?;
    
    // Create required directories
    fs::create_dir_all(mnt.path().join("dev_image/etc"))?;
    fs::create_dir_all(mnt.path().join("dev_image/factory/sh"))?;
    fs::File::create(mnt.path().join("dev_image/etc/lsb-factory"))?;
    
    log_info("Copying main payload");
    utils::copy_dir_recursive(payload_dir, mnt.path())?;
    make_executable_recursive(mnt.path())?;
    
    // Copy extra payloads
    if let Some(extra_dir) = extra_payload_dir {
        log_info("Copying extra payload");
        let dest = mnt.path().join("root/noarch/payloads");
        fs::create_dir_all(&dest)?;
        utils::copy_dir_recursive(extra_dir, &dest)?;
    }
    
    // Copy firmware
    if let Some(fw_dir) = firmware_dir {
        log_info("Copying firmware");
        let dest = mnt.path().join("root/noarch/lib/firmware");
        fs::create_dir_all(&dest)?;
        utils::copy_dir_recursive(fw_dir, &dest)?;
    }
    
    // Copy mounted payloads
    if let Some(mp_dir) = mounted_payload_dir {
        // Check if directory has any files
        if fs::read_dir(mp_dir)?.next().is_some() {
            log_info("Copying mounted payload");
            let dest = mnt.path().join("mounted_payloads");
            fs::create_dir_all(&dest)?;
            utils::copy_dir_recursive(mp_dir, &dest)?;
        }
    }
    
    // Extract chromebrew
    if let Some(cb_path) = chromebrew {
        log_info("Extracting chromebrew... increase m1nsh1m part size if this fails");
        let dest = mnt.path().join("chromebrew");
        fs::create_dir_all(&dest)?;
        
        // Use pv and tar to extract
        let cb_str = cb_path.to_str().unwrap();
        let dest_str = dest.to_str().unwrap();
        utils::shell(
            "sh",
            &["-c", &format!("pv '{}' | tar -xzf - --strip-components=1 -C '{}'", cb_str, dest_str)],
            true,
            false,
        )?;
    }
    
    utils::unmount_partition(mnt.path())?;
    
    Ok(())
}

/// Add a partition with automatic resizing if needed
fn cgpt_add_auto(
    image: &Path,
    loopdev: &str,
    part_num: u32,
    sectors: u64,
    type_name: &str,
    label: &str,
) -> Result<()> {
    let gpt = Gpt::load_from_file(Path::new(loopdev))?;
    let final_sector = gpt.get_final_sector();
    let backup_sector = gpt.get_backup_table_sector();
    
    // Check if we need to resize the image
    if final_sector + sectors + 1 > backup_sector {
        let sector_size = gpt.block_size;
        let current_sectors = gpt.get_size() / sector_size;
        let needed_sectors = final_sector + sectors + 35; // Buffer for GPT
        
        if needed_sectors > current_sectors {
            let new_size = needed_sectors * sector_size;
            log_info(&format!("Resizing image to {}", utils::format_bytes(new_size)));
            utils::truncate_file(image, new_size)?;
            utils::shell("sgdisk", &["-e", image.to_str().unwrap()], true, false)?;
            utils::shell("losetup", &["-c", loopdev], true, false)?;
        }
    }
    
    // Determine partition type GUID
    let type_guid = match type_name {
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
            "-i", &part_num.to_string(),
            "-b", &start_sector.to_string(),
            "-s", &sectors.to_string(),
            "-t", type_guid,
            "-l", label,
        ],
        true,
        false,
    )?;
    
    // Update kernel partition table
    utils::shell("partx", &["-u", "-n", &part_num.to_string(), loopdev], true, false)?;
    
    Ok(())
}

/// Truncate image to minimal size
fn truncate_image(image: &Path, size_file: Option<&Path>) -> Result<()> {
    const BUFFER_SECTORS: u64 = 35; // Magic number for GPT safety
    
    let gpt = Gpt::load_from_file(image)?;
    let sector_size = gpt.block_size;
    let final_sector = gpt.get_final_sector();
    let end_bytes = (final_sector + BUFFER_SECTORS) * sector_size;
    
    log_info(&format!("Truncating image to {}", utils::format_bytes(end_bytes)));
    
    if utils::is_block_device(image) {
        // For block devices, use losetup with size limit
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
    
    // Write size to file if requested
    if let Some(size_path) = size_file {
        fs::write(size_path, end_bytes.to_string())?;
    }
    
    Ok(())
}

/// Make all files in a directory executable
fn make_executable_recursive(path: &Path) -> Result<()> {
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

/// Cleanup loop device
fn cleanup_loop_device(loopdev: &str) -> Result<()> {
    // Attempt to unmount any mounted partitions
    for i in 1..=12 {
        let part = format!("{}p{}", loopdev, i);
        let _ = utils::shell("umount", &[&part], true, true);
    }
    
    // Detach the loop device
    let _ = utils::detach_loop_device(loopdev);
    
    Ok(())
}
