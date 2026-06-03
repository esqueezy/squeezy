use super::*;
use serde_json::json;
use squeezy_tools::{ToolCostHint, ToolReceipt};

fn sample_status() -> CostCapStatus {
    CostCapStatus {
        spent_usd_micros: 12_457,
        cap_usd_micros: 10_000,
        percent: 124,
    }
}

#[test]
fn cap_reached_reason_states_spent_cap_and_percent() {
    let msg = format_cap_reached_reason(sample_status());
    assert!(
        msg.contains("$0.012457"),
        "spent amount must be cited; got: {msg}"
    );
    assert!(
        msg.contains("$0.010000"),
        "cap amount must be cited; got: {msg}"
    );
    assert!(msg.contains("(124%)"), "percent must be cited; got: {msg}");
}

#[test]
fn cap_reached_reason_includes_next_step_guidance() {
    // squeezy-zp6e: the cap-reached error must steer the user to a
    // concrete next step (raise the cap via /config or env var).
    // Without this the user is left with a bare numeric message and
    // no idea how to recover.
    let msg = format_cap_reached_reason(sample_status());
    assert!(
        msg.contains("/config"),
        "cap-reached message must reference /config; got: {msg}"
    );
    assert!(
        msg.contains("max_session_cost_usd_micros"),
        "cap-reached message must name the setting; got: {msg}"
    );
    assert!(
        msg.contains("SQUEEZY_MAX_SESSION_COST_USD_MICROS"),
        "cap-reached message must cite the env var override; got: {msg}"
    );
}

#[test]
fn warn_threshold_notice_includes_next_step_guidance() {
    // squeezy-zp6e: the warning-tier notice also needs an actionable
    // hint so the user can raise the cap *before* the hard cap trips
    // and a turn fails outright.
    let status = CostCapStatus {
        spent_usd_micros: 9_600,
        cap_usd_micros: 10_000,
        percent: 96,
    };
    let notice = format_warn_threshold_notice(status);
    assert!(
        notice.contains("warning threshold"),
        "notice must label itself as the warning tier; got: {notice}"
    );
    assert!(
        notice.contains("/config"),
        "warn notice must reference /config; got: {notice}"
    );
    assert!(
        notice.contains("max_session_cost_usd_micros"),
        "warn notice must name the setting; got: {notice}"
    );
    assert!(
        notice.contains("(96%)"),
        "warn notice must cite the percent; got: {notice}"
    );
}

fn config_with_cap(cap_micros: u64) -> AppConfig {
    AppConfig {
        max_session_cost_usd_micros: Some(cap_micros),
        ..AppConfig::default()
    }
}

#[test]
fn unenforceable_cap_round_signals_once_and_freezes_accumulator() {
    // A cap is configured but the round has no per-round dollar estimate
    // (no registry pricing for this model). The accumulator can't advance,
    // so neither the warning nor the hard cap can ever fire — surface a
    // single notice instead of silently no-op'ing the guardrail.
    let mut broker = CostBroker::new(&config_with_cap(10_000));
    let no_pricing = CostSnapshot {
        input_tokens: Some(50_000),
        output_tokens: Some(50_000),
        estimated_usd_micros: None,
        ..Default::default()
    };

    let cap_status = broker.record_provider_cost(&no_pricing);
    assert!(
        cap_status.is_none(),
        "no dollar estimate means no warning/cap event can fire"
    );
    assert_eq!(
        broker.session_cost_usd_micros, 0,
        "an unpriced round leaves the accumulator at 0"
    );
    assert!(
        broker.note_unenforceable_cap_round(&no_pricing),
        "first unpriced round under a cap must emit the cap-unenforceable signal"
    );
    assert!(
        !broker.note_unenforceable_cap_round(&no_pricing),
        "the cap-unenforceable signal is one-shot"
    );
}

#[test]
fn unenforceable_cap_signal_suppressed_without_cap_or_with_pricing() {
    // No cap configured: the cap can't be unenforceable, so stay silent.
    let mut no_cap = CostBroker::new(&AppConfig::default());
    let no_pricing = CostSnapshot {
        input_tokens: Some(1_000),
        estimated_usd_micros: None,
        ..Default::default()
    };
    assert!(
        !no_cap.note_unenforceable_cap_round(&no_pricing),
        "no cap means there is nothing to warn about"
    );

    // Cap configured and the round carries a dollar estimate: the cap is
    // enforceable, so no notice.
    let mut priced = CostBroker::new(&config_with_cap(10_000));
    let priced_round = CostSnapshot {
        input_tokens: Some(1_000),
        estimated_usd_micros: Some(2_000),
        ..Default::default()
    };
    assert!(
        !priced.note_unenforceable_cap_round(&priced_round),
        "a priced round keeps the cap enforceable"
    );
}

#[test]
fn cap_unenforceable_notice_names_provider_model_and_setting() {
    let notice = format_cap_unenforceable_notice("openrouter", "anthropic/claude-opus-4-7");
    assert!(
        notice.contains("openrouter/anthropic/claude-opus-4-7"),
        "notice must cite the provider/model; got: {notice}"
    );
    assert!(
        notice.contains("cannot be enforced"),
        "notice must state the cap is inert; got: {notice}"
    );
    assert!(
        notice.contains("max_session_cost_usd_micros"),
        "notice must name the setting; got: {notice}"
    );
}

#[test]
fn pressure_percent_is_none_without_cap_and_tracks_spend_with_cap() {
    // No cap configured: there is nothing to be a percent of.
    let mut no_cap = CostBroker::new(&AppConfig::default());
    no_cap.session_cost_usd_micros = 50_000;
    assert_eq!(
        no_cap.pressure_percent(),
        None,
        "pressure has no meaning without a configured cap"
    );

    // Cap configured: pressure is spent/cap as a clamped percent.
    let mut broker = CostBroker::new(&config_with_cap(10_000));
    assert_eq!(
        broker.pressure_percent(),
        Some(0),
        "a fresh broker under a cap is at 0% pressure"
    );
    broker.session_cost_usd_micros = 5_000;
    assert_eq!(broker.pressure_percent(), Some(50));
    broker.session_cost_usd_micros = 7_999;
    assert_eq!(broker.pressure_percent(), Some(79));
    broker.session_cost_usd_micros = 8_000;
    assert_eq!(broker.pressure_percent(), Some(80));
    // Overshoot past the cap stays a valid percent (clamped, no overflow).
    broker.session_cost_usd_micros = 30_000;
    assert_eq!(broker.pressure_percent(), Some(255));
}

#[test]
fn pressure_gate_engages_at_threshold_when_cap_set() {
    let mut broker = CostBroker::new(&config_with_cap(10_000));
    // Just under 80%: no gate.
    broker.session_cost_usd_micros = 7_999;
    assert!(
        broker.pressure_gate().is_none(),
        "below the pressure threshold the gate must stay open"
    );
    // At exactly 80%: gate engages and reports the pressure status.
    broker.session_cost_usd_micros = 8_000;
    let status = broker
        .pressure_gate()
        .expect("gate must engage at the pressure threshold");
    assert_eq!(status.spent_usd_micros, 8_000);
    assert_eq!(status.cap_usd_micros, 10_000);
    assert_eq!(status.percent, 80);
}

#[test]
fn pressure_gate_engages_above_threshold_and_is_one_shot() {
    let mut broker = CostBroker::new(&config_with_cap(10_000));
    // Well above the threshold (but the hard cap is a separate check): gate fires.
    broker.session_cost_usd_micros = 9_500;
    assert!(
        broker.pressure_gate().is_some(),
        "the gate engages once spend is at or past the pressure threshold"
    );
    // One-shot latch: it does not re-fire on subsequent rounds even as spend climbs.
    broker.session_cost_usd_micros = 9_900;
    assert!(
        broker.pressure_gate().is_none(),
        "the pressure gate is one-shot per broker"
    );
}

#[test]
fn pressure_gate_never_engages_without_cap() {
    let mut broker = CostBroker::new(&AppConfig::default());
    // Arbitrary large spend: with no cap there is no pressure to govern.
    broker.session_cost_usd_micros = 1_000_000_000;
    assert!(
        broker.pressure_gate().is_none(),
        "with no configured cap the pressure gate must never engage"
    );
    assert!(
        broker.pressure_gate().is_none(),
        "repeat call with no cap also stays open"
    );
}

#[test]
fn pressure_gate_does_not_engage_below_threshold() {
    let mut broker = CostBroker::new(&config_with_cap(10_000));
    broker.session_cost_usd_micros = 5_000;
    assert!(
        broker.pressure_gate().is_none(),
        "at 50% pressure the gate stays open and behaviour is unchanged"
    );
    assert_eq!(
        broker.pressure_percent(),
        Some(50),
        "reading pressure must not arm the gate latch"
    );
    // Crossing the threshold afterwards still fires (latch was not consumed below threshold).
    broker.session_cost_usd_micros = 8_500;
    assert!(
        broker.pressure_gate().is_some(),
        "a sub-threshold gate check must not consume the one-shot latch"
    );
}

#[test]
fn pressure_gate_reason_states_spend_cap_percent_and_next_step() {
    let status = CostCapStatus {
        spent_usd_micros: 8_000,
        cap_usd_micros: 10_000,
        percent: 80,
    };
    let msg = format_pressure_gate_reason(status);
    assert!(
        msg.contains("approaching cap"),
        "reason must frame this as a proactive pressure stop; got: {msg}"
    );
    assert!(
        msg.contains("$0.008000"),
        "reason must cite the spent amount; got: {msg}"
    );
    assert!(
        msg.contains("$0.010000"),
        "reason must cite the cap amount; got: {msg}"
    );
    assert!(
        msg.contains("(80%)"),
        "reason must cite the percent; got: {msg}"
    );
    assert!(
        msg.contains("/config"),
        "reason must reference /config; got: {msg}"
    );
    assert!(
        msg.contains("max_session_cost_usd_micros"),
        "reason must name the setting; got: {msg}"
    );
}

#[test]
fn budget_denied_result_counts_once_across_accounting_paths() {
    let mut broker = CostBroker::new(&AppConfig::default());
    let result = ToolResult {
        call_id: "call-1".to_string(),
        tool_name: "read_file".to_string(),
        status: ToolStatus::Denied,
        content: json!({
            "budget_denied": true,
            "error": "budget exhausted",
        }),
        cost_hint: ToolCostHint::default(),
        receipt: ToolReceipt {
            output_sha256: "sha".to_string(),
            content_sha256: None,
        },
        spill_model_output: None,
    };

    broker.record_executed_result(&result);
    broker.record_model_result(&result);

    assert_eq!(broker.metrics.budget_denials, 1);
}
