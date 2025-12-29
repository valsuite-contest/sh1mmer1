//! GPT command - cgpt-style GPT manipulation

use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use std::path::PathBuf;

use crate::gpt::{self, Gpt};
use crate::utils::{self, log_info};

/// Arguments for the gpt command
#[derive(Args, Debug)]
pub struct GptArgs {
    #[command(subcommand)]
    pub command: GptCommands,
}

#[derive(Subcommand, Debug)]
pub enum GptCommands {
    /// Show partition table
    Show(ShowArgs),
    
    /// Add or modify a partition
    Add(AddArgs),
    
    /// Prioritize a kernel partition for booting
    Prioritize(PrioritizeArgs),
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// Image file
    pub image: PathBuf,
    
    /// Show partition number only
    #[arg(short = 'i', long)]
    pub partition: Option<u32>,
    
    /// Quick output (partition number, start, size, type)
    #[arg(short, long)]
    pub quick: bool,
    
    /// Numeric output only
    #[arg(short, long)]
    pub numeric: bool,
    
    /// Show size in sectors
    #[arg(short = 's', long)]
    pub size: bool,
    
    /// Show start sector  
    #[arg(short = 'b', long)]
    pub begin: bool,
}

#[derive(Args, Debug)]
pub struct AddArgs {
    /// Image file
    pub image: PathBuf,
    
    /// Partition number
    #[arg(short = 'i', long)]
    pub partition: u32,
    
    /// Start sector
    #[arg(short = 'b', long)]
    pub begin: Option<u64>,
    
    /// Size in sectors
    #[arg(short = 's', long)]
    pub size: Option<u64>,
    
    /// Partition type GUID
    #[arg(short = 't', long)]
    pub type_guid: Option<String>,
    
    /// Partition label
    #[arg(short = 'l', long)]
    pub label: Option<String>,
}

#[derive(Args, Debug)]
pub struct PrioritizeArgs {
    /// Image file
    pub image: PathBuf,
    
    /// Partition number to prioritize
    #[arg(short = 'i', long)]
    pub partition: u32,
}

/// Run the gpt command
pub fn run(args: GptArgs, debug: bool) -> Result<()> {
    match args.command {
        GptCommands::Show(show_args) => run_show(show_args, debug),
        GptCommands::Add(add_args) => run_add(add_args, debug),
        GptCommands::Prioritize(pri_args) => run_prioritize(pri_args, debug),
    }
}

fn run_show(args: ShowArgs, _debug: bool) -> Result<()> {
    let gpt = Gpt::load_from_file(&args.image)?;
    
    if let Some(part_num) = args.partition {
        let partition = gpt.get_partition(part_num)
            .ok_or_else(|| anyhow::anyhow!("Partition {} not found", part_num))?;
        
        if args.size {
            if args.numeric {
                println!("{}", partition.size_in_sectors());
            } else {
                println!("{}", partition.size_in_sectors());
            }
        } else if args.begin {
            println!("{}", partition.first_lba);
        } else {
            println!("Partition {}: {} - {}", part_num, partition.first_lba, partition.last_lba);
            println!("  Size: {} sectors ({} bytes)", 
                    partition.size_in_sectors(), 
                    partition.size_in_bytes(gpt.block_size));
            println!("  Type: {}", partition.type_guid);
            println!("  Name: {}", partition.name);
        }
    } else if args.quick {
        for (part_num, partition) in gpt.partitions.iter().enumerate() {
            if !partition.is_unused() {
                println!("{} {} {} {}", 
                        partition.first_lba, 
                        partition.size_in_sectors(), 
                        part_num + 1,
                        partition.name);
            }
        }
    } else {
        println!("Disk: {}", args.image.display());
        println!("Sector size: {} bytes", gpt.block_size);
        println!();
        println!("Partition table:");
        println!("{:>4} {:>12} {:>12} {:>12} {:^36} Name", 
                "Num", "Start", "Size", "End", "Type GUID");
        
        for (part_num, partition) in gpt.partitions.iter().enumerate() {
            if !partition.is_unused() {
                println!("{:>4} {:>12} {:>12} {:>12} {} {}", 
                        part_num + 1,
                        partition.first_lba, 
                        partition.size_in_sectors(),
                        partition.last_lba,
                        partition.type_guid,
                        partition.name);
            }
        }
    }
    
    Ok(())
}

fn run_add(args: AddArgs, _debug: bool) -> Result<()> {
    let mut gpt = Gpt::load_from_file(&args.image)?;
    
    let partition = gpt.get_partition_mut(args.partition)
        .ok_or_else(|| anyhow::anyhow!("Invalid partition number: {}", args.partition))?;
    
    if let Some(begin) = args.begin {
        partition.first_lba = begin;
    }
    
    if let Some(size) = args.size {
        if partition.first_lba > 0 {
            partition.last_lba = partition.first_lba + size - 1;
        }
    }
    
    if let Some(type_guid) = args.type_guid {
        partition.type_guid = uuid::Uuid::parse_str(&type_guid)?;
    }
    
    if let Some(label) = args.label {
        partition.name = label;
    }
    
    gpt.write_to_file()?;
    
    log_info(&format!("Updated partition {}", args.partition));
    Ok(())
}

fn run_prioritize(args: PrioritizeArgs, _debug: bool) -> Result<()> {
    // For ChromeOS kernel priority, use cgpt directly as it handles
    // the complex priority/tries/success attributes
    utils::shell(
        "cgpt",
        &["prioritize", "-i", &args.partition.to_string(), args.image.to_str().unwrap()],
        true,
        false,
    )?;
    
    log_info(&format!("Prioritized partition {}", args.partition));
    Ok(())
}
