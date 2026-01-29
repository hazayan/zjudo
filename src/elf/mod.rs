mod parser;
mod types;

pub use parser::*;
pub use types::*;

use crate::error::{BootError, Result};
use goblin::elf::{Elf, ProgramHeader, SectionHeader};
use std::path::Path;

/// ELF file representation
pub struct ElfFile {
    data: Vec<u8>,
    elf: Elf<'static>,
}

impl ElfFile {
    /// Load and parse ELF file from path
    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).map_err(BootError::Io)?;
        Self::from_bytes(data)
    }

    /// Parse ELF from byte vector
    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        let elf = Elf::parse(&data).map_err(|e| BootError::ElfParse(e.to_string()))?;

        // Convert to 'static lifetime by leaking (we own the data)
        let elf = unsafe {
            std::mem::transmute::<Elf<'_>, Elf<'static>>(elf)
        };

        Ok(Self { data, elf })
    }

    /// Get the ELF header
    pub fn header(&self) -> &goblin::elf::Header {
        &self.elf.header
    }

    /// Get program headers
    pub fn program_headers(&self) -> &[ProgramHeader] {
        &self.elf.program_headers
    }

    /// Get section headers
    pub fn section_headers(&self) -> &[SectionHeader] {
        &self.elf.section_headers
    }

    /// Get the raw ELF data
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Check if this is a 64-bit ELF
    pub fn is_64bit(&self) -> bool {
        self.elf.header.e_ident[goblin::elf::header::EI_CLASS] == goblin::elf::header::ELFCLASS64
    }

    /// Check if this is a little-endian ELF
    pub fn is_little_endian(&self) -> bool {
        self.elf.header.e_ident[goblin::elf::header::EI_DATA] == goblin::elf::header::ELFDATA2LSB
    }

    /// Get ELF type (ET_EXEC, ET_REL, ET_DYN)
    pub fn elf_type(&self) -> u16 {
        self.elf.header.e_type
    }

    /// Get machine type (EM_X86_64, etc.)
    pub fn machine(&self) -> u16 {
        self.elf.header.e_machine
    }

    /// Get entry point address
    pub fn entry_point(&self) -> u64 {
        self.elf.header.e_entry
    }

    /// Validate ELF for FreeBSD kernel/module
    pub fn validate_freebsd(&self) -> Result<()> {
        // Check ELF class (must be 64-bit)
        if !self.is_64bit() {
            return Err(BootError::InvalidElf("ELF must be 64-bit".to_string()));
        }

        // Check endianness (must be little-endian)
        if !self.is_little_endian() {
            return Err(BootError::InvalidElf("ELF must be little-endian".to_string()));
        }

        // Check machine type (must be x86_64)
        if self.machine() != 62 { // EM_X86_64
            return Err(BootError::InvalidElf(format!(
                "Unsupported machine type: {} (expected EM_X86_64)",
                self.machine()
            )));
        }

        // Check OS ABI (should be FreeBSD = 9, but Linux kexec may accept others)
        let os_abi = self.elf.header.e_ident[goblin::elf::header::EI_OSABI];
        if os_abi != 9 && os_abi != 0 { // 0 = System V, 9 = FreeBSD
            log::warn!("Unusual OS ABI: {}, expected 0 (System V) or 9 (FreeBSD)", os_abi);
        }

        Ok(())
    }

    /// Get loadable segments (PT_LOAD)
    pub fn loadable_segments(&self) -> Vec<&ProgramHeader> {
        self.elf.program_headers
            .iter()
            .filter(|ph| ph.p_type == goblin::elf::program_header::PT_LOAD)
            .collect()
    }

    /// Get dynamic segment (PT_DYNAMIC)
    pub fn dynamic_segment(&self) -> Option<&ProgramHeader> {
        self.elf.program_headers
            .iter()
            .find(|ph| ph.p_type == goblin::elf::program_header::PT_DYNAMIC)
    }

    /// Get symbol table section
    pub fn symtab_section(&self) -> Option<&SectionHeader> {
        self.elf.section_headers
            .iter()
            .find(|sh| sh.sh_type == goblin::elf::section_header::SHT_SYMTAB)
    }

    /// Get string table for symbol table
    pub fn symstr_section(&self) -> Option<&SectionHeader> {
        let symtab = self.symtab_section()?;
        let strtab_idx = symtab.sh_link as usize;
        if strtab_idx < self.elf.section_headers.len() {
            Some(&self.elf.section_headers[strtab_idx])
        } else {
            None
        }
    }

    /// Get section by name
    pub fn section_by_name(&self, name: &str) -> Option<&SectionHeader> {
        let shstrtab = &self.elf.shdr_strtab;
        for (_i, section) in self.elf.section_headers.iter().enumerate() {
            if let Some(section_name) = shstrtab.get_at(section.sh_name) {
                if section_name == name {
                    return Some(section);
                }
            }
        }
        None
    }

    /// Extract segment data from file
    pub fn segment_data(&self, ph: &ProgramHeader) -> Result<Vec<u8>> {
        let start = ph.p_offset as usize;
        let end = start + ph.p_filesz as usize;
        
        if end > self.data.len() {
            return Err(BootError::InvalidElf(format!(
                "Segment data out of bounds: {}..{} (file size: {})",
                start, end, self.data.len()
            )));
        }

        Ok(self.data[start..end].to_vec())
    }

    /// Extract section data from file
    pub fn section_data(&self, sh: &SectionHeader) -> Result<Vec<u8>> {
        let start = sh.sh_offset as usize;
        let end = start + sh.sh_size as usize;
        
        if end > self.data.len() {
            return Err(BootError::InvalidElf(format!(
                "Section data out of bounds: {}..{} (file size: {})",
                start, end, self.data.len()
            )));
        }

        Ok(self.data[start..end].to_vec())
    }

    /// Calculate virtual delta for FreeBSD kernel
    pub fn virtual_delta(&self) -> u64 {
        use crate::types::{KERNBASE, KERNEL_PHYS_BASE};
        
        let mut first_load_vaddr = None;
        let mut first_load_paddr = None;

        for ph in self.loadable_segments() {
            // Calculate physical address
            // FreeBSD kernel expects physical address = vaddr - KERNBASE
            let paddr = if ph.p_vaddr >= KERNBASE {
                ph.p_vaddr - KERNBASE
            } else {
                ph.p_vaddr
            };

            if first_load_vaddr.is_none() {
                first_load_vaddr = Some(ph.p_vaddr);
                first_load_paddr = Some(paddr);
                break;
            }
        }

        if let (Some(vaddr), Some(paddr)) = (first_load_vaddr, first_load_paddr) {
            vaddr - (paddr + KERNEL_PHYS_BASE)
        } else {
            0
        }
    }

    /// Extract symbol tables from ELF
    pub fn symbol_tables(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        let symtab_sh = self.symtab_section()?;
        let strtab_sh = self.symstr_section()?;

        let symtab_data = self.section_data(symtab_sh).ok()?;
        let strtab_data = self.section_data(strtab_sh).ok()?;

        Some((symtab_data, strtab_data))
    }

    /// Lookup a symbol value by name (returns None if symtab/strtab missing).
    pub fn symbol_value(&self, name: &str) -> Option<u64> {
        for sym in self.elf.syms.iter() {
            if let Some(sym_name) = self.elf.strtab.get_at(sym.st_name) {
                if sym_name == name {
                    return Some(sym.st_value);
                }
            }
        }
        None
    }
}

/// ELF types for FreeBSD
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfType {
    Executable,  // ET_EXEC - Kernel
    Relocatable, // ET_REL - Kernel object file
    Dynamic,     // ET_DYN - Shared object (module)
}

impl TryFrom<u16> for ElfType {
    type Error = BootError;

    fn try_from(value: u16) -> Result<Self> {
        match value {
            2 => Ok(ElfType::Executable),
            1 => Ok(ElfType::Relocatable),
            3 => Ok(ElfType::Dynamic),
            _ => Err(BootError::InvalidElf(format!(
                "Unsupported ELF type: {} (expected ET_EXEC=2, ET_REL=1, or ET_DYN=3)",
                value
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;

    #[test]
    fn test_elf_file_creation() {
        let elf_data = tests::create_minimal_elf();
        let elf = ElfFile::from_bytes(elf_data).unwrap();
        
        assert!(elf.is_64bit());
        assert!(elf.is_little_endian());
        assert_eq!(elf.elf_type(), 2); // ET_EXEC
        assert_eq!(elf.machine(), 62); // EM_X86_64
        assert_eq!(elf.entry_point(), 0x12345678);
    }

    #[test]
    fn test_elf_validation() {
        let elf_data = tests::create_minimal_elf();
        let elf = ElfFile::from_bytes(elf_data).unwrap();
        
        // Should validate successfully
        assert!(elf.validate_freebsd().is_ok());
    }

    #[test]
    fn test_loadable_segments() {
        let elf_data = tests::create_minimal_elf();
        let elf = ElfFile::from_bytes(elf_data).unwrap();
        
        let loadable = elf.loadable_segments();
        assert_eq!(loadable.len(), 1);
        
        let segment = loadable[0];
        assert_eq!(segment.p_type, goblin::elf::program_header::PT_LOAD);
        // Virtual address is 0x400000 (0x40 at byte position 2 in little-endian)
        assert_eq!(segment.p_vaddr, 0x400000);
        assert_eq!(segment.p_filesz, 0x100);
    }

    #[test]
    fn test_section_by_name() {
        // This test requires an ELF with named sections
        // For now, just test that the method doesn't panic
        let elf_data = tests::create_minimal_elf();
        let elf = ElfFile::from_bytes(elf_data).unwrap();
        
        // Should return None since our test ELF has no sections
        let section = elf.section_by_name(".text");
        assert!(section.is_none());
    }

    #[test]
    fn test_elf_type_conversion() {
        assert_eq!(ElfType::try_from(2).unwrap(), ElfType::Executable);
        assert_eq!(ElfType::try_from(1).unwrap(), ElfType::Relocatable);
        assert_eq!(ElfType::try_from(3).unwrap(), ElfType::Dynamic);
        
        assert!(ElfType::try_from(0).is_err());
        assert!(ElfType::try_from(4).is_err());
    }

    #[test]
    fn test_symbol_value_missing() {
        let elf_data = tests::create_minimal_elf();
        let elf = ElfFile::from_bytes(elf_data).unwrap();

        assert_eq!(elf.symbol_value("btext"), None);
    }

    #[test]
    fn test_invalid_elf() {
        let elf_data = tests::create_invalid_elf();
        let result = ElfFile::from_bytes(elf_data);
        assert!(result.is_err());
    }
}
