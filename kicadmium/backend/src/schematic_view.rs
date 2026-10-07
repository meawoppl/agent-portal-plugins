//! Backend-normalized schematic geometry using pastebom's canonical scene.

use crate::schematic_drawing::{self, Annotation, TitleInfo};
use crate::symbol_graphics::{body_prims, Fill};
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
use kct::{schema::schematic::Schematic, SExp};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};
use vector_view::{
    BBox, Group, GroupKind, Item, Layer, LayerKind, Net, Prim, Prop, Role, Scene, SceneKind, Side,
};

// Layer ids. Each schematic element class gets its own layer so the viewer
// (and any theme override) can colour it independently.
const CONNECTIVITY: u16 = 0;
const SYMBOLS: u16 = 1;
const FIELDS: u16 = 2;
const SHEETS: u16 = 3;
const BODY_FILL: u16 = 4;
const PINS: u16 = 5;
const PIN_TEXT: u16 = 6;
const NO_CONNECTS: u16 = 7;
const LABELS: u16 = 8;
const GLOBAL_LABELS: u16 = 9;
const POWER: u16 = 10;
const FRAME: u16 = 11;
const FRAME_TEXT: u16 = 12;
const GRAPHICS: u16 = 13;
const NOTES: u16 = 14;
const BUSES: u16 = 15;

/// Default schematic palette: the Agent Portal's Tokyo Night colours, drawn
/// over the #1a1b26 canvas the schematic tab sets.
pub(crate) mod palette {
    pub type Rgba = [u8; 4];
    /// Wires and junctions (green).
    pub const WIRE: Rgba = [0x9e, 0xce, 0x6a, 255];
    /// Symbol body outlines and `(fill (type outline))` shapes (blue).
    pub const BODY: Rgba = [0x7a, 0xa2, 0xf7, 255];
    /// `(fill (type background))` bodies: the body blue at ~9% (about
    /// #23283a on the canvas), a wash under everything that never hides
    /// pins or text.
    pub const BODY_FILL: Rgba = [0x7a, 0xa2, 0xf7, 24];
    /// Pin lines (orange).
    pub const PIN: Rgba = [0xe0, 0xaf, 0x68, 255];
    /// Pin names and numbers (muted foreground).
    pub const PIN_TEXT: Rgba = [0x9a, 0xa5, 0xce, 255];
    /// Reference, value and other visible fields (teal).
    pub const FIELD: Rgba = [0x7d, 0xcf, 0xff, 255];
    /// Local net labels (foreground).
    pub const LABEL: Rgba = [0xc0, 0xca, 0xf5, 255];
    /// Global and hierarchical labels (purple, like sheets).
    pub const GLOBAL_LABEL: Rgba = [0xbb, 0x9a, 0xf7, 255];
    /// Power symbols: body, pins and value (red).
    pub const POWER: Rgba = [0xf7, 0x76, 0x8e, 255];
    /// No-connect flags (blue).
    pub const NO_CONNECT: Rgba = [0x7a, 0xa2, 0xf7, 255];
    /// Hierarchical sheet frames (purple).
    pub const SHEET: Rgba = [0xbb, 0x9a, 0xf7, 255];
    /// Page border, reference grid and title block lines (muted).
    pub const FRAME: Rgba = [0x56, 0x5f, 0x89, 255];
    /// Reference-grid labels and title block text.
    pub const FRAME_TEXT: Rgba = [0x73, 0x7a, 0xa2, 255];
    /// Free graphic lines, boxes, circles and arcs drawn on the sheet.
    pub const GRAPHICS: Rgba = [0x9a, 0xa5, 0xce, 255];
    /// Free text notes and text boxes.
    pub const NOTES: Rgba = [0xa9, 0xb1, 0xd6, 255];
    /// Buses and bus entries (teal).
    pub const BUS: Rgba = [0x7d, 0xcf, 0xff, 255];
}

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
    /// Pages in traversal order, framed once the sheet count is known.
    pages: Vec<PageFrame>,
}
/// What a page's drawing sheet needs: path, paper and title block values.
struct PageFrame {
    page: String,
    size: (f64, f64),
    info: TitleInfo,
}
impl Builder {
    fn new() -> Self {
        let mut scene = Scene::new(SceneKind::Schematic, true);
        // Paint order (z): page frame, sheets, body wash, graphics, wires,
        // bodies, pins, text.
        scene.layers = vec![
            layer(
                FRAME,
                "Drawing sheet",
                LayerKind::Drawing,
                1,
                palette::FRAME,
            ),
            layer(
                FRAME_TEXT,
                "Title block",
                LayerKind::Drawing,
                2,
                palette::FRAME_TEXT,
            ),
            layer(SHEETS, "Sheets", LayerKind::Sheet, 5, palette::SHEET),
            layer(
                BODY_FILL,
                "Symbol body fill",
                LayerKind::Symbol,
                6,
                palette::BODY_FILL,
            ),
            layer(
                GRAPHICS,
                "Graphics",
                LayerKind::Drawing,
                7,
                palette::GRAPHICS,
            ),
            layer(BUSES, "Buses", LayerKind::Connectivity, 9, palette::BUS),
            layer(
                CONNECTIVITY,
                "Wires",
                LayerKind::Connectivity,
                10,
                palette::WIRE,
            ),
            layer(
                NO_CONNECTS,
                "No-connects",
                LayerKind::Connectivity,
                11,
                palette::NO_CONNECT,
            ),
            layer(SYMBOLS, "Symbols", LayerKind::Symbol, 20, palette::BODY),
            layer(PINS, "Pins", LayerKind::Symbol, 21, palette::PIN),
            layer(
                POWER,
                "Power symbols",
                LayerKind::Symbol,
                22,
                palette::POWER,
            ),
            layer(PIN_TEXT, "Pin text", LayerKind::Text, 30, palette::PIN_TEXT),
            layer(FIELDS, "Fields", LayerKind::Text, 31, palette::FIELD),
            layer(LABELS, "Labels", LayerKind::Text, 32, palette::LABEL),
            layer(
                GLOBAL_LABELS,
                "Global labels",
                LayerKind::Text,
                33,
                palette::GLOBAL_LABEL,
            ),
            layer(NOTES, "Notes", LayerKind::Text, 34, palette::NOTES),
        ];
        Self {
            scene,
            item: 0,
            group: 0,
            nets: HashMap::new(),
            pages: vec![],
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
    /// Draw every page's border, reference grid and title block.
    fn frames(&mut self) {
        let pages = std::mem::take(&mut self.pages);
        let count = pages.len();
        for (i, p) in pages.into_iter().enumerate() {
            let info = TitleInfo {
                sheet: i + 1,
                sheets: count,
                ..p.info
            };
            let f = schematic_drawing::frame(p.size.0, p.size.1, &info);
            let props = vec![Prop::new("page", &p.page)];
            self.add(
                FRAME,
                Role::Graphic,
                Prim::Strokes {
                    strokes: f.lines,
                    width: schematic_drawing::FRAME_LINE,
                },
                None,
                None,
                props.clone(),
            );
            for spec in f.texts {
                let text = spec.text.clone();
                add_text_spec(
                    self,
                    FRAME_TEXT,
                    &p.page,
                    "",
                    &text,
                    spec,
                    None,
                    Role::Text,
                    None,
                );
            }
        }
    }
    fn finish(&mut self) {
        self.frames();
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
    let (paper, w, h) = schematic_drawing::paper(s.sexp());
    b.scene.meta.extend([
        Prop::new(format!("page:{page}:paper"), &paper),
        Prop::new(format!("page:{page}:size"), format!("{w},{h}")),
    ]);
    let tb = s.title_block();
    b.pages.push(PageFrame {
        page: page.to_owned(),
        size: (w, h),
        info: TitleInfo {
            title: tb.title,
            date: tb.date,
            rev: tb.rev,
            company: tb.company,
            comments: [1, 2, 3, 4].map(|k| tb.comments.get(&k).cloned().unwrap_or_default()),
            paper,
            file: file
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default(),
            sheet_path: page.to_owned(),
            ..Default::default()
        },
    });
    add_annotations(&s, page, b);
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
            NO_CONNECTS,
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
            LABELS,
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
            GLOBAL_LABELS,
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
            GLOBAL_LABELS,
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
    add_symbols(&s, page, &nets, b)?;
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

/// Free text, graphics and buses drawn directly on a page.
fn add_annotations(s: &Schematic, page: &str, b: &mut Builder) {
    for a in schematic_drawing::annotations(s.sexp()) {
        match a {
            Annotation::Shape {
                uuid,
                outline,
                fill,
            } => {
                if let Some((kind, prim)) = fill {
                    let layer = if kind == Fill::Outline {
                        GRAPHICS
                    } else {
                        BODY_FILL
                    };
                    b.add(
                        layer,
                        Role::Graphic,
                        prim,
                        None,
                        None,
                        identity(page, &uuid),
                    );
                }
                b.add(
                    GRAPHICS,
                    Role::Graphic,
                    outline,
                    None,
                    None,
                    identity(page, &uuid),
                )
            }
            Annotation::Text { uuid, spec } => {
                let text = spec.text.clone();
                add_text_spec(b, NOTES, page, &uuid, &text, spec, None, Role::Text, None)
            }
            Annotation::Bus { uuid, prim } => {
                b.add(BUSES, Role::Bus, prim, None, None, identity(page, &uuid))
            }
            Annotation::BusEntry { uuid, prim } => {
                b.add(BUSES, Role::Bus, prim, None, None, identity(page, &uuid))
            }
        }
    }
}

/// Symbol bodies, pins, pin text and visible fields of one page.
fn add_symbols(
    s: &Schematic,
    page: &str,
    nets: &HashMap<String, String>,
    b: &mut Builder,
) -> Result<()> {
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
        let lib_sexp = s.get_lib_symbol(&inst.lib_id);
        let power =
            lib_sexp.is_some_and(|l| l.get("power").is_some()) || inst.lib_id.starts_with("power:");
        let (body_layer, pin_layer, field_layer) = if power {
            (POWER, POWER, POWER)
        } else {
            (SYMBOLS, PINS, FIELDS)
        };
        let place = |p| transform(p, inst.position, inst.rotation, &inst.mirror);
        for (i, g) in lib.graphics.iter().enumerate() {
            let Some(body) = body_prims(g, place, width) else {
                continue;
            };
            let props = identity(page, &format!("{}:{i}", inst.uuid));
            if let Some((fill, prim)) = body.fill {
                let fill_layer = match fill {
                    Fill::Outline => body_layer,
                    _ => BODY_FILL,
                };
                b.add(
                    fill_layer,
                    Role::SymbolBody,
                    prim,
                    None,
                    Some(gid),
                    props.clone(),
                );
            }
            b.add(
                body_layer,
                Role::SymbolBody,
                body.outline,
                None,
                Some(gid),
                props,
            );
        }
        let style = PinTextStyle::from_lib(lib_sexp);
        for pin in lib
            .pins
            .iter()
            .filter(|p| p.unit == inst.unit || p.unit == 0)
        {
            let pin_style = style.pins.get(&pin.number).cloned().unwrap_or_default();
            if pin_style.hidden {
                // KiCad does not draw hidden (usually power) pins.
                continue;
            }
            let at = place(pin.position);
            let a = pin.rotation.to_radians();
            let end = place((
                pin.position.0 + pin.length * a.cos(),
                pin.position.1 + pin.length * a.sin(),
            ));
            let n = nets.get(&key((at[0], at[1]))).map(String::as_str);
            let mut props = identity(page, &format!("{}:{}", inst.uuid, pin.number));
            props.extend([
                Prop::new("number", &pin.number),
                Prop::new("name", &pin.name),
                Prop::new("electrical_type", &pin.pin_type),
                Prop::new("shape", &pin.shape),
            ]);
            b.add(
                pin_layer,
                Role::Pin,
                Prim::Polyline {
                    points: vec![at, end],
                    width: 0.2,
                },
                n,
                Some(gid),
                props,
            );
            if !power {
                for (role, text, spec) in
                    pin_text(&style, &pin_style, &pin.name, &pin.number, at, end)
                {
                    add_text_spec(
                        b,
                        PIN_TEXT,
                        page,
                        &format!("{}:{}:{role}", inst.uuid, pin.number),
                        text,
                        spec,
                        None,
                        Role::Text,
                        Some(gid),
                    );
                }
            }
        }
        for p in inst.properties.values().filter(|p| p.visible) {
            add_text(
                b,
                field_layer,
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
    layer: u16,
    page: &str,
    id: &str,
    text: &str,
    pos: (f64, f64),
    rotation: f64,
    net: Option<&str>,
    role: Role,
    group: Option<u32>,
) {
    let spec = kicad_strokes::TextSpec {
        pos: [pos.0, pos.1],
        angle_deg: rotation,
        // The current schematic schema deliberately normalizes only the
        // common placement fields. Preserve KiCad's standard schematic text
        // metrics here until per-item effects are exposed by that schema.
        size: [1.27, 1.27],
        thickness: kicad_strokes::SCH_DEFAULT_PEN,
        keep_upright: true,
        ..kicad_strokes::TextSpec::default()
    };
    add_text_spec(b, layer, page, id, text, spec, net, role, group)
}
#[allow(clippy::too_many_arguments)]
fn add_text_spec(
    b: &mut Builder,
    layer: u16,
    page: &str,
    id: &str,
    text: &str,
    spec: kicad_strokes::TextSpec,
    net: Option<&str>,
    role: Role,
    group: Option<u32>,
) {
    let pos = spec.pos;
    let rotation = spec.angle_deg;
    let strokes = kicad_strokes::to_strokes(&kicad_strokes::TextSpec {
        text: text.to_owned(),
        ..spec
    });
    let mut props = identity(page, id);
    props.extend([
        Prop::new("text", text),
        Prop::new("at", format!("{},{}", pos[0], pos[1])),
        Prop::new("rotation", rotation.to_string()),
    ]);
    b.add(layer, role, strokes.into_prim(), net, group, props)
}

/// Symbol-level pin text settings from the embedded library symbol:
/// `(pin_names (offset o) [hide])`, `(pin_numbers [hide])` and per-pin
/// `hide` flags and font sizes.
#[derive(Debug, Clone)]
struct PinTextStyle {
    /// Pin-name offset into the body; `None` when names are hidden.
    name_offset: Option<f64>,
    numbers: bool,
    pins: HashMap<String, PinStyle>,
}
#[derive(Debug, Clone)]
struct PinStyle {
    hidden: bool,
    name_size: f64,
    number_size: f64,
    name_hidden: bool,
    number_hidden: bool,
}
impl Default for PinStyle {
    fn default() -> Self {
        Self {
            hidden: false,
            name_size: 1.27,
            number_size: 1.27,
            name_hidden: false,
            number_hidden: false,
        }
    }
}
impl PinTextStyle {
    fn from_lib(lib: Option<&SExp>) -> Self {
        let hide = |n: Option<&SExp>| n.is_some_and(|n| n.flag("hide"));
        let names = lib.and_then(|l| l.get("pin_names"));
        let name_offset =
            (!hide(names)).then(|| names.and_then(|n| n.child_f64("offset")).unwrap_or(0.508));
        let numbers = !hide(lib.and_then(|l| l.get("pin_numbers")));
        let mut pins = HashMap::new();
        for pin in lib.into_iter().flat_map(|l| l.find_all("pin")) {
            let text = |tag| pin.get(tag);
            let size = |tag| {
                text(tag)
                    .and_then(|t| t.find("size"))
                    .and_then(|s| s.float_at(0))
                    .filter(|s| *s > 0.0)
                    .unwrap_or(1.27)
            };
            let text_hidden = |tag| hide(text(tag).and_then(|t| t.get("effects")));
            let Some(number) = text("number").and_then(|t| t.string_at(0)) else {
                continue;
            };
            pins.insert(
                number.to_owned(),
                PinStyle {
                    hidden: pin.flag("hide"),
                    name_size: size("name"),
                    number_size: size("number"),
                    name_hidden: text_hidden("name"),
                    number_hidden: text_hidden("number"),
                },
            );
        }
        Self {
            name_offset,
            numbers,
            pins,
        }
    }
}

/// KiCad pin name/number placement for a pin from connection point `at` to
/// body end `end` (scene coordinates). Names sit inside the body past
/// `end` (or above the pin when the offset is 0); numbers sit above the pin
/// (below it when names are above). Vertical pins read bottom-to-top.
fn pin_text<'a>(
    style: &PinTextStyle,
    pin: &PinStyle,
    name: &'a str,
    number: &'a str,
    at: [f64; 2],
    end: [f64; 2],
) -> Vec<(&'static str, &'a str, kicad_strokes::TextSpec)> {
    use kicad_strokes::{HJustify, TextSpec, VJustify};
    const GAP: f64 = 0.3;
    let (dx, dy) = (end[0] - at[0], end[1] - at[1]);
    let len = dx.hypot(dy);
    if len < 1e-9 {
        return vec![];
    }
    let (ux, uy) = (dx / len, dy / len);
    let vertical = uy.abs() > ux.abs();
    let angle = if vertical { 90.0 } else { 0.0 };
    // "Above" the pin in the text frame: screen up, or screen left when
    // the text is rotated to read upward.
    let (ax, ay) = if vertical { (-1.0, 0.0) } else { (0.0, -1.0) };
    let mid = [(at[0] + end[0]) / 2.0, (at[1] + end[1]) / 2.0];
    let spec = |pos: [f64; 2], size: f64, h, v| TextSpec {
        pos,
        size: [size, size],
        angle_deg: angle,
        justify_h: h,
        justify_v: v,
        thickness: kicad_strokes::SCH_DEFAULT_PEN,
        keep_upright: true,
        ..TextSpec::default()
    };
    let mut out = vec![];
    let show_name = !name.is_empty() && name != "~" && !pin.name_hidden;
    let names_above = matches!(style.name_offset, Some(o) if o <= 0.0);
    if let (Some(offset), true) = (style.name_offset, show_name) {
        if offset > 0.0 {
            // Text runs away from the pin: rightward/upward pins start at
            // the anchor (Left), the others end at it (Right).
            let forward = if vertical { uy < 0.0 } else { ux > 0.0 };
            out.push((
                "name",
                name,
                spec(
                    [end[0] + ux * offset, end[1] + uy * offset],
                    pin.name_size,
                    if forward {
                        HJustify::Left
                    } else {
                        HJustify::Right
                    },
                    VJustify::Center,
                ),
            ));
        } else {
            out.push((
                "name",
                name,
                spec(
                    [mid[0] + ax * GAP, mid[1] + ay * GAP],
                    pin.name_size,
                    HJustify::Center,
                    VJustify::Bottom,
                ),
            ));
        }
    }
    if style.numbers && !pin.number_hidden && !number.is_empty() {
        let (pos, v) = if names_above && show_name {
            ([mid[0] - ax * GAP, mid[1] - ay * GAP], VJustify::Top)
        } else {
            ([mid[0] + ax * GAP, mid[1] + ay * GAP], VJustify::Bottom)
        };
        out.push((
            "number",
            number,
            spec(pos, pin.number_size, HJustify::Center, v),
        ));
    }
    out
}
fn identity(page: &str, id: &str) -> Vec<Prop> {
    vec![Prop::new("page", page), Prop::new("uuid", id)]
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
            LABELS,
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
    const SCH: &str = r#"(kicad_sch (version 20250114) (generator "eeschema")
  (lib_symbols
    (symbol "Tesla:U1" (pin_names (offset 0.762)) (in_bom yes) (on_board yes)
      (property "Reference" "U" (at 0 0 0) (effects (font (size 1.27 1.27))))
      (symbol "U1_0_1"
        (rectangle (start -5 5) (end 5 -5) (stroke (width 0.254) (type default)) (fill (type background)))
        (polyline (pts (xy -1 0) (xy 1 1) (xy 1 -1)) (stroke (width 0) (type default)) (fill (type outline))))
      (symbol "U1_1_1"
        (pin passive line (at -10 0 0) (length 5) (name "VIN" (effects (font (size 0.95 0.95)))) (number "1" (effects (font (size 0.95 0.95)))))
        (pin power_in line (at 10 0 180) (length 5) hide (name "GND" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27))))))))
  (symbol (lib_id "Tesla:U1") (at 100 100 0) (unit 1) (in_bom yes) (on_board yes) (uuid "u-1")
    (property "Reference" "U1" (at 100 92 0) (effects (font (size 1.27 1.27))))
    (property "Value" "LM21421" (at 100 108 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "Tesla:TerminalBlock" (at 100 100 0) (effects (font (size 1.27 1.27))) (hide yes))
    (property "Datasheet" "https://example.com/ds.pdf" (at 100 100 0) (effects (font (size 1.27 1.27)) (hide yes)))))"#;

    fn scene() -> Scene {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.kicad_sch");
        std::fs::write(&path, SCH).unwrap();
        let s = Schematic::load(&path).unwrap();
        let mut b = Builder::new();
        add_symbols(&s, "/", &HashMap::new(), &mut b).unwrap();
        b.scene
    }
    fn filled(p: &Prim) -> bool {
        matches!(
            p,
            Prim::Polygon { fill: true, .. } | Prim::Circle { fill: true, .. }
        )
    }

    #[test]
    fn background_fill_is_a_wash_under_unfilled_outline() {
        let sc = scene();
        let bodies: Vec<_> = sc
            .items
            .iter()
            .filter(|i| i.role == Role::SymbolBody)
            .collect();
        // Rectangle: wash + outline; triangle: outline fill + stroke.
        assert_eq!(bodies.len(), 4);
        let wash: Vec<_> = bodies.iter().filter(|i| i.layer == BODY_FILL).collect();
        assert_eq!(wash.len(), 1);
        assert!(filled(&wash[0].prim));
        // Nothing opaque is filled on the body layer except outline fills.
        let on_body: Vec<_> = bodies.iter().filter(|i| i.layer == SYMBOLS).collect();
        assert_eq!(on_body.iter().filter(|i| filled(&i.prim)).count(), 1);
        assert!(on_body
            .iter()
            .any(|i| matches!(i.prim, Prim::Polygon { fill: false, .. })));
        // The wash layer is translucent and painted below wires and bodies.
        let layer = |id| sc.layers.iter().find(|l| l.id == id).unwrap();
        assert!(layer(BODY_FILL).color[3] < 64);
        assert!(layer(BODY_FILL).z < layer(CONNECTIVITY).z);
        assert!(layer(BODY_FILL).z < layer(SYMBOLS).z);
    }

    #[test]
    fn hidden_fields_and_pins_are_not_drawn() {
        let sc = scene();
        let texts: Vec<_> = sc
            .items
            .iter()
            .filter(|i| i.role == Role::Field)
            .filter_map(|i| i.props.iter().find(|p| p.key == "text"))
            .map(|p| p.value.as_str())
            .collect();
        assert_eq!(texts, ["U1", "LM21421"]);
        let pins: Vec<_> = sc.items.iter().filter(|i| i.role == Role::Pin).collect();
        assert_eq!(pins.len(), 1, "hidden GND pin is skipped");
    }

    #[test]
    fn pin_name_inside_body_and_number_above_pin() {
        let sc = scene();
        let pin_text = |suffix: &str| {
            sc.items
                .iter()
                .find(|i| {
                    i.layer == PIN_TEXT
                        && i.props
                            .iter()
                            .any(|p| p.key == "uuid" && p.value.ends_with(suffix))
                })
                .unwrap()
        };
        let bbox = |i: &Item| {
            let mut s = Scene::new(SceneKind::Schematic, true);
            s.items.push(i.clone());
            s.recompute_bbox();
            s.bbox
        };
        // Pin 1 runs from x=90 to x=95; the name starts inside the body.
        let name = bbox(pin_text(":1:name"));
        assert!(name.min[0] >= 95.0 && name.max[0] < 105.0);
        let number = bbox(pin_text(":1:number"));
        assert!(number.min[0] > 90.0 && number.max[0] < 95.0);
        assert!(number.max[1] < 100.0, "number sits above the pin line");
    }
}
