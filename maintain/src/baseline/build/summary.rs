use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::acceptance::counter;
use super::reports::BUILD_STAGE_NAMES;
use super::{BuildResult, Distribution, Number, StateSummary};
use crate::MaintainResult;

fn numeric(value: &Value, context: &str) -> MaintainResult<Number> {
    if let Some(value) = value.as_i64() {
        return Ok(Number::Integer(value));
    }
    let value = value
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("{context} is not numeric"))?;
    Ok(Number::Float(value))
}

fn distribution(mut values: Vec<Number>) -> MaintainResult<Distribution> {
    if values.is_empty() {
        return Err("cannot summarize an empty distribution".to_owned());
    }
    values.sort_by(|left, right| left.as_f64().total_cmp(&right.as_f64()));
    let middle = values.len() / 2;
    let median = if values.len().is_multiple_of(2) {
        Number::Float((values[middle - 1].as_f64() + values[middle].as_f64()) / 2.0)
    } else {
        values[middle].clone()
    };

    // Use the nearest-rank p95 definition. It is deterministic for the small
    // repetition counts used by CI and always selects an observed sample.
    let p95_index = ((values.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(values.len() - 1);
    Ok(Distribution {
        median,
        p95: values[p95_index].clone(),
        min: values[0].clone(),
        max: values.last().expect("nonempty values").clone(),
    })
}

pub fn summarize_runs(runs: &[Vec<BuildResult>]) -> MaintainResult<Vec<StateSummary>> {
    let first_run = runs
        .first()
        .ok_or_else(|| "cannot summarize zero runs".to_owned())?;
    let mut summaries = Vec::new();
    for (state_index, first) in first_run.iter().enumerate() {
        let samples = runs
            .iter()
            .map(|run| {
                run.get(state_index)
                    .ok_or_else(|| "build run has missing state".to_owned())
            })
            .collect::<MaintainResult<Vec<_>>>()?;

        // Counters can be feature-dependent. Summarize the stable union and
        // let the typed counter reader supply zero for an absent counter.
        let counter_names = samples
            .iter()
            .flat_map(|sample| sample.reports.measurement.counters.keys().cloned())
            .collect::<BTreeSet<_>>();
        let mut process = BTreeMap::new();
        for name in ["wall_seconds", "max_rss_bytes"] {
            if samples.iter().all(|sample| {
                sample
                    .reports
                    .measurement
                    .process
                    .get(name)
                    .is_some_and(|value| !value.is_null())
            }) {
                process.insert(
                    name.to_owned(),
                    distribution(
                        samples
                            .iter()
                            .map(|sample| {
                                numeric(
                                    &sample.reports.measurement.process[name],
                                    &format!("process metric {name}"),
                                )
                            })
                            .collect::<MaintainResult<Vec<_>>>()?,
                    )?,
                );
            }
        }
        let stages = BUILD_STAGE_NAMES
            .iter()
            .map(|name| {
                let values = samples
                    .iter()
                    .map(|sample| {
                        numeric(
                            &sample.reports.measurement.stages[*name]["total_seconds"],
                            &format!("stage metric {name}"),
                        )
                    })
                    .collect::<MaintainResult<Vec<_>>>()?;
                Ok(((*name).to_owned(), distribution(values)?))
            })
            .collect::<MaintainResult<BTreeMap<_, _>>>()?;
        let counters = counter_names
            .into_iter()
            .map(|name| {
                let values = samples
                    .iter()
                    .map(|sample| counter(sample, &name).map(Number::Integer))
                    .collect::<MaintainResult<Vec<_>>>()?;
                Ok((name, distribution(values)?))
            })
            .collect::<MaintainResult<BTreeMap<_, _>>>()?;
        summaries.push(StateSummary {
            name: first.name.clone(),
            sample_count: samples.len(),
            wall_seconds_observed: distribution(
                samples
                    .iter()
                    .map(|sample| Number::Float(sample.wall_seconds_observed))
                    .collect(),
            )?,
            process,
            stages,
            counters,
        });
    }
    Ok(summaries)
}
