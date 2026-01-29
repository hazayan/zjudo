use thiserror::Error;

#[derive(Error, Debug)]
pub enum BootError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("ELF parsing error: {0}")]
    ElfParse(String),

    #[error("Invalid ELF file: {0}")]
    InvalidElf(String),

    #[error("Module error: {0}")]
    Module(String),

    #[error("Kexec error: {0}")]
    Kexec(String),

    #[error("Memory error: {0}")]
    Memory(String),

    #[error("System error: {0}")]
    System(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Font error: {0}")]
    Font(String),

    #[error("Unsupported feature: {0}")]
    Unsupported(String),

    #[error("Permission denied: {0}")]
    Permission(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
}

impl From<&str> for BootError {
    fn from(s: &str) -> Self {
        BootError::System(s.to_string())
    }
}

pub type Result<T> = std::result::Result<T, BootError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let io_error = BootError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"));
        assert!(format!("{}", io_error).contains("IO error"));

        let elf_error = BootError::ElfParse("invalid header".to_string());
        assert!(format!("{}", elf_error).contains("ELF parsing error"));

        let invalid_elf = BootError::InvalidElf("not 64-bit".to_string());
        assert!(format!("{}", invalid_elf).contains("Invalid ELF file"));

        let kexec_error = BootError::Kexec("not supported".to_string());
        assert!(format!("{}", kexec_error).contains("Kexec error"));

        let permission_error = BootError::Permission("root required".to_string());
        assert!(format!("{}", permission_error).contains("Permission denied"));
    }

    #[test]
    fn test_from_str() {
        let error: BootError = "test error".into();
        match error {
            BootError::System(msg) => assert_eq!(msg, "test error"),
            _ => panic!("Expected BootError::System"),
        }
    }

    #[test]
    fn test_error_conversions() {
        // Test IO error conversion
        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "test");
        let boot_err: BootError = io_err.into();
        match boot_err {
            BootError::Io(_) => (),
            _ => panic!("Expected BootError::Io"),
        }
    }
}
