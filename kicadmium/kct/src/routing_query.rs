//! Conservative routing probes against every traversed copper layer.
use crate::lint::{
    copper::{self, Geometry},
    intent::RoutePolicy,
    model::{Board, Point},
};
use anyhow::{bail, Context, Result};
use geo::{BoundingRect, Contains, Intersects};
use serde_json::{json, Value};
use std::path::Path;
pub struct Probe {
    board: Board,
    geometry: Geometry,
    policy: RoutePolicy,
    min_width: f64,
    min_size: f64,
    min_drill: f64,
    min_ring: f64,
    hole_gap: f64,
    copper_index: crate::geometry::strtree::StrTree,
}
fn length(s: &str) -> Result<f64> {
    let mut scale = 1.;
    let mut value = s;
    for (suffix, mul) in [("mm", 1.), ("mil", 0.0254), ("um", 0.001), ("in", 25.4)] {
        if let Some(v) = s.strip_suffix(suffix) {
            value = v;
            scale = mul;
            break;
        }
    }
    let n = value.parse::<f64>()? * scale;
    if !n.is_finite() || n < 0. {
        bail!("invalid rule length")
    };
    Ok(n)
}
impl Probe {
    pub fn load(path: &Path) -> Result<Self> {
        let source = std::fs::read_to_string(path)?;
        let board = Board::read(&source)?;
        let extra = copper::Extra::read(&source, &board)?;
        let mut policy = RoutePolicy::default();
        let pcb = crate::schema::pcb::Pcb::from_sexp(crate::sexp::parse(&source)?)?;
        for class in pcb.net_classes() {
            policy.clearance_mm = policy.clearance_mm.max(class.clearance.unwrap_or(0.));
        }
        let mut min_width: f64 = 0.1;
        let mut min_size: f64 = 0.3;
        let mut min_drill: f64 = 0.15;
        let mut min_ring: f64 = 0.075;
        let mut hole_gap: f64 = 0.2;
        let pro = path.with_extension("kicad_pro");
        if pro.exists() {
            let p: Value = serde_json::from_slice(&std::fs::read(pro)?)?;
            let rules = &p["board"]["design_settings"]["rules"];
            let get = |k: &str| rules[k].as_f64().unwrap_or(0.);
            policy.clearance_mm = policy.clearance_mm.max(get("min_clearance"));
            policy.edge_clearance_mm = policy
                .edge_clearance_mm
                .max(get("min_copper_edge_clearance"));
            min_width = min_width.max(get("min_track_width"));
            min_size = min_size.max(get("min_via_diameter"));
            min_drill = min_drill.max(get("min_through_hole_diameter"));
            min_ring = min_ring.max(get("min_via_annular_width"));
            hole_gap = hole_gap.max(get("min_hole_to_hole"));
            // Taking the strongest class is conservative even with regex/netclass assignments.
            for class in p["net_settings"]["classes"]
                .as_array()
                .into_iter()
                .flatten()
            {
                policy.clearance_mm = policy
                    .clearance_mm
                    .max(class["clearance"].as_f64().unwrap_or(0.));
            }
        }
        let dru = path.with_extension("kicad_dru");
        if dru.exists() {
            let nodes = crate::sexp::parse_all(&std::fs::read_to_string(dru)?)?;
            for rule in nodes.iter().filter(|n| n.has_tag("rule")) {
                for c in rule.find_children("constraint") {
                    let kind = c.string_at(0).context("invalid custom rule constraint")?;
                    let minimum = c
                        .get("min")
                        .and_then(|n| n.text_at(0))
                        .map(|s| length(&s))
                        .transpose()?;
                    if c.get("max").is_some() {
                        bail!("custom maximum constraints require KiCad evaluation; refusing to claim legal sites")
                    }
                    match kind {"clearance"|"silk_clearance"=>policy.clearance_mm=policy.clearance_mm.max(minimum.context("custom clearance needs a minimum")?),"edge_clearance"=>policy.edge_clearance_mm=policy.edge_clearance_mm.max(minimum.context("edge clearance needs minimum")?),"track_width"=>min_width=min_width.max(minimum.context("track width needs minimum")?),"via_diameter"=>min_size=min_size.max(minimum.context("via diameter needs minimum")?),"hole_size"=>min_drill=min_drill.max(minimum.context("hole size needs minimum")?),"annular_width"=>min_ring=min_ring.max(minimum.context("annular width needs minimum")?),"hole_to_hole"=>hole_gap=hole_gap.max(minimum.context("hole spacing needs minimum")?),_=>bail!("custom constraint {kind} requires KiCad evaluation; refusing to claim legal sites")}
                }
            }
        }
        let geometry = Geometry::new(&board, extra, &policy);
        if geometry.uncertain || geometry.outline.is_none() {
            bail!("incomplete copper geometry or board outline; cannot certify routing probes")
        }
        let copper_index = crate::geometry::strtree::StrTree::new(
            &geometry
                .items
                .iter()
                .map(|i| {
                    i.poly
                        .bounding_rect()
                        .map(|b| (b.min().x, b.min().y, b.max().x, b.max().y))
                })
                .collect::<Vec<_>>(),
        );
        Ok(Self {
            board,
            geometry,
            policy,
            min_width,
            min_size,
            min_drill,
            min_ring,
            hole_gap,
            copper_index,
        })
    }
    fn valid_net(&self, net: &str) -> Result<()> {
        if net.is_empty()
            || !self.board.pads.iter().any(|p| p.net == net)
                && !self.board.tracks.iter().any(|p| p.net == net)
                && !self.board.vias.iter().any(|p| p.net == net)
        {
            bail!("unknown routed net {net}")
        }
        Ok(())
    }
    fn blockers(
        &self,
        path: &[Point],
        width: f64,
        net: &str,
        layer: &str,
        via: bool,
    ) -> Vec<String> {
        let envelope = if via {
            copper::disk(path[0], width / 2. + self.policy.clearance_mm + 0.005)
        } else {
            copper::line(path, width + 2. * (self.policy.clearance_mm + 0.005))
        };
        let mut blockers = Vec::new();
        let Some(bounds) = envelope.bounding_rect() else {
            return vec!["zero-length or unsupported path".into()];
        };
        for index in self.copper_index.query((
            bounds.min().x,
            bounds.min().y,
            bounds.max().x,
            bounds.max().y,
        )) {
            let item = &self.geometry.items[index];
            if item.net != net
                && item.layers.iter().any(|l| l == layer)
                && item.poly.intersects(&envelope)
            {
                blockers.push(item.id.clone())
            }
        }
        for k in self.geometry.extra.keepouts.iter().chain(if via {
            self.geometry.extra.via_keepouts.iter()
        } else {
            [].iter()
        }) {
            if (k.layer == layer || k.layer == "*.Cu")
                && copper::polygon(&k.polygon).intersects(&envelope)
            {
                blockers.push("keepout".into())
            }
        }
        if !self.geometry.outline.as_ref().unwrap().contains(&if via {
            copper::disk(path[0], width / 2. + self.policy.edge_clearance_mm)
        } else {
            copper::line(path, width + 2. * self.policy.edge_clearance_mm)
        }) {
            blockers.push("board edge".into())
        }
        if via {
            for text in self
                .geometry
                .extra
                .texts
                .iter()
                .filter(|t| t.layer.ends_with("SilkS"))
            {
                let b = &text.bounds;
                let polygon = copper::polygon(&[
                    b.min,
                    Point {
                        x: b.max.x,
                        y: b.min.y,
                    },
                    b.max,
                    Point {
                        x: b.min.x,
                        y: b.max.y,
                    },
                ]);
                if polygon.intersects(&envelope) {
                    blockers.push(format!("silk:{}", text.id));
                }
            }
        }
        blockers.sort();
        blockers.dedup();
        blockers
    }
    #[allow(clippy::too_many_arguments)]
    pub fn via_sites(
        &self,
        net: &str,
        near: Point,
        radius: f64,
        size: f64,
        drill: f64,
        step: f64,
        limit: usize,
    ) -> Result<Value> {
        self.valid_net(net)?;
        if ![near.x, near.y, radius, size, drill, step]
            .iter()
            .all(|n| n.is_finite())
            || radius <= 0.
            || step <= 0.
            || size < self.min_size
            || drill < self.min_drill
            || (size - drill) / 2. < self.min_ring
        {
            bail!("invalid probe dimensions or via below project minima")
        }
        let steps = (radius / step).ceil() as i32;
        if steps > 200 {
            bail!("search grid exceeds 401×401; increase --step")
        }
        let mut sites = Vec::new();
        let mut blocked = std::collections::BTreeSet::new();
        for ix in -steps..=steps {
            for iy in -steps..=steps {
                let at = Point {
                    x: near.x + f64::from(ix) * step,
                    y: near.y + f64::from(iy) * step,
                };
                let distance = (at.x - near.x).hypot(at.y - near.y);
                if distance > radius {
                    continue;
                }
                let mut blockers = Vec::new();
                for layer in &self.board.copper_layers {
                    blockers.extend(self.blockers(
                        &[
                            at,
                            Point {
                                x: at.x + 1e-9,
                                y: at.y,
                            },
                        ],
                        size,
                        net,
                        layer,
                        true,
                    ));
                }
                for via in &self.board.vias {
                    if at.distance(via.at) < (drill + via.drill) / 2. + self.hole_gap {
                        blockers.push(via.id.clone())
                    }
                }
                for pad in self.board.pads.iter().filter(|p| p.drill.is_some()) {
                    if at.distance(pad.at)
                        < (drill + pad.drill.unwrap().x.max(pad.drill.unwrap().y)) / 2.
                            + self.hole_gap
                    {
                        blockers.push(pad.id.clone())
                    }
                }
                if blockers.is_empty() {
                    sites.push(
                        json!({"at":at,"distance_mm":distance,"layers":self.board.copper_layers}),
                    );
                } else {
                    blocked.extend(blockers);
                }
            }
        }
        sites.sort_by(|a, b| {
            a["distance_mm"]
                .as_f64()
                .unwrap()
                .total_cmp(&b["distance_mm"].as_f64().unwrap())
        });
        let count = sites.len();
        sites.truncate(limit);
        Ok(
            json!({"schema":"kct.route-query/v1","sites":sites,"total_candidates":count,"blocking_objects":blocked.into_iter().take(30).collect::<Vec<_>>(),"rules":"conservative maximum of project/netclass/custom minima","clearance_mm":self.policy.clearance_mm}),
        )
    }
    pub fn corridor(
        &self,
        net: &str,
        layer: &str,
        from: Point,
        to: Point,
        width: f64,
    ) -> Result<Value> {
        self.valid_net(net)?;
        if !self.board.copper_layers.iter().any(|l| l == layer) {
            bail!("unknown copper layer")
        }
        if ![from.x, from.y, to.x, to.y, width]
            .iter()
            .all(|n| n.is_finite())
            || width < self.min_width
        {
            bail!("invalid corridor or width below project minimum")
        }
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        let d = dx.abs().min(dy.abs());
        let routes = vec![
            vec![from, to],
            vec![
                from,
                Point {
                    x: from.x + dx.signum() * d,
                    y: from.y + dy.signum() * d,
                },
                to,
            ],
            vec![
                from,
                Point {
                    x: to.x - dx.signum() * d,
                    y: to.y - dy.signum() * d,
                },
                to,
            ],
        ];
        let candidates: Vec<_> = routes
            .into_iter()
            .map(|path| {
                let blockers = self.blockers(&path, width, net, layer, false);
                json!({"path":path,"legal":blockers.is_empty(),"blocking_objects":blockers})
            })
            .collect();
        Ok(
            json!({"schema":"kct.route-query/v1","fits":candidates.iter().any(|c|c["legal"]==true),"candidates":candidates,"clearance_mm":self.policy.clearance_mm,"rules":"conservative maximum of project/netclass/custom minima"}),
        )
    }
}
