//! Kernel Signature Check command - Check kernel signatures in a shim image

use anyhow::{bail, Result};
use clap::Args;
use std::path::PathBuf;
use std::fs;
use std::io::{BufRead, BufReader};

use crate::gpt::{self, Gpt, partition_types};
use crate::utils::{self, log_info, log_warn};

/// Arguments for the kernsigcheck command
#[derive(Args, Debug)]
pub struct KernSigCheckArgs {
    /// Image or kernel file to check
    pub input: PathBuf,
}

/// Run the kernsigcheck command
pub fn run(args: KernSigCheckArgs, debug: bool) -> Result<()> {
    let missing_deps = utils::check_deps(&["sfdisk", "futility"]);
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

    // Find keyfile
    let keyfile = find_keyfile()?;
    if !keyfile.exists() {
        bail!("Could not find required key list at {}", keyfile.display());
    }

    let workdir = tempfile::tempdir()?;

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
            if let Err(e) = check_kernel_signature(&part_path, &keyfile, workdir.path()) {
                log_warn(&format!("Error checking {}: {}", part_path, e));
            }
            println!();
        }
    } else {
        if let Err(e) = check_kernel_signature(input.to_str().unwrap(), &keyfile, workdir.path()) {
            log_warn(&format!("Error checking kernel: {}", e));
        }
    }

    Ok(())
}

/// Find the key file
fn find_keyfile() -> Result<PathBuf> {
    // Check relative to executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let keyfile = parent.join("lib/mp-reco-2024-05-03.txt");
            if keyfile.exists() {
                return Ok(keyfile);
            }
            let keyfile = parent.join("mp-reco-2024-05-03.txt");
            if keyfile.exists() {
                return Ok(keyfile);
            }
        }
    }
    
    // Check current directory
    let cwd = std::env::current_dir()?;
    let keyfile = cwd.join("lib/mp-reco-2024-05-03.txt");
    if keyfile.exists() {
        return Ok(keyfile);
    }
    
    let keyfile = cwd.join("wax/lib/mp-reco-2024-05-03.txt");
    if keyfile.exists() {
        return Ok(keyfile);
    }
    
    bail!("Could not find key file");
}

/// Check kernel signature
fn check_kernel_signature(kernel_path: &str, keyfile: &PathBuf, workdir: &std::path::Path) -> Result<()> {
    println!("{}", kernel_path);
    
    // First, verify the kernel passes basic hash verification
    let verify_result = utils::shell_output("futility", &["vbutil_kernel", "--verify", kernel_path], true);
    
    if verify_result.is_err() {
        println!("Did not pass hash verification (invalid)");
        return Ok(());
    }
    
    let verify_output = verify_result.unwrap();
    
    // Extract flags and key sum
    let mut flags = String::new();
    let mut ksum = String::new();
    
    for line in verify_output.lines() {
        if line.contains("Flags:") {
            flags = line.split("Flags:").nth(1).unwrap_or("").trim().to_string();
        } else if line.contains("Data key sha1sum:") {
            ksum = line.split("Data key sha1sum:").nth(1).unwrap_or("").trim().to_string();
        }
    }
    
    println!("Allowed boot mode flags: {}", flags);
    println!("Kernel key sum: {}", ksum);
    
    // Try each key in the keyfile
    let file = fs::File::open(keyfile)?;
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();
    let total = lines.len();
    
    for (i, keyline) in lines.iter().enumerate() {
        print!("\rTrying key {}/{}  ", i + 1, total);
        use std::io::Write;
        std::io::stdout().flush()?;
        
        let parts: Vec<&str> = keyline.split(';').collect();
        if parts.len() < 6 {
            continue;
        }
        
        let name = parts[0];
        let key_type = parts[1];
        let ver = parts[2];
        let _algo = parts[3];
        let fsum = parts[4];
        let key_b64 = parts[5];
        
        // Decode and write key
        let key_data = base64::decode(key_b64)?;
        let key_path = workdir.join("key.vbpubk");
        fs::write(&key_path, &key_data)?;
        
        // Try to verify with this key
        let result = utils::shell(
            "futility",
            &["vbutil_kernel", "--verify", kernel_path, "--signpubkey", key_path.to_str().unwrap()],
            true,
            true,
        );
        
        if result.is_ok() {
            println!();
            println!("Verified by key: {} {} v{}", name, key_type, ver);
            println!("Will boot on firmware with gbb.recovery_key: {}", fsum);
            
            if name == "developer" {
                log_warn("WARNING: Developer key detected. CTRL+U boot only on production firmware.");
            }
            
            return Ok(());
        }
        
        if i + 1 == total {
            println!();
            println!("Not verified by any known recovery key.");
        }
    }
    
    Ok(())
}
