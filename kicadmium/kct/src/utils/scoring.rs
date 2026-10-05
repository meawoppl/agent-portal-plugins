//! Unified confidence scoring framework (port of `kicad_tools.utils.scoring`).
//!
//! All confidence values are normalized to `[0.0, 1.0]`.

use std::str::FromStr;

use crate::exceptions::ValueError;

/// Standard confidence levels for categorizing match quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfidenceLevel {
    Exact,
    High,
    Medium,
    Low,
    VeryLow,
    None,
}

impl ConfidenceLevel {
    /// Enum value (`ConfidenceLevel.X.value`).
    pub fn value(self) -> f64 {
        match self {
            Self::Exact => 1.0,
            Self::High => 0.9,
            Self::Medium => 0.7,
            Self::Low => 0.5,
            Self::VeryLow => 0.3,
            Self::None => 0.0,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Exact => "EXACT",
            Self::High => "HIGH",
            Self::Medium => "MEDIUM",
            Self::Low => "LOW",
            Self::VeryLow => "VERY_LOW",
            Self::None => "NONE",
        }
    }

    /// Category for a score.
    pub fn from_score(score: f64) -> Self {
        if score >= 0.95 {
            Self::Exact
        } else if score >= 0.8 {
            Self::High
        } else if score >= 0.6 {
            Self::Medium
        } else if score >= 0.4 {
            Self::Low
        } else if score >= 0.2 {
            Self::VeryLow
        } else {
            Self::None
        }
    }
}

/// Result of a matching operation with a (clamped) confidence score.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchResult {
    pub confidence: f64,
    pub match_type: Option<String>,
    pub matched_value: Option<String>,
}

impl MatchResult {
    /// Build a result, clamping `confidence` to `[0, 1]` (`__post_init__`).
    pub fn new(confidence: f64, match_type: Option<&str>, matched_value: Option<&str>) -> Self {
        Self {
            confidence: confidence.clamp(0.0, 1.0),
            match_type: match_type.map(str::to_string),
            matched_value: matched_value.map(str::to_string),
        }
    }

    pub fn level(&self) -> ConfidenceLevel {
        ConfidenceLevel::from_score(self.confidence)
    }
}

/// Keyword options of `calculate_string_confidence`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StringConfidenceOptions {
    pub case_sensitive: bool,
    pub exact_score: f64,
    pub substring_score: f64,
    pub no_match_score: f64,
}

impl Default for StringConfidenceOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            exact_score: 1.0,
            substring_score: 0.9,
            no_match_score: 0.7,
        }
    }
}

/// Exact > substring (either direction) > no direct match.
pub fn calculate_string_confidence(
    query: &str,
    target: &str,
    opts: StringConfidenceOptions,
) -> MatchResult {
    let (q, t) = if opts.case_sensitive {
        (query.to_string(), target.to_string())
    } else {
        (query.to_lowercase(), target.to_lowercase())
    };
    let (score, kind) = if q.is_empty() && t.is_empty() {
        (opts.exact_score, "exact")
    } else if q.is_empty() || t.is_empty() {
        (opts.substring_score, "substring")
    } else if q == t {
        (opts.exact_score, "exact")
    } else if t.contains(&q) || q.contains(&t) {
        (opts.substring_score, "substring")
    } else {
        (opts.no_match_score, "search")
    };
    MatchResult::new(score, Some(kind), Some(target))
}

/// Combination method for [`combine_confidences`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombineMethod {
    WeightedAverage,
    Min,
    Max,
    Product,
}

impl FromStr for CombineMethod {
    type Err = ValueError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "weighted_average" => Ok(Self::WeightedAverage),
            "min" => Ok(Self::Min),
            "max" => Ok(Self::Max),
            "product" => Ok(Self::Product),
            other => Err(ValueError::new(format!(
                "Unknown combination method: {other}"
            ))),
        }
    }
}

/// Combine scores (`method` is the upstream string: `weighted_average`,
/// `min`, `max`, `product`); result clamped to `[0, 1]`.
pub fn combine_confidences(
    scores: &[f64],
    method: &str,
    weights: Option<&[f64]>,
) -> Result<f64, ValueError> {
    if scores.is_empty() {
        return Ok(0.0);
    }
    let result = match method.parse::<CombineMethod>()? {
        CombineMethod::WeightedAverage => match weights {
            None => scores.iter().sum::<f64>() / scores.len() as f64,
            Some(w) => {
                if w.len() != scores.len() {
                    return Err(ValueError::new(
                        "Number of weights must match number of scores",
                    ));
                }
                let total: f64 = w.iter().sum();
                if total == 0.0 {
                    return Ok(0.0);
                }
                scores.iter().zip(w).map(|(s, w)| s * w).sum::<f64>() / total
            }
        },
        CombineMethod::Min => scores.iter().copied().fold(f64::INFINITY, f64::min),
        CombineMethod::Max => scores.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        CombineMethod::Product => scores.iter().product(),
    };
    Ok(result.clamp(0.0, 1.0))
}

/// `base * multiplier + bonus - penalty`, clamped to `[minimum, maximum]`.
pub fn adjust_confidence(
    base_confidence: f64,
    multiplier: f64,
    bonus: f64,
    penalty: f64,
    minimum: f64,
    maximum: f64,
) -> f64 {
    let result = base_confidence * multiplier + bonus - penalty;
    minimum.max(maximum.min(result))
}
