//! Utility functions for M1NSH1M

use anyhow::{anyhow, bail, Context, Result};
use colored::Colorize;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// Log an info message
pub fn log_info(msg: &str) {
    println!("{} {}", "Info:".green(), msg);
}

/// Log a debug message (only if debug mode is enabled)
pub fn log_debug(msg: &str) {
    log::debug!("{}", msg);
}

/// Log a warning message
pub fn log_warn(msg: &str) {
    eprintln!("{} {}", "Warning:".yellow(), msg);
}

/// Log an error message
pub fn log_error(msg: &str) {
    eprintln!("{} {}", "Error:".red().bold(), msg);
}

/// Fail with an error message
pub fn fail(msg: &str) -> ! {
    log_error(msg);
    std::process::exit(1);
}

/// Check if running as root
pub fn check_root() -> bool {
    nix::unistd::geteuid().is_root()
}

/// Require root privileges
pub fn require_root() -> Result<()> {
    if !check_root() {
        bail!("Please run as root");
    }
    Ok(())
}

/// Check if a command exists in PATH
pub fn command_exists(cmd: &str) -> bool {
    which::which(cmd).is_ok()
}

/// Check for required dependencies
pub fn check_deps(deps: &[&str]) -> Vec<String> {
    let mut missing = Vec::new();
    for dep in deps {
        if !command_exists(dep) {
            missing.push(dep.to_string());
        }
    }
    missing
}

/// Execute a shell command and return success status
pub fn shell(cmd: &str, args: &[&str], sudo: bool, silent: bool) -> Result<bool> {
    let mut command = if sudo && !check_root() {
        let mut c = Command::new("sudo");
        c.arg("-E").arg(cmd);
        c
    } else {
        Command::new(cmd)
    };
    
    command.args(args);
    
    if silent {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    
    let status = command.status().context(format!("Failed to execute {}", cmd))?;
    Ok(status.success())
}

/// Execute a shell command and return the output
pub fn shell_output(cmd: &str, args: &[&str], sudo: bool) -> Result<String> {
    let mut command = if sudo && !check_root() {
        let mut c = Command::new("sudo");
        c.arg("-E").arg(cmd);
        c
    } else {
        Command::new(cmd)
    };
    
    command.args(args);
    
    let output = command.output().context(format!("Failed to execute {}", cmd))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Command {} failed: {}", cmd, stderr);
    }
    
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Suppress command output based on debug flag
pub fn suppress<F>(func: F, debug: bool) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    if debug {
        func()
    } else {
        // Suppress output
        func()
    }
}

/// Parse size strings like "4M", "1G", "512K" to bytes
pub fn parse_bytes(s: &str) -> Result<u64> {
    let s = s.trim().to_uppercase();
    let (num_str, multiplier) = if s.ends_with("G") || s.ends_with("GIB") {
        (s.trim_end_matches(|c| c == 'G' || c == 'I' || c == 'B'), 1024 * 1024 * 1024u64)
    } else if s.ends_with("M") || s.ends_with("MIB") {
        (s.trim_end_matches(|c| c == 'M' || c == 'I' || c == 'B'), 1024 * 1024u64)
    } else if s.ends_with("K") || s.ends_with("KIB") {
        (s.trim_end_matches(|c| c == 'K' || c == 'I' || c == 'B'), 1024u64)
    } else {
        (s.as_str(), 1u64)
    };
    
    let num: u64 = num_str.parse().context(format!("Invalid size: {}", s))?;
    Ok(num * multiplier)
}

/// Format bytes to human-readable string
pub fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;
    
    if bytes >= GIB {
        format!("{:.1}GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1}MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1}KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{}B", bytes)
    }
}

/// Check if a file exists and is readable/writable
pub fn check_file_rw(path: &Path) -> bool {
    path.is_file() && fs::metadata(path).map(|m| !m.permissions().readonly()).unwrap_or(false)
}

/// Check if a path is a block device
pub fn is_block_device(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    fs::metadata(path)
        .map(|m| m.file_type().is_block_device())
        .unwrap_or(false)
}

/// Sync filesystem
pub fn safesync() {
    let _ = Command::new("sync").status();
    std::thread::sleep(std::time::Duration::from_millis(200));
}

/// Create a loop device for the given image
pub fn create_loop_device(image: &Path) -> Result<String> {
    let output = shell_output("losetup", &["--show", "-f", "-P", image.to_str().unwrap()], true)?;
    Ok(output.trim().to_string())
}

/// Detach a loop device
pub fn detach_loop_device(loopdev: &str) -> Result<()> {
    shell("losetup", &["-d", loopdev], true, true)?;
    Ok(())
}

/// Create a temporary directory
pub fn create_temp_dir() -> Result<tempfile::TempDir> {
    tempfile::tempdir().context("Failed to create temporary directory")
}

/// Mount a partition
pub fn mount_partition(device: &str, mount_point: &Path, read_only: bool) -> Result<()> {
    let mut args = vec![device, mount_point.to_str().unwrap()];
    if read_only {
        args.insert(0, "-o");
        args.insert(1, "ro");
    }
    shell("mount", &args, true, false)?;
    Ok(())
}

/// Unmount a partition
pub fn unmount_partition(mount_point: &Path) -> Result<()> {
    shell("umount", &[mount_point.to_str().unwrap()], true, true)?;
    Ok(())
}

/// Read bytes from a file at a specific offset
pub fn read_bytes_at(path: &Path, offset: u64, count: usize) -> Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = vec![0u8; count];
    file.read_exact(&mut buffer)?;
    Ok(buffer)
}

/// Write bytes to a file at a specific offset
pub fn write_bytes_at(path: &Path, offset: u64, data: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(data)?;
    file.flush()?;
    Ok(())
}

/// Truncate a file to a specific size
pub fn truncate_file(path: &Path, size: u64) -> Result<()> {
    let file = OpenOptions::new().write(true).open(path)?;
    file.set_len(size)?;
    Ok(())
}

/// Copy file preserving permissions
pub fn copy_file(src: &Path, dst: &Path) -> Result<()> {
    fs::copy(src, dst).context(format!(
        "Failed to copy {} to {}",
        src.display(),
        dst.display()
    ))?;
    Ok(())
}

/// Copy directory recursively
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }
    
    for entry in walkdir::WalkDir::new(src) {
        let entry = entry?;
        let rel_path = entry.path().strip_prefix(src)?;
        let target = dst.join(rel_path);
        
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Get the architecture of the current system
pub fn get_architecture() -> Result<String> {
    let arch = std::env::consts::ARCH;
    match arch {
        "x86_64" => Ok("x86_64".to_string()),
        "aarch64" => Ok("aarch64".to_string()),
        "x86" => Ok("i386".to_string()),
        _ => bail!("Unsupported architecture: {}", arch),
    }
}

/// Check if we're running on WSL with a slow filesystem
pub fn check_slow_fs(path: &Path) -> Result<()> {
    // Check if running in WSL
    let uname = shell_output("uname", &["-r"], false)?;
    if uname.to_lowercase().contains("microsoft") {
        let real_path = fs::canonicalize(path)?;
        if real_path.starts_with("/mnt") {
            bail!(
                "You are attempting to run m1nsh1m on a file in your Windows filesystem.\n\
                Performance would suffer, so please move your file into your Linux filesystem (e.g. ~/file.bin)"
            );
        }
    }
    Ok(())
}

/// Get the file type using the 'file' command
pub fn get_file_type(path: &Path) -> Result<String> {
    let output = shell_output("file", &["-b", path.to_str().unwrap()], false)?;
    Ok(output.trim().to_string())
}

/// Detect architecture from a binary file
pub fn detect_binary_arch(path: &Path) -> Result<String> {
    let file_info = get_file_type(path)?;
    let file_info_lower = file_info.to_lowercase();
    
    if file_info_lower.contains("aarch64") || file_info_lower.contains("armv8") || file_info_lower.contains("arm") {
        Ok("aarch64".to_string())
    } else {
        Ok("x86_64".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bytes() {
        assert_eq!(parse_bytes("1024").unwrap(), 1024);
        assert_eq!(parse_bytes("1K").unwrap(), 1024);
        assert_eq!(parse_bytes("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_bytes("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_bytes("4M").unwrap(), 4 * 1024 * 1024);
        assert_eq!(parse_bytes("72M").unwrap(), 72 * 1024 * 1024);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(1024), "1.0KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0MiB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0GiB");
    }
}
