use std::collections::HashMap;
use std::time::Duration;

use super::{
    AllocationMeasurement, ProcessMeasurement, TimingEvent, TimingEventKind, TimingFormat,
    TimingMeasurement,
};

pub(super) const TIMING_REPORT_ENTRY_LIMIT: usize = 64;

pub(super) fn format_event(event: &TimingEvent) -> String {
    match event {
        TimingEvent::Measurement {
            kind: TimingEventKind::Stage,
            name,
            measurement,
        } => format_measurement_event("timing", name, *measurement),
        TimingEvent::Measurement {
            kind: TimingEventKind::Query,
            name,
            measurement,
        } => format_measurement_event("query timing", name, *measurement),
        TimingEvent::Note {
            kind: TimingEventKind::Stage,
            name,
            detail,
        } => format!("timing {name}: {detail}"),
        TimingEvent::Note {
            kind: TimingEventKind::Query,
            name,
            detail,
        } => format!("query timing {name}: {detail}"),
        TimingEvent::Counter { name, value } => format!("timing counter {name}: {value}"),
    }
}

fn format_measurement_event(prefix: &str, name: &str, measurement: TimingMeasurement) -> String {
    if measurement.count == 1 {
        return format!("{prefix} {name}: {:.3}s", measurement.total.as_secs_f64());
    }
    format!(
        "{prefix} {name}: total={:.3}s count={} max={:.3}s",
        measurement.total.as_secs_f64(),
        measurement.count,
        measurement.max.as_secs_f64()
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TimingReport {
    pub(super) entries: Vec<TimingReportEntry>,
    pub(super) counters: Vec<TimingCounter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TimingReportEntry {
    pub(super) kind: TimingEventKind,
    pub(super) name: String,
    pub(super) count: usize,
    pub(super) total: Duration,
    pub(super) max: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TimingCounter {
    pub(super) name: String,
    pub(super) value: u64,
}

#[derive(Debug, Default)]
pub(super) struct TimingReportBuilder {
    entries_by_key: HashMap<(TimingEventKind, String), TimingReportEntry>,
}

impl TimingReportBuilder {
    pub(super) fn record(
        &mut self,
        kind: TimingEventKind,
        name: String,
        measurement: TimingMeasurement,
    ) {
        let entry = self
            .entries_by_key
            .entry((kind, name.clone()))
            .or_insert_with(|| TimingReportEntry {
                kind,
                name,
                count: 0,
                total: Duration::ZERO,
                max: Duration::ZERO,
            });
        entry.count += measurement.count;
        entry.total += measurement.total;
        entry.max = entry.max.max(measurement.max);
    }

    pub(super) fn merge(&mut self, other: TimingReportBuilder) {
        for entry in other.entries_by_key.into_values() {
            self.record(
                entry.kind,
                entry.name,
                TimingMeasurement {
                    total: entry.total,
                    max: entry.max,
                    count: entry.count,
                },
            );
        }
    }

    pub(super) fn finish_entries(self) -> Vec<TimingReportEntry> {
        let mut entries = self.entries_by_key.into_values().collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .total
                .cmp(&left.total)
                .then_with(|| left.name.cmp(&right.name))
        });
        entries
    }
}

impl TimingReport {
    #[cfg(test)]
    pub(super) fn from_events(events: &[TimingEvent]) -> Self {
        let mut builder = TimingReportBuilder::default();
        for event in events {
            let TimingEvent::Measurement {
                kind,
                name,
                measurement,
            } = event
            else {
                continue;
            };
            builder.record(*kind, name.clone(), *measurement);
        }
        Self {
            entries: builder.finish_entries(),
            counters: Vec::new(),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.counters.is_empty()
    }

    pub(super) fn add_allocation_counters(&mut self, measurement: AllocationMeasurement) {
        self.counters.extend([
            TimingCounter {
                name: "allocator.alloc_calls".to_string(),
                value: measurement.allocation_calls,
            },
            TimingCounter {
                name: "allocator.allocated_bytes".to_string(),
                value: measurement.allocated_bytes,
            },
            TimingCounter {
                name: "allocator.dealloc_calls".to_string(),
                value: measurement.deallocation_calls,
            },
            TimingCounter {
                name: "allocator.deallocated_bytes".to_string(),
                value: measurement.deallocated_bytes,
            },
            TimingCounter {
                name: "allocator.live_bytes".to_string(),
                value: measurement.live_bytes,
            },
            TimingCounter {
                name: "allocator.peak_live_bytes".to_string(),
                value: measurement.peak_live_bytes,
            },
            TimingCounter {
                name: "allocator.realloc_calls".to_string(),
                value: measurement.reallocation_calls,
            },
        ]);
        self.counters
            .sort_by(|left, right| left.name.cmp(&right.name));
    }
}

pub(super) fn print_report(
    report: &TimingReport,
    format: TimingFormat,
    process: ProcessMeasurement,
) {
    match format {
        TimingFormat::Text => print_text_report(report),
        TimingFormat::Json => eprintln!("{}", format_json_report(report, process)),
    }
}

fn print_text_report(report: &TimingReport) {
    if report.is_empty() {
        return;
    }
    eprintln!("timing summary:");
    for entry in report.entries.iter().take(TIMING_REPORT_ENTRY_LIMIT) {
        eprintln!("{}", format_report_entry(entry));
    }
    if report.entries.len() > TIMING_REPORT_ENTRY_LIMIT {
        eprintln!(
            "timing summary omitted {} entries",
            report.entries.len() - TIMING_REPORT_ENTRY_LIMIT
        );
    }
    for counter in &report.counters {
        eprintln!("timing summary counter {}: {}", counter.name, counter.value);
    }
}

pub(super) fn format_report_entry(entry: &TimingReportEntry) -> String {
    let prefix = match entry.kind {
        TimingEventKind::Stage => "timing summary stage",
        TimingEventKind::Query => "timing summary query",
    };
    format!(
        "{prefix} {}: total={:.3}s count={} max={:.3}s",
        entry.name,
        entry.total.as_secs_f64(),
        entry.count,
        entry.max.as_secs_f64()
    )
}

pub(super) fn format_json_report(report: &TimingReport, process: ProcessMeasurement) -> String {
    let mut output = String::new();
    output.push_str("{\"release_compatibility\":");
    output.push_str(&nia_compat::RELEASE_COMPATIBILITY.to_string());
    output.push_str(",\"process\":{");
    push_json_duration(&mut output, "wall_seconds", Some(process.wall));
    output.push(',');
    push_json_duration(&mut output, "user_seconds", process.user);
    output.push(',');
    push_json_duration(&mut output, "system_seconds", process.system);
    output.push_str(",\"max_rss_bytes\":");
    push_json_optional_u64(&mut output, process.max_rss_bytes);
    output.push_str(",\"cpu_utilization_percent\":");
    push_json_optional_f64(&mut output, process.cpu_utilization_percent());
    output.push_str("},\"timings\":[");
    for (index, entry) in report.entries.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        output.push_str("{\"kind\":");
        push_json_string(
            &mut output,
            match entry.kind {
                TimingEventKind::Stage => "stage",
                TimingEventKind::Query => "query",
            },
        );
        output.push_str(",\"name\":");
        push_json_string(&mut output, &entry.name);
        output.push_str(&format!(
            ",\"count\":{},\"total_seconds\":{:.9},\"max_seconds\":{:.9}}}",
            entry.count,
            entry.total.as_secs_f64(),
            entry.max.as_secs_f64(),
        ));
    }
    output.push_str("],\"counters\":{");
    for (index, counter) in report.counters.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        push_json_string(&mut output, &counter.name);
        output.push(':');
        output.push_str(&counter.value.to_string());
    }
    output.push_str("}}");
    output
}

fn push_json_duration(output: &mut String, name: &str, value: Option<Duration>) {
    push_json_string(output, name);
    output.push(':');
    push_json_optional_f64(output, value.map(|duration| duration.as_secs_f64()));
}

fn push_json_optional_u64(output: &mut String, value: Option<u64>) {
    match value {
        Some(value) => output.push_str(&value.to_string()),
        None => output.push_str("null"),
    }
}

fn push_json_optional_f64(output: &mut String, value: Option<f64>) {
    match value {
        Some(value) => output.push_str(&format!("{value:.9}")),
        None => output.push_str("null"),
    }
}

fn push_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output.push('"');
}
