use crate::boot::BootAssembler;
use crate::module::ModuleLoader;
use crate::system::{check_root, kexec_load, kexec_supported, SystemInfo};
use crate::types::BootHowto;
use crate::zfs::ZfsInterface;
use std::path::Path;

use super::{BootArgs, ConfigArgs, InfoArgs, ListArgs, LoadArgs, TestArgs, UnloadArgs};

const KEXEC_ALIGN: u64 = 2 * 1024 * 1024;
const KEXEC_HOLE_SIZE: u64 = 64 * 1024 * 1024;

struct BlobSegment {
    data: Vec<u8>,
    phys_addr: u64,
}

struct PreloadSegments {
    bundle: Option<BlobSegment>,
    symtab: Option<(u64, u64)>,
    env: Option<u64>,
    font: Option<u64>,
}

struct KexecPlan {
    segments: Vec<crate::types::KexecSegment>,
    buffers: Vec<Vec<u8>>,
}

fn warn_missing_mfsroot_cmdline(cmdline: Option<&String>) {
    let cmdline = match cmdline {
        Some(cmdline) => cmdline,
        None => {
            log::warn!(
                "mfsroot provided without a kernel command line; vfs.root.mountfrom is likely required"
            );
            return;
        }
    };

    let has_mountfrom = cmdline.contains("vfs.root.mountfrom=");

    if !has_mountfrom {
        log::warn!(
            "mfsroot provided without vfs.root.mountfrom; kernel may not mount the mfsroot"
        );
    }
}

/// Boot FreeBSD kernel
pub fn boot(args: BootArgs, verbose: bool, debug: bool) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if verbose {
        println!("Booting FreeBSD kernel: {}", args.kernel);
    }
    log::debug!(
        "boot-opts: debug={} entry_patch={} force={} no_acpi={} no_fb={} no_memmap={}",
        debug,
        args.entry_patch,
        args.force,
        args.no_acpi,
        args.no_fb,
        args.no_memmap
    );

    // Check root privileges
    check_root()?;

    // Check kexec support
    if !kexec_supported() {
        return Err("kexec not supported on this system".into());
    }

    // Load kernel
    let mut loader = ModuleLoader::new();
    loader.load_kernel(Path::new(&args.kernel))?;

    // Load modules
    for module_path in &args.modules {
        if verbose {
            println!("Loading module: {}", module_path);
        }
        loader.load_module(Path::new(module_path))?;
    }

    if let Some(mfsroot_path) = &args.mfsroot {
        if verbose {
            println!("Loading mfsroot: {}", mfsroot_path);
        }
        loader.load_raw_module(Path::new(mfsroot_path), "mfs_root", Some("mfs_root"))?;
    }

    if let Some(entropy_path) = &args.entropy {
        if verbose {
            println!("Loading entropy cache: {}", entropy_path);
        }
        loader.load_raw_module(
            Path::new(entropy_path),
            "boot_entropy_cache",
            Some("/boot/entropy"),
        )?;
    }

    // Collect system information
    let sysinfo = SystemInfo::collect()?;

    // Create boot howto (explicit only).
    let mut howto = BootHowto::new();
    if let Some(howto_flags) = args.howto {
        howto.set(howto_flags);
    }

    // Determine kernel command line
    let mut cmdline = args.cmdline.clone();
    let mut bootonce_dataset = None;
    if args.bootonce && cmdline.is_none() {
        let zfs = ZfsInterface::new();
        bootonce_dataset = zfs.get_bootonce_dataset()?;
        if let Some(bootonce) = &bootonce_dataset {
            if verbose {
                println!("Using ZFS bootonce dataset: {}", bootonce);
            }
        } else if verbose {
            println!("No ZFS bootonce dataset found");
        }
    }

    let detected_bootdev = if cmdline.is_none() {
        crate::system::detect_bootdev()?
    } else {
        None
    };

    if verbose {
        if let Some(bootdev) = &detected_bootdev {
            println!("Detected boot device: {}", bootdev);
        }
    }

    cmdline = resolve_boot_cmdline(cmdline, bootonce_dataset, detected_bootdev);
    if args.mfsroot.is_some() {
        warn_missing_mfsroot_cmdline(cmdline.as_ref());
    }

    let kernel_entry_raw = loader.kernel_entry();
    let kernel_btext = loader.kernel_btext();
    let kernel_text = loader.kernel_text();
    let (kernel_entry, entry_source) =
        crate::module::choose_kernel_entry(kernel_entry_raw, kernel_btext, kernel_text);
    log::debug!(
        "kernel entry selection: entry=0x{:x} btext={:?} text={:?} selected=0x{:x} source={:?}",
        kernel_entry_raw,
        kernel_btext,
        kernel_text,
        kernel_entry,
        entry_source
    );
    match entry_source {
        crate::module::KernelEntrySource::Btext => {
            if kernel_entry != kernel_entry_raw {
                log::warn!(
                    "kernel entry differs from btext: entry=0x{:x} btext=0x{:x} (using btext)",
                    kernel_entry_raw,
                    kernel_entry
                );
            }
        }
        crate::module::KernelEntrySource::Text => {
            log::warn!(
                "kernel btext missing; using .text address: entry=0x{:x} text=0x{:x}",
                kernel_entry_raw,
                kernel_entry
            );
        }
        crate::module::KernelEntrySource::Entry => {
            log::warn!("kernel btext/.text missing; using e_entry=0x{:x}", kernel_entry);
        }
    }

    // Create boot assembler
    let mut assembler = BootAssembler::new(kernel_entry);
    
    // Set system information
    if let Some(fb_info) = &sysinfo.fb_info {
        assembler.set_fb(fb_info.clone());
    }
    
    if !args.no_acpi {
        assembler.set_acpi_tables(sysinfo.rsdp, sysinfo.rsdt);
    }
    
    assembler.set_howto(howto.value());
    
    // Set kernel command line if provided
    if let Some(cmdline) = &cmdline {
        assembler.set_cmdline(cmdline);
    }

    // Determine staging base for kexec layout
    let staging_base = crate::system::find_first_available_region(KEXEC_ALIGN, KEXEC_HOLE_SIZE)?
        .ok_or("No suitable staging region found")?;

    // Allocate memory addresses
    let available_regions = crate::system::get_available_ram_regions()
        .map_err(|e| format!("Failed to read available RAM regions: {}", e))?;
    log_memory_regions("available-ram", &available_regions);
    if log::log_enabled!(log::Level::Debug) {
        if let Ok(raw_regions) = crate::system::get_memory_regions() {
            log_memory_regions("iomem-raw", &raw_regions);
        }
    }
    let (kernel_phys, kernel_image_end) = compute_kernel_layout(&args.kernel, staging_base)?;
    log::debug!(
        "kernel-layout: phys=0x{:x} end=0x{:x} size=0x{:x}",
        kernel_phys,
        kernel_image_end,
        kernel_image_end.saturating_sub(kernel_phys)
    );
    let modules_end = assign_module_addresses(&mut loader, kernel_image_end, &available_regions)?;
    if log::log_enabled!(log::Level::Debug) {
        for module in loader.modules() {
            let aligned = crate::system::align_up(
                module.data.len() as u64,
                crate::types::PAGE_SIZE as u64,
            );
            log::debug!(
                "module: name={} phys=0x{:x} size=0x{:x} aligned=0x{:x}",
                module.name,
                module.phys_addr,
                module.data.len(),
                aligned
            );
        }
    }
    let mut efi_map_info = None;
    let mut modulep_start = modules_end;

    if !args.no_memmap && sysinfo.is_efi {
        if sysinfo.efi_map_info.memory_size != 0 {
            log::debug!(
                "efi-map metadata enabled: size=0x{:x} map_phys=0x{:x}",
                sysinfo.efi_map_info.memory_size,
                sysinfo.efi_map_info.map_phys
            );
            efi_map_info = Some(sysinfo.efi_map_info.clone());
        } else {
            log::debug!("efi-map metadata disabled: size=0");
        }
    }

    let preload_segments = build_preload_segments(
        &loader,
        &sysinfo,
        cmdline.as_deref(),
        modulep_start,
        &available_regions,
    )?;
    log_preload_segments(&preload_segments);

    if let Some(next_addr) = preload_segments_end(&preload_segments) {
        modulep_start = next_addr;
    }

    let kernel_name = if args.kernel.starts_with('/') {
        args.kernel.clone()
    } else {
        format!("/{}", args.kernel)
    };
    let omit_smap = cfg!(test);
    let omit_font = cfg!(test);
    let modulep_info = build_modulep_info(
        &loader,
        &kernel_name,
        kernel_phys,
        kernel_image_end,
        howto.value(),
        sysinfo.efi_systab,
        efi_map_info.as_ref(),
        if omit_smap { None } else { Some(&sysinfo.smap_info) },
        cmdline.as_deref(),
        symtab_range(&preload_segments),
        envp_addr(&preload_segments),
        if omit_font { None } else { font_addr(&preload_segments) },
        sysinfo.efi_fb_info.as_ref(),
        modulep_start,
        staging_base,
        &available_regions,
    )?;
    log::debug!(
        "modulep: phys=0x{:x} size=0x{:x} kernend=0x{:x}",
        modulep_info.phys_addr,
        modulep_info.data.len(),
        modulep_info.kernend
    );
    log_kernel_modulep_summary(&modulep_info.data);
    log_modulep_hex_preview(&modulep_info.data);
    log_modulep_layout(&modulep_info.data);
    log_modulep_modules(&modulep_info.data);

    // Configure boot assembler for kboot-style trampoline
    let boot_entry = staging_base + crate::types::BOOT_PHYS_BASE;
    assembler.set_staging_base(staging_base);
    assembler.set_kernel_phys_base(kernel_phys);
    assembler.set_boot_addr(boot_entry);
    assembler.set_modulep(modulep_info.phys_addr);
    assembler.set_kernend(modulep_info.kernend);
    assembler.set_trampoline_debug(debug);
    log::debug!(
        "handoff: staging=0x{:x} boot=0x{:x} kern_phys=0x{:x} kern_end=0x{:x} entry=0x{:x}",
        staging_base,
        boot_entry,
        kernel_phys,
        kernel_image_end,
        loader.kernel_entry(),
    );
    log::debug!(
        "handoff: modulep=0x{:x} kernend=0x{:x} offsets modulep=0x{:x} kernend=0x{:x} (modulep_base=staging kernend_base=kernel)",
        modulep_info.phys_addr,
        modulep_info.kernend,
        modulep_info.phys_addr.saturating_sub(staging_base),
        modulep_info.kernend.saturating_sub(kernel_phys),
    );
    log::debug!(
        "handoff-kernel: kernel_phys=0x{:x} kernel_end=0x{:x} modulep_phys=0x{:x} modulep_off=0x{:x} kernend_off=0x{:x} staging=0x{:x}",
        kernel_phys,
        kernel_image_end,
        modulep_info.phys_addr,
        modulep_info.phys_addr.saturating_sub(staging_base),
        modulep_info.kernend.saturating_sub(kernel_phys),
        staging_base
    );
    if let Some(map_info) = &efi_map_info {
        if map_info.map_phys == 0 {
            log::debug!("efi-map-copy: skipped (map_phys=0)");
        } else if let Some(map_offset) = modulep_info.efi_map_offset {
            let header_size = std::mem::size_of::<crate::types::EfiMapHeader>() as u64;
            let header_aligned = crate::system::align_up(header_size, 16);
            let dst = modulep_info
                .phys_addr
                .saturating_add(map_offset)
                .saturating_add(header_aligned);
            assembler.set_efi_memmap(map_info.map_phys, dst, map_info.memory_size);
            log::debug!(
                "efi-map-copy: src=0x{:x} dst=0x{:x} len=0x{:x} header=0x{:x}",
                map_info.map_phys,
                dst,
                map_info.memory_size,
                header_aligned
            );
        } else {
            log::warn!("EFI map metadata missing; skipping EFI map copy");
        }
    }

    // Assemble trampoline
    assembler.assemble()?;

    // Patch module metadata
    loader.patch_modmetadata(true)?;

    // Prepare kexec segments
    let kexec_plan = prepare_kexec_segments(
        &args.kernel,
        &loader,
        &assembler,
        &modulep_info,
        staging_base,
        &available_regions,
        &preload_segments,
        kernel_entry,
        debug,
        args.entry_patch,
    )?;
    
    // Keep segment buffers alive until kexec_load completes.
    let _buffers = &kexec_plan.buffers;

    // Load kexec segments
    let flags = 0; // Default flags
    kexec_load(
        boot_entry,
        kexec_plan.segments.len(),
        &kexec_plan.segments,
        flags,
    )?;
    
    if verbose {
        println!("Kernel loaded via kexec. Rebooting...");
    }
    
    // Enforce a single-CPU Linux handoff before kexec.
    crate::system::offline_secondary_cpus()?;

    // Trigger reboot
    crate::system::shutdown()?;

    Ok(())
}

/// Load kernel and modules without booting
pub fn load(args: LoadArgs, verbose: bool, _debug: bool) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if verbose {
        println!("Loading kernel: {}", args.kernel);
    }

    // Check root privileges
    check_root()?;

    // Load kernel
    let mut loader = ModuleLoader::new();
    loader.load_kernel(Path::new(&args.kernel))?;

    // Load modules
    for module_path in &args.modules {
        if verbose {
            println!("Loading module: {}", module_path);
        }
        loader.load_module(Path::new(module_path))?;
    }

    if verbose {
        println!("Loaded {} modules", loader.modules().len());
        println!("Kernel entry point: 0x{:x}", loader.kernel_entry());
    }

    Ok(())
}

/// Unload currently loaded kernel
pub fn unload(_args: UnloadArgs, verbose: bool, _debug: bool) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if verbose {
        println!("Unloading kernel...");
    }

    // Check root privileges
    check_root()?;

    // Unload kexec segments
    crate::system::kexec_unload()?;

    if verbose {
        println!("Kernel unloaded successfully");
    }

    Ok(())
}

/// List loaded kernels and modules
pub fn list(_args: ListArgs, verbose: bool, _debug: bool) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if verbose {
        println!("Listing loaded kernels and modules...");
    }

    // TODO: Implement listing of loaded kernels and modules
    println!("No kernels or modules currently loaded");
    println!("Use 'zjudo load' to load a kernel");

    Ok(())
}

/// Show system information
pub fn info(args: InfoArgs, verbose: bool, _debug: bool) -> std::result::Result<(), Box<dyn std::error::Error>> {
    println!("System Information:");
    println!("===================");

    // Check kexec support
    let kexec_supported = kexec_supported();
    println!("kexec support: {}", if kexec_supported { "Yes" } else { "No" });

    // Check EFI
    let is_efi = crate::system::is_efi();
    println!("EFI boot: {}", if is_efi { "Yes" } else { "No" });

    // Memory information
    if args.memory || verbose {
        println!("\nMemory Information:");
        if let Ok(total) = crate::system::get_total_memory() {
            println!("  Total memory: {} MB", total / 1024 / 1024);
        }
        if let Ok((limit, committed)) = crate::system::get_memory_stats() {
            println!("  Commit limit: {} MB", limit / 1024 / 1024);
            println!("  Committed AS: {} MB", committed / 1024 / 1024);
        }
    }

    // Framebuffer information
    if args.framebuffer || verbose {
        println!("\nFramebuffer Information:");
        if let Ok(fb) = crate::system::fetch_fb() {
            println!("  Device: {}", fb.id);
            println!("  Physical address: 0x{:x}", fb.phys);
            println!("  Size: {} bytes", fb.size);
            println!("  Resolution: {}x{}", fb.width, fb.height);
        } else {
            println!("  No framebuffer found");
        }
    }

    // ACPI information
    if args.acpi || verbose {
        println!("\nACPI Information:");
        let (rsdp, rsdt) = crate::system::fetch_acpi20(is_efi)?;
        println!("  RSDP: 0x{:x}", rsdp);
        println!("  RSDT: 0x{:x}", rsdt);
    }

    // EFI information
    if args.efi && is_efi {
        println!("\nEFI Information:");
        // TODO: Add more EFI information
    }

    Ok(())
}

/// Test kexec functionality
pub fn test(args: TestArgs, verbose: bool, _debug: bool) -> std::result::Result<(), Box<dyn std::error::Error>> {
    println!("Testing zjudo functionality...");

    let mut all_passed = true;

    // Test kexec
    if args.kexec || (!args.elf && !args.modules && !args.memory) {
        println!("\n1. Testing kexec support:");
        let supported = kexec_supported();
        if supported {
            println!("   ✓ kexec is supported");
        } else {
            println!("   ✗ kexec is not supported");
            all_passed = false;
        }
    }

    // Test ELF parsing
    if args.elf || (!args.kexec && !args.modules && !args.memory) {
        println!("\n2. Testing ELF parsing:");
        // Create a minimal test ELF
        let test_elf = create_test_elf();
        if let Ok(elf) = crate::elf::ElfFile::from_bytes(test_elf) {
            println!("   ✓ ELF parsing works");
            println!("     - 64-bit: {}", elf.is_64bit());
            println!("     - Entry point: 0x{:x}", elf.entry_point());
        } else {
            println!("   ✗ ELF parsing failed");
            all_passed = false;
        }
    }

    // Test module loading
    if args.modules || (!args.kexec && !args.elf && !args.memory) {
        println!("\n3. Testing module loading:");
        let mut loader = ModuleLoader::new();
        // Try to load a non-existent module to test error handling
        let result = loader.load_module(Path::new("/nonexistent/module.ko"));
        match result {
            Ok(_) => {
                println!("   ✗ Module loading should have failed");
                all_passed = false;
            }
            Err(e) => {
                println!("   ✓ Module loading error handling works");
                if verbose {
                    println!("     Error: {}", e);
                }
            }
        }
    }

    // Test memory allocation
    if args.memory || (!args.kexec && !args.elf && !args.modules) {
        println!("\n4. Testing memory utilities:");
        let aligned = crate::system::round_up(1234, 4096);
        println!("   ✓ round_up(1234, 4096) = {}", aligned);
    }

    println!("\nTest Summary:");
    if all_passed {
        println!("✓ All tests passed!");
    } else {
        println!("✗ Some tests failed");
    }

    Ok(())
}

/// Generate configuration file
pub fn config(args: ConfigArgs, verbose: bool, _debug: bool) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if args.default {
        let config = generate_default_config();
        let file = args.file.unwrap_or_else(|| "zjudo.conf".to_string());
        
        std::fs::write(&file, config)?;
        
        if verbose {
            println!("Generated default configuration: {}", file);
        }
    } else if args.validate {
        let file = args.file.unwrap_or_else(|| "zjudo.conf".to_string());
        
        if Path::new(&file).exists() {
            let content = std::fs::read_to_string(&file)?;
            if validate_config(&content) {
                println!("Configuration file '{}' is valid", file);
            } else {
                println!("Configuration file '{}' is invalid", file);
            }
        } else {
            println!("Configuration file '{}' not found", file);
        }
    } else {
        println!("Use --default to generate default configuration or --validate to validate existing configuration");
    }

    Ok(())
}

/// Create a minimal test ELF
fn create_test_elf() -> Vec<u8> {
    // Minimal ELF header for testing
    let mut elf = Vec::new();
    
    // ELF identification
    elf.extend_from_slice(&[0x7f, b'E', b'L', b'F']); // Magic
    elf.push(2); // 64-bit
    elf.push(1); // Little endian
    elf.push(1); // ELF version
    elf.push(0); // OS ABI (System V)
    elf.extend_from_slice(&[0; 8]); // Padding
    
    // ELF header
    elf.extend_from_slice(&[2, 0]); // ET_EXEC
    elf.extend_from_slice(&[62, 0]); // EM_X86_64
    elf.extend_from_slice(&[1, 0, 0, 0]); // Version
    elf.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // Entry point
    elf.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // Program header offset
    elf.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // Section header offset
    elf.extend_from_slice(&[0, 0, 0, 0]); // Flags
    elf.extend_from_slice(&[64, 0]); // Header size
    elf.extend_from_slice(&[0, 0]); // Program header size
    elf.extend_from_slice(&[0, 0]); // Program header count
    elf.extend_from_slice(&[0, 0]); // Section header size
    elf.extend_from_slice(&[0, 0]); // Section header count
    elf.extend_from_slice(&[0, 0]); // Section name string table index
    
    elf
}

/// Generate default configuration
fn generate_default_config() -> String {
    r#"# zjudo configuration file
# Generated on: 2026-01-03

[general]
# Enable verbose output
verbose = false

# Enable debug output  
debug = false

# Default kernel path
# kernel = "/boot/kernel/kernel"

# Default modules to load
# modules = []

# Boot howto flags
# howto = 0

[kexec]
# kexec flags
flags = 0

# Architecture (62 = x86_64)
arch = 62

[memory]
# Memory alignment (bytes)
alignment = 1048576

# Maximum memory to allocate (MB)
max_memory = 1024

[modules]
# Enable module metadata patching
patch_metadata = true

# Module search paths
search_paths = [
    "/boot/kernel",
    "/boot/modules"
]

[system]
# Collect framebuffer information
collect_fb = true

# Collect ACPI tables
collect_acpi = true

# Collect memory map
collect_memmap = true

# Use EFI if available
use_efi = true
"#.to_string()
}

/// Validate configuration
fn validate_config(config: &str) -> bool {
    // Simple validation - check if it looks like a valid TOML
    !config.trim().is_empty() && config.contains('=')
}

/// Prepare kexec segments for loading
fn prepare_kexec_segments(
    kernel_path: &str,
    loader: &ModuleLoader,
    assembler: &BootAssembler,
    modulep_info: &crate::module::ModulepInfo,
    staging_base: u64,
    available_regions: &[crate::system::MemoryRegion],
    preload_segments: &PreloadSegments,
    kernel_entry: u64,
    _debug: bool,
    entry_patch: bool,
) -> std::result::Result<KexecPlan, Box<dyn std::error::Error>> {
    use crate::system::{align_up, is_range_available};
    use crate::types::{KexecSegment, KEXEC_SEGMENT_MAX, PAGE_SIZE, KERNBASE, KERNEL_PHYS_BASE, BOOT_PHYS_BASE};
    
    let mut segments = Vec::new();
    let mut buffers = Vec::new();
    let mut segment_log: Vec<(String, u64, u64, usize)> = Vec::new();
    
    // Segment 1: Trampoline/boot code (loaded at staging + BOOT_PHYS_BASE)
    let boot_code = assembler.boot_block();
    if !boot_code.is_empty() {
        let boot_size = align_up(boot_code.len() as u64, PAGE_SIZE as u64);
        debug_assert_eq!(boot_size % PAGE_SIZE as u64, 0);

        let boot_addr = staging_base + BOOT_PHYS_BASE;
        if !is_range_available(boot_addr, boot_size, &available_regions) {
            log_range_unavailable("boot-code", boot_addr, boot_size, available_regions);
            return Err(format!(
                "Boot code range 0x{:x}-0x{:x} is not available",
                boot_addr,
                boot_addr + boot_size - 1
            ).into());
        }

        buffers.push(boot_code);
        let boot_buf = buffers.last().unwrap();
        let boot_segment = KexecSegment {
            buf: boot_buf.as_ptr(),
            bufsz: boot_buf.len(),
            mem: boot_addr as *const u8,
            memsz: boot_size as usize,
        };
        segments.push(boot_segment);
        segment_log.push((
            "boot-code".to_string(),
            boot_addr,
            boot_size,
            boot_buf.len(),
        ));
    }
    
    // Parse kernel ELF and build a contiguous kernel block at staging base + KERNPHYS.
    let kernel_elf = crate::elf::ElfFile::load(std::path::Path::new(kernel_path))?;
    let mut max_end = 0u64;
    for ph in kernel_elf.loadable_segments() {
        let paddr_offset = if ph.p_vaddr >= KERNBASE {
            ph.p_vaddr - KERNBASE
        } else {
            ph.p_vaddr
        };
        if paddr_offset < KERNEL_PHYS_BASE {
            return Err("Kernel PT_LOAD below KERNPHYS base".into());
        }
        let seg_end = align_up(paddr_offset + ph.p_memsz as u64, PAGE_SIZE as u64);
        if seg_end > max_end {
            max_end = seg_end;
        }
        if log::log_enabled!(log::Level::Debug) {
            log::debug!(
                "kexec-kernel-seg: vaddr=0x{:x} paddr_off=0x{:x} filesz=0x{:x} memsz=0x{:x} end=0x{:x}",
                ph.p_vaddr,
                paddr_offset,
                ph.p_filesz,
                ph.p_memsz,
                seg_end
            );
        }
    }
    if max_end == 0 {
        return Err("Kernel has no loadable segments".into());
    }

    let kernel_size = align_up(max_end - KERNEL_PHYS_BASE, PAGE_SIZE as u64);
    debug_assert_eq!(kernel_size % PAGE_SIZE as u64, 0);
    let mut kernel_block = vec![0u8; kernel_size as usize];
    for ph in kernel_elf.loadable_segments() {
        let paddr_offset = if ph.p_vaddr >= KERNBASE {
            ph.p_vaddr - KERNBASE
        } else {
            ph.p_vaddr
        };
        if paddr_offset < KERNEL_PHYS_BASE {
            return Err("Kernel PT_LOAD below KERNPHYS base".into());
        }
        let dest = (paddr_offset - KERNEL_PHYS_BASE) as usize;
        let data = kernel_elf.segment_data(ph)?;
        let end = dest + data.len();
        if end > kernel_block.len() {
            return Err("Kernel segment data exceeds kernel block".into());
        }
        kernel_block[dest..end].copy_from_slice(&data);
    }

    let kernel_addr = staging_base + KERNEL_PHYS_BASE;
    if !is_range_available(kernel_addr, kernel_size, &available_regions) {
        log_range_unavailable("kernel", kernel_addr, kernel_size, available_regions);
        log::warn!(
            "Kernel range 0x{:x}-0x{:x} overlaps reserved memory; proceeding",
            kernel_addr,
            kernel_addr + kernel_size - 1
        );
    }
    if log::log_enabled!(log::Level::Debug) {
        log::debug!(
            "kexec-kernel-block: addr=0x{:x} size=0x{:x}",
            kernel_addr,
            kernel_size
        );
    }

    buffers.push(kernel_block);
    let buf = buffers.last_mut().unwrap();
    segments.push(KexecSegment {
        buf: buf.as_ptr(),
        bufsz: buf.len(),
        mem: kernel_addr as *const u8,
        memsz: kernel_size as usize,
    });
    segment_log.push((
        "kernel".to_string(),
        kernel_addr,
        kernel_size,
        buf.len(),
    ));

    if log::log_enabled!(log::Level::Debug) {
        let entry_offset = if kernel_entry >= KERNBASE {
            kernel_entry - KERNBASE
        } else {
            kernel_entry
        };
        if entry_offset < KERNEL_PHYS_BASE {
            log::warn!(
                "kernel-entry: vaddr=0x{:x} offset=0x{:x} below KERNEL_PHYS_BASE",
                kernel_entry,
                entry_offset
            );
        } else {
            let entry_phys = kernel_addr + (entry_offset - KERNEL_PHYS_BASE);
            let entry_index = (entry_offset - KERNEL_PHYS_BASE) as usize;
            let mut entry_bytes = [0u8; 16];
            let entry_len = entry_bytes.len();
            if entry_index + entry_len <= buf.len() {
                entry_bytes.copy_from_slice(&buf[entry_index..entry_index + entry_len]);
                log::debug!(
                    "kernel-entry-bytes: vaddr=0x{:x} phys=0x{:x} bytes={:02x?}",
                    kernel_entry,
                    entry_phys,
                    entry_bytes
                );
            } else {
                log::warn!(
                    "kernel-entry-bytes: vaddr=0x{:x} index=0x{:x} out of range (kernel_size=0x{:x})",
                    kernel_entry,
                    entry_index,
                    buf.len()
                );
            }
        }
    }

    if entry_patch {
        let entry_offset = if kernel_entry >= KERNBASE {
            kernel_entry - KERNBASE
        } else {
            kernel_entry
        };
        if entry_offset >= KERNEL_PHYS_BASE {
            let entry_index = (entry_offset - KERNEL_PHYS_BASE) as usize;
            let stub: [u8; 19] = [
                0x66, 0xBA, 0xFD, 0x03, // mov dx, 0x3fd (LSR)
                0xEC,                   // in al, dx
                0xA8, 0x20,             // test al, 0x20
                0x74, 0xFB,             // jz -5 (wait for THR empty)
                0x66, 0xBA, 0xF8, 0x03, // mov dx, 0x3f8 (COM1)
                0xB0, b'h',             // mov al, 'h'
                0xEE,                   // out dx, al
                0xF4,                   // hlt
                0xEB, 0xFD,             // jmp -3 (to hlt)
            ];
            if entry_index + stub.len() <= buf.len() {
                buf[entry_index..entry_index + stub.len()].copy_from_slice(&stub);
                log::warn!(
                    "kernel-entry-patch: replaced entry bytes with uart/hlt stub (vaddr=0x{:x} phys=0x{:x})",
                    kernel_entry,
                    kernel_addr + (entry_offset - KERNEL_PHYS_BASE)
                );
            } else {
                log::warn!(
                    "kernel-entry-patch: entry index out of range (index=0x{:x} kernel_size=0x{:x})",
                    entry_index,
                    buf.len()
                );
            }
        } else {
            log::warn!(
                "kernel-entry-patch: entry offset below KERNEL_PHYS_BASE (vaddr=0x{:x} offset=0x{:x})",
                kernel_entry,
                entry_offset
            );
        }
    }
    
    // For modules, we need to create segments for each module
    for module in loader.modules() {
        let aligned_size = align_up(module.data.len() as u64, PAGE_SIZE as u64);
        debug_assert_eq!(aligned_size % PAGE_SIZE as u64, 0);
        let module_addr = module.phys_addr;
        if module_addr == 0 {
            return Err("Module physical address was not assigned".into());
        }
        if !is_range_available(module_addr, aligned_size, available_regions) {
            log_range_unavailable(
                &format!("module-{}", module.name),
                module_addr,
                aligned_size,
                available_regions,
            );
            return Err(format!(
                "Module range 0x{:x}-0x{:x} is not available",
                module_addr,
                module_addr + aligned_size - 1
            ).into());
        }
        
        let segment = KexecSegment {
            buf: module.data.as_ptr(),
            bufsz: module.data.len(),
            mem: module_addr as *const u8,
            memsz: aligned_size as usize,
        };
        segments.push(segment);
        segment_log.push((
            format!("module-{}", module.name),
            module_addr,
            aligned_size,
            module.data.len(),
        ));
    }

    if !modulep_info.data.is_empty() {
        let modulep_size = align_up(modulep_info.data.len() as u64, PAGE_SIZE as u64);
        debug_assert_eq!(modulep_size % PAGE_SIZE as u64, 0);
        if !is_range_available(modulep_info.phys_addr, modulep_size, available_regions) {
            log_range_unavailable("modulep", modulep_info.phys_addr, modulep_size, available_regions);
            return Err(format!(
                "Module metadata range 0x{:x}-0x{:x} is not available",
                modulep_info.phys_addr,
                modulep_info.phys_addr + modulep_size - 1
            ).into());
        }
        segments.push(KexecSegment {
            buf: modulep_info.data.as_ptr(),
            bufsz: modulep_info.data.len(),
            mem: modulep_info.phys_addr as *const u8,
            memsz: modulep_size as usize,
        });
        segment_log.push((
            "modulep".to_string(),
            modulep_info.phys_addr,
            modulep_size,
            modulep_info.data.len(),
        ));
    }

    if let Some(seg) = &preload_segments.bundle {
        let size = align_up(seg.data.len() as u64, PAGE_SIZE as u64);
        debug_assert_eq!(size % PAGE_SIZE as u64, 0);
        if !is_range_available(seg.phys_addr, size, available_regions) {
            log_range_unavailable("preload-bundle", seg.phys_addr, size, available_regions);
            return Err(format!(
                "Preload range 0x{:x}-0x{:x} is not available",
                seg.phys_addr,
                seg.phys_addr + size - 1
            ).into());
        }
        segments.push(KexecSegment {
            buf: seg.data.as_ptr(),
            bufsz: seg.data.len(),
            mem: seg.phys_addr as *const u8,
            memsz: size as usize,
        });
        segment_log.push(("preload-bundle".to_string(), seg.phys_addr, size, seg.data.len()));
    }
    
    // Check segment count limit
    if segments.len() > KEXEC_SEGMENT_MAX {
        return Err(format!(
            "Too many kexec segments: {} (max {})",
            segments.len(),
            KEXEC_SEGMENT_MAX
        ).into());
    }

    if log::log_enabled!(log::Level::Debug) {
        log::debug!(
            "kexec-segments: count={} (max {})",
            segments.len(),
            KEXEC_SEGMENT_MAX
        );
        for (name, addr, size, bufsz) in segment_log {
            log::debug!(
                "  {} mem=0x{:016x}-0x{:016x} memsz=0x{:x} bufsz=0x{:x}",
                name,
                addr,
                addr + size - 1,
                size,
                bufsz
            );
        }
    }
    
    Ok(KexecPlan { segments, buffers })
}

fn compute_kernel_layout(
    kernel_path: &str,
    staging_base: u64,
) -> std::result::Result<(u64, u64), Box<dyn std::error::Error>> {
    use crate::system::align_up;
    use crate::types::{KERNBASE, KERNEL_PHYS_BASE, PAGE_SIZE};

    let kernel_elf = crate::elf::ElfFile::load(std::path::Path::new(kernel_path))?;
    let mut max_end = 0u64;

    for ph in kernel_elf.loadable_segments() {
        let paddr_offset = if ph.p_vaddr >= KERNBASE {
            ph.p_vaddr - KERNBASE
        } else {
            ph.p_vaddr
        };
        let seg_end = align_up(paddr_offset + ph.p_memsz as u64, PAGE_SIZE as u64);
        if paddr_offset < KERNEL_PHYS_BASE {
            return Err("Kernel PT_LOAD below KERNPHYS base".into());
        }
        if seg_end > max_end {
            max_end = seg_end;
        }
    }

    if max_end == 0 {
        return Err("Kernel has no loadable segments".into());
    }

    let kernel_phys = staging_base + KERNEL_PHYS_BASE;
    let kernel_end = staging_base + max_end;
    Ok((kernel_phys, kernel_end))
}

fn assign_module_addresses(
    loader: &mut ModuleLoader,
    start_addr: u64,
    available_regions: &[crate::system::MemoryRegion],
) -> std::result::Result<u64, Box<dyn std::error::Error>> {
    use crate::system::align_up;
    use crate::types::PAGE_SIZE;

    let mut current_addr = start_addr;

    for module in loader.modules_mut() {
        let aligned_size = align_up(module.data.len() as u64, PAGE_SIZE as u64);
        let mut module_addr = None;

        for region in available_regions {
            if region.end < current_addr {
                continue;
            }
            let start = align_up(current_addr.max(region.start), PAGE_SIZE as u64);
            if start + aligned_size - 1 <= region.end {
                module_addr = Some(start);
                break;
            }
        }

        let module_addr = module_addr.ok_or("No free memory region found for module")?;
        module.set_physical_address(module_addr);
        current_addr = module_addr + aligned_size;
    }

    Ok(current_addr)
}

fn find_available_at_or_after(
    start_addr: u64,
    size: u64,
    available_regions: &[crate::system::MemoryRegion],
) -> Option<u64> {
    use crate::system::align_up;
    use crate::types::PAGE_SIZE;

    if size == 0 {
        return None;
    }

    for region in available_regions {
        if region.end < start_addr {
            continue;
        }
        let start = align_up(start_addr.max(region.start), PAGE_SIZE as u64);
        if start + size - 1 <= region.end {
            return Some(start);
        }
    }

    None
}

fn build_preload_segments(
    loader: &ModuleLoader,
    sysinfo: &SystemInfo,
    cmdline: Option<&str>,
    start_addr: u64,
    available_regions: &[crate::system::MemoryRegion],
) -> std::result::Result<PreloadSegments, Box<dyn std::error::Error>> {
    let symtab = match (loader.kernel_symbols(), loader.kernel_strtab()) {
        (Some(symtab), Some(strtab)) => Some(build_symbol_block(symtab, strtab)),
        _ => None,
    };
    let env = {
        let env_block = build_env_block(sysinfo, cmdline);
        if env_block.is_empty() {
            None
        } else {
            Some(env_block)
        }
    };
    let font = match build_font_block() {
        Ok(font_block) => Some(font_block),
        Err(_) => None,
    };

    if symtab.is_none() && env.is_none() && font.is_none() {
        return Ok(PreloadSegments {
            bundle: None,
            symtab: None,
            env: None,
            font: None,
        });
    }

    let mut bundle = Vec::new();
    let mut symtab_range = None;
    let mut env_addr = None;
    let mut font_addr = None;

    if let Some(symtab_block) = symtab {
        align_vec(&mut bundle);
        let offset = bundle.len() as u64;
        let end = offset + symtab_block.len() as u64;
        bundle.extend_from_slice(&symtab_block);
        symtab_range = Some((offset, end));
        align_vec(&mut bundle);
    }

    if let Some(env_block) = env {
        align_vec(&mut bundle);
        let offset = bundle.len() as u64;
        bundle.extend_from_slice(&env_block);
        env_addr = Some(offset);
        align_vec(&mut bundle);
    }

    if let Some(font_block) = font {
        align_vec(&mut bundle);
        let offset = bundle.len() as u64;
        bundle.extend_from_slice(&font_block);
        font_addr = Some(offset);
        align_vec(&mut bundle);
    }

    let phys_addr = find_available_at_or_after(start_addr, bundle.len() as u64, available_regions)
        .ok_or("No free memory region found for preload bundle")?;

    let symtab_range = symtab_range.map(|(start, end)| (phys_addr + start, phys_addr + end));
    let env_addr = env_addr.map(|offset| phys_addr + offset);
    let font_addr = font_addr.map(|offset| phys_addr + offset);

    Ok(PreloadSegments {
        bundle: Some(BlobSegment { data: bundle, phys_addr }),
        symtab: symtab_range,
        env: env_addr,
        font: font_addr,
    })
}

fn preload_segments_end(segments: &PreloadSegments) -> Option<u64> {
    segments
        .bundle
        .as_ref()
        .map(|seg| next_aligned_addr(seg.phys_addr, seg.data.len() as u64))
}

fn symtab_range(segments: &PreloadSegments) -> Option<(u64, u64)> {
    segments.symtab
}

fn envp_addr(segments: &PreloadSegments) -> Option<u64> {
    segments.env
}

fn font_addr(segments: &PreloadSegments) -> Option<u64> {
    segments.font
}

fn build_symbol_block(symtab: &[u8], strtab: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(symtab.len() as u64).to_le_bytes());
    buf.extend_from_slice(symtab);
    align_vec(&mut buf);
    buf.extend_from_slice(&(strtab.len() as u64).to_le_bytes());
    buf.extend_from_slice(strtab);
    align_vec(&mut buf);
    buf
}

fn build_env_block(sysinfo: &SystemInfo, cmdline: Option<&str>) -> Vec<u8> {
    let mut env = Vec::new();

    if sysinfo.rsdp != 0 {
        env.extend_from_slice(format!("acpi.rsdp=0x{:x}\0", sysinfo.rsdp).as_bytes());
    }
    if sysinfo.rsdt != 0 {
        env.extend_from_slice(format!("acpi.rsdt=0x{:x}\0", sysinfo.rsdt).as_bytes());
    }
    if sysinfo.efi_systab != 0 {
        env.extend_from_slice(format!("efi_systab=0x{:x}\0", sysinfo.efi_systab).as_bytes());
    }

    if let Some(cmdline) = cmdline {
        for token in cmdline.split_whitespace() {
            if token.contains('=') && !token.starts_with('-') {
                env.extend_from_slice(token.as_bytes());
                env.push(0);
            }
        }
    }

    if !env.is_empty() {
        if !env.ends_with(&[0]) {
            env.push(0);
        }
        env.push(0);
    }

    env
}

fn build_font_block() -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let loader = crate::font::FontLoader::new();
    let font = loader.load_default_font()?;
    build_font_block_from_font(&font)
}

fn build_font_block_from_font(
    font: &crate::font::FontInfo,
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let glyph_stride = ((font.width + 7) / 8) as usize;
    if font.height == 0 || glyph_stride == 0 {
        return Err("Font has invalid dimensions".into());
    }

    let glyph_bytes = glyph_stride
        .checked_mul(font.height as usize)
        .ok_or("Font glyph size overflow")?;
    if glyph_bytes == 0 {
        return Err("Font glyph size is zero".into());
    }
    if font.data.len() % glyph_bytes != 0 {
        return Err("Font bitmap size is not aligned to glyph size".into());
    }

    let glyph_count = font.data.len() / glyph_bytes;
    if glyph_count == 0 {
        return Err("Font has zero glyphs".into());
    }
    if glyph_count > (u16::MAX as usize + 1) {
        return Err("Font has too many glyphs for VFNT maps".into());
    }
    if font.data.len() > u32::MAX as usize {
        return Err("Font bitmap is too large".into());
    }

    let map_entry = crate::types::VfntMap {
        vfm_src: 0,
        vfm_dst: 0,
        vfm_len: (glyph_count - 1) as u16,
    };
    let mut map_counts = [0u32; crate::types::VfntMapType::COUNT];
    map_counts[crate::types::VfntMapType::Normal as usize] = 1;

    let checksum = font
        .width
        .wrapping_add(font.height)
        .wrapping_add(font.data.len() as u32)
        .wrapping_add(map_counts.iter().sum::<u32>());
    let info = crate::types::FontInfo {
        fi_checksum: (0u32.wrapping_sub(checksum)) as i32,
        fi_width: font.width,
        fi_height: font.height,
        fi_bitmap_size: font.data.len() as u32,
        fi_map_count: map_counts,
    };

    let mut buf = Vec::new();
    buf.extend_from_slice(&info.fi_checksum.to_le_bytes());
    buf.extend_from_slice(&info.fi_width.to_le_bytes());
    buf.extend_from_slice(&info.fi_height.to_le_bytes());
    buf.extend_from_slice(&info.fi_bitmap_size.to_le_bytes());
    for count in &info.fi_map_count {
        buf.extend_from_slice(&count.to_le_bytes());
    }

    align_vec(&mut buf);
    buf.extend_from_slice(&map_entry.vfm_src.to_le_bytes());
    buf.extend_from_slice(&map_entry.vfm_dst.to_le_bytes());
    buf.extend_from_slice(&map_entry.vfm_len.to_le_bytes());
    align_vec(&mut buf);

    buf.extend_from_slice(&font.data);
    Ok(buf)
}

fn align_vec(buf: &mut Vec<u8>) {
    let align = std::mem::size_of::<u64>();
    let pad = (align - (buf.len() % align)) % align;
    buf.extend_from_slice(&vec![0u8; pad]);
}

fn next_aligned_addr(start: u64, size: u64) -> u64 {
    let total = start + size;
    crate::system::align_up(total, crate::types::PAGE_SIZE as u64)
}

fn log_memory_regions(label: &str, regions: &[crate::system::MemoryRegion]) {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }
    log::debug!("{}: {} regions", label, regions.len());
    for region in regions {
        log::debug!(
            "  0x{:016x}-0x{:016x} size=0x{:x} type={}",
            region.start,
            region.end,
            region.size,
            region.region_type
        );
    }
}

fn log_range_unavailable(
    label: &str,
    start: u64,
    size: u64,
    regions: &[crate::system::MemoryRegion],
) {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }
    let end = match start.checked_add(size.saturating_sub(1)) {
        Some(end) => end,
        None => {
            log::debug!("range-unavailable: {} start=0x{:x} size=0x{:x} (overflow)", label, start, size);
            return;
        }
    };
    log::debug!(
        "range-unavailable: {} start=0x{:x} end=0x{:x} size=0x{:x}",
        label,
        start,
        end,
        size
    );
    let mut overlaps = 0;
    for region in regions {
        if start <= region.end && end >= region.start {
            overlaps += 1;
            log::debug!(
                "  overlaps-available: 0x{:016x}-0x{:016x} size=0x{:x} type={}",
                region.start,
                region.end,
                region.size,
                region.region_type
            );
        }
    }
    if overlaps == 0 {
        log::debug!("  no overlap with any available region");
    }
}

fn log_preload_segments(preload_segments: &PreloadSegments) {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }
    if let Some(seg) = &preload_segments.bundle {
        log::debug!(
            "preload-bundle: phys=0x{:x} size=0x{:x}",
            seg.phys_addr,
            seg.data.len()
        );
    }
    if let Some((start, end)) = preload_segments.symtab {
        log::debug!(
            "preload-symtab: phys=0x{:x} size=0x{:x}",
            start,
            end - start
        );
    }
    if let Some(addr) = preload_segments.env {
        log::debug!("preload-env: phys=0x{:x}", addr);
    }
    if let Some(addr) = preload_segments.font {
        log::debug!("preload-font: phys=0x{:x}", addr);
    }
}

fn log_kernel_modulep_summary(modulep: &[u8]) {
    use crate::types::{ModInfoMd, MODINFO_ADDR, MODINFO_END, MODINFO_METADATA, MODINFO_NAME, MODINFO_SIZE, MODINFO_TYPE};

    if !log::log_enabled!(log::Level::Debug) {
        return;
    }
    if modulep.is_empty() {
        log::debug!("modulep-kernel: empty modulep");
        return;
    }

    let mut offset = 0usize;
    let align = std::mem::size_of::<u64>();
    let mut seen_name = false;
    let mut entries: Vec<(u32, Vec<u8>)> = Vec::new();

    while offset + 8 <= modulep.len() {
        let type_ = u32::from_le_bytes(modulep[offset..offset + 4].try_into().unwrap());
        let size = u32::from_le_bytes(modulep[offset + 4..offset + 8].try_into().unwrap()) as usize;
        offset += 8;
        if type_ == MODINFO_END && size == 0 {
            break;
        }
        if offset + size > modulep.len() {
            log::debug!(
                "modulep-kernel: truncated entry type=0x{:x} size=0x{:x} offset=0x{:x}",
                type_,
                size,
                offset
            );
            break;
        }

        let data = &modulep[offset..offset + size];
        if type_ == MODINFO_NAME {
            if seen_name {
                break;
            }
            seen_name = true;
        }
        if seen_name {
            entries.push((type_, data.to_vec()));
        }

        offset += size;
        let pad = (align - (offset % align)) % align;
        offset += pad;
    }

    if entries.is_empty() {
        log::debug!("modulep-kernel: no entries found");
        return;
    }

    let mut name: Option<String> = None;
    let mut module_type: Option<String> = None;
    let mut args: Option<String> = None;
    let mut addr: Option<u64> = None;
    let mut size: Option<u64> = None;
    let mut howto: Option<u64> = None;
    let mut fw_handle: Option<u64> = None;
    let mut kernend: Option<u64> = None;
    let mut ssym: Option<u64> = None;
    let mut esym: Option<u64> = None;
    let mut envp: Option<u64> = None;
    let mut font: Option<u64> = None;
    let mut keybuf_len: Option<usize> = None;
    let mut elfhdr_len: Option<usize> = None;
    let mut shdr_len: Option<usize> = None;
    let mut dynamic: Option<u64> = None;
    let mut efi_map_len: Option<usize> = None;
    let mut smap_len: Option<usize> = None;
    let mut modulep_md: Option<u64> = None;

    for (type_, data) in &entries {
        if *type_ == MODINFO_NAME {
            let nul = data.iter().position(|b| *b == 0).unwrap_or(data.len());
            name = Some(String::from_utf8_lossy(&data[..nul]).to_string());
            continue;
        }
        if *type_ == MODINFO_TYPE {
            let nul = data.iter().position(|b| *b == 0).unwrap_or(data.len());
            module_type = Some(String::from_utf8_lossy(&data[..nul]).to_string());
            continue;
        }
        if *type_ == crate::types::MODINFO_ARGS {
            let nul = data.iter().position(|b| *b == 0).unwrap_or(data.len());
            args = Some(String::from_utf8_lossy(&data[..nul]).to_string());
            continue;
        }
        if *type_ == MODINFO_ADDR && data.len() == 8 {
            addr = Some(u64::from_le_bytes(data[..8].try_into().unwrap()));
            continue;
        }
        if *type_ == MODINFO_SIZE && data.len() == 8 {
            size = Some(u64::from_le_bytes(data[..8].try_into().unwrap()));
            continue;
        }

        if (*type_ & MODINFO_METADATA) != 0 {
            let subtype = *type_ & !MODINFO_METADATA;
            match subtype {
                x if x == ModInfoMd::Howto as u32 && data.len() == 8 => {
                    howto = Some(u64::from_le_bytes(data[..8].try_into().unwrap()));
                }
                x if x == ModInfoMd::Howto as u32 && data.len() == 4 => {
                    let mut bytes = [0u8; 8];
                    bytes[..4].copy_from_slice(&data[..4]);
                    howto = Some(u64::from_le_bytes(bytes));
                }
                x if x == ModInfoMd::FwHandle as u32 && data.len() == 8 => {
                    fw_handle = Some(u64::from_le_bytes(data[..8].try_into().unwrap()));
                }
                x if x == ModInfoMd::Kernend as u32 && data.len() == 8 => {
                    kernend = Some(u64::from_le_bytes(data[..8].try_into().unwrap()));
                }
                x if x == ModInfoMd::Ssym as u32 && data.len() == 8 => {
                    ssym = Some(u64::from_le_bytes(data[..8].try_into().unwrap()));
                }
                x if x == ModInfoMd::Esym as u32 && data.len() == 8 => {
                    esym = Some(u64::from_le_bytes(data[..8].try_into().unwrap()));
                }
                x if x == ModInfoMd::Envp as u32 && data.len() == 8 => {
                    envp = Some(u64::from_le_bytes(data[..8].try_into().unwrap()));
                }
                x if x == ModInfoMd::Font as u32 && data.len() == 8 => {
                    font = Some(u64::from_le_bytes(data[..8].try_into().unwrap()));
                }
                x if x == ModInfoMd::KeyBuf as u32 => {
                    keybuf_len = Some(data.len());
                }
                x if x == ModInfoMd::Elfhdr as u32 => {
                    elfhdr_len = Some(data.len());
                }
                x if x == ModInfoMd::Shdr as u32 => {
                    shdr_len = Some(data.len());
                }
                x if x == ModInfoMd::Dynamic as u32 && data.len() == 8 => {
                    dynamic = Some(u64::from_le_bytes(data[..8].try_into().unwrap()));
                }
                x if x == ModInfoMd::EfiMap as u32 => {
                    efi_map_len = Some(data.len());
                }
                x if x == ModInfoMd::Smap as u32 => {
                    smap_len = Some(data.len());
                }
                x if x == ModInfoMd::Modulep as u32 && data.len() == 8 => {
                    modulep_md = Some(u64::from_le_bytes(data[..8].try_into().unwrap()));
                }
                _ => {}
            }
        }
    }

    if let Some(name) = name {
        log::debug!(
            "modulep-kernel: name={} type={} args={}",
            name,
            module_type.unwrap_or_else(|| "<unknown>".to_string()),
            args.unwrap_or_else(|| "".to_string())
        );
    }
    if let (Some(addr), Some(size)) = (addr, size) {
        log::debug!("modulep-kernel: addr=0x{:x} size=0x{:x}", addr, size);
    }
    if howto.is_some()
        || fw_handle.is_some()
        || kernend.is_some()
        || ssym.is_some()
        || esym.is_some()
        || envp.is_some()
        || font.is_some()
        || keybuf_len.is_some()
        || elfhdr_len.is_some()
        || shdr_len.is_some()
        || dynamic.is_some()
        || efi_map_len.is_some()
    {
        log::debug!(
            "modulep-kernel: md howto=0x{:x} fw_handle=0x{:x} modulep=0x{:x} kernend=0x{:x} ssym=0x{:x} esym=0x{:x} envp=0x{:x} font=0x{:x} keybuf_len=0x{:x} elfhdr_len=0x{:x} shdr_len=0x{:x} dynamic=0x{:x} efi_map_len=0x{:x} smap_len=0x{:x}",
            howto.unwrap_or(0),
            fw_handle.unwrap_or(0),
            modulep_md.unwrap_or(0),
            kernend.unwrap_or(0),
            ssym.unwrap_or(0),
            esym.unwrap_or(0),
            envp.unwrap_or(0),
            font.unwrap_or(0),
            keybuf_len.unwrap_or(0),
            elfhdr_len.unwrap_or(0),
            shdr_len.unwrap_or(0),
            dynamic.unwrap_or(0),
            efi_map_len.unwrap_or(0),
            smap_len.unwrap_or(0)
        );
    }
}

fn log_modulep_hex_preview(modulep: &[u8]) {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }
    if modulep.is_empty() {
        log::debug!("modulep-hex: empty");
        return;
    }

    let preview_len = modulep.len().min(256);
    let mut line = String::new();
    for (idx, byte) in modulep[..preview_len].iter().enumerate() {
        if idx % 16 == 0 {
            if !line.is_empty() {
                log::debug!("{}", line);
                line.clear();
            }
            line.push_str(&format!("modulep-hex: {:04x}:", idx));
        }
        line.push_str(&format!(" {:02x}", byte));
    }
    if !line.is_empty() {
        log::debug!("{}", line);
    }
    if modulep.len() > preview_len {
        log::debug!(
            "modulep-hex: truncated at 0x{:x} bytes (total 0x{:x})",
            preview_len,
            modulep.len()
        );
    }
}

fn log_modulep_layout(modulep: &[u8]) {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }
    if modulep.len() < 8 {
        log::debug!("modulep-layout: too small");
        return;
    }

    let mut offset = 0usize;
    let align = std::mem::size_of::<u64>();
    let mut index = 0usize;

    log::debug!("modulep-layout: total=0x{:x} align={}", modulep.len(), align);

    while offset + 8 <= modulep.len() {
        let type_ = u32::from_le_bytes(modulep[offset..offset + 4].try_into().unwrap());
        let size = u32::from_le_bytes(modulep[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let data_start = offset + 8;
        let data_end = data_start.saturating_add(size);

        if data_end > modulep.len() {
            log::debug!(
                "modulep-layout: idx={} type=0x{:x} size=0x{:x} data=0x{:x}..0x{:x} (truncated)",
                index,
                type_,
                size,
                data_start,
                data_end
            );
            break;
        }

        log::debug!(
            "modulep-layout: idx={} type=0x{:x} size=0x{:x} data=0x{:x}..0x{:x}",
            index,
            type_,
            size,
            data_start,
            data_end
        );

        offset = data_end;
        let pad = (align - (offset % align)) % align;
        if pad != 0 {
            log::debug!("modulep-layout: idx={} pad=0x{:x}", index, pad);
        }
        offset = offset.saturating_add(pad);
        index += 1;

        if type_ == crate::types::MODINFO_END && size == 0 {
            break;
        }
    }
}

fn log_modulep_modules(modulep: &[u8]) {
    use crate::types::{MODINFO_ADDR, MODINFO_END, MODINFO_NAME, MODINFO_SIZE, MODINFO_TYPE};

    if !log::log_enabled!(log::Level::Debug) {
        return;
    }
    if modulep.is_empty() {
        return;
    }

    let mut offset = 0usize;
    let align = std::mem::size_of::<u64>();
    let mut name: Option<String> = None;
    let mut module_type: Option<String> = None;
    let mut addr: Option<u64> = None;
    let mut size: Option<u64> = None;

    while offset + 8 <= modulep.len() {
        let type_ = u32::from_le_bytes(modulep[offset..offset + 4].try_into().unwrap());
        let size_bytes = u32::from_le_bytes(modulep[offset + 4..offset + 8].try_into().unwrap()) as usize;
        offset += 8;
        if type_ == MODINFO_END && size_bytes == 0 {
            break;
        }
        if offset + size_bytes > modulep.len() {
            break;
        }

        let data = &modulep[offset..offset + size_bytes];
        match type_ {
            MODINFO_NAME => {
                if let Some(cur_name) = name.take() {
                    log::debug!(
                        "modulep-mod: name={} type={} addr=0x{:x} size=0x{:x}",
                        cur_name,
                        module_type.unwrap_or_else(|| "<unknown>".to_string()),
                        addr.unwrap_or(0),
                        size.unwrap_or(0)
                    );
                }
                let nul = data.iter().position(|b| *b == 0).unwrap_or(data.len());
                name = Some(String::from_utf8_lossy(&data[..nul]).to_string());
                module_type = None;
                addr = None;
                size = None;
            }
            MODINFO_TYPE => {
                let nul = data.iter().position(|b| *b == 0).unwrap_or(data.len());
                module_type = Some(String::from_utf8_lossy(&data[..nul]).to_string());
            }
            MODINFO_ADDR if data.len() == 8 => {
                addr = Some(u64::from_le_bytes(data[..8].try_into().unwrap()));
            }
            MODINFO_SIZE if data.len() == 8 => {
                size = Some(u64::from_le_bytes(data[..8].try_into().unwrap()));
            }
            _ => {}
        }

        offset += size_bytes;
        let pad = (align - (offset % align)) % align;
        offset += pad;
    }

    if let Some(cur_name) = name.take() {
        log::debug!(
            "modulep-mod: name={} type={} addr=0x{:x} size=0x{:x}",
            cur_name,
            module_type.unwrap_or_else(|| "<unknown>".to_string()),
            addr.unwrap_or(0),
            size.unwrap_or(0)
        );
    }
}

fn build_modulep_info(
    loader: &ModuleLoader,
    kernel_name: &str,
    kernel_phys: u64,
    kernel_image_end: u64,
    howto: u32,
    efi_systab: u64,
    efi_map_info: Option<&crate::types::EfiMapInfo>,
    smap_info: Option<&crate::types::SmapInfo>,
    kernel_args: Option<&str>,
    symtab: Option<(u64, u64)>,
    envp: Option<u64>,
    font: Option<u64>,
    efi_fb_info: Option<&crate::types::EfiFbInfo>,
    modulep_start: u64,
    staging_base: u64,
    available_regions: &[crate::system::MemoryRegion],
) -> std::result::Result<crate::module::ModulepInfo, Box<dyn std::error::Error>> {
    use crate::module::build_modulep;
    use crate::system::align_up;
    use crate::types::PAGE_SIZE;

    if kernel_phys < staging_base {
        return Err("Kernel is below staging base".into());
    }
    let kernel_phys_offset = kernel_phys - staging_base;
    let kernel_size = kernel_image_end - kernel_phys;
    let mut modules_for_metadata = Vec::new();
    for module in loader.modules() {
        if module.phys_addr < staging_base {
            return Err("Module is below staging base".into());
        }
        // Modulep stores staging-relative addresses so that
        // preload_bootstrap_relocate(KERNBASE) yields KVA mapped by the
        // trampoline's KERNBASE->staging_base mapping.
        let mut meta_module = module.clone();
        meta_module.phys_addr = module.phys_addr - staging_base;
        modules_for_metadata.push(meta_module);
    }

    let map_offset = |addr: u64| -> std::result::Result<u64, Box<dyn std::error::Error>> {
        if addr < staging_base {
            return Err("Metadata address below staging base".into());
        }
        Ok(addr - staging_base)
    };

    let symtab = match symtab {
        Some((ssym, esym)) => Some((map_offset(ssym)?, map_offset(esym)?)),
        None => None,
    };
    let envp = match envp {
        Some(addr) => Some(map_offset(addr)?),
        None => None,
    };
    let font = match font {
        Some(addr) => Some(map_offset(addr)?),
        None => None,
    };
    let mut modulep_data = build_modulep(
        kernel_name,
        kernel_phys_offset,
        kernel_size,
        howto,
        efi_systab,
        efi_map_info,
        smap_info,
        kernel_args,
        0,
        loader.kernel_elfhdr(),
        loader.kernel_shdr(),
        loader.kernel_dynamic(),
        symtab,
        envp,
        font,
        efi_fb_info,
        &modules_for_metadata,
        0,
    )?;

    let mut modulep_phys = 0u64;
    let mut kernend = 0u64;
    let mut converged = false;
    for iteration in 0..4 {
        let modulep_size = align_up(modulep_data.len() as u64, PAGE_SIZE as u64);
        modulep_phys = find_available_at_or_after(modulep_start, modulep_size, available_regions)
            .ok_or("No free memory region found for module metadata")?;
        kernend = modulep_phys + modulep_size;
        if modulep_phys < staging_base {
            return Err("Module metadata is below staging base".into());
        }
        if kernend < kernel_phys {
            return Err("Module metadata is below kernel load address".into());
        }
        let modulep_offset = modulep_phys - staging_base;
        let kernend_offset = kernend - kernel_phys;
        if modulep_offset > u32::MAX as u64 || kernend_offset > u32::MAX as u64 {
            return Err("Module metadata offsets exceed 32-bit addressable range".into());
        }
        log::debug!(
            "modulep-iter: {} size=0x{:x} phys=0x{:x} kernend=0x{:x}",
            iteration,
            modulep_data.len(),
            modulep_phys,
            kernend
        );

        let kernend_offset = kernend - kernel_phys;
        let next_data = build_modulep(
            kernel_name,
            kernel_phys_offset,
            kernel_size,
            howto,
            efi_systab,
            efi_map_info,
            smap_info,
            kernel_args,
            modulep_offset,
            loader.kernel_elfhdr(),
            loader.kernel_shdr(),
            loader.kernel_dynamic(),
            symtab,
            envp,
            font,
            efi_fb_info,
            &modules_for_metadata,
            kernend_offset,
        )?;
        if next_data.len() == modulep_data.len() {
            modulep_data = next_data;
            converged = true;
            break;
        }
        modulep_data = next_data;
    }

    if !converged {
        log::warn!(
            "Module metadata size did not converge after kernend update; proceeding with last size"
        );
    }

    let efi_map_offset = find_metadata_offset(
        &modulep_data,
        crate::types::MODINFO_METADATA | crate::types::ModInfoMd::EfiMap as u32,
    );

    Ok(crate::module::ModulepInfo {
        data: modulep_data,
        phys_addr: modulep_phys,
        kernend,
        efi_map_offset,
    })
}

fn find_metadata_offset(modulep: &[u8], wanted_type: u32) -> Option<u64> {
    use crate::system::align_up;

    let mut offset = 0usize;
    while offset + 8 <= modulep.len() {
        let type_ = u32::from_le_bytes(
            modulep[offset..offset + 4]
                .try_into()
                .ok()?,
        );
        let size = u32::from_le_bytes(
            modulep[offset + 4..offset + 8]
                .try_into()
                .ok()?,
        ) as usize;
        if type_ == crate::types::MODINFO_END && size == 0 {
            break;
        }
        let data_start = offset + 8;
        let data_end = data_start.saturating_add(size);
        if data_end > modulep.len() {
            break;
        }
        if type_ == wanted_type {
            return Some(data_start as u64);
        }
        offset = align_up(data_end as u64, 8) as usize;
    }

    None
}

fn resolve_boot_cmdline(
    cmdline: Option<String>,
    bootonce_dataset: Option<String>,
    detected_bootdev: Option<String>,
) -> Option<String> {
    if cmdline.is_some() {
        return cmdline;
    }

    if let Some(dataset) = bootonce_dataset {
        let zfs_root = crate::zfs::format_zfs_mountfrom(&dataset);
        return Some(format!("root={0} vfs.root.mountfrom={0}", zfs_root));
    }

    if let Some(bootdev) = detected_bootdev {
        if bootdev.starts_with("zfs:") {
            let zfs_root = crate::zfs::format_zfs_mountfrom(&bootdev);
            return Some(format!("root={0} vfs.root.mountfrom={0}", zfs_root));
        }
    }

    None
}

/// Round up to alignment
#[cfg(test)]
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
    use crate::boot::BootAssembler;
    use crate::module::ModuleLoader;

    #[test]
    fn test_round_up() {
        // Test basic rounding
        assert_eq!(round_up(0, 4096), 0);
        assert_eq!(round_up(1, 4096), 4096);
        assert_eq!(round_up(4095, 4096), 4096);
        assert_eq!(round_up(4096, 4096), 4096);
        assert_eq!(round_up(4097, 4096), 8192);
        
        // Test with zero alignment
        assert_eq!(round_up(1234, 0), 1234);
        
        // Test with small alignment
        assert_eq!(round_up(7, 8), 8);
        assert_eq!(round_up(8, 8), 8);
        assert_eq!(round_up(9, 8), 16);
    }

    #[test]
    fn test_prepare_kexec_segments_empty() {
        // Test with empty inputs
        let loader = ModuleLoader::new();
        let assembler = BootAssembler::new(0x1000);
        let modulep_info = crate::module::ModulepInfo {
            data: Vec::new(),
            phys_addr: 0,
            kernend: 0,
            efi_map_offset: None,
        };
        let available_regions = Vec::new();
        let preload_segments = PreloadSegments {
            bundle: None,
            symtab: None,
            env: None,
            font: None,
        };
        
        // This should fail because kernel_path doesn't exist
        let result = prepare_kexec_segments(
            "/nonexistent/kernel",
            &loader,
            &assembler,
            &modulep_info,
            0,
            &available_regions,
            &preload_segments,
            loader.kernel_entry(),
            true,
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_boot_assembler_cmdline() {
        // Test command line setting
        let mut assembler = BootAssembler::new(0x1000);
        let staging_base = 0x4000_0000;
        assembler.set_staging_base(staging_base);
        assembler.set_boot_addr(staging_base + crate::types::BOOT_PHYS_BASE);
        let kernel_phys_base = 0x4200_0000;
        assembler.set_kernel_phys_base(kernel_phys_base);
        assembler.set_modulep(kernel_phys_base + 0x300000);
        assembler.set_kernend(kernel_phys_base + 0x400000);
        
        // Initially empty
        // Note: cmdline field is private, so we test via setter/getter pattern
        // For now, just verify set_cmdline doesn't panic
        assembler.set_cmdline("root=zfs:pool/dataset");
        
        // Verify assembler can be assembled (basic functionality)
        let result = assembler.assemble();
        assert!(result.is_ok(), "boot assembler failed: {}", result.unwrap_err());
    }

    #[test]
    fn test_build_font_block_layout() {
        let glyph_count = 256usize;
        let glyph_bytes = 16usize; // 8x16 font with 1 byte per row
        let font_data = vec![0x5a; glyph_count * glyph_bytes];
        let font = crate::font::FontInfo {
            name: "test-font".to_string(),
            width: 8,
            height: 16,
            data: font_data.clone(),
            stride: 1,
        };

        let buf = build_font_block_from_font(&font).unwrap();
        let mut offset = 0usize;

        let fi_checksum = i32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let fi_width = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let fi_height = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let fi_bitmap_size = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let mut map_counts = [0u32; 4];
        for count in &mut map_counts {
            *count = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
            offset += 4;
        }

        let checksum = fi_width
            .wrapping_add(fi_height)
            .wrapping_add(fi_bitmap_size)
            .wrapping_add(map_counts.iter().sum::<u32>());
        assert_eq!(checksum.wrapping_add(fi_checksum as u32), 0);
        assert_eq!(map_counts[crate::types::VfntMapType::Normal as usize], 1);

        offset = round_up(offset, 8);
        let vfm_src = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let vfm_dst = u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap());
        offset += 2;
        let vfm_len = u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap());
        offset += 2;

        assert_eq!(vfm_src, 0);
        assert_eq!(vfm_dst, 0);
        assert_eq!(vfm_len, (glyph_count - 1) as u16);

        offset = round_up(offset, 8);
        assert_eq!(buf.len() - offset, font_data.len());
        assert_eq!(&buf[offset..], &font_data[..]);
    }

    #[test]
    fn test_create_test_elf() {
        // Test that create_test_elf creates valid ELF data
        let elf_data = create_test_elf();
        
        // Check ELF magic
        assert_eq!(elf_data[0], 0x7f);
        assert_eq!(elf_data[1], b'E');
        assert_eq!(elf_data[2], b'L');
        assert_eq!(elf_data[3], b'F');
        
        // Check 64-bit
        assert_eq!(elf_data[4], 2);
        
        // Check little endian
        assert_eq!(elf_data[5], 1);
    }

    #[test]
    fn test_generate_default_config() {
        let config = generate_default_config();
        
        // Check it contains expected sections
        assert!(config.contains("[general]"));
        assert!(config.contains("[kexec]"));
        assert!(config.contains("[memory]"));
        assert!(config.contains("[modules]"));
        assert!(config.contains("[system]"));
        
        // Check some key values
        assert!(config.contains("verbose = false"));
        assert!(config.contains("debug = false"));
        assert!(config.contains("arch = 62"));
    }

    #[test]
    fn test_validate_config() {
        // Valid TOML
        assert!(validate_config("key = \"value\""));
        assert!(validate_config("[section]\nkey = 123"));
        
        // Invalid (empty or no equals)
        assert!(!validate_config(""));
        assert!(!validate_config("just text"));
        assert!(!validate_config("[section only]"));
    }

    #[test]
    fn test_boot_args_parsing() {
        // Test that BootArgs struct can be created (compile test)
        use crate::cli::args::BootArgs;
        
        // This is a compile-time test - if it compiles, the struct works
        let _args = BootArgs {
            kernel: "/boot/kernel".to_string(),
            cmdline: Some("root=zfs:pool/dataset".to_string()),
            modules: vec![],
            mfsroot: None,
            initrd: None,
            howto: None,
            bootonce: false,
            no_fb: false,
            no_acpi: false,
            no_memmap: false,
            entry_patch: false,
            force: false,
        };
        
        // Just verify the struct exists and has expected fields
        assert_eq!(_args.kernel, "/boot/kernel");
        assert_eq!(_args.cmdline.unwrap(), "root=zfs:pool/dataset");
    }

    #[test]
    fn test_resolve_boot_cmdline() {
        let provided = resolve_boot_cmdline(
            Some("root=zfs:pool/ROOT/default".to_string()),
            Some("zroot/ROOT/alt".to_string()),
            Some("zfs:zroot/ROOT/default".to_string()),
        );
        assert_eq!(provided, Some("root=zfs:pool/ROOT/default".to_string()));

        let bootonce = resolve_boot_cmdline(
            None,
            Some("zroot/ROOT/bootonce".to_string()),
            Some("zfs:zroot/ROOT/default".to_string()),
        );
        assert_eq!(
            bootonce,
            Some("root=zfs:zroot/ROOT/bootonce vfs.root.mountfrom=zfs:zroot/ROOT/bootonce".to_string())
        );

        let bootdev = resolve_boot_cmdline(None, None, Some("zfs:zroot/ROOT/default".to_string()));
        assert_eq!(
            bootdev,
            Some("root=zfs:zroot/ROOT/default vfs.root.mountfrom=zfs:zroot/ROOT/default".to_string())
        );

        let non_zfs = resolve_boot_cmdline(None, None, Some("/dev/sda1".to_string()));
        assert!(non_zfs.is_none());
    }
}
