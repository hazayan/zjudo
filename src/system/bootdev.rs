use crate::error::Result;
use crate::zfs::ZfsInterface;
use std::fs;

/// Detect a default boot device from /proc/cmdline or available ZFS pools
pub fn detect_bootdev() -> Result<Option<String>> {
    if let Ok(cmdline) = fs::read_to_string("/proc/cmdline") {
        if let Some(root) = parse_root_from_cmdline(&cmdline) {
            return Ok(Some(root));
        }
    }

    let zfs = ZfsInterface::new();
    if zfs.check_zfs_available() {
        if let Ok(pools) = zfs.list_pools() {
            if !pools.is_empty() {
                return Ok(Some("zfs:".to_string()));
            }
        }
    }

    Ok(None)
}

fn parse_root_from_cmdline(cmdline: &str) -> Option<String> {
    for token in cmdline.split_whitespace() {
        if let Some(value) = token.strip_prefix("root=") {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_root_from_cmdline() {
        let cmdline = "console=ttyS0 root=/dev/sda1 ro quiet";
        assert_eq!(parse_root_from_cmdline(cmdline), Some("/dev/sda1".to_string()));
        assert_eq!(parse_root_from_cmdline("root=zfs:zroot/ROOT/default"), Some("zfs:zroot/ROOT/default".to_string()));
        assert_eq!(parse_root_from_cmdline("no-root-here"), None);
    }
}
