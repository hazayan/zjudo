//! CLI argument parsing
//!
//! This module contains the argument structures for the zjudo CLI.

use clap::{Parser, Subcommand};

/// Main CLI structure
#[derive(Parser, Debug)]
#[command(
    name = "zjudo",
    version = "0.1.0",
    about = "Boot FreeBSD from Linux using kexec",
    long_about = "A Rust implementation of beastie-boot for booting FreeBSD from Linux using kexec"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Enable debug output
    #[arg(short, long, global = true)]
    pub debug: bool,

    /// Configuration file
    #[arg(short, long, global = true)]
    pub config: Option<String>,
}

/// Available commands
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Boot FreeBSD kernel
    Boot(BootArgs),

    /// Load kernel and modules without booting
    Load(LoadArgs),

    /// Unload currently loaded kernel
    Unload(UnloadArgs),

    /// List loaded kernels and modules
    List(ListArgs),

    /// Show system information
    Info(InfoArgs),

    /// Test kexec functionality
    Test(TestArgs),

    /// Generate configuration file
    Config(ConfigArgs),
}

/// Boot command arguments
#[derive(Parser, Debug)]
pub struct BootArgs {
    /// Kernel file (ELF executable)
    #[arg(short, long)]
    pub kernel: String,

    /// Kernel command line
    #[arg(short = 'C', long)]
    pub cmdline: Option<String>,

    /// Module files (ELF objects)
    #[arg(short, long)]
    pub modules: Vec<String>,

    /// Preloaded mfsroot image (raw file)
    #[arg(long)]
    pub mfsroot: Option<String>,

    /// Preloaded entropy cache (raw file)
    #[arg(long)]
    pub entropy: Option<String>,

    /// Initial ramdisk file
    #[arg(short, long)]
    pub initrd: Option<String>,

    /// Boot howto flags
    #[arg(short = 'H', long)]
    pub howto: Option<u32>,

    /// Use ZFS bootonce property when no cmdline is provided
    #[arg(long)]
    pub bootonce: bool,

    /// Skip framebuffer initialization
    #[arg(long)]
    pub no_fb: bool,

    /// Skip ACPI tables
    #[arg(long)]
    pub no_acpi: bool,

    /// Skip memory map collection
    #[arg(long)]
    pub no_memmap: bool,

    /// Patch kernel entry with a debug UART+HLT stub (debug only)
    #[arg(long)]
    pub entry_patch: bool,

    /// Force boot even if checks fail
    #[arg(short, long)]
    pub force: bool,
}

/// Load command arguments
#[derive(Parser, Debug)]
pub struct LoadArgs {
    /// Kernel file (ELF executable)
    #[arg(short, long)]
    pub kernel: String,

    /// Module files (ELF objects)
    #[arg(short, long)]
    pub modules: Vec<String>,

    /// Preloaded mfsroot image (raw file)
    #[arg(long)]
    pub mfsroot: Option<String>,

    /// Preloaded entropy cache (raw file)
    #[arg(long)]
    pub entropy: Option<String>,

    /// Initial ramdisk file
    #[arg(short, long)]
    pub initrd: Option<String>,

    /// Load but don't set as current
    #[arg(long)]
    pub inactive: bool,
}

/// Unload command arguments
#[derive(Parser, Debug)]
pub struct UnloadArgs {
    /// Unload all loaded kernels
    #[arg(short, long)]
    pub all: bool,

    /// Kernel ID to unload
    #[arg(short, long)]
    pub id: Option<u32>,
}

/// List command arguments
#[derive(Parser, Debug)]
pub struct ListArgs {
    /// Show detailed information
    #[arg(short, long)]
    pub detailed: bool,

    /// Show only kernels
    #[arg(long)]
    pub kernels: bool,

    /// Show only modules
    #[arg(long)]
    pub modules: bool,
}

/// Info command arguments
#[derive(Parser, Debug)]
pub struct InfoArgs {
    /// Show memory information
    #[arg(short, long)]
    pub memory: bool,

    /// Show framebuffer information
    #[arg(short, long)]
    pub framebuffer: bool,

    /// Show ACPI information
    #[arg(short, long)]
    pub acpi: bool,

    /// Show EFI information
    #[arg(short, long)]
    pub efi: bool,

    /// Show kexec information
    #[arg(short, long)]
    pub kexec: bool,
}

/// Test command arguments
#[derive(Parser, Debug)]
pub struct TestArgs {
    /// Test kexec functionality
    #[arg(short, long)]
    pub kexec: bool,

    /// Test ELF parsing
    #[arg(short, long)]
    pub elf: bool,

    /// Test module loading
    #[arg(short, long)]
    pub modules: bool,

    /// Test memory allocation
    #[arg(short, long)]
    pub memory: bool,
}

/// Config command arguments
#[derive(Parser, Debug)]
pub struct ConfigArgs {
    /// Generate default configuration
    #[arg(short, long)]
    pub default: bool,

    /// Validate existing configuration
    #[arg(short, long)]
    pub validate: bool,

    /// Configuration file to generate/validate
    #[arg(short, long)]
    pub file: Option<String>,
}

/// Parse command line arguments
pub fn parse_args() -> Cli {
    Cli::parse()
}
