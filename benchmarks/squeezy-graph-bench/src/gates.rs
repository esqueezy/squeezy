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

    // Spec §10: Swift first-PR thresholds. precision >= 0.92, recall >=
    // 0.80. The speed gate stays disabled per the corpus entry
    // (`no_speed_gate: true`). We enforce thresholds only when the
    // oracle was actually invoked.
    if let Some(swift) = &report.swift_oracle {
        let precision = swift.symbols.precision;
        let recall = swift.symbols.recall;
        let denom = swift.symbols.true_positive
            + swift.symbols.false_positive
            + swift.symbols.false_negative;
        if denom > 0 {
            if precision < 0.92 {
                return Err(SqueezyError::Graph(format!(
                    "Swift symbol precision {precision:.3} below 0.92 gate (tp={} fp={} fn={})",
                    swift.symbols.true_positive,
                    swift.symbols.false_positive,
                    swift.symbols.false_negative,
                )));
            }
            if recall < 0.80 {
                return Err(SqueezyError::Graph(format!(
                    "Swift symbol recall {recall:.3} below 0.80 gate (tp={} fp={} fn={})",
                    swift.symbols.true_positive,
                    swift.symbols.false_positive,
                    swift.symbols.false_negative,
                )));
            }
        }
        // Spec §10: navigation probe gates. Only enforce when probes
        // actually ran (sourcekit-lsp present + probe_limit > 0). When
        // the LSP is unavailable the probe report is empty and the
        // accuracy stays at the f64 default of 1.0; gate that off the
        // `probes` count so a missing toolchain does not trip the
        // gate. Mirrors the symbol gate pattern above and the existing
        // Rust-side nav-accuracy treatment (no hard gate — observed by
        // hand on rust corpus today).
        let nav = &swift.navigation_accuracy;
        if nav.definitions.probes > 0 && nav.definitions.precision < 0.85 {
            return Err(SqueezyError::Graph(format!(
                "Swift definition precision {:.3} below 0.85 gate (probes={} tp={} fp={} fn={})",
                nav.definitions.precision,
                nav.definitions.probes,
                nav.definitions.true_positive,
                nav.definitions.false_positive,
                nav.definitions.false_negative,
            )));
        }
        if nav.references.symbols_sampled > 0 && nav.references.precision < 0.80 {
            return Err(SqueezyError::Graph(format!(
                "Swift reference precision {:.3} below 0.80 gate (symbols={} tp={} fp={} fn={})",
                nav.references.precision,
                nav.references.symbols_sampled,
                nav.references.true_positive,
                nav.references.false_positive,
                nav.references.false_negative,
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
