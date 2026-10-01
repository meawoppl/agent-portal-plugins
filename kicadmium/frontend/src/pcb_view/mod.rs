//! Rust/WASM PCB view (Canvas2D), the replacement for the KiCanvas PCB
//! renderer. Board data comes from `/api/kicad/pcbview` (built by the
//! backend from `kct::schema::pcb`) and is re-fetched whenever the
//! workbench revision changes. Rendering, hit testing, pointer/touch
//! handling and settings persistence are derived from pastebom's viewer
//! (`crates/viewer/src/{render,pcbdata,state,main}.rs`).
//!
//! THIRD-PARTY PROVENANCE: parts of this module are derived from pastebom's
//! viewer (github.com/meawoppl/pastebom.com, `crates/viewer`), same author.
//! That repository has NO license file; no license is claimed here for the
//! derived parts, and relicensing them under kicadmium's MIT is the author's
//! decision. Items are listed in `docs/third-party.md`.
//!
//! `text.rs` and `theme.rs` are kicadmium-original.

mod geom;
mod render;
mod text;
mod theme;

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gloo_events::{EventListener, EventListenerOptions};
use serde::{Deserialize, Serialize};
use shared::pcb::{PcbBoard, PcbShape, PcbViewResponse};
use wasm_bindgen::{closure::Closure, JsCast};
use wasm_bindgen_futures::spawn_local;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, PointerEvent, WheelEvent};
use yew::prelude::*;

use geom::{hit_net, Hit, View};
use render::{draw, pick_slop, Frame, Scene, TEXT_REFERENCES, TEXT_USER, TEXT_VALUES};
use theme::{ui_order, Theme};

const SETTINGS_KEY: &str = "kicadmium:pcb-rust";

#[derive(Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    hidden: Vec<String>,
    /// Layers missing from the board's layer table that the user turned on.
    #[serde(default)]
    shown: Vec<String>,
    #[serde(default)]
    flipped: bool,
    #[serde(default)]
    layers_open: bool,
    /// Footprint text classes shown (`render::TEXT_*`).
    #[serde(default = "default_text")]
    text: u8,
}

/// References on; values and fab/drawing user text (`${REFERENCE}`
/// copies) off, which is what the KiCanvas view shows.
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

fn view_key(project: &str) -> String {
    format!("{SETTINGS_KEY}:view:{project}")
}

fn load_view(project: &str) -> Option<View> {
    storage()?
        .get_item(&view_key(project))
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<View>(&s).ok())
        .filter(|v| v.scale.is_finite() && v.scale > 0.0)
}

fn load_settings() -> Settings {
    storage()
        .and_then(|s| s.get_item(SETTINGS_KEY).ok().flatten())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Mutable view state shared by event handlers and the frame callback.
struct Engine {
    canvas: Option<HtmlCanvasElement>,
    board: Option<Rc<PcbBoard>>,
    revision: String,
    project: String,
    scene: Option<Scene>,
    theme: Theme,
    view: View,
    fitted: bool,
    hidden: HashSet<String>,
    shown: HashSet<String>,
    flipped: bool,
    layers_open: bool,
    text: u8,
    pours: bool,
    selection: Option<Hit>,
    highlight: u32,
    frame_pending: bool,
    pointers: Vec<(i32, [f64; 2])>,
    press: Option<[f64; 2]>,
    dragged: bool,
    pinch: Option<(f64, [f64; 2])>,
    /// Yew re-render hook for panel state.
    notify: Option<Callback<()>>,
}

impl Engine {
    fn new() -> Self {
        let s = load_settings();
        Engine {
            canvas: None,
            board: None,
            revision: String::new(),
            project: String::new(),
            scene: None,
            theme: Theme::load(),
            view: View::default(),
            fitted: false,
            hidden: s.hidden.into_iter().collect(),
            shown: s.shown.into_iter().collect(),
            flipped: s.flipped,
            layers_open: s.layers_open,
            text: s.text,
            pours: true,
            selection: None,
            highlight: 0,
            frame_pending: false,
            pointers: Vec::new(),
            press: None,
            dragged: false,
            pinch: None,
            notify: None,
        }
    }

    fn save(&self) {
        let sorted = |set: &HashSet<String>| {
            let mut v: Vec<String> = set.iter().cloned().collect();
            v.sort();
            v
        };
        let s = Settings {
            hidden: sorted(&self.hidden),
            shown: sorted(&self.shown),
            flipped: self.flipped,
            layers_open: self.layers_open,
            text: self.text,
        };
        if let (Some(storage), Ok(json)) = (storage(), serde_json::to_string(&s)) {
            let _ = storage.set_item(SETTINGS_KEY, &json);
        }
        self.save_view();
    }

    /// Per-project camera, restored when the tab or project comes back.
    fn save_view(&self) {
        if self.board.is_none() || !self.fitted {
            return;
        }
        if let (Some(storage), Ok(json)) = (storage(), serde_json::to_string(&self.view)) {
            let _ = storage.set_item(&view_key(&self.project), &json);
        }
    }

    fn css_size(&self) -> (f64, f64) {
        self.canvas
            .as_ref()
            .map(|c| (c.client_width() as f64, c.client_height() as f64))
            .unwrap_or((0.0, 0.0))
    }

    fn fit(&mut self) {
        let (w, h) = self.css_size();
        if let (Some(board), true) = (&self.board, w > 0.0 && h > 0.0) {
            self.view = View::fit(board.bbox, w, h, self.flipped);
            self.fitted = true;
        }
    }

    fn paint(&mut self) {
        let Some(canvas) = self.canvas.clone() else {
            return;
        };
        let dpr = web_sys::window()
            .map(|w| w.device_pixel_ratio())
            .unwrap_or(1.0);
        let (w, h) = self.css_size();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (pw, ph) = ((w * dpr).round() as u32, (h * dpr).round() as u32);
        if canvas.width() != pw || canvas.height() != ph {
            canvas.set_width(pw);
            canvas.set_height(ph);
        }
        let Some(ctx) = canvas
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok())
        else {
            return;
        };
        let Some(board) = self.board.clone() else {
            let _ = ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            ctx.set_fill_style_str(&self.theme.background.css());
            ctx.fill_rect(0.0, 0.0, pw as f64, ph as f64);
            return;
        };
        if !self.fitted {
            self.fit();
        }
        if self
            .scene
            .as_ref()
            .is_none_or(|s| !s.matches(self.highlight, self.text))
        {
            self.scene = Some(Scene::build(&board, self.highlight, self.text));
        }
        let hidden = self.effective_hidden();
        let frame = Frame {
            board: &board,
            theme: &self.theme,
            view: self.view,
            width: w,
            height: h,
            dpr,
            hidden: &hidden,
            pours: self.pours,
            selection: self.selection,
        };
        if let Some(scene) = &self.scene {
            draw(&ctx, scene, &frame);
        }
    }

    /// Layers off by default (not in the board's table) stay off unless
    /// switched on; table layers are on unless switched off.
    fn name_visible(&self, layer: &shared::pcb::PcbLayer) -> bool {
        if layer.disabled {
            self.shown.contains(&layer.name)
        } else {
            !self.hidden.contains(&layer.name)
        }
    }

    fn effective_hidden(&self) -> HashSet<String> {
        self.board
            .iter()
            .flat_map(|b| b.layers.iter())
            .filter(|l| !self.name_visible(l))
            .map(|l| l.name.clone())
            .collect()
    }

    fn toggle_layer(&mut self, name: &str) {
        let Some(layer) = self
            .board
            .as_ref()
            .and_then(|b| b.layers.iter().find(|l| l.name == name).cloned())
        else {
            return;
        };
        if self.name_visible(&layer) {
            self.hidden.insert(layer.name.clone());
            self.shown.remove(&layer.name);
        } else {
            self.hidden.remove(&layer.name);
            self.shown.insert(layer.name.clone());
        }
    }

    fn layer_visible(&self, index: u16) -> bool {
        self.board
            .as_ref()
            .and_then(|b| b.layers.get(index as usize))
            .is_some_and(|l| self.name_visible(l))
    }

    fn front_layer(&self) -> u16 {
        let name = if self.flipped { "B.Cu" } else { "F.Cu" };
        self.board
            .as_ref()
            .and_then(|b| b.layers.iter().position(|l| l.name == name))
            .unwrap_or(0) as u16
    }

    fn click(&mut self, at: [f64; 2]) {
        let Some(board) = self.board.clone() else {
            return;
        };
        let p = self.view.to_board(at);
        let hit = geom::hit_test(
            &board,
            p,
            pick_slop(&self.view),
            &|l| self.layer_visible(l),
            self.front_layer(),
            self.pours,
        );
        self.selection = hit;
        self.highlight = hit.map(|h| hit_net(&board, h)).unwrap_or(0);
    }
}

type Shared = Rc<RefCell<Engine>>;

fn request_frame(engine: &Shared) {
    {
        let mut e = engine.borrow_mut();
        if e.frame_pending {
            return;
        }
        e.frame_pending = true;
    }
    let engine = engine.clone();
    let callback = Closure::once_into_js(move || {
        let mut e = engine.borrow_mut();
        e.frame_pending = false;
        e.paint();
    });
    if let Some(window) = web_sys::window() {
        let _ = window.request_animation_frame(callback.unchecked_ref());
    }
}

fn notify(engine: &Shared) {
    let cb = engine.borrow().notify.clone();
    if let Some(cb) = cb {
        cb.emit(());
    }
}

fn local_point(canvas: &HtmlCanvasElement, e: &web_sys::MouseEvent) -> [f64; 2] {
    let rect = canvas.get_bounding_client_rect();
    [
        e.client_x() as f64 - rect.left(),
        e.client_y() as f64 - rect.top(),
    ]
}

fn install_input(engine: &Shared, canvas: &HtmlCanvasElement) -> Vec<EventListener> {
    let mut out = Vec::new();
    let active = EventListenerOptions::enable_prevent_default();
    {
        let engine = engine.clone();
        let c = canvas.clone();
        out.push(EventListener::new_with_options(
            canvas,
            "pointerdown",
            active,
            move |event| {
                let Some(e) = event.dyn_ref::<PointerEvent>() else {
                    return;
                };
                let _ = c.set_pointer_capture(e.pointer_id());
                let p = local_point(&c, e);
                let mut s = engine.borrow_mut();
                s.pointers.retain(|(id, _)| *id != e.pointer_id());
                s.pointers.push((e.pointer_id(), p));
                if s.pointers.len() == 1 {
                    s.press = Some(p);
                    s.dragged = false;
                } else {
                    s.dragged = true;
                    s.pinch = None;
                }
                event.prevent_default();
            },
        ));
    }
    {
        let engine = engine.clone();
        let c = canvas.clone();
        out.push(EventListener::new(canvas, "pointermove", move |event| {
            let Some(e) = event.dyn_ref::<PointerEvent>() else {
                return;
            };
            let p = local_point(&c, e);
            let mut s = engine.borrow_mut();
            let Some(index) = s.pointers.iter().position(|(id, _)| *id == e.pointer_id()) else {
                return;
            };
            let last = s.pointers[index].1;
            s.pointers[index].1 = p;
            match s.pointers.len() {
                1 => {
                    if let Some(start) = s.press {
                        if (p[0] - start[0]).hypot(p[1] - start[1]) > 4.0 {
                            s.dragged = true;
                        }
                    }
                    if s.dragged {
                        s.view.tx += p[0] - last[0];
                        s.view.ty += p[1] - last[1];
                    }
                }
                2 => {
                    let (a, b) = (s.pointers[0].1, s.pointers[1].1);
                    let dist = (a[0] - b[0]).hypot(a[1] - b[1]).max(1.0);
                    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                    if let Some((d0, m0)) = s.pinch {
                        s.view.tx += mid[0] - m0[0];
                        s.view.ty += mid[1] - m0[1];
                        s.view.zoom_at(mid, dist / d0);
                    }
                    s.pinch = Some((dist, mid));
                }
                _ => return,
            }
            drop(s);
            request_frame(&engine);
        }));
    }
    for name in ["pointerup", "pointercancel"] {
        let engine = engine.clone();
        let c = canvas.clone();
        out.push(EventListener::new(canvas, name, move |event| {
            let Some(e) = event.dyn_ref::<PointerEvent>() else {
                return;
            };
            let p = local_point(&c, e);
            let clicked = {
                let mut s = engine.borrow_mut();
                let had = s.pointers.len();
                s.pointers.retain(|(id, _)| *id != e.pointer_id());
                s.pinch = None;
                let click = had == 1 && !s.dragged && event.type_() == "pointerup";
                if s.pointers.is_empty() {
                    s.press = None;
                }
                if click {
                    s.click(p);
                } else if s.pointers.is_empty() {
                    s.save_view();
                }
                click
            };
            if clicked {
                request_frame(&engine);
                notify(&engine);
            }
        }));
    }
    {
        let engine = engine.clone();
        let c = canvas.clone();
        out.push(EventListener::new_with_options(
            canvas,
            "wheel",
            active,
            move |event| {
                let Some(e) = event.dyn_ref::<WheelEvent>() else {
                    return;
                };
                event.prevent_default();
                let unit = match e.delta_mode() {
                    1 => 16.0,
                    2 => 400.0,
                    _ => 1.0,
                };
                let factor = (-e.delta_y() * unit * 0.0015).exp().clamp(0.2, 5.0);
                let p = local_point(&c, e);
                {
                    let mut s = engine.borrow_mut();
                    s.view.zoom_at(p, factor);
                    s.save_view();
                }
                request_frame(&engine);
            },
        ));
    }
    {
        let engine = engine.clone();
        let window = web_sys::window().expect("window");
        out.push(EventListener::new(&window, "resize", move |_| {
            request_frame(&engine);
        }));
    }
    {
        let engine = engine.clone();
        let window = web_sys::window().expect("window");
        out.push(EventListener::new(&window, "keydown", move |event| {
            let Some(e) = event.dyn_ref::<web_sys::KeyboardEvent>() else {
                return;
            };
            if e.key() == "Escape" {
                {
                    let mut s = engine.borrow_mut();
                    s.selection = None;
                    s.highlight = 0;
                }
                request_frame(&engine);
                notify(&engine);
            }
        }));
    }
    out
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

async fn fetch(project: &str) -> Result<PcbViewResponse, String> {
    crate::api::get_json("/api/kicad/pcbview", project).await
}

/// Re-target selection and net highlight onto a reloaded board.
fn carry_over(old: &PcbBoard, new: &PcbBoard, e: &mut Engine) {
    let net_name = old
        .nets
        .get(e.highlight as usize)
        .cloned()
        .unwrap_or_default();
    e.highlight = if net_name.is_empty() {
        0
    } else {
        new.nets.iter().position(|n| *n == net_name).unwrap_or(0) as u32
    };
    e.selection = match e.selection {
        Some(Hit::Footprint(i)) | Some(Hit::Pad(i, _)) => {
            let reference = &old.footprints[i].reference;
            new.footprints
                .iter()
                .position(|f| &f.reference == reference)
                .map(Hit::Footprint)
        }
        _ => None,
    };
}

#[function_component(PcbView)]
pub fn pcb_view(props: &PcbViewProps) -> Html {
    let engine: Shared = use_mut_ref(Engine::new);
    let canvas_ref = use_node_ref();
    let status = use_state(|| "Loading board…".to_string());
    let tick = use_state(|| 0u32);
    {
        let tick = tick.clone();
        engine.borrow_mut().notify = Some(Callback::from(move |_| tick.set(tick.wrapping_add(1))));
    }

    // Canvas + input wiring.
    {
        let engine = engine.clone();
        let canvas_ref = canvas_ref.clone();
        use_effect_with((), move |_| {
            let canvas = canvas_ref.cast::<HtmlCanvasElement>();
            let listeners = canvas
                .as_ref()
                .map(|c| install_input(&engine, c))
                .unwrap_or_default();
            engine.borrow_mut().canvas = canvas;
            request_frame(&engine);
            move || drop(listeners)
        });
    }
    // Polygon pours toggle.
    {
        let engine = engine.clone();
        use_effect_with(props.pours, move |pours| {
            engine.borrow_mut().pours = *pours;
            request_frame(&engine);
        });
    }
    // Board loading: project switch or revision change.
    {
        let engine = engine.clone();
        let status = status.clone();
        use_effect_with(
            (props.project.clone(), props.revision.clone()),
            move |(project, _)| {
                let project = project.to_string();
                spawn_local(async move {
                    match fetch(&project).await {
                        Ok(response) => {
                            {
                                let mut e = engine.borrow_mut();
                                let switched = e.project != project;
                                if !switched && e.revision == response.revision {
                                    return;
                                }
                                let board = Rc::new(response.board);
                                if switched {
                                    match load_view(&project) {
                                        Some(view) => {
                                            e.view = view;
                                            e.flipped = view.flipped;
                                            e.fitted = true;
                                        }
                                        None => e.fitted = false,
                                    }
                                    e.selection = None;
                                    e.highlight = 0;
                                } else if let Some(old) = e.board.clone() {
                                    carry_over(&old, &board, &mut e);
                                }
                                e.project = project;
                                e.revision = response.revision;
                                e.board = Some(board);
                                e.scene = None;
                            }
                            status.set(String::new());
                            request_frame(&engine);
                        }
                        Err(err) => status.set(format!("PCB view failed: {err}")),
                    }
                    notify(&engine);
                });
            },
        );
    }

    let e = engine.borrow();
    let board = e.board.clone();
    let action = |f: fn(&mut Engine)| {
        let engine = engine.clone();
        Callback::from(move |_: MouseEvent| {
            f(&mut engine.borrow_mut());
            engine.borrow().save();
            request_frame(&engine);
            notify(&engine);
        })
    };
    let zoom = |factor: f64| {
        let engine = engine.clone();
        Callback::from(move |_: MouseEvent| {
            {
                let mut s = engine.borrow_mut();
                let (w, h) = s.css_size();
                s.view.zoom_at([w / 2.0, h / 2.0], factor);
                s.save_view();
            }
            request_frame(&engine);
        })
    };
    let toolbar = html! {
        <div class="pcbv-toolbar" role="toolbar" aria-label="PCB view">
            <button type="button" aria-label="Zoom out" onclick={zoom(1.0 / 1.25)}>{"−"}</button>
            <button type="button" aria-label="Fit board" onclick={action(|s| s.fit())}>{"Fit"}</button>
            <button type="button" aria-label="Zoom in" onclick={zoom(1.25)}>{"+"}</button>
            <button type="button" class={classes!(e.flipped.then_some("on"))} aria-pressed={e.flipped.to_string()}
                title="View from the back (mirrored)"
                onclick={action(|s| { let (w, h) = s.css_size(); s.view.flip_at([w / 2.0, h / 2.0]); s.flipped = s.view.flipped; })}>
                {if e.flipped { "Back" } else { "Front" }}</button>
            <button type="button" class={classes!(e.layers_open.then_some("on"))} aria-pressed={e.layers_open.to_string()}
                onclick={action(|s| s.layers_open = !s.layers_open)}>{"Layers"}</button>
        </div>
    };
    let layers_panel = match (&board, e.layers_open) {
        (Some(board), true) => {
            let names: Vec<String> = board.layers.iter().map(|l| l.name.clone()).collect();
            let preset = |label: &'static str, keep: fn(&str) -> bool| {
                let engine = engine.clone();
                let names = names.clone();
                html! {<button type="button" onclick={Callback::from(move |_: MouseEvent| {
                    {
                        let mut s = engine.borrow_mut();
                        s.hidden = names.iter().filter(|n| !keep(n)).cloned().collect();
                        s.shown.clear();
                        s.save();
                    }
                    request_frame(&engine);
                    notify(&engine);
                })}>{label}</button>}
            };
            let rows = ui_order(&names).into_iter().map(|name| {
                let layer = board.layers.iter().find(|l| l.name == name).cloned();
                let visible = layer.as_ref().is_some_and(|l| e.name_visible(l));
                let color = e.theme.layer(&name).over(e.theme.background).css();
                let engine = engine.clone();
                let toggle_name = name.clone();
                let onchange = Callback::from(move |_: Event| {
                    {
                        let mut s = engine.borrow_mut();
                        s.toggle_layer(&toggle_name);
                        s.save();
                    }
                    request_frame(&engine);
                    notify(&engine);
                });
                let alias = layer
                    .map(|l| l.alias)
                    .filter(|a| !a.is_empty() && *a != name);
                html! {
                    <label class="pcbv-layer" title={alias.clone().unwrap_or_default()}>
                        <input type="checkbox" checked={visible} {onchange}/>
                        <span class="pcbv-swatch" style={format!("background:{color}")}/>
                        <span>{name.clone()}</span>
                    </label>
                }
            });
            let text_toggle = |label: &'static str, bit: u8| {
                let engine = engine.clone();
                let onchange = Callback::from(move |_: Event| {
                    {
                        let mut s = engine.borrow_mut();
                        s.text ^= bit;
                        s.save();
                    }
                    request_frame(&engine);
                    notify(&engine);
                });
                html! {
                    <label class="pcbv-layer">
                        <input type="checkbox" checked={e.text & bit != 0} {onchange}/>
                        <span>{label}</span>
                    </label>
                }
            };
            html! {
                <div class="pcbv-layers" aria-label="Layers">
                    <div class="pcbv-texts">
                        {text_toggle("References", TEXT_REFERENCES)}
                        {text_toggle("Values", TEXT_VALUES)}
                        {text_toggle("Fab / drawing text", TEXT_USER)}
                    </div>
                    <div class="pcbv-presets">
                        {preset("All", |_| true)}
                        {preset("Front", |n| n.starts_with("F.") || !n.contains('.') || n.starts_with("Edge") || n.starts_with("User") || n.ends_with(".User"))}
                        {preset("Back", |n| n.starts_with("B.") || !n.contains('.') || n.starts_with("Edge") || n.starts_with("User") || n.ends_with(".User"))}
                        {preset("Copper", |n| n.ends_with(".Cu") || n == "Edge.Cuts")}
                    </div>
                    {for rows}
                </div>
            }
        }
        _ => Html::default(),
    };
    let properties = match (&board, e.selection) {
        (Some(board), Some(hit)) => {
            let (title, rows) = describe(board, hit);
            let net = (e.highlight != 0).then(|| board.nets[e.highlight as usize].clone());
            let close = {
                let engine = engine.clone();
                Callback::from(move |_: MouseEvent| {
                    {
                        let mut s = engine.borrow_mut();
                        s.selection = None;
                        s.highlight = 0;
                    }
                    request_frame(&engine);
                    notify(&engine);
                })
            };
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
        }
        _ => Html::default(),
    };
    let message = (*status).clone();
    drop(e);
    html! {
        <div class="pcbv">
            <canvas ref={canvas_ref} class="pcbv-canvas" data-revision={engine.borrow().revision.clone()}/>
            {toolbar}
            {layers_panel}
            {properties}
            {if message.is_empty() { Html::default() } else { html!{<div class="pcbv-status" role="status">{message}</div>} }}
        </div>
    }
}

fn mm(v: f64) -> String {
    let s = format!("{v:.3}");
    format!("{} mm", s.trim_end_matches('0').trim_end_matches('.'))
}

fn layer_names(board: &PcbBoard, layers: &[u16]) -> String {
    layers
        .iter()
        .filter_map(|l| board.layers.get(*l as usize))
        .map(|l| l.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn net_name(board: &PcbBoard, net: u32) -> String {
    match board.nets.get(net as usize) {
        Some(n) if !n.is_empty() => n.clone(),
        _ => "(no net)".into(),
    }
}

/// Properties panel title and rows for a selection.
fn describe(board: &PcbBoard, hit: Hit) -> (String, Vec<(String, String)>) {
    let row = |k: &str, v: String| (k.to_string(), v);
    let footprint_rows = |i: usize| {
        let fp = &board.footprints[i];
        let mut rows = vec![
            row("Reference", fp.reference.clone()),
            row("Value", fp.value.clone()),
            row("Footprint", fp.footprint.clone()),
            row("Layer", layer_names(board, &[fp.layer])),
        ];
        rows.push(row(
            "Position",
            format!("{:.3}, {:.3} mm, {}°", fp.pos[0], fp.pos[1], fp.angle),
        ));
        if !fp.attr.is_empty() {
            rows.push(row("Type", fp.attr.clone()));
        }
        if fp.dnp {
            rows.push(row("DNP", "yes".into()));
        }
        rows
    };
    match hit {
        Hit::Footprint(i) => {
            let fp = &board.footprints[i];
            let mut rows = footprint_rows(i);
            if !fp.description.is_empty() {
                rows.push(row("Description", fp.description.clone()));
            }
            for (k, v) in fp.properties.iter().filter(|(_, v)| !v.is_empty()) {
                rows.push((k.clone(), v.clone()));
            }
            (format!("{} · {}", fp.reference, fp.value), rows)
        }
        Hit::Pad(fi, pi) => {
            let fp = &board.footprints[fi];
            let pad = &fp.pads[pi];
            let mut rows = vec![
                row("Net", net_name(board, pad.net)),
                row("Pad", pad.number.clone()),
                row("Pad type", format!("{} {}", pad.kind, pad.shape)),
                row("Size", format!("{} × {}", mm(pad.size[0]), mm(pad.size[1]))),
            ];
            if let Some(d) = &pad.drill {
                rows.push(row(
                    "Drill",
                    if d.oval {
                        format!("{} × {}", mm(d.size[0]), mm(d.size[1]))
                    } else {
                        mm(d.size[0])
                    },
                ));
            }
            if !pad.pin_function.is_empty() {
                rows.push(row("Pin function", pad.pin_function.clone()));
            }
            rows.push(row("Pad layers", layer_names(board, &pad.layers)));
            rows.extend(footprint_rows(fi));
            (format!("{} pad {}", fp.reference, pad.number), rows)
        }
        Hit::Track(i) => {
            let t = &board.tracks[i];
            let (kind, length) = match &t.shape {
                PcbShape::Segment { a, b } => ("Track", (b[0] - a[0]).hypot(b[1] - a[1])),
                PcbShape::Arc { r, start, end, .. } => ("Arc track", r * (end - start).abs()),
                _ => ("Track", 0.0),
            };
            (
                kind.to_string(),
                vec![
                    row("Net", net_name(board, t.net)),
                    row("Layer", layer_names(board, &[t.layer])),
                    row("Width", mm(t.width)),
                    row("Length", mm(length)),
                ],
            )
        }
        Hit::Via(i) => {
            let v = &board.vias[i];
            (
                format!("Via ({})", v.kind),
                vec![
                    row("Net", net_name(board, v.net)),
                    row("Size", mm(v.size)),
                    row("Drill", mm(v.drill)),
                    row("Layers", layer_names(board, &v.layers)),
                ],
            )
        }
        Hit::Zone(i) => {
            let z = &board.zones[i];
            let mut rows = vec![
                row("Net", net_name(board, z.net)),
                row("Layers", layer_names(board, &z.layers)),
                row("Priority", z.priority.to_string()),
            ];
            if !z.name.is_empty() {
                rows.insert(0, row("Name", z.name.clone()));
            }
            ("Zone".to_string(), rows)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mm;

    #[test]
    fn mm_formatting() {
        assert_eq!(mm(0.25), "0.25 mm");
        assert_eq!(mm(1.0), "1 mm");
        assert_eq!(mm(0.125), "0.125 mm");
    }
}
