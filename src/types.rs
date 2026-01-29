use serde::{Deserialize, Serialize};
use std::fmt;

/// Framebuffer information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FbInfo {
    pub id: String,
    pub phys: u64,
    pub size: usize,
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u32,
    pub bpp: u32,
    pub mask_red: u32,
    pub mask_green: u32,
    pub mask_blue: u32,
    pub mask_reserved: u32,
    pub extra1: u64,
}

impl Default for FbInfo {
    fn default() -> Self {
        Self {
            id: String::new(),
            phys: 0,
            size: 0,
            width: 0,
            height: 0,
            stride_bytes: 0,
            bpp: 0,
            mask_red: 0,
            mask_green: 0,
            mask_blue: 0,
            mask_reserved: 0,
            extra1: 0,
        }
    }
}

/// SMAP/E820 memory map entry
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SmapEntry {
    pub addr: u64,
    pub size: u64,
    pub type_: u32,
}

/// SMAP memory map information
#[derive(Debug, Clone)]
pub struct SmapInfo {
    pub e820_table: [SmapEntry; 128],
    pub e820_entries: u8,
}

impl Default for SmapInfo {
    fn default() -> Self {
        Self {
            e820_table: [SmapEntry {
                addr: 0,
                size: 0,
                type_: 0,
            }; 128],
            e820_entries: 0,
        }
    }
}

/// EFI memory map entry
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EfiMapEntry {
    pub type_: u32,
    pub pad: u32,
    pub phys: u64,
    pub virt: u64,
    pub pages: u64,
    pub attr: u64,
}

/// EFI memory map information
#[derive(Debug, Clone)]
pub struct EfiMapInfo {
    pub memory_size: u64,
    pub descriptor_size: u64,
    pub descriptor_version: u32,
    pub pad1: u32,
    pub map_phys: u64,
    pub efi_table: [EfiMapEntry; 128],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EfiMapHeader {
    pub memory_size: u64,
    pub descriptor_size: u64,
    pub descriptor_version: u32,
    pub pad: u32,
}

impl Default for EfiMapInfo {
    fn default() -> Self {
        Self {
            memory_size: 0,
            descriptor_size: 0,
            descriptor_version: 0,
            pad1: 0,
            map_phys: 0,
            efi_table: [EfiMapEntry {
                type_: 0,
                pad: 0,
                phys: 0,
                virt: 0,
                pages: 0,
                attr: 0,
            }; 128],
        }
    }
}

/// EFI framebuffer information
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EfiFbInfo {
    pub addr: u64,
    pub size: u64,
    pub height: u32,
    pub width: u32,
    pub stride: u32,
    pub mask_red: u32,
    pub mask_green: u32,
    pub mask_blue: u32,
    pub mask_reserved: u32,
}

/// Font header structure
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct FontHeader {
    pub fh_magic: [u8; 8],
    pub fh_width: u8,
    pub fh_height: u8,
    pub fh_pad: u16,
    pub fh_glyph_count: u32,
    pub fh_map_count: [u32; 4], // VFNT_MAPS = 4
}

/// Font information structure
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FontInfo {
    pub fi_checksum: i32,
    pub fi_width: u32,
    pub fi_height: u32,
    pub fi_bitmap_size: u32,
    pub fi_map_count: [u32; 4],
}

/// Font mapping structure
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct VfntMap {
    pub vfm_src: u32,
    pub vfm_dst: u16,
    pub vfm_len: u16,
}

/// Font map types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfntMapType {
    Normal = 0,
    NormalRight = 1,
    Bold = 2,
    BoldRight = 3,
}

impl VfntMapType {
    pub const COUNT: usize = 4;
}

/// Module types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleType {
    ElfKernel,
    ElfModule,
    ElfObj,
    Raw(String),
}

impl fmt::Display for ModuleType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModuleType::ElfKernel => write!(f, "elf kernel"),
            ModuleType::ElfModule => write!(f, "elf module"),
            ModuleType::ElfObj => write!(f, "elf obj"),
            ModuleType::Raw(name) => write!(f, "{}", name),
        }
    }
}

/// Boot howto flags (from FreeBSD's reboot.h)
#[derive(Debug, Clone, Copy, Default)]
pub struct BootHowto(u32);

impl BootHowto {
    // Values from FreeBSD sys/sys/reboot.h
    pub const RB_CDROM: u32 = 0x00002000;
    pub const RB_MULTIPLE: u32 = 0x20000000;
    pub const RB_SERIAL: u32 = 0x00001000;
    pub const RB_VERBOSE: u32 = 0x00000800;

    pub fn new() -> Self {
        Self(0)
    }

    pub fn set(&mut self, flag: u32) {
        self.0 |= flag;
    }

    pub fn has(&self, flag: u32) -> bool {
        self.0 & flag != 0
    }

    pub fn value(&self) -> u32 {
        self.0
    }
}

/// Kexec segment (compatible with Linux kexec interface)
#[repr(C)]
#[derive(Debug, Clone)]
pub struct KexecSegment {
    pub buf: *const u8,
    pub bufsz: usize,
    pub mem: *const u8,
    pub memsz: usize,
}

impl Default for KexecSegment {
    fn default() -> Self {
        Self {
            buf: std::ptr::null(),
            bufsz: 0,
            mem: std::ptr::null(),
            memsz: 0,
        }
    }
}

/// Module metadata types (from FreeBSD)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModMetadataType {
    Depend = 1,
    Module = 2,
    Version = 3,
    PnpInfo = 4,
    ModulePnp = 5,
    Sbuf = 6,
}

impl TryFrom<u32> for ModMetadataType {
    type Error = ();

    fn try_from(value: u32) -> std::result::Result<Self, Self::Error> {
        match value {
            1 => Ok(ModMetadataType::Depend),
            2 => Ok(ModMetadataType::Module),
            3 => Ok(ModMetadataType::Version),
            4 => Ok(ModMetadataType::PnpInfo),
            5 => Ok(ModMetadataType::ModulePnp),
            6 => Ok(ModMetadataType::Sbuf),
            _ => Err(()),
        }
    }
}

/// Module information metadata (MODINFO_METADATA)
#[derive(Debug, Clone, Copy)]
pub enum ModInfoMd {
    Ssym = 0x0003,      // Start of symbol table
    Esym = 0x0004,      // End of symbol table
    Dynamic = 0x0005,   // Dynamic section offset
    Envp = 0x0006,      // Environment pointer
    Howto = 0x0007,     // Boot howto
    Kernend = 0x0008,   // Kernel end
    Shdr = 0x0009,      // Section headers
    FwHandle = 0x000c,  // Firmware handle (EFI system table)
    KeyBuf = 0x000d,    // Crypto key intake buffer
    Font = 0x000e,      // Font pointer
    Smap = 0x1001,      // SMAP table
    EfiMap = 0x1004,    // EFI memory map
    EfiFb = 0x1005,     // EFI framebuffer
    Modulep = 0x1006,   // Modulep offset
    Elfhdr = 0x0002,    // ELF header
}

/// Constants
pub const MODINFO_END: u32 = 0x0000;
pub const MODINFO_NAME: u32 = 0x0001;
pub const MODINFO_TYPE: u32 = 0x0002;
pub const MODINFO_ADDR: u32 = 0x0003;
pub const MODINFO_SIZE: u32 = 0x0004;
pub const MODINFO_ARGS: u32 = 0x0006;
pub const MODINFO_METADATA: u32 = 0x8000;
pub const KEXEC_SEGMENT_MAX: usize = 16;
pub const KERNBASE: u64 = 0xffff_ffff_8000_0000;
pub const KERNEL_PHYS_BASE: u64 = 0x20_0000;
pub const BOOT_PHYS_BASE: u64 = 0x10_0000;
pub const PAGE_SIZE: usize = 4096;
pub const SEGALIGN: usize = 1024 * 1024; // 1MB alignment
pub const GELI_KEYBUF_SIZE: usize = 0x8104; // From FreeBSD geliboot.h

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fb_info_default() {
        let fb = FbInfo::default();
        assert_eq!(fb.id, "");
        assert_eq!(fb.phys, 0);
        assert_eq!(fb.width, 0);
        assert_eq!(fb.height, 0);
    }

    #[test]
    fn test_smap_info_default() {
        let smap = SmapInfo::default();
        assert_eq!(smap.e820_entries, 0);
        // We can't safely test packed struct fields without unsafe code
        // The default values are set in the Default implementation
    }

    #[test]
    fn test_efi_map_info_default() {
        let efi_map = EfiMapInfo::default();
        assert_eq!(efi_map.memory_size, 0);
        assert_eq!(efi_map.descriptor_size, 0);
        assert_eq!(efi_map.descriptor_version, 0);
        // We can't safely test packed struct fields without unsafe code
        // The default values are set in the Default implementation
    }

    #[test]
    fn test_boot_howto() {
        let mut howto = BootHowto::new();
        assert_eq!(howto.value(), 0);
        
        howto.set(BootHowto::RB_VERBOSE);
        howto.set(BootHowto::RB_SERIAL);
        
        assert!(howto.has(BootHowto::RB_VERBOSE));
        assert!(howto.has(BootHowto::RB_SERIAL));
        assert!(!howto.has(BootHowto::RB_CDROM));
        
        assert_eq!(howto.value(), BootHowto::RB_VERBOSE | BootHowto::RB_SERIAL);
    }

    #[test]
    fn test_module_type_display() {
        assert_eq!(ModuleType::ElfKernel.to_string(), "elf kernel");
        assert_eq!(ModuleType::ElfModule.to_string(), "elf module");
        assert_eq!(ModuleType::ElfObj.to_string(), "elf obj");
        assert_eq!(ModuleType::Raw("mfs_root".to_string()).to_string(), "mfs_root");
    }

    #[test]
    fn test_mod_metadata_type_conversion() {
        assert_eq!(ModMetadataType::try_from(1).unwrap(), ModMetadataType::Depend);
        assert_eq!(ModMetadataType::try_from(2).unwrap(), ModMetadataType::Module);
        assert_eq!(ModMetadataType::try_from(3).unwrap(), ModMetadataType::Version);
        assert_eq!(ModMetadataType::try_from(4).unwrap(), ModMetadataType::PnpInfo);
        assert_eq!(ModMetadataType::try_from(5).unwrap(), ModMetadataType::ModulePnp);
        assert_eq!(ModMetadataType::try_from(6).unwrap(), ModMetadataType::Sbuf);
        
        assert!(ModMetadataType::try_from(0).is_err());
        assert!(ModMetadataType::try_from(7).is_err());
    }

    #[test]
    fn test_kexec_segment_default() {
        let segment = KexecSegment::default();
        assert!(segment.buf.is_null());
        assert_eq!(segment.bufsz, 0);
        assert!(segment.mem.is_null());
        assert_eq!(segment.memsz, 0);
    }

    #[test]
    fn test_constants() {
        assert_eq!(KEXEC_SEGMENT_MAX, 16);
        assert_eq!(KERNBASE, 0xffff_ffff_8000_0000);
        assert_eq!(KERNEL_PHYS_BASE, 0x20_0000);
        assert_eq!(BOOT_PHYS_BASE, 0x10_0000);
        assert_eq!(PAGE_SIZE, 4096);
        assert_eq!(SEGALIGN, 1024 * 1024);
    }
}
