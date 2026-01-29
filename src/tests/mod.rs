//! Test utilities and fixtures for zjudo

mod integration;

use crate::elf::ElfFile;
use crate::error::Result;
use std::path::Path;

/// Test data directory
pub const TEST_DATA_DIR: &str = "test_data";

/// Create a test directory for test data
pub fn setup_test_dir() -> Result<()> {
    let test_dir = Path::new(TEST_DATA_DIR);
    if !test_dir.exists() {
        std::fs::create_dir_all(test_dir)?;
    }
    Ok(())
}

/// Clean up test directory
pub fn cleanup_test_dir() -> Result<()> {
    let test_dir = Path::new(TEST_DATA_DIR);
    if test_dir.exists() {
        std::fs::remove_dir_all(test_dir)?;
    }
    Ok(())
}

/// Create a minimal valid ELF file for testing
pub fn create_minimal_elf() -> Vec<u8> {
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
    elf.extend_from_slice(&[0x78, 0x56, 0x34, 0x12, 0, 0, 0, 0]); // Entry point
    elf.extend_from_slice(&[64, 0, 0, 0, 0, 0, 0, 0]); // Program header offset
    elf.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // Section header offset
    elf.extend_from_slice(&[0, 0, 0, 0]); // Flags
    elf.extend_from_slice(&[64, 0]); // Header size
    elf.extend_from_slice(&[56, 0]); // Program header size
    elf.extend_from_slice(&[1, 0]); // Program header count
    elf.extend_from_slice(&[0, 0]); // Section header size
    elf.extend_from_slice(&[0, 0]); // Section header count
    elf.extend_from_slice(&[0, 0]); // Section name string table index
    
    // Program header (PT_LOAD)
    elf.extend_from_slice(&[1, 0, 0, 0]); // PT_LOAD
    elf.extend_from_slice(&[7, 0, 0, 0]); // Flags: RWE
    elf.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // Offset
    elf.extend_from_slice(&[0, 0, 0x40, 0, 0, 0, 0, 0]); // Virtual address
    elf.extend_from_slice(&[0, 0, 0x40, 0, 0, 0, 0, 0]); // Physical address
    // File size = 0x100
    elf.extend_from_slice(&0x100u64.to_le_bytes());
    // Memory size = 0x100
    elf.extend_from_slice(&0x100u64.to_le_bytes());
    // Alignment = 0x1000
    elf.extend_from_slice(&0x1000u64.to_le_bytes());
    
    // Fill with some data
    elf.resize(0x100 + 64 + 56, 0x90); // NOP sled
    
    elf
}

/// Create an invalid ELF file for testing
pub fn create_invalid_elf() -> Vec<u8> {
    vec![0; 64] // Just zeros, not a valid ELF
}

/// Test ELF parsing
pub fn test_elf_parsing() -> Result<()> {
    let elf_data = create_minimal_elf();
    let elf = ElfFile::from_bytes(elf_data)?;
    
    assert!(elf.is_64bit());
    assert!(elf.is_little_endian());
    assert_eq!(elf.elf_type(), 2); // ET_EXEC
    assert_eq!(elf.machine(), 62); // EM_X86_64
    assert_eq!(elf.entry_point(), 0x12345678);
    
    let loadable_segments = elf.loadable_segments();
    assert_eq!(loadable_segments.len(), 1);
    
    Ok(())
}

/// Test ELF validation
pub fn test_elf_validation() -> Result<()> {
    let elf_data = create_minimal_elf();
    let elf = ElfFile::from_bytes(elf_data)?;
    
    // Should validate successfully for FreeBSD
    elf.validate_freebsd()?;
    
    Ok(())
}

/// Test invalid ELF handling
pub fn test_invalid_elf() -> Result<()> {
    let elf_data = create_invalid_elf();
    let result = ElfFile::from_bytes(elf_data);
    assert!(result.is_err());
    Ok(())
}

/// Run all tests
pub fn run_all_tests() -> Result<()> {
    println!("Running zjudo tests...");
    
    setup_test_dir()?;
    
    println!("  Testing ELF parsing...");
    test_elf_parsing()?;
    println!("    ✓ ELF parsing tests passed");
    
    println!("  Testing ELF validation...");
    test_elf_validation()?;
    println!("    ✓ ELF validation tests passed");
    
    println!("  Testing invalid ELF handling...");
    test_invalid_elf()?;
    println!("    ✓ Invalid ELF tests passed");
    
    cleanup_test_dir()?;
    
    println!("\nAll tests passed!");
    Ok(())
}
