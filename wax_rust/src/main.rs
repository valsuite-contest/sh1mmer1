//! M1NSH1M - Shim modifying automation tool
//! 
//! A Rust rewrite of the SH1MMER wax toolchain for ChromeOS shim modification.
//! Originally developed by the Mercury Workshop team.

mod commands;
mod core;
mod gpt;
mod utils;
mod ext2;

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;

const SCRIPT_DATE: &str = "2025-12-29";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// M1NSH1M - Shim modifying automation tool
#[derive(Parser)]
#[command(name = "m1nsh1m")]
#[command(version = VERSION)]
#[command(about = "M1NSH1M - Shim modifying automation tool (Rust rewrite)", long_about = None)]
struct Cli {
    /// Enable debug output
    #[arg(short, long, global = true)]
    debug: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Modify a factory shim image with M1NSH1M payloads (full featured)
    Wax(commands::wax::WaxArgs),
    
    /// Build a minimal shim with just enough to boot a custom payload
    Minimal(commands::minimal::MinimalArgs),
    
    /// Strip payloads from a shim image
    Strip(commands::stripper::StripperArgs),
    
    /// Convert a block device to a GPT image file
    Gpt2Image(commands::gpt2image::Gpt2ImageArgs),
    
    /// Truncate a GPT image to minimal size
    GptTruncate(commands::gpttruncate::GptTruncateArgs),
    
    /// Archive kernel partitions from a shim image
    KernArchive(commands::kernarchive::KernArchiveArgs),
    
    /// Check kernel patch status in a shim image
    KernPatchCheck(commands::kernpatchcheck::KernPatchCheckArgs),
    
    /// Check kernel signatures in a shim image
    KernSigCheck(commands::kernsigcheck::KernSigCheckArgs),
    
    /// Archive kernel signatures from firmware
    KernSigArchive(commands::kernsigarchive::KernSigArchiveArgs),
    
    /// Generate short kernel signature list
    KernSigShortGen(commands::kernsigshortgen::KernSigShortGenArgs),
    
    /// Download ChromeOS update payloads
    UpdateDownloader(commands::update_downloader::UpdateDownloaderArgs),
    
    /// Check miscellaneous info about a shim image
    MiscCheck(commands::misccheck::MiscCheckArgs),
    
    /// Merge multiple RMA shim images
    RmaMerge(commands::rma_merge::RmaMergeArgs),
    
    /// Bundle wax4web for distribution
    Wax4WebBundler(commands::wax4web_bundler::Wax4WebBundlerArgs),
    
    /// GPT partition table operations
    Gpt(commands::gpt_cmd::GptArgs),
}

fn print_banner() {
    println!("{}", "┌─────────────────────────────────────────────────────────────────┐".cyan());
    println!("{}", "│ Welcome to M1NSH1M, a shim modifying automation tool            │".cyan());
    println!("{}", "│ Credits: CoolElectronics, Sharp_Jack, r58playz, Rafflesia, OlyB │".cyan());
    println!("│ {} {}                                         │", "Script date:".cyan(), SCRIPT_DATE.cyan());
    println!("{}", "│ Rewritten in Rust for better performance and reliability        │".cyan());
    println!("{}", "└─────────────────────────────────────────────────────────────────┘".cyan());
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    
    // Initialize logging
    if cli.debug {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();
    } else {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    }
    
    print_banner();
    
    match cli.command {
        Commands::Wax(args) => commands::wax::run(args, cli.debug),
        Commands::Minimal(args) => commands::minimal::run(args, cli.debug),
        Commands::Strip(args) => commands::stripper::run(args, cli.debug),
        Commands::Gpt2Image(args) => commands::gpt2image::run(args, cli.debug),
        Commands::GptTruncate(args) => commands::gpttruncate::run(args, cli.debug),
        Commands::KernArchive(args) => commands::kernarchive::run(args, cli.debug),
        Commands::KernPatchCheck(args) => commands::kernpatchcheck::run(args, cli.debug),
        Commands::KernSigCheck(args) => commands::kernsigcheck::run(args, cli.debug),
        Commands::KernSigArchive(args) => commands::kernsigarchive::run(args, cli.debug),
        Commands::KernSigShortGen(args) => commands::kernsigshortgen::run(args, cli.debug),
        Commands::UpdateDownloader(args) => commands::update_downloader::run(args, cli.debug),
        Commands::MiscCheck(args) => commands::misccheck::run(args, cli.debug),
        Commands::RmaMerge(args) => commands::rma_merge::run(args, cli.debug),
        Commands::Wax4WebBundler(args) => commands::wax4web_bundler::run(args, cli.debug),
        Commands::Gpt(args) => commands::gpt_cmd::run(args, cli.debug),
    }
}
