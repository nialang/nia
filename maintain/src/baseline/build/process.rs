use std::path::Path;
use std::process::ExitStatus;

use crate::MaintainResult;
use crate::system::process;
use crate::system::resources::probe_host_resources;

const MIN_AVAILABLE_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub(super) struct BoundedOutput {
    pub(super) process_id: u32,
    pub(super) status: ExitStatus,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
    pub(super) elapsed: f64,
    pub(super) available_memory: Option<u64>,
}

fn require_memory_headroom() -> MaintainResult<Option<u64>> {
    let available = probe_host_resources().available_memory_bytes();
    if available.is_some_and(|value| value < MIN_AVAILABLE_MEMORY_BYTES) {
        return Err(format!(
            "build baseline refused to start under memory pressure: available={} required={MIN_AVAILABLE_MEMORY_BYTES}",
            available.unwrap_or_default()
        ));
    }
    Ok(available)
}

pub(super) fn run_bounded(
    command: &[String],
    cwd: &Path,
    timeout_seconds: u64,
) -> MaintainResult<BoundedOutput> {
    let available_memory = require_memory_headroom()?;
    let output = process::run_bounded(command, cwd, timeout_seconds)?;
    Ok(BoundedOutput {
        process_id: output.process_id,
        status: output.status,
        stdout: output.stdout,
        stderr: output.stderr,
        elapsed: output.elapsed,
        available_memory,
    })
}
