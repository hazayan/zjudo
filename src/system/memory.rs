use crate::error::{BootError, Result};
use std::fs;
use std::io::{BufRead, BufReader};

const SYSTEM_RAM_DESC: &str = "System RAM";
const RESERVED_DESC: &str = "reserved";
const RESERVED_DESC_CAP: &str = "Reserved";

const KERNEL_CODE_DESC: &str = "Kernel code";
const KERNEL_DATA_DESC: &str = "Kernel data";
const KERNEL_BSS_DESC: &str = "Kernel bss";

#[derive(Debug, Clone)]
struct IomemEntry {
    start: u64,
    end: u64,
    desc: String,
    indent: usize,
}

fn parse_iomem_line(line: &str) -> Option<IomemEntry> {
    let indent = line.chars().take_while(|c| c.is_whitespace()).count();
    let trimmed = line.trim();
    let (range, desc) = trimmed.split_once(':')?;
    let (start_str, end_str) = range.split_once('-')?;
    let start = u64::from_str_radix(start_str.trim(), 16).ok()?;
    let end = u64::from_str_radix(end_str.trim(), 16).ok()?;
    let desc = desc.trim().to_string();

    Some(IomemEntry {
        start,
        end,
        desc,
        indent,
    })
}

fn parse_iomem_lines<I>(lines: I) -> Vec<IomemEntry>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut entries = Vec::new();
    for line in lines {
        if let Some(entry) = parse_iomem_line(line.as_ref()) {
            entries.push(entry);
        }
    }
    entries
}

/// Memory region
#[derive(Debug, Clone, Copy)]
pub struct MemoryRegion {
    pub start: u64,
    pub end: u64,
    pub size: u64,
    pub region_type: u32,
}

impl MemoryRegion {
    pub fn new(start: u64, end: u64, region_type: u32) -> Self {
        Self {
            start,
            end,
            size: end - start + 1,
            region_type,
        }
    }

    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.start && addr <= self.end
    }

    pub fn overlaps(&self, other: &MemoryRegion) -> bool {
        !(self.end < other.start || other.end < self.start)
    }
}

fn subtract_ranges(available: &mut Vec<MemoryRegion>, reserved: &[MemoryRegion]) {
    for res in reserved {
        let mut next = Vec::new();
        for region in available.drain(..) {
            if !region.overlaps(res) {
                next.push(region);
                continue;
            }

            if res.start > region.start {
                let left_end = res.start - 1;
                if left_end >= region.start {
                    next.push(MemoryRegion::new(region.start, left_end, region.region_type));
                }
            }

            if res.end < region.end {
                let right_start = res.end + 1;
                if right_start <= region.end {
                    next.push(MemoryRegion::new(right_start, region.end, region.region_type));
                }
            }
        }
        *available = next;
    }
}

fn is_keeper_desc(desc: &str) -> bool {
    matches!(desc, KERNEL_CODE_DESC | KERNEL_DATA_DESC | KERNEL_BSS_DESC)
}

fn build_available_regions(entries: &[IomemEntry]) -> Vec<MemoryRegion> {
    let mut system_ram = Vec::new();
    let mut reserved = Vec::new();
    let mut current_top: Option<&IomemEntry> = None;

    for entry in entries {
        if entry.indent == 0 {
            current_top = Some(entry);
            match entry.desc.as_str() {
                SYSTEM_RAM_DESC => system_ram.push(MemoryRegion::new(entry.start, entry.end, 1)),
                RESERVED_DESC | RESERVED_DESC_CAP => {
                    reserved.push(MemoryRegion::new(entry.start, entry.end, 2))
                }
                _ => {}
            }
            continue;
        }

        if let Some(top) = current_top {
            if top.desc == SYSTEM_RAM_DESC && !is_keeper_desc(&entry.desc) {
                reserved.push(MemoryRegion::new(entry.start, entry.end, 2));
            }
        }
    }

    subtract_ranges(&mut system_ram, &reserved);
    system_ram
}

/// Get available RAM regions from /proc/iomem
pub fn get_available_ram_regions() -> Result<Vec<MemoryRegion>> {
    let file = fs::File::open("/proc/iomem").map_err(BootError::Io)?;
    let reader = BufReader::new(file);
    let entries = parse_iomem_lines(reader.lines().flatten());
    Ok(build_available_regions(&entries))
}

/// Find the first available region aligned to `alignment` with at least `min_size`
pub fn find_first_available_region(alignment: u64, min_size: u64) -> Result<Option<u64>> {
    let regions = get_available_ram_regions()?;
    for region in regions {
        let aligned_start = align_up(region.start, alignment);
        if aligned_start > region.end {
            continue;
        }
        let size = region.end - aligned_start + 1;
        if size >= min_size {
            return Ok(Some(aligned_start));
        }
    }
    Ok(None)
}

/// Check if a range is fully contained in available RAM regions
pub fn is_range_available(start: u64, size: u64, regions: &[MemoryRegion]) -> bool {
    if size == 0 {
        return false;
    }
    let end = match start.checked_add(size - 1) {
        Some(end) => end,
        None => return false,
    };
    regions.iter().any(|r| start >= r.start && end <= r.end)
}

/// Find free memory regions for kexec loading
pub fn find_free_memory_regions(
    required_size: u64,
    alignment: u64,
    avoid_regions: &[MemoryRegion],
) -> Result<Vec<MemoryRegion>> {
    let mut free_regions = Vec::new();
    
    // Get available RAM regions from /proc/iomem
    let system_ram = get_available_ram_regions()?;
    
    // For each System RAM region, find free sub-regions
    for ram_region in system_ram {
        // Start with the entire region as potentially free
        let mut free_start = ram_region.start;
        
        // Subtract any regions we need to avoid (kernel, reserved, etc.)
        for avoid in avoid_regions {
            if avoid.overlaps(&ram_region) {
                // If avoid region starts within our RAM region
                if avoid.start >= ram_region.start && avoid.start <= ram_region.end {
                    // Check if there's free space before the avoid region
                    if avoid.start > free_start {
                        let free_end = avoid.start - 1;
                        let free_size = free_end - free_start + 1;
                        if free_size >= required_size {
                            free_regions.push(MemoryRegion::new(free_start, free_end, 1));
                        }
                    }
                    // Move free_start past the avoid region
                    free_start = avoid.end + 1;
                }
            }
        }
        
        // Check if there's free space at the end
        if free_start <= ram_region.end {
            let free_end = ram_region.end;
            let free_size = free_end - free_start + 1;
            if free_size >= required_size {
                free_regions.push(MemoryRegion::new(free_start, free_end, 1));
            }
        }
    }
    
    // Apply alignment
    for region in &mut free_regions {
        // Align start up to alignment boundary
        let aligned_start = (region.start + alignment - 1) / alignment * alignment;
        if aligned_start > region.end {
            region.size = 0;
        } else {
            let aligned_size = region.end - aligned_start + 1;
            if aligned_size < required_size {
                region.size = 0;
            } else {
                region.start = aligned_start;
                region.size = aligned_size;
            }
        }
    }
    
    // Remove regions that are now too small
    free_regions.retain(|r| r.size >= required_size);
    
    // Sort by start address
    free_regions.sort_by_key(|r| r.start);
    
    Ok(free_regions)
}

/// Get all memory regions from /proc/iomem
pub fn get_memory_regions() -> Result<Vec<MemoryRegion>> {
    let mut regions = Vec::new();
    
    if let Ok(file) = fs::File::open("/proc/iomem") {
        let reader = BufReader::new(file);
        for line in reader.lines().flatten() {
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
                let region_type = match desc {
                    "System RAM" => 1,
                    "reserved" | "Reserved" => 2,
                    "ACPI Tables" => 3,
                    "ACPI Non-volatile Storage" => 4,
                    "PCI Bus 0000:00" => 5, // PCI MMIO
                    _ => 0,
                };

                if region_type != 0 {
                    regions.push(MemoryRegion::new(start, end, region_type));
                }
            }
        }
    }
    
    Ok(regions)
}

/// Get kernel reserved regions (where the current kernel is loaded)
pub fn get_kernel_reserved_regions() -> Result<Vec<MemoryRegion>> {
    let mut regions = Vec::new();
    
    // Typical kernel load addresses (simplified)
    // Kernel text/data: 0x1000000 (16MB) to 0x... (varies)
    // Actually, modern kernels load at 1MB+
    
    // For now, reserve first 16MB as a conservative estimate
    regions.push(MemoryRegion::new(0, 0xffffff, 2)); // 0-16MB
    
    // Also reserve where we think the kernel modules might be
    // This is a simplification - we should parse /proc/kallsyms or similar
    
    Ok(regions)
}

/// Round up to alignment
pub fn align_up(value: u64, alignment: u64) -> u64 {
    if alignment == 0 {
        return value;
    }
    (value + alignment - 1) / alignment * alignment
}

/// Round down to alignment
pub fn align_down(value: u64, alignment: u64) -> u64 {
    if alignment == 0 {
        return value;
    }
    value / alignment * alignment
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_region_new() {
        let region = MemoryRegion::new(0x1000, 0x2000, 1);
        assert_eq!(region.start, 0x1000);
        assert_eq!(region.end, 0x2000);
        assert_eq!(region.size, 0x1001); // end - start + 1
        assert_eq!(region.region_type, 1);
    }

    #[test]
    fn test_memory_region_contains() {
        let region = MemoryRegion::new(0x1000, 0x2000, 1);
        
        // Inside region
        assert!(region.contains(0x1000));
        assert!(region.contains(0x1500));
        assert!(region.contains(0x2000));
        
        // Outside region
        assert!(!region.contains(0x0fff));
        assert!(!region.contains(0x2001));
    }

    #[test]
    fn test_memory_region_overlaps() {
        let region1 = MemoryRegion::new(0x1000, 0x2000, 1);
        let region2 = MemoryRegion::new(0x1500, 0x2500, 1);
        let region3 = MemoryRegion::new(0x3000, 0x4000, 1);
        let region4 = MemoryRegion::new(0x0800, 0x1800, 1);
        
        // Overlapping regions
        assert!(region1.overlaps(&region2));
        assert!(region2.overlaps(&region1));
        assert!(region1.overlaps(&region4));
        
        // Non-overlapping regions
        assert!(!region1.overlaps(&region3));
        assert!(!region3.overlaps(&region1));
    }

    #[test]
    fn test_align_up() {
        assert_eq!(align_up(0, 4096), 0);
        assert_eq!(align_up(1, 4096), 4096);
        assert_eq!(align_up(4095, 4096), 4096);
        assert_eq!(align_up(4096, 4096), 4096);
        assert_eq!(align_up(4097, 4096), 8192);
        
        // Zero alignment
        assert_eq!(align_up(1234, 0), 1234);
        
        // Small alignment
        assert_eq!(align_up(7, 8), 8);
        assert_eq!(align_up(8, 8), 8);
        assert_eq!(align_up(9, 8), 16);
    }

    #[test]
    fn test_align_down() {
        assert_eq!(align_down(0, 4096), 0);
        assert_eq!(align_down(1, 4096), 0);
        assert_eq!(align_down(4095, 4096), 0);
        assert_eq!(align_down(4096, 4096), 4096);
        assert_eq!(align_down(4097, 4096), 4096);
        assert_eq!(align_down(8191, 4096), 4096);
        assert_eq!(align_down(8192, 4096), 8192);
        
        // Zero alignment
        assert_eq!(align_down(1234, 0), 1234);
        
        // Small alignment
        assert_eq!(align_down(7, 8), 0);
        assert_eq!(align_down(8, 8), 8);
        assert_eq!(align_down(9, 8), 8);
        assert_eq!(align_down(15, 8), 8);
        assert_eq!(align_down(16, 8), 16);
    }

    #[test]
    fn test_find_free_memory_regions_simple() {
        // Create some test regions
        let system_ram = MemoryRegion::new(0x100000, 0x200000, 1); // 1MB-2MB
        let reserved = MemoryRegion::new(0x150000, 0x180000, 2); // 1.5MB-1.8MB reserved
        
        // Test finding free region before reserved area
        let free_regions = find_free_memory_regions_simple(
            0x40000, // 256KB
            0x1000,  // 4KB alignment
            &[system_ram],
            &[reserved],
        ).unwrap();
        
        // Should find region from 0x100000 to 0x14ffff (320KB)
        assert!(!free_regions.is_empty());
    }

    // Helper function for testing
    fn find_free_memory_regions_simple(
        required_size: u64,
        alignment: u64,
        system_ram: &[MemoryRegion],
        avoid_regions: &[MemoryRegion],
    ) -> Result<Vec<MemoryRegion>> {
        let mut free_regions = Vec::new();
        
        for ram_region in system_ram {
            let mut free_start = ram_region.start;
            
            for avoid in avoid_regions {
                if avoid.overlaps(ram_region) {
                    if avoid.start >= ram_region.start && avoid.start <= ram_region.end {
                        if avoid.start > free_start {
                            let free_end = avoid.start - 1;
                            let free_size = free_end - free_start + 1;
                            if free_size >= required_size {
                                free_regions.push(MemoryRegion::new(free_start, free_end, 1));
                            }
                        }
                        free_start = avoid.end + 1;
                    }
                }
            }
            
            if free_start <= ram_region.end {
                let free_end = ram_region.end;
                let free_size = free_end - free_start + 1;
                if free_size >= required_size {
                    free_regions.push(MemoryRegion::new(free_start, free_end, 1));
                }
            }
        }
        
        // Apply alignment
        for region in &mut free_regions {
            let aligned_start = align_up(region.start, alignment);
            if aligned_start > region.end {
                region.size = 0;
            } else {
                let aligned_size = region.end - aligned_start + 1;
                if aligned_size < required_size {
                    region.size = 0;
                } else {
                    region.start = aligned_start;
                    region.size = aligned_size;
                }
            }
        }
        
        free_regions.retain(|r| r.size >= required_size);
        free_regions.sort_by_key(|r| r.start);
        
        Ok(free_regions)
    }

    #[test]
    fn test_build_available_regions() {
        let lines = vec![
            "00000000-00000fff : Reserved",
            "00001000-0009fbff : System RAM",
            "  00008000-00008fff : Kernel code",
            "0009fc00-0009ffff : reserved",
            "00100000-3fffffff : System RAM",
            "  00100000-001fffff : reserved",
            "  00200000-002fffff : Kernel data",
        ];

        let entries = parse_iomem_lines(&lines);
        let available = build_available_regions(&entries);

        assert!(available.iter().any(|r| r.start == 0x1000 && r.end >= 0x9fbf));
        assert!(available.iter().any(|r| r.start <= 0x200000 && r.end >= 0x2fffff));
        assert!(!available.iter().any(|r| r.start == 0x0));
        assert!(!available.iter().any(|r| r.start == 0x100000 && r.end == 0x1fffff));
    }
}
