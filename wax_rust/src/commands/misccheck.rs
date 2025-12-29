//! Misc Check command - Check miscellaneous info about a shim image

use anyhow::{bail, Result};
use clap::Args;
use std::path::PathBuf;
use std::fs;

use crate::ext2;
use crate::gpt::{self, Gpt, partition_types, PART_CROS_STATEFUL, PART_CROS_ROOTFS_A};
use crate::utils::{self, log_info, log_warn};

/// Arguments for the misccheck command
#[derive(Args, Debug)]
pub struct MiscCheckArgs {
    /// Image to check
    pub image: PathBuf,
}

/// Run the misccheck command
pub fn run(args: MiscCheckArgs, debug: bool) -> Result<()> {
    utils::require_root()?;
    
    let missing_deps = utils::check_deps(&["sfdisk", "tar", "file", "jq", "zcat", "tune2fs"]);
    if !missing_deps.is_empty() {
        bail!(
            "The following required commands weren't found in PATH:\n{}",
            missing_deps.join("\n")
        );
    }

    let image = &args.image;
    
    if !image.exists() || (!image.is_file() && !utils::is_block_device(image)) {
        bail!("{} doesn't exist or is not a file or block device", image.display());
    }
    
    if !gpt::check_gpt_image(image)? {
        bail!("{} is not GPT, or is corrupted", image.display());
    }

    println!("{}", image.display());

    let loopdev = utils::create_loop_device(image)?;
    let _cleanup = scopeguard::guard(loopdev.clone(), |dev: String| {
        let _ = utils::detach_loop_device(&dev);
    });

    let workdir = tempfile::tempdir()?;
    let mnt = workdir.path().join("mnt");
    fs::create_dir_all(&mnt)?;

    let gpt = Gpt::load_from_file(std::path::Path::new(&loopdev))?;

    let mut has_cros_payloads = false;
    let mut models = String::new();
    let mut hwids: Vec<String> = Vec::new();
    let mut release_image = String::new();
    let mut test_image = String::new();
    let mut toolkit = String::new();
    let mut rootfs_version = String::new();
    let mut kernel_version = String::new();
    let mut frecon = false;
    let mut last_mounted = String::new();
    let mut dumped = false;

    for (part_num, partition) in gpt.partitions.iter().enumerate() {
        if partition.is_unused() {
            continue;
        }
        
        let sectors = partition.size_in_sectors();
        if sectors <= 1 {
            continue;
        }
        
        if partition.name == "OEM" {
            continue;
        }
        
        let num = part_num + 1;
        let type_str = partition.type_guid.to_string().to_uppercase();
        let part_path = format!("{}p{}", loopdev, num);

        if num == PART_CROS_STATEFUL as usize {
            // Check stateful partition
            if type_str == partition_types::LINUX_DATA || type_str == partition_types::MS_BASIC_DATA {
                if let Ok(info) = ext2::get_fs_info(&part_path) {
                    last_mounted = info.last_mounted.clone();
                    if last_mounted == "/stateful" {
                        dumped = true;
                    }
                }
                
                // Mount and check for cros_payloads
                let result = utils::mount_partition(&part_path, &mnt, true);
                if result.is_ok() {
                    if mnt.join("cros_payloads").is_dir() {
                        has_cros_payloads = true;
                        
                        // Read metadata
                        let rma_metadata = mnt.join("cros_payloads/rma_metadata.json");
                        if rma_metadata.exists() {
                            if let Ok(content) = fs::read_to_string(&rma_metadata) {
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                                    if let Some(board) = json.get(0).and_then(|v| v.get("board")).and_then(|v| v.as_str()) {
                                        let board_json = mnt.join(format!("cros_payloads/{}.json", board));
                                        if let Ok(board_content) = fs::read_to_string(&board_json) {
                                            if let Ok(board_data) = serde_json::from_str::<serde_json::Value>(&board_content) {
                                                release_image = board_data.get("release_image")
                                                    .and_then(|v| v.get("version"))
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("")
                                                    .to_string();
                                                test_image = board_data.get("test_image")
                                                    .and_then(|v| v.get("version"))
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("")
                                                    .to_string();
                                                toolkit = board_data.get("toolkit")
                                                    .and_then(|v| v.get("version"))
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("")
                                                    .to_string();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    
                    // Check for dev_image/factory
                    let test_lists = mnt.join("dev_image/factory/py/test/test_lists");
                    if test_lists.is_dir() {
                        // Extract models (simplified)
                        if let Ok(entries) = fs::read_dir(&test_lists) {
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if let Ok(content) = fs::read_to_string(&path) {
                                    // Look for model_name patterns
                                    if content.contains("model_name") {
                                        // Simplified extraction
                                        models = "See test_lists".to_string();
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    
                    // Check for HWIDs
                    let hwid_dir = mnt.join("dev_image/factory/hwid");
                    if hwid_dir.is_dir() {
                        if let Ok(entries) = fs::read_dir(&hwid_dir) {
                            for entry in entries.flatten() {
                                let name = entry.file_name().to_string_lossy().to_string();
                                if name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-') {
                                    hwids.push(name);
                                }
                            }
                        }
                    }
                    
                    // Check for toolkit version
                    let toolkit_version = mnt.join("dev_image/factory/TOOLKIT_VERSION");
                    if toolkit_version.exists() && toolkit.is_empty() {
                        if let Ok(content) = fs::read_to_string(&toolkit_version) {
                            toolkit = content.lines().next().unwrap_or("").to_string();
                        }
                    }
                    
                    let _ = utils::unmount_partition(&mnt);
                }
            }
        } else if type_str == partition_types::CHROMEOS_ROOTFS 
               || type_str == partition_types::LINUX_DATA 
               || type_str == partition_types::MS_BASIC_DATA 
        {
            // Check rootfs partitions
            if !ext2::is_ext2(std::path::Path::new(&part_path), 0).unwrap_or(false) {
                continue;
            }
            
            let result = utils::mount_partition(&part_path, &mnt, true);
            if result.is_ok() {
                // Get version from lsb-release
                let lsb_release = mnt.join("etc/lsb-release");
                if lsb_release.exists() {
                    if let Ok(content) = fs::read_to_string(&lsb_release) {
                        for line in content.lines() {
                            if line.starts_with("CHROMEOS_RELEASE_DESCRIPTION=") {
                                let version = line.split('=').nth(1).unwrap_or("");
                                match num as u32 {
                                    3 => rootfs_version = version.to_string(),
                                    5 => test_image = version.to_string(),
                                    7 => release_image = version.to_string(),
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                
                // Get kernel version from modules
                if num == PART_CROS_ROOTFS_A as usize {
                    let modules_dir = mnt.join("lib/modules");
                    if modules_dir.is_dir() {
                        if let Ok(mut entries) = fs::read_dir(&modules_dir) {
                            if let Some(Ok(entry)) = entries.next() {
                                kernel_version = entry.file_name().to_string_lossy().to_string();
                            }
                        }
                    }
                    
                    // Check for frecon
                    let factory_tty = mnt.join("usr/sbin/factory_tty.sh");
                    frecon = factory_tty.exists();
                }
                
                let _ = utils::unmount_partition(&mnt);
            }
        }
    }

    hwids.sort();
    hwids.dedup();
    
    let hwids_str = hwids.join(", ");
    
    println!("Models: {}", models);
    println!("HWIDs: {}", hwids_str);
    println!("Release image: {}", release_image);
    println!("Test image: {}", test_image);
    println!("Toolkit: {}", toolkit);
    println!("Rootfs version: {}", rootfs_version);
    println!("Kernel version: {}", kernel_version);
    println!("Last mounted on: {}", last_mounted);
    println!("Dumped from USB drive: {}", if dumped { "yes" } else { "no" });
    println!("Uses cros_payloads: {}", if has_cros_payloads { "yes" } else { "no" });
    println!("Frecon: {}", if frecon { "yes" } else { "no" });

    Ok(())
}
