//! Router-grid pad alignment preflight (port of
//! `kicad_tools.router.preflight`, with the slice of
//! `router.io.load_pads_for_analysis` / `auto_select_grid_resolution` it
//! needs).

use crate::pyjson::py_float_repr;
use crate::sexp::SExp;

pub const DEFAULT_PAD_GRID_TOLERANCE_MM: f64 = 0.05;
pub const AUTO_DERIVED_TOLERANCE_HARD_CAP_MM: f64 = 0.15;
pub const AUTO_DERIVED_TOLERANCE_FLOOR_MM: f64 = DEFAULT_PAD_GRID_TOLERANCE_MM;
pub const AUTO_DERIVED_TOLERANCE_MARGIN_MM: f64 = 0.005;
pub const AUTO_DERIVED_TOLERANCE_PERCENTILE: f64 = 0.99;

/// Pad record for grid analysis (sheet-absolute coordinates).
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisPad {
    pub x: f64,
    pub y: f64,
    pub reference: String,
    pub pin: String,
    pub footprint_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreflightOffGridPad {
    pub reference: String,
    pub pin: String,
    pub x: f64,
    pub y: f64,
    pub offset_mm: f64,
    pub footprint_name: String,
}

impl PreflightOffGridPad {
    pub fn label(&self) -> String {
        if !self.reference.is_empty() && !self.pin.is_empty() {
            format!("{}.{}", self.reference, self.pin)
        } else if !self.reference.is_empty() {
            self.reference.clone()
        } else {
            format!("({:.3}, {:.3})", self.x, self.y)
        }
    }

    pub fn message(&self, grid: f64, suggested: Option<f64>) -> String {
        let mut lines = vec![format!(
            "Pad {} at ({:.3}, {:.3}) is off-grid by {:.3}mm (grid {}mm).",
            self.label(),
            self.x,
            self.y,
            self.offset_mm,
            py_float_repr(grid)
        )];
        if !self.footprint_name.is_empty() {
            lines.push(format!("Footprint: {}", self.footprint_name));
        }
        match suggested {
            Some(s) => lines.push(format!(
                "Suggested fix: round pad position OR set finer router grid ({}mm would align all \
                 pads).",
                py_float_repr(s)
            )),
            None => lines.push("Suggested fix: round pad position to the router grid.".into()),
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OffGridReport {
    pub grid_resolution: f64,
    pub threshold: f64,
    pub off_grid_pads: Vec<PreflightOffGridPad>,
    pub suggested_grid: Option<f64>,
    pub total_pads: usize,
}

impl OffGridReport {
    /// Insertion-ordered `ref -> pads`.
    pub fn grouped_by_ref(&self) -> Vec<(String, Vec<&PreflightOffGridPad>)> {
        let mut out: Vec<(String, Vec<&PreflightOffGridPad>)> = Vec::new();
        for p in &self.off_grid_pads {
            match out.iter_mut().find(|(r, _)| *r == p.reference) {
                Some(e) => e.1.push(p),
                None => out.push((p.reference.clone(), vec![p])),
            }
        }
        out
    }
}

fn first_at(node: &SExp) -> Option<(f64, f64, Option<f64>)> {
    let at = node.get("at")?;
    Some((at.float_at(0)?, at.float_at(1)?, at.float_at(2)))
}

/// Footprint reference with `PCB.load` precedence (modern `property`
/// overrides legacy `fp_text reference`; last of each kind wins).
fn footprint_reference(fp: &SExp) -> String {
    let mut modern: Option<String> = None;
    let mut legacy = String::new();
    for c in &fp.children {
        match c.tag() {
            Some("property") if c.string_at(0) == Some("Reference") => {
                modern = Some(c.text_at(1).unwrap_or_default());
            }
            Some("fp_text") if c.string_at(0) == Some("reference") => {
                legacy = c.text_at(1).unwrap_or_default();
            }
            _ => {}
        }
    }
    modern.unwrap_or(legacy)
}

/// `load_pads_for_analysis` (positions / refs / pins / footprint names).
pub fn load_pads_for_analysis(root: &SExp) -> Vec<AnalysisPad> {
    let mut out = Vec::new();
    for fp in root
        .children
        .iter()
        .filter(|c| c.has_tag("footprint") || c.has_tag("module"))
    {
        let name = if fp.has_tag("footprint") {
            fp.children
                .first()
                .filter(|c| c.is_atom())
                .and_then(|c| c.value.as_ref())
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        } else {
            String::new()
        };
        let reference = footprint_reference(fp);
        let Some((fx, fy, frot)) = first_at(fp) else {
            continue;
        };
        let r = (-frot.unwrap_or(0.0)).to_radians();
        let (c, s) = (r.cos(), r.sin());
        for pad in fp.find_all("pad") {
            let pin = pad.text_at(0).unwrap_or_default();
            let Some((px, py, _)) = first_at(pad) else {
                continue;
            };
            out.push(AnalysisPad {
                x: fx + px * c - py * s,
                y: fy + px * s + py * c,
                reference: reference.clone(),
                pin,
                footprint_name: name.clone(),
            });
        }
    }
    out
}

/// Python `round(x)` (banker's rounding) as f64.
fn py_round0(x: f64) -> f64 {
    x.round_ties_even()
}

fn axis_distance(value: f64, res: f64, offset: f64) -> f64 {
    let nearest = py_round0((value - offset) / res) * res + offset;
    (value - nearest).abs()
}

fn l2_distance(x: f64, y: f64, res: f64) -> f64 {
    let dx = axis_distance(x, res, 0.0);
    let dy = axis_distance(y, res, 0.0);
    (dx * dx + dy * dy).powf(0.5)
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 {
        a.abs()
    } else {
        gcd(b, a % b)
    }
}

fn gcd_candidates(pads: &[AnalysisPad]) -> Vec<f64> {
    if pads.len() < 2 {
        return vec![];
    }
    let snap = |v: f64| py_round0(v / 0.005) * 0.005;
    let uniq = |it: Vec<f64>| {
        let mut v = it;
        v.sort_by(f64::total_cmp);
        v.dedup();
        v
    };
    let xs = uniq(pads.iter().map(|p| snap(p.x)).collect());
    let ys = uniq(pads.iter().map(|p| snap(p.y)).collect());
    let mut deltas = Vec::new();
    for coords in [&xs, &ys] {
        for w in coords.windows(2) {
            let d = w[1] - w[0];
            if d > 0.001 {
                deltas.push(d);
            }
        }
    }
    let um: Vec<i64> = deltas
        .iter()
        .map(|d| py_round0(d * 1000.0) as i64)
        .filter(|d| *d > 0)
        .collect();
    if um.is_empty() {
        return vec![];
    }
    let g = um[1..].iter().fold(um[0], |a, &b| gcd(a, b));
    let mm = g as f64 / 1000.0;
    let mut out: Vec<f64> = [mm, mm * 2.0, mm * 5.0]
        .into_iter()
        .filter(|c| *c >= 0.005)
        .collect();
    out.sort_by(|a, b| b.total_cmp(a));
    out.dedup();
    out
}

/// Candidate resolutions `auto_select_grid_resolution` evaluates (no board
/// size, so no memory filter).
fn candidates_tried(pads: &[AnalysisPad], clearance: f64) -> Vec<f64> {
    let mut c = vec![0.5, 0.25, 0.127, 0.1, 0.065, 0.0635, 0.05, 0.0508];
    for g in gcd_candidates(pads) {
        if !c.contains(&g) {
            c.push(g);
        }
    }
    c.sort_by(|a, b| b.total_cmp(a));
    let valid: Vec<f64> = c.into_iter().filter(|r| *r <= clearance).collect();
    if valid.is_empty() {
        vec![clearance / 2.0]
    } else {
        valid
    }
}

fn suggest_finer_grid(pads: &[AnalysisPad], current: f64, clearance: f64) -> Option<f64> {
    if pads.is_empty() {
        return None;
    }
    let mut cands: Vec<f64> = candidates_tried(pads, clearance)
        .into_iter()
        .filter(|r| *r < current)
        .collect();
    cands.sort_by(|a, b| b.total_cmp(a));
    cands.dedup();
    for res in cands {
        let th = res / 10.0;
        let eps = 1e-9f64.max(th * 1e-6);
        if pads.iter().all(|p| l2_distance(p.x, p.y, res) <= th + eps) {
            return Some(res);
        }
    }
    None
}

fn percentile(values: &[f64], pct: f64) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    if pct <= 0.0 {
        return v[0];
    }
    if pct >= 1.0 {
        return v[v.len() - 1];
    }
    let rank = pct * (v.len() - 1) as f64;
    let lo = rank as usize;
    let hi = (lo + 1).min(v.len() - 1);
    let frac = rank - lo as f64;
    v[lo] + (v[hi] - v[lo]) * frac
}

/// `compute_pad_grid_tolerance`.
pub fn compute_pad_grid_tolerance(pads: &[AnalysisPad], grid: f64) -> f64 {
    if pads.is_empty() {
        return AUTO_DERIVED_TOLERANCE_FLOOR_MM;
    }
    let offsets: Vec<f64> = pads.iter().map(|p| l2_distance(p.x, p.y, grid)).collect();
    let raw = percentile(&offsets, AUTO_DERIVED_TOLERANCE_PERCENTILE) + AUTO_DERIVED_TOLERANCE_MARGIN_MM;
    AUTO_DERIVED_TOLERANCE_HARD_CAP_MM.min(AUTO_DERIVED_TOLERANCE_FLOOR_MM.max(raw))
}

/// `check_pad_grid_alignment` over a parsed board.
pub fn check_pad_grid_alignment(
    root: &SExp,
    grid: f64,
    threshold: Option<f64>,
    clearance: f64,
    auto_derive: bool,
) -> OffGridReport {
    let pads = load_pads_for_analysis(root);
    let threshold = match threshold {
        Some(t) => t,
        None if auto_derive => compute_pad_grid_tolerance(&pads, grid),
        None => DEFAULT_PAD_GRID_TOLERANCE_MM,
    };
    let eps = 1e-9f64.max(threshold * 1e-6);
    let off: Vec<PreflightOffGridPad> = pads
        .iter()
        .filter_map(|p| {
            let d = l2_distance(p.x, p.y, grid);
            (d > threshold + eps).then(|| PreflightOffGridPad {
                reference: p.reference.clone(),
                pin: p.pin.clone(),
                x: p.x,
                y: p.y,
                offset_mm: d,
                footprint_name: p.footprint_name.clone(),
            })
        })
        .collect();
    let suggested = if off.is_empty() {
        None
    } else {
        suggest_finer_grid(&pads, grid, clearance)
    };
    OffGridReport {
        grid_resolution: grid,
        threshold,
        off_grid_pads: off,
        suggested_grid: suggested,
        total_pads: pads.len(),
    }
}
