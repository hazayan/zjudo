use crate::error::{BootError, Result};
use crate::system::read_file_string;
use crate::types::{EfiFbInfo, EfiMapInfo, FbInfo, SmapInfo};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

const BOOT_PARAMS_PATH: &str = "/sys/kernel/boot_params/data";
const EFI_INFO_OFFSET: usize = 0x1c0;
const BOOT_PARAMS_SIZE: usize = 0x1000;

#[derive(Debug, Clone, Copy)]
struct BootParamsEfiInfo {
    systab: u64,
    memmap: u64,
    memmap_size: u32,
    desc_size: u32,
    desc_version: u32,
}

/// Check if system is booted with EFI
pub fn is_efi() -> bool {
    Path::new("/sys/firmware/efi").exists()
}

/// Fetch framebuffer information
pub fn fetch_fb() -> Result<FbInfo> {
    // Try to read from /sys/class/graphics/fb0
    let fb0_path = Path::new("/sys/class/graphics/fb0");
    if !fb0_path.exists() {
        return Err(BootError::System("No framebuffer found".to_string()));
    }

    let mut fb_info = FbInfo::default();
    fb_info.id = "fb0".to_string();

    // Read framebuffer physical address
    if let Ok(phys_addr) = fs::read_to_string(fb0_path.join("phys_addr")) {
        if let Ok(addr) = u64::from_str_radix(phys_addr.trim(), 16) {
            fb_info.phys = addr;
        }
    }

    // Read framebuffer size
    if let Ok(size) = fs::read_to_string(fb0_path.join("size")) {
        if let Ok(sz) = size.trim().parse::<usize>() {
            fb_info.size = sz;
        }
    }

    // Read bits per pixel
    if let Ok(bpp) = fs::read_to_string(fb0_path.join("bits_per_pixel")) {
        if let Ok(bpp_value) = bpp.trim().parse::<u32>() {
            fb_info.bpp = bpp_value;
        }
    }

    // Read stride (bytes per line)
    if let Ok(stride) = fs::read_to_string(fb0_path.join("stride")) {
        if let Ok(stride_value) = stride.trim().parse::<u32>() {
            fb_info.stride_bytes = stride_value;
        }
    } else if let Ok(line_length) = fs::read_to_string(fb0_path.join("line_length")) {
        if let Ok(line_length_value) = line_length.trim().parse::<u32>() {
            fb_info.stride_bytes = line_length_value;
        }
    }

    // Read screen info
    let mode_path = fb0_path.join("modes");
    if mode_path.exists() {
        if let Ok(modes) = fs::read_to_string(&mode_path) {
            for line in modes.lines() {
                if line.contains('x') {
                    let parts: Vec<&str> = line.split('x').collect();
                    if parts.len() >= 2 {
                        if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                            fb_info.width = w;
                            fb_info.height = h;
                            break;
                        }
                    }
                }
            }
        }
    }

    // Fallback to virtual_size if modes are unavailable.
    if fb_info.width == 0 || fb_info.height == 0 {
        if let Ok(virtual_size) = fs::read_to_string(fb0_path.join("virtual_size")) {
            let parts: Vec<&str> = virtual_size.trim().split(',').collect();
            if parts.len() >= 2 {
                if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                    fb_info.width = w;
                    fb_info.height = h;
                }
            }
        }
    }

    // Parse pixel format if exposed by the driver.
    if let Ok(format) = fs::read_to_string(fb0_path.join("format")) {
        if let Some((bpp, red, green, blue, reserved)) = parse_fb_format(format.trim()) {
            if fb_info.bpp == 0 {
                fb_info.bpp = bpp;
            }
            fb_info.mask_red = red;
            fb_info.mask_green = green;
            fb_info.mask_blue = blue;
            fb_info.mask_reserved = reserved;
        }
    }

    // Derive stride and size if missing.
    if fb_info.stride_bytes == 0 && fb_info.size > 0 && fb_info.height > 0 {
        let stride = fb_info.size / fb_info.height as usize;
        fb_info.stride_bytes = stride as u32;
    }
    if fb_info.bpp == 0 && fb_info.stride_bytes > 0 && fb_info.width > 0 {
        let bytes_per_pixel = fb_info.stride_bytes / fb_info.width;
        if bytes_per_pixel > 0 {
            fb_info.bpp = bytes_per_pixel * 8;
        }
    }
    if fb_info.size == 0 && fb_info.stride_bytes > 0 && fb_info.height > 0 {
        fb_info.size = fb_info.stride_bytes as usize * fb_info.height as usize;
    }

    // Default masks for common pixel layouts when format is unknown.
    if fb_info.mask_red == 0 && fb_info.mask_green == 0 && fb_info.mask_blue == 0 {
        match fb_info.bpp {
            32 => {
                fb_info.mask_red = 0x00ff0000;
                fb_info.mask_green = 0x0000ff00;
                fb_info.mask_blue = 0x000000ff;
                fb_info.mask_reserved = 0xff000000;
            }
            24 => {
                fb_info.mask_red = 0x00ff0000;
                fb_info.mask_green = 0x0000ff00;
                fb_info.mask_blue = 0x000000ff;
                fb_info.mask_reserved = 0;
            }
            16 => {
                fb_info.mask_red = 0x0000f800;
                fb_info.mask_green = 0x000007e0;
                fb_info.mask_blue = 0x0000001f;
                fb_info.mask_reserved = 0;
            }
            _ => {}
        }
    }

    Ok(fb_info)
}

/// Check if framebuffer is usable
pub fn is_framebuffer_usable(fb: &FbInfo) -> bool {
    fb.phys != 0 && fb.width > 0 && fb.height > 0 && fb.stride_bytes > 0 && fb.bpp > 0
}

pub fn build_efi_fb_info(fb: &FbInfo) -> Option<EfiFbInfo> {
    if !is_framebuffer_usable(fb) {
        return None;
    }
    if fb.mask_red == 0 && fb.mask_green == 0 && fb.mask_blue == 0 {
        return None;
    }

    let bytes_per_pixel = fb.bpp / 8;
    if bytes_per_pixel == 0 {
        return None;
    }
    let stride = fb.stride_bytes / bytes_per_pixel;
    if stride == 0 {
        return None;
    }

    Some(EfiFbInfo {
        addr: fb.phys,
        size: fb.size as u64,
        height: fb.height,
        width: fb.width,
        stride,
        mask_red: fb.mask_red,
        mask_green: fb.mask_green,
        mask_blue: fb.mask_blue,
        mask_reserved: fb.mask_reserved,
    })
}

fn parse_fb_format(format: &str) -> Option<(u32, u32, u32, u32, u32)> {
    match format.to_ascii_lowercase().as_str() {
        "a8r8g8b8" => Some((32, 0x00ff0000, 0x0000ff00, 0x000000ff, 0xff000000)),
        "x8r8g8b8" => Some((32, 0x00ff0000, 0x0000ff00, 0x000000ff, 0x00000000)),
        "a8b8g8r8" => Some((32, 0x000000ff, 0x0000ff00, 0x00ff0000, 0xff000000)),
        "x8b8g8r8" => Some((32, 0x000000ff, 0x0000ff00, 0x00ff0000, 0x00000000)),
        "r5g6b5" => Some((16, 0x0000f800, 0x000007e0, 0x0000001f, 0)),
        "b5g6r5" => Some((16, 0x0000001f, 0x000007e0, 0x0000f800, 0)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_fb_format_known() {
        assert_eq!(
            parse_fb_format("x8r8g8b8"),
            Some((32, 0x00ff0000, 0x0000ff00, 0x000000ff, 0x00000000))
        );
        assert_eq!(
            parse_fb_format("a8b8g8r8"),
            Some((32, 0x000000ff, 0x0000ff00, 0x00ff0000, 0xff000000))
        );
        assert_eq!(
            parse_fb_format("r5g6b5"),
            Some((16, 0x0000f800, 0x000007e0, 0x0000001f, 0))
        );
    }

    #[test]
    fn test_build_efi_fb_info_rejects_invalid() {
        let mut fb = FbInfo::default();
        fb.phys = 0x1000;
        fb.width = 1024;
        fb.height = 768;
        fb.bpp = 32;
        fb.stride_bytes = 4096;
        assert!(build_efi_fb_info(&fb).is_none());
    }

    #[test]
    fn test_build_efi_fb_info_success() {
        let mut fb = FbInfo::default();
        fb.phys = 0x1000;
        fb.width = 1024;
        fb.height = 768;
        fb.size = 1024 * 768 * 4;
        fb.bpp = 32;
        fb.stride_bytes = 4096;
        fb.mask_red = 0x00ff0000;
        fb.mask_green = 0x0000ff00;
        fb.mask_blue = 0x000000ff;
        fb.mask_reserved = 0xff000000;

        let efi = build_efi_fb_info(&fb).expect("expected EFI fb info");
        assert_eq!(efi.addr, 0x1000);
        assert_eq!(efi.width, 1024);
        assert_eq!(efi.height, 768);
        assert_eq!(efi.stride, 1024);
        assert_eq!(efi.size, fb.size as u64);
    }
}

/// Fetch SMAP/E820 memory map
pub fn fetch_smap() -> Result<SmapInfo> {
    let mut smap_info = SmapInfo::default();
    let mut entries = 0;

    // Try /sys/firmware/memmap first
    let memmap_path = Path::new("/sys/firmware/memmap");
    if memmap_path.exists() {
        if let Ok(entries_dir) = fs::read_dir(memmap_path) {
            for entry in entries_dir.flatten() {
                if entries >= 128 {
                    break;
                }

                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let start_path = path.join("start");
                let end_path = path.join("end");
                let type_path = path.join("type");

                if let (Ok(start_str), Ok(end_str), Ok(type_str)) = (
                    fs::read_to_string(start_path),
                    fs::read_to_string(end_path),
                    fs::read_to_string(type_path),
                ) {
                    if let (Ok(start), Ok(end)) = (
                        u64::from_str_radix(start_str.trim(), 16),
                        u64::from_str_radix(end_str.trim(), 16),
                    ) {
                        let size = end.saturating_sub(start).saturating_add(1);
                        let desc = type_str.trim();
                        let type_num = if desc.starts_with("System RAM") {
                            1
                        } else if desc.eq_ignore_ascii_case("reserved") {
                            2
                        } else if desc.starts_with("ACPI Tables") {
                            3
                        } else if desc.starts_with("ACPI Non-volatile Storage") {
                            4
                        } else {
                            0
                        };

                        smap_info.e820_table[entries] = crate::types::SmapEntry {
                            addr: start,
                            size,
                            type_: type_num,
                        };
                        entries += 1;
                    }
                }
            }
        }
    }

    // Fall back to /proc/iomem if we didn't get enough entries
    if entries == 0 {
        if let Ok(file) = fs::File::open("/proc/iomem") {
            let reader = BufReader::new(file);
            for line in reader.lines().flatten() {
                if entries >= 128 {
                    break;
                }

                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() < 2 {
                    continue;
                }

                let range = parts[0].trim();
                let desc = parts[1].trim();

                let range_parts: Vec<&str> = range.split('-').collect();
                if range_parts.len() != 2 {
                    continue;
                }

                if let (Ok(start), Ok(end)) = (
                    u64::from_str_radix(range_parts[0], 16),
                    u64::from_str_radix(range_parts[1], 16),
                ) {
                    let size = end.saturating_sub(start).saturating_add(1);
                    let type_num = if desc.starts_with("System RAM") {
                        1
                    } else if desc.eq_ignore_ascii_case("reserved") {
                        2
                    } else if desc.starts_with("ACPI Tables") {
                        3
                    } else if desc.starts_with("ACPI Non-volatile Storage") {
                        4
                    } else {
                        0
                    };

                    if type_num != 0 {
                        smap_info.e820_table[entries] = crate::types::SmapEntry {
                            addr: start,
                            size,
                            type_: type_num,
                        };
                        entries += 1;
                    }
                }
            }
        }
    }

    smap_info.e820_entries = entries as u8;
    log::debug!("smap: entries={}", entries);
    Ok(smap_info)
}

/// Fetch EFI memory map
pub fn fetch_efi_map() -> Result<EfiMapInfo> {
    let mut efi_info = EfiMapInfo::default();

    if let Some(boot_params) = read_boot_params_efi_info()? {
        log::debug!(
            "efi-map: boot_params memmap=0x{:x} size=0x{:x} desc_size=0x{:x} desc_version={}",
            boot_params.memmap,
            boot_params.memmap_size,
            boot_params.desc_size,
            boot_params.desc_version
        );
        efi_info.map_phys = boot_params.memmap;
        efi_info.memory_size = boot_params.memmap_size as u64;
        efi_info.descriptor_size = boot_params.desc_size as u64;
        efi_info.descriptor_version = boot_params.desc_version;
        if boot_params.memmap != 0 && boot_params.memmap_size != 0 {
            let read_ok = try_read_efi_map(&mut efi_info)?;
            if !read_ok {
                let runtime_ok = try_read_efi_map_runtime(&mut efi_info)?;
                if !runtime_ok {
                    efi_info.memory_size = 0;
                    efi_info.descriptor_size = 0;
                } else {
                    efi_info.map_phys = 0;
                    efi_info.memory_size = 0;
                    efi_info.descriptor_size = 0;
                    log::debug!(
                        "efi-map: runtime-map fallback is incomplete; disabling EFI map to prefer SMAP"
                    );
                }
            }
        } else {
            log::debug!("efi-map: boot_params missing memmap values");
        }
        return Ok(efi_info);
    }

    // Fall back to /sys/firmware/efi/runtime-map if present
    let runtime_map = Path::new("/sys/firmware/efi/runtime-map");
    if !runtime_map.exists() {
        log::debug!("efi-map: runtime-map path missing");
        return Err(BootError::System("EFI runtime map not available".to_string()));
    }

    log::debug!("efi-map: boot_params missing, trying runtime-map");
    let read_ok = try_read_efi_map_runtime(&mut efi_info)?;
    if !read_ok {
        return Err(BootError::System(
            "EFI runtime map did not contain any entries".to_string(),
        ));
    }
    efi_info.memory_size = 0;
    efi_info.descriptor_size = 0;
    efi_info.map_phys = 0;
    log::debug!(
        "efi-map: runtime-map only; disabling EFI map to prefer SMAP"
    );

    Ok(efi_info)
}

/// Fetch EFI system table physical address from boot params (amd64)
pub fn fetch_efi_systab() -> Result<u64> {
    if let Some(boot_params) = read_boot_params_efi_info()? {
        log::debug!("efi-systab: boot_params systab=0x{:x}", boot_params.systab);
        return Ok(boot_params.systab);
    }
    log::debug!("efi-systab: boot_params missing, returning 0");
    Ok(0)
}

/// Fetch ACPI 2.0 tables (RSDP and RSDT)
pub fn fetch_acpi20(is_efi: bool) -> Result<(u64, u64)> {
    let mut rsdp = 0u64;
    let rsdt = 0u64;

    if is_efi {
        rsdp = match rsdp_from_efi_systab()? {
            Some(addr) => {
                log::debug!("acpi: rsdp from efi systab=0x{:x}", addr);
                addr
            }
            None => {
                log::debug!("acpi: rsdp not found in efi systab");
                0
            }
        };
    } else {
        log::debug!("acpi: legacy rsdp scan not available");
    }

    Ok((rsdp, rsdt))
}

/// Get commit limit and committed AS from /proc/meminfo
pub fn get_memory_stats() -> Result<(usize, usize)> {
    let mut commit_limit = 0;
    let mut committed_as = 0;

    if let Ok(file) = fs::File::open("/proc/meminfo") {
        let reader = BufReader::new(file);
        for line in reader.lines().flatten() {
            if line.starts_with("CommitLimit:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(limit) = parts[1].parse::<usize>() {
                        commit_limit = limit * 1024; // Convert from kB to bytes
                    }
                }
            } else if line.starts_with("Committed_AS:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(committed) = parts[1].parse::<usize>() {
                        committed_as = committed * 1024; // Convert from kB to bytes
                    }
                }
            }
        }
    }

    Ok((commit_limit, committed_as))
}

/// Get total available memory
pub fn get_total_memory() -> Result<usize> {
    if let Ok(file) = fs::File::open("/proc/meminfo") {
        let reader = BufReader::new(file);
        for line in reader.lines().flatten() {
            if line.starts_with("MemTotal:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(total) = parts[1].parse::<usize>() {
                        return Ok(total * 1024); // Convert from kB to bytes
                    }
                }
            }
        }
    }

    Err(BootError::System("Could not determine total memory".to_string()))
}

fn read_boot_params_efi_info() -> Result<Option<BootParamsEfiInfo>> {
    let path = Path::new(BOOT_PARAMS_PATH);
    if !path.exists() {
        log::debug!("boot-params: {} not found", BOOT_PARAMS_PATH);
        return Ok(None);
    }

    let data = fs::read(path).map_err(BootError::Io)?;
    log::debug!("boot-params: read {} bytes", data.len());
    if data.len() < BOOT_PARAMS_SIZE {
        log::debug!("boot-params: short read (need 0x{:x})", BOOT_PARAMS_SIZE);
        return Ok(None);
    }

    parse_boot_params_efi_info(&data)
}

fn parse_boot_params_efi_info(data: &[u8]) -> Result<Option<BootParamsEfiInfo>> {
    if data.len() < EFI_INFO_OFFSET + 0x20 {
        log::debug!("boot-params: EFI info offset out of range");
        return Ok(None);
    }

    let offset = EFI_INFO_OFFSET;
    let read_u32 = |off: usize| -> u32 {
        let bytes: [u8; 4] = data[off..off + 4].try_into().unwrap_or([0; 4]);
        u32::from_le_bytes(bytes)
    };

    let efi_systab = read_u32(offset + 0x04);
    let efi_memdesc_size = read_u32(offset + 0x08);
    let efi_memdesc_version = read_u32(offset + 0x0c);
    let efi_memmap = read_u32(offset + 0x10);
    let efi_memmap_size = read_u32(offset + 0x14);
    let efi_systab_hi = read_u32(offset + 0x18);
    let efi_memmap_hi = read_u32(offset + 0x1c);

    let systab = ((efi_systab_hi as u64) << 32) | efi_systab as u64;
    let memmap = ((efi_memmap_hi as u64) << 32) | efi_memmap as u64;

    if systab == 0 && memmap == 0 {
        log::debug!("boot-params: EFI info empty");
        return Ok(None);
    }

    log::debug!(
        "boot-params: systab=0x{:x} memmap=0x{:x} memmap_size=0x{:x} desc_size=0x{:x} desc_version={}",
        systab,
        memmap,
        efi_memmap_size,
        efi_memdesc_size,
        efi_memdesc_version
    );

    Ok(Some(BootParamsEfiInfo {
        systab,
        memmap,
        memmap_size: efi_memmap_size,
        desc_size: efi_memdesc_size,
        desc_version: efi_memdesc_version,
    }))
}

fn rsdp_from_efi_systab() -> Result<Option<u64>> {
    let path = Path::new("/sys/firmware/efi/systab");
    if !path.exists() {
        log::debug!("acpi: efi systab path missing");
        return Ok(None);
    }

    let data = read_file_string(path)?;
    for line in data.lines() {
        if let Some(value) = line.strip_prefix("ACPI20=") {
            return parse_hex_u64(value).map(Some);
        }
        if let Some(value) = line.strip_prefix("ACPI=") {
            return parse_hex_u64(value).map(Some);
        }
    }

    Ok(None)
}

fn parse_hex_u64(value: &str) -> Result<u64> {
    let trimmed = value.trim();
    let hex = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")).unwrap_or(trimmed);
    u64::from_str_radix(hex, 16).map_err(|err| BootError::System(format!("invalid hex: {err}")))
}

fn try_read_efi_map(efi_info: &mut EfiMapInfo) -> Result<bool> {
    let entry_size = efi_info.descriptor_size as usize;
    if entry_size == 0 || efi_info.memory_size == 0 {
        log::debug!("efi-map: skip read entry_size=0x{:x} size=0x{:x}", entry_size, efi_info.memory_size);
        return Ok(false);
    }

    let max_entries = efi_info.efi_table.len();
    let total_entries = (efi_info.memory_size as usize) / entry_size;
    let entries_to_read = max_entries.min(total_entries);

    let bytes_to_read = entry_size.saturating_mul(entries_to_read);
    if bytes_to_read == 0 {
        return Ok(false);
    }

    let mut buf = vec![0u8; bytes_to_read];
    let mut file = match fs::File::open("/dev/mem") {
        Ok(file) => file,
        Err(_) => {
            log::debug!("efi-map: /dev/mem open failed");
            let read_ok = try_read_efi_map_sysfs(efi_info)?;
            return Ok(read_ok);
        }
    };
    if let Err(_) = std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(efi_info.map_phys)) {
        log::debug!("efi-map: /dev/mem seek failed");
        let read_ok = try_read_efi_map_sysfs(efi_info)?;
        return Ok(read_ok);
    }
    if let Err(_) = std::io::Read::read_exact(&mut file, &mut buf) {
        log::debug!("efi-map: /dev/mem read failed");
        let read_ok = try_read_efi_map_sysfs(efi_info)?;
        return Ok(read_ok);
    }

    for i in 0..entries_to_read {
        let start = i * entry_size;
        if start + std::mem::size_of::<crate::types::EfiMapEntry>() > buf.len() {
            break;
        }
        let entry = parse_efi_map_entry(&buf[start..start + entry_size]);
        efi_info.efi_table[i] = entry;
    }
    log::debug!("efi-map: read {} entries", entries_to_read);

    Ok(entries_to_read > 0)
}

fn try_read_efi_map_sysfs(efi_info: &mut EfiMapInfo) -> Result<bool> {
    let entry_size = efi_info.descriptor_size as usize;
    if entry_size == 0 {
        log::debug!("efi-map: sysfs read skipped (entry_size=0)");
        return Ok(false);
    }

    let base = Path::new("/sys/firmware/efi/memmap");
    if !base.exists() {
        log::debug!("efi-map: sysfs path missing");
        return Ok(false);
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Ok(idx) = name.parse::<u32>() {
            entries.push((idx, entry.path()));
        }
    }
    entries.sort_by_key(|(idx, _)| *idx);
    log::debug!(
        "efi-map: runtime-map entries discovered={}",
        entries.len()
    );

    let mut count = 0usize;
    for (_, path) in entries {
        if count >= efi_info.efi_table.len() {
            break;
        }
        let raw_path = path.join("raw");
        let data = match fs::read(&raw_path) {
            Ok(data) => data,
            Err(_) => continue,
        };
        if data.len() < entry_size {
            continue;
        }
        let entry = parse_efi_map_entry(&data[..entry_size]);
        efi_info.efi_table[count] = entry;
        count += 1;
    }

    if count == 0 {
        return Ok(false);
    }

    efi_info.memory_size = (count as u64) * efi_info.descriptor_size;
    log::debug!("efi-map: sysfs read {} entries", count);
    Ok(true)
}

fn try_read_efi_map_runtime(efi_info: &mut EfiMapInfo) -> Result<bool> {
    let base = Path::new("/sys/firmware/efi/runtime-map");
    if !base.exists() {
        log::debug!("efi-map: runtime-map path missing");
        return Ok(false);
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Ok(idx) = name.parse::<u32>() {
            entries.push((idx, entry.path()));
        }
    }
    entries.sort_by_key(|(idx, _)| *idx);

    let mut count = 0usize;
    for (_, path) in entries {
        if count >= efi_info.efi_table.len() {
            break;
        }

        let attr = read_file_string(&path.join("attribute"))?;
        let pages = read_file_string(&path.join("num_pages"))?;
        let phys = read_file_string(&path.join("phys_addr"))?;
        let type_ = read_file_string(&path.join("type"))?;
        let virt = read_file_string(&path.join("virt_addr"))?;

        let entry_type = parse_u32_auto(&type_)?;
        let entry_phys = parse_u64_auto(&phys)?;
        let entry_pages = parse_u64_auto(&pages)?;
        let entry_attr = parse_u64_auto(&attr)?;
        let entry = crate::types::EfiMapEntry {
            type_: entry_type,
            pad: 0,
            phys: entry_phys,
            virt: parse_u64_auto(&virt)?,
            pages: entry_pages,
            attr: entry_attr,
        };

        if count < 4 {
            log::debug!(
                "efi-map: runtime-map[{}] type=0x{:x} phys=0x{:x} pages=0x{:x} attr=0x{:x}",
                count,
                entry_type,
                entry_phys,
                entry_pages,
                entry_attr
            );
        }
        efi_info.efi_table[count] = entry;
        count += 1;
    }

    if count == 0 {
        return Ok(false);
    }

    if efi_info.descriptor_size == 0 {
        efi_info.descriptor_size = std::mem::size_of::<crate::types::EfiMapEntry>() as u64;
    }
    if efi_info.descriptor_version == 0 {
        efi_info.descriptor_version = 1;
    }
    efi_info.memory_size = (count as u64) * efi_info.descriptor_size;
    log::debug!("efi-map: runtime-map read {} entries", count);
    Ok(true)
}

fn parse_efi_map_entry(data: &[u8]) -> crate::types::EfiMapEntry {
    let read_u32 = |off: usize| -> u32 {
        data.get(off..off + 4)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map(u32::from_le_bytes)
            .unwrap_or(0)
    };
    let read_u64 = |off: usize| -> u64 {
        data.get(off..off + 8)
            .and_then(|b| <[u8; 8]>::try_from(b).ok())
            .map(u64::from_le_bytes)
            .unwrap_or(0)
    };

    crate::types::EfiMapEntry {
        type_: read_u32(0),
        pad: read_u32(4),
        phys: read_u64(8),
        virt: read_u64(16),
        pages: read_u64(24),
        attr: read_u64(32),
    }
}

fn parse_u64_auto(value: &str) -> Result<u64> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16)
            .map_err(|err| BootError::System(format!("invalid hex: {err}")));
    }
    trimmed
        .parse::<u64>()
        .map_err(|err| BootError::System(format!("invalid int: {err}")))
}

fn parse_u32_auto(value: &str) -> Result<u32> {
    let parsed = parse_u64_auto(value)?;
    u32::try_from(parsed)
        .map_err(|_| BootError::System("value exceeds u32".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_boot_params_efi_info() {
        let mut data = vec![0u8; BOOT_PARAMS_SIZE];
        let offset = EFI_INFO_OFFSET;

        data[offset + 0x04..offset + 0x08].copy_from_slice(&0x11223344u32.to_le_bytes());
        data[offset + 0x08..offset + 0x0c].copy_from_slice(&48u32.to_le_bytes());
        data[offset + 0x0c..offset + 0x10].copy_from_slice(&1u32.to_le_bytes());
        data[offset + 0x10..offset + 0x14].copy_from_slice(&0x55667788u32.to_le_bytes());
        data[offset + 0x14..offset + 0x18].copy_from_slice(&0x99u32.to_le_bytes());
        data[offset + 0x18..offset + 0x1c].copy_from_slice(&0x1u32.to_le_bytes());
        data[offset + 0x1c..offset + 0x20].copy_from_slice(&0x2u32.to_le_bytes());

        let info = parse_boot_params_efi_info(&data).unwrap().unwrap();
        assert_eq!(info.systab, 0x1_11223344);
        assert_eq!(info.memmap, 0x2_55667788);
        assert_eq!(info.memmap_size, 0x99);
        assert_eq!(info.desc_size, 48);
        assert_eq!(info.desc_version, 1);
    }
}
