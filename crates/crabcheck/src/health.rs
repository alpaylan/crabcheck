//! Health checks: things about a run an author should look at, computed
//! from the report's statistics once the run is over.

use crate::{
    Config,
    report::{
        Report,
        Status,
    },
};

/// Add health warnings to `report`.
pub(crate) fn assess(report: &mut Report, config: &Config) {
    let attempts = report.tests + report.discarded;
    if attempts >= 20 && report.discarded * 2 > attempts {
        let dominant = report
            .discard_reasons
            .iter()
            .max_by_key(|(_, count)| **count)
            .map(|(reason, count)| format!(" (mostly `{reason}`, {count} times)"))
            .unwrap_or_default();
        report.warn(
            "discard_ratio",
            format!(
                "{:.0}% of inputs were discarded{dominant}; generate valid inputs directly instead of filtering",
                report.discarded as f64 * 100.0 / attempts as f64
            ),
        );
    }

    if report.tests >= 20 && report.distinct_inputs_ratio < 0.5 {
        report.warn(
            "low_distinct_inputs",
            format!(
                "only {:.0}% of generated inputs were distinct; the generator's space is small or max_size ({}) is too low",
                report.distinct_inputs_ratio * 100.0,
                config.max_size
            ),
        );
    }

    if report.gen_time.as_millis() >= 100 && report.gen_time > report.prop_time * 4 {
        report.warn(
            "slow_generation",
            format!(
                "generating inputs took {:.0} ms but running the property took {:.0} ms; simplify the generator or lower max_size",
                report.gen_time.as_secs_f64() * 1000.0,
                report.prop_time.as_secs_f64() * 1000.0
            ),
        );
    }

    if report.status == Status::GaveUp
        && let Some(largest) = report.sizes.keys().max()
        && *largest * 2 < config.max_size
    {
        report.warn(
            "size_never_reached",
            format!(
                "the largest input size that passed was {largest} of max_size {}; discards concentrate at larger sizes",
                config.max_size
            ),
        );
    }
}
