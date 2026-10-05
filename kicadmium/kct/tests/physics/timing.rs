//! Port of upstream `tests/test_timing.py`.

use kct::physics::timing::{DifferentialPairSkew, PropagationResult, TimingAnalyzer, TimingBudget};
use kct::physics::{Stackup, SPEED_OF_LIGHT};

fn jlc() -> TimingAnalyzer {
    TimingAnalyzer::new(Stackup::jlcpcb_4layer())
}

fn base_result() -> PropagationResult {
    PropagationResult {
        delay_ps_per_mm: 6.0,
        delay_ns_per_inch: 0.15,
        velocity_m_per_s: 1.5e8,
        velocity_percent_c: 50.0,
        total_delay_ns: 0.0,
        trace_length_mm: 0.0,
    }
}

fn err_contains<T: std::fmt::Debug>(r: kct::physics::PhysResult<T>, needle: &str) {
    let e = r.expect_err("expected ValueError");
    assert!(
        e.0.contains(needle),
        "{:?} does not contain {needle:?}",
        e.0
    );
}

// --- TestPropagationResult ---

#[test]
fn test_basic_properties() {
    let result = base_result();
    assert_eq!(result.delay_ps_per_mm, 6.0);
    assert_eq!(result.delay_ns_per_inch, 0.15);
    assert_eq!(result.velocity_m_per_s, 1.5e8);
    assert_eq!(result.velocity_percent_c, 50.0);
    assert_eq!(result.total_delay_ns, 0.0);
    assert_eq!(result.trace_length_mm, 0.0);
}

#[test]
fn test_repr_without_total_delay() {
    let repr_str = base_result().to_string();
    assert!(repr_str.contains("6.00ps/mm"));
    assert!(repr_str.contains("50.0%c"));
    assert!(!repr_str.contains("total="));
}

#[test]
fn test_repr_with_total_delay() {
    let result = PropagationResult {
        total_delay_ns: 0.3,
        trace_length_mm: 50.0,
        ..base_result()
    };
    assert!(result.to_string().contains("total=0.300ns"));
}

// --- TestTimingBudget ---

#[test]
fn test_timing_budget_basic_properties() {
    let budget = TimingBudget::new("DATA0", 45.0, 0.27);
    assert_eq!(budget.net_name, "DATA0");
    assert_eq!(budget.trace_length_mm, 45.0);
    assert_eq!(budget.propagation_delay_ns, 0.27);
    assert!(budget.target_delay_ns.is_none());
    assert!(budget.skew_ns.is_none());
    assert!(budget.within_budget);
}

#[test]
fn test_with_skew_within_budget() {
    let budget = TimingBudget {
        target_delay_ns: Some(0.28),
        skew_ns: Some(-0.01),
        within_budget: true,
        ..TimingBudget::new("DATA0", 45.0, 0.27)
    };
    let repr_str = budget.to_string();
    assert!(repr_str.contains("DATA0"));
    assert!(repr_str.contains("OK"));
}

#[test]
fn test_with_skew_exceeding_budget() {
    let budget = TimingBudget {
        target_delay_ns: Some(0.30),
        skew_ns: Some(-0.03),
        within_budget: false,
        ..TimingBudget::new("DATA0", 45.0, 0.27)
    };
    assert!(budget.to_string().contains("FAIL"));
}

// --- TestDifferentialPairSkew ---

fn skew(p: f64, n: f64, skew_ps: f64, within_spec: bool) -> DifferentialPairSkew {
    DifferentialPairSkew {
        positive_net: "USB_D+".into(),
        negative_net: "USB_D-".into(),
        p_delay_ns: p,
        n_delay_ns: n,
        skew_ps,
        max_skew_ps: 10.0,
        within_spec,
    }
}

#[test]
fn test_within_spec() {
    let s = skew(0.312, 0.310, 2.0, true);
    assert!(s.within_spec);
    assert!(s.p_longer());
    assert!(s.recommendation().is_none());
    assert!(s.to_string().contains("OK"));
}

#[test]
fn test_exceeds_spec() {
    let s = skew(0.330, 0.310, 20.0, false);
    assert!(!s.within_spec);
    assert!(s.p_longer());
    let rec = s.recommendation().expect("recommendation");
    assert!(rec.contains('P'));
    assert!(s.to_string().contains("FAIL"));
}

#[test]
fn test_n_longer() {
    let s = skew(0.310, 0.330, 20.0, false);
    assert!(!s.p_longer());
    assert!(s.recommendation().unwrap().contains('N'));
}

// --- TestTimingAnalyzerPropagationDelay ---

#[test]
fn test_fr4_propagation_delay() {
    let result = jlc().propagation_delay(0.2, "F.Cu", "auto").unwrap();
    assert!(5.0 < result.delay_ps_per_mm && result.delay_ps_per_mm < 8.0);
    assert!(0.12 < result.delay_ns_per_inch && result.delay_ns_per_inch < 0.20);
}

#[test]
fn test_velocity_percent_of_c() {
    let result = jlc().propagation_delay(0.2, "F.Cu", "auto").unwrap();
    assert!(40.0 < result.velocity_percent_c && result.velocity_percent_c < 70.0);
    let expected_v = result.velocity_m_per_s;
    let expected_percent = (expected_v / SPEED_OF_LIGHT) * 100.0;
    assert_approx!(result.velocity_percent_c, expected_percent, rel = 0.01);
}

#[test]
fn test_stripline_slower_than_microstrip() {
    let t = jlc();
    let microstrip = t.propagation_delay(0.2, "F.Cu", "microstrip").unwrap();
    let stripline = t.propagation_delay(0.2, "In1.Cu", "stripline").unwrap();
    assert!(stripline.delay_ps_per_mm > microstrip.delay_ps_per_mm);
    assert!(stripline.velocity_m_per_s < microstrip.velocity_m_per_s);
}

#[test]
fn test_auto_mode_detection() {
    let t = jlc();
    let outer_auto = t.propagation_delay(0.2, "F.Cu", "auto").unwrap();
    let outer_explicit = t.propagation_delay(0.2, "F.Cu", "microstrip").unwrap();
    assert_approx!(
        outer_auto.delay_ps_per_mm,
        outer_explicit.delay_ps_per_mm,
        rel = 0.01
    );
    let inner_auto = t.propagation_delay(0.2, "In1.Cu", "auto").unwrap();
    let inner_explicit = t.propagation_delay(0.2, "In1.Cu", "stripline").unwrap();
    assert_approx!(
        inner_auto.delay_ps_per_mm,
        inner_explicit.delay_ps_per_mm,
        rel = 0.01
    );
}

#[test]
fn test_invalid_width() {
    let t = jlc();
    err_contains(t.propagation_delay(0.0, "F.Cu", "auto"), "positive");
    err_contains(t.propagation_delay(-0.1, "F.Cu", "auto"), "positive");
}

#[test]
fn test_invalid_mode() {
    err_contains(
        jlc().propagation_delay(0.2, "F.Cu", "invalid"),
        "Invalid mode",
    );
}

// --- TestTimingAnalyzerAnalyzeTrace ---

#[test]
fn test_total_delay_calculation() {
    let result = jlc().analyze_trace(50.0, 0.2, "F.Cu", "auto").unwrap();
    let expected_total = result.delay_ps_per_mm * 50.0 / 1000.0;
    assert_approx!(result.total_delay_ns, expected_total, rel = 0.01);
    assert_eq!(result.trace_length_mm, 50.0);
}

#[test]
fn test_longer_trace_more_delay() {
    let t = jlc();
    let short = t.analyze_trace(25.0, 0.2, "F.Cu", "auto").unwrap();
    let long = t.analyze_trace(100.0, 0.2, "F.Cu", "auto").unwrap();
    assert!(long.total_delay_ns > short.total_delay_ns);
    assert_approx!(long.total_delay_ns, 4.0 * short.total_delay_ns, rel = 0.01);
}

#[test]
fn test_typical_trace_delay() {
    let result = jlc().analyze_trace(50.0, 0.2, "F.Cu", "auto").unwrap();
    assert!(0.2 < result.total_delay_ns && result.total_delay_ns < 0.4);
}

#[test]
fn test_invalid_length() {
    let t = jlc();
    err_contains(t.analyze_trace(0.0, 0.2, "F.Cu", "auto"), "positive");
    err_contains(t.analyze_trace(-10.0, 0.2, "F.Cu", "auto"), "positive");
}

// --- TestTimingAnalyzerLengthForDelay ---

#[test]
fn test_roundtrip_calculation() {
    let t = jlc();
    let trace = t.analyze_trace(75.0, 0.2, "F.Cu", "auto").unwrap();
    let length = t
        .length_for_delay(trace.total_delay_ns, 0.2, "F.Cu", "auto")
        .unwrap();
    assert_approx!(length, 75.0, rel = 0.01);
}

#[test]
fn test_typical_delay_to_length() {
    let length = jlc().length_for_delay(0.5, 0.2, "F.Cu", "auto").unwrap();
    assert!(60.0 < length && length < 100.0);
}

#[test]
fn test_longer_delay_longer_length() {
    let t = jlc();
    let length_short = t.length_for_delay(0.1, 0.2, "F.Cu", "auto").unwrap();
    let length_long = t.length_for_delay(0.5, 0.2, "F.Cu", "auto").unwrap();
    assert!(length_long > length_short);
    assert_approx!(length_long, 5.0 * length_short, rel = 0.01);
}

#[test]
fn test_length_for_delay_invalid_delay() {
    let t = jlc();
    err_contains(t.length_for_delay(0.0, 0.2, "F.Cu", "auto"), "positive");
    err_contains(t.length_for_delay(-0.1, 0.2, "F.Cu", "auto"), "positive");
}

// --- TestTimingAnalyzerLengthMatching ---

#[test]
fn test_basic_length_matching() {
    let nets = [("DATA0", 45.0), ("DATA1", 46.0), ("DATA2", 44.5)];
    let results = jlc()
        .analyze_length_matching(&nets, 0.2, "F.Cu", 0.1, "auto")
        .unwrap();
    assert_eq!(results.len(), 3);
    for r in &results {
        assert!(r.target_delay_ns.is_some());
        assert!(r.skew_ns.is_some());
    }
}

#[test]
fn test_all_within_budget() {
    let nets = [("DATA0", 45.0), ("DATA1", 45.5), ("DATA2", 45.2)];
    let results = jlc()
        .analyze_length_matching(&nets, 0.2, "F.Cu", 0.1, "auto")
        .unwrap();
    assert!(results.iter().all(|r| r.within_budget));
}

#[test]
fn test_some_exceeding_budget() {
    let nets = [("DATA0", 40.0), ("DATA1", 45.0), ("DATA2", 60.0)];
    let results = jlc()
        .analyze_length_matching(&nets, 0.2, "F.Cu", 0.05, "auto")
        .unwrap();
    assert!(!results.iter().all(|r| r.within_budget));
}

#[test]
fn test_empty_nets_list() {
    let results = jlc()
        .analyze_length_matching(&[], 0.2, "F.Cu", 0.1, "auto")
        .unwrap();
    assert!(results.is_empty());
}

#[test]
fn test_target_delay_is_average() {
    let nets = [("DATA0", 40.0), ("DATA1", 50.0), ("DATA2", 60.0)];
    let results = jlc()
        .analyze_length_matching(&nets, 0.2, "F.Cu", 0.1, "auto")
        .unwrap();
    let targets: Vec<Option<f64>> = results.iter().map(|r| r.target_delay_ns).collect();
    assert!(targets.iter().all(|t| *t == targets[0]));
    let avg_delay =
        results.iter().map(|r| r.propagation_delay_ns).sum::<f64>() / results.len() as f64;
    assert_approx!(results[0].target_delay_ns.unwrap(), avg_delay, rel = 0.01);
}

// --- TestTimingAnalyzerDifferentialPairSkew ---

#[test]
fn test_within_usb2_spec() {
    let result = jlc()
        .analyze_differential_pair_skew(52.0, 52.1, 0.15, "F.Cu", "D+", "D-", 10.0, "auto")
        .unwrap();
    assert!(result.within_spec);
    assert!(result.skew_ps < 10.0);
}

#[test]
fn test_analyzer_exceeds_spec() {
    let result = jlc()
        .analyze_differential_pair_skew(50.0, 55.0, 0.15, "F.Cu", "D+", "D-", 10.0, "auto")
        .unwrap();
    assert!(!result.within_spec);
    assert!(result.skew_ps > 10.0);
}

#[test]
fn test_custom_net_names() {
    let result = jlc()
        .analyze_differential_pair_skew(
            50.0,
            50.0,
            0.15,
            "F.Cu",
            "HDMI_TX0+",
            "HDMI_TX0-",
            10.0,
            "auto",
        )
        .unwrap();
    assert_eq!(result.positive_net, "HDMI_TX0+");
    assert_eq!(result.negative_net, "HDMI_TX0-");
}

#[test]
fn test_invalid_lengths() {
    let t = jlc();
    err_contains(
        t.analyze_differential_pair_skew(0.0, 50.0, 0.15, "F.Cu", "D+", "D-", 10.0, "auto"),
        "positive",
    );
    err_contains(
        t.analyze_differential_pair_skew(50.0, -10.0, 0.15, "F.Cu", "D+", "D-", 10.0, "auto"),
        "positive",
    );
}

// --- TestTimingAnalyzerLengthDifferenceForSkew ---

#[test]
fn test_usb2_skew_budget() {
    let max_diff = jlc()
        .length_difference_for_skew(10.0, 0.15, "F.Cu", "auto")
        .unwrap();
    assert!(1.0 < max_diff && max_diff < 3.0);
}

#[test]
fn test_pcie_skew_budget() {
    let t = jlc();
    let max_diff_10ps = t
        .length_difference_for_skew(10.0, 0.15, "F.Cu", "auto")
        .unwrap();
    let max_diff_5ps = t
        .length_difference_for_skew(5.0, 0.15, "F.Cu", "auto")
        .unwrap();
    assert_approx!(max_diff_5ps, max_diff_10ps / 2.0, rel = 0.01);
}

#[test]
fn test_invalid_skew() {
    err_contains(
        jlc().length_difference_for_skew(0.0, 0.15, "F.Cu", "auto"),
        "positive",
    );
}

// --- TestTimingAnalyzerSerpentineParameters ---

#[test]
fn test_basic_serpentine_calculation() {
    // Upstream checks dict keys; here they are struct fields.
    let params = jlc()
        .serpentine_parameters(0.2, 0.2, 0.3, "F.Cu", "auto")
        .unwrap();
    assert!(params.extra_length_mm > 0.0);
    assert!(params.meander_amplitude_mm > 0.0);
    assert!(params.meander_pitch_mm > 0.0);
    assert!(params.num_meanders > 0.0);
}

#[test]
fn test_more_delay_more_meanders() {
    let t = jlc();
    let params_small = t
        .serpentine_parameters(0.1, 0.2, 0.3, "F.Cu", "auto")
        .unwrap();
    let params_large = t
        .serpentine_parameters(0.5, 0.2, 0.3, "F.Cu", "auto")
        .unwrap();
    assert!(params_large.extra_length_mm > params_small.extra_length_mm);
    assert!(params_large.num_meanders > params_small.num_meanders);
}

#[test]
fn test_amplitude_respects_spacing() {
    let params = jlc()
        .serpentine_parameters(0.2, 0.2, 0.3, "F.Cu", "auto")
        .unwrap();
    assert!(params.meander_amplitude_mm >= 3.0 * 0.2);
}

#[test]
fn test_serpentine_invalid_delay() {
    err_contains(
        jlc().serpentine_parameters(0.0, 0.2, 0.3, "F.Cu", "auto"),
        "positive",
    );
}

#[test]
fn test_invalid_spacing() {
    err_contains(
        jlc().serpentine_parameters(0.2, 0.2, 0.0, "F.Cu", "auto"),
        "positive",
    );
}

// --- TestTimingAnalyzerIntegration ---

#[test]
fn test_2layer_stackup() {
    let t = TimingAnalyzer::new(Stackup::default_2layer(1.6));
    let result = t.propagation_delay(0.3, "F.Cu", "auto").unwrap();
    assert!(result.delay_ps_per_mm > 0.0);
    assert!(result.velocity_percent_c > 0.0);
}

#[test]
fn test_6layer_stackup() {
    let t = TimingAnalyzer::new(Stackup::default_6layer());
    let outer = t.propagation_delay(0.2, "F.Cu", "auto").unwrap();
    assert!(outer.delay_ps_per_mm > 0.0);
    let inner = t.propagation_delay(0.15, "In2.Cu", "auto").unwrap();
    assert!(inner.delay_ps_per_mm > 0.0);
}

#[test]
fn test_oshpark_stackup() {
    let t = TimingAnalyzer::new(Stackup::oshpark_4layer());
    let result = t.propagation_delay(0.2, "F.Cu", "auto").unwrap();
    assert!(5.0 < result.delay_ps_per_mm && result.delay_ps_per_mm < 8.0);
}

// --- TestTimingAnalyzerAccuracy ---

#[test]
fn test_typical_fr4_delay() {
    let t = jlc();
    let microstrip = t.propagation_delay(0.2, "F.Cu", "auto").unwrap();
    assert!(5.0 < microstrip.delay_ps_per_mm && microstrip.delay_ps_per_mm < 7.5);
    let stripline = t.propagation_delay(0.15, "In1.Cu", "auto").unwrap();
    assert!(6.0 < stripline.delay_ps_per_mm && stripline.delay_ps_per_mm < 8.5);
}

#[test]
fn test_velocity_consistency() {
    let result = jlc().propagation_delay(0.2, "F.Cu", "auto").unwrap();
    let expected_delay = 1e12 * 0.001 / result.velocity_m_per_s;
    assert_approx!(result.delay_ps_per_mm, expected_delay, rel = 0.01);
}

#[test]
fn test_unit_conversion_consistency() {
    let result = jlc().propagation_delay(0.2, "F.Cu", "auto").unwrap();
    let expected_ns_per_inch = result.delay_ps_per_mm * 25.4 / 1000.0;
    assert_approx!(result.delay_ns_per_inch, expected_ns_per_inch, rel = 0.01);
}
