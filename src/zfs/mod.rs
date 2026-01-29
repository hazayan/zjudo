use crate::error::{BootError, Result};
use std::process::Command;

const FREEBSD_BOOTONCE: &str = "freebsd:bootonce";
const FREEBSD_BOOTONCE_USED: &str = "freebsd:bootonce-used";

/// ZFS pool information
#[derive(Debug, Clone)]
pub struct ZfsPool {
    pub name: String,
    pub guid: u64,
    pub state: String,
    pub health: String,
    pub encrypted: bool,
    pub locked: bool,
}

/// ZFS dataset information
#[derive(Debug, Clone)]
pub struct ZfsDataset {
    pub name: String,
    pub mountpoint: String,
    pub used: u64,
    pub available: u64,
    pub referenced: u64,
}

/// ZFS interface for pool management
pub struct ZfsInterface {
    zfs_module_loaded: bool,
}

impl ZfsInterface {
    pub fn new() -> Self {
        Self {
            zfs_module_loaded: false,
        }
    }

    /// Check if ZFS tools are available
    pub fn check_zfs_available(&self) -> bool {
        // Check if zfs command exists
        Command::new("which")
            .arg("zfs")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// Load ZFS kernel module
    pub fn load_zfs_module(&mut self) -> Result<()> {
        if self.zfs_module_loaded {
            return Ok(());
        }

        // Try to load zfs module
        let output = Command::new("modprobe")
            .arg("zfs")
            .output()
            .map_err(|e| BootError::System(format!("Failed to run modprobe: {}", e)))?;

        if !output.status.success() {
            return Err(BootError::System(
                "Failed to load ZFS kernel module".to_string(),
            ));
        }

        self.zfs_module_loaded = true;
        Ok(())
    }

    /// List available ZFS pools
    pub fn list_pools(&self) -> Result<Vec<ZfsPool>> {
        let output = Command::new("zpool")
            .arg("list")
            .arg("-H")
            .arg("-o")
            .arg("name,guid,state,health")
            .output()
            .map_err(|e| BootError::System(format!("Failed to list pools: {}", e)))?;

        if !output.status.success() {
            return Err(BootError::System(
                "Failed to list ZFS pools".to_string(),
            ));
        }

        let mut pools = Vec::new();
        let output_str = String::from_utf8_lossy(&output.stdout);

        for line in output_str.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 4 {
                let pool = ZfsPool {
                    name: parts[0].to_string(),
                    guid: parts[1].parse().unwrap_or(0),
                    state: parts[2].to_string(),
                    health: parts[3].to_string(),
                    encrypted: false, // Need additional check
                    locked: false,    // Need additional check
                };
                pools.push(pool);
            }
        }

        Ok(pools)
    }

    /// Get a ZFS pool property value
    pub fn get_pool_property(&self, pool_name: &str, property: &str) -> Result<Option<String>> {
        let output = Command::new("zpool")
            .arg("get")
            .arg("-H")
            .arg("-o")
            .arg("value")
            .arg(property)
            .arg(pool_name)
            .output()
            .map_err(|e| BootError::System(format!("Failed to query property: {}", e)))?;

        if !output.status.success() {
            return Ok(None);
        }

        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() || value == "-" {
            return Ok(None);
        }

        Ok(Some(value))
    }

    /// Get bootonce dataset name from pool bootenv properties
    pub fn get_bootonce_dataset(&self) -> Result<Option<String>> {
        let pools = self.list_pools()?;
        for pool in pools {
            if let Some(value) = self.get_pool_property(&pool.name, FREEBSD_BOOTONCE)? {
                if let Some(used) = self.get_pool_property(&pool.name, FREEBSD_BOOTONCE_USED)? {
                    if used == value {
                        continue;
                    }
                }
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    /// Check if pool is encrypted and locked
    pub fn check_pool_encryption(&self, pool_name: &str) -> Result<(bool, bool)> {
        // Check encryption property
        let output = Command::new("zpool")
            .arg("get")
            .arg("-H")
            .arg("-o")
            .arg("value")
            .arg("encryption")
            .arg(pool_name)
            .output()
            .map_err(|e| BootError::System(format!("Failed to check encryption: {}", e)))?;

        if !output.status.success() {
            return Ok((false, false)); // Pool might not exist or not support encryption
        }

        let encryption = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let encrypted = encryption == "on" || encryption == "aes-256-ccm" || encryption == "aes-256-gcm";

        // Check if pool is locked (mounted)
        let output = Command::new("zpool")
            .arg("status")
            .arg(pool_name)
            .output()
            .map_err(|e| BootError::System(format!("Failed to check pool status: {}", e)))?;

        let status = String::from_utf8_lossy(&output.stdout);
        let locked = status.contains("LOCKED") || !status.contains("ONLINE");

        Ok((encrypted, locked))
    }

    /// Import ZFS pool (without mounting)
    pub fn import_pool(&self, pool_name: &str, read_only: bool) -> Result<()> {
        let mut args = vec!["import".to_string()];

        if read_only {
            args.push("-o".to_string());
            args.push("readonly=on".to_string());
        }

        args.push("-N".to_string()); // Do not mount filesystems
        args.push(pool_name.to_string());

        let output = Command::new("zpool")
            .args(&args)
            .output()
            .map_err(|e| BootError::System(format!("Failed to import pool: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BootError::System(format!(
                "Failed to import pool {}: {}",
                pool_name, stderr
            )));
        }

        Ok(())
    }

    /// Export ZFS pool
    pub fn export_pool(&self, pool_name: &str) -> Result<()> {
        let output = Command::new("zpool")
            .arg("export")
            .arg(pool_name)
            .output()
            .map_err(|e| BootError::System(format!("Failed to export pool: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BootError::System(format!(
                "Failed to export pool {}: {}",
                pool_name, stderr
            )));
        }

        Ok(())
    }

    /// Get datasets in a pool
    pub fn list_datasets(&self, pool_name: &str) -> Result<Vec<ZfsDataset>> {
        let output = Command::new("zfs")
            .arg("list")
            .arg("-H")
            .arg("-o")
            .arg("name,mountpoint,used,avail,refer")
            .arg("-r")
            .arg(pool_name)
            .output()
            .map_err(|e| BootError::System(format!("Failed to list datasets: {}", e)))?;

        if !output.status.success() {
            return Err(BootError::System(
                "Failed to list ZFS datasets".to_string(),
            ));
        }

        let mut datasets = Vec::new();
        let output_str = String::from_utf8_lossy(&output.stdout);

        for line in output_str.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 5 {
                let dataset = ZfsDataset {
                    name: parts[0].to_string(),
                    mountpoint: parts[1].to_string(),
                    used: parse_zfs_size(parts[2]).unwrap_or(0),
                    available: parse_zfs_size(parts[3]).unwrap_or(0),
                    referenced: parse_zfs_size(parts[4]).unwrap_or(0),
                };
                datasets.push(dataset);
            }
        }

        Ok(datasets)
    }

    /// Find boot environment datasets
    pub fn find_boot_environments(&self, pool_name: &str) -> Result<Vec<ZfsDataset>> {
        let all_datasets = self.list_datasets(pool_name)?;
        let boot_envs: Vec<ZfsDataset> = all_datasets
            .into_iter()
            .filter(|ds| {
                // Look for typical FreeBSD boot environment patterns
                ds.name.contains("ROOT/") || ds.mountpoint.contains("/boot/")
            })
            .collect();

        Ok(boot_envs)
    }
}

/// Normalize a ZFS dataset into a mountfrom string
pub fn format_zfs_mountfrom(dataset: &str) -> String {
    if dataset.starts_with("zfs:") {
        dataset.to_string()
    } else {
        format!("zfs:{}", dataset)
    }
}

/// Parse ZFS size string (e.g., "1.23G", "456M", "789K")
fn parse_zfs_size(size_str: &str) -> Option<u64> {
    if size_str == "-" {
        return Some(0);
    }

    let size_str = size_str.trim();
    if size_str.is_empty() {
        return Some(0);
    }

    // Parse number and suffix
    let mut num_str = String::new();
    let mut suffix = ' ';

    for c in size_str.chars() {
        if c.is_ascii_digit() || c == '.' {
            num_str.push(c);
        } else {
            suffix = c;
            break;
        }
    }

    let num: f64 = num_str.parse().ok()?;
    let multiplier = match suffix.to_ascii_uppercase() {
        'K' => 1024.0,
        'M' => 1024.0 * 1024.0,
        'G' => 1024.0 * 1024.0 * 1024.0,
        'T' => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        'P' => 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0,
        'E' => 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => 1.0, // Assume bytes if no suffix
    };

    Some((num * multiplier) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_zfs_mountfrom() {
        assert_eq!(format_zfs_mountfrom("zroot/ROOT/default"), "zfs:zroot/ROOT/default");
        assert_eq!(format_zfs_mountfrom("zfs:zroot/ROOT/default"), "zfs:zroot/ROOT/default");
    }

    #[test]
    fn test_parse_zfs_size() {
        assert_eq!(parse_zfs_size("123"), Some(123));
        assert_eq!(parse_zfs_size("1.5K"), Some(1536)); // 1.5 * 1024
        assert_eq!(parse_zfs_size("2M"), Some(2 * 1024 * 1024));
        assert_eq!(parse_zfs_size("1.5G"), Some((1.5 * 1024.0 * 1024.0 * 1024.0) as u64));
        assert_eq!(parse_zfs_size("-"), Some(0));
        assert_eq!(parse_zfs_size(""), Some(0));
    }

    #[test]
    fn test_zfs_interface_creation() {
        let zfs = ZfsInterface::new();
        // Just test that it creates without panic
        assert!(!zfs.zfs_module_loaded);
    }
}
