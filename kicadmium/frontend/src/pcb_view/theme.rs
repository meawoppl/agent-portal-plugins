//! Board palette and layer paint order, matching the KiCanvas view: the
//! bundled American Embedded dark theme (`board` section) and KiCanvas's
//! layer stack (back layers first, holes above copper, drawing layers on
//! top). Like KiCanvas, every layer blends at its colour's alpha (forced to
//! 0.8 for non-copper layers and the hole/wall passes).

use std::collections::HashMap;

use serde_json::Value;

const THEME_JSON: &str = include_str!("../../static/kicad-viewer/american-embedded-dark.json");

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba(pub f64, pub f64, pub f64, pub f64);

impl Rgba {
    fn parse(css: &str) -> Option<Rgba> {
        let css = css.trim();
        let inner = css
            .strip_prefix("rgba(")
            .or_else(|| css.strip_prefix("rgb("))?
            .strip_suffix(')')?;
        let parts: Vec<f64> = inner
            .split(',')
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        match parts.as_slice() {
            [r, g, b] => Some(Rgba(*r, *g, *b, 1.0)),
            [r, g, b, a] => Some(Rgba(*r, *g, *b, *a)),
            _ => None,
        }
    }

    /// Composite onto `bg` (opaque result).
    pub fn over(self, bg: Rgba) -> Rgba {
        let a = self.3;
        Rgba(
            self.0 * a + bg.0 * (1.0 - a),
            self.1 * a + bg.1 * (1.0 - a),
            self.2 * a + bg.2 * (1.0 - a),
            1.0,
        )
    }

    pub fn with_alpha(self, a: f64) -> Rgba {
        Rgba(self.0, self.1, self.2, a)
    }

    pub fn mix(self, other: Rgba, t: f64) -> Rgba {
        Rgba(
            self.0 + (other.0 - self.0) * t,
            self.1 + (other.1 - self.1) * t,
            self.2 + (other.2 - self.2) * t,
            self.3 + (other.3 - self.3) * t,
        )
    }

    pub fn css(self) -> String {
        format!(
            "rgba({},{},{},{})",
            self.0.round(),
            self.1.round(),
            self.2.round(),
            self.3
        )
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub background: Rgba,
    pub grid: Rgba,
    pub via_hole: Rgba,
    pub via_wall: Rgba,
    pub pad_wall: Rgba,
    pub npth: Rgba,
    pub selection: Rgba,
    colors: HashMap<String, Rgba>,
}

impl Theme {
    pub fn load() -> Theme {
        let json: Value = serde_json::from_str(THEME_JSON).unwrap_or(Value::Null);
        let board = json.get("board").cloned().unwrap_or(Value::Null);
        let get = |path: &[&str], fallback: Rgba| {
            let mut v = &board;
            for p in path {
                v = v.get(p).unwrap_or(&Value::Null);
            }
            v.as_str().and_then(Rgba::parse).unwrap_or(fallback)
        };
        let mut colors = HashMap::new();
        if let Some(map) = board.as_object() {
            for (k, v) in map {
                if let Some(c) = v.as_str().and_then(Rgba::parse) {
                    colors.insert(k.clone(), c);
                }
            }
        }
        if let Some(map) = board.get("copper").and_then(Value::as_object) {
            for (k, v) in map {
                if let Some(c) = v.as_str().and_then(Rgba::parse) {
                    colors.insert(format!("copper.{k}"), c);
                }
            }
        }
        let background = get(&["background"], Rgba(30.0, 30.0, 30.0, 1.0));
        Theme {
            background,
            grid: get(&["grid"], Rgba(50.0, 50.0, 50.0, 1.0)),
            via_hole: get(&["via_hole"], Rgba(140.0, 115.0, 80.0, 0.8)).with_alpha(0.8),
            via_wall: get(&["via_through"], Rgba(200.0, 200.0, 200.0, 1.0)).with_alpha(0.8),
            pad_wall: get(&["pad_through_hole"], Rgba(143.0, 188.0, 187.0, 0.75)).with_alpha(0.8),
            npth: get(&["non_plated_hole"], Rgba(26.0, 196.0, 210.0, 1.0)).with_alpha(0.8),
            selection: Rgba(64.0, 169.0, 255.0, 1.0),
            colors,
        }
    }

    /// Layer colour as KiCanvas paints it (non-copper alpha 0.8).
    pub fn layer(&self, name: &str) -> Rgba {
        let key = if let Some(stem) = name.strip_suffix(".Cu") {
            format!("copper.{}", stem.to_ascii_lowercase())
        } else {
            name.replace('.', "_").to_ascii_lowercase()
        };
        let mut c = self
            .colors
            .get(&key)
            .copied()
            .unwrap_or(Rgba(200.0, 200.0, 200.0, 1.0));
        if !name.ends_with(".Cu") {
            c.3 = 0.8;
        }
        c
    }
}

/// What a paint step draws.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Pass {
    /// Graphics, text, tracks (everything owned by the layer itself).
    Layer(String),
    Pads(String),
    Zones(String),
    ViaWalls,
    PadWalls,
    PadHoles,
    ViaHoles,
    NonPlatedHoles,
}

fn copper_rank(name: &str) -> Option<usize> {
    match name {
        "F.Cu" => Some(0),
        "B.Cu" => Some(1000),
        _ => name
            .strip_prefix("In")
            .and_then(|n| n.strip_suffix(".Cu"))
            .and_then(|n| n.parse().ok()),
    }
}

const SIDE: [&str; 7] = ["Cu", "Mask", "SilkS", "Adhes", "Paste", "CrtYd", "Fab"];
const TOP_DRAWING: [&str; 6] = [
    "Dwgs.User",
    "Cmts.User",
    "Eco1.User",
    "Eco2.User",
    "Edge.Cuts",
    "Margin",
];

/// Front-most-first stack (KiCanvas `LayerSet` order) for the board's
/// layers; `flipped` puts the back side on top.
fn stack(names: &[String], flipped: bool) -> Vec<Pass> {
    let (near, far) = if flipped { ("B", "F") } else { ("F", "B") };
    let has = |n: &str| names.iter().any(|x| x == n);
    let mut out = Vec::new();
    for n in TOP_DRAWING {
        if has(n) {
            out.push(Pass::Layer(n.to_string()));
        }
    }
    let mut users: Vec<&String> = names
        .iter()
        .filter(|n| {
            !TOP_DRAWING.contains(&n.as_str())
                && copper_rank(n).is_none()
                && !SIDE
                    .iter()
                    .any(|s| *n == &format!("F.{s}") || *n == &format!("B.{s}"))
        })
        .collect();
    users.sort_by_key(|n| {
        n.strip_prefix("User.")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(usize::MAX)
    });
    out.extend(users.into_iter().map(|n| Pass::Layer(n.clone())));
    out.extend([
        Pass::NonPlatedHoles,
        Pass::ViaHoles,
        Pass::PadHoles,
        Pass::PadWalls,
        Pass::ViaWalls,
    ]);
    let copper_group = |out: &mut Vec<Pass>, name: &str| {
        if has(name) {
            out.push(Pass::Zones(name.to_string()));
            out.push(Pass::Pads(name.to_string()));
            out.push(Pass::Layer(name.to_string()));
        }
    };
    let side = |out: &mut Vec<Pass>, prefix: &str| {
        for s in SIDE.iter().skip(1) {
            let n = format!("{prefix}.{s}");
            if has(&n) {
                out.push(Pass::Layer(n));
            }
        }
    };
    copper_group(&mut out, &format!("{near}.Cu"));
    side(&mut out, near);
    let mut inner: Vec<&String> = names
        .iter()
        .filter(|n| copper_rank(n).is_some_and(|r| r > 0 && r < 1000))
        .collect();
    inner.sort_by_key(|n| copper_rank(n));
    if flipped {
        inner.reverse();
    }
    for n in inner {
        copper_group(&mut out, n);
    }
    copper_group(&mut out, &format!("{far}.Cu"));
    side(&mut out, far);
    out
}

/// Paint order, back-most first.
pub fn paint_order(names: &[String], flipped: bool) -> Vec<Pass> {
    let mut s = stack(names, flipped);
    s.reverse();
    s
}

/// Layer-panel order (KiCanvas `in_ui_order`), then anything else.
pub fn ui_order(names: &[String]) -> Vec<String> {
    let mut copper: Vec<&String> = names.iter().filter(|n| copper_rank(n).is_some()).collect();
    copper.sort_by_key(|n| copper_rank(n));
    let fixed = [
        "F.Adhes",
        "B.Adhes",
        "F.Paste",
        "B.Paste",
        "F.SilkS",
        "B.SilkS",
        "F.Mask",
        "B.Mask",
        "Dwgs.User",
        "Cmts.User",
        "Eco1.User",
        "Eco2.User",
        "Edge.Cuts",
        "Margin",
        "F.CrtYd",
        "B.CrtYd",
        "F.Fab",
        "B.Fab",
    ];
    let mut out: Vec<String> = copper.into_iter().cloned().collect();
    for f in fixed {
        if names.iter().any(|n| n == f) {
            out.push(f.to_string());
        }
    }
    let mut rest: Vec<&String> = names.iter().filter(|n| !out.contains(n)).collect();
    rest.sort_by_key(|n| {
        n.strip_prefix("User.")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(usize::MAX)
    });
    out.extend(rest.into_iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_matches_kicanvas_stack() {
        let names: Vec<String> = ["F.Cu", "In1.Cu", "B.Cu", "F.SilkS", "B.SilkS", "Edge.Cuts"]
            .into_iter()
            .map(String::from)
            .collect();
        let order = paint_order(&names, false);
        let pos = |p: &Pass| order.iter().position(|x| x == p).unwrap();
        assert!(pos(&Pass::Layer("B.SilkS".into())) < pos(&Pass::Layer("B.Cu".into())));
        assert!(pos(&Pass::Layer("B.Cu".into())) < pos(&Pass::Layer("In1.Cu".into())));
        assert!(pos(&Pass::Layer("F.Cu".into())) < pos(&Pass::Zones("F.Cu".into())));
        assert!(pos(&Pass::ViaWalls) < pos(&Pass::ViaHoles));
        assert_eq!(order.last(), Some(&Pass::Layer("Edge.Cuts".into())));
        let flipped = paint_order(&names, true);
        let fpos = |p: &Pass| flipped.iter().position(|x| x == p).unwrap();
        assert!(fpos(&Pass::Layer("F.Cu".into())) < fpos(&Pass::Layer("B.Cu".into())));
    }

    #[test]
    fn theme_colors() {
        let t = Theme::load();
        assert_eq!(t.background, Rgba(30.0, 30.0, 30.0, 1.0));
        let f = t.layer("F.Cu");
        assert!(f.0 > f.2, "front copper is warm: {f:?}");
        assert!((f.3 - 0.749).abs() < 1e-9);
        assert_eq!(t.layer("F.SilkS").3, 0.8);
    }
}
