//! Read-only region inventory shared by CLI and annotation surfaces.
use crate::lint::{
    copper,
    model::{Board, Point},
};
use anyhow::{bail, Result};
use geo::Intersects;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Bounds {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}
impl Bounds {
    pub fn validate(self) -> Result<()> {
        if ![self.x0, self.y0, self.x1, self.y1]
            .iter()
            .all(|n| n.is_finite())
            || self.x0 >= self.x1
            || self.y0 >= self.y1
        {
            bail!("region must have finite increasing x/y bounds");
        }
        Ok(())
    }
    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x0 && p.x <= self.x1 && p.y >= self.y0 && p.y <= self.y1
    }
    fn touches(self, b: (f64, f64, f64, f64)) -> bool {
        b.0 <= self.x1 && b.2 >= self.x0 && b.1 <= self.y1 && b.3 >= self.y0
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Inventory {
    pub schema: u32,
    pub source_sha256: String,
    pub region: Bounds,
    pub counts: BTreeMap<String, usize>,
    pub items: Vec<Value>,
    pub omitted: usize,
    pub limitations: Vec<String>,
}
pub fn query(
    source: &str,
    bounds: Bounds,
    layers: &[String],
    nets: &[String],
    limit: usize,
) -> Result<Inventory> {
    bounds.validate()?;
    let board = Board::read(source)?;
    let extra = copper::Extra::read(source, &board)?;
    let pcb = crate::schema::pcb::Pcb::from_sexp(crate::sexp::parse(source)?)?;
    let rect = geo::Rect::new(
        geo::Coord {
            x: bounds.x0,
            y: bounds.y0,
        },
        geo::Coord {
            x: bounds.x1,
            y: bounds.y1,
        },
    )
    .to_polygon();
    let net_ok = |net: &str| nets.is_empty() || nets.iter().any(|n| n == net);
    let layer_ok = |ls: &[String]| {
        layers.is_empty() || layers.iter().any(|l| crate::lint::model::on_layer(ls, l))
    };
    let mut items = Vec::new();
    for fp in pcb.footprints() {
        let courtyard = crate::geometry::pcb_adapters::courtyard_geom(
            fp,
            if fp.layer.starts_with('B') { "B" } else { "F" },
        )
        .and_then(|g| g.bounds());
        let nearby_pad = board
            .pads
            .iter()
            .any(|p| p.reference == fp.reference && copper::pad_poly(p).intersects(&rect));
        if layer_ok(std::slice::from_ref(&fp.layer))
            && (nets.is_empty() || fp.pads.iter().any(|p| net_ok(&p.net_name)))
            && (courtyard.is_some_and(|b| bounds.touches(b))
                || nearby_pad
                || bounds.contains(Point {
                    x: fp.position.0,
                    y: fp.position.1,
                }))
        {
            items.push(json!({"kind":"footprint","id":fp.uuid,"reference":fp.reference,"value":fp.value,"at":{"x":fp.position.0,"y":fp.position.1},"rotation":fp.rotation,"side":if fp.layer.starts_with('B'){"back"}else{"front"},"layer":fp.layer,"courtyard_bbox":courtyard}));
        }
    }
    for pad in &board.pads {
        if net_ok(&pad.net) && layer_ok(&pad.layers) && copper::pad_poly(pad).intersects(&rect) {
            let mut item = serde_json::to_value(pad)?;
            item["kind"] = json!("pad");
            items.push(item);
        }
    }
    for track in &board.tracks {
        if net_ok(&track.net)
            && layer_ok(std::slice::from_ref(&track.layer))
            && copper::line(&[track.a, track.b], track.width).intersects(&rect)
        {
            let mut item = serde_json::to_value(track)?;
            item["kind"] = json!("track");
            items.push(item);
        }
    }
    for arc in &extra.arcs {
        if net_ok(&arc.net)
            && layer_ok(std::slice::from_ref(&arc.layer))
            && copper::line(&arc.points, arc.width).intersects(&rect)
        {
            let mut item = serde_json::to_value(arc)?;
            item["kind"] = json!("arc");
            items.push(item);
        }
    }
    for via in &board.vias {
        let via_layers = board.via_layers(via);
        if net_ok(&via.net)
            && layer_ok(&via_layers)
            && copper::disk(via.at, via.size / 2.).intersects(&rect)
        {
            let mut item = serde_json::to_value(via)?;
            item["kind"] = json!("via");
            item["layers"] = json!(via_layers);
            items.push(item);
        }
    }
    for zone in &extra.zones {
        if net_ok(&zone.net)
            && layer_ok(std::slice::from_ref(&zone.layer))
            && zone
                .outlines
                .iter()
                .any(|p| copper::polygon(p).intersects(&rect))
        {
            items.push(json!({"kind":"zone","id":zone.id,"net":zone.net,"layer":zone.layer,"outlines":zone.outlines,"filled":!zone.filled.is_empty()}));
        }
    }
    items.sort_by_key(|item| format!("{}:{}:{}", item["kind"], item["reference"], item["id"]));
    let mut counts = BTreeMap::new();
    for item in &items {
        *counts
            .entry(item["kind"].as_str().unwrap().to_owned())
            .or_insert(0) += 1;
    }
    let total = items.len();
    if limit > 0 {
        items.truncate(limit);
    }
    let omitted = total - items.len();
    Ok(Inventory {
        schema: 1,
        source_sha256: crate::lint::hash(source),
        region: bounds,
        counts,
        items,
        omitted,
        limitations: board.unsupported.keys().cloned().collect(),
    })
}
impl Inventory {
    pub fn table(&self) -> String {
        let mut lines = vec![format!(
            "Region [{:.3},{:.3}]–[{:.3},{:.3}] mm | {:?}",
            self.region.x0, self.region.y0, self.region.x1, self.region.y1, self.counts
        )];
        for i in &self.items {
            let kind = i["kind"].as_str().unwrap_or_default();
            let name = i["reference"]
                .as_str()
                .or_else(|| i["id"].as_str())
                .unwrap_or("?");
            let net = i["net"].as_str().unwrap_or("");
            let location = if i.get("a").is_some() {
                format!("{} → {}", i["a"], i["b"])
            } else {
                format!("{}", i["at"])
            };
            let layer = i.get("layers").unwrap_or(&i["layer"]);
            let details = match kind {
                "footprint" => format!(
                    " value={} rot={} side={} courtyard={}",
                    i["value"], i["rotation"], i["side"], i["courtyard_bbox"]
                ),
                "pad" | "via" => format!(" size={} drill={}", i["size"], i["drill"]),
                "track" | "arc" => format!(" width={}", i["width"]),
                _ => String::new(),
            };
            lines.push(format!(
                "{kind:<9} {name} {} net={net} layers={layer} {location}{details}",
                i["number"].as_str().unwrap_or_default()
            ));
        }
        if self.omitted > 0 {
            lines.push(format!(
                "{} more items; increase --limit (0 means all)",
                self.omitted
            ));
        }
        if !self.limitations.is_empty() {
            lines.push(format!("Limitations: {}", self.limitations.join(", ")));
        }
        lines.join("\n")
    }
}
