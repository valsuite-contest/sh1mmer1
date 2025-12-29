//! Minimal command - Build a shim with minimal changes to boot a custom payload
//!
//! This command creates a minimal shim that boots directly to a custom payload
//! with the fewest possible modifications to the original shim structure.

use anyhow::{bail, Context, Result};
use clap::Args;
use std::fs;
use std::path::{Path, PathBuf};

use crate::core::{self, LoopDeviceManager, PartitionConfig, PayloadConfig};
use crate::ext2;
use crate::gpt::{self, Gpt};
use crate::utils::{self, log_info, log_debug};

/// Arguments for the minimal command
#[derive(Args, Debug)]
pub struct MinimalArgs {
    /// Path to factory shim image
    #[arg(short, long)]
    pub image: PathBuf,

    /// Custom init script to execute on boot
    /// This script will be executed as the primary boot payload
    #[arg(short = 'x', long)]
    pub init_script: Option<PathBuf>,

    /// Custom payload directory to copy to the shim
    /// Contents will be available at /stateful during boot
    #[arg(short, long)]
    pub payload_dir: Option<PathBuf>,

    /// Size for the payload partition (e.g., "16M", "64M")
    #[arg(short = 's', long, default_value = "16M")]
    pub payload_size: String,

    /// Force architecture for target device (x86_64 or aarch64)
    #[arg(long)]
    pub arch: Option<String>,

    /// Skip shrinking partitions (faster but larger image)
    #[arg(long)]
    pub fast: bool,

    /// Write final image size in bytes to this file
    #[arg(long)]
    pub finalsizefile: Option<PathBuf>,
}

/// Run the minimal command
pub fn run(args: MinimalArgs, debug: bool) -> Result<()> {
    // Check root privileges
    utils::require_root()?;
    
    // Check required dependencies
    let missing_deps = utils::check_deps(&[
        "partx", "sgdisk", "mkfs.ext4", "mkfs.ext2", "tune2fs", 
        "e2fsck", "resize2fs", "file"
    ]);
    if !missing_deps.is_empty() {
        bail!(
            "The following required commands weren't found in PATH:\n{}",
            missing_deps.join("\n")
        );
    }

    // Validate image
    core::validate_shim_image(&args.image)?;

    // Validate inputs
    if args.init_script.is_none() && args.payload_dir.is_none() {
        bail!("Either --init-script or --payload-dir must be specified");
    }

    if let Some(ref script) = args.init_script {
        if !script.is_file() {
            bail!("Init script doesn't exist: {}", script.display());
        }
    }

    if let Some(ref dir) = args.payload_dir {
        if !dir.is_dir() {
            bail!("Payload directory doesn't exist: {}", dir.display());
        }
    }

    // Parse payload size
    let payload_size = utils::parse_bytes(&args.payload_size)
        .context(format!("Could not parse size '{}'", args.payload_size))?;

    // Fix backup GPT table
    core::fix_gpt_backup(&args.image, debug)?;
    
    // Delete all partitions except kernel and rootfs (2 and 3)
    core::delete_partitions_except(&args.image, &[2, 3])?;
    utils::safesync();

    // Create loop device (automatically cleaned up when dropped)
    let loop_mgr = LoopDeviceManager::new(&args.image)?;
    let loopdev = &loop_mgr.device;
    utils::safesync();

    // Detect or use specified architecture
    let target_arch = if let Some(arch) = args.arch {
        log_info(&format!("Using specified architecture: {}", arch));
        arch
    } else {
        core::detect_architecture(loopdev)?
    };
    utils::safesync();

    // Shrink root partition (unless fast mode)
    if !args.fast {
        core::shrink_root_partition(loopdev)?;
        utils::safesync();
        
        core::squash_partitions(loopdev)?;
        utils::safesync();
    } else {
        log_info("Fast mode on, skipping shrink/squash");
    }

    // Create minimal bootloader partition (partition 4)
    create_minimal_bootloader(
        loopdev,
        &args.image,
        &target_arch,
        args.init_script.as_deref(),
    )?;
    utils::safesync();

    // Swap partition 3 to partition 4
    core::swap_partitions(loopdev, 3, 4, debug)?;
    utils::safesync();

    // Create payload partition (partition 1)
    create_payload_partition(
        loopdev,
        &args.image,
        args.payload_dir.as_deref(),
        payload_size,
    )?;
    utils::safesync();

    // Drop loop device manager to release it
    drop(loop_mgr);
    utils::safesync();

    // Truncate image
    core::truncate_image(&args.image, args.finalsizefile.as_deref())?;
    utils::safesync();

    log_info("Done. Minimal shim created successfully!");
    Ok(())
}

/// Create a minimal bootloader partition
fn create_minimal_bootloader(
    loopdev: &str,
    image: &Path,
    arch: &str,
    init_script: Option<&Path>,
) -> Result<()> {
    // Use a small bootloader partition - 2MB is usually enough for minimal setup
    let bootloader_size = 2 * 1024 * 1024; // 2MB
    
    let config = PartitionConfig {
        number: 4,
        size: bootloader_size,
        partition_type: "rootfs".to_string(),
        label: "ROOT-A".to_string(),
        filesystem: "ext2".to_string(),
    };
    
    core::add_partition(image, loopdev, &config)?;
    
    // Mount and create minimal bootloader
    let part4 = format!("{}p4", loopdev);
    let mnt = tempfile::tempdir()?;
    utils::mount_partition(&part4, mnt.path(), false)?;
    
    // Create minimal directory structure
    fs::create_dir_all(mnt.path().join("bin"))?;
    fs::create_dir_all(mnt.path().join("etc/init"))?;
    
    // Create minimal init script
    let init_content = if let Some(script_path) = init_script {
        fs::read_to_string(script_path)?
    } else {
        create_default_init_script()
    };
    
    fs::write(mnt.path().join("bin/init.sh"), &init_content)?;
    
    // Create upstart job to run init
    let upstart_content = r#"# Minimal m1nsh1m boot job
start on startup

script
    /bin/init.sh
end script
"#;
    fs::write(mnt.path().join("etc/init/m1nsh1m.conf"), upstart_content)?;
    
    // Create busybox symlinks if busybox exists
    let busybox_src = find_busybox(arch)?;
    if let Some(busybox) = busybox_src {
        fs::copy(&busybox, mnt.path().join("bin/busybox"))?;
        
        // Install busybox applets - make bin directory executable
        let bin_dir = mnt.path().join("bin");
        core::make_executable_recursive(&bin_dir)?;
    }
    
    // Make everything executable
    core::make_executable_recursive(mnt.path())?;
    
    utils::unmount_partition(mnt.path())?;
    
    log_info("Minimal bootloader partition created");
    Ok(())
}

/// Create the payload partition
fn create_payload_partition(
    loopdev: &str,
    image: &Path,
    payload_dir: Option<&Path>,
    size: u64,
) -> Result<()> {
    let config = PartitionConfig {
        number: 1,
        size,
        partition_type: "data".to_string(),
        label: "PAYLOAD".to_string(),
        filesystem: "ext4".to_string(),
    };
    
    core::add_partition(image, loopdev, &config)?;
    
    // Mount and populate payload partition
    let part1 = format!("{}p1", loopdev);
    let mnt = tempfile::tempdir()?;
    utils::mount_partition(&part1, mnt.path(), false)?;
    
    // Create minimal required structure for ChromeOS factory shim
    fs::create_dir_all(mnt.path().join("dev_image/etc"))?;
    fs::create_dir_all(mnt.path().join("dev_image/factory/sh"))?;
    fs::File::create(mnt.path().join("dev_image/etc/lsb-factory"))?;
    
    // Copy payload directory if provided
    if let Some(dir) = payload_dir {
        log_info(&format!("Copying payload from {}", dir.display()));
        utils::copy_dir_recursive(dir, mnt.path())?;
        core::make_executable_recursive(mnt.path())?;
    }
    
    utils::unmount_partition(mnt.path())?;
    
    log_info("Payload partition created");
    Ok(())
}

/// Create default minimal init script
fn create_default_init_script() -> String {
    r#"#!/bin/sh
# Minimal m1nsh1m init script
# This script is executed when the shim boots

set -e

# Mount stateful partition
STATEFUL_MNT="/tmp/stateful"
mkdir -p "$STATEFUL_MNT"

# Try to find and mount the payload partition
for dev in /dev/sd*1 /dev/mmcblk*p1; do
    if [ -b "$dev" ]; then
        if mount -o ro "$dev" "$STATEFUL_MNT" 2>/dev/null; then
            echo "Mounted $dev at $STATEFUL_MNT"
            break
        fi
    fi
done

# Execute custom payload if it exists
if [ -x "$STATEFUL_MNT/payload.sh" ]; then
    exec "$STATEFUL_MNT/payload.sh" "$STATEFUL_MNT"
fi

# Otherwise, drop to shell if available
if [ -x /bin/sh ]; then
    echo "No payload found. Dropping to shell."
    exec /bin/sh
fi

# If no shell, just wait
echo "No payload or shell available. Waiting..."
while true; do sleep 3600; done
"#.to_string()
}

/// Find busybox binary for the target architecture
fn find_busybox(arch: &str) -> Result<Option<PathBuf>> {
    let script_dir = core::find_script_dir()?;
    
    // Check common locations
    let locations = [
        script_dir.join("bootstrap").join(arch).join("busybox"),
        script_dir.join("m1nsh1m_bw/bootstrap").join(arch).join("busybox"),
        script_dir.join("m1nsh1m_legacy/bootstrap").join(arch).join("busybox"),
        PathBuf::from("/usr/bin/busybox"),
    ];
    
    for loc in &locations {
        if loc.exists() {
            return Ok(Some(loc.clone()));
        }
    }
    
    Ok(None)
}
