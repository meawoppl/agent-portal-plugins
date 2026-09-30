//! Self-contained, paged HTML contact sheets. Geometry is the linter's board-space
//! approximation, never a claim to be a native KiCad render.
use crate::{
    Report,
    copper::{self, Extra},
    hash,
    intent::Rect,
    model::{Board, Point},
};
use anyhow::{Result, bail};
use geo::MultiPolygon;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn union(a: &Rect, b: &Rect) -> Rect {
    Rect {
        min: Point {
            x: a.min.x.min(b.min.x),
            y: a.min.y.min(b.min.y),
        },
        max: Point {
            x: a.max.x.max(b.max.x),
            y: a.max.y.max(b.max.y),
        },
    }
}
fn around(p: Point, radius: f64) -> Rect {
    Rect {
        min: Point {
            x: p.x - radius,
            y: p.y - radius,
        },
        max: Point {
            x: p.x + radius,
            y: p.y + radius,
        },
    }
}
fn viewport(r: &Rect) -> Rect {
    let w = ((r.max.x - r.min.x) * 1.3 + 3.).max(8.);
    let h = ((r.max.y - r.min.y) * 1.3 + 3.).max(6.);
    let w = w.max(h * 4. / 3.);
    let h = h.max(w * 3. / 4.);
    let cx = (r.min.x + r.max.x) / 2.;
    let cy = (r.min.y + r.max.y) / 2.;
    Rect {
        min: Point {
            x: cx - w / 2.,
            y: cy - h / 2.,
        },
        max: Point {
            x: cx + w / 2.,
            y: cy + h / 2.,
        },
    }
}
fn path(poly: &MultiPolygon<f64>) -> String {
    let mut s = String::new();
    for p in &poly.0 {
        for ring in std::iter::once(p.exterior()).chain(p.interiors()) {
            for (i, c) in ring.0.iter().enumerate() {
                let _ = write!(s, "{}{:.5},{:.5}", if i == 0 { "M" } else { "L" }, c.x, c.y);
            }
            s.push('Z');
        }
    }
    s
}
struct Shape {
    subjects: Vec<String>,
    bounds: Rect,
    layers: Vec<String>,
    class: &'static str,
    svg: String,
}
#[derive(Serialize)]
struct Card {
    finding: crate::Finding,
    crop: [f64; 4],
    objects: Vec<usize>,
    highlighted: Vec<usize>,
    labels: Vec<(Point, String)>,
    layers: Vec<String>,
    spatial: bool,
}
#[derive(Serialize)]
struct Data<'a> {
    board: &'a str,
    source_sha256: &'a str,
    findings: Vec<Card>,
    coverage: &'a [crate::Coverage],
    limitations: Vec<String>,
    layers: &'a [String],
}

/// Produces a single offline HTML file. Findings include ignored entries so the
/// reviewer can opt into viewing them; initially only actionable entries show.
pub fn render(source: &str, report: &Report) -> Result<String> {
    if hash(source) != report.source_sha256 {
        bail!("contact sheet source does not match report");
    }
    let b = Board::read(source)?;
    let extra = Extra::read(source, &b)?;
    let mut shapes = vec![];
    for z in &extra.zones {
        for p in &z.filled {
            let poly = MultiPolygon(vec![copper::polygon(p)]);
            if let Some(bounds) = copper::bounds(&poly) {
                shapes.push(Shape {
                    subjects: vec![z.id.clone()],
                    bounds,
                    layers: vec![z.layer.clone()],
                    class: "zone",
                    svg: format!("<path d=\"{}\" fill-rule=\"evenodd\"/>", path(&poly)),
                });
            }
        }
    }
    for t in &b.tracks {
        let bounds = union(&around(t.a, t.width / 2.), &around(t.b, t.width / 2.));
        shapes.push(Shape { subjects: vec![t.id.clone()], bounds,
            layers: vec![t.layer.clone()], class:"track",
            svg:format!("<path d=\"M{},{}L{},{}\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"{}\" stroke-linecap=\"round\"/>",t.a.x,t.a.y,t.b.x,t.b.y,t.width.max(0.03)) });
    }
    for a in &extra.arcs {
        let poly = copper::line(&a.points, a.width);
        if let Some(bounds) = copper::bounds(&poly) {
            shapes.push(Shape {
                subjects: vec![a.id.clone()],
                bounds,
                layers: vec![a.layer.clone()],
                class: "track",
                svg: format!("<path d=\"{}\"/>", path(&poly)),
            });
        }
    }
    for p in &b.pads {
        let poly = copper::pad_poly(p);
        let bounds = copper::bounds(&poly).unwrap_or_else(|| around(p.at, 0.25));
        let mut subjects = vec![p.id.clone()];
        if let Some(f) = b.parts.iter().find(|f| f.reference == p.reference) {
            subjects.push(f.id.clone());
        }
        shapes.push(Shape {
            subjects,
            bounds,
            layers: b
                .copper_layers
                .iter()
                .filter(|l| crate::model::on_layer(&p.layers, l))
                .cloned()
                .collect(),
            class: if p.kind == "np_thru_hole" {
                "hole"
            } else {
                "pad"
            },
            svg: format!("<path d=\"{}\" fill-rule=\"evenodd\"/>", path(&poly)),
        });
    }
    for v in &b.vias {
        shapes.push(Shape { subjects:vec![v.id.clone()], bounds:around(v.at,v.size/2.),
            layers:b.via_layers(v),class:"via",
            svg:format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"{}\"/>",
                v.at.x,v.at.y,(v.size+v.drill)/4.,((v.size-v.drill)/2.).max(0.01)) });
    }
    let outlines = if report.config.intent.route.outline.len() >= 3 {
        vec![report.config.intent.route.outline.clone()]
    } else {
        extra.outlines.clone()
    };
    for outline in outlines {
        let poly = MultiPolygon(vec![copper::polygon(&outline)]);
        if let Some(bounds) = copper::bounds(&poly) {
            shapes.push(Shape {
                subjects: vec![],
                bounds,
                layers: vec![],
                class: "edge",
                svg: format!(
                    "<path d=\"{}\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"0.12\"/>",
                    path(&poly)
                ),
            });
        }
    }
    // Text geometry findings get their measured box, not an invented glyph render.
    for t in &extra.texts {
        shapes.push(Shape {subjects:vec![t.id.clone()],bounds:t.bounds.clone(),
            layers:vec![t.layer.clone()],class:"text-box",
            svg:format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"0.08\"/>",
                t.bounds.min.x,t.bounds.min.y,
                (t.bounds.max.x-t.bounds.min.x).max(0.05),(t.bounds.max.y-t.bounds.min.y).max(0.05))});
    }
    let whole = shapes
        .iter()
        .map(|s| s.bounds.clone())
        .reduce(|a, b| union(&a, &b))
        .unwrap_or_else(|| around(Point { x: 0., y: 0. }, 5.));
    let mut cards = vec![];
    // Stable order for browsing; original report order and finding keys unchanged.
    let mut findings = report.findings.clone();
    findings.sort_by(|a, b| {
        let rank = |s: &str| match s {
            "error" => 0,
            "warning" => 1,
            _ => 2,
        };
        (rank(&a.severity), &a.rule, &a.key).cmp(&(rank(&b.severity), &b.rule, &b.key))
    });
    for f in findings {
        let subject_set: BTreeSet<_> = f.subjects.iter().collect();
        let highlighted: Vec<_> = shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| s.subjects.iter().any(|id| subject_set.contains(id)))
            .map(|(i, _)| i)
            .collect();
        let mut focus = highlighted
            .iter()
            .map(|&i| shapes[i].bounds.clone())
            .reduce(|a, b| union(&a, &b));
        // Empty footprints still have an honest positional anchor.
        for p in &b.parts {
            if subject_set.contains(&p.id) {
                focus =
                    Some(focus.map_or_else(|| around(p.at, 1.), |r| union(&r, &around(p.at, 1.))));
            }
        }
        let spatial = focus.is_some();
        let crop = viewport(focus.as_ref().unwrap_or(&whole));
        let mut layers = BTreeSet::new();
        for &i in &highlighted {
            layers.extend(shapes[i].layers.iter().cloned());
        }
        let objects = shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| s.class != "text-box" && copper::overlaps(&crop, &s.bounds))
            .map(|(i, _)| i)
            .collect();
        let labels = b
            .parts
            .iter()
            .filter(|p| {
                p.at.x >= crop.min.x
                    && p.at.x <= crop.max.x
                    && p.at.y >= crop.min.y
                    && p.at.y <= crop.max.y
            })
            .map(|p| (p.at, p.reference.clone()))
            .collect();
        cards.push(Card {
            finding: f,
            crop: [
                crop.min.x,
                crop.min.y,
                crop.max.x - crop.min.x,
                crop.max.y - crop.min.y,
            ],
            objects,
            highlighted,
            labels,
            layers: layers.into_iter().collect(),
            spatial,
        });
    }
    let mut definitions = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"0\" height=\"0\" aria-hidden=\"true\" style=\"position:absolute\"><defs>",
    );
    let mut metadata = vec![];
    for (i, s) in shapes.iter().enumerate() {
        write!(
            definitions,
            "<g id=\"obj-{i}\" fill=\"currentColor\">{}</g>",
            s.svg
        )?;
        metadata.push((&s.class, &s.layers, &s.bounds));
    }
    definitions.push_str("</defs></svg>");
    let mut limitations = report.limitations.clone();
    limitations.push("Images show the linter's parsed copper in top-view board coordinates, not a native KiCad plot. Back copper is not mirrored. Component labels are synthetic; text findings show measured boxes. Missing/custom geometry may be absent or approximated. No schematic imagery is implied by a board overview.".into());
    limitations.extend(extra.incomplete);
    let data = Data {
        board: &report.board_id,
        source_sha256: &report.source_sha256,
        findings: cards,
        coverage: &report.coverage,
        limitations,
        layers: &b.copper_layers,
    };
    // JSON in a script element must never contain a literal closing script tag.
    let json = serde_json::to_string(&data)?
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");
    let objects = serde_json::to_string(&metadata)?
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");
    let mut counts = BTreeMap::new();
    for f in &report.findings {
        *counts.entry(&f.severity).or_insert(0usize) += 1;
    }
    let title = escape(&report.board_id);
    let header = format!(
        "<h1>{title}</h1><p>{} findings · {} errors · {} warnings · {} notes</p><p class=\"muted\">Source SHA-256 <code>{}</code></p>",
        report.findings.len(),
        counts.get(&"error".to_owned()).unwrap_or(&0),
        counts.get(&"warning".to_owned()).unwrap_or(&0),
        counts.get(&"info".to_owned()).unwrap_or(&0),
        escape(&report.source_sha256)
    );
    // Substitute tokens in one pass: user-controlled strings are never templates.
    let tokens = [
        ("TITLE", &title),
        ("HEADER", &header),
        ("DEFS", &definitions),
        ("DATA", &json),
        ("OBJECTS", &objects),
    ];
    let template = include_str!("contact_sheet.html");
    let mut out = String::new();
    for (i, part) in template.split("%%").enumerate() {
        if i % 2 == 0 {
            out.push_str(part);
        } else if let Some((_, value)) = tokens.iter().find(|(key, _)| *key == part) {
            out.push_str(value);
        } else {
            bail!("unknown HTML template token {part}");
        }
    }
    Ok(out)
}
