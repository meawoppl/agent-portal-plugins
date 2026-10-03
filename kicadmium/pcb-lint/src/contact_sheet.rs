//! Self-contained, paged HTML contact sheets. Geometry is the linter's board-space
//! approximation, never a claim to be a native KiCad render.
use crate::{
    Report,
    board_file::{self, BoardLintFile, ExceptionAudit, ExceptionStatus},
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
struct Frame {
    crop: [f64; 4],
    objects: Vec<usize>,
    highlighted: Vec<usize>,
    labels: Vec<(Point, String)>,
    layers: Vec<String>,
    spatial: bool,
}
#[derive(Serialize)]
struct Card {
    finding: crate::Finding,
    #[serde(flatten)]
    frame: Frame,
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

/// Parsed board geometry as reusable SVG symbols plus crop framing.
struct Scene {
    b: Board,
    shapes: Vec<Shape>,
    whole: Rect,
    incomplete: Vec<String>,
}
impl Scene {
    fn read(source: &str, config_outline: &[Point]) -> Result<Self> {
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
        let outlines = if config_outline.len() >= 3 {
            vec![config_outline.to_vec()]
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
        Ok(Self {
            b,
            shapes,
            whole,
            incomplete: extra.incomplete,
        })
    }
    /// Crop around the shapes of `subjects`; `anchor` frames a stored location
    /// when no subject geometry is present.
    fn frame(&self, subjects: &[String], anchor: Option<Point>) -> Frame {
        let subject_set: BTreeSet<_> = subjects.iter().collect();
        let highlighted: Vec<_> = self
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| s.subjects.iter().any(|id| subject_set.contains(id)))
            .map(|(i, _)| i)
            .collect();
        let mut focus = highlighted
            .iter()
            .map(|&i| self.shapes[i].bounds.clone())
            .reduce(|a, b| union(&a, &b));
        // Empty footprints still have an honest positional anchor.
        for p in &self.b.parts {
            if subject_set.contains(&p.id) {
                focus =
                    Some(focus.map_or_else(|| around(p.at, 1.), |r| union(&r, &around(p.at, 1.))));
            }
        }
        if focus.is_none() {
            focus = anchor.map(|p| around(p, 1.5));
        }
        let spatial = focus.is_some();
        let crop = viewport(focus.as_ref().unwrap_or(&self.whole));
        let mut layers = BTreeSet::new();
        for &i in &highlighted {
            layers.extend(self.shapes[i].layers.iter().cloned());
        }
        let objects = self
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| s.class != "text-box" && copper::overlaps(&crop, &s.bounds))
            .map(|(i, _)| i)
            .collect();
        let labels = self
            .b
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
        Frame {
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
        }
    }
    /// Hidden SVG `<defs>` with one symbol per shape, plus JSON shape metadata.
    fn definitions(&self) -> Result<(String, String)> {
        let mut definitions = String::from(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"0\" height=\"0\" aria-hidden=\"true\" style=\"position:absolute\"><defs>",
        );
        let mut metadata = vec![];
        for (i, s) in self.shapes.iter().enumerate() {
            write!(
                definitions,
                "<g id=\"obj-{i}\" fill=\"currentColor\">{}</g>",
                s.svg
            )?;
            metadata.push((&s.class, &s.layers, &s.bounds));
        }
        definitions.push_str("</defs></svg>");
        Ok((definitions, script_json(&metadata)?))
    }
    fn use_shape(&self, i: usize, highlight: bool) -> String {
        let s = &self.shapes[i];
        let color = if highlight {
            "#e0af68"
        } else if s.class == "edge" {
            "#c0caf5"
        } else if s.class == "hole" {
            "#565f89"
        } else if s.layers.len() > 1 {
            "#7dcfff"
        } else if s.layers.iter().any(|l| l == "F.Cu") {
            "#f7768e"
        } else if s.layers.iter().any(|l| l == "B.Cu") {
            "#7aa2f7"
        } else {
            "#bb9af7"
        };
        let extra = if highlight {
            " highlight"
        } else if s.class == "zone" || s.class == "edge" {
            ""
        } else {
            " dim"
        };
        format!(
            "<use href=\"#obj-{i}\" class=\"{}{extra}\" style=\"color:{color}\"/>",
            s.class
        )
    }
    /// Static (server-side) equivalent of the findings sheet's crop renderer.
    fn crop_svg(&self, f: &Frame, label: &str) -> String {
        let [x, y, w, h] = f.crop;
        let font = w / 40.;
        let mut s = format!(
            "<svg class=\"drawing\" xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{x} {y} {w} {h}\" role=\"img\" aria-label=\"{}\"><rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" fill=\"#1a1b26\"/>",
            escape(label)
        );
        for &i in &f.objects {
            if self.shapes[i].class != "zone" {
                s.push_str(&self.use_shape(i, false));
            }
        }
        for &i in &f.highlighted {
            s.push_str(&self.use_shape(i, true));
        }
        for (p, name) in &f.labels {
            let _ = write!(
                s,
                "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-size=\"{font}\" style=\"stroke-width:{}\">{}</text>",
                p.x,
                p.y - font * 0.6,
                font * 0.15,
                escape(name)
            );
        }
        let scale: f64 = format!("{:.1e}", w / 5.).parse().unwrap_or(1.);
        let sy = y + h - font * 2.;
        let _ = write!(
            s,
            "<path d=\"M{},{sy}h{scale}\" stroke=\"#c0caf5\" stroke-width=\"{}\"/><text x=\"{}\" y=\"{}\" font-size=\"{font}\" style=\"stroke-width:{}\">{scale} mm</text></svg>",
            x + font * 2.,
            font * 0.22,
            x + font * 2.,
            sy - font * 0.7,
            font * 0.15
        );
        s
    }
}

/// JSON in a script element must never contain a literal closing script tag.
fn script_json<T: Serialize>(v: &T) -> Result<String> {
    Ok(serde_json::to_string(v)?
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026"))
}

/// Substitute `%%TOKEN%%`s in one pass: user-controlled strings are never templates.
fn fill(template: &str, tokens: &[(&str, &str)]) -> Result<String> {
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

const STYLE: &str = include_str!("contact_sheet.css");

/// Produces a single offline HTML file. Findings include ignored entries so the
/// reviewer can opt into viewing them; initially only actionable entries show.
pub fn render(source: &str, report: &Report) -> Result<String> {
    if hash(source) != report.source_sha256 {
        bail!("contact sheet source does not match report");
    }
    let scene = Scene::read(source, &report.config.intent.route.outline)?;
    // Stable order for browsing; original report order and finding keys unchanged.
    let mut findings = report.findings.clone();
    findings.sort_by(|a, b| {
        (severity_rank(&a.severity), &a.rule, &a.key).cmp(&(
            severity_rank(&b.severity),
            &b.rule,
            &b.key,
        ))
    });
    let cards = findings
        .into_iter()
        .map(|f| Card {
            frame: scene.frame(&f.subjects, None),
            finding: f,
        })
        .collect();
    let (definitions, objects) = scene.definitions()?;
    let mut limitations = report.limitations.clone();
    limitations.push(GEOMETRY_NOTE.into());
    limitations.extend(scene.incomplete.iter().cloned());
    let data = Data {
        board: &report.board_id,
        source_sha256: &report.source_sha256,
        findings: cards,
        coverage: &report.coverage,
        limitations,
        layers: &scene.b.copper_layers,
    };
    let json = script_json(&data)?;
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
    fill(
        include_str!("contact_sheet.html"),
        &[
            ("STYLE", STYLE),
            ("TITLE", &title),
            ("HEADER", &header),
            ("DEFS", &definitions),
            ("DATA", &json),
            ("OBJECTS", &objects),
        ],
    )
}

const GEOMETRY_NOTE: &str = "Images show the linter's parsed copper in top-view board coordinates, not a native KiCad plot. Back copper is not mirrored. Component labels are synthetic; text findings show measured boxes. Missing/custom geometry may be absent or approximated. No schematic imagery is implied by a board overview.";

fn severity_rank(s: &str) -> u8 {
    match s {
        "error" => 0,
        "warning" => 1,
        _ => 2,
    }
}

fn status_rank(s: ExceptionStatus) -> u8 {
    match s {
        ExceptionStatus::Orphaned => 0,
        ExceptionStatus::Changed => 1,
        ExceptionStatus::Expired => 2,
        ExceptionStatus::Resolved => 3,
        ExceptionStatus::Unverified => 4,
        ExceptionStatus::Applied => 5,
    }
}

/// UTC calendar date (YYYY-MM-DD) for Unix seconds.
pub fn date(secs: u64) -> String {
    // Howard Hinnant's civil_from_days.
    let z = (secs / 86400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

pub const ORPHANED_NOTE: &str = "geometry no longer on board — run kct lint prune";

/// Self-contained HTML contact sheet of a board's exceptions: one card per
/// exception with a highlighted crop (when its geometry exists), the review
/// record and an audit status badge. Orphaned cards list stored subject ids.
pub fn render_exceptions(
    source: &str,
    report: &Report,
    file: &BoardLintFile,
    audit: &[ExceptionAudit],
) -> Result<String> {
    if hash(source) != report.source_sha256 {
        bail!("contact sheet source does not match report");
    }
    let scene = Scene::read(source, &report.config.intent.route.outline)?;
    let status_of = |key: &str| {
        audit
            .iter()
            .find(|a| a.key == key)
            .map(|a| (a.status, a.detail.as_str()))
            .unwrap_or((ExceptionStatus::Unverified, "not audited in this run"))
    };
    let mut entries: Vec<_> = file
        .exceptions
        .iter()
        .map(|e| (e, status_of(&e.key)))
        .collect();
    entries.sort_by(|(a, (sa, _)), (b, (sb, _))| {
        (status_rank(*sa), &a.rule, &a.key).cmp(&(status_rank(*sb), &b.rule, &b.key))
    });
    let mut counts: BTreeMap<ExceptionStatus, usize> = BTreeMap::new();
    let mut cards = String::new();
    for (e, (status, detail)) in &entries {
        *counts.entry(*status).or_default() += 1;
        let finding = report.findings.iter().find(|f| f.key == e.key);
        let rule = finding.map_or(e.rule.as_str(), |f| f.rule.as_str());
        let rule = if rule.is_empty() {
            "(unknown rule)"
        } else {
            rule
        };
        let severity = finding.map_or(e.severity.as_str(), |f| f.severity.as_str());
        let message = finding
            .map(|f| f.message.as_str())
            .filter(|m| !m.is_empty())
            .or(Some(e.message.as_str()).filter(|m| !m.is_empty()))
            .or_else(|| board_file::rule_action(rule))
            .unwrap_or("");
        let at = finding.map(|f| f.at).or(e.at);
        let nets = finding.map_or(&e.nets, |f| &f.nets);
        let status_name = status.as_str();
        let _ = write!(
            cards,
            "<article data-status=\"{status_name}\" data-key=\"{}\"><div class=\"card-head\"><span class=\"badge status status-{status_name}\">{status_name}</span><strong>{}</strong><span class=\"badge {}\">{} · {}</span></div>",
            escape(&e.key),
            escape(rule),
            escape(severity),
            escape(e.action.as_str()),
            escape(if severity.is_empty() { "?" } else { severity }),
        );
        if *status == ExceptionStatus::Orphaned {
            let _ = write!(
                cards,
                "<div class=\"gone\"><p><strong>{}</strong></p><p class=\"small\">Stored subjects:</p><ul>",
                escape(ORPHANED_NOTE)
            );
            for s in &e.subjects {
                let _ = write!(cards, "<li><code>{}</code></li>", escape(s));
            }
            if e.subjects.is_empty() {
                cards.push_str("<li class=\"muted\">none recorded</li>");
            }
            cards.push_str("</ul></div>");
        } else {
            let subjects = finding.map_or(&e.subjects, |f| &f.subjects);
            let frame = scene.frame(subjects, at);
            if frame.spatial {
                let _ = write!(
                    cards,
                    "<figure>{}<figcaption>{} · {}</figcaption></figure>",
                    scene.crop_svg(&frame, &format!("{rule}; highlighted exception subjects")),
                    if frame.highlighted.is_empty() {
                        "stored location · no subject geometry"
                    } else {
                        "yellow = subjects"
                    },
                    escape(&e.key[..e.key.len().min(12)])
                );
            } else {
                cards.push_str(
                    "<div class=\"gone\"><p class=\"muted\">No board location for this exception.</p></div>",
                );
            }
        }
        let _ = write!(
            cards,
            "<div class=\"body\"><p>{}</p><p class=\"note\">{}</p><dl><dt>Reviewer</dt><dd>{}</dd><dt>Reviewed</dt><dd>{}</dd><dt>Expires</dt><dd>{}</dd><dt>Status</dt><dd>{}</dd>",
            escape(message),
            escape(&e.reason),
            escape(&e.reviewer),
            date(e.reviewed_at),
            e.expires_at.map_or_else(|| "never".into(), date),
            escape(&if detail.is_empty() {
                status_name.to_owned()
            } else {
                format!("{status_name}: {detail}")
            }),
        );
        if let Some(p) = at {
            let _ = write!(
                cards,
                "<dt>Location</dt><dd>({:.3}, {:.3}) mm</dd>",
                p.x, p.y
            );
        }
        if !nets.is_empty() {
            let _ = write!(cards, "<dt>Nets</dt><dd>{}</dd>", escape(&nets.join(", ")));
        }
        let _ = write!(
            cards,
            "<dt>Key</dt><dd><code>{}</code></dd></dl></div></article>",
            escape(&e.key)
        );
    }
    if entries.is_empty() {
        cards.push_str("<p id=\"empty\">No exceptions recorded for this board.</p>");
    }
    let title = escape(&file.board_id);
    let mut summary = format!("{} exceptions", entries.len());
    let mut options = String::new();
    for s in ExceptionStatus::ALL {
        let n = counts.get(&s).copied().unwrap_or(0);
        let _ = write!(
            summary,
            " · <span class=\"status-{0}\">{n} {0}</span>",
            s.as_str()
        );
        let _ = write!(
            options,
            "<option value=\"{0}\">{0} ({n})</option>",
            s.as_str()
        );
    }
    let stale: usize = counts
        .iter()
        .filter(|(s, _)| s.is_stale())
        .map(|(_, n)| n)
        .sum();
    let header = format!(
        "<h1>{title} · exceptions</h1><p>{summary}</p>{}<p class=\"muted\">Source SHA-256 <code>{}</code> · lint file schema {}</p>",
        if stale > 0 {
            format!(
                "<p class=\"error\">{stale} stale exception{} (orphaned, resolved or expired) — run <code>kct lint prune</code></p>",
                if stale == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        },
        escape(&report.source_sha256),
        file.schema
    );
    let (definitions, _) = scene.definitions()?;
    fill(
        include_str!("exceptions.html"),
        &[
            ("STYLE", STYLE),
            ("TITLE", &title),
            ("HEADER", &header),
            ("OPTIONS", &options),
            ("DEFS", &definitions),
            ("CARDS", &cards),
            ("NOTE", &escape(GEOMETRY_NOTE)),
        ],
    )
}
