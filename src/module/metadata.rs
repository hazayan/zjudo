use crate::error::{BootError, Result};
use crate::types::{
    ModInfoMd, ModMetadataType, MODINFO_ADDR, MODINFO_ARGS, MODINFO_END, MODINFO_METADATA,
    MODINFO_NAME, MODINFO_SIZE, MODINFO_TYPE,
};
use goblin::elf;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ModulepInfo {
    pub data: Vec<u8>,
    pub phys_addr: u64,
    pub kernend: u64,
    pub efi_map_offset: Option<u64>,
}

struct ModulepWriter {
    buf: Vec<u8>,
}

impl ModulepWriter {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    fn add_str(&mut self, type_: u32, value: &str) {
        let size = value.len() + 1;
        self.push_u32(type_);
        self.push_u32(size as u32);
        self.buf.extend_from_slice(value.as_bytes());
        self.buf.push(0);
        self.align_ptr();
    }

    fn add_u64(&mut self, type_: u32, value: u64) {
        self.push_u32(type_);
        self.push_u32(8);
        self.buf.extend_from_slice(&value.to_le_bytes());
        self.align_ptr();
    }

    fn add_u32(&mut self, type_: u32, value: u32) {
        self.push_u32(type_);
        self.push_u32(4);
        self.buf.extend_from_slice(&value.to_le_bytes());
        self.align_ptr();
    }

    fn add_bytes(&mut self, type_: u32, bytes: &[u8]) {
        self.push_u32(type_);
        self.push_u32(bytes.len() as u32);
        self.buf.extend_from_slice(bytes);
        self.align_ptr();
    }

    fn add_end(&mut self) {
        self.push_u32(MODINFO_END);
        self.push_u32(0);
    }

    fn push_u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    fn align_ptr(&mut self) {
        const ALIGN: usize = std::mem::size_of::<u64>();
        let pad = (ALIGN - (self.buf.len() % ALIGN)) % ALIGN;
        self.buf.extend_from_slice(&vec![0u8; pad]);
    }
}

fn build_efi_map_metadata(info: &crate::types::EfiMapInfo) -> Result<Vec<u8>> {
    use crate::system::align_up;
    use crate::types::EfiMapHeader;

    if info.memory_size == 0 {
        return Ok(Vec::new());
    }

    let header = EfiMapHeader {
        memory_size: info.memory_size,
        descriptor_size: info.descriptor_size,
        descriptor_version: info.descriptor_version,
        pad: 0,
    };
    let header_size = std::mem::size_of::<EfiMapHeader>() as u64;
    let header_aligned = align_up(header_size, 16);
    let map_size = usize::try_from(info.memory_size)
        .map_err(|_| BootError::System("EFI map size exceeds addressable range".to_string()))?;
    let total_size = usize::try_from(header_aligned)
        .map_err(|_| BootError::System("EFI map header size exceeds addressable range".to_string()))?
        .saturating_add(map_size);

    let mut bytes = Vec::with_capacity(total_size);
    bytes.extend_from_slice(&header.memory_size.to_le_bytes());
    bytes.extend_from_slice(&header.descriptor_size.to_le_bytes());
    bytes.extend_from_slice(&header.descriptor_version.to_le_bytes());
    bytes.extend_from_slice(&header.pad.to_le_bytes());
    bytes.resize(header_aligned as usize, 0);

    if info.descriptor_size == 0 {
        bytes.resize(total_size, 0);
        return Ok(bytes);
    }

    let entry_size = info.descriptor_size as usize;
    let entries_total = map_size / entry_size;
    for i in 0..entries_total {
        let mut entry = vec![0u8; entry_size];
        if i < info.efi_table.len() {
            let src = info.efi_table[i];
            entry[0..4].copy_from_slice(&src.type_.to_le_bytes());
            entry[4..8].copy_from_slice(&src.pad.to_le_bytes());
            entry[8..16].copy_from_slice(&src.phys.to_le_bytes());
            entry[16..24].copy_from_slice(&src.virt.to_le_bytes());
            entry[24..32].copy_from_slice(&src.pages.to_le_bytes());
            entry[32..40].copy_from_slice(&src.attr.to_le_bytes());
        }
        bytes.extend_from_slice(&entry);
    }
    bytes.resize(total_size, 0);

    Ok(bytes)
}

fn build_smap_metadata(info: &crate::types::SmapInfo) -> Vec<u8> {
    let entries = info.e820_entries as usize;
    if entries == 0 {
        return Vec::new();
    }
    let entry_size = std::mem::size_of::<crate::types::SmapEntry>();
    let mut bytes = Vec::with_capacity(entries.saturating_mul(entry_size));
    for entry in info.e820_table.iter().take(entries) {
        bytes.extend_from_slice(&entry.addr.to_le_bytes());
        bytes.extend_from_slice(&entry.size.to_le_bytes());
        bytes.extend_from_slice(&entry.type_.to_le_bytes());
    }
    bytes
}

fn build_efi_fb_metadata(info: &crate::types::EfiFbInfo) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of::<crate::types::EfiFbInfo>());
    bytes.extend_from_slice(&info.addr.to_le_bytes());
    bytes.extend_from_slice(&info.size.to_le_bytes());
    bytes.extend_from_slice(&info.height.to_le_bytes());
    bytes.extend_from_slice(&info.width.to_le_bytes());
    bytes.extend_from_slice(&info.stride.to_le_bytes());
    bytes.extend_from_slice(&info.mask_red.to_le_bytes());
    bytes.extend_from_slice(&info.mask_green.to_le_bytes());
    bytes.extend_from_slice(&info.mask_blue.to_le_bytes());
    bytes.extend_from_slice(&info.mask_reserved.to_le_bytes());
    bytes
}

pub fn build_modulep(
    kernel_name: &str,
    kernel_phys: u64,
    kernel_size: u64,
    howto: u32,
    efi_systab: u64,
    efi_map_info: Option<&crate::types::EfiMapInfo>,
    smap_info: Option<&crate::types::SmapInfo>,
    kernel_args: Option<&str>,
    modulep_offset: u64,
    kernel_elfhdr: Option<&[u8]>,
    kernel_shdr: Option<&[u8]>,
    kernel_dynamic: Option<u64>,
    symtab: Option<(u64, u64)>,
    envp: Option<u64>,
    font: Option<u64>,
    efi_fb_info: Option<&crate::types::EfiFbInfo>,
    modules: &[crate::module::Module],
    kernend_override: u64,
) -> Result<Vec<u8>> {
    let mut writer = ModulepWriter::new();

    writer.add_str(MODINFO_NAME, kernel_name);
    writer.add_str(MODINFO_TYPE, "elf kernel");
    if let Some(args) = kernel_args {
        if !args.is_empty() {
            writer.add_str(MODINFO_ARGS, args);
        }
    }
    writer.add_u64(MODINFO_ADDR, kernel_phys);
    writer.add_u64(MODINFO_SIZE, kernel_size);

    if let Some(efi_map) = efi_map_info {
        let bytes = build_efi_map_metadata(efi_map)?;
        if !bytes.is_empty() {
            writer.add_bytes(MODINFO_METADATA | ModInfoMd::EfiMap as u32, &bytes);
        }
    }
    if efi_systab != 0 {
        writer.add_u64(MODINFO_METADATA | ModInfoMd::FwHandle as u32, efi_systab);
    }
    {
        let keybuf = vec![0u8; crate::types::GELI_KEYBUF_SIZE];
        writer.add_bytes(MODINFO_METADATA | ModInfoMd::KeyBuf as u32, &keybuf);
    }
    if modulep_offset != 0 {
        writer.add_u64(MODINFO_METADATA | ModInfoMd::Modulep as u32, modulep_offset);
    }
    if kernend_override != 0 {
        writer.add_u64(MODINFO_METADATA | ModInfoMd::Kernend as u32, kernend_override);
    }
    if let Some(envp) = envp {
        writer.add_u64(MODINFO_METADATA | ModInfoMd::Envp as u32, envp);
    }
    if howto != 0 {
        writer.add_u32(MODINFO_METADATA | ModInfoMd::Howto as u32, howto);
    }
    if let Some(smap) = smap_info {
        let bytes = build_smap_metadata(smap);
        if !bytes.is_empty() {
            writer.add_bytes(MODINFO_METADATA | ModInfoMd::Smap as u32, &bytes);
        }
    }
    if let Some(elfhdr) = kernel_elfhdr {
        if !elfhdr.is_empty() {
            writer.add_bytes(MODINFO_METADATA | ModInfoMd::Elfhdr as u32, elfhdr);
        }
    }
    if let Some(dynamic) = kernel_dynamic {
        if dynamic != 0 {
            writer.add_u64(MODINFO_METADATA | ModInfoMd::Dynamic as u32, dynamic);
        }
    }
    if let Some((ssym, esym)) = symtab {
        writer.add_u64(MODINFO_METADATA | ModInfoMd::Esym as u32, esym);
        writer.add_u64(MODINFO_METADATA | ModInfoMd::Ssym as u32, ssym);
    }
    if let Some(shdr) = kernel_shdr {
        if !shdr.is_empty() {
            writer.add_bytes(MODINFO_METADATA | ModInfoMd::Shdr as u32, shdr);
        }
    }
    if let Some(font) = font {
        writer.add_u64(MODINFO_METADATA | ModInfoMd::Font as u32, font);
    }
    if let Some(efi_fb) = efi_fb_info {
        let bytes = build_efi_fb_metadata(efi_fb);
        writer.add_bytes(MODINFO_METADATA | ModInfoMd::EfiFb as u32, &bytes);
    }

    for module in modules {
        let name = if module.meta_name.is_empty() {
            module.name.as_str()
        } else {
            module.meta_name.as_str()
        };

        writer.add_str(MODINFO_NAME, name);
        writer.add_str(MODINFO_TYPE, &module.module_type.to_string());
        writer.add_str(MODINFO_ARGS, "");
        writer.add_u64(MODINFO_ADDR, module.phys_addr);
        writer.add_u64(MODINFO_SIZE, module.data.len() as u64);

        if module.has_dynamic {
            writer.add_u64(
                MODINFO_METADATA | ModInfoMd::Dynamic as u32,
                module.dyn_offset,
            );
        }

        if module.is_relocatable {
            if let Ok(elf) = elf::Elf::parse(&module.data) {
                let hdr_size = elf.header.e_ehsize as usize;
                if hdr_size != 0 && hdr_size <= module.data.len() {
                    writer.add_bytes(
                        MODINFO_METADATA | ModInfoMd::Elfhdr as u32,
                        &module.data[..hdr_size],
                    );
                }
                let shdr_size = (elf.header.e_shentsize as usize)
                    .saturating_mul(elf.header.e_shnum as usize);
                let shdr_off = elf.header.e_shoff as usize;
                if shdr_size != 0 && shdr_off + shdr_size <= module.data.len() {
                    writer.add_bytes(
                        MODINFO_METADATA | ModInfoMd::Shdr as u32,
                        &module.data[shdr_off..shdr_off + shdr_size],
                    );
                }
            }
        }
    }

    writer.add_end();
    Ok(writer.buf)
}

/// Patch module metadata
pub fn patch_module_metadata(
    module: &mut crate::module::Module,
    virt_delta: u64,
    kernel_symbols: &Option<Vec<u8>>,
    kernel_strtab: &Option<Vec<u8>>,
) -> Result<()> {
    if module.modmeta_set_count == 0 {
        return Ok(());
    }

    // Parse kernel symbols if available
    let kernel_sym_map = if let (Some(symtab), Some(strtab)) = (kernel_symbols, kernel_strtab) {
        parse_symbol_table(symtab, strtab)?
    } else {
        HashMap::new()
    };

    // Patch modmetadata set
    patch_modmetadata_set(module, virt_delta, &kernel_sym_map)?;

    Ok(())
}

/// Parse symbol table
fn parse_symbol_table(symtab: &[u8], strtab: &[u8]) -> Result<HashMap<String, u64>> {
    let mut symbols = HashMap::new();

    // Parse ELF symbols
    // This is a simplified implementation
    // In practice, we would use goblin to parse the symbol table properly
    let sym_size = 24; // Size of Elf64_Sym
    let sym_count = symtab.len() / sym_size;

    for i in 0..sym_count {
        let offset = i * sym_size;
        if offset + sym_size > symtab.len() {
            break;
        }

        // Parse symbol entry (simplified)
        let name_offset = u32::from_le_bytes([
            symtab[offset],
            symtab[offset + 1],
            symtab[offset + 2],
            symtab[offset + 3],
        ]);

        let value = u64::from_le_bytes([
            symtab[offset + 8],
            symtab[offset + 9],
            symtab[offset + 10],
            symtab[offset + 11],
            symtab[offset + 12],
            symtab[offset + 13],
            symtab[offset + 14],
            symtab[offset + 15],
        ]);

        if name_offset == 0 || value == 0 {
            continue;
        }

        // Get symbol name
        if let Ok(name) = get_string_from_strtab(strtab, name_offset as usize) {
            symbols.insert(name, value);
        }
    }

    Ok(symbols)
}

/// Get string from string table
fn get_string_from_strtab(strtab: &[u8], offset: usize) -> Result<String> {
    if offset >= strtab.len() {
        return Err(BootError::InvalidElf("String offset out of bounds".to_string()));
    }

    let mut end = offset;
    while end < strtab.len() && strtab[end] != 0 {
        end += 1;
    }

    if end == offset {
        return Ok(String::new());
    }

    std::str::from_utf8(&strtab[offset..end])
        .map(|s| s.to_string())
        .map_err(|e| BootError::ElfParse(e.to_string()))
}

/// Patch modmetadata set
fn patch_modmetadata_set(
    module: &mut crate::module::Module,
    virt_delta: u64,
    kernel_sym_map: &HashMap<String, u64>,
) -> Result<()> {
    let set_offset = module.modmeta_set_offset as usize;
    let set_size = module.modmeta_set_count * 8; // Array of uint64_t

    if set_offset + set_size > module.data.len() {
        return Err(BootError::InvalidElf(
            "Modmetadata set out of bounds".to_string(),
        ));
    }

    // Parse modmetadata set
    let mut modmeta_ptrs = Vec::new();
    for i in 0..module.modmeta_set_count {
        let offset = set_offset + i * 8;
        let ptr = u64::from_le_bytes([
            module.data[offset],
            module.data[offset + 1],
            module.data[offset + 2],
            module.data[offset + 3],
            module.data[offset + 4],
            module.data[offset + 5],
            module.data[offset + 6],
            module.data[offset + 7],
        ]);
        modmeta_ptrs.push(ptr);
    }

    // Patch each modmetadata entry
    for &ptr in &modmeta_ptrs {
        if ptr == 0 {
            continue;
        }

        let offset = ptr as usize;
        if offset + 16 > module.data.len() {
            continue;
        }

        // Parse modmetadata header
        let type_ = u32::from_le_bytes([
            module.data[offset],
            module.data[offset + 1],
            module.data[offset + 2],
            module.data[offset + 3],
        ]);

        let _subtype = u32::from_le_bytes([
            module.data[offset + 4],
            module.data[offset + 5],
            module.data[offset + 6],
            module.data[offset + 7],
        ]);

        let data = u64::from_le_bytes([
            module.data[offset + 8],
            module.data[offset + 9],
            module.data[offset + 10],
            module.data[offset + 11],
            module.data[offset + 12],
            module.data[offset + 13],
            module.data[offset + 14],
            module.data[offset + 15],
        ]);

        // Patch based on type
        match ModMetadataType::try_from(type_) {
            Ok(ModMetadataType::Depend) => {
                patch_depend_metadata(module, offset, data, virt_delta, kernel_sym_map)?;
            }
            Ok(ModMetadataType::Module) => {
                patch_module_metadata_entry(module, offset, data, virt_delta)?;
            }
            Ok(ModMetadataType::Version) => {
                patch_version_metadata(module, offset, data, virt_delta)?;
            }
            _ => {
                // Other metadata types don't need patching
            }
        }
    }

    Ok(())
}

/// Patch depend metadata
fn patch_depend_metadata(
    module: &mut crate::module::Module,
    offset: usize,
    data: u64,
    virt_delta: u64,
    kernel_sym_map: &HashMap<String, u64>,
) -> Result<()> {
    // Depend metadata contains symbol names that need to be resolved
    if data == 0 {
        return Ok(());
    }

    let data_offset = data as usize;
    if data_offset >= module.data.len() {
        return Err(BootError::InvalidElf(
            "Depend metadata data out of bounds".to_string(),
        ));
    }

    // Find null-terminated string
    let mut end = data_offset;
    while end < module.data.len() && module.data[end] != 0 {
        end += 1;
    }

    if end == data_offset {
        return Ok(());
    }

    let symbol_name = std::str::from_utf8(&module.data[data_offset..end])
        .map_err(|e| BootError::ElfParse(e.to_string()))?;

    // Look up symbol in kernel
    if let Some(&sym_value) = kernel_sym_map.get(symbol_name) {
        // Patch the pointer to point to kernel symbol
        let patched_value = sym_value + virt_delta;
        module.data[offset + 8..offset + 16].copy_from_slice(&patched_value.to_le_bytes());
    }

    Ok(())
}

/// Patch module metadata
fn patch_module_metadata_entry(
    module: &mut crate::module::Module,
    offset: usize,
    data: u64,
    virt_delta: u64,
) -> Result<()> {
    // Module metadata contains module name
    if data == 0 {
        return Ok(());
    }

    let data_offset = data as usize;
    if data_offset >= module.data.len() {
        return Err(BootError::InvalidElf(
            "Module metadata data out of bounds".to_string(),
        ));
    }

    // The module name is already in the module data, no patching needed
    // But we need to adjust the pointer if it's relative
    if !module.modmeta_absolute {
        let patched_data = data + virt_delta;
        module.data[offset + 8..offset + 16].copy_from_slice(&patched_data.to_le_bytes());
    }

    Ok(())
}

/// Patch version metadata
fn patch_version_metadata(
    module: &mut crate::module::Module,
    offset: usize,
    data: u64,
    virt_delta: u64,
) -> Result<()> {
    // Version metadata contains version string
    if data == 0 {
        return Ok(());
    }

    // Adjust pointer if relative
    if !module.modmeta_absolute {
        let patched_data = data + virt_delta;
        module.data[offset + 8..offset + 16].copy_from_slice(&patched_data.to_le_bytes());
    }

    Ok(())
}

/// Create modmetadata for kernel
pub fn create_kernel_modmetadata(
    kernel_data: &[u8],
    _virt_delta: u64,
    _fb_info: Option<&crate::types::FbInfo>,
    smap_info: Option<&crate::types::SmapInfo>,
    efi_map_info: Option<&crate::types::EfiMapInfo>,
    efi_fb_info: Option<&crate::types::EfiFbInfo>,
    font_info: Option<&crate::types::FontInfo>,
    rsdp: u64,
    _rsdt: u64,
    howto: u32,
) -> Result<Vec<u8>> {
    let mut metadata = Vec::new();

    // Add howto metadata
    if howto != 0 {
        add_modmetadata(&mut metadata, ModInfoMd::Howto as u32, 0, howto as u64)?;
    }

    // Add firmware handle (RSDP)
    if rsdp != 0 {
        add_modmetadata(&mut metadata, ModInfoMd::FwHandle as u32, 0, rsdp)?;
    }

    // Add SMAP table
    if let Some(smap) = smap_info {
        let smap_ptr = smap as *const _ as u64;
        add_modmetadata(&mut metadata, ModInfoMd::Smap as u32, 0, smap_ptr)?;
    }

    // Add EFI memory map
    if let Some(efi_map) = efi_map_info {
        let efi_map_ptr = efi_map as *const _ as u64;
        add_modmetadata(&mut metadata, ModInfoMd::EfiMap as u32, 0, efi_map_ptr)?;
    }

    // Add EFI framebuffer
    if let Some(efi_fb) = efi_fb_info {
        let efi_fb_ptr = efi_fb as *const _ as u64;
        add_modmetadata(&mut metadata, ModInfoMd::EfiFb as u32, 0, efi_fb_ptr)?;
    }

    // Add font
    if let Some(font) = font_info {
        let font_ptr = font as *const _ as u64;
        add_modmetadata(&mut metadata, ModInfoMd::Font as u32, 0, font_ptr)?;
    }

    // Add kernel end
    let kernel_end = kernel_data.len() as u64;
    add_modmetadata(&mut metadata, ModInfoMd::Kernend as u32, 0, kernel_end)?;

    Ok(metadata)
}

/// Add modmetadata entry
fn add_modmetadata(metadata: &mut Vec<u8>, type_: u32, subtype: u32, data: u64) -> Result<()> {
    metadata.extend_from_slice(&type_.to_le_bytes());
    metadata.extend_from_slice(&subtype.to_le_bytes());
    metadata.extend_from_slice(&data.to_le_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::Module;
    use crate::types::ModuleType;

    fn parse_modinfo_entries(buf: &[u8]) -> Vec<(u32, Vec<u8>)> {
        let mut entries = Vec::new();
        let mut offset = 0usize;
        let align = std::mem::size_of::<u64>();

        while offset + 8 <= buf.len() {
            let type_ = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
            let size = u32::from_le_bytes(buf[offset + 4..offset + 8].try_into().unwrap()) as usize;
            offset += 8;
            if type_ == MODINFO_END {
                entries.push((type_, Vec::new()));
                break;
            }
            if offset + size > buf.len() {
                break;
            }
            let data = buf[offset..offset + size].to_vec();
            entries.push((type_, data));
            offset += size;
            let pad = (align - (offset % align)) % align;
            offset += pad;
        }

        entries
    }

    fn metadata_types(entries: &[(u32, Vec<u8>)]) -> Vec<u32> {
        entries
            .iter()
            .filter(|(t, _)| (*t & MODINFO_METADATA) == MODINFO_METADATA)
            .map(|(t, _)| *t)
            .collect()
    }

    #[test]
    fn test_build_modulep_kernel_and_module() {
        let mut module = Module::new("boot/kernel/zfs.ko".to_string(), ModuleType::ElfModule, vec![1, 2, 3]);
        module.set_physical_address(0x3000_0000);

        let buf = build_modulep(
            "/boot/kernel/kernel",
            0x2000_0000,
            0x1234,
            0x9,
            0x0,
            None,
            None,
            None,
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            &[module],
            0x4000_0000,
        )
        .unwrap();
        let entries = parse_modinfo_entries(&buf);

        let names: Vec<String> = entries
            .iter()
            .filter(|(t, _)| *t == MODINFO_NAME)
            .map(|(_, data)| {
                let nul = data.iter().position(|b| *b == 0).unwrap_or(data.len());
                String::from_utf8_lossy(&data[..nul]).to_string()
            })
            .collect();

        assert!(names.contains(&"/boot/kernel/kernel".to_string()));
        assert!(names.contains(&"zfs".to_string()));

        let has_kernend = entries.iter().any(|(t, data)| {
            *t == (MODINFO_METADATA | ModInfoMd::Kernend as u32)
                && data.len() == 8
        });
        assert!(has_kernend);
    }

    #[test]
    fn test_build_modulep_metadata_order_and_sizes() {
        let mut efi_map = crate::types::EfiMapInfo::default();
        efi_map.memory_size = 0x40;
        efi_map.descriptor_size = 0x30;
        efi_map.descriptor_version = 1;
        efi_map.efi_table[0] = crate::types::EfiMapEntry {
            type_: 7,
            pad: 0,
            phys: 0x1122_3344_5566_7788,
            virt: 0,
            pages: 0x99,
            attr: 0xaa,
        };

        let elfhdr = vec![0x7f, b'E', b'L', b'F'];
        let shdr = vec![0u8; 0x30];

        let buf = build_modulep(
            "/boot/kernel/kernel",
            0x2000_0000,
            0x2000,
            0x1,
            0x1234,
            Some(&efi_map),
            None,
            None,
            0x8000,
            Some(&elfhdr),
            Some(&shdr),
            Some(0xfeed_0000),
            Some((0x9000, 0x9100)),
            Some(0x9200),
            Some(0x9300),
            None,
            &[],
            0x4010_0000,
        )
        .unwrap();
        let entries = parse_modinfo_entries(&buf);
        let meta_types = metadata_types(&entries);

        let types: Vec<u32> = entries.iter().map(|(t, _)| *t).collect();
        let keybuf_type = MODINFO_METADATA | ModInfoMd::KeyBuf as u32;
        let kernend_type = MODINFO_METADATA | ModInfoMd::Kernend as u32;
        let howto_type = MODINFO_METADATA | ModInfoMd::Howto as u32;

        let keybuf_idx = types.iter().position(|t| *t == keybuf_type).unwrap();
        let kernend_idx = types.iter().position(|t| *t == kernend_type).unwrap();
        let howto_idx = types.iter().position(|t| *t == howto_type).unwrap();

        assert!(keybuf_idx < kernend_idx);
        assert!(kernend_idx < howto_idx);

        let keybuf_size = entries
            .iter()
            .find(|(t, _)| *t == keybuf_type)
            .map(|(_, data)| data.len())
            .unwrap();
        assert_eq!(keybuf_size, crate::types::GELI_KEYBUF_SIZE);

        let howto_size = entries
            .iter()
            .find(|(t, _)| *t == howto_type)
            .map(|(_, data)| data.len())
            .unwrap();
        assert_eq!(howto_size, 4);

        let elfhdr_size = entries
            .iter()
            .find(|(t, _)| *t == (MODINFO_METADATA | ModInfoMd::Elfhdr as u32))
            .map(|(_, data)| data.len())
            .unwrap();
        assert_eq!(elfhdr_size, elfhdr.len());

        let shdr_size = entries
            .iter()
            .find(|(t, _)| *t == (MODINFO_METADATA | ModInfoMd::Shdr as u32))
            .map(|(_, data)| data.len())
            .unwrap();
        assert_eq!(shdr_size, shdr.len());

        let efi_map_size = entries
            .iter()
            .find(|(t, _)| *t == (MODINFO_METADATA | ModInfoMd::EfiMap as u32))
            .map(|(_, data)| data.len())
            .unwrap();
        let header_size = std::mem::size_of::<crate::types::EfiMapHeader>() as u64;
        let header_aligned = crate::system::align_up(header_size, 16);
        let expected_efi_map = (header_aligned + efi_map.memory_size) as usize;
        assert_eq!(efi_map_size, expected_efi_map);
        let efi_map_bytes = entries
            .iter()
            .find(|(t, _)| *t == (MODINFO_METADATA | ModInfoMd::EfiMap as u32))
            .map(|(_, data)| data.as_slice())
            .unwrap();
        let entry_offset = header_aligned as usize;
        let entry = &efi_map_bytes[entry_offset..entry_offset + 0x30];
        assert_eq!(u32::from_le_bytes(entry[0..4].try_into().unwrap()), 7);
        assert_eq!(u64::from_le_bytes(entry[8..16].try_into().unwrap()), 0x1122_3344_5566_7788);
        assert_eq!(u64::from_le_bytes(entry[24..32].try_into().unwrap()), 0x99);
        assert_eq!(u64::from_le_bytes(entry[32..40].try_into().unwrap()), 0xaa);

        let expected = vec![
            MODINFO_METADATA | ModInfoMd::EfiMap as u32,
            MODINFO_METADATA | ModInfoMd::FwHandle as u32,
            MODINFO_METADATA | ModInfoMd::KeyBuf as u32,
            MODINFO_METADATA | ModInfoMd::Modulep as u32,
            MODINFO_METADATA | ModInfoMd::Kernend as u32,
            MODINFO_METADATA | ModInfoMd::Envp as u32,
            MODINFO_METADATA | ModInfoMd::Howto as u32,
            MODINFO_METADATA | ModInfoMd::Elfhdr as u32,
            MODINFO_METADATA | ModInfoMd::Dynamic as u32,
            MODINFO_METADATA | ModInfoMd::Esym as u32,
            MODINFO_METADATA | ModInfoMd::Ssym as u32,
            MODINFO_METADATA | ModInfoMd::Shdr as u32,
            MODINFO_METADATA | ModInfoMd::Font as u32,
        ];
        assert_eq!(meta_types, expected);
    }
}
