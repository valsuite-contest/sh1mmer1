//! ext2/ext4 filesystem utilities for M1NSH1M
//!
//! Handles operations related to ext2/ext4 filesystems including
//! rootfs verification manipulation.

use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// ext2 superblock magic number offset
const EXT2_SB_MAGIC_OFFSET: u64 = 0x438;
/// ext2 magic number
const EXT2_MAGIC: &[u8; 2] = &[0x53, 0xEF]; // '\123\357' in octal

/// Read-only compatible feature flags offset
const EXT2_RO_COMPAT_OFFSET: u64 = 0x464 + 3;

/// Check if a partition/file contains an ext2/ext3/ext4 filesystem
pub fn is_ext2(path: &Path, offset: u64) -> Result<bool> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset + EXT2_SB_MAGIC_OFFSET))?;
    
    let mut magic = [0u8; 2];
    if file.read_exact(&mut magic).is_err() {
        return Ok(false);
    }
    
    Ok(&magic == EXT2_MAGIC)
}

/// Enable read-write mounting for ChromeOS rootfs
/// 
/// ChromeOS rootfs partitions have a special flag that prevents mounting as read-write.
/// This function clears that flag to allow modifications.
pub fn enable_rw_mount(path: &Path, offset: u64) -> Result<()> {
    if !is_ext2(path, offset)? {
        bail!("enable_rw_mount called on non-ext2 filesystem");
    }
    
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.seek(SeekFrom::Start(offset + EXT2_RO_COMPAT_OFFSET))?;
    file.write_all(&[0x00])?;
    file.flush()?;
    
    Ok(())
}

/// Disable read-write mounting for ChromeOS rootfs
///
/// This restores the ChromeOS rootfs protection flag after modifications.
pub fn disable_rw_mount(path: &Path, offset: u64) -> Result<()> {
    if !is_ext2(path, offset)? {
        bail!("disable_rw_mount called on non-ext2 filesystem");
    }
    
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.seek(SeekFrom::Start(offset + EXT2_RO_COMPAT_OFFSET))?;
    file.write_all(&[0xFF])?;
    file.flush()?;
    
    Ok(())
}

/// Check if read-write mounting is disabled for this filesystem
pub fn rw_mount_disabled(path: &Path, offset: u64) -> Result<bool> {
    if !is_ext2(path, offset)? {
        return Ok(false);
    }
    
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset + EXT2_RO_COMPAT_OFFSET))?;
    
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte)?;
    
    Ok(byte[0] == 0xFF)
}

/// Get filesystem information using tune2fs
pub fn get_fs_info(device: &str) -> Result<FilesystemInfo> {
    let output = crate::utils::shell_output("tune2fs", &["-l", device], true)?;
    
    let mut info = FilesystemInfo::default();
    
    for line in output.lines() {
        if line.starts_with("Block count:") {
            info.block_count = line
                .split_whitespace()
                .last()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        } else if line.starts_with("Block size:") {
            info.block_size = line
                .split_whitespace()
                .last()
                .and_then(|s| s.parse().ok())
                .unwrap_or(4096);
        } else if line.starts_with("Filesystem volume name:") {
            info.volume_name = line
                .split(':')
                .nth(1)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
        } else if line.starts_with("Last mounted on:") {
            info.last_mounted = line
                .split(':')
                .nth(1)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
        }
    }
    
    Ok(info)
}

/// Filesystem information
#[derive(Debug, Default)]
pub struct FilesystemInfo {
    pub block_count: u64,
    pub block_size: u64,
    pub volume_name: String,
    pub last_mounted: String,
}

impl FilesystemInfo {
    /// Get the total size of the filesystem in bytes
    pub fn size(&self) -> u64 {
        self.block_count * self.block_size
    }
}

/// Run e2fsck (filesystem check) on a partition
pub fn check_filesystem(device: &str) -> Result<()> {
    // e2fsck may return 1 "errors corrected" or 2 "corrected and need reboot"
    let result = crate::utils::shell("e2fsck", &["-fy", device], true, false)?;
    if !result {
        // Check return code - codes 0, 1, 2 are acceptable
        crate::utils::log_warn("e2fsck reported issues that were corrected");
    }
    Ok(())
}

/// Resize an ext2/ext4 filesystem
pub fn resize_filesystem(device: &str, size: Option<u64>) -> Result<()> {
    // First run fsck
    check_filesystem(device)?;
    
    let mut args = vec!["-f", device];
    
    // If size specified, use it in megabytes
    let size_str;
    if let Some(size) = size {
        size_str = format!("{}M", size / (1024 * 1024));
        args.push(&size_str);
    }
    
    crate::utils::shell("resize2fs", &args, true, false)?;
    Ok(())
}

/// Resize filesystem to minimum size
pub fn shrink_filesystem_to_minimum(device: &str) -> Result<()> {
    // First run fsck
    check_filesystem(device)?;
    
    // Use -M flag to shrink to minimum
    crate::utils::shell("resize2fs", &["-M", "-p", device], true, false)?;
    Ok(())
}

/// Create an ext2 filesystem
pub fn mkfs_ext2(device: &str, label: &str) -> Result<()> {
    crate::utils::shell("mkfs.ext2", &["-F", "-b", "4096", "-L", label, device], true, false)?;
    Ok(())
}

/// Create an ext4 filesystem
pub fn mkfs_ext4(device: &str, label: &str) -> Result<()> {
    crate::utils::shell("mkfs.ext4", &["-F", "-b", "4096", "-L", label, device], true, false)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_is_ext2_not_ext2() {
        let file = NamedTempFile::new().unwrap();
        // Write some random data
        std::fs::write(file.path(), vec![0u8; 4096]).unwrap();
        
        assert!(!is_ext2(file.path(), 0).unwrap());
    }
}
