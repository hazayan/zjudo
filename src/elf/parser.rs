use crate::error::{BootError, Result};
use crate::types::{KERNBASE, KERNEL_PHYS_BASE, PAGE_SIZE};
use goblin::elf;
use std::collections::HashMap;

/// Kernel loading information
#[derive(Debug, Clone)]
pub struct KernelLoadInfo {
    pub entry_point: u64,
    pub load_segments: Vec<LoadSegment>,
    pub virt_delta: u64,
    pub symtab: Option<Vec<u8>>,
    pub strtab: Option<Vec<u8>>,
}

/// Loadable segment information
#[derive(Debug, Clone)]
pub struct LoadSegment {
    pub vaddr: u64,
    pub paddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub data: Vec<u8>,
    pub flags: u32,
}

/// Parse kernel ELF (ET_EXEC)
pub fn parse_kernel_elf(elf_data: &[u8]) -> Result<KernelLoadInfo> {
    let elf = elf::Elf::parse(elf_data).map_err(|e| BootError::ElfParse(e.to_string()))?;

    // Validate kernel ELF
    if elf.header.e_type != elf::header::ET_EXEC {
        return Err(BootError::InvalidElf(
            "Kernel must be ET_EXEC type".to_string(),
        ));
    }

    if elf.header.e_machine != 62 {
        // EM_X86_64
        return Err(BootError::InvalidElf(
            "Kernel must be x86_64".to_string(),
        ));
    }

    let entry_point = elf.header.e_entry;

    // Collect loadable segments
    let mut load_segments = Vec::new();
    let mut first_load_vaddr = None;
    let mut first_load_paddr = None;

    for ph in &elf.program_headers {
        if ph.p_type != elf::program_header::PT_LOAD {
            continue;
        }

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
        }

        // Extract segment data
        let start = ph.p_offset as usize;
        let end = start + ph.p_filesz as usize;
        if end > elf_data.len() {
            return Err(BootError::InvalidElf(format!(
                "Segment data out of bounds: {}..{}",
                start, end
            )));
        }

        let data = elf_data[start..end].to_vec();

        load_segments.push(LoadSegment {
            vaddr: ph.p_vaddr,
            paddr,
            filesz: ph.p_filesz,
            memsz: ph.p_memsz,
            data,
            flags: ph.p_flags,
        });
    }

    // Calculate virtual delta
    let virt_delta = if let (Some(vaddr), Some(paddr)) = (first_load_vaddr, first_load_paddr) {
        vaddr - (paddr + KERNEL_PHYS_BASE)
    } else {
        0
    };

    // Extract symbol table and string table
    let (symtab, strtab) = extract_symbol_tables(&elf, elf_data)?;

    Ok(KernelLoadInfo {
        entry_point,
        load_segments,
        virt_delta,
        symtab,
        strtab,
    })
}

/// Extract symbol tables from ELF
fn extract_symbol_tables(elf: &elf::Elf, elf_data: &[u8]) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
    let mut symtab = None;
    let mut strtab = None;

    for (_i, sh) in elf.section_headers.iter().enumerate() {
        if sh.sh_type == elf::section_header::SHT_SYMTAB {
            let start = sh.sh_offset as usize;
            let end = start + sh.sh_size as usize;
            if end > elf_data.len() {
                return Err(BootError::InvalidElf(
                    "Symbol table out of bounds".to_string(),
                ));
            }
            symtab = Some(elf_data[start..end].to_vec());

            // Get corresponding string table
            let strtab_idx = sh.sh_link as usize;
            if strtab_idx < elf.section_headers.len() {
                let str_sh = &elf.section_headers[strtab_idx];
                if str_sh.sh_type == elf::section_header::SHT_STRTAB {
                    let str_start = str_sh.sh_offset as usize;
                    let str_end = str_start + str_sh.sh_size as usize;
                    if str_end <= elf_data.len() {
                        strtab = Some(elf_data[str_start..str_end].to_vec());
                    }
                }
            }
            break;
        }
    }

    Ok((symtab, strtab))
}

/// Parse module ELF (ET_DYN or ET_REL)
pub fn parse_module_elf(elf_data: &[u8], module_type: crate::elf::ElfType) -> Result<ModuleLoadInfo> {
    let elf = elf::Elf::parse(elf_data).map_err(|e| BootError::ElfParse(e.to_string()))?;

    match module_type {
        crate::elf::ElfType::Dynamic => parse_dynamic_module(&elf, elf_data),
        crate::elf::ElfType::Relocatable => parse_relocatable_module(&elf, elf_data),
        _ => Err(BootError::InvalidElf(
            "Module must be ET_DYN or ET_REL type".to_string(),
        )),
    }
}

/// Dynamic module (ET_DYN) information
#[derive(Debug, Clone)]
pub struct DynamicModuleInfo {
    pub name: String,
    pub data: Vec<u8>,
    pub dyn_offset: u64,
    pub min_vaddr: u64,
    pub max_vaddr: u64,
}

/// Relocatable module (ET_REL) information
#[derive(Debug, Clone)]
pub struct RelocatableModuleInfo {
    pub name: String,
    pub data: Vec<u8>,
    pub elf_header: elf::Header,
    pub modmeta_set_offset: u64,
    pub modmeta_set_count: usize,
}

/// Module load information (union of both types)
#[derive(Debug, Clone)]
pub enum ModuleLoadInfo {
    Dynamic(DynamicModuleInfo),
    Relocatable(RelocatableModuleInfo),
}

/// Parse dynamic module (ET_DYN)
fn parse_dynamic_module(elf: &elf::Elf, elf_data: &[u8]) -> Result<ModuleLoadInfo> {
    // Check for required segments
    let mut has_load = false;
    let mut has_dynamic = false;
    let mut min_vaddr = u64::MAX;
    let mut max_vaddr = 0;
    let mut dyn_vaddr = 0;

    for ph in &elf.program_headers {
        if ph.p_type == elf::program_header::PT_LOAD {
            has_load = true;
            if ph.p_vaddr < min_vaddr {
                min_vaddr = ph.p_vaddr;
            }
            if ph.p_vaddr + ph.p_memsz > max_vaddr {
                max_vaddr = ph.p_vaddr + ph.p_memsz;
            }
        } else if ph.p_type == elf::program_header::PT_DYNAMIC {
            has_dynamic = true;
            dyn_vaddr = ph.p_vaddr;
        }
    }

    if !has_load || !has_dynamic {
        return Err(BootError::InvalidElf(
            "Dynamic module missing PT_LOAD or PT_DYNAMIC".to_string(),
        ));
    }

    // Create image in memory
    let image_size = round_up(max_vaddr - min_vaddr, PAGE_SIZE);
    let mut image = vec![0u8; image_size as usize];

    for ph in &elf.program_headers {
        if ph.p_type != elf::program_header::PT_LOAD || ph.p_filesz == 0 {
            continue;
        }

        let start = ph.p_offset as usize;
        let end = start + ph.p_filesz as usize;
        if end > elf_data.len() {
            return Err(BootError::InvalidElf(
                "Segment data out of bounds".to_string(),
            ));
        }

        let offset = (ph.p_vaddr - min_vaddr) as usize;
        image[offset..offset + ph.p_filesz as usize].copy_from_slice(&elf_data[start..end]);
    }

    let dyn_offset = (dyn_vaddr - min_vaddr) as u64;

    Ok(ModuleLoadInfo::Dynamic(DynamicModuleInfo {
        name: String::new(), // Will be set by caller
        data: image,
        dyn_offset,
        min_vaddr,
        max_vaddr,
    }))
}

/// Parse relocatable module (ET_REL)
fn parse_relocatable_module(elf: &elf::Elf, elf_data: &[u8]) -> Result<ModuleLoadInfo> {
    if elf.section_headers.is_empty() || elf.header.e_shoff == 0 {
        return Err(BootError::InvalidElf(
            "Relocatable module missing section headers".to_string(),
        ));
    }

    let mut sections = Vec::new();
    let mut current_addr = 0u64;

    // First pass: allocate space for allocatable sections
    for (i, sh) in elf.section_headers.iter().enumerate() {
        if sh.sh_size == 0 {
            continue;
        }

        match sh.sh_type {
            elf::section_header::SHT_PROGBITS
            | elf::section_header::SHT_NOBITS
            | elf::section_header::SHT_X86_64_UNWIND
            | elf::section_header::SHT_INIT_ARRAY
            | elf::section_header::SHT_FINI_ARRAY => {
                if sh.sh_flags & elf::section_header::SHF_ALLOC as u64 == 0 {
                    continue;
                }

                // Align current address
                let align = if sh.sh_addralign > 0 {
                    sh.sh_addralign
                } else {
                    1
                };
                current_addr = round_up(current_addr, align as usize) as u64;

                sections.push(SectionInfo {
                    index: i,
                    addr: current_addr,
                });
                current_addr += sh.sh_size;
            }
            _ => {}
        }
    }

    // Find symbol table and string table
    let mut symtab_idx = None;
    let mut strtab_idx = None;
    for (i, sh) in elf.section_headers.iter().enumerate() {
        if sh.sh_type == elf::section_header::SHT_SYMTAB {
            symtab_idx = Some(i);
            strtab_idx = Some(sh.sh_link as usize);
            break;
        }
    }

    // Allocate space for symbol table and string table
    if let (Some(sym_idx), Some(str_idx)) = (symtab_idx, strtab_idx) {
        if str_idx < elf.section_headers.len() {
            let sym_sh = &elf.section_headers[sym_idx];
            let str_sh = &elf.section_headers[str_idx];

            // Symbol table
            let sym_align = if sym_sh.sh_addralign > 0 {
                sym_sh.sh_addralign
            } else {
                1
            };
            current_addr = round_up(current_addr, sym_align as usize) as u64;
            sections.push(SectionInfo {
                index: sym_idx,
                addr: current_addr,
            });
            current_addr += sym_sh.sh_size;

            // String table
            let str_align = if str_sh.sh_addralign > 0 {
                str_sh.sh_addralign
            } else {
                1
            };
            current_addr = round_up(current_addr, str_align as usize) as u64;
            sections.push(SectionInfo {
                index: str_idx,
                addr: current_addr,
            });
            current_addr += str_sh.sh_size;
        }
    }

    // Allocate space for section name string table
    let shstr_idx = elf.header.e_shstrndx as usize;
    if shstr_idx < elf.section_headers.len() {
        let shstr_sh = &elf.section_headers[shstr_idx];
        if shstr_sh.sh_type == elf::section_header::SHT_STRTAB {
            let align = if shstr_sh.sh_addralign > 0 {
                shstr_sh.sh_addralign
            } else {
                1
            };
            current_addr = round_up(current_addr, align as usize) as u64;
            sections.push(SectionInfo {
                index: shstr_idx,
                addr: current_addr,
            });
            current_addr += shstr_sh.sh_size;
        }
    }

    // Allocate space for relocation sections
    for (i, sh) in elf.section_headers.iter().enumerate() {
        if sh.sh_type != elf::section_header::SHT_REL && sh.sh_type != elf::section_header::SHT_RELA {
            continue;
        }

        let target_idx = sh.sh_info as usize;
        if target_idx >= elf.section_headers.len() {
            continue;
        }

        let target_sh = &elf.section_headers[target_idx];
        if target_sh.sh_flags & elf::section_header::SHF_ALLOC as u64 == 0 {
            continue;
        }

        let align = if sh.sh_addralign > 0 {
            sh.sh_addralign
        } else {
            1
        };
        current_addr = round_up(current_addr, align as usize) as u64;
        sections.push(SectionInfo {
            index: i,
            addr: current_addr,
        });
        current_addr += sh.sh_size;
    }

    // Create image with all allocated sections
    let image_size = current_addr as usize;
    let mut image = vec![0u8; image_size];

    // Copy section data
    for section in &sections {
        let sh = &elf.section_headers[section.index];
        if sh.sh_type == elf::section_header::SHT_NOBITS || sh.sh_size == 0 {
            continue;
        }

        let start = sh.sh_offset as usize;
        let end = start + sh.sh_size as usize;
        if end > elf_data.len() {
            return Err(BootError::InvalidElf(
                "Section data out of bounds".to_string(),
            ));
        }

        let dst_start = section.addr as usize;
        let dst_end = dst_start + sh.sh_size as usize;
        if dst_end > image.len() {
            return Err(BootError::InvalidElf(
                "Section would overflow image".to_string(),
            ));
        }

        image[dst_start..dst_end].copy_from_slice(&elf_data[start..end]);
    }

    // Find modmetadata set section
    let mut modmeta_set_offset = 0;
    let mut modmeta_set_count = 0;

    let shstrtab = &elf.shdr_strtab;
    for (i, sh) in elf.section_headers.iter().enumerate() {
        if let Some(name) = shstrtab.get_at(sh.sh_name) {
            if name == "set_modmetadata_set" {
                if let Some(section) = sections.iter().find(|s| s.index == i) {
                    modmeta_set_offset = section.addr;
                    modmeta_set_count = (sh.sh_size / 8) as usize; // Array of uint64_t
                }
                break;
            }
        }
    }

    Ok(ModuleLoadInfo::Relocatable(RelocatableModuleInfo {
        name: String::new(), // Will be set by caller
        data: image,
        elf_header: elf.header.clone(),
        modmeta_set_offset,
        modmeta_set_count,
    }))
}

/// Helper struct for section information
#[derive(Debug, Clone)]
struct SectionInfo {
    index: usize,
    addr: u64,
}

/// Round up to alignment
fn round_up(value: u64, alignment: usize) -> u64 {
    if alignment == 0 {
        return value;
    }
    let remainder = value % alignment as u64;
    if remainder == 0 {
        value
    } else {
        value + alignment as u64 - remainder
    }
}

/// Build symbol map from ELF
pub fn build_symbol_map(_elf: &elf::Elf, _elf_data: &[u8]) -> Result<HashMap<String, u64>> {
    let symbols = HashMap::new();

    // This is a simplified implementation
    // In a real implementation, we would parse the symbol table properly
    // For now, we'll just return an empty map since this is not critical for booting
    Ok(symbols)
}
