 mod dependency;
mod loader;
mod metadata;

pub use dependency::*;
pub use loader::*;
pub use metadata::*;

use crate::error::{BootError, Result};
use crate::types::{ModuleType, PAGE_SIZE};
use std::path::Path;

/// Module information
#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub meta_name: String,
    pub module_type: ModuleType,
    pub data: Vec<u8>,
    pub phys_addr: u64,
    pub dyn_offset: u64,
    pub has_dynamic: bool,
    pub is_relocatable: bool,
    pub elf_header: Option<Vec<u8>>,
    pub section_headers: Option<Vec<u8>>,
    pub modmeta_set_offset: u64,
    pub modmeta_set_count: usize,
    pub modmeta_absolute: bool,
    pub modmeta_data_relocated: bool,
}

impl Module {
    pub fn new(name: String, module_type: ModuleType, data: Vec<u8>) -> Self {
        let meta_name = Path::new(&name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&name)
            .to_string();

        Self {
            name,
            meta_name,
            module_type,
            data,
            phys_addr: 0,
            dyn_offset: 0,
            has_dynamic: false,
            is_relocatable: false,
            elf_header: None,
            section_headers: None,
            modmeta_set_offset: 0,
            modmeta_set_count: 0,
            modmeta_absolute: false,
            modmeta_data_relocated: false,
        }
    }

    pub fn new_raw(name: String, meta_name: String, type_name: String, data: Vec<u8>) -> Self {
        let mut module = Self::new(name, ModuleType::Raw(type_name), data);
        module.meta_name = meta_name;
        module
    }

    pub fn size(&self) -> usize {
        self.data.len()
    }

    pub fn aligned_size(&self) -> usize {
        round_up(self.data.len(), PAGE_SIZE)
    }

    pub fn set_physical_address(&mut self, addr: u64) {
        self.phys_addr = addr;
    }

    pub fn set_dynamic_offset(&mut self, offset: u64) {
        self.dyn_offset = offset;
        self.has_dynamic = true;
    }

    pub fn set_relocatable(&mut self, elf_header: Vec<u8>, section_headers: Vec<u8>) {
        self.is_relocatable = true;
        self.elf_header = Some(elf_header);
        self.section_headers = Some(section_headers);
    }

    pub fn set_modmetadata_set(&mut self, offset: u64, count: usize) {
        self.modmeta_set_offset = offset;
        self.modmeta_set_count = count;
    }
}

/// Module loader
pub struct ModuleLoader {
    kernel_data: Option<Vec<u8>>,
    kernel_entry: u64,
    kernel_btext: Option<u64>,
    kernel_text: Option<u64>,
    modules: Vec<Module>,
    kernel_symbols: Option<Vec<u8>>,
    kernel_strtab: Option<Vec<u8>>,
    kernel_elfhdr: Option<Vec<u8>>,
    kernel_shdr: Option<Vec<u8>>,
    kernel_dynamic: Option<u64>,
    virt_delta: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelEntrySource {
    Btext,
    Text,
    Entry,
}

pub fn choose_kernel_entry(
    entry: u64,
    btext: Option<u64>,
    text: Option<u64>,
) -> (u64, KernelEntrySource) {
    if let Some(btext) = btext {
        return (btext, KernelEntrySource::Btext);
    }
    if let Some(text) = text {
        return (text, KernelEntrySource::Text);
    }
    (entry, KernelEntrySource::Entry)
}

impl ModuleLoader {
    pub fn new() -> Self {
        Self {
            kernel_data: None,
            kernel_entry: 0,
            kernel_btext: None,
            kernel_text: None,
            modules: Vec::new(),
            kernel_symbols: None,
            kernel_strtab: None,
            kernel_elfhdr: None,
            kernel_shdr: None,
            kernel_dynamic: None,
            virt_delta: 0,
        }
    }

    /// Load kernel from file
    pub fn load_kernel(&mut self, path: &Path) -> Result<()> {
        let data = std::fs::read(path).map_err(BootError::Io)?;
        self.load_kernel_from_data(data)
    }

    /// Load kernel from memory
    pub fn load_kernel_from_data(&mut self, data: Vec<u8>) -> Result<()> {
        self.kernel_data = Some(data);
        self.parse_kernel()?;
        Ok(())
    }

    /// Parse kernel ELF
    fn parse_kernel(&mut self) -> Result<()> {
        let data = self.kernel_data.as_ref().ok_or_else(|| {
            BootError::InvalidElf("No kernel data loaded".to_string())
        })?;
        
        let elf = crate::elf::ElfFile::from_bytes(data.clone())?;
        elf.validate_freebsd()?;

        if elf.elf_type() != 2 { // ET_EXEC
            return Err(BootError::InvalidElf("Kernel must be ET_EXEC".to_string()));
        }

        self.kernel_entry = elf.entry_point();
        self.kernel_btext = elf.symbol_value("btext");
        self.kernel_text = elf
            .section_by_name(".text")
            .map(|section| section.sh_addr);
        self.virt_delta = elf.virtual_delta();
        self.kernel_elfhdr = None;
        self.kernel_shdr = None;
        self.kernel_dynamic = None;

        let ehdr_size = elf.header().e_ehsize as usize;
        if ehdr_size != 0 && ehdr_size <= data.len() {
            self.kernel_elfhdr = Some(data[..ehdr_size].to_vec());
        }

        // Extract symbol tables
        if let Some((symtab, strtab)) = elf.symbol_tables() {
            self.kernel_symbols = Some(symtab);
            self.kernel_strtab = Some(strtab);
        }

        let shdr_size = (elf.header().e_shentsize as usize)
            .saturating_mul(elf.header().e_shnum as usize);
        let shdr_off = elf.header().e_shoff as usize;
        if shdr_size != 0 && shdr_off + shdr_size <= data.len() {
            self.kernel_shdr = Some(data[shdr_off..shdr_off + shdr_size].to_vec());
        }

        if let Some(dynamic) = elf.dynamic_segment() {
            self.kernel_dynamic = Some(dynamic.p_vaddr);
        }

        Ok(())
    }

    /// Get kernel entry point
    pub fn kernel_entry(&self) -> u64 {
        self.kernel_entry
    }

    pub fn kernel_btext(&self) -> Option<u64> {
        self.kernel_btext
    }

    pub fn kernel_text(&self) -> Option<u64> {
        self.kernel_text
    }

    /// Get virtual delta
    pub fn virt_delta(&self) -> u64 {
        self.virt_delta
    }

    pub fn set_kernel_symbols(&mut self, symtab: Vec<u8>, strtab: Vec<u8>) {
        self.kernel_symbols = Some(symtab);
        self.kernel_strtab = Some(strtab);
    }

    pub fn set_virt_delta(&mut self, delta: u64) {
        self.virt_delta = delta;
    }

    pub fn load_module(&mut self, path: &Path) -> Result<()> {
        let module = load_module_file(path)?;
        self.modules.push(module);
        Ok(())
    }

    pub fn load_raw_module(
        &mut self,
        path: &Path,
        type_name: &str,
        meta_name: Option<&str>,
    ) -> Result<()> {
        let data = std::fs::read(path).map_err(BootError::Io)?;
        let name = path.to_string_lossy().to_string();
        let meta_name = meta_name.unwrap_or(type_name).to_string();
        let module = Module::new_raw(name, meta_name, type_name.to_string(), data);
        self.modules.push(module);
        Ok(())
    }

    pub fn load_module_from_data(&mut self, name: String, data: Vec<u8>, module_type: ModuleType) -> Result<()> {
        let module = Module::new(name, module_type, data);
        self.modules.push(module);
        Ok(())
    }

    pub fn modules(&self) -> &[Module] {
        &self.modules
    }

    pub fn modules_mut(&mut self) -> &mut [Module] {
        &mut self.modules
    }

    pub fn kernel_symbols(&self) -> Option<&[u8]> {
        self.kernel_symbols.as_deref()
    }

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

    pub fn allocate_addresses(&mut self, kernel_end: u64) -> u64 {
        let mut current_addr = kernel_end;

        for module in &mut self.modules {
            module.phys_addr = current_addr;
            current_addr += module.aligned_size() as u64;
        }

        current_addr
    }

    pub fn patch_modmetadata(&mut self, enabled: bool) -> Result<()> {
        if !enabled {
            return Ok(());
        }

        for module in &mut self.modules {
            if module.modmeta_set_count > 0 {
                patch_module_metadata(module, self.virt_delta, &self.kernel_symbols, &self.kernel_strtab)?;
                module.modmeta_data_relocated = true;
            }
        }

        Ok(())
    }
}

/// Load module from file
fn load_module_file(path: &Path) -> Result<Module> {
    let data = std::fs::read(path).map_err(BootError::Io)?;
    
    // Parse ELF to determine module type
    let elf = crate::elf::ElfFile::from_bytes(data.clone())?;
    elf.validate_freebsd()?;

    let module_type = match elf.elf_type() {
        1 => ModuleType::ElfObj,      // ET_REL
        3 => ModuleType::ElfModule,   // ET_DYN
        _ => return Err(BootError::InvalidElf(
            "Module must be ET_REL or ET_DYN".to_string(),
        )),
    };

    let mut module = Module::new(
        path.to_string_lossy().to_string(),
        module_type.clone(),
        data,
    );

    // Additional processing based on module type
    match module_type {
        ModuleType::ElfModule => {
            // ET_DYN modules have dynamic section
            if let Some(dyn_seg) = elf.dynamic_segment() {
                module.set_dynamic_offset(dyn_seg.p_vaddr);
            }
        }
        ModuleType::ElfObj => {
            // ET_REL modules need section headers
            // We'll store the raw ELF data which contains the headers
            let elf_data = elf.data().to_vec();
            module.set_relocatable(elf_data, Vec::new()); // Simplified for now

            // Find modmetadata set
            if let Some(sh) = elf.section_by_name("set_modmetadata_set") {
                module.set_modmetadata_set(sh.sh_addr, (sh.sh_size / 8) as usize);
            }
        }
        _ => {}
    }

    Ok(module)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_choose_kernel_entry_prefers_btext() {
        let (entry, source) = choose_kernel_entry(0x1000, Some(0x2000), Some(0x3000));
        assert_eq!(entry, 0x2000);
        assert_eq!(source, KernelEntrySource::Btext);
    }

    #[test]
    fn test_choose_kernel_entry_falls_back_to_text() {
        let (entry, source) = choose_kernel_entry(0x1000, None, Some(0x3000));
        assert_eq!(entry, 0x3000);
        assert_eq!(source, KernelEntrySource::Text);
    }

    #[test]
    fn test_choose_kernel_entry_falls_back_to_entry() {
        let (entry, source) = choose_kernel_entry(0x1000, None, None);
        assert_eq!(entry, 0x1000);
        assert_eq!(source, KernelEntrySource::Entry);
    }
}
