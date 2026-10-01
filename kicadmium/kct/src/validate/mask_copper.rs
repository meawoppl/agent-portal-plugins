//! Explicit process policy and source-qualified solder-mask exposure
//! assessment (port of `kicad_tools.validate.mask_copper`).
//!
//! The check is opt-in: neither a manufacturer's generic mask expansion nor a
//! mask-web width supplies a mask-to-copper process clearance requirement.

use crate::jobj;
use crate::pyjson::Json;
use crate::validate::violations::{DRCResults, DRCViolation};

#[derive(Debug, Clone, PartialEq)]
pub struct MaskCopperPolicy {
    pub clearance_mm: f64,
    pub source: String,
    pub process: String,
    pub revision: String,
}

impl MaskCopperPolicy {
    pub fn new(
        clearance_mm: f64,
        source: &str,
        process: &str,
        revision: &str,
    ) -> anyhow::Result<Self> {
        if !clearance_mm.is_finite() || clearance_mm < 0.0 {
            anyhow::bail!("Mask-to-copper clearance must be finite and nonnegative");
        }
        if [source, process, revision]
            .iter()
            .any(|v| v.trim().is_empty())
        {
            anyhow::bail!("Mask-to-copper policy requires source, process and revision");
        }
        Ok(MaskCopperPolicy {
            clearance_mm,
            source: source.into(),
            process: process.into(),
            revision: revision.into(),
        })
    }

    pub fn to_dict(&self) -> Json {
        jobj! {
            "clearance_mm" => self.clearance_mm,
            "source" => self.source.as_str(),
            "process" => self.process.as_str(),
            "revision" => self.revision.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaskSourceBinding {
    pub source_sha256: String,
    pub project_sha256: Option<String>,
    pub rules_sha256: Option<String>,
    pub export_profile_sha256: String,
}

impl MaskSourceBinding {
    pub fn new(
        source_sha256: &str,
        project_sha256: Option<&str>,
        rules_sha256: Option<&str>,
        export_profile_sha256: &str,
    ) -> anyhow::Result<Self> {
        let ok = |v: &str| {
            v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        for v in [
            Some(source_sha256),
            project_sha256,
            rules_sha256,
            Some(export_profile_sha256),
        ]
        .into_iter()
        .flatten()
        {
            if !ok(v) {
                anyhow::bail!("Malformed source binding SHA256");
            }
        }
        Ok(MaskSourceBinding {
            source_sha256: source_sha256.into(),
            project_sha256: project_sha256.map(Into::into),
            rules_sha256: rules_sha256.map(Into::into),
            export_profile_sha256: export_profile_sha256.into(),
        })
    }

    pub fn to_dict(&self) -> Json {
        jobj! {
            "source_sha256" => self.source_sha256.as_str(),
            "project_sha256" => self.project_sha256.clone(),
            "rules_sha256" => self.rules_sha256.clone(),
            "export_profile_sha256" => self.export_profile_sha256.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaskEscapeIntent {
    pub binding: MaskSourceBinding,
    pub owner_uuid: String,
    pub conductor_uuid: String,
    pub mask_side: String,
    pub rationale: String,
    pub scope: String,
}

impl MaskEscapeIntent {
    pub fn to_dict(&self) -> Json {
        jobj! {
            "binding" => self.binding.to_dict(),
            "owner_uuid" => self.owner_uuid.as_str(),
            "conductor_uuid" => self.conductor_uuid.as_str(),
            "mask_side" => self.mask_side.as_str(),
            "rationale" => self.rationale.as_str(),
            "scope" => self.scope.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaskCopperMeasurement {
    pub opening_uuids: Vec<String>,
    pub conductor_uuid: String,
    pub mask_side: String,
    pub copper_layer: String,
    pub location_mm: (f64, f64),
    pub clearance_mm: f64,
    pub exposed_area_mm2: f64,
    pub relation: String,
    pub disposition: String,
    pub uncertainty_mm: f64,
    pub opening_kind: String,
    pub mask_defined: bool,
    pub margin_provenance: String,
    pub rationale: Option<String>,
    pub negative_expansion: bool,
}

impl MaskCopperMeasurement {
    pub fn to_dict(&self) -> Json {
        jobj! {
            "opening_uuids" => self.opening_uuids.clone(),
            "conductor_uuid" => self.conductor_uuid.as_str(),
            "mask_side" => self.mask_side.as_str(),
            "copper_layer" => self.copper_layer.as_str(),
            "location_mm" => Json::Arr(vec![Json::Float(self.location_mm.0), Json::Float(self.location_mm.1)]),
            "clearance_mm" => self.clearance_mm,
            "exposed_area_mm2" => self.exposed_area_mm2,
            "relation" => self.relation.as_str(),
            "disposition" => self.disposition.as_str(),
            "uncertainty_mm" => self.uncertainty_mm,
            "opening_kind" => self.opening_kind.as_str(),
            "mask_defined" => self.mask_defined,
            "margin_provenance" => self.margin_provenance.as_str(),
            "rationale" => self.rationale.clone(),
            "negative_expansion" => self.negative_expansion,
        }
    }
}

/// Assessment of a requested mask-to-copper check.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskCopperAssessment {
    /// `complete`, `incomplete` or `not_run`.
    pub coverage: String,
    pub reasons: Vec<String>,
    pub policy: Option<MaskCopperPolicy>,
    pub binding: Option<MaskSourceBinding>,
    pub measurements: Vec<MaskCopperMeasurement>,
    pub intent_audit: Vec<Json>,
    pub geometry_provenance: Json,
    pub evaluated_pairs: i64,
}

impl Default for MaskCopperAssessment {
    fn default() -> Self {
        MaskCopperAssessment {
            coverage: "not_run".into(),
            reasons: Vec::new(),
            policy: None,
            binding: None,
            measurements: Vec::new(),
            intent_audit: Vec::new(),
            geometry_provenance: Json::obj(),
            evaluated_pairs: 0,
        }
    }
}

impl MaskCopperAssessment {
    pub fn with_reasons(coverage: &str, reasons: Vec<String>) -> Self {
        MaskCopperAssessment {
            coverage: coverage.into(),
            reasons,
            ..Default::default()
        }
    }

    pub fn passed(&self) -> bool {
        self.reasons.is_empty()
            && self.policy.is_some()
            && self.binding.is_some()
            && self.coverage == "complete"
            && !self
                .measurements
                .iter()
                .any(|m| m.disposition == "violation" || m.disposition == "uncertain")
    }

    pub fn to_dict(&self) -> Json {
        jobj! {
            "schema" => "kct.mask-copper-assessment.v1",
            "coverage" => self.coverage.as_str(),
            "passed" => self.passed(),
            "reasons" => self.reasons.clone(),
            "policy" => self.policy.as_ref().map(|p| p.to_dict()),
            "binding" => self.binding.as_ref().map(|b| b.to_dict()),
            "measurements" => Json::Arr(self.measurements.iter().map(|m| m.to_dict()).collect()),
            "intent_audit" => Json::Arr(self.intent_audit.clone()),
            "geometry_provenance" => self.geometry_provenance.clone(),
            "evaluated_pairs" => self.evaluated_pairs,
        }
    }
}

/// `assessment_results`: wrap an assessment into checker results.
pub fn assessment_results(assessment: MaskCopperAssessment) -> DRCResults {
    let mut result = DRCResults::with_rules_checked(1);
    result.set_rule("mask_to_copper", 1);
    for (index, m) in assessment.measurements.iter().enumerate() {
        let severity = if m.disposition == "intentional" {
            "info"
        } else {
            "error"
        };
        let mut items = m.opening_uuids.clone();
        items.push(m.conductor_uuid.clone());
        result.add(
            DRCViolation::new(
                "mask_to_copper",
                severity,
                format!(
                    "Mask-to-copper {}: {}; exposed area {} mm²; assessment measurement {index}",
                    m.relation,
                    m.disposition,
                    crate::utils::pyfmt::format_g(m.exposed_area_mm2, 9)
                ),
            )
            .at(m.location_mm.0, m.location_mm.1)
            .layer(m.copper_layer.clone())
            .actual(m.clearance_mm)
            .required_opt(assessment.policy.as_ref().map(|p| p.clearance_mm))
            .items(items),
        );
    }
    result.mask_copper_assessments.push(assessment);
    result
}

/// Parsed `kct.mask-copper-request.v1` (upstream `MaskCopperRequest`).
#[derive(Debug, Clone, Default)]
pub struct MaskCopperRequest {
    pub policy: Option<MaskCopperPolicy>,
    pub intents: Vec<MaskEscapeIntent>,
    /// Raw `native` options object (validated keys only).
    pub native_options: Json,
}

impl MaskCopperRequest {
    /// `MaskCopperRequest.from_file`.
    pub fn from_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let data = crate::pyjson::loads(&text)?;
        if data.get("schema").and_then(Json::as_str) != Some("kct.mask-copper-request.v1") {
            anyhow::bail!("Expected kct.mask-copper-request.v1 schema");
        }
        let s = |v: &Json, k: &str| -> anyhow::Result<String> {
            v.get(k)
                .and_then(Json::as_str)
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("{}", crate::pyjson::py_repr_str(k)))
        };
        let policy = match data.get("policy") {
            None | Some(Json::Null) => None,
            Some(p) => Some(MaskCopperPolicy::new(
                p.get("clearance_mm")
                    .and_then(Json::as_f64)
                    .ok_or_else(|| anyhow::anyhow!("'clearance_mm'"))?,
                &s(p, "source")?,
                &s(p, "process")?,
                &s(p, "revision")?,
            )?),
        };
        let mut intents = Vec::new();
        if let Some(list) = data.get("intents").and_then(Json::as_array) {
            for d in list {
                let b = d
                    .get("binding")
                    .ok_or_else(|| anyhow::anyhow!("'binding'"))?;
                let opt = |k: &str| b.get(k).and_then(Json::as_str).map(str::to_string);
                let binding = MaskSourceBinding::new(
                    &s(b, "source_sha256")?,
                    opt("project_sha256").as_deref(),
                    opt("rules_sha256").as_deref(),
                    &s(b, "export_profile_sha256")?,
                )?;
                let intent = MaskEscapeIntent {
                    binding,
                    owner_uuid: s(d, "owner_uuid")?,
                    conductor_uuid: s(d, "conductor_uuid")?,
                    mask_side: s(d, "mask_side")?,
                    rationale: s(d, "rationale")?,
                    scope: d
                        .get("scope")
                        .and_then(Json::as_str)
                        .unwrap_or("connected_escape")
                        .to_string(),
                };
                if !matches!(intent.mask_side.as_str(), "F.Mask" | "B.Mask")
                    || intent.scope != "connected_escape"
                    || intent.rationale.trim().is_empty()
                {
                    anyhow::bail!("Intent needs a mask side, connected_escape scope and rationale");
                }
                intents.push(intent);
            }
        }
        let native = data.get("native").cloned().unwrap_or_else(Json::obj);
        if let Json::Obj(items) = &native {
            const ALLOWED: &[&str] = &[
                "native_command",
                "native_python_command",
                "options",
                "artifact_dir",
                "scratch_dir",
            ];
            if items.iter().any(|(k, _)| !ALLOWED.contains(&k.as_str())) {
                anyhow::bail!("Unknown native mask request options");
            }
            for key in ["native_command", "native_python_command"] {
                if let Some(v) = native.get(key) {
                    let ok = v.as_array().is_some_and(|a| {
                        !a.is_empty() && a.iter().all(|p| p.as_str().is_some_and(|s| !s.is_empty()))
                    });
                    if !ok {
                        anyhow::bail!("{key} must be a nonempty argument list");
                    }
                }
            }
        }
        Ok(MaskCopperRequest {
            policy,
            intents,
            native_options: native,
        })
    }
}

/// `check_mask_to_copper(path, policy, intents, **native)`.
///
/// The native mask-geometry inspection (KiCad plot export + attributed
/// object geometry) is not ported yet, so a request with a policy reports
/// incomplete coverage rather than a false pass.
pub fn check_mask_to_copper(
    _path: &std::path::Path,
    policy: Option<&MaskCopperPolicy>,
    _intents: &[MaskEscapeIntent],
    _native_options: &Json,
) -> MaskCopperAssessment {
    let Some(policy) = policy else {
        return MaskCopperAssessment::with_reasons(
            "not_run",
            vec!["An explicit process-specific mask-to-copper policy is required".into()],
        );
    };
    MaskCopperAssessment {
        coverage: "incomplete".into(),
        policy: Some(policy.clone()),
        reasons: vec!["native mask geometry inspection is not available in this build".into()],
        ..Default::default()
    }
}

/// `DRCChecker.check_mask_to_copper` body.
pub fn check_requested(
    pcb: &crate::schema::pcb::Pcb,
    request: Option<&MaskCopperRequest>,
) -> DRCResults {
    let default = MaskCopperRequest::default();
    let request = request.unwrap_or(&default);
    let assessment = match pcb.path() {
        None => MaskCopperAssessment::with_reasons(
            "not_run",
            vec!["Mask geometry requires a saved source PCB".into()],
        ),
        Some(path) => match std::fs::read(path) {
            Err(e) => MaskCopperAssessment::with_reasons("incomplete", vec![e.to_string()]),
            Ok(raw) => {
                let current = pcb.sexp().to_string();
                let canonical = String::from_utf8(raw)
                    .map_err(|e| e.to_string())
                    .and_then(|t| {
                        crate::schema::pcb::Pcb::parse_str(&t)
                            .map(|p| p.sexp().to_string())
                            .map_err(|e| e.to_string())
                    });
                match canonical {
                    Err(e) => MaskCopperAssessment::with_reasons("incomplete", vec![e]),
                    Ok(c) if c != current => MaskCopperAssessment::with_reasons(
                        "incomplete",
                        vec!["PCB object differs from current source bytes".into()],
                    ),
                    Ok(_) => check_mask_to_copper(
                        path,
                        request.policy.as_ref(),
                        &request.intents,
                        &request.native_options,
                    ),
                }
            }
        },
    };
    assessment_results(assessment)
}
