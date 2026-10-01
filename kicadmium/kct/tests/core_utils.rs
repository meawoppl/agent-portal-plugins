//! Ports of upstream tests/test_utils.py and tests/test_utils_scoring.py.

use kct::utils::{
    adjust_confidence, calculate_string_confidence, combine_confidences, ensure_parent_dir,
    ConfidenceLevel, MatchResult, StringConfidenceOptions,
};

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

// ------------------------------------------------------------ ensure_parent_dir

#[test]
fn ensure_parent_dir_creates_parent_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("nested").join("dir").join("file.txt");
    assert!(!file.parent().unwrap().exists());
    let result = ensure_parent_dir(&file).unwrap();
    assert_eq!(result, file);
    assert!(file.parent().unwrap().is_dir());
}

#[test]
fn ensure_parent_dir_returns_original_path() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("test").join("file.txt");
    let result = ensure_parent_dir(&file).unwrap();
    assert!(std::ptr::eq(result, file.as_path()));
}

#[test]
fn ensure_parent_dir_existing_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("existing").join("file.txt");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    assert_eq!(ensure_parent_dir(&file).unwrap(), file);
    assert!(file.parent().unwrap().exists());
}

#[test]
fn ensure_parent_dir_chaining_with_write() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("chain").join("test.txt");
    std::fs::write(ensure_parent_dir(&file).unwrap(), "test content").unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "test content");
}

#[test]
fn ensure_parent_dir_deeply_nested() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("a/b/c/d/e/file.txt");
    assert_eq!(ensure_parent_dir(&file).unwrap(), file);
    assert!(file.parent().unwrap().exists());
}

// ------------------------------------------------------------ ConfidenceLevel

#[test]
fn confidence_level_from_score() {
    use ConfidenceLevel::*;
    for (score, level) in [
        (1.0, Exact),
        (0.95, Exact),
        (0.99, Exact),
        (0.9, High),
        (0.8, High),
        (0.85, High),
        (0.7, Medium),
        (0.6, Medium),
        (0.75, Medium),
        (0.5, Low),
        (0.4, Low),
        (0.55, Low),
        (0.3, VeryLow),
        (0.2, VeryLow),
        (0.35, VeryLow),
        (0.0, None),
        (0.1, None),
        (0.19, None),
    ] {
        assert_eq!(ConfidenceLevel::from_score(score), level, "score {score}");
    }
}

// ------------------------------------------------------------ MatchResult

#[test]
fn match_result_creation_and_level() {
    let r = MatchResult::new(0.9, Some("exact"), Some("test"));
    assert_eq!(r.confidence, 0.9);
    assert_eq!(r.match_type.as_deref(), Some("exact"));
    assert_eq!(r.matched_value.as_deref(), Some("test"));
    assert_eq!(
        MatchResult::new(0.95, None, None).level(),
        ConfidenceLevel::Exact
    );
    assert_eq!(
        MatchResult::new(0.5, None, None).level(),
        ConfidenceLevel::Low
    );
}

#[test]
fn match_result_confidence_clamped() {
    assert_eq!(MatchResult::new(1.5, None, None).confidence, 1.0);
    assert_eq!(MatchResult::new(-0.5, None, None).confidence, 0.0);
}

// ------------------------------------------------------------ calculate_string_confidence

fn scs(q: &str, t: &str) -> MatchResult {
    calculate_string_confidence(q, t, StringConfidenceOptions::default())
}

#[test]
fn string_confidence_cases() {
    let r = scs("STM32F103", "STM32F103");
    assert_eq!(
        (r.confidence, r.match_type.as_deref()),
        (1.0, Some("exact"))
    );
    let r = scs("stm32f103", "STM32F103");
    assert_eq!(
        (r.confidence, r.match_type.as_deref()),
        (1.0, Some("exact"))
    );
    let r = calculate_string_confidence(
        "stm32f103",
        "STM32F103",
        StringConfidenceOptions {
            case_sensitive: true,
            ..Default::default()
        },
    );
    assert_eq!(
        (r.confidence, r.match_type.as_deref()),
        (0.7, Some("search"))
    );
    let r = scs("STM32", "STM32F103C8T6");
    assert_eq!(
        (r.confidence, r.match_type.as_deref()),
        (0.9, Some("substring"))
    );
    let r = scs("STM32F103C8T6-FULL", "STM32F103C8T6");
    assert_eq!(
        (r.confidence, r.match_type.as_deref()),
        (0.9, Some("substring"))
    );
    let r = scs("STM32", "ATMEGA328P");
    assert_eq!(
        (r.confidence, r.match_type.as_deref()),
        (0.7, Some("search"))
    );
    let r = scs("", "STM32F103");
    assert_eq!(
        (r.confidence, r.match_type.as_deref()),
        (0.9, Some("substring"))
    );
    let r = scs("STM32F103", "");
    assert_eq!(
        (r.confidence, r.match_type.as_deref()),
        (0.9, Some("substring"))
    );
    let r = scs("", "");
    assert_eq!(
        (r.confidence, r.match_type.as_deref()),
        (1.0, Some("exact"))
    );
}

#[test]
fn string_confidence_custom_scores() {
    let r = calculate_string_confidence(
        "STM32",
        "ATMEGA328P",
        StringConfidenceOptions {
            case_sensitive: false,
            exact_score: 0.99,
            substring_score: 0.85,
            no_match_score: 0.5,
        },
    );
    assert_eq!(r.confidence, 0.5);
}

// ------------------------------------------------------------ combine_confidences

#[test]
fn combine_confidences_methods() {
    assert!(approx(
        combine_confidences(&[0.9, 0.8, 0.7], "weighted_average", None).unwrap(),
        0.8,
        1e-3
    ));
    let expected = 0.9 * 0.5 + 0.8 * 0.3 + 0.7 * 0.2;
    assert!(approx(
        combine_confidences(&[0.9, 0.8, 0.7], "weighted_average", Some(&[0.5, 0.3, 0.2])).unwrap(),
        expected,
        1e-3
    ));
    assert_eq!(
        combine_confidences(&[0.9, 0.8, 0.7], "min", None).unwrap(),
        0.7
    );
    assert_eq!(
        combine_confidences(&[0.9, 0.8, 0.7], "max", None).unwrap(),
        0.9
    );
    assert!(approx(
        combine_confidences(&[0.9, 0.8], "product", None).unwrap(),
        0.72,
        1e-3
    ));
    assert_eq!(
        combine_confidences(&[], "weighted_average", None).unwrap(),
        0.0
    );
    assert_eq!(
        combine_confidences(&[0.85], "weighted_average", None).unwrap(),
        0.85
    );
    let r = combine_confidences(&[0.9, 0.9, 0.9], "max", None).unwrap();
    assert!((0.0..=1.0).contains(&r));
}

#[test]
fn combine_confidences_errors() {
    let e = combine_confidences(&[0.9, 0.8], "invalid", None).unwrap_err();
    assert!(e.to_string().contains("Unknown combination method"));
    let e = combine_confidences(&[0.9, 0.8], "weighted_average", Some(&[0.5])).unwrap_err();
    assert!(e.to_string().contains("Number of weights must match"));
}

// ------------------------------------------------------------ adjust_confidence

#[test]
fn adjust_confidence_cases() {
    let adj = |b, m, bo, p, mi, ma| adjust_confidence(b, m, bo, p, mi, ma);
    assert_eq!(adj(0.5, 1.0, 0.0, 0.0, 0.0, 1.0), 0.5);
    assert!(approx(adj(0.5, 1.0, 0.2, 0.0, 0.0, 1.0), 0.7, 1e-12));
    assert!(approx(adj(0.8, 1.0, 0.0, 0.3, 0.0, 1.0), 0.5, 1e-12));
    assert_eq!(adj(0.8, 0.5, 0.0, 0.0, 0.0, 1.0), 0.4);
    assert!(approx(adj(0.8, 0.5, 0.1, 0.05, 0.0, 1.0), 0.45, 1e-3));
    assert_eq!(adj(0.8, 1.0, 0.5, 0.0, 0.0, 1.0), 1.0);
    assert_eq!(adj(0.3, 1.0, 0.0, 0.5, 0.0, 1.0), 0.0);
    assert_eq!(adj(0.3, 1.0, 0.0, 0.5, 0.1, 1.0), 0.1);
    assert_eq!(adj(0.8, 1.0, 0.5, 0.0, 0.0, 0.9), 0.9);
}
