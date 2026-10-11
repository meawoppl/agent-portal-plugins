//! PCB tab: the backend-reified board scene (`/api/kicad/pcbscene`) drawn by
//! the shared [`VectorSceneCanvas`] host over pastebom's `vector-view`.
//!
//! Everything format-specific (pad shapes, transforms, text strokes, layer
//! order and colours) is done by the backend; the fetched scene is drawn as
//! is. This module owns only the PCB tab's chrome, expressed as canvas props:
//! the layers panel (`visibility`), the pours and reference/value/fab-text
//! toggles (`hidden_items`), the back view (`mirrored` plus the producer's
//! back-view paint order as `passes`), net highlight and the selection
//! overlay, per-project camera persistence, revision refresh with selection
//! carry-over, and the properties panel.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gloo_events::EventListener;
use serde::{Deserialize, Serialize};
use vector_view::style::{parse_css_color, Grid, Highlight, LayerVisibility, Overlay, Pass, Theme};
use vector_view::view::View;
use vector_view::{
    BBox, GroupId, GroupKind, Item, ItemId, Layer, LayerId, LayerKind, NetId, Role, Scene, Side,
    SCENE_VERSION,
};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::vector_scene::VectorSceneCanvas;

const SETTINGS_KEY: &str = "kicadmium:pcb-rust";

/// Footprint text classes shown (bit set).
const TEXT_REFERENCES: u8 = 1;
const TEXT_VALUES: u8 = 2;
/// Footprint user text off the silkscreen (fab-layer `${REFERENCE}` copies,
/// drawing notes). Silkscreen user text always follows its layer.
const TEXT_USER: u8 = 4;

/// Selection overlay colour. White, like the theme's board cursor: composited
/// over any copper it reads as "lit". The previous light blue at 55% turned a
/// selected front track the colour of B.Cu, which looked like a layer change.
const SELECTION: [u8; 4] = [255, 255, 255, 255];
/// Opacity of the selection overlay; high enough that the layer colour
/// underneath only tints the highlight.
const SELECTION_ALPHA: f64 = 0.75;
/// Board outline share of the viewport after a fit (KiCanvas framing).
const FIT_FILL: f64 = 0.78;
/// KiCanvas draws zone fills at this fraction of the copper opacity.
const ZONE_ALPHA: f64 = 0.6;
/// Padding `VectorSceneCanvas` (and `View::fit`) leaves around a fit.
const FIT_PAD: f64 = 24.0;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    hidden: Vec<String>,
    /// Layers off by default (not in the board's layer table) turned on.
    #[serde(default)]
    shown: Vec<String>,
    #[serde(default)]
    flipped: bool,
    #[serde(default)]
    layers_open: bool,
    /// Footprint text classes shown (`TEXT_*`).
    #[serde(default = "default_text")]
    text: u8,
}

/// References on; values and fab/drawing user text off, as KiCanvas shows.
fn default_text() -> u8 {
    TEXT_REFERENCES
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            hidden: Vec::new(),
            shown: Vec::new(),
            flipped: false,
            layers_open: false,
            text: default_text(),
        }
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn load_settings() -> Settings {
    storage()
        .and_then(|s| s.get_item(SETTINGS_KEY).ok().flatten())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(s: &Settings) {
    if let (Some(storage), Ok(json)) = (storage(), serde_json::to_string(s)) {
        let _ = storage.set_item(SETTINGS_KEY, &json);
    }
}

/// Persisted camera (`screen = board * (±scale) + t`, the same JSON as the
/// previous PCB view's).
#[derive(Clone, Copy, Serialize, Deserialize)]
struct Camera {
    scale: f64,
    tx: f64,
    ty: f64,
    #[serde(default)]
    flipped: bool,
}

fn view_key(project: &str) -> String {
    format!("{SETTINGS_KEY}:view:{project}")
}

fn load_view(project: &str) -> Option<View> {
    let c: Camera = storage()?
        .get_item(&view_key(project))
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())?;
    (c.scale.is_finite() && c.scale > 0.0).then_some(View {
        scale: c.scale,
        tx: c.tx,
        ty: c.ty,
        mirrored: c.flipped,
        ..View::default()
    })
}

fn save_view(project: &str, v: &View) {
    let c = Camera {
        scale: v.scale,
        tx: v.tx,
        ty: v.ty,
        flipped: v.mirrored,
    };
    if let (Some(storage), Ok(json)) = (storage(), serde_json::to_string(&c)) {
        let _ = storage.set_item(&view_key(project), &json);
    }
}

impl Settings {
    /// Table layers are on unless switched off; layers the board does not
    /// enable stay off unless switched on.
    fn layer_visible(&self, layer: &Layer) -> bool {
        if layer.visible {
            !self.hidden.contains(&layer.name)
        } else {
            self.shown.contains(&layer.name)
        }
    }

    fn toggle_layer(&mut self, layer: &Layer) {
        let name = layer.name.clone();
        if self.layer_visible(layer) {
            self.shown.retain(|n| *n != name);
            if layer.visible {
                self.hidden.push(name);
            }
        } else {
            self.hidden.retain(|n| *n != name);
            if !layer.visible {
                self.shown.push(name);
            }
        }
    }
}

fn meta<'a>(scene: &'a Scene, key: &str) -> Option<&'a str> {
    scene
        .meta
        .iter()
        .find(|p| p.key == key)
        .map(|p| p.value.as_str())
}

fn prop<'a>(item: &'a Item, key: &str) -> Option<&'a str> {
    item.props
        .iter()
        .find(|p| p.key == key)
        .map(|p| p.value.as_str())
}

/// KiCad layers (the panel), excluding drill passes and pick overlays.
fn panel_layers(scene: &Scene) -> impl Iterator<Item = &Layer> {
    scene
        .layers
        .iter()
        .filter(|l| !matches!(l.kind, LayerKind::Drill | LayerKind::Overlay))
}

/// Text class bit of a text item, or `None` when it always shows.
fn text_class(scene: &Scene, item: &Item) -> Option<u8> {
    match prop(item, "kind")? {
        "reference" => Some(TEXT_REFERENCES),
        "value" => Some(TEXT_VALUES),
        "board" => None,
        _ if scene
            .layer(item.layer)
            .is_some_and(|l| l.kind == LayerKind::Silkscreen) =>
        {
            None
        }
        _ => Some(TEXT_USER),
    }
}

/// Roles drawn in a copper layer's base pass (everything but pads and zones).
const BASE_ROLES: [Role; 16] = [
    Role::Track,
    Role::Via,
    Role::Hole,
    Role::Graphic,
    Role::Text,
    Role::Outline,
    Role::Wire,
    Role::Bus,
    Role::Junction,
    Role::NoConnect,
    Role::Pin,
    Role::Label,
    Role::Field,
    Role::SymbolBody,
    Role::SheetFrame,
    Role::Other,
];

/// Compositing passes for a paint order (bottom first). Every layer blends
/// as one unit at its colour's alpha, as in KiCanvas; copper layers split
/// into tracks/drawings, then pads, then zones at `ZONE_ALPHA`.
fn passes(scene: &Scene, order: &[LayerId]) -> Vec<Pass> {
    let mut out = Vec::new();
    for layer in order.iter().filter_map(|id| scene.layer(*id)) {
        let alpha = layer.color[3] as f64 / 255.0;
        match layer.kind {
            // Pick-only layer (footprint bounds): pickable, never painted.
            LayerKind::Overlay => out.push(Pass::new(layer.id, 0.0)),
            LayerKind::Copper => {
                let pass = |roles: &[Role], a: f64| Pass {
                    layer: layer.id,
                    roles: Some(roles.to_vec()),
                    alpha: a,
                };
                out.push(pass(&BASE_ROLES, alpha));
                out.push(pass(&[Role::Pad], alpha));
                out.push(pass(&[Role::Zone], alpha * ZONE_ALPHA));
            }
            _ => out.push(Pass::new(layer.id, alpha)),
        }
    }
    out
}

/// Everything derived once per fetched scene.
struct Prepared {
    front: Vec<Pass>,
    back: Vec<Pass>,
    theme: Theme,
    background: [u8; 4],
    grid: Grid,
    /// Board outline (Edge.Cuts) bounds.
    outline: BBox,
    zones: HashSet<ItemId>,
    /// Text items per class bit.
    texts: HashMap<u8, HashSet<ItemId>>,
    /// Items per footprint group, without the pick-only bounds.
    groups: HashMap<GroupId, Vec<ItemId>>,
}

/// Bounds of the board outline (Edge.Cuts), else the whole scene.
fn outline_bbox(scene: &Scene) -> BBox {
    let mut b = BBox::EMPTY;
    for item in scene.items.iter().filter(|i| i.role == Role::Outline) {
        b.union(&item.prim.bbox());
    }
    if b.is_empty() {
        scene.bbox
    } else {
        b
    }
}

/// `b` grown about its centre so it fills `fill` of a fitted viewport.
fn framed(b: BBox, fill: f64) -> BBox {
    if b.is_empty() {
        return b;
    }
    let c = [(b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0];
    let (hw, hh) = (b.width() / 2.0 / fill, b.height() / 2.0 / fill);
    BBox {
        min: [c[0] - hw, c[1] - hh],
        max: [c[0] + hw, c[1] + hh],
    }
}

/// The box to hand the host's fit (which pads by `FIT_PAD` CSS pixels) so
/// the outline fills `FIT_FILL` of the limiting viewport side, as KiCanvas
/// frames a board.
fn fit_frame(outline: BBox, w: f64, h: f64) -> BBox {
    let (bw, bh) = (outline.width().max(1e-6), outline.height().max(1e-6));
    let want = (w / bw).min(h / bh) * FIT_FILL;
    let padded = ((w - 2.0 * FIT_PAD).max(1.0) / bw).min((h - 2.0 * FIT_PAD).max(1.0) / bh);
    framed(outline, (want / padded).clamp(0.05, 1.0))
}

fn prepare(scene: &Scene) -> Prepared {
    let mut front: Vec<&Layer> = scene.layers.iter().collect();
    front.sort_by_key(|l| l.z);
    let mut front: Vec<LayerId> = front.into_iter().map(|l| l.id).collect();
    let mut back: Vec<LayerId> = meta(scene, "back_order")
        .unwrap_or("")
        .split(',')
        .filter_map(|v| v.parse().ok())
        .collect();
    if back.is_empty() {
        back = front.clone();
    }
    // The pick-only layer goes on top of both stacks.
    let overlays: Vec<LayerId> = scene
        .layers
        .iter()
        .filter(|l| l.kind == LayerKind::Overlay)
        .map(|l| l.id)
        .collect();
    for order in [&mut front, &mut back] {
        order.retain(|id| !overlays.contains(id));
        order.extend(&overlays);
    }
    // A highlighted net keeps each layer's colour while unrelated items dim.
    let mut theme = Theme::default();
    for l in &scene.layers {
        // Passes apply the layer alpha; items draw opaque.
        theme
            .layer_colors
            .insert(l.id, [l.color[0], l.color[1], l.color[2], 255]);
    }
    let mut zones = HashSet::new();
    let mut texts: HashMap<u8, HashSet<ItemId>> = HashMap::new();
    let mut groups: HashMap<GroupId, Vec<ItemId>> = HashMap::new();
    for item in &scene.items {
        match item.role {
            Role::Zone => {
                zones.insert(item.id);
            }
            Role::Text => {
                if let Some(bit) = text_class(scene, item) {
                    texts.entry(bit).or_default().insert(item.id);
                }
            }
            _ => {}
        }
        let pick_only = scene
            .layer(item.layer)
            .is_some_and(|l| l.kind == LayerKind::Overlay);
        if let (Some(g), false) = (item.group, pick_only) {
            groups.entry(g).or_default().push(item.id);
        }
    }
    let color = |key: &str, fallback| {
        meta(scene, key)
            .and_then(parse_css_color)
            .unwrap_or(fallback)
    };
    Prepared {
        front: passes(scene, &front),
        back: passes(scene, &back),
        theme,
        background: color("background", [30, 30, 30, 255]),
        grid: Grid {
            color: color("grid", [50, 50, 50, 255]),
            min_spacing_px: 15.0,
            dot_px: 2.0,
        },
        outline: outline_bbox(scene),
        zones,
        texts,
        groups,
    }
}

#[derive(Properties, PartialEq)]
pub struct PcbViewProps {
    pub project: AttrValue,
    /// Workbench revision; a change re-fetches the board.
    #[prop_or_default]
    pub revision: AttrValue,
    #[prop_or(true)]
    pub pours: bool,
}

#[derive(Clone, PartialEq)]
struct Loaded {
    project: String,
    revision: String,
    scene: Rc<Scene>,
}

/// A selected item and the net it highlights.
#[derive(Clone, Copy, PartialEq)]
struct Selection {
    item: ItemId,
    net: Option<NetId>,
}

fn select(scene: &Scene, id: ItemId) -> Option<Selection> {
    let item = scene.items.iter().find(|i| i.id == id)?;
    Some(Selection {
        item: id,
        net: item.net,
    })
}

/// Re-target a selection onto a reloaded scene: the same footprint pad, or
/// the same footprint item, else nothing.
fn carry_over(old: &Scene, new: &Scene, sel: Selection) -> Option<Selection> {
    let item = old.items.iter().find(|i| i.id == sel.item)?;
    let group = old.group(item.group?)?;
    let pad = prop(item, "Pad");
    let found = new.items.iter().find(|i| {
        i.role == item.role
            && i.layer == item.layer
            && prop(i, "Pad") == pad
            && i.group
                .and_then(|g| new.group(g))
                .is_some_and(|g| g.label == group.label && g.kind == group.kind)
    })?;
    select(new, found.id)
}

/// Items drawn in the selection overlay: the whole pad (all its copper
/// copies) or footprint; vias and tracks alone; nothing for zones (the net
/// highlight shows them).
fn overlay_items(scene: &Scene, prepared: &Prepared, sel: Selection) -> HashSet<ItemId> {
    let Some(item) = scene.items.iter().find(|i| i.id == sel.item) else {
        return HashSet::new();
    };
    let footprint = item.group.filter(|g| {
        scene
            .group(*g)
            .is_some_and(|g| g.kind == GroupKind::Footprint)
    });
    match (item.role, footprint) {
        (Role::Zone, _) => HashSet::new(),
        (Role::Pad | Role::Hole, Some(g)) => {
            let pad = prop(item, "Pad");
            prepared.groups[&g]
                .iter()
                .copied()
                .filter(|id| {
                    scene
                        .items
                        .iter()
                        .find(|i| i.id == *id)
                        .is_some_and(|i| i.role == Role::Pad && prop(i, "Pad") == pad)
                })
                .collect()
        }
        (_, Some(g)) => prepared
            .groups
            .get(&g)
            .into_iter()
            .flatten()
            .copied()
            .collect(),
        _ => HashSet::from([item.id]),
    }
}

/// A toolbar camera change, given the viewport size in CSS pixels.
type CameraAction = Rc<dyn Fn(&mut View, f64, f64)>;

fn container_size(node: &NodeRef) -> (f64, f64) {
    node.cast::<web_sys::Element>()
        .map(|e| (e.client_width() as f64, e.client_height() as f64))
        .unwrap_or((1.0, 1.0))
}

#[function_component(PcbView)]
pub fn pcb_view(props: &PcbViewProps) -> Html {
    let loaded = use_state(|| None::<Loaded>);
    let status = use_state(|| "Loading board…".to_string());
    let settings = use_state(load_settings);
    let selection = use_state(|| None::<Selection>);
    let initial_view = use_state_eq(|| None::<View>);
    let last_view: Rc<RefCell<Option<View>>> = use_mut_ref(|| None);
    let stage = use_node_ref();

    {
        let loaded = loaded.clone();
        let status = status.clone();
        let selection = selection.clone();
        let initial_view = initial_view.clone();
        let last_view = last_view.clone();
        use_effect_with(
            (props.project.clone(), props.revision.clone()),
            move |(project, _)| {
                let project = project.to_string();
                status.set("Loading board…".to_string());
                if loaded
                    .as_ref()
                    .is_some_and(|current| current.project != project)
                {
                    loaded.set(None);
                    selection.set(None);
                }
                let token = crate::request::RequestToken::default();
                let task_token = token.clone();
                spawn_local(async move {
                    let response = crate::api::pcb_scene(&project).await;
                    if !task_token.current() {
                        return;
                    }
                    match response {
                        Ok(scene) if scene.version != SCENE_VERSION => status.set(format!(
                            "Unsupported PCB scene version {} (viewer expects {SCENE_VERSION})",
                            scene.version
                        )),
                        Ok(scene) => {
                            let revision = meta(&scene, "revision").unwrap_or("").to_string();
                            let previous = (*loaded).clone();
                            let same_project =
                                previous.as_ref().is_some_and(|p| p.project == project);
                            if same_project
                                && previous.as_ref().is_some_and(|p| p.revision == revision)
                            {
                                status.set(String::new());
                                return;
                            }
                            let scene = Rc::new(scene);
                            match (&previous, *selection) {
                                (Some(p), Some(sel)) if same_project => {
                                    selection.set(carry_over(&p.scene, &scene, sel))
                                }
                                _ => selection.set(None),
                            }
                            // Keep the camera across revisions; restore the
                            // saved one (or fit) on a project switch.
                            let view = if same_project {
                                *last_view.borrow()
                            } else {
                                load_view(&project)
                            };
                            *last_view.borrow_mut() = view;
                            initial_view.set(view);
                            loaded.set(Some(Loaded {
                                project,
                                revision,
                                scene,
                            }));
                            status.set(String::new());
                        }
                        Err(err) => status.set(format!("PCB view failed: {err}")),
                    }
                });
                move || token.cancel()
            },
        );
    }
    // Escape clears the selection.
    {
        let selection = selection.clone();
        use_effect_with((), move |_| {
            let listener = web_sys::window().map(|w| {
                EventListener::new(&w, "keydown", move |e| {
                    if e.dyn_ref::<web_sys::KeyboardEvent>()
                        .is_some_and(|k| k.key() == "Escape")
                    {
                        selection.set(None);
                    }
                })
            });
            move || drop(listener)
        });
    }

    let scene = loaded.as_ref().map(|l| l.scene.clone());
    let prepared = use_memo(scene.clone(), |scene| scene.as_ref().map(|s| prepare(s)));

    let s = (*settings).clone();
    let set = |f: Rc<dyn Fn(&mut Settings)>| {
        let settings = settings.clone();
        Callback::from(move |_: MouseEvent| {
            let mut s = (*settings).clone();
            f(&mut s);
            save_settings(&s);
            settings.set(s);
        })
    };
    // Camera actions computed here and handed to the host as `initial_view`.
    let camera = |f: CameraAction| {
        let initial_view = initial_view.clone();
        let last_view = last_view.clone();
        let stage = stage.clone();
        Callback::from(move |_: MouseEvent| {
            let (w, h) = container_size(&stage);
            let mut v = last_view.borrow().unwrap_or_default();
            f(&mut v, w, h);
            *last_view.borrow_mut() = Some(v);
            initial_view.set(Some(v));
        })
    };
    let outline = prepared
        .as_ref()
        .as_ref()
        .map_or(BBox::EMPTY, |p| p.outline);
    let flip = {
        let settings = settings.clone();
        let last_view = last_view.clone();
        let stage = stage.clone();
        Callback::from(move |_: MouseEvent| {
            let mut s = (*settings).clone();
            s.flipped = !s.flipped;
            // The host mirrors about the viewport centre; track it so a
            // later refresh restores the same camera.
            if let Some(v) = last_view.borrow_mut().as_mut() {
                v.set_mirrored(s.flipped, container_size(&stage).0);
            }
            save_settings(&s);
            settings.set(s);
        })
    };
    let flipped = s.flipped;
    let toolbar = html! {
        <div class="pcbv-toolbar" role="toolbar" aria-label="PCB view">
            <button type="button" aria-label="Zoom out"
                onclick={camera(Rc::new(|v: &mut View, w, h| v.zoom_at(w / 2.0, h / 2.0, 1.0 / 1.25)))}>{"−"}</button>
            <button type="button" aria-label="Fit board"
                onclick={camera(Rc::new(move |v: &mut View, w, h| { v.mirrored = flipped; v.fit(&fit_frame(outline, w, h), w, h, FIT_PAD) }))}>{"Fit"}</button>
            <button type="button" aria-label="Zoom in"
                onclick={camera(Rc::new(|v: &mut View, w, h| v.zoom_at(w / 2.0, h / 2.0, 1.25)))}>{"+"}</button>
            <button type="button" class={classes!(s.flipped.then_some("on"))} aria-pressed={s.flipped.to_string()}
                title="View from the back (mirrored)" onclick={flip}>
                {if s.flipped { "Back" } else { "Front" }}</button>
            <button type="button" class={classes!(s.layers_open.then_some("on"))} aria-pressed={s.layers_open.to_string()}
                onclick={set(Rc::new(|s: &mut Settings| s.layers_open = !s.layers_open))}>{"Layers"}</button>
        </div>
    };

    let layers_panel = match (&scene, &*prepared, s.layers_open) {
        (Some(scene), Some(prepared), true) => {
            let preset = |label: &'static str, keep: fn(&Layer) -> bool| {
                let names: Vec<String> = panel_layers(scene)
                    .filter(|l| !keep(l))
                    .map(|l| l.name.clone())
                    .collect();
                html! {<button type="button" onclick={set(Rc::new(move |s: &mut Settings| {
                    s.hidden = names.clone();
                    s.shown.clear();
                }))}>{label}</button>}
            };
            let rows = panel_layers(scene).map(|layer| {
                let visible = s.layer_visible(layer);
                let swatch = over(layer.color, prepared.background);
                let alias = meta(scene, &format!("layer_alias:{}", layer.name))
                    .unwrap_or("")
                    .to_string();
                let toggle = {
                    let settings = settings.clone();
                    let layer = layer.clone();
                    Callback::from(move |_: Event| {
                        let mut s = (*settings).clone();
                        s.toggle_layer(&layer);
                        save_settings(&s);
                        settings.set(s);
                    })
                };
                html! {
                    <label class="pcbv-layer" title={alias.clone()}>
                        <input type="checkbox" checked={visible} onchange={toggle}/>
                        <span class="pcbv-swatch" style={format!("background:{swatch}")}/>
                        <span class="pcbv-layer-name">
                            <span>{layer.name.clone()}</span>
                            if !alias.is_empty() && alias != layer.name {
                                <span class="pcbv-layer-alias">{alias}</span>
                            }
                        </span>
                    </label>
                }
            });
            let text_toggle = |label: &'static str, bit: u8| {
                let settings = settings.clone();
                let onchange = Callback::from(move |_: Event| {
                    let mut s = (*settings).clone();
                    s.text ^= bit;
                    save_settings(&s);
                    settings.set(s);
                });
                html! {
                    <label class="pcbv-layer">
                        <input type="checkbox" checked={s.text & bit != 0} {onchange}/>
                        <span>{label}</span>
                    </label>
                }
            };
            let front = |l: &Layer| matches!(l.side, Side::Front | Side::None);
            let back = |l: &Layer| matches!(l.side, Side::Back | Side::None);
            let copper = |l: &Layer| matches!(l.kind, LayerKind::Copper | LayerKind::EdgeCuts);
            html! {
                <div class="pcbv-layers" aria-label="Layers">
                    <div class="pcbv-texts">
                        {text_toggle("References", TEXT_REFERENCES)}
                        {text_toggle("Values", TEXT_VALUES)}
                        {text_toggle("Fab / drawing text", TEXT_USER)}
                    </div>
                    <div class="pcbv-presets">
                        {preset("All", |_| true)}
                        {preset("Front", front)}
                        {preset("Back", back)}
                        {preset("Copper", copper)}
                    </div>
                    {for rows}
                </div>
            }
        }
        _ => Html::default(),
    };

    let properties = match (&scene, *selection) {
        (Some(scene), Some(sel)) => {
            let close = {
                let selection = selection.clone();
                Callback::from(move |_: MouseEvent| selection.set(None))
            };
            describe(scene, sel)
                .map(|(title, rows)| {
                    let net = sel.net.and_then(|n| scene.net(n)).map(|n| n.name.clone());
                    html! {
                        <div class="pcbv-props" role="dialog" aria-label="Selection properties">
                            <div class="pcbv-props-head">
                                <strong>{title}</strong>
                                {net.map(|n| html!{<span class="pcbv-net">{format!("net {n} highlighted")}</span>}).unwrap_or_default()}
                                <button type="button" aria-label="Clear selection" onclick={close}>{"×"}</button>
                            </div>
                            <dl>{for rows.into_iter().map(|(k, v)| html!{<><dt>{k}</dt><dd>{v}</dd></>})}</dl>
                        </div>
                    }
                })
                .unwrap_or_default()
        }
        _ => Html::default(),
    };

    let canvas = match (&scene, &*prepared, &*loaded) {
        (Some(scene), Some(p), Some(l)) => {
            let copper_on = scene
                .layers
                .iter()
                .any(|l| l.kind == LayerKind::Copper && s.layer_visible(l));
            let mut visibility = LayerVisibility::default();
            for layer in &scene.layers {
                let on = match layer.kind {
                    LayerKind::Drill => copper_on,
                    LayerKind::Overlay => true,
                    _ => s.layer_visible(layer),
                };
                visibility.set(layer.id, on);
            }
            let mut hidden_items: HashSet<ItemId> = HashSet::new();
            if !props.pours {
                hidden_items.extend(&p.zones);
            }
            for (bit, ids) in &p.texts {
                if s.text & bit == 0 {
                    hidden_items.extend(ids);
                }
            }
            let sel = *selection;
            let highlight = sel
                .and_then(|s| s.net)
                .map_or(Highlight::None, Highlight::Net);
            let overlay = sel
                .map(|sel| overlay_items(scene, p, sel))
                .filter(|ids| !ids.is_empty())
                .map(|ids| Overlay {
                    highlight: Highlight::Items(ids.clone()),
                    color: SELECTION,
                    alpha: SELECTION_ALPHA,
                });
            let outline_items = sel
                .map(|sel| overlay_items(scene, p, sel))
                .unwrap_or_default();
            let on_select = {
                let selection = selection.clone();
                let scene = scene.clone();
                Callback::from(move |id: Option<ItemId>| {
                    selection.set(id.and_then(|id| select(&scene, id)))
                })
            };
            let on_view = {
                let last_view = last_view.clone();
                let project = l.project.clone();
                Callback::from(move |v: View| {
                    *last_view.borrow_mut() = Some(v);
                    save_view(&project, &v);
                })
            };
            let passes = if s.flipped { &p.back } else { &p.front };
            html! {
                <VectorSceneCanvas scene={scene.clone()} {on_select} {visibility} {hidden_items}
                    mirrored={s.flipped} passes={Some(passes.clone())} {highlight} {overlay}
                    {outline_items}
                    theme={p.theme.clone()} background={Some(p.background)} grid={Some(p.grid)}
                    fit_bbox={Some(p.outline)} fit_fill={Some(FIT_FILL)} initial_view={*initial_view} {on_view}/>
            }
        }
        _ => Html::default(),
    };
    let revision = loaded
        .as_ref()
        .map(|l| l.revision.clone())
        .unwrap_or_default();
    let message = (*status).clone();
    // Advertised for the annotation composer: which copper is on screen.
    let annotate_layers = scene
        .as_ref()
        .map(|scene| {
            scene
                .layers
                .iter()
                .filter(|l| l.kind == LayerKind::Copper && s.layer_visible(l))
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    html! {
        <div class="pcbv" ref={stage} data-revision={revision} data-annotate-layers={annotate_layers}>
            {canvas}
            {toolbar}
            {layers_panel}
            {properties}
            {if message.is_empty() { Html::default() } else { html!{<div class="pcbv-status" role="status">{message}</div>} }}
        </div>
    }
}

/// CSS colour of `c` composited over `bg`.
fn over(c: [u8; 4], bg: [u8; 4]) -> String {
    let a = c[3] as f64 / 255.0;
    let mix = |i: usize| (c[i] as f64 * a + bg[i] as f64 * (1.0 - a)).round() as u8;
    format!("rgb({},{},{})", mix(0), mix(1), mix(2))
}

/// Properties panel title and rows for a selection: the item's own
/// properties, then its footprint's.
fn describe(scene: &Scene, sel: Selection) -> Option<(String, Vec<(String, String)>)> {
    let item = scene.items.iter().find(|i| i.id == sel.item)?;
    let group = item.group.and_then(|g| scene.group(g));
    let rows_of = |props: &[vector_view::Prop]| {
        props
            .iter()
            .filter(|p| p.key != "Type")
            .map(|p| (p.key.clone(), p.value.clone()))
            .collect::<Vec<_>>()
    };
    let footprint = group.filter(|g| g.kind == GroupKind::Footprint);
    Some(match (item.role, footprint) {
        (Role::Pad | Role::Hole, Some(fp)) => {
            let mut rows = rows_of(&item.props);
            rows.extend(rows_of(&fp.props));
            let pad = prop(item, "Pad").unwrap_or("");
            (format!("{} pad {pad}", fp.label), rows)
        }
        (_, Some(fp)) => {
            let value = fp
                .props
                .iter()
                .find(|p| p.key == "Value")
                .map_or("", |p| p.value.as_str());
            (format!("{} · {value}", fp.label), rows_of(&fp.props))
        }
        _ => {
            let title = prop(item, "Type")
                .map(str::to_string)
                .unwrap_or_else(|| format!("{:?}", item.role));
            let mut rows = rows_of(&item.props);
            if rows.is_empty() {
                rows.push((
                    "Layer".into(),
                    scene
                        .layer(item.layer)
                        .map(|l| l.name.clone())
                        .unwrap_or_default(),
                ));
            }
            (title, rows)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vector_view::{Group, Prim, Prop, SceneKind};

    fn layer(id: u16, name: &str, kind: LayerKind, z: i32, visible: bool) -> Layer {
        Layer {
            id,
            name: name.into(),
            kind,
            side: Side::Front,
            z,
            color: [200, 100, 50, 204],
            visible,
        }
    }

    fn item(id: u32, layer: u16, role: Role, group: Option<u32>, props: Vec<Prop>) -> Item {
        Item {
            id,
            layer,
            role,
            prim: Prim::Circle {
                center: [0.0, 0.0],
                radius: 1.0,
                fill: true,
                stroke: 0.0,
            },
            net: None,
            group,
            props,
        }
    }

    fn scene() -> Scene {
        let mut s = Scene::new(SceneKind::Pcb, true);
        s.layers = vec![
            layer(0, "F.Cu", LayerKind::Copper, 20, true),
            layer(1, "F.SilkS", LayerKind::Silkscreen, 10, true),
            layer(2, "F.Fab", LayerKind::Fabrication, 0, false),
            layer(9, "Footprint bounds", LayerKind::Overlay, 99, false),
        ];
        s.groups.push(Group {
            id: 1,
            kind: GroupKind::Footprint,
            label: "R1".into(),
            props: vec![Prop::new("Value", "10k")],
        });
        let pad = || vec![Prop::new("Pad", "1")];
        s.items = vec![
            item(1, 0, Role::Zone, None, vec![]),
            item(
                2,
                1,
                Role::Text,
                Some(1),
                vec![Prop::new("kind", "reference")],
            ),
            item(3, 2, Role::Text, Some(1), vec![Prop::new("kind", "value")]),
            item(4, 2, Role::Text, Some(1), vec![Prop::new("kind", "user")]),
            item(5, 1, Role::Text, None, vec![Prop::new("kind", "user")]),
            item(6, 0, Role::Pad, Some(1), pad()),
            item(7, 0, Role::Pad, Some(1), vec![Prop::new("Pad", "2")]),
            item(8, 9, Role::Other, Some(1), vec![]),
        ];
        s.meta.push(Prop::new("back_order", "2,0,1"));
        s
    }

    #[test]
    fn prepare_classifies_and_orders() {
        let s = scene();
        let p = prepare(&s);
        assert_eq!(p.zones, HashSet::from([1]));
        assert_eq!(p.texts[&TEXT_REFERENCES], HashSet::from([2]));
        assert_eq!(p.texts[&TEXT_VALUES], HashSet::from([3]));
        assert_eq!(p.texts[&TEXT_USER], HashSet::from([4]));
        // Front: by z, copper split base/pads/zones, bounds last at alpha 0.
        let layers: Vec<LayerId> = p.front.iter().map(|x| x.layer).collect();
        assert_eq!(layers, vec![2, 1, 0, 0, 0, 9]);
        assert_eq!(p.front[4].roles.as_deref(), Some(&[Role::Zone][..]));
        assert!((p.front[4].alpha - 0.8 * ZONE_ALPHA).abs() < 1e-9);
        assert_eq!(p.front[5].alpha, 0.0);
        let back: Vec<LayerId> = p.back.iter().map(|x| x.layer).collect();
        assert_eq!(back, vec![2, 0, 0, 0, 1, 9]);
        assert_eq!(p.theme.layer_colors[&0][3], 255);
        // Footprint groups exclude the pick-only bounds.
        assert_eq!(p.groups[&1], vec![2, 3, 4, 6, 7]);
    }

    #[test]
    fn overlay_expands_pads_and_footprints() {
        let s = scene();
        let p = prepare(&s);
        let sel = |id| select(&s, id).unwrap();
        assert_eq!(overlay_items(&s, &p, sel(6)), HashSet::from([6]));
        assert_eq!(overlay_items(&s, &p, sel(8)).len(), 5);
        assert!(overlay_items(&s, &p, sel(1)).is_empty());
        let (title, _) = describe(&s, sel(6)).unwrap();
        assert_eq!(title, "R1 pad 1");
        let (title, _) = describe(&s, sel(8)).unwrap();
        assert_eq!(title, "R1 · 10k");
    }

    #[test]
    fn settings_toggle_table_and_disabled_layers() {
        let s = scene();
        let mut set = Settings::default();
        assert!(set.layer_visible(&s.layers[0]) && !set.layer_visible(&s.layers[2]));
        set.toggle_layer(&s.layers[0]);
        set.toggle_layer(&s.layers[2]);
        assert!(!set.layer_visible(&s.layers[0]) && set.layer_visible(&s.layers[2]));
        set.toggle_layer(&s.layers[2]);
        assert!(set.shown.is_empty());
    }

    #[test]
    fn framing_and_swatch() {
        let b = framed(
            BBox {
                min: [0.0, 0.0],
                max: [78.0, 39.0],
            },
            0.78,
        );
        assert!((b.width() - 100.0).abs() < 1e-9 && (b.height() - 50.0).abs() < 1e-9);
        assert_eq!(over([255, 0, 0, 128], [0, 0, 0, 255]), "rgb(128,0,0)");
        // The host's padded fit of the frame lands on the KiCanvas scale.
        let outline = BBox {
            min: [0.0, 0.0],
            max: [100.0, 50.0],
        };
        let mut v = View::default();
        v.fit(&fit_frame(outline, 1000.0, 500.0), 1000.0, 500.0, FIT_PAD);
        assert!((v.scale - 7.8).abs() < 1e-9, "{}", v.scale);
    }
}
