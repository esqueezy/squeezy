use squeezy_core::{Result, SqueezyError};

use crate::report::BenchmarkReport;

pub(crate) fn enforce_gates(report: &BenchmarkReport, no_speed_gate: bool) -> Result<()> {
    let missing = report
        .queries
        .iter()
        .flat_map(|query| {
            query
                .missing
                .iter()
                .map(|missing| format!("{} missing {missing}", query.id))
        })
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(SqueezyError::Graph(format!(
            "benchmark expected results missing: {}",
            missing.join(", ")
        )));
    }

    if !no_speed_gate && !report.faster_than_validation {
        return Err(SqueezyError::Graph(format!(
            "Squeezy graph was not faster than {} validation: {}ms >= {}ms",
            report.validation_status, report.squeezy_total_ms, report.validation_ms
        )));
    }

    if let Some(refresh) = &report.refresh_probe
        && refresh.reparsed_files != refresh.edited_files
    {
        return Err(SqueezyError::Graph(format!(
            "refresh probe reparsed {} files after {} edits",
            refresh.reparsed_files, refresh.edited_files
        )));
    }

    if !no_speed_gate
        && let Some(go) = &report.go_oracle
        && (go.symbols.false_positive != 0 || go.symbols.false_negative != 0)
    {
        return Err(SqueezyError::Graph(format!(
            "Go oracle accuracy regressed: fp={} fn={}",
            go.symbols.false_positive, go.symbols.false_negative
        )));
    }

    // Kotlin smoke-tier symbol parity gate. Thresholds from
    // `target/lang-specs/kotlin.md` §10: precision >= 0.94, recall >= 0.85.
    // Skipped when the oracle was unavailable so missing kotlinc/JDK doesn't
    // mask the fixture-query gates above.
    if !no_speed_gate
        && let Some(kotlin) = &report.kotlin_oracle
        && kotlin.oracle_ms.is_some()
    {
        if kotlin.symbols.precision < 0.94 {
            return Err(SqueezyError::Graph(format!(
                "Kotlin oracle precision below 0.94 floor: {:.3} (fp={})",
                kotlin.symbols.precision, kotlin.symbols.false_positive,
            )));
        }
        if kotlin.symbols.recall < 0.85 {
            return Err(SqueezyError::Graph(format!(
                "Kotlin oracle recall below 0.85 floor: {:.3} (fn={})",
                kotlin.symbols.recall, kotlin.symbols.false_negative,
            )));
        }
    }

    if let Some(mixed) = &report.mixed_workload
        && mixed.refresh_probe.reparsed_files != mixed.refresh_probe.edited_files
    {
        return Err(SqueezyError::Graph(format!(
            "refresh probe reparsed {} files after {} edits",
            mixed.refresh_probe.reparsed_files, mixed.refresh_probe.edited_files
        )));
    }

    Ok(())
}
