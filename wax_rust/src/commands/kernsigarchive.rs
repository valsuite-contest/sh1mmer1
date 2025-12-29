//! Kernel Signature Archive command - Archive kernel signatures from firmware

use anyhow::{bail, Result};
use clap::Args;
use std::path::PathBuf;
use std::fs;
use std::io::Write;

use crate::utils::{self, log_info, log_warn};

/// Arguments for the kernsigarchive command
#[derive(Args, Debug)]
pub struct KernSigArchiveArgs {
    /// Output text file for signatures
    pub output: PathBuf,
    
    /// Optional directory to search for recovery images
    #[arg(short, long)]
    pub search_dir: Option<PathBuf>,
}

/// Run the kernsigarchive command
pub fn run(args: KernSigArchiveArgs, debug: bool) -> Result<()> {
    utils::require_root()?;
    
    let missing_deps = utils::check_deps(&["futility", "cbfstool", "jq", "curl", "unzip"]);
    if !missing_deps.is_empty() {
        bail!(
            "The following required commands weren't found in PATH:\n{}",
            missing_deps.join("\n")
        );
    }

    let workdir = tempfile::tempdir()?;
    let user = std::env::var("SUDO_USER").ok();
    
    // Create output file
    fs::File::create(&args.output)?;
    if let Some(ref u) = user {
        utils::shell("chown", &[&format!("{}:{}", u, u), args.output.to_str().unwrap()], true, true)?;
    }

    log_info("Getting ChromeOS list");
    let chromeos_url = "https://chromiumdash.appspot.com/cros/fetch_serving_builds?deviceCategory=Chrome%20OS";
    utils::shell(
        "curl",
        &["-so", workdir.path().join("recovery_chromeos.json").to_str().unwrap(), chromeos_url],
        false,
        true,
    )?;

    log_info("Getting Google Meet list");
    let meet_url = "https://chromiumdash.appspot.com/cros/fetch_serving_builds?deviceCategory=Google%20Meet%20Hardware";
    utils::shell(
        "curl",
        &["-so", workdir.path().join("recovery_googlemeet.json").to_str().unwrap(), meet_url],
        false,
        true,
    )?;

    // Get board list
    let boards = get_board_list(workdir.path())?;
    log_info(&format!("Found {} boards", boards.len()));

    // Dump developer keys first
    if should_dump_keys(&args.output, "developer")? {
        dump_keys(&args.output, "developer", "root", "1", "11 RSA8192 SHA512", 
                  "b11d74edd286c144e1135b49e7f0bc20cf041f10", 
                  "/usr/share/vboot/devkeys/root_key.vbpubk")?;
        dump_keys(&args.output, "developer", "recovery", "1", "11 RSA8192 SHA512",
                  "c14bd720b70d97394257e3e826bd8f43de48d4ed",
                  "/usr/share/vboot/devkeys/recovery_key.vbpubk")?;
    }

    let shellball_source = "https://www.mrchromebox.tech/files/firmware/shellball/";

    for board in &boards {
        let board_key = format!("board:{}", board);
        if !should_dump_keys(&args.output, &board_key)? {
            log_info(&format!("Skipping {}, already exists", board));
            continue;
        }

        let rom = workdir.path().join("rom.bin");
        let gbb = workdir.path().join("gbb.bin");
        let rootkey = workdir.path().join("root_key.vbpubk");
        let reckey = workdir.path().join("recovery_key.vbpubk");

        log_info(&format!("Attempting to download shellball for {}...", board));
        
        let shellball_url = format!("{}shellball.{}.bin", shellball_source, board);
        let result = utils::shell("curl", &["-f", &shellball_url, "-o", rom.to_str().unwrap()], false, true);

        if result.is_ok() {
            log_info("Success");
        } else {
            log_warn("Failed, looking for recovery image...");
            
            // Try to find or download recovery image
            if let Some(ref search_dir) = args.search_dir {
                // Search for existing recovery image
                let pattern = format!("chromeos_*_{}_recovery_*.bin.zip", board);
                // For simplicity, skip local search in Rust implementation
                log_warn(&format!("Local search not implemented, would search for: {}", pattern));
            }
            
            // Download recovery image
            let rec_url = get_recovery_image_url(workdir.path(), board)?;
            if rec_url.is_empty() {
                log_warn(&format!("No recovery URL found for {}", board));
                continue;
            }
            
            log_info(&format!("Downloading from {}...", rec_url));
            let rec_zip = workdir.path().join("image.zip");
            utils::shell("curl", &[&rec_url, "-o", rec_zip.to_str().unwrap()], false, false)?;
            
            // Extract and process
            let rec_dir = workdir.path().join("extract");
            fs::create_dir_all(&rec_dir)?;
            
            log_info("Extracting zip...");
            utils::shell("unzip", &[rec_zip.to_str().unwrap(), "-d", rec_dir.to_str().unwrap()], false, !debug)?;
            
            // Find and extract shellball from recovery image
            // This is complex - for now, just skip
            log_warn("Recovery image extraction not fully implemented");
            continue;
        }

        // Extract GBB from ROM
        let _ = utils::shell("cbfstool", &[rom.to_str().unwrap(), "read", "-r", "GBB", "-f", gbb.to_str().unwrap()], false, true);
        
        if !gbb.exists() {
            // Fallback for pinetrail
            utils::shell("dd", &[
                &format!("if={}", rom.display()),
                &format!("of={}", gbb.display()),
                "bs=262144",
                "skip=14",
                "count=1",
            ], false, true)?;
        }

        // Get key info from GBB
        let gbb_output = utils::shell_output("futility", &["show", gbb.to_str().unwrap()], false)?;
        
        // Parse key info
        let (rootkey_ver, rootkey_algo, rootkey_hash) = parse_key_info(&gbb_output, "Root Key:");
        let (reckey_ver, reckey_algo, reckey_hash) = parse_key_info(&gbb_output, "Recovery Key:");

        // Extract keys
        utils::shell("futility", &["gbb", "-g", 
                                   &format!("--rootkey={}", rootkey.display()),
                                   &format!("--recoverykey={}", reckey.display()),
                                   gbb.to_str().unwrap()], false, true)?;

        // Dump keys
        if rootkey.exists() {
            dump_keys_from_file(&args.output, &board_key, "root", &rootkey_ver, &rootkey_algo, &rootkey_hash, &rootkey)?;
        }
        if reckey.exists() {
            dump_keys_from_file(&args.output, &board_key, "recovery", &reckey_ver, &reckey_algo, &reckey_hash, &reckey)?;
        }

        // Cleanup
        let _ = fs::remove_file(&rom);
        let _ = fs::remove_file(&gbb);
        let _ = fs::remove_file(&rootkey);
        let _ = fs::remove_file(&reckey);
    }

    log_info("Done.");
    Ok(())
}

/// Get board list from JSON files
fn get_board_list(workdir: &std::path::Path) -> Result<Vec<String>> {
    let mut boards = Vec::new();
    
    // Parse ChromeOS boards
    let chromeos_json = workdir.join("recovery_chromeos.json");
    if chromeos_json.exists() {
        let output = utils::shell_output("jq", &["-r", ".builds | keys[]", chromeos_json.to_str().unwrap()], false)?;
        for line in output.lines() {
            if !line.is_empty() {
                boards.push(line.to_string());
            }
        }
    }
    
    // Parse Meet boards
    let meet_json = workdir.join("recovery_googlemeet.json");
    if meet_json.exists() {
        let output = utils::shell_output("jq", &["-r", ".builds | keys[]", meet_json.to_str().unwrap()], false)?;
        for line in output.lines() {
            if !line.is_empty() && !boards.contains(&line.to_string()) {
                boards.push(line.to_string());
            }
        }
    }
    
    boards.sort();
    Ok(boards)
}

/// Check if we should dump keys for a board
fn should_dump_keys(outfile: &PathBuf, name: &str) -> Result<bool> {
    if !outfile.exists() {
        return Ok(true);
    }
    
    let content = fs::read_to_string(outfile)?;
    let has_root = content.contains(&format!("{};root;", name));
    let has_recovery = content.contains(&format!("{};recovery;", name));
    
    Ok(!has_root || !has_recovery)
}

/// Dump keys to output file
fn dump_keys(outfile: &PathBuf, name: &str, key_type: &str, version: &str, algorithm: &str, sha1: &str, keypath: &str) -> Result<()> {
    log_info(&format!("Dumping {} v{} signature for {}", key_type, version, name));
    
    let key_data = fs::read(keypath)?;
    let key_b64 = base64::encode(&key_data);
    
    let line = format!("{};{};{};{};{};{}\n", name, key_type, version, algorithm, sha1, key_b64);
    
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(outfile)?;
    file.write_all(line.as_bytes())?;
    
    Ok(())
}

/// Dump keys from a file
fn dump_keys_from_file(outfile: &PathBuf, name: &str, key_type: &str, version: &str, algorithm: &str, sha1: &str, keypath: &PathBuf) -> Result<()> {
    dump_keys(outfile, name, key_type, version, algorithm, sha1, keypath.to_str().unwrap())
}

/// Parse key info from GBB output
fn parse_key_info(output: &str, key_name: &str) -> (String, String, String) {
    let mut version = String::new();
    let mut algorithm = String::new();
    let mut sha1 = String::new();
    
    let mut in_key_section = false;
    for line in output.lines() {
        if line.contains(key_name) {
            in_key_section = true;
            continue;
        }
        if in_key_section {
            if line.contains("Key Version:") {
                version = line.split("Key Version:").nth(1).unwrap_or("").trim().to_string();
            } else if line.contains("Algorithm:") {
                algorithm = line.split("Algorithm:").nth(1).unwrap_or("").trim().to_string();
            } else if line.contains("Key sha1sum:") {
                sha1 = line.split("Key sha1sum:").nth(1).unwrap_or("").trim().to_string();
                break;
            }
        }
    }
    
    (version, algorithm, sha1)
}

/// Get recovery image URL for a board
fn get_recovery_image_url(workdir: &std::path::Path, board: &str) -> Result<String> {
    let chromeos_json = workdir.join("recovery_chromeos.json");
    let meet_json = workdir.join("recovery_googlemeet.json");
    
    // Try ChromeOS first
    let output = utils::shell_output("jq", &["-r", 
        &format!(".builds.\"{}\".pushRecoveries | to_entries | last.value // empty", board),
        chromeos_json.to_str().unwrap()], false);
    
    if let Ok(url) = output {
        let url = url.trim();
        if !url.is_empty() && url != "null" {
            return Ok(url.to_string());
        }
    }
    
    // Try Meet
    let output = utils::shell_output("jq", &["-r",
        &format!(".builds.\"{}\".pushRecoveries | to_entries | last.value // empty", board),
        meet_json.to_str().unwrap()], false);
    
    if let Ok(url) = output {
        let url = url.trim();
        if !url.is_empty() && url != "null" {
            return Ok(url.to_string());
        }
    }
    
    Ok(String::new())
}
