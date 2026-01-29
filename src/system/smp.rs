use crate::error::{BootError, Result};
use crate::system::read_file_string;
use std::path::Path;
use std::thread;
use std::time::Duration;

const CPU_SYSFS: &str = "/sys/devices/system/cpu";

pub fn offline_secondary_cpus() -> Result<()> {
    let online_path = Path::new(CPU_SYSFS).join("online");
    if !online_path.exists() {
        return Err(BootError::System(
            "smp: cpu online list missing; cannot enforce single-CPU handoff".to_string(),
        ));
    }

    let online = read_file_string(&online_path).map_err(|err| {
        BootError::System(format!(
            "smp: failed to read cpu online list: {err}"
        ))
    })?;
    let cpu_ids = parse_cpu_list(online.trim());
    if cpu_ids.is_empty() {
        return Err(BootError::System(
            "smp: empty cpu online list; cannot enforce single-CPU handoff".to_string(),
        ));
    }

    let mut saw_secondary = false;
    for cpu_id in cpu_ids {
        if cpu_id == 0 {
            continue;
        }
        saw_secondary = true;
        let cpu_online_path = Path::new(CPU_SYSFS)
            .join(format!("cpu{cpu_id}"))
            .join("online");
        if !cpu_online_path.exists() {
            return Err(BootError::System(format!(
                "smp: cpu{cpu_id} has no online control; cannot enforce single-CPU handoff"
            )));
        }

        let current = read_file_string(&cpu_online_path).map_err(|err| {
            BootError::System(format!(
                "smp: failed to read cpu{cpu_id} online state: {err}"
            ))
        })?;
        if current.trim() == "0" {
            continue;
        }

        std::fs::write(&cpu_online_path, "0").map_err(|err| {
            BootError::System(format!("smp: failed to offline cpu{cpu_id}: {err}"))
        })?;

        let mut offline = false;
        for _ in 0..20 {
            let state = read_file_string(&cpu_online_path).map_err(|err| {
                BootError::System(format!(
                    "smp: failed to poll cpu{cpu_id} online state: {err}"
                ))
            })?;
            if state.trim() == "0" {
                offline = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        if !offline {
            return Err(BootError::System(format!(
                "smp: cpu{cpu_id} did not offline within timeout"
            )));
        }
    }

    if !saw_secondary {
        log::debug!("smp: only cpu0 online; handoff already single-CPU");
    }

    Ok(())
}

fn parse_cpu_list(list: &str) -> Vec<u32> {
    let mut cpus = Vec::new();
    for part in list.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            if let (Ok(start), Ok(end)) = (start.trim().parse::<u32>(), end.trim().parse::<u32>())
            {
                for cpu in start..=end {
                    cpus.push(cpu);
                }
            }
        } else if let Ok(cpu) = part.parse::<u32>() {
            cpus.push(cpu);
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    cpus
}
