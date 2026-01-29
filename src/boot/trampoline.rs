use crate::error::Result;

/// Trampoline code generator
pub struct TrampolineGenerator {
    code: Vec<u8>,
    data: Vec<u8>,
}

impl TrampolineGenerator {
    pub fn new() -> Self {
        Self {
            code: Vec::new(),
            data: Vec::new(),
        }
    }

    pub fn add_code(&mut self, code: &[u8]) {
        self.code.extend_from_slice(code);
    }

    pub fn add_data(&mut self, data: &[u8]) {
        self.data.extend_from_slice(data);
    }

    pub fn add_data_u64(&mut self, value: u64) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub fn add_data_u32(&mut self, value: u32) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub fn add_data_u16(&mut self, value: u16) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub fn add_data_u8(&mut self, value: u8) {
        self.data.push(value);
    }

    pub fn build(self) -> (Vec<u8>, Vec<u8>) {
        (self.code, self.data)
    }
}

/// Generate trampoline for FreeBSD kernel
pub fn generate_freebsd_trampoline(
    kernel_entry: u64,
    _virt_delta: u64,
    smap_table: Option<u64>,
    efi_map_table: Option<u64>,
    efi_fb_info: Option<u64>,
    font_info: Option<u64>,
    rsdp: u64,
    rsdt: u64,
    howto: u32,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut generator = TrampolineGenerator::new();

    // Simple trampoline that jumps to kernel
    // In a real implementation, this would set up the proper environment
    // including memory maps, ACPI tables, etc.

    // Entry point code
    let mut code = Vec::new();

    // Save registers
    code.extend_from_slice(&[
        0x50, // push rax
        0x53, // push rbx
        0x51, // push rcx
        0x52, // push rdx
        0x56, // push rsi
        0x57, // push rdi
        0x55, // push rbp
        0x41, 0x50, // push r8
        0x41, 0x51, // push r9
        0x41, 0x52, // push r10
        0x41, 0x53, // push r11
        0x41, 0x54, // push r12
        0x41, 0x55, // push r13
        0x41, 0x56, // push r14
        0x41, 0x57, // push r15
    ]);

    // Set up kernel arguments
    // FreeBSD expects:
    // - rdi: howto flags
    // - rsi: bootinfo pointer (or 0)
    // - rdx: environment pointer (or 0)
    // - rcx: kernel entry point

    // Set howto flags
    code.extend_from_slice(&[0x48, 0xbf]); // mov rdi, howto
    code.extend_from_slice(&howto.to_le_bytes());

    // Set bootinfo pointer (0 for now)
    code.extend_from_slice(&[0x48, 0xbe, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // mov rsi, 0

    // Set environment pointer (0 for now)
    code.extend_from_slice(&[0x48, 0xba, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // mov rdx, 0

    // Load kernel entry point
    code.extend_from_slice(&[0x48, 0xb8]); // mov rax, kernel_entry
    code.extend_from_slice(&kernel_entry.to_le_bytes());

    // Jump to kernel
    code.extend_from_slice(&[0xff, 0xe0]); // jmp rax

    // Restore registers (unreachable)
    code.extend_from_slice(&[
        0x41, 0x5f, // pop r15
        0x41, 0x5e, // pop r14
        0x41, 0x5d, // pop r13
        0x41, 0x5c, // pop r12
        0x41, 0x5b, // pop r11
        0x41, 0x5a, // pop r10
        0x41, 0x59, // pop r9
        0x41, 0x58, // pop r8
        0x5d, // pop rbp
        0x5f, // pop rdi
        0x5e, // pop rsi
        0x5a, // pop rdx
        0x59, // pop rcx
        0x5b, // pop rbx
        0x58, // pop rax
    ]);

    generator.add_code(&code);

    // Add data section with system information
    if let Some(smap) = smap_table {
        generator.add_data_u64(smap);
    }

    if let Some(efi_map) = efi_map_table {
        generator.add_data_u64(efi_map);
    }

    if let Some(efi_fb) = efi_fb_info {
        generator.add_data_u64(efi_fb);
    }

    if let Some(font) = font_info {
        generator.add_data_u64(font);
    }

    if rsdp != 0 {
        generator.add_data_u64(rsdp);
    }

    if rsdt != 0 {
        generator.add_data_u64(rsdt);
    }

    Ok(generator.build())
}
