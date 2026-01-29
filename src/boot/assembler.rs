use crate::error::Result;
use crate::types::{FbInfo, PAGE_SIZE};

/// Boot assembler for x86_64
pub struct BootAssembler {
    code: Vec<u8>,
    data: Vec<u8>,
    kernel_entry: u64,
    modulep: u64,
    kernend: u64,
    fb: FbInfo,
    rsdp: u64,
    rsdt: u64,
    howto: u32,
    cmdline: String,
    boot_addr: u64,
    staging_base: u64,
    kernel_phys_base: u64,
    efi_memmap_src: u64,
    efi_memmap_dst: u64,
    efi_memmap_len: u64,
    debug_trampoline: bool,
}

impl BootAssembler {
    pub fn new(kernel_entry: u64) -> Self {
        Self {
            code: Vec::new(),
            data: Vec::new(),
            kernel_entry,
            modulep: 0,
            kernend: 0,
            fb: FbInfo::default(),
            rsdp: 0,
            rsdt: 0,
            howto: 0,
            cmdline: String::new(),
            boot_addr: 0,
            staging_base: 0,
            kernel_phys_base: 0,
            efi_memmap_src: 0,
            efi_memmap_dst: 0,
            efi_memmap_len: 0,
            debug_trampoline: false,
        }
    }

    pub fn set_acpi_tables(&mut self, rsdp: u64, rsdt: u64) {
        self.rsdp = rsdp;
        self.rsdt = rsdt;
    }

    pub fn set_howto(&mut self, howto: u32) {
        self.howto = howto;
    }

    pub fn set_modulep(&mut self, modulep: u64) {
        self.modulep = modulep;
    }

    pub fn set_kernend(&mut self, kernend: u64) {
        self.kernend = kernend;
    }

    pub fn set_fb(&mut self, fb: FbInfo) {
        self.fb = fb;
    }

    pub fn set_cmdline(&mut self, cmdline: &str) {
        self.cmdline = cmdline.to_string();
    }

    pub fn set_boot_addr(&mut self, boot_addr: u64) {
        self.boot_addr = boot_addr;
    }

    pub fn set_staging_base(&mut self, staging_base: u64) {
        self.staging_base = staging_base;
    }

    pub fn set_kernel_phys_base(&mut self, kernel_phys_base: u64) {
        self.kernel_phys_base = kernel_phys_base;
    }

    pub fn set_efi_memmap(&mut self, src: u64, dst: u64, len: u64) {
        self.efi_memmap_src = src;
        self.efi_memmap_dst = dst;
        self.efi_memmap_len = len;
    }

    pub fn set_trampoline_debug(&mut self, enabled: bool) {
        self.debug_trampoline = enabled;
    }

    /// Generate boot code
    pub fn assemble(&mut self) -> Result<()> {
        self.generate_kboot_trampoline()?;
        Ok(())
    }

    /// Get the assembled code
    pub fn code(&self) -> &[u8] {
        &self.code
    }

    /// Get the assembled data
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Get complete boot block (code + data)
    pub fn boot_block(&self) -> Vec<u8> {
        let mut block = Vec::new();
        block.extend_from_slice(&self.code);
        block.extend_from_slice(&self.data);
        block
    }

    fn generate_kboot_trampoline(&mut self) -> Result<()> {
        use crate::error::BootError;

        if self.boot_addr == 0 || self.staging_base == 0 || self.kernel_phys_base == 0 {
            return Err(BootError::System(
                "Boot assembler requires boot_addr, staging_base, and kernel_phys_base".to_string(),
            ));
        }

        const PG_V: u64 = 0x001;
        const PG_RW: u64 = 0x002;
        const PG_PS: u64 = 0x080;
        const NPML4EPG: usize = 512;
        const NPDPEPG: usize = 512;
        const NPDEPG: usize = 512;
        const PDRSHIFT: u64 = 21;
        const NBPDR: u64 = 1 << PDRSHIFT;
        const EFI_MEMMAP_COPY_MAX: u32 = 0x20000;

        self.code.clear();
        self.data.clear();

        self.code.push(0xFA); // cli
        self.code.push(0xFC); // cld
        // Heartbeat: init COM1 (0x3f8) and write 'Z' to confirm trampoline execution.
        self.code.extend_from_slice(&[
            0x66, 0xBA, 0xF9, 0x03, // mov dx, 0x3f9 (IER)
            0xB0, 0x00,             // mov al, 0
            0xEE,                   // out dx, al
            0x66, 0xBA, 0xFB, 0x03, // mov dx, 0x3fb (LCR)
            0xB0, 0x80,             // mov al, DLAB
            0xEE,                   // out dx, al
            0x66, 0xBA, 0xF8, 0x03, // mov dx, 0x3f8 (DLL)
            0xB0, 0x01,             // mov al, divisor low (1 => 115200)
            0xEE,                   // out dx, al
            0x66, 0xBA, 0xF9, 0x03, // mov dx, 0x3f9 (DLM)
            0xB0, 0x00,             // mov al, divisor high
            0xEE,                   // out dx, al
            0x66, 0xBA, 0xFB, 0x03, // mov dx, 0x3fb (LCR)
            0xB0, 0x03,             // mov al, 8N1
            0xEE,                   // out dx, al
            0x66, 0xBA, 0xFA, 0x03, // mov dx, 0x3fa (FCR)
            0xB0, 0xC7,             // mov al, enable FIFO, clear, 14-byte
            0xEE,                   // out dx, al
            0x66, 0xBA, 0xFC, 0x03, // mov dx, 0x3fc (MCR)
            0xB0, 0x0B,             // mov al, IRQs enabled, RTS/DSR set
            0xEE,                   // out dx, al
            0x66, 0xBA, 0xFD, 0x03, // mov dx, 0x3fd (LSR)
            0xEC,                   // in al, dx
            0xA8, 0x20,             // test al, 0x20
            0x74, 0xFB,             // jz -5 (wait for THR empty)
            0x66, 0xBA, 0xF8, 0x03, // mov dx, 0x3f8 (COM1)
            0xB0, b'Z',             // mov al, 'Z'
            0xEE,                   // out dx, al
        ]);
        if self.debug_trampoline {
            self.emit_uart_char(b'1');
        }
        self.code.extend_from_slice(&[0x48, 0x8D, 0x25]); // lea rsp, [rip+disp32]
        let lea_disp_pos = self.code.len();
        self.code.extend_from_slice(&[0, 0, 0, 0]);
        if self.debug_trampoline {
            self.emit_uart_char(b'2');
        }
        self.code.push(0x5E); // pop rsi
        self.code.push(0x5F); // pop rdi
        self.code.push(0x59); // pop rcx
        if self.debug_trampoline {
            self.emit_uart_char(b'3');
        }
        self.code.extend_from_slice(&[0x48, 0x85, 0xF6]); // test rsi, rsi
        self.code.extend_from_slice(&[0x74, 0x00]); // je rel8
        let je_disp_pos = self.code.len() - 1;
        self.code.extend_from_slice(&[0x48, 0x81, 0xF9]); // cmp rcx, imm32
        self.code.extend_from_slice(&EFI_MEMMAP_COPY_MAX.to_le_bytes());
        self.code.extend_from_slice(&[0x77, 0x00]); // ja rel8
        let ja_disp_pos = self.code.len() - 1;
        self.code.extend_from_slice(&[0xF3, 0xA4]); // rep movsb
        let no_copy_offset = self.code.len();
        if self.debug_trampoline {
            self.emit_uart_char(b'4');
        }
        let pt4_store_disp_pos;
        let mut pt4_load_disp_positions = Vec::new();
        let mut idtr_disp_pos = None;
        let mut gp_handler_offset = None;
        let mut pf_handler_offset = None;
        let mut off_a = None;
        let mut off_g = None;
        let mut off_c = None;
        let mut off_r = None;
        let mut off_cap_r = None;
        let mut off_s = None;
        let mut off_d = None;
        let mut off_b = None;
        let mut off_c_jump = None;
        let mut off_entry_load = None;
        let mut off_rsp_set = None;
        let mut off_jmp = None;
        self.code.push(0x58); // pop rax
        // Cache the kernel entry from the parameter stack before we overwrite it.
        self.emit_bytes(&[0x4C, 0x8B, 0x0C, 0x24]); // mov r9, [rsp]
        self.code.extend_from_slice(&[0x48, 0x89, 0x05]); // mov [rip+disp32], rax
        let store_disp_pos = self.code.len();
        self.code.extend_from_slice(&[0, 0, 0, 0]);
        pt4_store_disp_pos = store_disp_pos;
        self.code.extend_from_slice(&[0x0F, 0x22, 0xD8]); // mov cr3, rax
        if off_a.is_none() {
            off_a = Some(self.code.len());
        }
        self.emit_uart_char(b'A');
        if self.debug_trampoline {
            self.emit_uart_char(b'5');
        }
        let mut jmp_stub_disp_pos = None;
        let gdtr_disp_pos: Option<usize>;
        let lgdt_offset: Option<usize>;
        let mut cs_reload_disp_pos: Option<usize> = None;
        let mut cs_reload_label: Option<usize> = None;
        let mut cs_reload_skip_disp_pos: Option<usize> = None;
        let mut cs_reload_done_disp_pos: Option<usize> = None;
        let mut cs_reload_done_label: Option<usize> = None;
        let cs_reload_scratch_disp_pos: Option<usize>;
        self.emit_bytes(&[0x48, 0x8D, 0x05]); // lea rax, [rip+disp32]
        let disp_pos = self.code.len();
        self.emit_bytes(&[0, 0, 0, 0]);
        gdtr_disp_pos = Some(disp_pos);
        self.emit_bytes(&[0x0F, 0x01, 0x10]); // lgdt [rax]
        lgdt_offset = Some(self.code.len().saturating_sub(3));
        if off_g.is_none() {
            off_g = Some(self.code.len());
        }
        self.emit_uart_char(b'g');
        // Preserve the post-pop stack pointer for kernel entry setup.
        self.emit_bytes(&[0x49, 0x89, 0xE3]); // mov r11, rsp
        // Reload CS only if it's not already the expected selector (0x20).
        self.emit_bytes(&[0x66, 0x8C, 0xC8]); // mov ax, cs
        if off_c.is_none() {
            off_c = Some(self.code.len());
        }
        self.emit_uart_char(b'c');
        self.emit_bytes(&[0x66, 0x83, 0xF8, 0x20]); // cmp ax, 0x20
        self.emit_bytes(&[0x0F, 0x84, 0, 0, 0, 0]); // je rel32
        let cs_skip_disp_pos_local = self.code.len() - 4;
        // Reload CS to match FreeBSD's expected selectors (GCODE_SEL=0x20).
        if off_r.is_none() {
            off_r = Some(self.code.len());
        }
        self.emit_uart_char(b'r');
        self.emit_bytes(&[0x48, 0x8D, 0x25]); // lea rsp, [rip+disp32]
        let cs_scratch_disp_pos = self.code.len();
        self.emit_bytes(&[0, 0, 0, 0]);
        self.emit_bytes(&[0x6A, 0x20]); // push 0x20
        self.emit_bytes(&[0x48, 0x8D, 0x05]); // lea rax, [rip+disp32]
        let cs_reload_disp_pos_local = self.code.len();
        self.emit_bytes(&[0, 0, 0, 0]);
        self.emit_bytes(&[0x50]); // push rax
        self.emit_bytes(&[0x48, 0xCB]); // lretq
        let cs_reload_label_local = self.code.len();
        if off_cap_r.is_none() {
            off_cap_r = Some(self.code.len());
        }
        self.emit_uart_char(b'R');
        cs_reload_disp_pos.get_or_insert(cs_reload_disp_pos_local);
        cs_reload_label.get_or_insert(cs_reload_label_local);
        cs_reload_scratch_disp_pos = Some(cs_scratch_disp_pos);
        // Restore the original parameter stack after CS reload.
        self.emit_bytes(&[0x4C, 0x89, 0xDC]); // mov rsp, r11
        self.code.push(0xE9); // jmp rel32 to cs_done
        let cs_done_disp_pos_local = self.code.len();
        self.emit_bytes(&[0, 0, 0, 0]);
        cs_reload_done_disp_pos.get_or_insert(cs_done_disp_pos_local);
        let cs_reload_done_label_local = self.code.len();
        if off_s.is_none() {
            off_s = Some(self.code.len());
        }
        self.emit_uart_char(b's');
        cs_reload_skip_disp_pos.get_or_insert(cs_skip_disp_pos_local);
        cs_reload_done_label.get_or_insert(cs_reload_done_label_local);
        // Skip data segment reloads for now (kboot trampoline leaves them as-is).
        if off_d.is_none() {
            off_d = Some(self.code.len());
        }
        self.emit_uart_char(b'd');
        if self.debug_trampoline {
            // Load a minimal IDT so page faults / GPs report a marker.
            self.emit_bytes(&[0x48, 0x8D, 0x05]); // lea rax, [rip+disp32]
            let disp_pos = self.code.len();
            self.emit_bytes(&[0, 0, 0, 0]);
            idtr_disp_pos = Some(disp_pos);
            self.emit_bytes(&[0x0F, 0x01, 0x18]); // lidt [rax]
            self.code.push(0xE8); // call rel32
            let disp_pos = self.code.len();
            self.code.extend_from_slice(&[0, 0, 0, 0]);
            jmp_stub_disp_pos = Some(disp_pos);
        }
        let modulep_phys = self.modulep;
        let kernend_phys = self.kernend;
        let modulep_offset = if modulep_phys == 0 {
            0u64
        } else if modulep_phys >= self.staging_base {
            modulep_phys - self.staging_base
        } else {
            return Err(BootError::System(
                "modulep is below staging base".to_string(),
            ));
        };
        let kernend_offset = if kernend_phys == 0 {
            0u64
        } else if kernend_phys >= self.kernel_phys_base {
            kernend_phys - self.kernel_phys_base
        } else {
            return Err(BootError::System(
                "kernend is below kernel load address".to_string(),
            ));
        };
        let modulep_offset32 = u32::try_from(modulep_offset).map_err(|_| {
            BootError::System("modulep offset exceeds 32-bit range for btext".to_string())
        })?;
        let kernend_offset32 = u32::try_from(kernend_offset).map_err(|_| {
            BootError::System("kernend offset exceeds 32-bit range for btext".to_string())
        })?;
        let mut call_positions = Vec::new();
        let emit_call = |assembler: &mut BootAssembler, positions: &mut Vec<usize>| {
            assembler.emit_byte(0xE8); // call rel32
            let pos = assembler.code.len();
            assembler.emit_bytes(&[0, 0, 0, 0]);
            positions.push(pos);
        };
        if off_b.is_none() {
            off_b = Some(self.code.len());
        }
        self.emit_uart_char(b'B');
        if off_entry_load.is_none() {
            off_entry_load = Some(self.code.len());
        }
        self.emit_bytes(&[0x4C, 0x89, 0xC8]); // mov rax, r9
        if off_rsp_set.is_none() {
            off_rsp_set = Some(self.code.len());
        }
        self.emit_bytes(&[0x49, 0x8D, 0xA3]); // lea rsp, [r11+disp32]
        let btext_rsp_disp_pos = self.code.len();
        self.emit_bytes(&[0, 0, 0, 0]);
        // Populate the 32-bit btext stack frame at [rsp].
        self.emit_bytes(&[0xC7, 0x04, 0x24]); // mov dword ptr [rsp], imm32
        self.emit_bytes(&0u32.to_le_bytes()); // ret addr placeholder
        self.emit_bytes(&[0xC7, 0x44, 0x24, 0x04]); // mov dword ptr [rsp+4], imm32
        self.emit_bytes(&modulep_offset32.to_le_bytes());
        self.emit_bytes(&[0xC7, 0x44, 0x24, 0x08]); // mov dword ptr [rsp+8], imm32
        self.emit_bytes(&kernend_offset32.to_le_bytes());
        self.emit_bytes(&[0xC7, 0x44, 0x24, 0x0C]); // mov dword ptr [rsp+12], imm32
        self.emit_bytes(&0u32.to_le_bytes());
        if self.debug_trampoline {
            self.emit_uart_char(b'B');
            self.emit_uart_char(b'T');
            self.emit_uart_char(b'0');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x8B, 0x04, 0x24]); // mov eax, [rsp]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'B');
            self.emit_uart_char(b'T');
            self.emit_uart_char(b'1');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x8B, 0x44, 0x24, 0x04]); // mov eax, [rsp+4]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'B');
            self.emit_uart_char(b'T');
            self.emit_uart_char(b'2');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x8B, 0x44, 0x24, 0x08]); // mov eax, [rsp+8]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            // Restore rax with the kernel entry after debug prints.
            self.emit_bytes(&[0x4C, 0x89, 0xC8]); // mov rax, r9
        }
        // Rewrite the btext frame after any debug calls to avoid clobbering by call/ret.
        self.emit_bytes(&[0xC7, 0x04, 0x24]); // mov dword ptr [rsp], imm32
        self.emit_bytes(&0u32.to_le_bytes()); // ret addr placeholder
        self.emit_bytes(&[0xC7, 0x44, 0x24, 0x04]); // mov dword ptr [rsp+4], imm32
        self.emit_bytes(&modulep_offset32.to_le_bytes());
        self.emit_bytes(&[0xC7, 0x44, 0x24, 0x08]); // mov dword ptr [rsp+8], imm32
        self.emit_bytes(&kernend_offset32.to_le_bytes());
        self.emit_bytes(&[0xC7, 0x44, 0x24, 0x0C]); // mov dword ptr [rsp+12], imm32
        self.emit_bytes(&0u32.to_le_bytes());
        if off_c_jump.is_none() {
            off_c_jump = Some(self.code.len());
        }
        self.emit_bytes(&[0x50]); // push rax
        self.emit_uart_char(b'C');
        self.emit_bytes(&[0x58]); // pop rax
        if self.debug_trampoline {
            self.emit_bytes(&[0x50]); // push rax
            self.emit_uart_char(b'\r');
            self.emit_uart_char(b'\n');
            self.emit_uart_char(b'C');
            self.emit_uart_char(b'J');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x58]); // pop rax
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_bytes(&[0x4C, 0x89, 0xC8]); // mov rax, r9
        }
        if off_jmp.is_none() {
            off_jmp = Some(self.code.len());
        }
        self.code.extend_from_slice(&[0xFF, 0xE0]); // jmp rax

        let entry_addr = self.kernel_entry;
        if self.debug_trampoline {
            let stub_offset = self.code.len();
            self.emit_uart_char(b'6');
            // Save the post-pop stack pointer for later verification prints.
            self.emit_bytes(&[0x49, 0x89, 0xE4]); // mov r12, rsp
            self.emit_bytes(&[
                0x50, // push rax
                0x51, // push rcx
                0x52, // push rdx
                0x53, // push rbx
                0x56, // push rsi
                0x57, // push rdi
                0x55, // push rbp
                0x41, 0x53, // push r11
                0x41, 0x54, // push r12
            ]);

            self.emit_uart_char(b'\r');
            self.emit_uart_char(b'\n');
            self.emit_uart_char(b'R');
            self.emit_uart_char(b':');
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'T');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0xB8]); // mov rax, 0x0123456789ABCDEF
            self.emit_u64(0x0123_4567_89AB_CDEF);
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'D');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x44, 0x24, 0x10]); // mov rax, [rsp+0x10]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'S');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x44, 0x24, 0x18]); // mov rax, [rsp+0x18]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'X');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x44, 0x24, 0x28]); // mov rax, [rsp+0x28]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'C');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x44, 0x24, 0x30]); // mov rax, [rsp+0x30]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'P');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8D, 0x44, 0x24, 0x40]); // lea rax, [rsp+0x40]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'E');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0xB8]); // mov rax, kernel_entry
            self.emit_u64(self.kernel_entry);
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            // Print stack-based entry/modulep/kernend values for verification.
            self.emit_uart_char(b'S');
            self.emit_uart_char(b'E');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x4C, 0x89, 0xE3]); // mov rbx, r12
            self.emit_bytes(&[0x48, 0x8B, 0x43, 0x08]); // mov rax, [rbx+0x08]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'M');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x4C, 0x89, 0xE3]); // mov rbx, r12
            self.emit_bytes(&[0x8B, 0x43, 0x14]); // mov eax, [rbx+0x14]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'K');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x4C, 0x89, 0xE3]); // mov rbx, r12
            self.emit_bytes(&[0x8B, 0x43, 0x18]); // mov eax, [rbx+0x18]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'Q');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x05]); // mov rax, [rip+disp32]
            let pt4_load_disp_pos_local = self.code.len();
            self.emit_bytes(&[0, 0, 0, 0]);
            pt4_load_disp_positions.push(pt4_load_disp_pos_local);
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'c');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x66, 0x8C, 0xC8]); // mov ax, cs
            self.emit_bytes(&[0x0F, 0xB7, 0xC0]); // movzx eax, ax
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b's');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x66, 0x8C, 0xD0]); // mov ax, ss
            self.emit_bytes(&[0x0F, 0xB7, 0xC0]); // movzx eax, ax
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            // Dump GDTR base/limit and the CS descriptor for mode diagnostics.
            self.emit_bytes(&[0x48, 0x83, 0xEC, 0x20]); // sub rsp, 0x20
            self.emit_bytes(&[0x0F, 0x01, 0x04, 0x24]); // sgdt [rsp]
            self.emit_bytes(&[0x4C, 0x8B, 0x6C, 0x24, 0x02]); // mov r13, [rsp+2]
            self.emit_bytes(&[0x44, 0x0F, 0xB7, 0x34, 0x24]); // movzx r14d, word [rsp]
            self.emit_bytes(&[0x48, 0x83, 0xC4, 0x20]); // add rsp, 0x20
            self.emit_uart_char(b'b');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x4C, 0x89, 0xE8]); // mov rax, r13
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'l');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x4C, 0x89, 0xF0]); // mov rax, r14
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'g');
            self.emit_uart_char(b'8');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x49, 0x8B, 0x45, 0x08]); // mov rax, [r13+0x08]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'g');
            self.emit_uart_char(b'1');
            self.emit_uart_char(b'0');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x49, 0x8B, 0x45, 0x10]); // mov rax, [r13+0x10]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'g');
            self.emit_uart_char(b'1');
            self.emit_uart_char(b'8');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x49, 0x8B, 0x45, 0x18]); // mov rax, [r13+0x18]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_bytes(&[0x48, 0x83, 0xEC, 0x20]); // sub rsp, 0x20
            self.emit_bytes(&[0x66, 0x8C, 0xC8]); // mov ax, cs
            self.emit_bytes(&[0x0F, 0xB7, 0xC0]); // movzx eax, ax
            self.emit_bytes(&[0x48, 0x89, 0x44, 0x24, 0x10]); // mov [rsp+0x10], rax
            self.emit_bytes(&[0x25, 0xF8, 0x00, 0x00, 0x00]); // and eax, 0xf8
            self.emit_bytes(&[0x48, 0x89, 0x44, 0x24, 0x18]); // mov [rsp+0x18], rax

            self.emit_uart_char(b'v');
            self.emit_uart_char(b's');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x44, 0x24, 0x10]); // mov rax, [rsp+0x10]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'v');
            self.emit_uart_char(b'o');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x44, 0x24, 0x18]); // mov rax, [rsp+0x18]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'v');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x44, 0x24, 0x18]); // mov rax, [rsp+0x18]
            self.emit_bytes(&[0x4C, 0x01, 0xE8]); // add rax, r13
            self.emit_bytes(&[0x48, 0x8B, 0x00]); // mov rax, [rax]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_bytes(&[0x48, 0x83, 0xC4, 0x20]); // add rsp, 0x20

            self.emit_uart_char(b'4');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x1D]); // mov rbx, [rip+disp32]
            let pt4_load_disp_pos_local = self.code.len();
            self.emit_bytes(&[0, 0, 0, 0]);
            pt4_load_disp_positions.push(pt4_load_disp_pos_local);
            self.emit_bytes(&[0x48, 0x8B, 0x83, 0xF8, 0x0F, 0x00, 0x00]); // mov rax, [rbx+0xff8]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'3');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x1D]); // mov rbx, [rip+disp32]
            let pt4_load_disp_pos_local = self.code.len();
            self.emit_bytes(&[0, 0, 0, 0]);
            pt4_load_disp_positions.push(pt4_load_disp_pos_local);
            self.emit_bytes(&[0x48, 0x8B, 0x83, 0xF8, 0x0F, 0x00, 0x00]); // mov rax, [rbx+0xff8]
            self.emit_bytes(&[0x48, 0x25, 0x00, 0xF0, 0xFF, 0xFF]); // and rax, 0xfffff000
            self.emit_bytes(&[0x48, 0x8B, 0x80, 0xF0, 0x0F, 0x00, 0x00]); // mov rax, [rax+0xff0]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'2');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x1D]); // mov rbx, [rip+disp32]
            let pt4_load_disp_pos_local = self.code.len();
            self.emit_bytes(&[0, 0, 0, 0]);
            pt4_load_disp_positions.push(pt4_load_disp_pos_local);
            self.emit_bytes(&[0x48, 0x8B, 0x83, 0xF8, 0x0F, 0x00, 0x00]); // mov rax, [rbx+0xff8]
            self.emit_bytes(&[0x48, 0x25, 0x00, 0xF0, 0xFF, 0xFF]); // and rax, 0xfffff000
            self.emit_bytes(&[0x48, 0x8B, 0x80, 0xF0, 0x0F, 0x00, 0x00]); // mov rax, [rax+0xff0]
            self.emit_bytes(&[0x48, 0x25, 0x00, 0xF0, 0xFF, 0xFF]); // and rax, 0xfffff000
            self.emit_bytes(&[0x48, 0x8B, 0x40, 0x08]); // mov rax, [rax+0x08]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'G');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x0F, 0x20, 0xE0]); // mov rax, cr4
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'F');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0xB9, 0x80, 0x00, 0x00, 0xC0]); // mov ecx, 0xC0000080
            self.emit_bytes(&[0x0F, 0x32]); // rdmsr
            self.emit_bytes(&[0x48, 0xC1, 0xE2, 0x20]); // shl rdx, 32
            self.emit_bytes(&[0x48, 0x09, 0xD0]); // or rax, rdx
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            // Read first 16 bytes at kernel entry to validate the mapping.
            self.emit_uart_char(b'I');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0xB8]); // mov rax, kernel_entry
            self.emit_u64(self.kernel_entry);
            self.emit_bytes(&[0x48, 0x8B, 0x00]); // mov rax, [rax]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'J');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0xB8]); // mov rax, kernel_entry+8
            self.emit_u64(self.kernel_entry + 8);
            self.emit_bytes(&[0x48, 0x8B, 0x00]); // mov rax, [rax]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'K');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0xB8]); // mov rax, kernel_entry+0x40
            self.emit_u64(self.kernel_entry + 0x40);
            self.emit_bytes(&[0x48, 0x8B, 0x00]); // mov rax, [rax]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');

            self.emit_uart_char(b'L');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0xB8]); // mov rax, kernel_entry+0x48
            self.emit_u64(self.kernel_entry + 0x48);
            self.emit_bytes(&[0x48, 0x8B, 0x00]); // mov rax, [rax]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b'\r');
            self.emit_uart_char(b'\n');

            self.emit_bytes(&[
                0x41, 0x5C, // pop r12
                0x41, 0x5B, // pop r11
                0x5D, // pop rbp
                0x5F, // pop rdi
                0x5E, // pop rsi
                0x5B, // pop rbx
                0x5A, // pop rdx
                0x59, // pop rcx
                0x58, // pop rax
            ]);
            self.emit_bytes(&[0xC3]); // retq

            let print_hex_start = self.code.len();
            self.emit_bytes(&[0x49, 0x89, 0xC2]); // mov r10, rax
            self.emit_bytes(&[0x48, 0x8D, 0x1D]); // lea rbx, [rip+disp32]
            let table_disp_pos = self.code.len();
            self.emit_bytes(&[0, 0, 0, 0]);
            self.emit_bytes(&[0x66, 0xBA, 0xF8, 0x03]); // mov dx, 0x3f8 (COM1)
            self.emit_bytes(&[0xB9, 0x10, 0x00, 0x00, 0x00]); // mov ecx, 16
            let loop_start = self.code.len();
            self.emit_bytes(&[0x4D, 0x89, 0xD0]); // mov r8, r10
            self.emit_bytes(&[0x49, 0xC1, 0xE8, 0x3C]); // shr r8, 60
            self.emit_bytes(&[0x44, 0x88, 0xC0]); // mov al, r8b
            self.emit_bytes(&[0xD7]); // xlatb
            self.emit_bytes(&[0xEE]); // out dx, al
            self.emit_bytes(&[0x49, 0xC1, 0xE2, 0x04]); // shl r10, 4
            self.emit_bytes(&[0x48, 0xFF, 0xC9]); // dec rcx
            self.emit_bytes(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jnz rel32
            let loop_disp_pos = self.code.len() - 4;
            self.emit_bytes(&[0xC3]); // ret
            let table_start = self.code.len();
            self.emit_bytes(b"0123456789ABCDEF");

            let table_next = table_disp_pos + 4;
            let table_disp = (table_start as i32 - table_next as i32) as i32;
            self.code[table_disp_pos..table_disp_pos + 4]
                .copy_from_slice(&table_disp.to_le_bytes());

            let loop_next = loop_disp_pos + 4;
            let loop_disp = (loop_start as i32 - loop_next as i32) as i32;
            self.code[loop_disp_pos..loop_disp_pos + 4].copy_from_slice(&loop_disp.to_le_bytes());

            if let Some(disp_pos) = jmp_stub_disp_pos {
                let next = disp_pos + 4;
                let rel = (stub_offset as i64 - next as i64) as i32;
                self.code[disp_pos..disp_pos + 4].copy_from_slice(&rel.to_le_bytes());
            }

            gp_handler_offset = Some(self.code.len());
            self.emit_bytes(&[0x49, 0x89, 0xC7]); // mov r15, rax
            self.emit_uart_char(b'A');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x4C, 0x89, 0xF8]); // mov rax, r15
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'G');
            self.emit_uart_char(b'E');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x04, 0x24]); // mov rax, [rsp]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'R');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x44, 0x24, 0x08]); // mov rax, [rsp+8]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'C');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x44, 0x24, 0x10]); // mov rax, [rsp+16]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'S');
            self.emit_uart_char(b'P');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x44, 0x24, 0x20]); // mov rax, [rsp+32]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'S');
            self.emit_uart_char(b'S');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x44, 0x24, 0x28]); // mov rax, [rsp+40]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'D');
            self.emit_uart_char(b'S');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x66, 0x8C, 0xD8]); // mov ax, ds
            self.emit_bytes(&[0x0F, 0xB7, 0xC0]); // movzx eax, ax
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'E');
            self.emit_uart_char(b'S');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x66, 0x8C, 0xC0]); // mov ax, es
            self.emit_bytes(&[0x0F, 0xB7, 0xC0]); // movzx eax, ax
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'F');
            self.emit_uart_char(b'S');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x66, 0x8C, 0xE0]); // mov ax, fs
            self.emit_bytes(&[0x0F, 0xB7, 0xC0]); // movzx eax, ax
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'G');
            self.emit_uart_char(b'S');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x66, 0x8C, 0xE8]); // mov ax, gs
            self.emit_bytes(&[0x0F, 0xB7, 0xC0]); // movzx eax, ax
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b'\r');
            self.emit_uart_char(b'\n');
            self.emit_bytes(&[0xF4, 0xEB, 0xFD]); // hlt; jmp -3

            pf_handler_offset = Some(self.code.len());
            self.emit_uart_char(b'P');
            self.emit_uart_char(b'E');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x04, 0x24]); // mov rax, [rsp]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b' ');
            self.emit_uart_char(b'R');
            self.emit_uart_char(b'=');
            self.emit_bytes(&[0x48, 0x8B, 0x44, 0x24, 0x08]); // mov rax, [rsp+8]
            emit_call(self, &mut call_positions);
            self.emit_uart_char(b'\r');
            self.emit_uart_char(b'\n');
            self.emit_bytes(&[0xF4, 0xEB, 0xFD]); // hlt; jmp -3

            for pos in &call_positions {
                let next = pos + 4;
                let rel = (print_hex_start as i64 - next as i64) as i32;
                self.code[*pos..*pos + 4].copy_from_slice(&rel.to_le_bytes());
            }
        }

        let code_len = self.code.len();
        let data_start = crate::system::align_up(code_len as u64, PAGE_SIZE as u64) as usize;
        if data_start > code_len {
            self.code.resize(data_start, 0);
        }
        let padded_code_len = self.code.len();
        let lea_next = lea_disp_pos + 4;
        let stack_start_offset = padded_code_len as i64 - lea_next as i64;
        let stack_start_disp = (stack_start_offset as i32).to_le_bytes();
        self.code[lea_disp_pos..lea_disp_pos + 4].copy_from_slice(&stack_start_disp);

        let je_next = je_disp_pos + 1;
        let je_disp = (no_copy_offset as i32 - je_next as i32) as i8;
        self.code[je_disp_pos] = je_disp as u8;
        let ja_next = ja_disp_pos + 1;
        let ja_disp = (no_copy_offset as i32 - ja_next as i32) as i8;
        self.code[ja_disp_pos] = ja_disp as u8;

        let modulep_phys = self.modulep;
        let kernend_phys = self.kernend;

        let mut stack = Vec::new();
        // Align stack layout to 8-byte slots for pop order: rsi, rdi, rcx, rax.
        stack.extend_from_slice(&self.efi_memmap_src.to_le_bytes());
        stack.extend_from_slice(&self.efi_memmap_dst.to_le_bytes());
        stack.extend_from_slice(&self.efi_memmap_len.to_le_bytes());
        stack.extend_from_slice(&0u64.to_le_bytes()); // pt4 placeholder
        stack.extend_from_slice(&entry_addr.to_le_bytes());
        stack.extend_from_slice(&modulep_offset32.to_le_bytes());
        stack.extend_from_slice(&0u32.to_le_bytes()); // pad to 8 bytes
        stack.extend_from_slice(&kernend_offset32.to_le_bytes());
        stack.extend_from_slice(&0u32.to_le_bytes()); // pad to 8 bytes

        debug_assert_eq!(stack.len(), 56);
        let btext_rsp_disp = 0i32;
        self.code[btext_rsp_disp_pos..btext_rsp_disp_pos + 4]
            .copy_from_slice(&btext_rsp_disp.to_le_bytes());
        if log::log_enabled!(log::Level::Debug) {
            log::debug!(
                "tramp-stack: memmap_src=0x{:x} memmap_dst=0x{:x} memmap_len=0x{:x} pt4=0x{:x} entry=0x{:x} modulep_phys=0x{:x} kernend_phys=0x{:x} modulep_off=0x{:x} kernend_off=0x{:x} kern_load=0x{:x} staging=0x{:x}",
                self.efi_memmap_src,
                self.efi_memmap_dst,
                self.efi_memmap_len,
                0u64,
                entry_addr,
                modulep_phys,
                kernend_phys,
                modulep_offset,
                kernend_offset,
                self.kernel_phys_base,
                self.staging_base
            );
            log::debug!(
            "tramp-handoff: entry=0x{:x} modulep_off=0x{:x} kernend_off=0x{:x} modulep_phys=0x{:x} kernend_phys=0x{:x} modulep_base=staging howto=0x{:x}",
                entry_addr,
                modulep_offset,
                kernend_offset,
                modulep_phys,
                kernend_phys,
                self.howto
            );
            log::debug!(
                "tramp-handoff-kernel: kernel_phys=0x{:x} staging=0x{:x} modulep_off=0x{:x} kernend_off=0x{:x}",
                self.kernel_phys_base,
                self.staging_base,
                modulep_offset,
                kernend_offset
            );
            let boot_base = self.boot_addr;
            log::debug!(
                "tramp-offsets: base=0x{:x} A=0x{:x} g=0x{:x} c=0x{:x} r=0x{:x} R=0x{:x} s=0x{:x} d=0x{:x} B=0x{:x} C=0x{:x} entry_load=0x{:x} rsp_set=0x{:x} jmp=0x{:x}",
                boot_base,
                boot_base + off_a.unwrap_or(0) as u64,
                boot_base + off_g.unwrap_or(0) as u64,
                boot_base + off_c.unwrap_or(0) as u64,
                boot_base + off_r.unwrap_or(0) as u64,
                boot_base + off_cap_r.unwrap_or(0) as u64,
                boot_base + off_s.unwrap_or(0) as u64,
                boot_base + off_d.unwrap_or(0) as u64,
                boot_base + off_b.unwrap_or(0) as u64,
                boot_base + off_c_jump.unwrap_or(0) as u64,
                boot_base + off_entry_load.unwrap_or(0) as u64,
                boot_base + off_rsp_set.unwrap_or(0) as u64,
                boot_base + off_jmp.unwrap_or(0) as u64
            );
        }

        self.data.extend_from_slice(&stack);
        let gdt_offset = self.data.len();
        // Match FreeBSD amd64 GDT layout (GCODE_SEL=4, GDATA_SEL=5).
        for entry in [
            0u64,                  // null
            0u64,                  // null
            0u64,                  // GUFS32_SEL
            0u64,                  // GUGS32_SEL
            0x00AF9A000000FFFFu64, // GCODE_SEL (0x20)
            0x00AF92000000FFFFu64, // GDATA_SEL (0x28)
        ] {
            self.data.extend_from_slice(&entry.to_le_bytes());
        }
        let gdtr_offset = self.data.len();
        self.data.extend_from_slice(&[0u8; 10]);
        let cs_scratch_offset = self.data.len();
        self.data.extend_from_slice(&[0u8; 16]);
        let pt4_slot_offset = self.data.len();
        self.data.extend_from_slice(&0u64.to_le_bytes());
        self.align_data(PAGE_SIZE);

        let pt4_offset = self.data.len();
        let pt4_phys = self.boot_addr + padded_code_len as u64 + pt4_offset as u64;
        debug_assert_eq!(pt4_phys % PAGE_SIZE as u64, 0);

        self.data.extend_from_slice(&vec![0u8; PAGE_SIZE * 9]);

        let stack_pt4_offset = 24;
        self.data[stack_pt4_offset..stack_pt4_offset + 8]
            .copy_from_slice(&pt4_phys.to_le_bytes());
        if log::log_enabled!(log::Level::Debug) {
            log::debug!("tramp-stack: patched pt4=0x{:x}", pt4_phys);
            if let Some(lgdt_off) = lgdt_offset {
                log::debug!(
                    "tramp-gdt: lgdt_off=0x{:x} lgdt=0x{:x}",
                    lgdt_off,
                    self.boot_addr + lgdt_off as u64
                );
            }
        }

        let pt4_slot_phys = self.boot_addr + padded_code_len as u64 + pt4_slot_offset as u64;
        let store_next = pt4_store_disp_pos + 4;
        let store_rel = (pt4_slot_phys as i64 - (self.boot_addr as i64 + store_next as i64)) as i32;
        self.code[pt4_store_disp_pos..pt4_store_disp_pos + 4]
            .copy_from_slice(&store_rel.to_le_bytes());
        for load_pos in pt4_load_disp_positions {
            let next = load_pos + 4;
            let rel = (pt4_slot_phys as i64 - (self.boot_addr as i64 + next as i64)) as i32;
            self.code[load_pos..load_pos + 4].copy_from_slice(&rel.to_le_bytes());
        }
        if let Some(gdtr_pos) = gdtr_disp_pos {
            let gdt_phys = self.boot_addr + padded_code_len as u64 + gdt_offset as u64;
            let gdtr_phys = self.boot_addr + padded_code_len as u64 + gdtr_offset as u64;
            let next = gdtr_pos + 4;
            let rel = (gdtr_phys as i64 - (self.boot_addr as i64 + next as i64)) as i32;
            self.code[gdtr_pos..gdtr_pos + 4].copy_from_slice(&rel.to_le_bytes());

            let limit = (6 * 8 - 1) as u16;
            self.data[gdtr_offset..gdtr_offset + 2].copy_from_slice(&limit.to_le_bytes());
            self.data[gdtr_offset + 2..gdtr_offset + 10].copy_from_slice(&gdt_phys.to_le_bytes());
        }
        if let (Some(disp_pos), Some(label_pos)) = (cs_reload_disp_pos, cs_reload_label) {
            let next = disp_pos + 4;
            let rel = (label_pos as i64 - next as i64) as i32;
            self.code[disp_pos..disp_pos + 4].copy_from_slice(&rel.to_le_bytes());
        }
        if let (Some(disp_pos), Some(label_pos)) =
            (cs_reload_skip_disp_pos, cs_reload_done_label)
        {
            let next = disp_pos + 4;
            let rel = (label_pos as i64 - next as i64) as i32;
            self.code[disp_pos..disp_pos + 4].copy_from_slice(&rel.to_le_bytes());
        }
        if let (Some(disp_pos), Some(label_pos)) =
            (cs_reload_done_disp_pos, cs_reload_done_label)
        {
            let next = disp_pos + 4;
            let rel = (label_pos as i64 - next as i64) as i32;
            self.code[disp_pos..disp_pos + 4].copy_from_slice(&rel.to_le_bytes());
        }
        if let Some(disp_pos) = cs_reload_scratch_disp_pos {
            let scratch_phys = self.boot_addr + padded_code_len as u64 + cs_scratch_offset as u64;
            let next = disp_pos + 4;
            let rel = (scratch_phys as i64 - (self.boot_addr as i64 + next as i64)) as i32;
            self.code[disp_pos..disp_pos + 4].copy_from_slice(&rel.to_le_bytes());
        }

        if self.debug_trampoline {
            let idt_offset = self.data.len();
            let idt_len = 16 * 16;
            self.data.extend_from_slice(&vec![0u8; idt_len]);
            let idtr_offset = self.data.len();
            self.data.extend_from_slice(&[0u8; 10]);

            let idt_phys = self.boot_addr + padded_code_len as u64 + idt_offset as u64;
            let idtr_phys = self.boot_addr + padded_code_len as u64 + idtr_offset as u64;

            if let Some(disp_pos) = idtr_disp_pos {
                let next = disp_pos + 4;
                let rel = (idtr_phys as i64 - (self.boot_addr as i64 + next as i64)) as i32;
                self.code[disp_pos..disp_pos + 4].copy_from_slice(&rel.to_le_bytes());
            }

            let limit = (idt_len - 1) as u16;
            self.data[idtr_offset..idtr_offset + 2].copy_from_slice(&limit.to_le_bytes());
            self.data[idtr_offset + 2..idtr_offset + 10].copy_from_slice(&idt_phys.to_le_bytes());

            let gp_handler = self.boot_addr + gp_handler_offset.unwrap_or(0) as u64;
            let pf_handler = self.boot_addr + pf_handler_offset.unwrap_or(0) as u64;
            let code_sel: u16 = 0x20;
            let type_attr: u8 = 0x8E;

            let mut write_idt = |vec: usize, handler: u64| {
                let base = idt_offset + vec * 16;
                let off_lo = handler as u16;
                let off_mid = (handler >> 16) as u16;
                let off_hi = (handler >> 32) as u32;
                self.data[base..base + 2].copy_from_slice(&off_lo.to_le_bytes());
                self.data[base + 2..base + 4].copy_from_slice(&code_sel.to_le_bytes());
                self.data[base + 4] = 0;
                self.data[base + 5] = type_attr;
                self.data[base + 6..base + 8].copy_from_slice(&off_mid.to_le_bytes());
                self.data[base + 8..base + 12].copy_from_slice(&off_hi.to_le_bytes());
                self.data[base + 12..base + 16].fill(0);
            };

            write_idt(13, gp_handler);
            write_idt(14, pf_handler);
        }

        let tables = &mut self.data[pt4_offset..pt4_offset + PAGE_SIZE * 9];
        let (pt4, rest) = tables.split_at_mut(PAGE_SIZE);
        let (pt3_l, rest) = rest.split_at_mut(PAGE_SIZE);
        let (pt3_u, rest) = rest.split_at_mut(PAGE_SIZE);
        let (pt2_l, pt2_u) = rest.split_at_mut(PAGE_SIZE * 4);

        let pa_pt3_l = pt4_phys + PAGE_SIZE as u64 * 1;
        let pa_pt3_u = pt4_phys + PAGE_SIZE as u64 * 2;
        let pa_pt2_l0 = pt4_phys + PAGE_SIZE as u64 * 3;
        let pa_pt2_l1 = pt4_phys + PAGE_SIZE as u64 * 4;
        let pa_pt2_l2 = pt4_phys + PAGE_SIZE as u64 * 5;
        let pa_pt2_l3 = pt4_phys + PAGE_SIZE as u64 * 6;
        let pa_pt2_u0 = pt4_phys + PAGE_SIZE as u64 * 7;
        let pa_pt2_u1 = pt4_phys + PAGE_SIZE as u64 * 8;

        if log::log_enabled!(log::Level::Debug) {
            let stack_ptr = self.boot_addr + padded_code_len as u64;
            log::debug!(
                "tramp-pt: stack=0x{:x} pt4=0x{:x} pt3_l=0x{:x} pt3_u=0x{:x} pt2_l=[0x{:x},0x{:x},0x{:x},0x{:x}] pt2_u=[0x{:x},0x{:x}]",
                stack_ptr,
                pt4_phys,
                pa_pt3_l,
                pa_pt3_u,
                pa_pt2_l0,
                pa_pt2_l1,
                pa_pt2_l2,
                pa_pt2_l3,
                pa_pt2_u0,
                pa_pt2_u1
            );
        }

        write_u64(pt4, 0, pa_pt3_l | PG_V | PG_RW);
        write_u64(pt3_l, 0, pa_pt2_l0 | PG_V | PG_RW);
        write_u64(pt3_l, 1, pa_pt2_l1 | PG_V | PG_RW);
        write_u64(pt3_l, 2, pa_pt2_l2 | PG_V | PG_RW);
        write_u64(pt3_l, 3, pa_pt2_l3 | PG_V | PG_RW);

        for i in 0..(4 * NPDEPG) {
            let pa = (i as u64) << PDRSHIFT;
            write_u64(pt2_l, i, pa | PG_V | PG_RW | PG_PS);
        }

        write_u64(pt4, NPML4EPG - 1, pa_pt3_u | PG_V | PG_RW);
        write_u64(pt3_u, NPDPEPG - 2, pa_pt2_u0 | PG_V | PG_RW);
        write_u64(pt3_u, NPDPEPG - 1, pa_pt2_u1 | PG_V | PG_RW);

        let kernel_map_base = self
            .kernel_phys_base
            .saturating_sub(crate::types::KERNEL_PHYS_BASE);
        write_u64(pt2_u, 0, PG_V | PG_RW | PG_PS);
        for i in 1..(2 * NPDEPG) {
            let pa = self.staging_base + (i as u64) * NBPDR;
            write_u64(pt2_u, i, pa | PG_V | PG_RW | PG_PS);
        }

        if log::log_enabled!(log::Level::Debug) {
            let pt2_u0 = read_u64(pt2_u, 0);
            let pt2_u1 = read_u64(pt2_u, 1);
            log::debug!(
                "pt2-u0[0]=0x{:016x} (maps pa=0x{:x}) pt2-u0[1]=0x{:016x} (maps pa=0x{:x}) kernel_phys=0x{:x} map_base=0x{:x}",
                pt2_u0,
                pt2_u0 & !0xfff,
                pt2_u1,
                pt2_u1 & !0xfff,
                self.kernel_phys_base,
                kernel_map_base
            );
        }

        Ok(())
    }

    // Instruction emission helpers
    fn emit_byte(&mut self, byte: u8) {
        self.code.push(byte);
    }

    fn emit_bytes(&mut self, bytes: &[u8]) {
        self.code.extend_from_slice(bytes);
    }

    fn emit_uart_char(&mut self, ch: u8) {
        self.code.extend_from_slice(&[
            0x66, 0xBA, 0xF8, 0x03, // mov dx, 0x3f8 (COM1)
            0xB0, ch,               // mov al, ch
            0xEE,                   // out dx, al
        ]);
    }

    fn emit_u64(&mut self, value: u64) {
        self.code.extend_from_slice(&value.to_le_bytes());
    }

    // Data section helpers
    fn data_byte(&mut self, byte: u8) {
        self.data.push(byte);
    }

    fn align_data(&mut self, alignment: usize) {
        let padding = (alignment - (self.data.len() % alignment)) % alignment;
        for _ in 0..padding {
            self.data_byte(0);
        }
    }

}

fn write_u64(buf: &mut [u8], index: usize, value: u64) {
    let offset = index * 8;
    if offset + 8 <= buf.len() {
        buf[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
}

fn read_u64(buf: &[u8], index: usize) -> u64 {
    let offset = index * 8;
    if offset + 8 <= buf.len() {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&buf[offset..offset + 8]);
        u64::from_le_bytes(bytes)
    } else {
        0
    }
}

/// Generate simple trampoline code (fallback)
pub fn generate_simple_trampoline(kernel_entry: u64) -> Result<Vec<u8>> {
    let mut asm = Vec::new();

    // Save registers
    asm.extend_from_slice(&[0x50]); // push rax
    asm.extend_from_slice(&[0x53]); // push rbx
    asm.extend_from_slice(&[0x51]); // push rcx
    asm.extend_from_slice(&[0x52]); // push rdx
    asm.extend_from_slice(&[0x56]); // push rsi
    asm.extend_from_slice(&[0x57]); // push rdi

    // Load kernel entry point
    asm.extend_from_slice(&[0x48, 0xb8]); // mov rax, kernel_entry
    asm.extend_from_slice(&kernel_entry.to_le_bytes());

    // Jump to kernel
    asm.extend_from_slice(&[0xff, 0xe0]); // jmp rax

    // Restore registers (unreachable)
    asm.extend_from_slice(&[0x5f]); // pop rdi
    asm.extend_from_slice(&[0x5e]); // pop rsi
    asm.extend_from_slice(&[0x5a]); // pop rdx
    asm.extend_from_slice(&[0x59]); // pop rcx
    asm.extend_from_slice(&[0x5b]); // pop rbx
    asm.extend_from_slice(&[0x58]); // pop rax

    Ok(asm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BOOT_PHYS_BASE;

    #[test]
    fn test_trampoline_stack_layout() -> Result<()> {
        let kernel_entry = 0xffff_ffff_8000_1000;
        let mut assembler = BootAssembler::new(kernel_entry);
        let staging_base = 0x4000_0000;
        let kernel_phys_base = 0x4200_0000;
        assembler.set_staging_base(staging_base);
        assembler.set_kernel_phys_base(kernel_phys_base);
        assembler.set_boot_addr(staging_base + BOOT_PHYS_BASE);
        assembler.set_modulep(kernel_phys_base + 0x300000);
        assembler.set_kernend(kernel_phys_base + 0x400000);
        assembler.set_efi_memmap(0x1111, 0x2222, 0x3333);
        assembler.assemble()?;

        let data = assembler.data();
        assert!(data.len() >= 56);
        assert_eq!(u64::from_le_bytes(data[0..8].try_into().unwrap()), 0x1111);
        assert_eq!(u64::from_le_bytes(data[8..16].try_into().unwrap()), 0x2222);
        assert_eq!(u64::from_le_bytes(data[16..24].try_into().unwrap()), 0x3333);
        assert_ne!(u64::from_le_bytes(data[24..32].try_into().unwrap()), 0);
        assert_eq!(
            u64::from_le_bytes(data[32..40].try_into().unwrap()),
            kernel_entry
        );
        assert_eq!(
            u32::from_le_bytes(data[40..44].try_into().unwrap()),
            0x02300000
        );
        assert_eq!(u32::from_le_bytes(data[44..48].try_into().unwrap()), 0);
        assert_eq!(
            u32::from_le_bytes(data[48..52].try_into().unwrap()),
            0x00400000
        );
        assert_eq!(u32::from_le_bytes(data[52..56].try_into().unwrap()), 0);

        let pt4_offset = PAGE_SIZE;
        let pt2_u_offset = pt4_offset + PAGE_SIZE * 7;
        let pt2_u0 = u64::from_le_bytes(
            data[pt2_u_offset..pt2_u_offset + 8]
                .try_into()
                .unwrap(),
        );
        let pt2_u1 = u64::from_le_bytes(
            data[pt2_u_offset + 8..pt2_u_offset + 16]
                .try_into()
                .unwrap(),
        );
        assert_eq!(pt2_u0 & !0xfff, 0);
        assert_eq!(pt2_u1 & !0xfff, staging_base + (1 << 21));

        Ok(())
    }
}
