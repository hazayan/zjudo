mod bootdev;
mod kexec;
mod memory;
mod smp;
mod sysinfo;

pub use bootdev::*;
pub use kexec::*;
pub use memory::*;
pub use smp::*;
pub use sysinfo::*;

use crate::error::{BootError, Result};
use crate::types::{EfiFbInfo, EfiMapInfo, FbInfo, SmapInfo};
use std::fs;
use std::path::Path;

/// Check if running as root
pub fn check_root() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(BootError::Permission(
            "This operation requires root privileges".to_string(),
        ));
    }
    Ok(())
}

/// Read entire file to vector
pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(BootError::Io)
}

/// Read file as string
pub fn read_file_string(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(BootError::Io)
}

/// Check if path exists
pub fn path_exists(path: &Path) -> bool {
    path.exists()
}

/// Round up to alignment
pub fn round_up(value: usize, alignment: usize) -> usize {
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

/// Round down to alignment
pub fn round_down(value: usize, alignment: usize) -> usize {
    if alignment == 0 {
        return value;
    }
    value - (value % alignment)
}

/// System information collector
pub struct SystemInfo {
    pub fb_info: Option<FbInfo>,
    pub efi_fb_info: Option<EfiFbInfo>,
    pub smap_info: SmapInfo,
    pub efi_map_info: EfiMapInfo,
    pub efi_systab: u64,
    pub rsdp: u64,
    pub rsdt: u64,
    pub is_efi: bool,
}

impl SystemInfo {
    pub fn collect() -> Result<Self> {
        let is_efi = sysinfo::is_efi();
        let fb_info = sysinfo::fetch_fb().ok();
        let efi_fb_info = if is_efi {
            fb_info
                .as_ref()
                .and_then(|fb| sysinfo::build_efi_fb_info(fb))
        } else {
            None
        };
        let smap_info = sysinfo::fetch_smap()?;
        let efi_map_info = if is_efi {
            sysinfo::fetch_efi_map()?
        } else {
            EfiMapInfo::default()
        };
        let efi_systab = if is_efi {
            sysinfo::fetch_efi_systab()?
        } else {
            0
        };
        let (rsdp, rsdt) = sysinfo::fetch_acpi20(is_efi)?;

        Ok(Self {
            fb_info,
            efi_fb_info,
            smap_info,
            efi_map_info,
            efi_systab,
            rsdp,
            rsdt,
            is_efi,
        })
    }
}
