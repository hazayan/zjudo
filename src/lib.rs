//! zjudo - A Rust implementation of beastie-boot for booting FreeBSD from Linux using kexec
//!
//! This library provides functionality to:
//! - Parse FreeBSD kernel and module ELF files
//! - Load kernels and modules into memory
//! - Generate trampoline code for booting
//! - Interface with Linux kexec system calls
//! - Collect system information (memory maps, framebuffer, ACPI, etc.)

pub mod boot;
pub mod cli;
pub mod elf;
pub mod error;
pub mod font;
pub mod module;
pub mod system;
#[cfg(test)]
pub mod tests;
pub mod types;
pub mod zfs;

// Re-export commonly used types
pub use error::{BootError, Result};
pub use types::*;

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Library name
pub const NAME: &str = "zjudo";

/// Library description
pub const DESCRIPTION: &str = "Boot FreeBSD from Linux using kexec";

/// Initialize the library
pub fn init() -> Result<()> {
    // Check if running as root (required for kexec operations)
    if unsafe { libc::geteuid() } != 0 {
        return Err(BootError::Permission(
            "This operation requires root privileges".to_string(),
        ));
    }

    // Check kexec support
    if !system::kexec_supported() {
        return Err(BootError::Kexec(
            "kexec not supported on this system".to_string(),
        ));
    }

    Ok(())
}

/// Clean up library resources
pub fn cleanup() -> Result<()> {
    // Unload any loaded kexec segments
    system::kexec_unload()?;
    Ok(())
}

/// Test library functionality
pub fn test() -> Result<()> {
    println!("Testing {} v{}", NAME, VERSION);
    println!("{}", DESCRIPTION);

    // Test kexec support
    println!("\n1. Testing kexec support:");
    if system::kexec_supported() {
        println!("   ✓ kexec is supported");
    } else {
        println!("   ✗ kexec is not supported");
        return Err(BootError::Kexec("kexec not supported".to_string()));
    }

    // Test ELF parsing
    println!("\n2. Testing ELF parsing:");
    let test_elf = create_test_elf();
    if let Ok(elf) = elf::ElfFile::from_bytes(test_elf) {
        println!("   ✓ ELF parsing works");
        println!("     - 64-bit: {}", elf.is_64bit());
        println!("     - Entry point: 0x{:x}", elf.entry_point());
    } else {
        println!("   ✗ ELF parsing failed");
        return Err(BootError::ElfParse("ELF parsing test failed".to_string()));
    }

    // Test system information collection
    println!("\n3. Testing system information:");
    if let Ok(sysinfo) = system::SystemInfo::collect() {
        println!("   ✓ System information collection works");
        println!("     - EFI boot: {}", sysinfo.is_efi);
        println!("     - Framebuffer: {}", sysinfo.fb_info.is_some());
    } else {
        println!("   ✗ System information collection failed");
    }

    println!("\nAll tests passed!");
    Ok(())
}

/// Create a minimal test ELF
fn create_test_elf() -> Vec<u8> {
    // Minimal ELF header for testing
    let mut elf = Vec::new();
    
    // ELF identification
    elf.extend_from_slice(&[0x7f, b'E', b'L', b'F']); // Magic
    elf.push(2); // 64-bit
    elf.push(1); // Little endian
    elf.push(1); // ELF version
    elf.push(0); // OS ABI (System V)
    elf.extend_from_slice(&[0; 8]); // Padding
    
    // ELF header
    elf.extend_from_slice(&[2, 0]); // ET_EXEC
    elf.extend_from_slice(&[62, 0]); // EM_X86_64
    elf.extend_from_slice(&[1, 0, 0, 0]); // Version
    elf.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // Entry point
    elf.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // Program header offset
    elf.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // Section header offset
    elf.extend_from_slice(&[0, 0, 0, 0]); // Flags
    elf.extend_from_slice(&[64, 0]); // Header size
    elf.extend_from_slice(&[0, 0]); // Program header size
    elf.extend_from_slice(&[0, 0]); // Program header count
    elf.extend_from_slice(&[0, 0]); // Section header size
    elf.extend_from_slice(&[0, 0]); // Section header count
    elf.extend_from_slice(&[0, 0]); // Section name string table index
    
    elf
}
