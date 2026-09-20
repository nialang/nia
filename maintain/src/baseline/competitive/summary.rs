use std::collections::BTreeMap;

use super::schema::{CompetitiveSample, CompetitiveSummary, Distribution, SampleState};
use super::{Language, Profile};

fn distribution(mut values: Vec<f64>) -> Distribution {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    let median = if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    };
    let p95_index = ((values.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(values.len() - 1);
    Distribution {
        median,
        p95: values[p95_index],
        min: values[0],
        max: *values.last().expect("nonempty distribution"),
    }
}

pub(super) fn summarize(samples: &[CompetitiveSample]) -> Vec<CompetitiveSummary> {
    let mut groups =
        BTreeMap::<(Profile, &'static str, Language, SampleState), Vec<&CompetitiveSample>>::new();
    for sample in samples {
        groups
            .entry((
                sample.profile,
                sample.workload,
                sample.language,
                sample.state,
            ))
            .or_default()
            .push(sample);
    }
    groups
        .into_iter()
        .map(|((profile, workload, language, state), samples)| {
            let metric = |read: fn(&CompetitiveSample) -> f64| {
                distribution(samples.iter().map(|sample| read(sample)).collect())
            };
            let metrics = BTreeMap::from([
                (
                    "wall_seconds_observed",
                    metric(|sample| sample.metrics.wall_seconds_observed),
                ),
                ("wall_seconds", metric(|sample| sample.metrics.wall_seconds)),
                ("user_seconds", metric(|sample| sample.metrics.user_seconds)),
                (
                    "system_seconds",
                    metric(|sample| sample.metrics.system_seconds),
                ),
                (
                    "cpu_utilization_percent",
                    metric(|sample| sample.metrics.cpu_utilization_percent),
                ),
                (
                    "max_rss_bytes",
                    metric(|sample| sample.metrics.max_rss_bytes as f64),
                ),
            ]);
            let artifact_sizes = samples
                .iter()
                .filter_map(|sample| sample.artifact.as_ref())
                .map(|artifact| artifact.size_bytes as f64)
                .collect::<Vec<_>>();
            CompetitiveSummary {
                profile,
                workload,
                language,
                state,
                sample_count: samples.len(),
                metrics,
                artifact_size_bytes: (!artifact_sizes.is_empty())
                    .then(|| distribution(artifact_sizes)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_uses_nearest_rank_p95_and_even_median() {
        let summary = distribution(vec![4.0, 1.0, 3.0, 2.0]);
        assert_eq!(summary.median, 2.5);
        assert_eq!(summary.p95, 4.0);
        assert_eq!(summary.min, 1.0);
        assert_eq!(summary.max, 4.0);
    }
}
