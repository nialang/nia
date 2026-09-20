use std::fs;
use std::path::Path;

use super::schema::ProcessMetrics;
use crate::MaintainResult;
use crate::system::process::run_bounded;

const TIME_FORMAT: &str = "%e\n%U\n%S\n%M\n%x";

pub(super) struct MeasuredOutput {
    pub(super) process_id: u32,
    pub(super) return_code: i32,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
    pub(super) metrics: ProcessMetrics,
}

fn parse_metrics(value: &str, observed_wall: f64) -> MaintainResult<(ProcessMetrics, i32)> {
    let fields = value.lines().map(str::trim).collect::<Vec<_>>();
    if fields.len() < 5 {
        return Err(format!(
            "GNU time emitted {} metric fields, expected at least 5: {value:?}",
            fields.len()
        ));
    }
    let fields = &fields[fields.len() - 5..];
    let [wall, user, system, max_rss_kib, return_code] = fields else {
        unreachable!("five trailing fields were selected")
    };
    let parse_float = |field: &str, name: &str| {
        field
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| format!("GNU time emitted invalid {name}: {field:?}"))
    };
    let wall = parse_float(wall, "wall time")?;
    let user = parse_float(user, "user time")?;
    let system = parse_float(system, "system time")?;
    let max_rss_bytes = max_rss_kib
        .parse::<u64>()
        .ok()
        .and_then(|value| value.checked_mul(1024))
        .ok_or_else(|| format!("GNU time emitted invalid maximum RSS: {max_rss_kib:?}"))?;
    let return_code = return_code
        .parse::<i32>()
        .map_err(|_| format!("GNU time emitted invalid return code: {return_code:?}"))?;
    let cpu_utilization_percent = if wall > 0.0 {
        (user + system) * 100.0 / wall
    } else {
        0.0
    };
    Ok((
        ProcessMetrics {
            wall_seconds_observed: observed_wall,
            wall_seconds: wall,
            user_seconds: user,
            system_seconds: system,
            cpu_utilization_percent,
            max_rss_bytes,
        },
        return_code,
    ))
}

pub(super) fn measure(
    time: &Path,
    command: &[String],
    cwd: &Path,
    metrics_path: &Path,
    timeout_seconds: u64,
) -> MaintainResult<MeasuredOutput> {
    let mut timed = vec![
        time.to_string_lossy().into_owned(),
        "--output".to_owned(),
        metrics_path.to_string_lossy().into_owned(),
        "--format".to_owned(),
        TIME_FORMAT.to_owned(),
        "--".to_owned(),
    ];
    timed.extend(command.iter().cloned());
    let output = run_bounded(&timed, cwd, timeout_seconds)?;
    let metrics = fs::read_to_string(metrics_path).map_err(|error| {
        format!(
            "failed to read GNU time metrics {}: {error}",
            metrics_path.display()
        )
    })?;
    let (metrics, reported_return_code) = parse_metrics(&metrics, output.elapsed)?;
    let return_code = output.status.code().unwrap_or(-1);
    if return_code != reported_return_code {
        return Err(format!(
            "GNU time status mismatch: process={return_code} report={reported_return_code}"
        ));
    }
    Ok(MeasuredOutput {
        process_id: output.process_id,
        return_code,
        stdout: output.stdout,
        stderr: output.stderr,
        metrics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gnu_time_metrics_and_derives_cpu_utilization() {
        let (metrics, status) = parse_metrics("2.00\n1.25\n0.25\n1024\n0\n", 2.01).unwrap();
        assert_eq!(status, 0);
        assert_eq!(metrics.wall_seconds, 2.0);
        assert_eq!(metrics.cpu_utilization_percent, 75.0);
        assert_eq!(metrics.max_rss_bytes, 1024 * 1024);
    }

    #[test]
    fn ignores_gnu_time_failure_annotation_before_metrics() {
        let (_, status) = parse_metrics(
            "Command exited with non-zero status 1\n0.10\n0.01\n0.02\n20\n1\n",
            0.11,
        )
        .unwrap();
        assert_eq!(status, 1);
    }
}
