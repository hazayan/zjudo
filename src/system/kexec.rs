use crate::error::{BootError, Result};
use crate::types::KexecSegment;
use libc::{c_void, syscall, SYS_kexec_load};
use std::ptr;

/// Kexec file operation flags (from linux/kexec.h)
const KEXEC_FILE_UNLOAD: u64 = 0;

/// Load kexec segments
pub fn kexec_load(
    entry: u64,
    nr_segments: usize,
    segments: &[KexecSegment],
    flags: u64,
) -> Result<()> {
    let result = unsafe {
        syscall(
            SYS_kexec_load,
            entry,
            nr_segments,
            segments.as_ptr(),
            flags,
        )
    };

    if result < 0 {
        let errno = unsafe { *libc::__errno_location() };
        let err_msg = format!("kexec_load failed: {}", std::io::Error::from_raw_os_error(errno));
        return Err(BootError::Kexec(err_msg));
    }

    Ok(())
}

/// Unload kexec segments
pub fn kexec_unload() -> Result<()> {
    let result = unsafe {
        syscall(
            SYS_kexec_load,
            0 as u64,
            0 as usize,
            ptr::null::<c_void>(),
            KEXEC_FILE_UNLOAD,
        )
    };

    if result < 0 {
        let errno = unsafe { *libc::__errno_location() };
        let err_msg = format!("kexec_unload failed: {}", std::io::Error::from_raw_os_error(errno));
        return Err(BootError::Kexec(err_msg));
    }

    Ok(())
}

/// Shutdown system (graceful)
pub fn shutdown() -> Result<()> {
    let result = unsafe { libc::reboot(libc::LINUX_REBOOT_CMD_KEXEC) };

    if result < 0 {
        let errno = unsafe { *libc::__errno_location() };
        let err_msg = format!("shutdown failed: {}", std::io::Error::from_raw_os_error(errno));
        return Err(BootError::System(err_msg));
    }

    Ok(())
}

/// Force immediate shutdown
pub fn forced_shutdown() -> Result<()> {
    // First try to unload any kexec segments
    let _ = kexec_unload();

    // Then force reboot
    let result = unsafe { libc::reboot(libc::LINUX_REBOOT_CMD_RESTART2) };

    if result < 0 {
        let errno = unsafe { *libc::__errno_location() };
        let err_msg = format!("forced_shutdown failed: {}", std::io::Error::from_raw_os_error(errno));
        return Err(BootError::System(err_msg));
    }

    Ok(())
}

/// Check if kexec is supported
pub fn kexec_supported() -> bool {
    // Try to perform a no-op kexec_load to check support
    let result = unsafe {
        syscall(
            SYS_kexec_load,
            0 as u64,
            0 as usize,
            ptr::null::<c_void>(),
            KEXEC_FILE_UNLOAD,
        )
    };

    // Either success or EINVAL (which means kexec is supported but we gave invalid args)
    result == 0 || unsafe { *libc::__errno_location() } != libc::ENOSYS
}
