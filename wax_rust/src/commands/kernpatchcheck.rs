//! Kernel Patch Check command - Check kernel patch status in a shim image

use anyhow::{bail, Result};
use clap::Args;
use std::path::PathBuf;
use std::fs;

use crate::gpt::{self, Gpt, partition_types};
use crate::utils::{self, log_info, log_warn};

/// Arguments for the kernpatchcheck command
#[derive(Args, Debug)]
pub struct KernPatchCheckArgs {
    /// Image or kernel file to check
    pub input: PathBuf,
}

/// Run the kernpatchcheck command
pub fn run(args: KernPatchCheckArgs, debug: bool) -> Result<()> {
    let missing_deps = utils::check_deps(&["sfdisk", "futility", "binwalk", "lz4", "zcat", "file", "cpio"]);
    if !missing_deps.is_empty() {
        bail!(
            "The following required commands weren't found in PATH:\n{}",
            missing_deps.join("\n")
        );
    }

    let input = &args.input;
    
    if !input.exists() || (!input.is_file() && !utils::is_block_device(input)) {
        bail!("{} doesn't exist or is not a file or block device", input.display());
    }
    
    if fs::metadata(input).is_err() {
        bail!("Cannot read {}, try running as root?", input.display());
    }

    let workdir = tempfile::tempdir()?;
    let user = std::env::var("SUDO_USER").ok();

    // Check if this is a GPT image or a single kernel
    if gpt::check_gpt_image(input).unwrap_or(false) {
        utils::require_root()?;
        
        let loopdev = utils::create_loop_device(input)?;
        let _cleanup = scopeguard::guard(loopdev.clone(), |dev: String| {
            let _ = utils::detach_loop_device(&dev);
        });

        let gpt = Gpt::load_from_file(std::path::Path::new(&loopdev))?;
        
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
            
            let part_path = format!("{}p{}", loopdev, part_num + 1);
            if let Err(e) = check_kernel(&part_path, workdir.path(), user.as_deref(), debug) {
                log_warn(&format!("Error checking {}: {}", part_path, e));
            }
            println!();
        }
    } else {
        if let Err(e) = check_kernel(input.to_str().unwrap(), workdir.path(), user.as_deref(), debug) {
            log_warn(&format!("Error checking kernel: {}", e));
        }
    }

    Ok(())
}

/// Check a single kernel file/partition
fn check_kernel(kernel_path: &str, workdir: &std::path::Path, user: Option<&str>, debug: bool) -> Result<()> {
    println!("{}", kernel_path);
    log_info("Extracting kernel");
    
    let vmlinuz = workdir.join("vmlinuz");
    
    // Try to extract using futility
    let result = utils::shell(
        "futility",
        &["vbutil_kernel", "--get-vmlinuz", kernel_path, "--vmlinuz-out", vmlinuz.to_str().unwrap()],
        true,
        true,
    );
    
    // Check if we got a valid Linux kernel
    let is_linux_kernel = if result.is_ok() && vmlinuz.exists() {
        let file_type = utils::get_file_type(&vmlinuz)?;
        file_type.contains("Linux kernel")
    } else {
        false
    };
    
    if !is_linux_kernel {
        // Try alternative extraction methods
        let _ = fs::remove_file(&vmlinuz);
        
        // Use binwalk to find kernel or compressed data
        let binwalk_output = utils::shell_output("binwalk", &[kernel_path], false)?;
        
        if binwalk_output.contains("Linux kernel") || 
           binwalk_output.contains("LZ4 compressed data") ||
           binwalk_output.contains("gzip compressed data") 
        {
            // Extract the offset and try to decompress
            for line in binwalk_output.lines() {
                if line.contains("Linux kernel") {
                    if let Some(offset_str) = line.split_whitespace().next() {
                        if let Ok(offset) = offset_str.parse::<u64>() {
                            let bin_file = workdir.join("bin");
                            utils::shell(
                                "dd",
                                &[
                                    &format!("if={}", kernel_path),
                                    &format!("of={}", bin_file.display()),
                                    "bs=4194304",
                                    &format!("iflag=skip_bytes"),
                                    &format!("skip={}", offset),
                                ],
                                true,
                                true,
                            )?;
                            fs::rename(&bin_file, &vmlinuz)?;
                            break;
                        }
                    }
                } else if line.contains("LZ4 compressed data") {
                    if let Some(offset_str) = line.split_whitespace().next() {
                        if let Ok(offset) = offset_str.parse::<u64>() {
                            let bin_file = workdir.join("bin");
                            utils::shell(
                                "dd",
                                &[
                                    &format!("if={}", kernel_path),
                                    &format!("of={}", bin_file.display()),
                                    "bs=4194304",
                                    &format!("iflag=skip_bytes"),
                                    &format!("skip={}", offset),
                                ],
                                true,
                                true,
                            )?;
                            let _ = utils::shell("lz4", &["-q", "-d", bin_file.to_str().unwrap(), vmlinuz.to_str().unwrap()], false, true);
                            let _ = fs::remove_file(&bin_file);
                            break;
                        }
                    }
                }
            }
        }
        
        if !vmlinuz.exists() {
            log_warn("Could not extract linux kernel from KERN blob.");
            return Ok(());
        }
    }
    
    log_info("Extracting initramfs");
    let extract_dir = workdir.join("extract");
    fs::create_dir_all(&extract_dir)?;
    
    // Use binwalk to extract initramfs
    let run_as_arg;
    let mut binwalk_args: Vec<&str> = vec![
        "-Me", "-q", "-C", extract_dir.to_str().unwrap(), vmlinuz.to_str().unwrap()
    ];
    if let Some(u) = user {
        run_as_arg = format!("--run-as={}", u);
        binwalk_args.insert(0, &run_as_arg);
    }
    let _ = utils::shell("binwalk", &binwalk_args, false, true);
    
    // Find cpio-root directory
    let cpio_root = find_cpio_root(&extract_dir);
    
    if cpio_root.is_none() {
        log_warn("Could not extract initramfs.");
        return Ok(());
    }
    let cpio_root = cpio_root.unwrap();
    
    // Check for patch status
    let check_file = if cpio_root.join("bin/bootstrap.sh").exists() {
        cpio_root.join("bin/bootstrap.sh")
    } else {
        cpio_root.join("lib/factory_init.sh")
    };
    
    let mount_file = if cpio_root.join("bin/bootstrap.sh").exists() {
        cpio_root.join("bin/bootstrap.sh")
    } else {
        cpio_root.join("init")
    };
    
    if !mount_file.exists() {
        log_warn("Missing /bin/bootstrap.sh, cannot determine patch.");
        return Ok(());
    }
    
    // Check for block_devmode
    if check_file.exists() {
        let content = fs::read_to_string(&check_file)?;
        if content.contains("block_devmode") {
            log_warn("WARNING: initramfs appears to check block_devmode in crossystem.");
            println!("Disable WP to bypass, or hope that crossystem is broken (hana/elm)");
        }
    }
    
    // Check mount status
    let mount_content = fs::read_to_string(&mount_file)?;
    if mount_content.contains("Mounting usb") {
        println!("Not patched!");
    } else if mount_content.contains("Mounting rootfs...") {
        println!("Patched (forced rootfs verification)");
    } else {
        println!("Cannot determine patch.");
    }
    
    Ok(())
}

/// Find cpio-root directory in extraction results
fn find_cpio_root(dir: &std::path::Path) -> Option<PathBuf> {
    for entry in walkdir::WalkDir::new(dir) {
        if let Ok(e) = entry {
            if e.file_type().is_dir() && e.file_name() == "cpio-root" {
                return Some(e.path().to_path_buf());
            }
        }
    }
    None
}
