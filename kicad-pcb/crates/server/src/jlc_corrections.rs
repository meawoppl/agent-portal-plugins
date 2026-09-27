//! JLCPCB CPL placement corrections carried as part-level KiCad fields.
//!
//! Two optional fields on a symbol (propagated to its footprint by *Update PCB
//! from Schematic*) or directly on a footprint describe how the assembler's
//! part model differs from the KiCad footprint:
//!
//! - `JLCPCB Rotation Offset`: degrees added to KiCad's rotation (KiCad
//!   convention, counter-clockwise positive as seen from the top), e.g. `-90`
//!   for a quarter turn clockwise.
//! - `JLCPCB Position Offset`: `x,y` in millimetres in the footprint-local
//!   frame (KiCad +Y down, before placement rotation and side flip).
//!
//! Because both live in the footprint frame they stay correct when the part
//! is moved, rotated, or flipped. The exporter maps the local offset to board
//! coordinates exactly as KiCad maps a pad offset (verified against pcbnew at
//! 0/90/180/270 degrees on both sides), then to CPL coordinates (+Y up).

use std::{collections::BTreeMap, path::Path};

use anyhow::{anyhow, Context, Result};
use serde::Serialize;

use crate::sexp::Sexp;

pub(crate) const ROTATION_FIELD: &str = "JLCPCB Rotation Offset";
pub(crate) const POSITION_FIELD: &str = "JLCPCB Position Offset";

fn field_key(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Which correction field (if any) a KiCad field name denotes. Matching
/// ignores case, spaces and punctuation.
pub(crate) fn correction_field(name: &str) -> Option<&'static str> {
    match field_key(name).as_str() {
        "jlcpcbrotationoffset" => Some(ROTATION_FIELD),
        "jlcpcbpositionoffset" => Some(POSITION_FIELD),
        _ => None,
    }
}

/// A part-level correction, in the footprint-local frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub(crate) struct PartCorrection {
    pub rotation_deg: f64,
    /// Footprint-local offset, KiCad +Y down.
    pub offset_mm: (f64, f64),
}

impl PartCorrection {
    pub(crate) fn has_offset(&self) -> bool {
        self.offset_mm != (0.0, 0.0)
    }

    /// Short human summary, e.g. `rot -90°` or `rot -90° · offset 0,-2.75 mm`.
    pub(crate) fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.rotation_deg != 0.0 || !self.has_offset() {
            parts.push(format!("rot {}°", num(self.rotation_deg)));
        }
        if self.has_offset() {
            parts.push(format!(
                "offset {},{} mm",
                num(self.offset_mm.0),
                num(self.offset_mm.1)
            ));
        }
        parts.join(" · ")
    }
}

/// Compact decimal: up to 6 places, trailing zeros trimmed, no `-0`.
pub(crate) fn num(value: f64) -> String {
    let text = format!("{:.6}", clean(value));
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// Suppress floating-point noise from trigonometry (e.g. `-1e-16`).
pub(crate) fn clean(value: f64) -> f64 {
    if value.abs() < 5e-10 {
        0.0
    } else {
        value
    }
}

/// Angle in `[0, 360)`, with float noise around 0/360 folded to 0.
pub(crate) fn normalize_degrees(value: f64) -> f64 {
    let value = clean(value).rem_euclid(360.0);
    if (value - 360.0).abs() < 5e-10 {
        0.0
    } else {
        value
    }
}

fn is_blank(raw: &str) -> bool {
    matches!(raw.trim(), "" | "~" | "-")
}

fn number(raw: &str, units: &[&str]) -> Option<f64> {
    let mut text = raw.trim().to_ascii_lowercase();
    for unit in units {
        if let Some(stripped) = text.strip_suffix(unit) {
            text = stripped.trim_end().to_string();
            break;
        }
    }
    let text = text.strip_prefix('+').unwrap_or(&text);
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// `-90`, `-90°`, `-90 deg`, `+90.5degrees`; empty means no correction.
pub(crate) fn parse_rotation(raw: &str) -> Result<Option<f64>> {
    if is_blank(raw) {
        return Ok(None);
    }
    number(raw, &["degrees", "degree", "deg", "°"])
        .map(Some)
        .ok_or_else(|| anyhow!("{ROTATION_FIELD} {raw:?} is not a number of degrees"))
}

/// `1.2,-0.5`, `1.2, -0.5`, `1.2mm,-0.5mm`, `(1.2; -0.5)`, `1.2 -0.5`; empty
/// means no correction.
pub(crate) fn parse_position(raw: &str) -> Result<Option<(f64, f64)>> {
    if is_blank(raw) {
        return Ok(None);
    }
    let bad = || anyhow!("{POSITION_FIELD} {raw:?} must be \"x,y\" in millimetres");
    let inner = raw
        .trim()
        .trim_start_matches(['(', '['])
        .trim_end_matches([')', ']']);
    let pieces: Vec<&str> = if inner.contains([',', ';']) {
        inner.split([',', ';']).collect()
    } else {
        inner.split_whitespace().collect()
    };
    let [x, y] = pieces.as_slice() else {
        return Err(bad());
    };
    let x = number(x, &["mm"]).ok_or_else(bad)?;
    let y = number(y, &["mm"]).ok_or_else(bad)?;
    Ok(Some((x, y)))
}

/// Reads the correction fields from a symbol's or footprint's field list.
/// `None` when neither field carries a value.
pub(crate) fn from_fields(fields: &[(String, String)]) -> Result<Option<PartCorrection>> {
    let mut rotation = None;
    let mut position = None;
    for (name, value) in fields {
        match correction_field(name) {
            Some(ROTATION_FIELD) => rotation = parse_rotation(value)?.or(rotation),
            Some(POSITION_FIELD) => position = parse_position(value)?.or(position),
            _ => {}
        }
    }
    if rotation.is_none() && position.is_none() {
        return Ok(None);
    }
    Ok(Some(PartCorrection {
        rotation_deg: rotation.unwrap_or(0.0),
        offset_mm: position.unwrap_or((0.0, 0.0)),
    }))
}

/// Where a resolved property correction came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FieldOrigin {
    Footprint,
    Schematic,
}

impl FieldOrigin {
    pub(crate) fn label(self) -> &'static str {
        match self {
            FieldOrigin::Footprint => "footprint",
            FieldOrigin::Schematic => "schematic",
        }
    }
}

/// Resolves one reference's correction. The footprint property wins over the
/// symbol field; any disagreement is reported in `warnings`. Invalid values
/// are errors naming the reference.
pub(crate) fn resolve(
    reference: &str,
    schematic: Option<&[(String, String)]>,
    footprint: Option<&[(String, String)]>,
    warnings: &mut Vec<String>,
) -> Result<Option<(PartCorrection, FieldOrigin)>> {
    let read = |fields: Option<&[(String, String)]>, origin: &str| {
        fields
            .map(from_fields)
            .transpose()
            .map(Option::flatten)
            .with_context(|| format!("{reference}: invalid {origin} JLCPCB correction"))
    };
    let sch = read(schematic, "schematic")?;
    let fp = read(footprint, "footprint")?;
    if sch != fp && schematic.is_some() && footprint.is_some() {
        let show = |value: &Option<PartCorrection>| {
            value
                .map(|value| value.summary())
                .unwrap_or_else(|| "none".into())
        };
        warnings.push(format!(
            "{reference}: footprint JLCPCB correction ({}) differs from schematic ({}); using the footprint. Run Update PCB from Schematic to resync.",
            show(&fp),
            show(&sch)
        ));
    }
    Ok(match (fp, sch) {
        (Some(value), _) => Some((value, FieldOrigin::Footprint)),
        (None, Some(value)) if footprint.is_none() => Some((value, FieldOrigin::Schematic)),
        // The footprint exists without the field: it wins (already warned).
        (None, _) => None,
    })
}

/// Footprint placement as stored in the `.kicad_pcb` (`(at x y rot)`, KiCad
/// board coordinates, +Y down).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub(crate) struct Placement {
    pub x: f64,
    pub y: f64,
    pub rotation_deg: f64,
    pub bottom: bool,
}

impl Placement {
    pub(crate) fn of_footprint(node: &Sexp) -> Placement {
        let at = node.child("at");
        let get = |index: usize| {
            at.and_then(|at| at.str_at(index))
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        Placement {
            x: get(1),
            y: get(2),
            rotation_deg: get(3),
            bottom: node
                .child("layer")
                .and_then(|layer| layer.str_at(1))
                .is_some_and(|layer| layer == "B.Cu"),
        }
    }
}

/// Maps a footprint-local offset to a KiCad board-frame delta (+Y down), the
/// same way KiCad places a pad: bottom-side footprints mirror local Y, then
/// the footprint rotation (counter-clockwise on screen) is applied.
pub(crate) fn local_to_board(offset: (f64, f64), rotation_deg: f64, bottom: bool) -> (f64, f64) {
    let (x, y) = (offset.0, if bottom { -offset.1 } else { offset.1 });
    let (sin, cos) = rotation_deg.to_radians().sin_cos();
    (clean(x * cos + y * sin), clean(-x * sin + y * cos))
}

/// KiCad board delta (+Y down) to CPL delta (+Y up, X unchanged on both
/// sides, matching `kicad-cli pcb export pos`).
pub(crate) fn board_to_cpl(delta: (f64, f64)) -> (f64, f64) {
    (clean(delta.0), clean(-delta.1))
}

/// CPL rotation delta. A footprint-frame rotation appears mirrored when the
/// footprint is flipped to the bottom, so it is subtracted there.
pub(crate) fn cpl_rotation_delta(rotation_deg: f64, bottom: bool) -> f64 {
    if bottom {
        -rotation_deg
    } else {
        rotation_deg
    }
}

/// A property correction resolved against the board placement.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Resolved {
    pub correction: PartCorrection,
    pub origin: FieldOrigin,
    pub placement: Placement,
}

impl Resolved {
    pub(crate) fn cpl_offset(&self) -> (f64, f64) {
        board_to_cpl(local_to_board(
            self.correction.offset_mm,
            self.placement.rotation_deg,
            self.placement.bottom,
        ))
    }

    pub(crate) fn cpl_rotation(&self) -> f64 {
        cpl_rotation_delta(self.correction.rotation_deg, self.placement.bottom)
    }
}

/// Property corrections for every board reference, from the schematic (with
/// hierarchical sheets) and the PCB footprints.
pub(crate) fn collect(
    schematic: Option<&Path>,
    pcb: &Path,
    warnings: &mut Vec<String>,
) -> Result<BTreeMap<String, Resolved>> {
    let board = crate::library::load_pcb(pcb)?;
    let mut sch_fields: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    if let Some(path) = schematic {
        for instance in crate::library::load_schematic(path)?.instances {
            for reference in &instance.references {
                sch_fields.insert(reference.clone(), instance.fields.clone());
            }
        }
    }
    let mut out = BTreeMap::new();
    for footprint in &board.footprints {
        if footprint.reference.is_empty() {
            continue;
        }
        let resolved = resolve(
            &footprint.reference,
            sch_fields.get(&footprint.reference).map(Vec::as_slice),
            Some(&footprint.fields),
            warnings,
        )?;
        if let Some((correction, origin)) = resolved {
            out.insert(
                footprint.reference.clone(),
                Resolved {
                    correction,
                    origin,
                    placement: Placement::of_footprint(&footprint.node),
                },
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn parses_tolerant_formats() {
        assert_eq!(parse_rotation("-90").unwrap(), Some(-90.0));
        assert_eq!(parse_rotation(" +90.5 deg ").unwrap(), Some(90.5));
        assert_eq!(parse_rotation("-90°").unwrap(), Some(-90.0));
        assert_eq!(parse_rotation("").unwrap(), None);
        assert_eq!(parse_rotation("~").unwrap(), None);
        assert!(parse_rotation("left").is_err());
        assert!(parse_rotation("nan").is_err());
        for raw in [
            "1.2,-0.5",
            "1.2, -0.5",
            "1.2mm,-0.5mm",
            " 1.2 mm , -0.5 MM ",
            "(1.2; -0.5)",
            "1.2 -0.5",
            "+1.2,-0.5",
        ] {
            assert_eq!(parse_position(raw).unwrap(), Some((1.2, -0.5)), "{raw}");
        }
        assert_eq!(parse_position("  ").unwrap(), None);
        for raw in ["1.2", "1,2,3", "a,b", "1.2in,0", ","] {
            assert!(parse_position(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn reads_fields_case_insensitively() {
        let value = from_fields(&fields(&[
            ("LCSC", "C1"),
            ("jlcpcb rotation offset", "-90"),
            ("JLCPCB_Position_Offset", "0,-2.75"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(value.rotation_deg, -90.0);
        assert_eq!(value.offset_mm, (0.0, -2.75));
        assert_eq!(value.summary(), "rot -90° · offset 0,-2.75 mm");
        assert_eq!(from_fields(&fields(&[(ROTATION_FIELD, "")])).unwrap(), None);
        assert_eq!(
            from_fields(&fields(&[(ROTATION_FIELD, "0")]))
                .unwrap()
                .unwrap()
                .summary(),
            "rot 0°"
        );
    }

    /// Expected board deltas for local (1, 2) mm, taken from pcbnew
    /// (KiCad 10) pad positions: top at 0/90/180/270 and bottom after flip.
    #[test]
    fn local_to_board_matches_kicad_pad_transform() {
        let local = (1.0, 2.0);
        let top = [
            (0.0, (1.0, 2.0)),
            (90.0, (2.0, -1.0)),
            (180.0, (-1.0, -2.0)),
            (270.0, (-2.0, 1.0)),
            (-90.0, (-2.0, 1.0)),
        ];
        for (rotation, expected) in top {
            assert!(
                close(local_to_board(local, rotation, false), expected),
                "top {rotation}"
            );
        }
        let bottom = [
            (0.0, (1.0, -2.0)),
            (90.0, (-2.0, -1.0)),
            (180.0, (-1.0, 2.0)),
            (270.0, (2.0, 1.0)),
        ];
        for (rotation, expected) in bottom {
            assert!(
                close(local_to_board(local, rotation, true), expected),
                "bottom {rotation}"
            );
        }
        assert_eq!(board_to_cpl((1.0, 2.0)), (1.0, -2.0));
        assert_eq!(cpl_rotation_delta(-90.0, false), -90.0);
        assert_eq!(cpl_rotation_delta(-90.0, true), 90.0);
    }

    #[test]
    fn footprint_wins_over_schematic() {
        let mut warnings = Vec::new();
        let sch = fields(&[(ROTATION_FIELD, "-90")]);
        let fp = fields(&[(ROTATION_FIELD, "90")]);
        let (value, origin) = resolve("U1", Some(&sch), Some(&fp), &mut warnings)
            .unwrap()
            .unwrap();
        assert_eq!((value.rotation_deg, origin), (90.0, FieldOrigin::Footprint));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("U1: footprint"));

        warnings.clear();
        let (value, origin) = resolve("U1", Some(&sch), None, &mut warnings)
            .unwrap()
            .unwrap();
        assert_eq!(
            (value.rotation_deg, origin),
            (-90.0, FieldOrigin::Schematic)
        );
        assert!(warnings.is_empty());

        // Footprint lacks the field: the footprint (no correction) wins.
        let empty = fields(&[("LCSC", "C1")]);
        assert_eq!(
            resolve("U1", Some(&sch), Some(&empty), &mut warnings).unwrap(),
            None
        );
        assert_eq!(warnings.len(), 1);

        let bad = fields(&[(POSITION_FIELD, "3")]);
        let err = resolve("J9", None, Some(&bad), &mut warnings).unwrap_err();
        assert!(format!("{err:#}").contains("J9"));
    }

    #[test]
    fn placement_reads_footprint_at_and_side() {
        let node = crate::sexp::parse(
            r#"(footprint "X:Y" (layer "B.Cu") (at 65 47 90) (property "Reference" "SW1"))"#,
        )
        .unwrap();
        assert_eq!(
            Placement::of_footprint(&node),
            Placement {
                x: 65.0,
                y: 47.0,
                rotation_deg: 90.0,
                bottom: true
            }
        );
    }
}
