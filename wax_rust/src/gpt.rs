//! GPT (GUID Partition Table) handling for M1NSH1M
//!
//! This module provides functionality to read and manipulate GPT partition tables,
//! similar to cgpt functionality.

use anyhow::{anyhow, bail, Context, Result};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use uuid::Uuid;

/// GPT Header magic signature
const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";

/// Default block/sector size
pub const DEFAULT_BLOCK_SIZE: u64 = 512;

/// ChromeOS partition type GUIDs
pub mod partition_types {
    /// ChromeOS kernel partition type
    pub const CHROMEOS_KERNEL: &str = "FE3A2A5D-4F32-41A7-B725-ACCC3285A309";
    /// ChromeOS rootfs partition type
    pub const CHROMEOS_ROOTFS: &str = "3CB8E202-3B7E-47DD-8A3C-7FF2A13CFCEC";
    /// Linux data partition type
    pub const LINUX_DATA: &str = "0FC63DAF-8483-4772-8E79-3D69D8477DE4";
    /// Microsoft basic data partition type  
    pub const MS_BASIC_DATA: &str = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7";
}

/// ChromeOS partition indices
pub const PART_CROS_STATEFUL: u32 = 1;
pub const PART_CROS_KERNEL_A: u32 = 2;
pub const PART_CROS_ROOTFS_A: u32 = 3;
pub const PART_CROS_KERNEL_B: u32 = 4;
pub const PART_CROS_ROOTFS_B: u32 = 5;

/// GPT Header structure
#[derive(Debug, Clone)]
pub struct GptHeader {
    pub signature: [u8; 8],
    pub revision: u32,
    pub header_size: u32,
    pub header_crc32: u32,
    pub reserved: u32,
    pub my_lba: u64,
    pub alternate_lba: u64,
    pub first_usable_lba: u64,
    pub last_usable_lba: u64,
    pub disk_guid: Uuid,
    pub partition_entry_lba: u64,
    pub num_partition_entries: u32,
    pub partition_entry_size: u32,
    pub partition_entries_crc32: u32,
}

impl GptHeader {
    /// Read GPT header from file at given offset
    pub fn read_from<R: Read + Seek>(reader: &mut R, offset: u64) -> Result<Self> {
        reader.seek(SeekFrom::Start(offset))?;
        
        let mut signature = [0u8; 8];
        reader.read_exact(&mut signature)?;
        
        if &signature != GPT_SIGNATURE {
            bail!("Invalid GPT signature");
        }
        
        let revision = reader.read_u32::<LittleEndian>()?;
        let header_size = reader.read_u32::<LittleEndian>()?;
        let header_crc32 = reader.read_u32::<LittleEndian>()?;
        let reserved = reader.read_u32::<LittleEndian>()?;
        let my_lba = reader.read_u64::<LittleEndian>()?;
        let alternate_lba = reader.read_u64::<LittleEndian>()?;
        let first_usable_lba = reader.read_u64::<LittleEndian>()?;
        let last_usable_lba = reader.read_u64::<LittleEndian>()?;
        
        let mut guid_bytes = [0u8; 16];
        reader.read_exact(&mut guid_bytes)?;
        let disk_guid = Uuid::from_bytes_le(guid_bytes);
        
        let partition_entry_lba = reader.read_u64::<LittleEndian>()?;
        let num_partition_entries = reader.read_u32::<LittleEndian>()?;
        let partition_entry_size = reader.read_u32::<LittleEndian>()?;
        let partition_entries_crc32 = reader.read_u32::<LittleEndian>()?;
        
        Ok(GptHeader {
            signature,
            revision,
            header_size,
            header_crc32,
            reserved,
            my_lba,
            alternate_lba,
            first_usable_lba,
            last_usable_lba,
            disk_guid,
            partition_entry_lba,
            num_partition_entries,
            partition_entry_size,
            partition_entries_crc32,
        })
    }
    
    /// Write GPT header to file at given offset
    pub fn write_to<W: Write + Seek>(&self, writer: &mut W, offset: u64) -> Result<()> {
        writer.seek(SeekFrom::Start(offset))?;
        
        writer.write_all(&self.signature)?;
        writer.write_u32::<LittleEndian>(self.revision)?;
        writer.write_u32::<LittleEndian>(self.header_size)?;
        writer.write_u32::<LittleEndian>(self.header_crc32)?;
        writer.write_u32::<LittleEndian>(self.reserved)?;
        writer.write_u64::<LittleEndian>(self.my_lba)?;
        writer.write_u64::<LittleEndian>(self.alternate_lba)?;
        writer.write_u64::<LittleEndian>(self.first_usable_lba)?;
        writer.write_u64::<LittleEndian>(self.last_usable_lba)?;
        writer.write_all(&self.disk_guid.to_bytes_le())?;
        writer.write_u64::<LittleEndian>(self.partition_entry_lba)?;
        writer.write_u32::<LittleEndian>(self.num_partition_entries)?;
        writer.write_u32::<LittleEndian>(self.partition_entry_size)?;
        writer.write_u32::<LittleEndian>(self.partition_entries_crc32)?;
        
        Ok(())
    }
}

/// GPT Partition Entry
#[derive(Debug, Clone)]
pub struct GptPartitionEntry {
    pub type_guid: Uuid,
    pub unique_guid: Uuid,
    pub first_lba: u64,
    pub last_lba: u64,
    pub attributes: u64,
    pub name: String,
}

impl GptPartitionEntry {
    /// Create an empty/unused partition entry
    pub fn empty() -> Self {
        GptPartitionEntry {
            type_guid: Uuid::nil(),
            unique_guid: Uuid::nil(),
            first_lba: 0,
            last_lba: 0,
            attributes: 0,
            name: String::new(),
        }
    }
    
    /// Check if this partition entry is unused
    pub fn is_unused(&self) -> bool {
        self.type_guid.is_nil()
    }
    
    /// Read partition entry from reader
    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self> {
        let mut type_guid_bytes = [0u8; 16];
        reader.read_exact(&mut type_guid_bytes)?;
        let type_guid = Uuid::from_bytes_le(type_guid_bytes);
        
        let mut unique_guid_bytes = [0u8; 16];
        reader.read_exact(&mut unique_guid_bytes)?;
        let unique_guid = Uuid::from_bytes_le(unique_guid_bytes);
        
        let mut buf = [0u8; 8];
        reader.read_exact(&mut buf)?;
        let first_lba = u64::from_le_bytes(buf);
        
        reader.read_exact(&mut buf)?;
        let last_lba = u64::from_le_bytes(buf);
        
        reader.read_exact(&mut buf)?;
        let attributes = u64::from_le_bytes(buf);
        
        let mut name_bytes = [0u8; 72];
        reader.read_exact(&mut name_bytes)?;
        
        // Parse UTF-16LE name
        let name_u16: Vec<u16> = name_bytes
            .chunks(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .take_while(|&c| c != 0)
            .collect();
        let name = String::from_utf16_lossy(&name_u16);
        
        Ok(GptPartitionEntry {
            type_guid,
            unique_guid,
            first_lba,
            last_lba,
            attributes,
            name,
        })
    }
    
    /// Write partition entry to writer
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_all(&self.type_guid.to_bytes_le())?;
        writer.write_all(&self.unique_guid.to_bytes_le())?;
        writer.write_all(&self.first_lba.to_le_bytes())?;
        writer.write_all(&self.last_lba.to_le_bytes())?;
        writer.write_all(&self.attributes.to_le_bytes())?;
        
        // Write UTF-16LE name
        let mut name_bytes = [0u8; 72];
        let name_u16: Vec<u16> = self.name.encode_utf16().collect();
        for (i, &c) in name_u16.iter().take(36).enumerate() {
            let bytes = c.to_le_bytes();
            name_bytes[i * 2] = bytes[0];
            name_bytes[i * 2 + 1] = bytes[1];
        }
        writer.write_all(&name_bytes)?;
        
        Ok(())
    }
    
    /// Get the size of this partition in sectors
    pub fn size_in_sectors(&self) -> u64 {
        if self.is_unused() {
            0
        } else {
            self.last_lba - self.first_lba + 1
        }
    }
    
    /// Get the size of this partition in bytes
    pub fn size_in_bytes(&self, block_size: u64) -> u64 {
        self.size_in_sectors() * block_size
    }
}

/// Complete GPT representation
#[derive(Debug)]
pub struct Gpt {
    pub path: String,
    pub block_size: u64,
    pub primary_header: GptHeader,
    pub backup_header: Option<GptHeader>,
    pub partitions: Vec<GptPartitionEntry>,
}

impl Gpt {
    /// Load GPT from a file or device
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let mut file = File::open(path).context(format!("Failed to open {}", path.display()))?;
        
        // Detect block size - try 512 first, then 4096
        let block_size = Self::detect_block_size(&mut file)?;
        
        // Read primary GPT header (at LBA 1)
        let primary_header = GptHeader::read_from(&mut file, block_size)?;
        
        // Read partition entries
        let partition_entry_offset = primary_header.partition_entry_lba * block_size;
        file.seek(SeekFrom::Start(partition_entry_offset))?;
        
        let mut partitions = Vec::new();
        for _ in 0..primary_header.num_partition_entries {
            let entry = GptPartitionEntry::read_from(&mut file)?;
            partitions.push(entry);
            
            // Skip any padding in entry size
            let remaining = primary_header.partition_entry_size as i64 - 128;
            if remaining > 0 {
                file.seek(SeekFrom::Current(remaining))?;
            }
        }
        
        // Try to read backup header
        let file_size = file.seek(SeekFrom::End(0))?;
        let backup_lba = (file_size / block_size) - 1;
        let backup_header = GptHeader::read_from(&mut file, backup_lba * block_size).ok();
        
        Ok(Gpt {
            path: path.to_string_lossy().to_string(),
            block_size,
            primary_header,
            backup_header,
            partitions,
        })
    }
    
    /// Detect the block size of the device/image
    fn detect_block_size<R: Read + Seek>(reader: &mut R) -> Result<u64> {
        // Try 512-byte sectors first
        if let Ok(header) = GptHeader::read_from(reader, 512) {
            if &header.signature == GPT_SIGNATURE {
                return Ok(512);
            }
        }
        
        // Try 4096-byte sectors
        if let Ok(header) = GptHeader::read_from(reader, 4096) {
            if &header.signature == GPT_SIGNATURE {
                return Ok(4096);
            }
        }
        
        // Default to 512
        Ok(512)
    }
    
    /// Get a specific partition by number (1-based)
    pub fn get_partition(&self, number: u32) -> Option<&GptPartitionEntry> {
        if number == 0 || number as usize > self.partitions.len() {
            None
        } else {
            Some(&self.partitions[number as usize - 1])
        }
    }
    
    /// Get a mutable reference to a specific partition
    pub fn get_partition_mut(&mut self, number: u32) -> Option<&mut GptPartitionEntry> {
        if number == 0 || number as usize > self.partitions.len() {
            None
        } else {
            Some(&mut self.partitions[number as usize - 1])
        }
    }
    
    /// Get all partition numbers that are in use
    pub fn get_used_partitions(&self) -> Vec<u32> {
        self.partitions
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.is_unused())
            .map(|(i, _)| (i + 1) as u32)
            .collect()
    }
    
    /// Get the total disk size in bytes
    pub fn get_size(&self) -> u64 {
        let last_sector = self.primary_header.alternate_lba + 1;
        last_sector * self.block_size
    }
    
    /// Get the final sector (highest end sector of any partition)
    pub fn get_final_sector(&self) -> u64 {
        self.partitions
            .iter()
            .filter(|p| !p.is_unused())
            .map(|p| p.last_lba)
            .max()
            .unwrap_or(0)
    }
    
    /// Get the backup GPT table sector
    pub fn get_backup_table_sector(&self) -> u64 {
        self.primary_header.alternate_lba - 32 // Usually 32 sectors before alternate header
    }
    
    /// Get partitions sorted by physical order (start LBA)
    pub fn get_partitions_physical_order(&self) -> Vec<(u32, &GptPartitionEntry)> {
        let mut parts: Vec<(u32, &GptPartitionEntry)> = self
            .partitions
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.is_unused())
            .map(|(i, p)| ((i + 1) as u32, p))
            .collect();
        parts.sort_by_key(|(_, p)| p.first_lba);
        parts
    }
    
    /// Check if this is a valid GPT image
    pub fn is_valid(&self) -> bool {
        &self.primary_header.signature == GPT_SIGNATURE
    }
    
    /// Get the offset of a partition in bytes
    pub fn get_partition_offset(&self, number: u32) -> Option<u64> {
        self.get_partition(number).map(|p| p.first_lba * self.block_size)
    }
    
    /// Get the size of a partition in bytes
    pub fn get_partition_size(&self, number: u32) -> Option<u64> {
        self.get_partition(number).map(|p| p.size_in_bytes(self.block_size))
    }
    
    /// Check if a partition is a ChromeOS kernel partition
    pub fn is_kernel_partition(&self, number: u32) -> bool {
        self.get_partition(number)
            .map(|p| p.type_guid.to_string().to_uppercase() == partition_types::CHROMEOS_KERNEL)
            .unwrap_or(false)
    }
    
    /// Check if a partition is a ChromeOS rootfs partition  
    pub fn is_rootfs_partition(&self, number: u32) -> bool {
        self.get_partition(number)
            .map(|p| {
                let type_str = p.type_guid.to_string().to_uppercase();
                type_str == partition_types::CHROMEOS_ROOTFS
                    || type_str == partition_types::LINUX_DATA
                    || type_str == partition_types::MS_BASIC_DATA
            })
            .unwrap_or(false)
    }
    
    /// Write the GPT back to file
    pub fn write_to_file(&self) -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .open(&self.path)
            .context(format!("Failed to open {} for writing", self.path))?;
        
        // Write partition entries
        let partition_entry_offset = self.primary_header.partition_entry_lba * self.block_size;
        file.seek(SeekFrom::Start(partition_entry_offset))?;
        
        for partition in &self.partitions {
            partition.write_to(&mut file)?;
            // Pad to entry size
            let remaining = self.primary_header.partition_entry_size as i64 - 128;
            if remaining > 0 {
                let padding = vec![0u8; remaining as usize];
                file.write_all(&padding)?;
            }
        }
        
        // Calculate new CRC for partition entries
        let entries_crc = self.calculate_partition_entries_crc()?;
        
        // Update and write primary header
        let mut header = self.primary_header.clone();
        header.partition_entries_crc32 = entries_crc;
        header.header_crc32 = 0;
        header.header_crc32 = self.calculate_header_crc(&header)?;
        header.write_to(&mut file, self.block_size)?;
        
        // Write backup header if present
        if let Some(ref backup) = self.backup_header {
            let mut backup = backup.clone();
            backup.partition_entries_crc32 = entries_crc;
            backup.header_crc32 = 0;
            backup.header_crc32 = self.calculate_header_crc(&backup)?;
            backup.write_to(&mut file, backup.my_lba * self.block_size)?;
        }
        
        file.flush()?;
        Ok(())
    }
    
    /// Calculate CRC32 of partition entries
    fn calculate_partition_entries_crc(&self) -> Result<u32> {
        let mut data = Vec::new();
        for partition in &self.partitions {
            partition.write_to(&mut data)?;
            let remaining = self.primary_header.partition_entry_size as usize - 128;
            if remaining > 0 {
                data.extend(vec![0u8; remaining]);
            }
        }
        Ok(crc32fast::hash(&data))
    }
    
    /// Calculate CRC32 of GPT header
    fn calculate_header_crc(&self, header: &GptHeader) -> Result<u32> {
        let mut data = Vec::new();
        let mut temp_header = header.clone();
        temp_header.header_crc32 = 0;
        temp_header.write_to(&mut std::io::Cursor::new(&mut data), 0)?;
        data.truncate(header.header_size as usize);
        Ok(crc32fast::hash(&data))
    }
    
    /// Add or update a partition
    pub fn add_partition(
        &mut self,
        number: u32,
        start_lba: u64,
        size_sectors: u64,
        type_guid: &str,
        name: &str,
    ) -> Result<()> {
        if number == 0 || number as usize > self.partitions.len() {
            bail!("Invalid partition number: {}", number);
        }
        
        let partition = &mut self.partitions[number as usize - 1];
        partition.type_guid = Uuid::parse_str(type_guid)?;
        partition.unique_guid = Uuid::new_v4();
        partition.first_lba = start_lba;
        partition.last_lba = start_lba + size_sectors - 1;
        partition.attributes = 0;
        partition.name = name.to_string();
        
        Ok(())
    }
    
    /// Delete a partition (mark as unused)
    pub fn delete_partition(&mut self, number: u32) -> Result<()> {
        if let Some(partition) = self.get_partition_mut(number) {
            *partition = GptPartitionEntry::empty();
            Ok(())
        } else {
            bail!("Invalid partition number: {}", number);
        }
    }
}

/// Check if a file is a valid GPT image
pub fn check_gpt_image(path: &Path) -> Result<bool> {
    let gpt = Gpt::load_from_file(path)?;
    Ok(gpt.is_valid())
}

/// Get sector size of a device or image
pub fn get_sector_size(path: &Path) -> Result<u64> {
    let gpt = Gpt::load_from_file(path)?;
    Ok(gpt.block_size)
}

/// Get total sectors of a device or image  
pub fn get_sectors(path: &Path) -> Result<u64> {
    let gpt = Gpt::load_from_file(path)?;
    Ok(gpt.get_size() / gpt.block_size)
}

/// Get the final sector (highest partition end)
pub fn get_final_sector(path: &Path) -> Result<u64> {
    let gpt = Gpt::load_from_file(path)?;
    Ok(gpt.get_final_sector())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partition_entry_empty() {
        let entry = GptPartitionEntry::empty();
        assert!(entry.is_unused());
        assert_eq!(entry.size_in_sectors(), 0);
    }
}
