use std::fs;
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::resources::probe_host_resources;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineMetadata {
    pub runner_class: Option<String>,
    pub system: String,
    pub platform: String,
    pub architecture: String,
    pub cpu_model: Option<String>,
    pub logical_cpus: Option<usize>,
    pub affinity_cpus: Option<usize>,
    pub cgroup_cpu_quota: Option<f64>,
    pub effective_cpu_limit: Option<f64>,
    pub system_memory_bytes: Option<u64>,
    pub cgroup_memory_limit_bytes: Option<u64>,
    pub effective_memory_limit_bytes: Option<u64>,
}

fn command_output(command: &str, argument: &str) -> Option<String> {
    let output = Command::new(command).arg(argument).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn affinity_cpus() -> Option<usize> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let list = status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))?
        .trim();
    list.split(',').try_fold(0usize, |count, range| {
        if let Some((start, end)) = range.split_once('-') {
            let start = start.parse::<usize>().ok()?;
            let end = end.parse::<usize>().ok()?;
            count.checked_add(end.checked_sub(start)?.checked_add(1)?)
        } else {
            range
                .parse::<usize>()
                .ok()
                .and_then(|_| count.checked_add(1))
        }
    })
}

fn logical_cpus() -> Option<usize> {
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").ok()?;
    let count = cpuinfo
        .lines()
        .filter(|line| line.starts_with("processor"))
        .count();
    (count > 0).then_some(count)
}

pub fn machine_metadata(runner_class: Option<String>) -> MachineMetadata {
    let resources = probe_host_resources();
    let effective_memory_limit_bytes = resources.effective_memory_limit_bytes();
    let affinity = affinity_cpus();
    let effective_cpu_limit = affinity
        .map(|value| value as f64)
        .into_iter()
        .chain(resources.cgroup_cpu_quota)
        .filter(|value| *value > 0.0)
        .reduce(f64::min);
    let system = command_output("uname", "-s").unwrap_or_else(|| std::env::consts::OS.to_owned());
    let architecture =
        command_output("uname", "-m").unwrap_or_else(|| std::env::consts::ARCH.to_owned());
    let release = command_output("uname", "-r").unwrap_or_default();
    MachineMetadata {
        runner_class,
        platform: format!("{system}-{release}-{architecture}"),
        system,
        architecture,
        cpu_model: resources.cpu_model,
        logical_cpus: logical_cpus(),
        affinity_cpus: affinity,
        cgroup_cpu_quota: resources.cgroup_cpu_quota,
        effective_cpu_limit,
        system_memory_bytes: resources.system_memory_bytes,
        cgroup_memory_limit_bytes: resources.cgroup_memory_limit_bytes,
        effective_memory_limit_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_explicit_runner_class() {
        let metadata = machine_metadata(Some("controlled-linux".to_owned()));
        assert_eq!(metadata.runner_class.as_deref(), Some("controlled-linux"));
    }
}
