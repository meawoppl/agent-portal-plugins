//! Rust/WASM lifecycle and UI for the vendored Three.js model renderer.
//!
//! Three.js stays JavaScript; the application-specific loading, camera,
//! lifecycle, resize and error handling are native Rust.

use std::{cell::RefCell, rc::Rc};

use js_sys::{Function, Object, Reflect};
use wasm_bindgen::{closure::Closure, prelude::*, JsCast};
use web_sys::HtmlCanvasElement;
use yew::prelude::*;

type AnimationSlot = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;
type LoadSlot = Rc<RefCell<Option<Closure<dyn FnMut(JsValue)>>>>;

#[wasm_bindgen(raw_module = "three")]
extern "C" {
    #[wasm_bindgen(js_name = Scene)]
    type Scene;
    #[wasm_bindgen(constructor, js_class = Scene)]
    fn new_scene() -> Scene;
    #[wasm_bindgen(method)]
    fn add(this: &Scene, object: &JsValue);

    #[wasm_bindgen(js_name = PerspectiveCamera)]
    type Camera;
    #[wasm_bindgen(constructor, js_class = PerspectiveCamera)]
    fn new_camera(fov: f64, aspect: f64, near: f64, far: f64) -> Camera;
    #[wasm_bindgen(method, js_name = updateProjectionMatrix)]
    fn update_projection(this: &Camera);

    #[wasm_bindgen(js_name = WebGLRenderer)]
    type Renderer;
    #[wasm_bindgen(constructor, js_class = WebGLRenderer)]
    fn new_renderer(options: &JsValue) -> Renderer;
    #[wasm_bindgen(method, js_name = setPixelRatio)]
    fn set_pixel_ratio(this: &Renderer, ratio: f64);
    #[wasm_bindgen(method, js_name = setSize)]
    fn set_size(this: &Renderer, width: f64, height: f64, update_style: bool);
    #[wasm_bindgen(method)]
    fn render(this: &Renderer, scene: &Scene, camera: &Camera);
    #[wasm_bindgen(method)]
    fn dispose(this: &Renderer);

    #[wasm_bindgen(js_name = AmbientLight)]
    type AmbientLight;
    #[wasm_bindgen(constructor, js_class = AmbientLight)]
    fn new_ambient(color: u32, intensity: f64) -> AmbientLight;
    #[wasm_bindgen(js_name = DirectionalLight)]
    type DirectionalLight;
    #[wasm_bindgen(constructor, js_class = DirectionalLight)]
    fn new_directional(color: u32, intensity: f64) -> DirectionalLight;

    #[wasm_bindgen(js_name = Box3)]
    type Box3;
    #[wasm_bindgen(constructor, js_class = Box3)]
    fn new_box() -> Box3;
    #[wasm_bindgen(method, js_name = setFromObject)]
    fn set_from_object(this: &Box3, object: &JsValue) -> Box3;
    #[wasm_bindgen(method, js_name = getSize)]
    fn get_size(this: &Box3, target: &Vec3) -> Vec3;
    #[wasm_bindgen(method, js_name = getCenter)]
    fn get_center(this: &Box3, target: &Vec3) -> Vec3;

    #[wasm_bindgen(js_name = Vector3)]
    type Vec3;
    #[wasm_bindgen(constructor, js_class = Vector3)]
    fn new_vec3(x: f64, y: f64, z: f64) -> Vec3;
    #[wasm_bindgen(method)]
    fn set(this: &Vec3, x: f64, y: f64, z: f64) -> Vec3;
    #[wasm_bindgen(method, getter)]
    fn x(this: &Vec3) -> f64;
    #[wasm_bindgen(method, getter)]
    fn y(this: &Vec3) -> f64;
    #[wasm_bindgen(method, getter)]
    fn z(this: &Vec3) -> f64;
}

#[wasm_bindgen(raw_module = "three/addons/loaders/GLTFLoader.js")]
extern "C" {
    #[wasm_bindgen(js_name = GLTFLoader)]
    type GltfLoader;
    #[wasm_bindgen(constructor, js_class = GLTFLoader)]
    fn new_loader() -> GltfLoader;
}

#[wasm_bindgen(raw_module = "three/addons/controls/OrbitControls.js")]
extern "C" {
    #[wasm_bindgen(js_name = OrbitControls)]
    type OrbitControls;
    #[wasm_bindgen(constructor, js_class = OrbitControls)]
    fn new_controls(camera: &Camera, canvas: &HtmlCanvasElement) -> OrbitControls;
    #[wasm_bindgen(method)]
    fn update(this: &OrbitControls);
    #[wasm_bindgen(method)]
    fn dispose(this: &OrbitControls);
}

/// What the board view can change after the GLB has loaded, without a
/// rebuild: all of it is material and visibility state on the loaded meshes.
#[derive(Clone, PartialEq, Debug)]
pub struct ViewOptions {
    /// Pad finish: `board` (as KiCad exported it) or one of
    /// [`finish_material`]'s names.
    pub finish: String,
    /// Translucent board body and soldermask, so inner layers show through.
    pub xray: bool,
    /// Inner copper layers visible at all.
    pub inner: bool,
    /// Inner copper drawn as wireframe outlines instead of solid.
    pub outline: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            finish: "board".into(),
            xray: false,
            inner: true,
            outline: false,
        }
    }
}

/// `(r, g, b, metalness, roughness)` for a pad finish, in Three's working
/// (linear) colour space. `None` keeps whatever KiCad exported, which it
/// derives from the board stackup's own finish setting.
pub fn finish_material(finish: &str) -> Option<(f64, f64, f64, f64, f64)> {
    match finish {
        "enig" => Some((0.85, 0.62, 0.15, 0.45, 0.35)),
        "hasl" => Some((0.62, 0.64, 0.66, 0.45, 0.45)),
        "osp" => Some((0.70, 0.30, 0.10, 0.45, 0.40)),
        "immersion-silver" => Some((0.85, 0.85, 0.82, 0.45, 0.30)),
        "hard-gold" => Some((0.95, 0.70, 0.18, 0.45, 0.25)),
        _ => None,
    }
}

/// The finishes the picker offers, as `(value, label)`.
pub const FINISHES: &[(&str, &str)] = &[
    ("board", "Board setting"),
    ("enig", "ENIG (gold)"),
    ("hasl", "HASL (tin)"),
    ("osp", "OSP (bare copper)"),
    ("immersion-silver", "Immersion silver"),
    ("hard-gold", "Hard gold"),
];

/// The board parts KiCad's GLB export names its meshes after.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BoardPart {
    Copper,
    Pad,
    Via,
    Silkscreen,
    Soldermask,
    Body,
}

/// KiCad names each board mesh `<board>_<part>` (`controller_pad`), and
/// Three.js may append `_<n>` to keep names unique; footprint models keep
/// their own names and fall through as `None`.
pub fn board_part(name: &str) -> Option<BoardPart> {
    let base = name
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches('_');
    let (_, suffix) = base.rsplit_once('_')?;
    Some(match suffix {
        "copper" => BoardPart::Copper,
        "pad" => BoardPart::Pad,
        "via" => BoardPart::Via,
        "silkscreen" => BoardPart::Silkscreen,
        "soldermask" => BoardPart::Soldermask,
        "PCB" => BoardPart::Body,
        _ => return None,
    })
}

/// Whether copper spanning `[min_y, max_y]` lies strictly inside a board whose
/// faces are at `bottom` and `top`: an inner layer. Outer copper and
/// through-vias touch or cross a face.
pub fn is_inner_copper(min_y: f64, max_y: f64, bottom: f64, top: f64) -> bool {
    let margin = (top - bottom).abs() * 0.01;
    min_y > bottom + margin && max_y < top - margin
}

/// The loaded board's meshes sorted by what the options touch. Materials are
/// shared between the primitives KiCad exports, so inner copper gets its own
/// clones: a wireframe toggle on the shared copper material would outline the
/// outer layers too.
/// A pad material with the colour, metalness and roughness KiCad exported.
struct PadMaterial {
    material: JsValue,
    rgb: (f64, f64, f64),
    metalness: f64,
    roughness: f64,
}

struct Classified {
    /// Pad materials with their exported look.
    pads: Vec<PadMaterial>,
    /// Board body materials with their exported opacity.
    body: Vec<(JsValue, f64)>,
    /// Soldermask materials with their exported opacity.
    mask: Vec<(JsValue, f64)>,
    /// Outer copper, pad, via and silkscreen materials with their exported
    /// opacity: what has to thin out for the inner layers to show through.
    outer: Vec<(JsValue, f64)>,
    /// Inner-layer copper, pad and via meshes.
    inner: Vec<JsValue>,
    /// The cloned materials those inner meshes now use.
    inner_materials: Vec<JsValue>,
}

type ClassifiedSlot = Rc<RefCell<Option<Classified>>>;

fn push_unique(list: &mut Vec<JsValue>, value: &JsValue) -> bool {
    if list.iter().any(|known| known == value) {
        return false;
    }
    list.push(value.clone());
    true
}

fn get_f64(object: &JsValue, name: &str) -> Option<f64> {
    Reflect::get(object, &name.into()).ok()?.as_f64()
}

fn get_str(object: &JsValue, name: &str) -> Option<String> {
    Reflect::get(object, &name.into()).ok()?.as_string()
}

fn set(object: &JsValue, name: &str, value: &JsValue) {
    let _ = Reflect::set(object, &name.into(), value);
}

fn touch(material: &JsValue) {
    set(material, "needsUpdate", &JsValue::TRUE);
}

fn y_extent(object: &JsValue) -> Option<(f64, f64)> {
    let bounds = Box3::new_box().set_from_object(object);
    let min = Reflect::get(bounds.as_ref(), &"min".into()).ok()?;
    let max = Reflect::get(bounds.as_ref(), &"max".into()).ok()?;
    Some((get_f64(&min, "y")?, get_f64(&max, "y")?))
}

/// The glTF mesh name behind a loaded object. Three.js names a
/// single-primitive mesh after its glTF *node*, which KiCad leaves as a bare
/// number for some board items, so the loader's object → mesh-index map is
/// consulted first and the object's own name is the fallback.
fn mesh_name(gltf: &JsValue, object: &JsValue) -> Option<String> {
    let parser = Reflect::get(gltf, &"parser".into()).ok()?;
    let associations = Reflect::get(&parser, &"associations".into()).ok()?;
    let get: Function = Reflect::get(&associations, &"get".into())
        .ok()?
        .dyn_into()
        .ok()?;
    let entry = get.call1(&associations, object).ok()?;
    let index = get_f64(&entry, "meshes")?;
    let json = Reflect::get(&parser, &"json".into()).ok()?;
    let meshes = Reflect::get(&json, &"meshes".into()).ok()?;
    let def = Reflect::get_u32(&meshes, index as u32).ok()?;
    get_str(&def, "name")
}

fn classify(gltf: &JsValue, model: &JsValue) -> Classified {
    let found: Rc<RefCell<Vec<(BoardPart, JsValue)>>> = Rc::new(RefCell::new(Vec::new()));
    let collect = found.clone();
    let gltf = gltf.clone();
    let visitor = Closure::<dyn FnMut(JsValue)>::new(move |object: JsValue| {
        let is_mesh = Reflect::get(&object, &"isMesh".into())
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !is_mesh {
            return;
        }
        let name = mesh_name(&gltf, &object)
            .or_else(|| get_str(&object, "name"))
            .unwrap_or_default();
        if let Some(part) = board_part(&name) {
            collect.borrow_mut().push((part, object));
        }
    });
    if let Ok(value) = Reflect::get(model, &"traverse".into()) {
        if let Ok(traverse) = value.dyn_into::<Function>() {
            let _ = traverse.call1(model, visitor.as_ref().unchecked_ref());
        }
    }
    let found = found.borrow();
    // The board body's faces bound the inner layers.
    let (mut bottom, mut top) = (f64::INFINITY, f64::NEG_INFINITY);
    for (part, mesh) in found.iter() {
        if *part == BoardPart::Body {
            if let Some((lo, hi)) = y_extent(mesh) {
                bottom = bottom.min(lo);
                top = top.max(hi);
            }
        }
    }
    let mut out = Classified {
        pads: Vec::new(),
        body: Vec::new(),
        mask: Vec::new(),
        outer: Vec::new(),
        inner: Vec::new(),
        inner_materials: Vec::new(),
    };
    let remember_outer = |material: JsValue, out: &mut Classified| {
        if !out.outer.iter().any(|(known, _)| known == &material) {
            let opacity = get_f64(&material, "opacity").unwrap_or(1.0);
            out.outer.push((material, opacity));
        }
    };
    let mut clones: Vec<(JsValue, JsValue)> = Vec::new();
    let mut pad_materials: Vec<JsValue> = Vec::new();
    for (part, mesh) in found.iter() {
        match part {
            BoardPart::Body | BoardPart::Soldermask => {
                for material in materials_of(mesh) {
                    let opacity = get_f64(&material, "opacity").unwrap_or(1.0);
                    let list = if *part == BoardPart::Body {
                        &mut out.body
                    } else {
                        &mut out.mask
                    };
                    if !list.iter().any(|(known, _)| known == &material) {
                        list.push((material, opacity));
                    }
                }
            }
            BoardPart::Copper | BoardPart::Pad | BoardPart::Via => {
                let inner = bottom.is_finite()
                    && y_extent(mesh).is_some_and(|(lo, hi)| is_inner_copper(lo, hi, bottom, top));
                if inner {
                    // One clone per exported material keeps inner layers
                    // independent of the outer ones they shared it with.
                    let originals = materials_of(mesh);
                    let mut replaced: Vec<JsValue> = Vec::new();
                    for original in &originals {
                        let clone = match clones.iter().find(|(o, _)| o == original) {
                            Some((_, clone)) => clone.clone(),
                            None => {
                                let clone = Reflect::get(original, &"clone".into())
                                    .ok()
                                    .and_then(|f| f.dyn_into::<Function>().ok())
                                    .and_then(|f| f.call0(original).ok())
                                    .unwrap_or_else(|| original.clone());
                                clones.push((original.clone(), clone.clone()));
                                push_unique(&mut out.inner_materials, &clone);
                                clone
                            }
                        };
                        replaced.push(clone);
                    }
                    if replaced.len() == 1 {
                        set(mesh, "material", &replaced[0]);
                    } else if !replaced.is_empty() {
                        let array = js_sys::Array::new();
                        for m in &replaced {
                            array.push(m);
                        }
                        set(mesh, "material", &array);
                    }
                    out.inner.push(mesh.clone());
                    if *part == BoardPart::Pad {
                        for m in replaced {
                            push_unique(&mut pad_materials, &m);
                        }
                    }
                } else {
                    for m in materials_of(mesh) {
                        if *part == BoardPart::Pad {
                            push_unique(&mut pad_materials, &m);
                        }
                        remember_outer(m, &mut out);
                    }
                }
            }
            BoardPart::Silkscreen => {
                for m in materials_of(mesh) {
                    remember_outer(m, &mut out);
                }
            }
        }
    }
    for material in pad_materials {
        let colour = Reflect::get(&material, &"color".into()).unwrap_or(JsValue::UNDEFINED);
        let rgb = (
            get_f64(&colour, "r").unwrap_or(0.5),
            get_f64(&colour, "g").unwrap_or(0.5),
            get_f64(&colour, "b").unwrap_or(0.5),
        );
        let metalness = get_f64(&material, "metalness").unwrap_or(1.0);
        let roughness = get_f64(&material, "roughness").unwrap_or(0.5);
        out.pads.push(PadMaterial {
            material,
            rgb,
            metalness,
            roughness,
        });
    }
    web_sys::console::debug_1(
        &format!(
            "kicadmium 3d: {} pad, {} body, {} mask, {} outer material(s); {} inner copper mesh(es); board faces y={bottom:.5}..{top:.5}",
            out.pads.len(),
            out.body.len(),
            out.mask.len(),
            out.outer.len(),
            out.inner.len()
        )
        .into(),
    );
    out
}

fn set_colour(material: &JsValue, (r, g, b): (f64, f64, f64)) {
    if let Ok(colour) = Reflect::get(material, &"color".into()) {
        if let Ok(f) =
            Reflect::get(&colour, &"setRGB".into()).and_then(|f| f.dyn_into::<Function>())
        {
            let _ = f.call3(&colour, &r.into(), &g.into(), &b.into());
        }
    }
}

fn apply_options(c: &Classified, o: &ViewOptions) {
    for pad in &c.pads {
        match finish_material(&o.finish) {
            Some((r, g, b, m, rough)) => {
                set_colour(&pad.material, (r, g, b));
                number(&pad.material, "metalness", m);
                number(&pad.material, "roughness", rough);
            }
            None => {
                set_colour(&pad.material, pad.rgb);
                number(&pad.material, "metalness", pad.metalness);
                number(&pad.material, "roughness", pad.roughness);
            }
        }
        touch(&pad.material);
    }
    for (material, exported) in &c.body {
        set(material, "transparent", &JsValue::TRUE);
        number(material, "opacity", if o.xray { 0.18 } else { *exported });
        set(material, "depthWrite", &JsValue::from_bool(!o.xray));
        touch(material);
    }
    for (material, exported) in &c.mask {
        number(material, "opacity", if o.xray { 0.12 } else { *exported });
        touch(material);
    }
    // Outer copper (zones included) covers the inner layers completely, so
    // x-ray has to thin it as well; it goes back to opaque when switched off.
    for (material, exported) in &c.outer {
        set(
            material,
            "transparent",
            &JsValue::from_bool(o.xray || *exported < 1.0),
        );
        number(material, "opacity", if o.xray { 0.3 } else { *exported });
        touch(material);
    }
    for mesh in &c.inner {
        set(mesh, "visible", &JsValue::from_bool(o.inner));
    }
    for material in &c.inner_materials {
        set(material, "wireframe", &JsValue::from_bool(o.outline));
        touch(material);
    }
    web_sys::console::debug_1(
        &format!(
            "kicadmium 3d: applied finish={} xray={} inner={} outline={} to {} pad / {} body / {} mask / {} outer material(s), {} inner mesh(es)",
            o.finish,
            o.xray,
            o.inner,
            o.outline,
            c.pads.len(),
            c.body.len(),
            c.mask.len(),
            c.outer.len(),
            c.inner.len()
        )
        .into(),
    );
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub project: AttrValue,
    pub revision: AttrValue,
    /// Explicit GLB endpoint for library/model previews. The board viewer
    /// derives its revision-pinned endpoint when this is absent.
    #[prop_or_default]
    pub url: Option<AttrValue>,
    #[prop_or(true)]
    pub active: bool,
    /// Finish, see-through and inner-copper settings; library previews leave
    /// the defaults, which change nothing.
    #[prop_or_default]
    pub options: Option<ViewOptions>,
}

struct Runtime {
    renderer: Rc<Renderer>,
    controls: Rc<OrbitControls>,
    animation: AnimationSlot,
    animation_id: Rc<RefCell<Option<i32>>>,
    resize: Closure<dyn FnMut()>,
    loaded: LoadSlot,
    failed: LoadSlot,
    model: Rc<RefCell<Option<JsValue>>>,
    classified: ClassifiedSlot,
    options: Rc<RefCell<ViewOptions>>,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            if let Some(id) = self.animation_id.borrow_mut().take() {
                let _ = window.cancel_animation_frame(id);
            }
            let _ = window.remove_event_listener_with_callback(
                "resize",
                self.resize.as_ref().unchecked_ref(),
            );
        }
        self.animation.borrow_mut().take();
        self.loaded.borrow_mut().take();
        self.failed.borrow_mut().take();
        if let Some(model) = self.model.borrow_mut().take() {
            dispose_model(&model);
        }
        self.controls.dispose();
        self.renderer.dispose();
    }
}

fn call_no_args(object: &JsValue, method: &str) {
    if let Ok(value) = Reflect::get(object, &method.into()) {
        if let Ok(function) = value.dyn_into::<Function>() {
            let _ = function.call0(object);
        }
    }
}

/// Release GPU-backed geometry, material, and texture allocations retained by
/// Three.js. `WebGLRenderer::dispose` alone intentionally does not own these.
fn dispose_model(model: &JsValue) {
    let visitor = Closure::<dyn FnMut(JsValue)>::new(move |object: JsValue| {
        if let Ok(geometry) = Reflect::get(&object, &"geometry".into()) {
            if !geometry.is_undefined() {
                call_no_args(&geometry, "dispose");
            }
        }
        {
            for value in materials_of(&object) {
                // Dispose textures reachable from enumerable material fields.
                for key in Object::keys(value.unchecked_ref::<Object>()) {
                    if let Ok(candidate) = Reflect::get(&value, &key) {
                        if Reflect::get(&candidate, &"isTexture".into())
                            .ok()
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                        {
                            call_no_args(&candidate, "dispose");
                        }
                    }
                }
                call_no_args(&value, "dispose");
            }
        }
    });
    if let Ok(value) = Reflect::get(model, &"traverse".into()) {
        if let Ok(traverse) = value.dyn_into::<Function>() {
            let _ = traverse.call1(model, visitor.as_ref().unchecked_ref());
        }
    }
}

/// Every material on `object`, whether it holds one or an array.
fn materials_of(object: &JsValue) -> Vec<JsValue> {
    let Ok(material) = Reflect::get(object, &"material".into()) else {
        return Vec::new();
    };
    let list = if js_sys::Array::is_array(&material) {
        js_sys::Array::from(&material)
    } else {
        js_sys::Array::of1(&material)
    };
    list.iter()
        .filter(|v| !v.is_null() && !v.is_undefined())
        .collect()
}

/// KiCad exports the soldermask, silkscreen and board body as blended
/// materials, and `GLTFLoader` makes every blended material transparent
/// without depth writes. Three.js then orders those flat faces only by mesh
/// centre: at grazing angles the soldermask's centre lands closer than the
/// silkscreen's, so the mask is painted last and tints the legend away, and
/// the near-opaque body can likewise end up under the far-side mask. They are
/// real surfaces 15–50 µm apart, which the depth buffer resolves now that the
/// clip planes track the camera, so let them write depth and let the depth
/// test decide instead of the sort order.
fn settle_layers(model: &JsValue) {
    let visitor = Closure::<dyn FnMut(JsValue)>::new(move |object: JsValue| {
        for material in materials_of(&object) {
            let transparent = Reflect::get(&material, &"transparent".into())
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if transparent {
                let _ = Reflect::set(&material, &"depthWrite".into(), &JsValue::TRUE);
            }
        }
    });
    if let Ok(value) = Reflect::get(model, &"traverse".into()) {
        if let Ok(traverse) = value.dyn_into::<Function>() {
            let _ = traverse.call1(model, visitor.as_ref().unchecked_ref());
        }
    }
}

/// Extra spacing pushed between the board's flat layer faces, in metres per
/// rank away from the board centre (20 µm: invisible at any zoom).
const LAYER_SPACING: f64 = 2.0e-5;

/// Thinner than this (metres) counts as a flat face rather than a solid.
const FLAT_FACE: f64 = 1.0e-6;

/// KiCad exports the soldermask as a flat face 10–15 µm outside the copper
/// and the silkscreen another 25 µm out. Whether a depth buffer separates
/// surfaces that close depends on the GPU, the zoom and the depth format, and
/// when it cannot the mask sizzles through the copper along every trace.
/// Rather than depend on that, move each flat transparent face outward by
/// `LAYER_SPACING` per rank (mask 20 µm, silkscreen 40 µm, on each side), so
/// the smallest gap the depth test must resolve is 30 µm instead of 10. The
/// shift is far below anything visible.
fn separate_layers(model: &JsValue) {
    let faces: Rc<RefCell<Vec<(JsValue, f64)>>> = Rc::new(RefCell::new(Vec::new()));
    let collect = faces.clone();
    let visitor = Closure::<dyn FnMut(JsValue)>::new(move |object: JsValue| {
        let is_mesh = Reflect::get(&object, &"isMesh".into())
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let transparent = materials_of(&object).iter().any(|material| {
            Reflect::get(material, &"transparent".into())
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        });
        if !is_mesh || !transparent {
            return;
        }
        let bounds = Box3::new_box().set_from_object(&object);
        let origin = Vec3::new_vec3(0.0, 0.0, 0.0);
        if bounds.get_size(&origin).y() < FLAT_FACE {
            let y = bounds.get_center(&origin).y();
            collect.borrow_mut().push((object, y));
        }
    });
    if let Ok(value) = Reflect::get(model, &"traverse".into()) {
        if let Ok(traverse) = value.dyn_into::<Function>() {
            let _ = traverse.call1(model, visitor.as_ref().unchecked_ref());
        }
    }
    let faces = faces.borrow();
    // KiCad writes each mask aperture and silkscreen glyph as its own face, so
    // hundreds of meshes share a level. The distinct levels sit just outside
    // the two board surfaces; the widest gap between neighbouring levels is
    // the board itself, which tells the sides apart without trusting the
    // model's bounding box (tall connectors pull its centre off the board).
    let mut levels: Vec<f64> = Vec::new();
    for (_, y) in faces.iter() {
        if !levels.iter().any(|level| (level - y).abs() < FLAT_FACE) {
            levels.push(*y);
        }
    }
    levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let split = levels
        .windows(2)
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            (a[1] - a[0])
                .partial_cmp(&(b[1] - b[0]))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map_or(0, |(index, _)| index + 1);
    let (bottom, top) = levels.split_at(split);
    let shift_of = |y: f64| -> f64 {
        if let Some(rank) = top.iter().position(|level| (level - y).abs() < FLAT_FACE) {
            LAYER_SPACING * (rank + 1) as f64
        } else if let Some(rank) = bottom
            .iter()
            .rev()
            .position(|level| (level - y).abs() < FLAT_FACE)
        {
            -LAYER_SPACING * (rank + 1) as f64
        } else {
            0.0
        }
    };
    for (mesh, y) in faces.iter() {
        if let Some(position) = property::<Vec3>(mesh, "position") {
            position.set(position.x(), position.y() + shift_of(*y), position.z());
        }
    }
    web_sys::console::debug_1(
        &format!(
            "kicadmium 3d: {} flat layer faces on {} levels above and {} below the board, spaced {} µm per level",
            faces.len(),
            top.len(),
            bottom.len(),
            LAYER_SPACING * 1.0e6
        )
        .into(),
    );
}

fn property<T: JsCast>(object: &JsValue, name: &str) -> Option<T> {
    Reflect::get(object, &name.into()).ok()?.dyn_into().ok()
}

fn number(object: &JsValue, name: &str, value: f64) {
    let _ = Reflect::set(object, &name.into(), &value.into());
}

fn fit_distance(width: f64, height: f64, depth: f64, vertical_fov: f64, aspect: f64) -> f64 {
    // Fit the bounding sphere against whichever viewport dimension has the
    // tighter field of view. Unlike an arbitrary world-unit floor, this works
    // for KiCad GLBs, whose coordinates are metres (a normal PCB spans ~0.1).
    let radius = 0.5 * (width * width + height * height + depth * depth).sqrt();
    let vertical_half = 0.5 * vertical_fov.to_radians();
    let horizontal_half = (vertical_half.tan() * aspect.max(0.01)).atan();
    radius / vertical_half.min(horizontal_half).sin().max(1.0e-6) * 1.12
}

/// Fitted model bounds, in GLB units, kept for per-frame clip planes.
#[derive(Clone, Copy)]
struct Bounds {
    center: (f64, f64, f64),
    radius: f64,
}

type BoundsSlot = Rc<RefCell<Option<Bounds>>>;

/// Depth range for a camera `distance` from the centre of a model bounded by a
/// sphere of `radius`.
///
/// A 24-bit depth buffer resolves roughly `distance² / (near · 2²⁴)` at the
/// model, so a fixed near plane tuned for close zoom (a thousandth of the
/// radius) tears KiCad boards from the fitted view: their copper is 35 µm
/// thick and the exported soldermask is a flat face only 15 µm above it. The
/// planes are therefore recomputed every frame from the camera's actual
/// distance.  Outside the bounding sphere nothing can be nearer than
/// `distance - radius`; inside it the near plane follows the camera.  The far
/// plane likewise hugs the sphere's back, so the whole buffer spans the model.
fn clip_planes(distance: f64, radius: f64) -> (f64, f64) {
    let radius = radius.max(1.0e-9);
    let distance = distance.max(1.0e-9);
    let near = ((distance - radius) * 0.9).max(distance / 1_000.0);
    let far = ((distance + radius) * 1.1).max(near * 10.0);
    (near, far)
}

fn apply_clip_planes(camera: &Camera, bounds: &Bounds) {
    if let Some(position) = property::<Vec3>(camera.as_ref(), "position") {
        let (cx, cy, cz) = bounds.center;
        let (dx, dy, dz) = (position.x() - cx, position.y() - cy, position.z() - cz);
        let (near, far) = clip_planes((dx * dx + dy * dy + dz * dz).sqrt(), bounds.radius);
        number(camera.as_ref(), "near", near);
        number(camera.as_ref(), "far", far);
        camera.update_projection();
    }
}

fn resize(renderer: &Renderer, camera: &Camera, canvas: &HtmlCanvasElement) {
    let width = f64::from(canvas.client_width().max(1));
    let height = f64::from(canvas.client_height().max(1));
    number(camera.as_ref(), "aspect", width / height);
    camera.update_projection();
    renderer.set_size(width, height, false);
}

fn frame(camera: &Camera, controls: &OrbitControls, model: &JsValue) -> Bounds {
    // setFromObject walks only this GLTF scene. Lights and helper objects from
    // our outer scene therefore cannot inflate the fitted bounds.
    let bounds = Box3::new_box().set_from_object(model);
    let size = bounds.get_size(&Vec3::new_vec3(0.0, 0.0, 0.0));
    let center = bounds.get_center(&Vec3::new_vec3(0.0, 0.0, 0.0));
    let radius = 0.5
        * (size.x() * size.x() + size.y() * size.y() + size.z() * size.z())
            .sqrt()
            .max(1.0e-9);
    let aspect = Reflect::get(camera.as_ref(), &"aspect".into())
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);
    let distance = fit_distance(size.x(), size.y(), size.z(), 42.0, aspect);
    // A stable oblique view shows both faces and component height while the
    // bounding-sphere distance guarantees every orientation remains framed.
    let direction = (0.65_f64, 0.4_f64, 0.65_f64);
    let direction_length =
        (direction.0 * direction.0 + direction.1 * direction.1 + direction.2 * direction.2).sqrt();
    if let Some(position) = property::<Vec3>(camera.as_ref(), "position") {
        position.set(
            center.x() + distance * direction.0 / direction_length,
            center.y() + distance * direction.1 / direction_length,
            center.z() + distance * direction.2 / direction_length,
        );
    }
    if let Some(target) = property::<Vec3>(controls.as_ref(), "target") {
        target.set(center.x(), center.y(), center.z());
    }
    let bounds = Bounds {
        center: (center.x(), center.y(), center.z()),
        radius,
    };
    number(controls.as_ref(), "minDistance", radius * 0.05);
    number(controls.as_ref(), "maxDistance", distance * 50.0);
    controls.update();
    apply_clip_planes(camera, &bounds);
    bounds
}

#[cfg(test)]
mod tests {
    use super::{
        board_part, clip_planes, finish_material, fit_distance, is_inner_copper, BoardPart,
    };

    #[test]
    fn board_meshes_are_told_apart_from_footprint_models() {
        assert_eq!(board_part("controller_pad"), Some(BoardPart::Pad));
        assert_eq!(board_part("controller_pad_12"), Some(BoardPart::Pad));
        assert_eq!(board_part("leg-controller_copper"), Some(BoardPart::Copper));
        assert_eq!(board_part("controller_PCB"), Some(BoardPart::Body));
        assert_eq!(
            board_part("controller_soldermask_2"),
            Some(BoardPart::Soldermask)
        );
        assert_eq!(board_part("C_0805_2012Metric"), None);
        assert_eq!(board_part("=>[0:1:1:3]"), None);
        assert_eq!(board_part(""), None);
    }

    #[test]
    fn inner_layers_sit_strictly_between_the_board_faces() {
        // The six-layer leg controller: faces at 0 and 1.51 mm, inner copper
        // at 1.201–1.236 mm, outer copper 1.51–1.545 and -0.035–0, vias
        // spanning the lot.
        let (bottom, top) = (0.0, 1.51e-3);
        assert!(is_inner_copper(1.201e-3, 1.236e-3, bottom, top));
        assert!(is_inner_copper(0.274e-3, 0.309e-3, bottom, top));
        assert!(!is_inner_copper(1.51e-3, 1.545e-3, bottom, top));
        assert!(!is_inner_copper(-0.035e-3, 0.0, bottom, top));
        assert!(!is_inner_copper(-0.035e-3, 1.545e-3, bottom, top));
    }

    #[test]
    fn only_named_finishes_override_the_export() {
        assert!(finish_material("board").is_none());
        assert!(finish_material("").is_none());
        assert!(finish_material("enig").is_some());
        assert!(finish_material("hasl").is_some());
    }

    /// Smallest depth difference a 24-bit buffer resolves at `distance`.
    fn depth_resolution(distance: f64, near: f64) -> f64 {
        distance * distance / (near * f64::from(1u32 << 24))
    }

    #[test]
    fn fits_metre_scale_kicad_board_without_world_unit_floor() {
        let distance = fit_distance(0.1253, 0.00355, 0.14995, 42.0, 16.0 / 9.0);
        assert!(distance > 0.2 && distance < 0.4, "distance={distance}");
    }

    #[test]
    fn portrait_view_needs_more_distance() {
        let landscape = fit_distance(0.1, 0.01, 0.05, 42.0, 16.0 / 9.0);
        let portrait = fit_distance(0.1, 0.01, 0.05, 42.0, 9.0 / 16.0);
        assert!(portrait > landscape);
    }

    #[test]
    fn fitted_view_resolves_copper_above_board() {
        // A 50 × 30 mm KiCad board in metres: copper tops sit 15 µm below the
        // flat soldermask face and 35 µm above the board body.
        let radius = 0.5_f64 * (0.05_f64.powi(2) + 0.03_f64.powi(2) + 0.0016_f64.powi(2)).sqrt();
        let distance = fit_distance(0.05, 0.0016, 0.03, 42.0, 16.0 / 9.0);
        let (near, far) = clip_planes(distance, radius);
        assert!(near < distance - radius && near > 0.0, "near={near}");
        assert!(far > distance + radius, "far={far}");
        let resolution = depth_resolution(distance + radius, near);
        assert!(resolution < 1.0e-6, "resolution={resolution}");
        // The old fixed near plane (radius / 1000) could not tell them apart.
        assert!(depth_resolution(distance, radius / 1_000.0) > 15.0e-6);
    }

    #[test]
    fn clipping_plane_stays_close_when_orbiting_into_the_model() {
        let radius = 0.1;
        // OrbitControls permits a distance of radius * 0.05; the near plane
        // must stay well inside that so zooming in never cuts the board.
        for distance in [radius * 0.05, radius * 0.5, radius, radius * 3.0] {
            let (near, far) = clip_planes(distance, radius);
            assert!(near < distance * 0.95, "near={near} distance={distance}");
            assert!(far > distance + radius, "far={far} distance={distance}");
        }
    }
}

fn start(
    canvas: HtmlCanvasElement,
    url: &str,
    active: bool,
    status: UseStateHandle<String>,
    initial: ViewOptions,
) -> Result<Runtime, JsValue> {
    let options = Object::new();
    Reflect::set(&options, &"canvas".into(), canvas.as_ref())?;
    Reflect::set(&options, &"antialias".into(), &JsValue::TRUE)?;
    // KiCad's GLB stacks real surfaces 10–15 µm apart (copper under a flat
    // soldermask face, both sides), and the far side shows through the
    // near-opaque board body. A linear 24-bit depth buffer resolves roughly
    // distance / 16 000 once the camera is inside the model's bounding
    // sphere, so zoomed-in views let the far side's mask and copper fight.
    // Logarithmic depth keeps the resolution near-uniform (nanometres here)
    // at the cost of early-depth rejection, which this scene does not need.
    Reflect::set(&options, &"logarithmicDepthBuffer".into(), &JsValue::TRUE)?;
    // Annotation snapshots are captured after Three.js has presented the frame.
    // Retain the drawing buffer so `canvas.toDataURL()` does not return black.
    Reflect::set(&options, &"preserveDrawingBuffer".into(), &JsValue::TRUE)?;
    let renderer = Rc::new(Renderer::new_renderer(&options));
    renderer.set_pixel_ratio(web_sys::window().map_or(1.0, |w| w.device_pixel_ratio().min(2.0)));
    let clear: Function =
        Reflect::get(renderer.as_ref().as_ref(), &"setClearColor".into())?.dyn_into()?;
    clear.call2(
        renderer.as_ref().as_ref(),
        &(0x11131d_u32).into(),
        &1.0.into(),
    )?;

    let scene = Rc::new(Scene::new_scene());
    let camera = Rc::new(Camera::new_camera(42.0, 1.0, 0.01, 100_000.0));
    let controls = Rc::new(OrbitControls::new_controls(&camera, &canvas));
    let _ = Reflect::set(
        controls.as_ref().as_ref(),
        &"enableDamping".into(),
        &JsValue::TRUE,
    );
    number(controls.as_ref().as_ref(), "dampingFactor", 0.08);
    let ambient = AmbientLight::new_ambient(0xffffff, 1.8);
    scene.add(ambient.as_ref());
    let key = DirectionalLight::new_directional(0xffffff, 3.2);
    if let Some(position) = property::<Vec3>(key.as_ref(), "position") {
        position.set(1.0, 2.0, 3.0);
    }
    scene.add(key.as_ref());
    resize(&renderer, &camera, &canvas);

    let loaded = Rc::new(RefCell::new(None));
    let failed = Rc::new(RefCell::new(None));
    let model = Rc::new(RefCell::new(None));
    let bounds: BoundsSlot = Rc::new(RefCell::new(None));
    let classified: ClassifiedSlot = Rc::new(RefCell::new(None));
    let options = Rc::new(RefCell::new(initial));
    *loaded.borrow_mut() = Some({
        let scene = scene.clone();
        let camera = camera.clone();
        let controls = controls.clone();
        let model_slot = model.clone();
        let bounds = bounds.clone();
        let status = status.clone();
        let classified = classified.clone();
        let options = options.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |gltf: JsValue| {
            match Reflect::get(&gltf, &"scene".into()) {
                Ok(model) => {
                    settle_layers(&model);
                    separate_layers(&model);
                    let parts = classify(&gltf, &model);
                    apply_options(&parts, &options.borrow());
                    *classified.borrow_mut() = Some(parts);
                    scene.add(&model);
                    *bounds.borrow_mut() = Some(frame(&camera, &controls, &model));
                    *model_slot.borrow_mut() = Some(model);
                    status.set(String::new());
                }
                Err(_) => status.set("The GLB loaded without a scene".into()),
            }
        })
    });
    *failed.borrow_mut() = Some({
        let status = status.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |error: JsValue| {
            status.set(format!(
                "Could not load board model: {}",
                error
                    .as_string()
                    .unwrap_or_else(|| "unknown GLB error".into())
            ));
        })
    });
    let loader = GltfLoader::new_loader();
    let load: Function = Reflect::get(loader.as_ref(), &"load".into())?.dyn_into()?;
    load.call4(
        loader.as_ref(),
        &url.into(),
        loaded.borrow().as_ref().unwrap().as_ref(),
        &JsValue::UNDEFINED,
        failed.borrow().as_ref().unwrap().as_ref(),
    )?;

    let animation: AnimationSlot = Rc::new(RefCell::new(None));
    let animation_id = Rc::new(RefCell::new(None));
    let callback_slot = animation.clone();
    let id_slot = animation_id.clone();
    let loop_renderer = renderer.clone();
    let loop_scene = scene.clone();
    let loop_camera = camera.clone();
    let loop_controls = controls.clone();
    let loop_bounds = bounds.clone();
    *animation.borrow_mut() = Some(Closure::<dyn FnMut(f64)>::new(move |_| {
        if active {
            loop_controls.update();
            if let Some(bounds) = loop_bounds.borrow().as_ref() {
                apply_clip_planes(&loop_camera, bounds);
            }
            loop_renderer.render(&loop_scene, &loop_camera);
        }
        if let (Some(window), Some(callback)) = (web_sys::window(), callback_slot.borrow().as_ref())
        {
            *id_slot.borrow_mut() = window
                .request_animation_frame(callback.as_ref().unchecked_ref())
                .ok();
        }
    }));
    if let (Some(window), Some(callback)) = (web_sys::window(), animation.borrow().as_ref()) {
        *animation_id.borrow_mut() = window
            .request_animation_frame(callback.as_ref().unchecked_ref())
            .ok();
    }

    let rr = renderer.clone();
    let rc = camera.clone();
    let resize_canvas = canvas.clone();
    let resize_callback = Closure::<dyn FnMut()>::new(move || resize(&rr, &rc, &resize_canvas));
    if let Some(window) = web_sys::window() {
        window
            .add_event_listener_with_callback("resize", resize_callback.as_ref().unchecked_ref())?;
    }
    Ok(Runtime {
        renderer,
        controls,
        animation,
        animation_id,
        resize: resize_callback,
        loaded,
        failed,
        model,
        classified,
        options,
    })
}

#[function_component(ModelView)]
pub fn model_view(props: &Props) -> Html {
    let canvas = use_node_ref();
    let status = use_state(|| "Loading board model…".to_owned());
    let runtime = use_mut_ref(|| None::<Runtime>);
    {
        let canvas = canvas.clone();
        let project = props.project.to_string();
        let revision = props.revision.to_string();
        let explicit_url = props.url.as_ref().map(ToString::to_string);
        let active = props.active;
        let status = status.clone();
        let runtime = runtime.clone();
        let initial = props.options.clone().unwrap_or_default();
        use_effect_with(
            (
                project.clone(),
                revision.clone(),
                explicit_url.clone(),
                active,
            ),
            move |_| {
                runtime.borrow_mut().take();
                status.set(if explicit_url.is_some() {
                    "Loading library model…".into()
                } else {
                    "Loading board model…".into()
                });
                if let Some(canvas) = canvas.cast::<HtmlCanvasElement>() {
                    let path = explicit_url.clone().unwrap_or_else(|| {
                        format!("/api/kicad/model.glb?rev={}", crate::api::encode(&revision))
                    });
                    match start(
                        canvas,
                        &crate::api::url(&path, &project),
                        active,
                        status.clone(),
                        initial.clone(),
                    ) {
                        Ok(next) => *runtime.borrow_mut() = Some(next),
                        Err(error) => status.set(format!(
                            "Could not start 3D renderer: {}",
                            error.as_string().unwrap_or_else(|| "unknown error".into())
                        )),
                    }
                }
                move || {
                    runtime.borrow_mut().take();
                }
            },
        );
    }
    // Option changes are material and visibility edits on the loaded meshes;
    // nothing is refetched.
    {
        let runtime = runtime.clone();
        use_effect_with(props.options.clone().unwrap_or_default(), move |options| {
            if let Some(rt) = runtime.borrow().as_ref() {
                *rt.options.borrow_mut() = options.clone();
                if let Some(parts) = rt.classified.borrow().as_ref() {
                    apply_options(parts, options);
                }
            }
        });
    }
    html! { <div class="modelv">
        <canvas ref={canvas} class="modelv-canvas" aria-label="Interactive 3D board model" />
        if !status.is_empty() { <div class="modelv-status">{status.as_str()}</div> }
    </div> }
}
