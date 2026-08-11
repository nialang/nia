use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct CgroupPaths {
    pub unified: Option<PathBuf>,
    pub memory: Option<PathBuf>,
    pub cpu: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResourceSnapshot {
    pub system_memory_bytes: Option<u64>,
    pub system_available_memory_bytes: Option<u64>,
    pub cgroup_memory_limit_bytes: Option<u64>,
    pub cgroup_memory_current_bytes: Option<u64>,
    pub cgroup_cpu_quota: Option<f64>,
    pub cpu_model: Option<String>,
}

impl ResourceSnapshot {
    pub fn effective_memory_limit_bytes(&self) -> Option<u64> {
        [self.system_memory_bytes, self.cgroup_memory_limit_bytes]
            .into_iter()
            .flatten()
            .filter(|value| *value > 0)
            .min()
    }

    pub fn available_memory_bytes(&self) -> Option<u64> {
        let mut candidates = self
            .system_available_memory_bytes
            .into_iter()
            .collect::<Vec<_>>();
        if let (Some(limit), Some(current)) = (
            self.cgroup_memory_limit_bytes,
            self.cgroup_memory_current_bytes,
        ) {
            candidates.push(limit.saturating_sub(current));
        }
        candidates.into_iter().min()
    }
}

fn read_optional(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

pub fn parse_limit(value: Option<&str>) -> Option<u64> {
    let text = value?.trim();
    if text.is_empty() || text == "max" {
        return None;
    }
    text.parse().ok()
}

pub fn parse_proc_memory(value: Option<&str>) -> (Option<u64>, Option<u64>) {
    let mut total = None;
    let mut available = None;
    for line in value.unwrap_or_default().lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        if !matches!(name, "MemTotal" | "MemAvailable") {
            continue;
        }
        let Some(amount) = rest
            .split_whitespace()
            .next()
            .and_then(|field| field.parse::<u64>().ok())
            .and_then(|kilobytes| kilobytes.checked_mul(1024))
        else {
            continue;
        };
        if name == "MemTotal" {
            total = Some(amount);
        } else {
            available = Some(amount);
        }
    }
    (total, available)
}

pub fn parse_cpu_model(value: Option<&str>) -> Option<String> {
    value.unwrap_or_default().lines().find_map(|line| {
        let (name, model) = line.split_once(':')?;
        if !matches!(name.trim(), "model name" | "Hardware") || model.trim().is_empty() {
            return None;
        }
        Some(model.trim().to_owned())
    })
}

pub fn parse_cgroup_paths(value: Option<&str>, cgroup_root: &Path) -> CgroupPaths {
    let mut paths = CgroupPaths {
        unified: None,
        memory: None,
        cpu: None,
    };
    for line in value.unwrap_or_default().lines() {
        let fields = line.splitn(3, ':').collect::<Vec<_>>();
        if fields.len() != 3 {
            continue;
        }
        let relative = fields[2].trim_start_matches('/');
        if fields[0] == "0" && fields[1].is_empty() {
            paths.unified = Some(cgroup_root.join(relative));
            continue;
        }
        let controllers = fields[1].split(',').collect::<Vec<_>>();
        if controllers.contains(&"memory") {
            paths.memory = Some(cgroup_root.join("memory").join(relative));
        }
        if controllers.contains(&"cpu") {
            paths.cpu = Some(cgroup_root.join("cpu").join(relative));
        }
    }
    paths
}

pub fn parse_cpu_max(value: Option<&str>) -> Option<f64> {
    let fields = value
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>();
    if fields.len() != 2 || fields[0] == "max" {
        return None;
    }
    let quota = fields[0].parse::<u64>().ok()?;
    let period = fields[1].parse::<u64>().ok()?;
    (period > 0).then_some(quota as f64 / period as f64)
}

pub fn probe_resources(proc_root: &Path, cgroup_root: &Path) -> ResourceSnapshot {
    let memory = read_optional(&proc_root.join("meminfo"));
    let (total, available) = parse_proc_memory(memory.as_deref());
    let cpu_info = read_optional(&proc_root.join("cpuinfo"));
    let paths = parse_cgroup_paths(
        read_optional(&proc_root.join("self/cgroup")).as_deref(),
        cgroup_root,
    );
    let (memory_limit, memory_current, cpu_quota) = if let Some(unified) = paths.unified {
        (
            parse_limit(read_optional(&unified.join("memory.max")).as_deref()),
            parse_limit(read_optional(&unified.join("memory.current")).as_deref()),
            parse_cpu_max(read_optional(&unified.join("cpu.max")).as_deref()),
        )
    } else {
        let memory_limit = paths.memory.as_ref().and_then(|root| {
            parse_limit(read_optional(&root.join("memory.limit_in_bytes")).as_deref())
        });
        let memory_current = paths.memory.as_ref().and_then(|root| {
            parse_limit(read_optional(&root.join("memory.usage_in_bytes")).as_deref())
        });
        let quota = paths
            .cpu
            .as_ref()
            .and_then(|root| parse_limit(read_optional(&root.join("cpu.cfs_quota_us")).as_deref()));
        let period = paths.cpu.as_ref().and_then(|root| {
            parse_limit(read_optional(&root.join("cpu.cfs_period_us")).as_deref())
        });
        let cpu_quota = quota
            .zip(period)
            .and_then(|(quota, period)| (period > 0).then_some(quota as f64 / period as f64));
        (memory_limit, memory_current, cpu_quota)
    };
    ResourceSnapshot {
        system_memory_bytes: total,
        system_available_memory_bytes: available,
        cgroup_memory_limit_bytes: memory_limit,
        cgroup_memory_current_bytes: memory_current,
        cgroup_cpu_quota: cpu_quota,
        cpu_model: parse_cpu_model(cpu_info.as_deref()),
    }
}

pub fn probe_host_resources() -> ResourceSnapshot {
    probe_resources(Path::new("/proc"), Path::new("/sys/fs/cgroup"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDirectory;

    #[test]
    fn parses_proc_memory_without_order_dependency() {
        let (total, available) = parse_proc_memory(Some(
            "MemAvailable: 512 kB\nIgnored: 4 kB\nMemTotal: 1024 kB\n",
        ));
        assert_eq!(total, Some(1024 * 1024));
        assert_eq!(available, Some(512 * 1024));
    }

    #[test]
    fn parses_cgroup_v1_and_v2_paths() {
        let root = Path::new("/cgroup");
        let v2 = parse_cgroup_paths(Some("0::/user.slice/session.scope\n"), root);
        let v1 = parse_cgroup_paths(Some("4:memory:/build\n3:cpu,cpuacct:/build\n"), root);
        assert_eq!(v2.unified, Some(root.join("user.slice/session.scope")));
        assert_eq!(v1.memory, Some(root.join("memory/build")));
        assert_eq!(v1.cpu, Some(root.join("cpu/build")));
    }

    #[test]
    fn rejects_unbounded_or_invalid_cpu_quota() {
        assert_eq!(parse_cpu_max(Some("max 100000")), None);
        assert_eq!(parse_cpu_max(Some("100000 0")), None);
        assert_eq!(parse_cpu_max(Some("150000 100000")), Some(1.5));
    }

    #[test]
    fn snapshot_uses_tightest_available_memory() {
        let snapshot = ResourceSnapshot {
            system_memory_bytes: Some(16_000),
            system_available_memory_bytes: Some(8_000),
            cgroup_memory_limit_bytes: Some(6_000),
            cgroup_memory_current_bytes: Some(2_000),
            cgroup_cpu_quota: None,
            cpu_model: None,
        };
        assert_eq!(snapshot.effective_memory_limit_bytes(), Some(6_000));
        assert_eq!(snapshot.available_memory_bytes(), Some(4_000));
    }

    #[test]
    fn probes_unified_cgroup_files() {
        let directory = TestDirectory::new("resources");
        directory.write("proc/meminfo", "MemTotal: 1000 kB\nMemAvailable: 600 kB\n");
        directory.write("proc/cpuinfo", "model name: Test CPU\n");
        directory.write("proc/self/cgroup", "0::/job\n");
        directory.write("cgroup/job/memory.max", "512000\n");
        directory.write("cgroup/job/memory.current", "128000\n");
        directory.write("cgroup/job/cpu.max", "200000 100000\n");

        let snapshot = probe_resources(
            &directory.path().join("proc"),
            &directory.path().join("cgroup"),
        );
        assert_eq!(snapshot.system_memory_bytes, Some(1_024_000));
        assert_eq!(snapshot.cgroup_memory_limit_bytes, Some(512_000));
        assert_eq!(snapshot.available_memory_bytes(), Some(384_000));
        assert_eq!(snapshot.cgroup_cpu_quota, Some(2.0));
        assert_eq!(snapshot.cpu_model.as_deref(), Some("Test CPU"));
    }
}
