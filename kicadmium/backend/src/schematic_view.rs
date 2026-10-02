//! Backend-normalized schematic geometry using pastebom's canonical scene.

use crate::{
    current_source_revision, pick_project_file, rel, selected_project, AppError, AppState,
    ProjectContext, ProjectQuery,
};
use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use kct::schema::{library::SymbolGraphic, schematic::Schematic};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};
use vector_view::{
    BBox, Group, GroupKind, Item, Layer, LayerKind, Net, Prim, Prop, Role, Scene, SceneKind, Side,
};

const CONNECTIVITY: u16 = 0;
const SYMBOLS: u16 = 1;
const TEXT: u16 = 2;
const SHEETS: u16 = 3;

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/kicad/schematic", get(endpoint))
}
async fn endpoint(
    State(state): State<AppState>,
    Query(q): Query<ProjectQuery>,
) -> Result<Json<Scene>, AppError> {
    let p = selected_project(&state, q.project.as_deref())?;
    Ok(Json(
        tokio::task::spawn_blocking(move || build_scene(&p)).await??,
    ))
}

fn build_scene(project: &ProjectContext) -> Result<Scene> {
    let root = pick_project_file(project, "kicad_sch")?
        .ok_or_else(|| anyhow!("project {} has no .kicad_sch", project.id))?;
    for _ in 0..4 {
        let rev = current_source_revision(project)?;
        let mut b = Builder::new();
        collect_page(project, &root, "/", None, &mut HashSet::new(), &mut b)?;
        if current_source_revision(project)? == rev {
            b.scene.meta.extend([
                Prop::new("revision", rev),
                Prop::new("root", rel(&project.root, &root)?),
            ]);
            b.finish();
            return Ok(b.scene);
        }
    }
    Err(anyhow!("schematic kept changing while it was read"))
}

struct Builder {
    scene: Scene,
    item: u32,
    group: u32,
    nets: HashMap<String, u32>,
}
impl Builder {
    fn new() -> Self {
        let mut scene = Scene::new(SceneKind::Schematic, true);
        scene.layers = vec![
            layer(
                CONNECTIVITY,
                "Connectivity",
                LayerKind::Connectivity,
                10,
                [80, 180, 90, 255],
            ),
            layer(
                SYMBOLS,
                "Symbols",
                LayerKind::Symbol,
                20,
                [205, 170, 80, 255],
            ),
            layer(TEXT, "Text", LayerKind::Text, 30, [190, 200, 220, 255]),
            layer(SHEETS, "Sheets", LayerKind::Sheet, 5, [120, 130, 190, 255]),
        ];
        Self {
            scene,
            item: 0,
            group: 0,
            nets: HashMap::new(),
        }
    }
    fn net(&mut self, n: Option<&str>) -> Option<u32> {
        let n = n.filter(|x| !x.is_empty())?;
        if let Some(id) = self.nets.get(n) {
            return Some(*id);
        }
        let id = self.nets.len() as u32 + 1;
        self.nets.insert(n.into(), id);
        self.scene.nets.push(Net { id, name: n.into() });
        Some(id)
    }
    fn group(&mut self, kind: GroupKind, label: String, props: Vec<Prop>) -> u32 {
        self.group += 1;
        self.scene.groups.push(Group {
            id: self.group,
            kind,
            label,
            props,
        });
        self.group
    }
    fn add(
        &mut self,
        layer: u16,
        role: Role,
        prim: Prim,
        net: Option<&str>,
        group: Option<u32>,
        props: Vec<Prop>,
    ) {
        self.item += 1;
        let net = self.net(net);
        self.scene.items.push(Item {
            id: self.item,
            layer,
            role,
            prim,
            net,
            group,
            props,
        })
    }
    fn finish(&mut self) {
        self.scene.recompute_bbox();
        if self.scene.bbox.is_empty() {
            self.scene.bbox = BBox::default()
        }
    }
}

fn collect_page(
    project: &ProjectContext,
    file: &Path,
    page: &str,
    parent: Option<&str>,
    seen: &mut HashSet<PathBuf>,
    b: &mut Builder,
) -> Result<()> {
    let file = file
        .canonicalize()
        .with_context(|| format!("reading {}", file.display()))?;
    if !file.starts_with(&project.root) || !seen.insert(file.clone()) {
        return Ok(());
    }
    let s = Schematic::load(&file)?;
    let nets = connectivity_names(&s);
    b.scene.meta.push(Prop::new(
        format!("page:{page}:file"),
        rel(&project.root, &file)?,
    ));
    if let Some(p) = parent {
        b.scene
            .meta
            .push(Prop::new(format!("page:{page}:parent"), p))
    }
    if let Some(p) = s.paper() {
        b.scene
            .meta
            .push(Prop::new(format!("page:{page}:paper"), p))
    }
    for w in s.wires() {
        let n = nets.get(&key(w.start)).map(String::as_str);
        b.add(
            CONNECTIVITY,
            Role::Wire,
            Prim::Polyline {
                points: vec![pt(w.start), pt(w.end)],
                width: width(w.stroke_width),
            },
            n,
            None,
            identity(page, &w.uuid),
        )
    }
    for j in s.junctions() {
        let n = nets.get(&key(j.position)).map(String::as_str);
        b.add(
            CONNECTIVITY,
            Role::Junction,
            Prim::Circle {
                center: pt(j.position),
                radius: if j.diameter > 0.0 {
                    j.diameter / 2.0
                } else {
                    0.45
                },
                fill: true,
                stroke: 0.0,
            },
            n,
            None,
            identity(page, &j.uuid),
        )
    }
    for n in s.no_connects() {
        let p = pt(n.position);
        b.add(
            CONNECTIVITY,
            Role::NoConnect,
            Prim::Polyline {
                points: vec![
                    [p[0] - 0.65, p[1] - 0.65],
                    [p[0] + 0.65, p[1] + 0.65],
                    [p[0], p[1]],
                    [p[0] - 0.65, p[1] + 0.65],
                    [p[0] + 0.65, p[1] - 0.65],
                ],
                width: 0.25,
            },
            None,
            None,
            identity(page, &n.uuid),
        )
    }
    for l in s.labels() {
        add_text(
            b,
            page,
            &l.uuid,
            &l.text,
            l.position,
            l.rotation,
            Some(&l.text),
            Role::Label,
            None,
        )
    }
    for l in s.hierarchical_labels() {
        add_text(
            b,
            page,
            &l.uuid,
            &l.text,
            l.position,
            l.rotation,
            Some(&l.text),
            Role::Label,
            None,
        )
    }
    for l in s.global_labels() {
        add_text(
            b,
            page,
            &l.uuid,
            &l.text,
            l.position,
            l.rotation,
            Some(&l.text),
            Role::Label,
            None,
        )
    }
    for inst in s.symbols() {
        let Some(lib) = s.get_lib_symbol_resolved(&inst.lib_id)? else {
            continue;
        };
        let gid = b.group(
            GroupKind::Symbol,
            inst.reference().into(),
            vec![
                Prop::new("page", page),
                Prop::new("uuid", &inst.uuid),
                Prop::new("lib_id", &inst.lib_id),
                Prop::new("value", inst.value()),
                Prop::new("unit", inst.unit.to_string()),
            ],
        );
        for (i, g) in lib.graphics.iter().enumerate() {
            let props = identity(page, &format!("{}:{i}", inst.uuid));
            match g {
                SymbolGraphic::Polyline(x) => b.add(
                    SYMBOLS,
                    Role::SymbolBody,
                    Prim::Polyline {
                        points: x
                            .points
                            .iter()
                            .map(|&p| transform(p, inst.position, inst.rotation, &inst.mirror))
                            .collect(),
                        width: width(x.stroke_width),
                    },
                    None,
                    Some(gid),
                    props,
                ),
                SymbolGraphic::Circle(x) => b.add(
                    SYMBOLS,
                    Role::SymbolBody,
                    Prim::Circle {
                        center: transform(x.center, inst.position, inst.rotation, &inst.mirror),
                        radius: x.radius,
                        fill: x.fill_type != "none",
                        stroke: width(x.stroke_width),
                    },
                    None,
                    Some(gid),
                    props,
                ),
                SymbolGraphic::Arc(x) => b.add(
                    SYMBOLS,
                    Role::SymbolBody,
                    arc_prim(
                        transform(x.start, inst.position, inst.rotation, &inst.mirror),
                        transform(x.mid, inst.position, inst.rotation, &inst.mirror),
                        transform(x.end, inst.position, inst.rotation, &inst.mirror),
                        width(x.stroke_width),
                    ),
                    None,
                    Some(gid),
                    props,
                ),
                SymbolGraphic::Rectangle(x) => {
                    let a = transform(x.start, inst.position, inst.rotation, &inst.mirror);
                    let c = transform(x.end, inst.position, inst.rotation, &inst.mirror);
                    b.add(
                        SYMBOLS,
                        Role::SymbolBody,
                        Prim::Polygon {
                            outer: vec![a, [c[0], a[1]], c, [a[0], c[1]]],
                            holes: vec![],
                            fill: x.fill_type != "none",
                            stroke: width(x.stroke_width),
                        },
                        None,
                        Some(gid),
                        props,
                    )
                }
            }
        }
        for pin in lib
            .pins
            .iter()
            .filter(|p| p.unit == inst.unit || p.unit == 0)
        {
            let at = transform(pin.position, inst.position, inst.rotation, &inst.mirror);
            let a = (pin.rotation + inst.rotation).to_radians();
            let end = [at[0] + pin.length * a.cos(), at[1] - pin.length * a.sin()];
            let n = nets.get(&key((at[0], at[1]))).map(String::as_str);
            let mut props = identity(page, &format!("{}:{}", inst.uuid, pin.number));
            props.extend([
                Prop::new("number", &pin.number),
                Prop::new("name", &pin.name),
                Prop::new("electrical_type", &pin.pin_type),
                Prop::new("shape", &pin.shape),
            ]);
            b.add(
                SYMBOLS,
                Role::Pin,
                Prim::Polyline {
                    points: vec![at, end],
                    width: 0.2,
                },
                n,
                Some(gid),
                props,
            )
        }
        for p in inst.properties.values().filter(|p| p.visible) {
            add_text(
                b,
                page,
                &format!("{}:{}", inst.uuid, p.name),
                &p.value,
                p.position,
                p.rotation,
                None,
                Role::Field,
                Some(gid),
            )
        }
    }
    let sheets: Vec<_> = s
        .sheets()
        .iter()
        .map(|x| {
            let child = format!(
                "{}/{}",
                page.trim_end_matches('/'),
                segment(if x.name.is_empty() { &x.uuid } else { &x.name })
            );
            (x.clone(), child)
        })
        .collect();
    for (x, child) in &sheets {
        let gid = b.group(
            GroupKind::Sheet,
            if x.name.is_empty() {
                x.filename.clone()
            } else {
                x.name.clone()
            },
            vec![
                Prop::new("page", page),
                Prop::new("uuid", &x.uuid),
                Prop::new("filename", &x.filename),
                Prop::new("child_page", child),
            ],
        );
        let a = pt(x.position);
        let c = [a[0] + x.size.0, a[1] + x.size.1];
        b.add(
            SHEETS,
            Role::SheetFrame,
            Prim::Polygon {
                outer: vec![a, [c[0], a[1]], c, [a[0], c[1]]],
                holes: vec![],
                fill: false,
                stroke: 0.3,
            },
            None,
            Some(gid),
            vec![Prop::new("page", page)],
        )
    }
    for (x, child) in sheets {
        let target = file.parent().unwrap_or(Path::new(".")).join(x.filename);
        if target.exists() {
            collect_page(project, &target, &child, Some(page), seen, b)?
        }
    }
    Ok(())
}

fn connectivity_names(s: &Schematic) -> HashMap<String, String> {
    let mut n = HashMap::new();
    for l in s.labels() {
        n.insert(key(l.position), l.text.clone());
    }
    for l in s.hierarchical_labels() {
        n.insert(key(l.position), l.text.clone());
    }
    for l in s.global_labels() {
        n.insert(key(l.position), l.text.clone());
    }
    for _ in 0..=s.wires().len() {
        let mut changed = false;
        for w in s.wires() {
            let a = key(w.start);
            let z = key(w.end);
            match (n.get(&a).cloned(), n.get(&z).cloned()) {
                (Some(x), None) => {
                    n.insert(z, x);
                    changed = true
                }
                (None, Some(x)) => {
                    n.insert(a, x);
                    changed = true
                }
                _ => {}
            }
        }
        if !changed {
            break;
        }
    }
    n
}
#[allow(clippy::too_many_arguments)]
fn add_text(
    b: &mut Builder,
    page: &str,
    id: &str,
    text: &str,
    pos: (f64, f64),
    rotation: f64,
    net: Option<&str>,
    role: Role,
    group: Option<u32>,
) {
    let strokes = kicad_strokes::to_strokes(&kicad_strokes::TextSpec {
        text: text.to_owned(),
        pos: [pos.0, pos.1],
        angle_deg: rotation,
        // The current schematic schema deliberately normalizes only the
        // common placement fields. Preserve KiCad's standard schematic text
        // metrics here until per-item effects are exposed by that schema.
        size: [1.27, 1.27],
        thickness: kicad_strokes::SCH_DEFAULT_PEN,
        keep_upright: true,
        ..kicad_strokes::TextSpec::default()
    });
    let mut props = identity(page, id);
    props.extend([
        Prop::new("text", text),
        Prop::new("at", format!("{},{}", pos.0, pos.1)),
        Prop::new("rotation", rotation.to_string()),
    ]);
    b.add(TEXT, role, strokes.into_prim(), net, group, props)
}
fn identity(page: &str, id: &str) -> Vec<Prop> {
    vec![Prop::new("page", page), Prop::new("uuid", id)]
}
fn arc_prim(s: [f64; 2], m: [f64; 2], e: [f64; 2], w: f64) -> Prim {
    let d = 2.0 * (s[0] * (m[1] - e[1]) + m[0] * (e[1] - s[1]) + e[0] * (s[1] - m[1]));
    if d.abs() < 1e-9 {
        return Prim::Polyline {
            points: vec![s, m, e],
            width: w,
        };
    }
    let ss = s[0] * s[0] + s[1] * s[1];
    let mm = m[0] * m[0] + m[1] * m[1];
    let ee = e[0] * e[0] + e[1] * e[1];
    let c = [
        (ss * (m[1] - e[1]) + mm * (e[1] - s[1]) + ee * (s[1] - m[1])) / d,
        (ss * (e[0] - m[0]) + mm * (s[0] - e[0]) + ee * (m[0] - s[0])) / d,
    ];
    Prim::Arc {
        center: c,
        radius: ((s[0] - c[0]).powi(2) + (s[1] - c[1]).powi(2)).sqrt(),
        start: (s[1] - c[1]).atan2(s[0] - c[0]),
        end: (e[1] - c[1]).atan2(e[0] - c[0]),
        width: w,
    }
}
fn transform((mut x, mut y): (f64, f64), o: (f64, f64), rot: f64, mirror: &str) -> [f64; 2] {
    if mirror == "x" {
        x = -x
    } else if mirror == "y" {
        y = -y
    }
    let (s, c) = rot.to_radians().sin_cos();
    [r(o.0 + x * c - y * s), r(o.1 - (x * s + y * c))]
}
fn layer(id: u16, name: &str, kind: LayerKind, z: i32, color: [u8; 4]) -> Layer {
    Layer {
        id,
        name: name.into(),
        kind,
        side: Side::None,
        z,
        color,
        visible: true,
    }
}
fn width(w: f64) -> f64 {
    if w > 0.0 {
        w
    } else {
        0.254
    }
}
fn pt(p: (f64, f64)) -> [f64; 2] {
    [r(p.0), r(p.1)]
}
fn r(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}
fn key(p: (f64, f64)) -> String {
    format!("{:.4},{:.4}", r(p.0), r(p.1))
}
fn segment(s: &str) -> String {
    let x: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if x.is_empty() {
        "sheet".into()
    } else {
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transform_is_y_down() {
        assert_eq!(transform((2., 0.), (10., 20.), 90., ""), [10., 18.])
    }
    #[test]
    fn text_has_narrow_stroke_adapter() {
        let mut b = Builder::new();
        add_text(
            &mut b,
            "/",
            "u",
            "R1",
            (2., 3.),
            0.,
            None,
            Role::Label,
            None,
        );
        assert!(matches!(b.scene.items[0].prim, Prim::Strokes { .. }));
        assert!(b.scene.items[0]
            .props
            .iter()
            .any(|p| p.key == "text" && p.value == "R1"))
    }
    #[test]
    fn arc_reifies() {
        assert!(matches!(
            arc_prim([1., 0.], [0., 1.], [-1., 0.], 0.2),
            Prim::Arc { .. }
        ))
    }
}
