use crate::error::Result;
use goblin::elf;

/// ELF relocation types for x86_64
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocationType {
    R64 = 1,      // R_X86_64_64
    Pc32 = 2,     // R_X86_64_PC32
    Got32 = 3,    // R_X86_64_GOT32
    Plt32 = 4,    // R_X86_64_PLT32
    Copy = 5,     // R_X86_64_COPY
    GlobDat = 6,  // R_X86_64_GLOB_DAT
    JumpSlot = 7, // R_X86_64_JUMP_SLOT
    Relative = 8, // R_X86_64_RELATIVE
    GotPcRel = 9, // R_X86_64_GOTPCREL
    TlsDtpMod32 = 18, // R_X86_64_TLS_DTPMOD32
    TlsDtpOff32 = 19, // R_X86_64_TLS_DTPOFF32
    TlsTpOff32 = 20,  // R_X86_64_TLS_TPOFF32
}

impl TryFrom<u32> for RelocationType {
    type Error = ();

    fn try_from(value: u32) -> std::result::Result<Self, Self::Error> {
        match value {
            1 => Ok(RelocationType::R64),
            2 => Ok(RelocationType::Pc32),
            3 => Ok(RelocationType::Got32),
            4 => Ok(RelocationType::Plt32),
            5 => Ok(RelocationType::Copy),
            6 => Ok(RelocationType::GlobDat),
            7 => Ok(RelocationType::JumpSlot),
            8 => Ok(RelocationType::Relative),
            9 => Ok(RelocationType::GotPcRel),
            18 => Ok(RelocationType::TlsDtpMod32),
            19 => Ok(RelocationType::TlsDtpOff32),
            20 => Ok(RelocationType::TlsTpOff32),
            _ => Err(()),
        }
    }
}

/// ELF symbol binding
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolBinding {
    Local = 0,
    Global = 1,
    Weak = 2,
}

impl TryFrom<u8> for SymbolBinding {
    type Error = ();

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(SymbolBinding::Local),
            1 => Ok(SymbolBinding::Global),
            2 => Ok(SymbolBinding::Weak),
            _ => Err(()),
        }
    }
}

/// ELF symbol type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolType {
    NoType = 0,
    Object = 1,
    Func = 2,
    Section = 3,
    File = 4,
    Common = 5,
    Tls = 6,
}

impl TryFrom<u8> for SymbolType {
    type Error = ();

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(SymbolType::NoType),
            1 => Ok(SymbolType::Object),
            2 => Ok(SymbolType::Func),
            3 => Ok(SymbolType::Section),
            4 => Ok(SymbolType::File),
            5 => Ok(SymbolType::Common),
            6 => Ok(SymbolType::Tls),
            _ => Err(()),
        }
    }
}

/// ELF symbol visibility
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolVisibility {
    Default = 0,
    Internal = 1,
    Hidden = 2,
    Protected = 3,
}

impl TryFrom<u8> for SymbolVisibility {
    type Error = ();

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(SymbolVisibility::Default),
            1 => Ok(SymbolVisibility::Internal),
            2 => Ok(SymbolVisibility::Hidden),
            3 => Ok(SymbolVisibility::Protected),
            _ => Err(()),
        }
    }
}

/// ELF symbol information
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub name: String,
    pub value: u64,
    pub size: u64,
    pub binding: SymbolBinding,
    pub type_: SymbolType,
    pub visibility: SymbolVisibility,
    pub section_index: u16,
    pub is_defined: bool,
    pub is_absolute: bool,
    pub is_common: bool,
}

impl SymbolInfo {
    pub fn from_goblin(sym: &elf::Sym, strtab: &[u8]) -> Result<Self> {
        let name = if sym.st_name == 0 {
            String::new()
        } else {
            let strtab_obj =
                goblin::strtab::Strtab::from_slice_unparsed(strtab, 0, strtab.len(), 0);
            match strtab_obj.get_unsafe(sym.st_name as usize) {
                Some(s) => s.to_string(),
                None => String::new(),
            }
        };

        let binding = SymbolBinding::try_from(sym.st_bind()).unwrap_or(SymbolBinding::Local);
        let type_ = SymbolType::try_from(sym.st_type()).unwrap_or(SymbolType::NoType);
        let visibility = SymbolVisibility::try_from(sym.st_visibility())
            .unwrap_or(SymbolVisibility::Default);

        let section_index: u16 = sym.st_shndx as u16;
        let is_defined = section_index != elf::section_header::SHN_UNDEF as u16;
        let is_absolute = section_index == elf::section_header::SHN_ABS as u16;
        let is_common = section_index == elf::section_header::SHN_COMMON as u16;

        Ok(Self {
            name,
            value: sym.st_value,
            size: sym.st_size,
            binding,
            type_,
            visibility,
            section_index: section_index as u16,
            is_defined,
            is_absolute,
            is_common,
        })
    }
}

/// ELF relocation entry
#[derive(Debug, Clone)]
pub struct Relocation {
    pub offset: u64,
    pub symbol_index: u32,
    pub type_: RelocationType,
    pub addend: Option<i64>, // Only for RELA
}

impl Relocation {
    pub fn from_rel(rel: &goblin::elf64::reloc::Rel, is_64bit: bool) -> Result<Self> {
        let type_ = if is_64bit {
            goblin::elf::reloc::reloc64::r_type(rel.r_info)
        } else {
            goblin::elf::reloc::reloc32::r_type(rel.r_info as u32)
        };

        let symbol_index = if is_64bit {
            goblin::elf::reloc::reloc64::r_sym(rel.r_info)
        } else {
            goblin::elf::reloc::reloc32::r_sym(rel.r_info as u32)
        };

        Ok(Self {
            offset: rel.r_offset,
            symbol_index,
            type_: RelocationType::try_from(type_).map_err(|_| {
                crate::error::BootError::ElfParse(format!("Unknown relocation type: {}", type_))
            })?,
            addend: None,
        })
    }

    pub fn from_rela(rela: &goblin::elf64::reloc::Rela, is_64bit: bool) -> Result<Self> {
        let type_ = if is_64bit {
            goblin::elf::reloc::reloc64::r_type(rela.r_info)
        } else {
            goblin::elf::reloc::reloc32::r_type(rela.r_info as u32)
        };

        let symbol_index = if is_64bit {
            goblin::elf::reloc::reloc64::r_sym(rela.r_info)
        } else {
            goblin::elf::reloc::reloc32::r_sym(rela.r_info as u32)
        };

        Ok(Self {
            offset: rela.r_offset,
            symbol_index,
            type_: RelocationType::try_from(type_).map_err(|_| {
                crate::error::BootError::ElfParse(format!("Unknown relocation type: {}", type_))
            })?,
            addend: Some(rela.r_addend),
        })
    }
}

/// Section flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionFlag {
    Write = 0x1,      // SHF_WRITE
    Alloc = 0x2,      // SHF_ALLOC
    Exec = 0x4,       // SHF_EXECINSTR
    Merge = 0x10,     // SHF_MERGE
    Strings = 0x20,   // SHF_STRINGS
    InfoLink = 0x40,  // SHF_INFO_LINK
    LinkOrder = 0x80, // SHF_LINK_ORDER
    OsNonConforming = 0x100, // SHF_OS_NONCONFORMING
    Group = 0x200,    // SHF_GROUP
    Tls = 0x400,      // SHF_TLS
}

impl SectionFlag {
    pub fn from_bits(bits: u64) -> Vec<Self> {
        let mut flags = Vec::new();
        if bits & 0x1 != 0 {
            flags.push(SectionFlag::Write);
        }
        if bits & 0x2 != 0 {
            flags.push(SectionFlag::Alloc);
        }
        if bits & 0x4 != 0 {
            flags.push(SectionFlag::Exec);
        }
        if bits & 0x10 != 0 {
            flags.push(SectionFlag::Merge);
        }
        if bits & 0x20 != 0 {
            flags.push(SectionFlag::Strings);
        }
        if bits & 0x40 != 0 {
            flags.push(SectionFlag::InfoLink);
        }
        if bits & 0x80 != 0 {
            flags.push(SectionFlag::LinkOrder);
        }
        if bits & 0x100 != 0 {
            flags.push(SectionFlag::OsNonConforming);
        }
        if bits & 0x200 != 0 {
            flags.push(SectionFlag::Group);
        }
        if bits & 0x400 != 0 {
            flags.push(SectionFlag::Tls);
        }
        flags
    }
}

/// Section information
#[derive(Debug, Clone)]
pub struct SectionInfo {
    pub name: String,
    pub type_: u32,
    pub flags: u64,
    pub addr: u64,
    pub offset: u64,
    pub size: u64,
    pub link: u32,
    pub info: u32,
    pub addralign: u64,
    pub entsize: u64,
}

impl SectionInfo {
    pub fn from_goblin(sh: &elf::SectionHeader, name: &str) -> Self {
        Self {
            name: name.to_string(),
            type_: sh.sh_type,
            flags: sh.sh_flags,
            addr: sh.sh_addr,
            offset: sh.sh_offset,
            size: sh.sh_size,
            link: sh.sh_link,
            info: sh.sh_info,
            addralign: sh.sh_addralign,
            entsize: sh.sh_entsize,
        }
    }

    pub fn has_flag(&self, flag: SectionFlag) -> bool {
        self.flags & (flag as u64) != 0
    }

    pub fn is_alloc(&self) -> bool {
        self.has_flag(SectionFlag::Alloc)
    }

    pub fn is_write(&self) -> bool {
        self.has_flag(SectionFlag::Write)
    }

    pub fn is_exec(&self) -> bool {
        self.has_flag(SectionFlag::Exec)
    }
}

/// Program header flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentFlag {
    Execute = 0x1, // PF_X
    Write = 0x2,   // PF_W
    Read = 0x4,    // PF_R
}

impl SegmentFlag {
    pub fn from_bits(bits: u32) -> Vec<Self> {
        let mut flags = Vec::new();
        if bits & 0x1 != 0 {
            flags.push(SegmentFlag::Execute);
        }
        if bits & 0x2 != 0 {
            flags.push(SegmentFlag::Write);
        }
        if bits & 0x4 != 0 {
            flags.push(SegmentFlag::Read);
        }
        flags
    }
}

/// Program segment information
#[derive(Debug, Clone)]
pub struct SegmentInfo {
    pub type_: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub paddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

impl SegmentInfo {
    pub fn from_goblin(ph: &elf::ProgramHeader) -> Self {
        Self {
            type_: ph.p_type,
            flags: ph.p_flags,
            offset: ph.p_offset,
            vaddr: ph.p_vaddr,
            paddr: ph.p_paddr,
            filesz: ph.p_filesz,
            memsz: ph.p_memsz,
            align: ph.p_align,
        }
    }

    pub fn has_flag(&self, flag: SegmentFlag) -> bool {
        self.flags & (flag as u32) != 0
    }

    pub fn is_load(&self) -> bool {
        self.type_ == elf::program_header::PT_LOAD
    }

    pub fn is_dynamic(&self) -> bool {
        self.type_ == elf::program_header::PT_DYNAMIC
    }

    pub fn is_interp(&self) -> bool {
        self.type_ == elf::program_header::PT_INTERP
    }

    pub fn is_gnu_stack(&self) -> bool {
        self.type_ == elf::program_header::PT_GNU_STACK
    }

    pub fn is_gnu_relro(&self) -> bool {
        self.type_ == elf::program_header::PT_GNU_RELRO
    }
}
