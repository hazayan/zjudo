use crate::error::{BootError, Result};
use crate::types::{KERNBASE, KERNEL_PHYS_BASE, PAGE_SIZE};
use goblin::elf;

/// Load kernel and modules into memory
pub struct BootLoader {
    kernel_data: Vec<u8>,
    kernel_entry: u64,
    kernel_segments: Vec<KernelSegment>,
    modules: Vec<Module>,
    kernel_symbols: Option<Vec<u8>>,
    kernel_strtab: Option<Vec<u8>>,
    kernel_elfhdr: Option<Vec<u8>>,
    kernel_shdr: Option<Vec<u8>>,
    kernel_dynamic: Option<u64>,
    virt_delta: u64,
    kernel_btext: Option<u64>,
}

/// Kernel segment information
#[derive(Debug, Clone)]
pub struct KernelSegment {
    pub vaddr: u64,
    pub paddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub data: Vec<u8>,
    pub flags: u32,
}

/// Module information for loading
#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub data: Vec<u8>,
    pub phys_addr: u64,
    pub dyn_offset: u64,
    pub has_dynamic: bool,
    pub is_relocatable: bool,
    pub elf_header: Option<Vec<u8>>,
    pub section_headers: Option<Vec<u8>>,
    pub modmeta_set_offset: u64,
    pub modmeta_set_count: usize,
}

impl BootLoader {
    pub fn new() -> Self {
        Self {
            kernel_data: Vec::new(),
            kernel_entry: 0,
            kernel_segments: Vec::new(),
            modules: Vec::new(),
            kernel_symbols: None,
            kernel_strtab: None,
            kernel_elfhdr: None,
            kernel_shdr: None,
            kernel_dynamic: None,
            virt_delta: 0,
            kernel_btext: None,
        }
    }

    /// Load kernel from file
    pub fn load_kernel(&mut self, path: &std::path::Path) -> Result<()> {
        let data = std::fs::read(path).map_err(BootError::Io)?;
        self.load_kernel_from_data(data)
    }

    /// Load kernel from memory
    pub fn load_kernel_from_data(&mut self, data: Vec<u8>) -> Result<()> {
        self.kernel_data = data;
        self.parse_kernel()?;
        Ok(())
    }

    /// Parse kernel ELF
    fn parse_kernel(&mut self) -> Result<()> {
        let elf = elf::Elf::parse(&self.kernel_data).map_err(|e| BootError::ElfParse(e.to_string()))?;

        // Validate kernel
        if elf.header.e_type != elf::header::ET_EXEC {
            return Err(BootError::InvalidElf("Kernel must be ET_EXEC".to_string()));
        }

        if elf.header.e_machine != 62 {
            // EM_X86_64
            return Err(BootError::InvalidElf("Kernel must be x86_64".to_string()));
        }

        self.kernel_entry = elf.header.e_entry;
        self.kernel_elfhdr = None;
        self.kernel_shdr = None;
        self.kernel_dynamic = None;
        self.kernel_btext = find_symbol_value(&elf, "btext");

        let ehdr_size = elf.header.e_ehsize as usize;
        if ehdr_size != 0 && ehdr_size <= self.kernel_data.len() {
            self.kernel_elfhdr = Some(self.kernel_data[..ehdr_size].to_vec());
        }

        // Parse loadable segments
        let mut first_load_vaddr = None;
        let mut first_load_paddr = None;

        for ph in &elf.program_headers {
            if ph.p_type != elf::program_header::PT_LOAD {
                continue;
            }

            // Calculate physical address
            let paddr = if ph.p_vaddr >= KERNBASE {
                let vaddr = ph.p_vaddr;
                let base = KERNBASE + KERNEL_PHYS_BASE;
                if vaddr < base {
                    return Err(BootError::InvalidElf(
                        "Kernel PT_LOAD below KERNPHYS base".to_string(),
                    ));
                }
                vaddr - KERNBASE - KERNEL_PHYS_BASE
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
            if end > self.kernel_data.len() {
                return Err(BootError::InvalidElf(
                    "Segment data out of bounds".to_string(),
                ));
            }

            let data = self.kernel_data[start..end].to_vec();

            self.kernel_segments.push(KernelSegment {
                vaddr: ph.p_vaddr,
                paddr,
                filesz: ph.p_filesz,
                memsz: ph.p_memsz,
                data,
                flags: ph.p_flags,
            });
        }

        for ph in &elf.program_headers {
            if ph.p_type == elf::program_header::PT_DYNAMIC {
                self.kernel_dynamic = Some(ph.p_vaddr);
                break;
            }
        }

        // Calculate virtual delta
        self.virt_delta = if let (Some(vaddr), Some(paddr)) = (first_load_vaddr, first_load_paddr) {
            vaddr - (paddr + KERNEL_PHYS_BASE)
        } else {
            0
        };

        // Extract symbol tables information from ELF
        let mut symtab_start = None;
        let mut symtab_end = None;
        let mut strtab_start = None;
        let mut strtab_end = None;

        for sh in &elf.section_headers {
            if sh.sh_type == elf::section_header::SHT_SYMTAB {
                symtab_start = Some(sh.sh_offset as usize);
                symtab_end = Some(sh.sh_offset as usize + sh.sh_size as usize);
                
                // Get corresponding string table
                let strtab_idx = sh.sh_link as usize;
                if strtab_idx < elf.section_headers.len() {
                    let str_sh = &elf.section_headers[strtab_idx];
                    if str_sh.sh_type == elf::section_header::SHT_STRTAB {
                        strtab_start = Some(str_sh.sh_offset as usize);
                        strtab_end = Some(str_sh.sh_offset as usize + str_sh.sh_size as usize);
                    }
                }
                break;
            }
        }

        let shdr_size = (elf.header.e_shentsize as usize)
            .saturating_mul(elf.header.e_shnum as usize);
        let shdr_off = elf.header.e_shoff as usize;
        if shdr_size != 0 && shdr_off + shdr_size <= self.kernel_data.len() {
            self.kernel_shdr = Some(self.kernel_data[shdr_off..shdr_off + shdr_size].to_vec());
        }

        // Now update self with the symbol table information
        if let (Some(start), Some(end)) = (symtab_start, symtab_end) {
            if end <= self.kernel_data.len() {
                self.kernel_symbols = Some(self.kernel_data[start..end].to_vec());
            }
        }

        if let (Some(start), Some(end)) = (strtab_start, strtab_end) {
            if end <= self.kernel_data.len() {
                self.kernel_strtab = Some(self.kernel_data[start..end].to_vec());
            }
        }

        Ok(())
    }

    /// Add module
    pub fn add_module(&mut self, module: Module) {
        self.modules.push(module);
    }

    /// Get kernel entry point
    pub fn kernel_entry(&self) -> u64 {
        self.kernel_entry
    }

    pub fn kernel_btext(&self) -> Option<u64> {
        self.kernel_btext
    }

    /// Get kernel segments
    pub fn kernel_segments(&self) -> &[KernelSegment] {
        &self.kernel_segments
    }

    /// Get modules
    pub fn modules(&self) -> &[Module] {
        &self.modules
    }

    /// Get virtual delta
    pub fn virt_delta(&self) -> u64 {
        self.virt_delta
    }

    /// Get kernel symbols
    pub fn kernel_symbols(&self) -> Option<&[u8]> {
        self.kernel_symbols.as_deref()
    }

    /// Get kernel string table
    pub fn kernel_strtab(&self) -> Option<&[u8]> {
        self.kernel_strtab.as_deref()
    }

    pub fn kernel_elfhdr(&self) -> Option<&[u8]> {
        self.kernel_elfhdr.as_deref()
    }

    pub fn kernel_shdr(&self) -> Option<&[u8]> {
        self.kernel_shdr.as_deref()
    }

    pub fn kernel_dynamic(&self) -> Option<u64> {
        self.kernel_dynamic
    }

    /// Calculate total memory needed
    pub fn total_memory_needed(&self) -> usize {
        let mut total = 0;

        // Kernel segments
        for seg in &self.kernel_segments {
            total += round_up(seg.memsz as usize, PAGE_SIZE);
        }

        // Modules
        for module in &self.modules {
            total += round_up(module.data.len(), PAGE_SIZE);
        }

        total
    }

    /// Build memory map for kexec
    pub fn build_memory_map(&self, base_addr: u64) -> Vec<(u64, usize, u32)> {
        let mut map = Vec::new();
        let mut current_addr = base_addr;

        // Kernel segments
        for seg in &self.kernel_segments {
            let size = round_up(seg.memsz as usize, PAGE_SIZE);
            map.push((current_addr, size, seg.flags));
            current_addr += size as u64;
        }

        // Modules
        for module in &self.modules {
            let size = round_up(module.data.len(), PAGE_SIZE);
            map.push((current_addr, size, 0x7)); // RWX permissions
            current_addr += size as u64;
        }

        map
    }
}

fn find_symbol_value_in<I>(syms: I, strtab: &goblin::strtab::Strtab, name: &str) -> Option<u64>
where
    I: IntoIterator<Item = elf::sym::Sym>,
{
    for sym in syms {
        if let Some(sym_name) = strtab.get_at(sym.st_name) {
            if sym_name == name {
                return Some(sym.st_value);
            }
        }
    }
    None
}

fn find_symbol_value(elf: &elf::Elf, name: &str) -> Option<u64> {
    find_symbol_value_in(elf.syms.iter(), &elf.strtab, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use goblin::strtab::Strtab;

    #[test]
        fn find_symbol_value_returns_match() {
        let strtab_bytes = b"\0btext\0foo\0".to_vec();
        let strtab = Strtab::parse(&strtab_bytes, 0, strtab_bytes.len(), 0)
            .expect("parse strtab");
            let sym_btext = elf::sym::Sym {
                st_name: 1,
                st_info: 0,
            st_other: 0,
            st_shndx: 1,
            st_value: 0x1234_5678,
            st_size: 0,
        };
        let sym_foo = elf::sym::Sym {
            st_name: 7,
            st_info: 0,
            st_other: 0,
            st_shndx: 1,
            st_value: 0x10,
            st_size: 0,
        };
        let syms = vec![sym_foo, sym_btext];

        assert_eq!(
            find_symbol_value_in(syms.clone(), &strtab, "btext"),
            Some(0x1234_5678)
        );
        assert_eq!(
            find_symbol_value_in(syms, &strtab, "missing"),
            None
        );
    }
}

/// Round up to alignment
fn round_up(value: usize, alignment: usize) -> usize {
    if alignment == 0 {
        return value;
    }
    let remainder = value % alignment;
    if remainder == 0 {
        value
    } else {
        value + alignment - remainder
    }
}
