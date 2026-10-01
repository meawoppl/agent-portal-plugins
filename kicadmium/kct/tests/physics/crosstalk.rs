//! Port of upstream `tests/test_crosstalk.py`.

use kct::physics::{CrosstalkAnalyzer, CrosstalkResult, Stackup};

fn jlc() -> CrosstalkAnalyzer {
    CrosstalkAnalyzer::new(Stackup::jlcpcb_4layer())
}

fn assert_err_contains<T: std::fmt::Debug>(r: kct::physics::PhysResult<T>, needle: &str) {
    let e = r.expect_err("expected ValueError");
    assert!(
        e.0.contains(needle),
        "{:?} does not contain {needle:?}",
        e.0
    );
}

fn severity_ok(s: &str) -> bool {
    matches!(s, "acceptable" | "marginal" | "excessive")
}

fn max_pct(r: &CrosstalkResult) -> f64 {
    r.next_percent.max(r.fext_percent)
}

// --- TestCrosstalkResult ---

#[test]
fn test_basic_result() {
    let result = CrosstalkResult {
        next_coefficient: 0.05,
        fext_coefficient: 0.03,
        next_db: -26.0,
        fext_db: -30.5,
        next_percent: 5.0,
        fext_percent: 3.0,
        coupled_length_mm: 20.0,
        saturation_length_mm: 75.0,
        severity: "marginal".into(),
        recommendation: Some("Increase spacing".into()),
    };
    assert_eq!(result.next_coefficient, 0.05);
    assert_eq!(result.fext_coefficient, 0.03);
    assert_eq!(result.next_percent, 5.0);
    assert_eq!(result.fext_percent, 3.0);
    assert_eq!(result.severity, "marginal");
    assert_eq!(result.recommendation.as_deref(), Some("Increase spacing"));
}

#[test]
fn test_repr() {
    let result = CrosstalkResult {
        next_coefficient: 0.05,
        fext_coefficient: 0.03,
        next_db: -26.0,
        fext_db: -30.5,
        next_percent: 5.0,
        fext_percent: 3.0,
        coupled_length_mm: 20.0,
        saturation_length_mm: 75.0,
        severity: "marginal".into(),
        recommendation: None,
    };
    let repr_str = result.to_string();
    assert!(repr_str.contains("5.0%"));
    assert!(repr_str.contains("3.0%"));
    assert!(repr_str.contains("marginal"));
}

// --- TestCrosstalkAnalyzerBasic ---

#[test]
fn test_basic_analysis() {
    let result = jlc().analyze(0.2, 0.2, 0.2, 20.0, "F.Cu", 1.0).unwrap();
    assert!(0.0 <= result.next_coefficient && result.next_coefficient <= 1.0);
    assert!(0.0 <= result.fext_coefficient && result.fext_coefficient <= 1.0);
    assert!(result.next_percent >= 0.0);
    assert!(result.fext_percent >= 0.0);
    assert_eq!(result.coupled_length_mm, 20.0);
    assert!(result.saturation_length_mm > 0.0);
    assert!(severity_ok(&result.severity));
}

#[test]
fn test_db_values_negative() {
    let result = jlc().analyze(0.2, 0.2, 0.2, 20.0, "F.Cu", 1.0).unwrap();
    assert!(result.next_db < 0.0);
    assert!(result.fext_db < 0.0);
}

#[test]
fn test_db_calculation_correct() {
    let result = jlc().analyze(0.2, 0.2, 0.2, 20.0, "F.Cu", 1.0).unwrap();
    let expected_next_db = 20.0 * result.next_coefficient.max(1e-6).log10();
    let expected_fext_db = 20.0 * result.fext_coefficient.max(1e-6).log10();
    assert_approx!(result.next_db, expected_next_db, rel = 0.01);
    assert_approx!(result.fext_db, expected_fext_db, rel = 0.01);
}

#[test]
fn test_percent_calculation_correct() {
    let result = jlc().analyze(0.2, 0.2, 0.2, 20.0, "F.Cu", 1.0).unwrap();
    assert_approx!(
        result.next_percent,
        result.next_coefficient * 100.0,
        rel = 0.001
    );
    assert_approx!(
        result.fext_percent,
        result.fext_coefficient * 100.0,
        rel = 0.001
    );
}

// --- TestCrosstalkPhysicalBehavior ---

#[test]
fn test_fext_increases_with_length() {
    let xt = jlc();
    let short = xt.analyze(0.2, 0.2, 0.2, 10.0, "F.Cu", 1.0).unwrap();
    let long = xt.analyze(0.2, 0.2, 0.2, 50.0, "F.Cu", 1.0).unwrap();
    assert!(long.fext_percent > short.fext_percent);
}

#[test]
fn test_next_saturates() {
    let xt = jlc();
    let baseline = xt.analyze(0.2, 0.2, 0.2, 10.0, "F.Cu", 1.0).unwrap();
    let lsat = baseline.saturation_length_mm;

    let short = xt.analyze(0.2, 0.2, 0.2, lsat * 0.5, "F.Cu", 1.0).unwrap();
    let at_sat = xt.analyze(0.2, 0.2, 0.2, lsat, "F.Cu", 1.0).unwrap();
    let beyond_sat = xt.analyze(0.2, 0.2, 0.2, lsat * 3.0, "F.Cu", 1.0).unwrap();

    assert!(short.next_coefficient < at_sat.next_coefficient);
    assert_approx!(
        beyond_sat.next_coefficient,
        at_sat.next_coefficient,
        rel = 0.05
    );
}

#[test]
fn test_crosstalk_decreases_with_spacing() {
    let xt = jlc();
    let tight = xt.analyze(0.2, 0.2, 0.15, 20.0, "F.Cu", 1.0).unwrap();
    let loose = xt.analyze(0.2, 0.2, 0.4, 20.0, "F.Cu", 1.0).unwrap();
    assert!(loose.next_percent < tight.next_percent);
    assert!(loose.fext_percent < tight.fext_percent);
}

#[test]
fn test_faster_rise_time_increases_fext() {
    let xt = jlc();
    let slow = xt.analyze(0.2, 0.2, 0.2, 30.0, "F.Cu", 2.0).unwrap();
    let fast = xt.analyze(0.2, 0.2, 0.2, 30.0, "F.Cu", 0.5).unwrap();
    assert!(fast.fext_percent > slow.fext_percent);
}

#[test]
fn test_slower_rise_time_increases_saturation_length() {
    let xt = jlc();
    let slow = xt.analyze(0.2, 0.2, 0.2, 20.0, "F.Cu", 2.0).unwrap();
    let fast = xt.analyze(0.2, 0.2, 0.2, 20.0, "F.Cu", 0.5).unwrap();
    assert!(slow.saturation_length_mm > fast.saturation_length_mm);
}

// --- TestSeverityClassification ---

#[test]
fn test_acceptable_severity() {
    let result = jlc().analyze(0.2, 0.2, 1.0, 5.0, "F.Cu", 1.0).unwrap();
    assert!(max_pct(&result) < 3.0);
    assert_eq!(result.severity, "acceptable");
    assert!(result.recommendation.is_none());
}

#[test]
fn test_marginal_severity() {
    let result = jlc().analyze(0.2, 0.2, 0.2, 20.0, "F.Cu", 1.0).unwrap();
    if result.severity == "marginal" {
        let m = max_pct(&result);
        assert!((3.0..10.0).contains(&m));
        assert!(result.recommendation.is_some());
    }
}

#[test]
fn test_excessive_severity() {
    let result = jlc().analyze(0.2, 0.2, 0.1, 100.0, "F.Cu", 1.0).unwrap();
    if result.severity == "excessive" {
        assert!(max_pct(&result) >= 10.0);
        let rec = result.recommendation.as_deref().unwrap().to_lowercase();
        assert!(rec.contains("layer") || rec.contains("spacing"));
    }
}

// --- TestSpacingForCrosstalkBudget ---

#[test]
fn test_spacing_for_5_percent_budget() {
    let xt = jlc();
    let spacing = xt
        .spacing_for_crosstalk_budget(5.0, 0.2, 20.0, "F.Cu", 1.0)
        .unwrap();
    let result = xt.analyze(0.2, 0.2, spacing, 20.0, "F.Cu", 1.0).unwrap();
    assert!(max_pct(&result) <= 5.5);
}

#[test]
fn test_spacing_for_3_percent_budget() {
    let xt = jlc();
    let spacing = xt
        .spacing_for_crosstalk_budget(3.0, 0.2, 15.0, "F.Cu", 1.0)
        .unwrap();
    let result = xt.analyze(0.2, 0.2, spacing, 15.0, "F.Cu", 1.0).unwrap();
    assert!(max_pct(&result) <= 3.3);
    assert!(matches!(
        result.severity.as_str(),
        "acceptable" | "marginal"
    ));
}

#[test]
fn test_tighter_budget_requires_wider_spacing() {
    let xt = jlc();
    let spacing_5pct = xt
        .spacing_for_crosstalk_budget(5.0, 0.2, 20.0, "F.Cu", 1.0)
        .unwrap();
    let spacing_3pct = xt
        .spacing_for_crosstalk_budget(3.0, 0.2, 20.0, "F.Cu", 1.0)
        .unwrap();
    assert!(spacing_3pct > spacing_5pct);
}

#[test]
fn test_longer_run_requires_wider_spacing() {
    let xt = jlc();
    let spacing_short = xt
        .spacing_for_crosstalk_budget(5.0, 0.2, 10.0, "F.Cu", 1.0)
        .unwrap();
    let spacing_long = xt
        .spacing_for_crosstalk_budget(5.0, 0.2, 50.0, "F.Cu", 1.0)
        .unwrap();
    assert!(spacing_long > spacing_short);
}

// --- TestInnerLayerCrosstalk ---

#[test]
fn test_inner_layer_analysis() {
    let result = jlc()
        .analyze(0.15, 0.15, 0.15, 20.0, "In1.Cu", 1.0)
        .unwrap();
    assert!(result.next_coefficient >= 0.0);
    assert!(result.fext_coefficient >= 0.0);
    assert!(severity_ok(&result.severity));
}

#[test]
fn test_inner_layer_has_different_saturation_length() {
    let xt = jlc();
    let outer = xt.analyze(0.2, 0.2, 0.2, 20.0, "F.Cu", 1.0).unwrap();
    let inner = xt.analyze(0.2, 0.2, 0.2, 20.0, "In1.Cu", 1.0).unwrap();
    assert_ne!(inner.saturation_length_mm, outer.saturation_length_mm);
}

// --- TestErrorHandling ---

#[test]
fn test_invalid_aggressor_width() {
    let xt = jlc();
    assert_err_contains(xt.analyze(0.0, 0.2, 0.2, 20.0, "F.Cu", 1.0), "positive");
    assert_err_contains(xt.analyze(-0.1, 0.2, 0.2, 20.0, "F.Cu", 1.0), "positive");
}

#[test]
fn test_invalid_victim_width() {
    assert_err_contains(jlc().analyze(0.2, 0.0, 0.2, 20.0, "F.Cu", 1.0), "positive");
}

#[test]
fn test_invalid_spacing() {
    assert_err_contains(jlc().analyze(0.2, 0.2, 0.0, 20.0, "F.Cu", 1.0), "positive");
}

#[test]
fn test_invalid_parallel_length() {
    assert_err_contains(jlc().analyze(0.2, 0.2, 0.2, 0.0, "F.Cu", 1.0), "positive");
}

#[test]
fn test_invalid_rise_time() {
    assert_err_contains(jlc().analyze(0.2, 0.2, 0.2, 20.0, "F.Cu", 0.0), "positive");
}

#[test]
fn test_spacing_budget_invalid_crosstalk() {
    assert_err_contains(
        jlc().spacing_for_crosstalk_budget(0.0, 0.2, 20.0, "F.Cu", 1.0),
        "positive",
    );
}

#[test]
fn test_spacing_budget_invalid_width() {
    assert_err_contains(
        jlc().spacing_for_crosstalk_budget(5.0, 0.0, 20.0, "F.Cu", 1.0),
        "positive",
    );
}

#[test]
fn test_spacing_budget_invalid_length() {
    assert_err_contains(
        jlc().spacing_for_crosstalk_budget(5.0, 0.2, 0.0, "F.Cu", 1.0),
        "positive",
    );
}

// --- TestStackupIntegration ---

#[test]
fn test_2layer_stackup() {
    let xt = CrosstalkAnalyzer::new(Stackup::default_2layer(1.6));
    let result = xt.analyze(0.2, 0.2, 0.2, 20.0, "F.Cu", 1.0).unwrap();
    assert!(result.next_coefficient >= 0.0);
    assert!(result.fext_coefficient >= 0.0);
}

#[test]
fn test_6layer_stackup() {
    let xt = CrosstalkAnalyzer::new(Stackup::default_6layer());
    let result_outer = xt.analyze(0.2, 0.2, 0.2, 20.0, "F.Cu", 1.0).unwrap();
    assert!(result_outer.next_coefficient >= 0.0);
    let result_inner = xt.analyze(0.15, 0.15, 0.15, 20.0, "In2.Cu", 1.0).unwrap();
    assert!(result_inner.next_coefficient >= 0.0);
}

#[test]
fn test_oshpark_4layer() {
    let xt = CrosstalkAnalyzer::new(Stackup::oshpark_4layer());
    let result = xt.analyze(0.15, 0.15, 0.15, 20.0, "F.Cu", 1.0).unwrap();
    assert!(result.next_coefficient >= 0.0);
    assert!(result.fext_coefficient >= 0.0);
    assert!(severity_ok(&result.severity));
}

// --- TestAsymmetricTraces ---

#[test]
fn test_different_widths() {
    let result = jlc().analyze(0.3, 0.15, 0.2, 20.0, "F.Cu", 1.0).unwrap();
    assert!(result.next_coefficient >= 0.0);
    assert!(result.fext_coefficient >= 0.0);
}

#[test]
fn test_symmetric_vs_asymmetric() {
    let xt = jlc();
    let symmetric = xt.analyze(0.225, 0.225, 0.2, 20.0, "F.Cu", 1.0).unwrap();
    let asymmetric = xt.analyze(0.3, 0.15, 0.2, 20.0, "F.Cu", 1.0).unwrap();
    assert_approx!(
        symmetric.next_coefficient,
        asymmetric.next_coefficient,
        rel = 0.01
    );
}
